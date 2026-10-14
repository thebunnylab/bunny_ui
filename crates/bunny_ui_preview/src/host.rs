//! The preview host — the app's own binary in preview mode.
//!
//! `bunny preview` (or an IDE) starts the app with `BUNNY_PREVIEW_PORT`
//! set, and `bunny_ui::app!` brings it here instead of opening a window.
//! The host calls back on that port, says `Hello`, lists the previews the
//! running code exports (`Ready`), and then renders whatever the tool
//! selects: one scene per preview variant, laid out headlessly with the
//! platform's text and image engines, its display list and its sites sent
//! as a [`Frame`](wire::Frame) whenever they change. Input the tool
//! forwards reaches the scene through the runtime's own doors, so a button
//! in a preview counts its taps like one in the app.
//!
//! The host shares the app's one copy of the framework, so a generation
//! `bunny preview` loads after a save swaps the previews' code the way it
//! swaps the app's — and the state a preview made survives, anchored under
//! the host's own boundary. Each new generation sends a new `Ready`.
//!
//! A platform view's box — a webview, a video — rides every frame as a
//! `HostBox` with its kind and its rect, so the tool draws an honest
//! placeholder where the platform would composite its view. A shell
//! that can picture such a view hands the host a [`Pictures`] door (the
//! macOS shell's offscreen desk, `bunny_ui_macos::pictures`, under the
//! facade): the host turns the door each tick, renders the scenes with
//! boxes again when a picture moved, splices each picture into the
//! display list where its host stood (`DisplayList::with_host_pixels`),
//! tells the door which boxes still live, and holds a `RequestSnapshot`
//! while the door is busy — a page loading, a shot in flight — so
//! `bunny preview --png` paints the page. Without a door the box is all
//! there is, and the placeholder is the truth.
//!
//! A panic inside one preview's pass is caught at that scene's boundary:
//! the tool hears `Panicked { preview }`, the scene shows the message
//! until the next generation, and the other scenes keep rendering. A panic
//! anywhere else ends the host with `Panic`, as the hot road ends an app.
//!
//! `BUNNY_PREVIEW_CHECK=<ident,…>` renders those previews once without a
//! tool on the line and prints `@@bunny-preview <ident> commands=<n>
//! sites=<n> ms=<t>` per preview — the door a machine with no screen
//! checks a build through.
//!
//! ## Wiring
//!
//! [`serve`] is what the facade calls; [`Host`] underneath it takes the
//! tool's messages through [`Host::handle`] and renders through
//! [`Host::tick`], with a sink for what it sends — which is how the tests
//! drive it with no socket at all.
//!
//! ## Production gotchas
//!
//! - A preview whose size is `Fits` is laid out under an unspecified
//!   proposal after a settling pass at a generous size: three passes, not
//!   one. A `Fixed` or `Device` size takes one.
//! - Timers in a preview (`task::sleep`) do not advance here: the host
//!   polls tasks per turn but keeps the engine's manual clock, so an
//!   animation driven by a sleep stands still. A frame asked for by state
//!   or a task's result is rendered on the next turn.
//! - Legacy selections retain the thread-wide override table. Instance
//!   selections own versioned override contexts; painting, input and dirty
//!   checks enter only their context and restore the outer table afterward.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use bunny_ui_core::host::HostSpec;
use bunny_ui_core::image_engine::{ImageEngine, ImageSource};
use bunny_ui_core::layout::{Proposal, Size};
use bunny_ui_core::prelude::*;
use bunny_ui_core::preview::{ExecutionMode, ExecutionScope, Traits};
use bunny_ui_core::text_engine::TextEngine;
use bunny_ui_hot::preview::preview_entry;

use bunny_ui_core::layout::{DisplayList, HostPlacement};
use bunny_ui_core::provenance::{self, Override, SiteKey, SitePlacement};

use crate::convert;
use crate::scan;
use crate::wire::{self, Message, Reader, Received, ToHost, ToIde};

mod instances;
use instances::{SceneLane, SceneMode};

/// The text and image engines the scenes render with — the platform's
/// in an app, the house ones in a test.
pub struct Engines {
    pub text: Rc<dyn TextEngine>,
    pub images: Rc<dyn ImageEngine>,
}

impl Engines {
    /// The deterministic house engines: `PixelFont` and raw images.
    pub fn house() -> Engines {
        Engines { text: Rc::new(PixelFont), images: Rc::new(bunny_ui_core::image_engine::RawImages::default()) }
    }
}

/// A hosted box's identity across the host's scenes: the scene it stands
/// in and its path there. The same preview under two variants is two
/// scenes, and its page two pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HostId<'a> {
    /// The scene's name — `p0`, `p1`, … in the order the host opened them.
    pub scene: &'a str,
    /// The box's path in the scene, as the layout names it.
    pub path: &'a str,
}

/// A shell's door to the pictures of its platform views: the page a
/// webview shows, the frame a video is on. The host asks per placement
/// on every render, turns the door once per tick, and tells it which
/// boxes still live. A closure of the placement alone is a door too —
/// one that never moves, is never busy, and keeps nothing.
pub trait Pictures {
    /// The picture of the box `id` at `placement`, if the shell has one
    /// — a shell mounts on the first ask and answers `None` until the
    /// page pictures.
    fn picture(&mut self, id: HostId<'_>, placement: &HostPlacement) -> Option<ImageSource>;

    /// One turn of the shell's own loop: its callbacks landed, its shots
    /// taken. Answers whether a picture moved, so the scenes with boxes
    /// render again.
    fn turn(&mut self) -> bool {
        false
    }

    /// Whether the shell has work in flight — a page loading, a shot
    /// asked — so the host keeps its turns short and holds a snapshot
    /// of the frame for the picture.
    fn busy(&self) -> bool {
        false
    }

    /// The boxes still on a frame; the pages of every other may go.
    fn sweep(&mut self, _alive: &[HostId<'_>]) {}
}

impl<F: FnMut(&HostPlacement) -> Option<ImageSource>> Pictures for F {
    fn picture(&mut self, _id: HostId<'_>, placement: &HostPlacement) -> Option<ImageSource> {
        self(placement)
    }
}

/// How long a snapshot waits for a busy door before it answers with the
/// frame as it stands.
const PATIENCE: Duration = Duration::from_secs(5);

/// What the host says about the app it runs.
#[derive(Clone, Debug)]
pub struct Config {
    /// The app's package name.
    pub package: String,
    /// The app's manifest directory — where `src/` is scanned for previews.
    pub manifest_dir: PathBuf,
    /// The framework's version, as `Hello` announces it.
    pub framework: &'static str,
}

/// How the host runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Call the tool back on this loopback port and serve it.
    Serve { port: u16 },
    /// Render these previews once, print a line each, and end.
    Check { idents: Vec<String> },
}

impl Mode {
    /// The mode the environment asks for: `BUNNY_PREVIEW_PORT`, else
    /// `BUNNY_PREVIEW_CHECK` (idents separated by commas), else none.
    pub fn from_env() -> Option<Mode> {
        if let Some(port) = std::env::var("BUNNY_PREVIEW_PORT").ok().and_then(|port| port.parse::<u16>().ok()) {
            return Some(Mode::Serve { port });
        }
        let idents = std::env::var("BUNNY_PREVIEW_CHECK").ok()?;
        Some(Mode::Check { idents: idents.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect() })
    }
}

/// One scene: a preview variant the tool selected, rendering on a runtime
/// of its own.
struct Scene {
    execution: ExecutionScope,
    id: String,
    mode: SceneMode,
    ident: Rc<str>,
    variant: u32,
    name: Rc<str>,
    runtime: Runtime,
    traits: Traits,
    resized: Option<Size>,
    seq: u64,
    last: Option<Sent>,
    /// A panic inside the pass: the scene shows the message until the
    /// next generation.
    poisoned: Option<String>,
    dirty: bool,
    overrides: Vec<(wire::SiteKey, Override)>,
    /// The paths of the boxes on the last frame — what the door keeps
    /// pages for, and what renders again when a picture moved.
    hosts: Vec<String>,
}

/// Whether an override put on a site stands in for one standing there: a
/// `Replace` or a `Hide` for either — the last gesture wins — and an
/// `Append` for an `Append` of the same modifier (a scrub of an appended
/// padding), while an `Append` of another modifier stacks beside it.
fn supersedes(incoming: &Override, standing: &Override) -> bool {
    match (incoming, standing) {
        (Override::Replace(_) | Override::Hide, Override::Replace(_) | Override::Hide) => true,
        (Override::Append(incoming), Override::Append(standing)) => incoming.name() == standing.name(),
        _ => false,
    }
}

fn as_override(op: wire::OverrideOp, modifier: bunny_ui_core::modifier::Modifier) -> Override {
    match op {
        wire::OverrideOp::Replace => Override::Replace(modifier),
        wire::OverrideOp::Append => Override::Append(modifier),
        wire::OverrideOp::Hide => Override::Hide,
    }
}

/// What a scene last sent — a frame is sent again only when one of these
/// moved.
struct Sent {
    size: Size,
    commands: Vec<wire::Draw>,
    sites: Vec<wire::SiteBox>,
    hosts: Vec<wire::HostBox>,
    strings: Vec<String>,
}

impl Sent {
    fn same(&self, size: Size, commands: &[wire::Draw], sites: &[wire::SiteBox], hosts: &[wire::HostBox], strings: &[String]) -> bool {
        self.size == size && self.commands == commands && self.sites == sites && self.hosts == hosts && self.strings == strings
    }
}

/// What one pass of a scene produced.
struct Pass {
    size: Size,
    display: DisplayList,
    sites: Vec<SitePlacement>,
    hosts: Vec<HostPlacement>,
    body_passes: u32,
}

/// The word a platform view's box carries on the wire.
fn host_kind(spec: &HostSpec) -> &'static str {
    match spec {
        HostSpec::Webview { .. } => "webview",
        HostSpec::Video { .. } => "video",
    }
}

thread_local! {
    /// Set while a scene's pass runs: a panic then is that scene's, not
    /// the host's.
    static IN_PASS: Cell<bool> = const { Cell::new(false) };
    /// The message of the panic the pass caught.
    static PANIC_MESSAGE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Installs the host's panic hook over whatever stands: a panic inside a
/// scene's pass is recorded and swallowed; any other panic goes to the
/// previous hook (the hot road's, which ends the app).
fn guard_passes() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if IN_PASS.with(Cell::get) {
            PANIC_MESSAGE.with(|message| *message.borrow_mut() = Some(info.to_string()));
            return;
        }
        previous(info);
    }));
}

/// The view a scene renders: the preview's entry, resolved inside the
/// pass — so the `State`s the preview makes anchor under this boundary
/// and survive a swap — or the refusal when nothing answers.
#[derive(Clone)]
struct PreviewHost {
    ident: Rc<str>,
}

impl Component for PreviewHost {
    fn body(self) -> impl View {
        // the root may be several nodes; a stack of them is one, which is
        // what an `Either` arm must be
        match preview_entry(&self.ident) {
            Ok(entry) => Either::First(vstack!((entry.make)().root).spacing(0.0)),
            Err(why) => Either::Second(text(why.to_string())),
        }
    }
}

/// The host: the scenes, what was sent, and the sink it sends through.
pub struct Host {
    engines: Engines,
    config: Config,
    scenes: Vec<Scene>,
    next_scene: u32,
    sent_resources: HashSet<u64>,
    generation: u32,
    sink: Box<dyn FnMut(ToIde)>,
    /// A shell's door to the pictures of its platform views, when it
    /// has one.
    pictures: Option<Box<dyn Pictures>>,
    /// How long a snapshot waits for a busy door.
    patience: Duration,
    /// Snapshots asked while the door was busy: each scene id and the
    /// instant it stops waiting.
    held: Vec<(String, Instant)>,
    held_instances: Vec<(wire::SceneInstanceId, u32, Rc<str>, Instant)>,
    /// A scene or a box left since the door was last told what lives.
    resweep: bool,
}

impl Host {
    /// A host that sends through `sink`. Installs the pass guard.
    pub fn new(engines: Engines, config: Config, sink: Box<dyn FnMut(ToIde)>) -> Host {
        guard_passes();
        Host {
            engines,
            config,
            scenes: Vec::new(),
            next_scene: 0,
            sent_resources: HashSet::new(),
            generation: bunny_ui_hot::generation(),
            sink,
            pictures: None,
            patience: PATIENCE,
            held: Vec::new(),
            held_instances: Vec::new(),
            resweep: false,
        }
    }

    /// `Hello`, then the first `Ready`.
    pub fn hello(&mut self) {
        (self.sink)(ToIde::Hello {
            wire: wire::WIRE_VERSION,
            framework: self.config.framework.to_string(),
            pid: std::process::id(),
            generation: self.generation,
            workspace_root: self.config.manifest_dir.to_string_lossy().into_owned(),
            package: self.config.package.clone(),
            caps: wire::caps::PLACE | wire::caps::INSTANCES | wire::caps::SHAPES | wire::caps::MOUNT | wire::caps::EXECUTION,
        });
        self.ready();
    }

    /// Scans the app's sources and resolves each preview in the code that
    /// runs: `Ready` with what answered and what did not.
    pub fn ready(&mut self) {
        // `src/` is what is scanned (never `target/`), and the files are
        // named from the manifest dir, `src/` and all
        let report = scan::scan_dir(&self.config.manifest_dir.join("src"));
        let mut previews = Vec::new();
        let mut missing = Vec::new();
        let found = report.app.iter().map(|site| (wire::APP_IDENT, site.file.as_str(), site.line))
            .chain(report.previews.iter().map(|preview| (preview.ident.as_str(), preview.file.as_str(), preview.line)));
        for (ident, file, line) in found {
            let file = format!("src/{file}");
            match preview_entry(ident) {
                Ok(entry) => {
                    IN_PASS.with(|flag| flag.set(true));
                    let traits = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        let discovery = ExecutionScope::new(ExecutionMode::Design);
                        discovery.with(|| convert::traits(&(entry.make)().traits))
                    }));
                    IN_PASS.with(|flag| flag.set(false));
                    let traits = match traits {
                        Ok(traits) => traits,
                        Err(_) => {
                            let message = PANIC_MESSAGE
                                .with(|message| message.borrow_mut().take())
                                .unwrap_or_else(|| String::from("a panic with no message"));
                            (self.sink)(ToIde::Panicked { preview: ident.to_string(), message });
                            wire::Traits::default()
                        }
                    };
                    previews.push(wire::PreviewInfo {
                        id: format!("{file}#{ident}"),
                        ident: ident.to_string(),
                        name: entry.meta.name.to_string(),
                        scope: entry.meta.scope.to_string(),
                        example: entry.meta.example.to_string(),
                        module: entry.meta.module.to_string(),
                        file,
                        line,
                        traits,
                    });
                }
                Err(_) => missing.push(ident.to_string()),
            }
        }
        (self.sink)(ToIde::Ready { generation: self.generation, previews, missing });
    }

    /// The host with a shell's door to its platform views' pictures:
    /// each spliced into the frame where its host stood, and sent as
    /// the resource it is. Without one the boxes ride the frame alone.
    pub fn with_pictures(mut self, pictures: Box<dyn Pictures>) -> Host {
        self.pictures = Some(pictures);
        self.resweep = true;
        for scene in &mut self.scenes {
            scene.dirty = true;
        }
        self
    }

    /// How long a snapshot asked while the door is busy waits for the
    /// picture before it answers with the frame as it stands — five
    /// seconds unless told otherwise.
    pub fn with_patience(mut self, patience: Duration) -> Host {
        self.patience = patience;
        self
    }

    /// One message from the tool.
    pub fn handle(&mut self, message: ToHost) {
        match self.execution_for(&message) {
            Some(context) => context.with(|| self.handle_scoped(message)),
            None => self.handle_scoped(message),
        }
    }

    fn handle_scoped(&mut self, message: ToHost) {
        match message {
            ToHost::SetExecution { instance, variant, mode } => self.set_execution(instance, variant, mode),
            ToHost::Select(selections) => self.select(selections),
            ToHost::SelectInstances(selections) => self.select_instances(selections),
            ToHost::Instance {
                instance,
                variant,
                command,
            } => self.instance_command(instance, variant, command),
            ToHost::Retrait { id, traits } => {
                let traits = convert::core_traits(&traits);
                let variants = traits.variants();
                for scene in self
                    .scenes
                    .iter_mut()
                    .filter(|scene| scene.instance().is_none() && scene.id == id)
                {
                    if let Some(variant) = variants.get(scene.variant as usize) {
                        variant.apply(&scene.runtime);
                        scene.traits = variant.clone();
                        scene.dirty = true;
                    }
                }
            }
            ToHost::Resize { id, size } => {
                for scene in self
                    .scenes
                    .iter_mut()
                    .filter(|scene| scene.instance().is_none() && scene.id == id)
                {
                    scene.resized = Some(size.into());
                    scene.dirty = true;
                }
            }
            ToHost::Pointer { id, kind, x, y, modifiers } => {
                if let Some(scene) = self.scene(&id) {
                    let moved = match kind {
                        wire::PointerKind::Move => scene.runtime.pointer_moved(x, y, modifiers_of(modifiers)),
                        wire::PointerKind::Down => scene.runtime.pointer_pressed(x, y),
                        wire::PointerKind::Up => scene.runtime.pointer_released(x, y).is_some(),
                        wire::PointerKind::Leave => scene.runtime.pointer_exited(),
                    };
                    scene.dirty |= moved;
                }
            }
            ToHost::Wheel { id, x, y, dx, dy } => {
                if let Some(scene) = self.scene(&id) {
                    scene.dirty |= scene.runtime.wheel(x, y, dx, dy);
                }
            }
            ToHost::Touch { id, phase, touch, x, y, taps } => {
                if let Some(scene) = self.scene(&id) {
                    scene.dirty |= match phase {
                        wire::TouchPhase::Began => scene.runtime.touch_began(touch, x, y, taps),
                        wire::TouchPhase::Moved => scene.runtime.touch_moved(touch, x, y),
                        wire::TouchPhase::Ended => scene.runtime.touch_ended(touch, x, y),
                        wire::TouchPhase::Cancelled => scene.runtime.touch_cancelled(touch),
                    };
                }
            }
            ToHost::Magnify { id, x, y, scale } => {
                if let Some(scene) = self.scene(&id) {
                    scene.dirty |= scene.runtime.magnify(x, y, scale);
                }
            }
            ToHost::Key { id, key, modifiers } => {
                if let Some(scene) = self.scene(&id)
                    && let Some(stroke) = stroke_named(&key, modifiers)
                {
                    scene.dirty |= scene.runtime.key_stroke(stroke).handled;
                }
            }
            ToHost::Text { id, text } => {
                if let Some(scene) = self.scene(&id) {
                    for typed in text.chars() {
                        let stroke = bunny_ui_core::action::Stroke { pattern: KeyPattern::key(Key::Char(typed)), typed: Some(typed) };
                        scene.dirty |= scene.runtime.key_stroke(stroke).handled;
                    }
                }
            }
            ToHost::Override { id, site, op, patch } => {
                self.apply_override(&id, site, as_override(op, convert::modifier(patch)));
            }
            ToHost::Place { id, site, op, patch } => {
                self.apply_override(&id, site, as_override(op, convert::place_modifier(patch)));
            }
            ToHost::ClearOverrides { id } => {
                if let Some(scene) = self.scene(&id) {
                    scene.overrides.clear();
                    scene.dirty = true;
                    self.push_overrides();
                }
            }
            ToHost::RequestSnapshot { id } => {
                if !self
                    .scenes
                    .iter()
                    .any(|scene| scene.instance().is_none() && scene.id == id)
                {
                    (self.sink)(ToIde::Status {
                        text: format!("{id}: not selected, nothing to snapshot"),
                    });
                } else if self.pictures.as_ref().is_some_and(|door| door.busy()) {
                    // a page still loading, a shot in flight: the picture
                    // is worth the wait, and the frame as it stands is
                    // the answer when patience runs out
                    self.held.push((id, Instant::now() + self.patience));
                } else {
                    self.snapshot_scenes(&id);
                }
            }
            ToHost::Forget { keys } => {
                for key in keys {
                    self.sent_resources.remove(&key);
                }
            }
            ToHost::Ping { nonce } => (self.sink)(ToIde::Pong { nonce }),
            ToHost::Bye => {}
        }
    }

    fn scene(&mut self, id: &str) -> Option<&mut Scene> {
        self.scenes
            .iter_mut()
            .find(|scene| scene.instance().is_none() && scene.id == id)
    }

    /// The tool's selection replaces its legacy scenes: one per variant of each
    /// selected preview; a scene already standing for the same id and
    /// variant under the same traits keeps its state.
    fn select(&mut self, selections: Vec<wire::Selection>) {
        self.select_scenes(
            SceneLane::Legacy,
            selections
                .into_iter()
                .map(|selection| (selection, None))
                .collect(),
        );
    }

    fn select_scenes(
        &mut self,
        lane: SceneLane,
        selections: Vec<(wire::Selection, Option<wire::SceneInstanceId>)>,
    ) {
        let mut kept = Vec::new();
        for (selection, instance) in selections {
            let traits = convert::core_traits(&selection.traits);
            for (variant, traits) in traits.variants().into_iter().enumerate() {
                let variant = variant as u32;
                let standing = self.scenes.iter().position(|scene| {
                    scene.instance() == instance.as_ref()
                        && scene.id == selection.id
                        && scene.variant == variant
                        && scene.ident.as_ref() == selection.ident
                });
                if let Some(at) = standing {
                    let mut scene = self.scenes.remove(at);
                    if scene.traits != traits {
                        traits.apply(&scene.runtime);
                        scene.traits = traits;
                        scene.dirty = true;
                    }
                    kept.push(scene);
                } else {
                    kept.push(self.open(
                        &selection.id,
                        &selection.ident,
                        variant,
                        traits,
                        instance.clone(),
                    ));
                }
            }
        }
        kept.extend(self.scenes.drain(..).filter(|scene| !lane.contains(scene)));
        self.scenes = kept;
        self.resweep = true;
        self.push_overrides();
    }

    fn apply_override(&mut self, id: &str, site: wire::SiteKey, incoming: Override) {
        if let Some(scene) = self.scene(id) {
            // A scrub replaces only the same operation at this site. Style
            // and placement appends keep distinct modifier names.
            scene
                .overrides
                .retain(|(standing, value)| !(standing == &site && supersedes(&incoming, value)));
            scene.overrides.push((site, incoming));
            scene.dirty = true;
            self.push_overrides();
        }
    }

    /// Every scene's overrides, on the thread's one table.
    fn push_overrides(&self) {
        provenance::set_overrides(
            self.scenes
                .iter()
                .filter(|scene| scene.instance().is_none())
                .flat_map(|scene| {
                    scene.overrides.iter().map(|(site, value)| {
                        let key = SiteKey {
                            file: site.file.clone(),
                            line: site.line,
                            column: site.column,
                        };
                        (key, value.clone())
                    })
                }),
        );
    }

    fn open(
        &mut self,
        id: &str,
        ident: &str,
        variant: u32,
        traits: Traits,
        instance: Option<wire::SceneInstanceId>,
    ) -> Scene {
        let name: Rc<str> = Rc::from(format!("p{}", self.next_scene));
        self.next_scene += 1;
        let runtime = Runtime::scene(Rc::clone(&name))
            .text_engine(Rc::clone(&self.engines.text))
            .image_engine(Rc::clone(&self.engines.images));
        runtime.record_sites(true);
        traits.apply(&runtime);
        Scene {
            execution: ExecutionScope::new(ExecutionMode::Design),
            mode: instance.map_or(SceneMode::Legacy, |id| SceneMode::Instance {
                id,
                overrides: provenance::OverrideSet::new(std::iter::empty()),
            }),
            id: id.to_string(),
            ident: Rc::from(ident),
            variant,
            name,
            runtime,
            traits,
            resized: None,
            seq: 0,
            last: None,
            poisoned: None,
            dirty: true,
            overrides: Vec::new(),
            hosts: Vec::new(),
        }
    }

    /// Whether any scene wants a frame without being asked.
    pub fn needs_turn(&self) -> bool {
        self.scenes
            .iter()
            .any(|scene| scene.dirty || scene.needs_frame())
            || self.pictures.as_ref().is_some_and(|door| door.busy())
    }

    /// One turn: tasks polled, a new generation announced, every scene
    /// that needs it rendered and sent. Answers whether a frame went out.
    pub fn tick(&mut self) -> bool {
        // the tasks first: the hot road's swap is one of them, and the
        // generation it loads must be read before any scene renders —
        // the frame that carries the new code carries its number too
        let _ = bunny_ui_core::task::poll_ready();
        let generation = bunny_ui_hot::generation();
        if generation != self.generation {
            self.generation = generation;
            self.ready();
            for scene in &mut self.scenes {
                scene.poisoned = None;
                scene.last = None;
                scene.dirty = true;
            }
        }
        // the door's turn next: a picture that landed rides this tick's
        // frames, in every scene with a box
        if let Some(door) = self.pictures.as_mut()
            && door.turn()
        {
            for scene in &mut self.scenes {
                if !scene.hosts.is_empty() {
                    scene.dirty = true;
                }
            }
        }
        let mut sent = false;
        for index in 0..self.scenes.len() {
            if self.scenes[index].poisoned.is_some() {
                continue;
            }
            if !(self.scenes[index].dirty || self.scenes[index].needs_frame()) {
                continue;
            }
            sent |= self.render(index);
        }
        self.answer_held();
        self.answer_held_instances();
        self.sweep_door();
        sent
    }

    /// The snapshots held for the door: answered when it settled, or
    /// when their patience ran out.
    fn answer_held(&mut self) {
        if self.held.is_empty() {
            return;
        }
        let busy = self.pictures.as_ref().is_some_and(|door| door.busy());
        let now = Instant::now();
        let (due, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.held).into_iter().partition(|(_, until)| !busy || *until <= now);
        self.held = waiting;
        for (id, _) in due {
            self.snapshot_scenes(&id);
        }
    }

    /// Tells the door which boxes still live, once a scene or a box left.
    fn sweep_door(&mut self) {
        if !self.resweep {
            return;
        }
        self.resweep = false;
        let Some(door) = self.pictures.as_mut() else { return };
        let alive: Vec<HostId<'_>> = self.scenes.iter().flat_map(|scene| scene.hosts.iter().map(|path| HostId { scene: &scene.name, path })).collect();
        door.sweep(&alive);
    }

    /// Every scene of `id`, snapshot.
    fn snapshot_scenes(&mut self, id: &str) {
        let scenes: Vec<usize> = (0..self.scenes.len())
            .filter(|&index| self.scenes[index].instance().is_none() && self.scenes[index].id == id)
            .collect();
        for index in scenes {
            self.snapshot(index);
        }
    }

    /// The display with the door's pictures spliced where the hosts
    /// stood, so the tool paints the page and not a placeholder — or
    /// the display as it is, without a door or before a page pictures.
    fn pictured(&mut self, index: usize, hosts: &[HostPlacement], display: DisplayList) -> DisplayList {
        let Some(door) = self.pictures.as_mut() else { return display };
        let scene = &self.scenes[index].name;
        let pictures: Vec<(usize, bunny_ui_core::layout::Rect, ImageSource)> = hosts
            .iter()
            .filter_map(|host| door.picture(HostId { scene, path: &host.path }, host).map(|picture| (host.mark, host.frame, picture)))
            .collect();
        if pictures.is_empty() { display } else { display.with_host_pixels(&pictures) }
    }

    /// One pass of scene `index` under its size, inside the panic
    /// boundary: the size it took, its display list, its sites and the
    /// body passes it cost — or nothing, with the scene poisoned, the
    /// tool told, and a fresh runtime standing in for the next generation.
    fn pass(&mut self, index: usize) -> Option<Pass> {
        let scope = self.scenes[index].override_set().cloned();
        let context = self.scenes[index].execution.context();
        context.with(|| match scope {
            Some(scope) => scope.with(|| self.pass_scoped(index)),
            None => self.pass_scoped(index),
        })
    }

    fn pass_scoped(&mut self, index: usize) -> Option<Pass> {
        let scene = &mut self.scenes[index];
        let root = PreviewHost { ident: Rc::clone(&scene.ident) };
        let fixed = scene.resized.or_else(|| scene.traits.frame());
        IN_PASS.with(|flag| flag.set(true));
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let size = match fixed {
                Some(size) => size,
                None => {
                    // settle at a generous size, then ask what the view wants
                    let _ = scene.runtime.display_frame(&root, Size { width: 4096.0, height: 4096.0 });
                    let wanted = scene.runtime.layout(&root, Proposal::unspecified()).size;
                    Size { width: wanted.width.max(1.0), height: wanted.height.max(1.0) }
                }
            };
            let display = scene.runtime.display_frame(&root, size);
            let sites = scene.runtime.sites();
            let hosts = scene.runtime.hosts();
            let stats = bunny_ui_core::stats::take();
            Pass { size, display, sites, hosts, body_passes: stats.body_passes }
        }));
        IN_PASS.with(|flag| flag.set(false));
        match outcome {
            Ok(pass) => Some(pass),
            Err(_) => {
                let message = PANIC_MESSAGE.with(|message| message.borrow_mut().take()).unwrap_or_else(|| String::from("a panic with no message"));
                let ident = scene.ident.to_string();
                scene.poisoned = Some(message.clone());
                // the runtime that panicked mid-pass is not trusted again: a
                // fresh one under the same name starts that scene over
                let fresh = Runtime::scene(Rc::clone(&scene.name))
                    .text_engine(Rc::clone(&self.engines.text))
                    .image_engine(Rc::clone(&self.engines.images));
                fresh.record_sites(true);
                scene.traits.apply(&fresh);
                let mode = scene.execution.context().mode().unwrap_or(ExecutionMode::Design);
                scene.execution = ExecutionScope::new(mode);
                scene.runtime = fresh;
                let event = match scene.instance() {
                    Some(instance) => ToIde::InstancePanicked {
                        instance: instance.clone(),
                        preview: ident,
                        variant: scene.variant,
                        message,
                    },
                    None => ToIde::Panicked {
                        preview: ident,
                        message,
                    },
                };
                (self.sink)(event);
                None
            }
        }
    }

    /// Paints scene `index` through the engines its frames are laid out
    /// with and sends the pixels: `Runtime::frame`'s picture of the same
    /// display list, at the scene's own scale.
    fn snapshot(&mut self, index: usize) {
        if self.scenes[index].poisoned.is_some() {
            return;
        }
        let Some(Pass { size, display, hosts, .. }) = self.pass(index) else { return };
        let display = self.pictured(index, &hosts, display);
        let scene = &self.scenes[index];
        let scale = scene.traits.resolved_scale().round().max(1.0) as usize;
        let width = (size.width * scale as f64).round().max(1.0) as usize;
        let height = (size.height * scale as f64).round().max(1.0) as usize;
        let background = scene.runtime.scene_theme().unwrap_or_else(Theme::light).canvas;
        let bitmap = bunny_ui_core::raster::rasterize_with(&display, width, height, scale, background, &*self.engines.text, &*self.engines.images);
        let event = match scene.instance() {
            Some(instance) => ToIde::InstanceSnapshot {
                instance: instance.clone(),
                preview: scene.ident.to_string(),
                variant: scene.variant,
                size: (width as u32, height as u32),
                rgba: bitmap.to_rgba_bytes(),
            },
            None => ToIde::Snapshot {
                preview: scene.ident.to_string(),
                variant: scene.variant,
                size: (width as u32, height as u32),
                rgba: bitmap.to_rgba_bytes(),
            },
        };
        (self.sink)(event);
    }

    /// Renders scene `index` and sends its frame when it changed.
    fn render(&mut self, index: usize) -> bool {
        let began = Instant::now();
        self.scenes[index].dirty = false;
        let Some(Pass { size, display, sites, hosts, body_passes }) = self.pass(index) else { return false };
        let display = self.pictured(index, &hosts, display);
        let paths: Vec<String> = hosts.iter().map(|host| host.path.clone()).collect();
        let scene = &self.scenes[index];
        let mut strings = wire::Strings::new();
        let (commands, images) = convert::lower(&display, &mut strings);
        let sites: Vec<wire::SiteBox> = sites.iter().map(|placement| convert::site_box(placement, &mut strings)).collect();
        let hosts: Vec<wire::HostBox> = hosts
            .iter()
            .enumerate()
            .map(|(index, host)| wire::HostBox { index: index as u32, kind: strings.intern(host_kind(&host.spec)), frame: host.frame.into() })
            .collect();
        let table = strings.into_table();
        if scene.last.as_ref().is_some_and(|last| last.same(size, &commands, &sites, &hosts, &table)) {
            return false;
        }
        for image in images {
            let key = convert::image_key(&image);
            if self.sent_resources.contains(&key) {
                continue;
            }
            if let Some(resource) = convert::resource(&image) {
                self.sent_resources.insert(key);
                (self.sink)(ToIde::Resource(resource));
            }
        }
        let scene = &mut self.scenes[index];
        scene.seq += 1;
        if scene.hosts != paths {
            scene.hosts = paths;
            self.resweep = true;
        }
        let background = scene.runtime.scene_theme().unwrap_or_else(Theme::light).canvas;
        let frame = wire::Frame {
            preview: scene.ident.to_string(),
            variant: scene.variant,
            seq: scene.seq,
            generation: self.generation,
            size: size.into(),
            scale: scene.traits.resolved_scale(),
            background: background.into(),
            strings: table.clone(),
            commands: commands.clone(),
            sites: sites.clone(),
            hosts: hosts.clone(),
            metrics: wire::Metrics {
                build_ms: 0.0,
                layout_ms: began.elapsed().as_secs_f64() * 1000.0,
                body_passes,
                commands: commands.len() as u32,
            },
        };
        scene.last = Some(Sent { size, commands, sites, hosts, strings: table });
        let event = match scene.instance() {
            Some(instance) => ToIde::InstanceFrame {
                instance: instance.clone(),
                frame: Box::new(frame),
            },
            None => ToIde::Frame(frame),
        };
        (self.sink)(event);
        true
    }

    /// The scenes' ids, in order — what a test reads.
    pub fn scene_ids(&self) -> Vec<(String, u32)> {
        self.scenes.iter().map(|scene| (scene.id.clone(), scene.variant)).collect()
    }
}

fn modifiers_of(bits: u32) -> bunny_ui_core::action::Modifiers {
    bunny_ui_core::action::Modifiers { shift: bits & 1 != 0, command: bits & 2 != 0, option: bits & 4 != 0, control: bits & 8 != 0 }
}

/// A stroke from the wire's key name: a named key, or one character.
fn stroke_named(name: &str, modifiers: u32) -> Option<bunny_ui_core::action::Stroke> {
    let key = match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => Key::Enter,
        "escape" | "esc" => Key::Escape,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        _ => {
            let mut chars = name.chars();
            let first = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            Key::Char(first)
        }
    };
    let held = modifiers_of(modifiers);
    let typed = match key {
        Key::Char(c) if !held.command && !held.control => Some(c),
        _ => None,
    };
    Some(bunny_ui_core::action::Stroke {
        pattern: KeyPattern { key, shift: held.shift, command: held.command, option: held.option, control: held.control },
        typed,
    })
}

/// What the serving loop hears: a message off the wire, a task that woke
/// from another thread, or the line closing.
enum Order {
    Message(ToHost),
    Wake,
    Closed,
}

/// Runs the host in `mode` and never returns: `Check` prints and exits,
/// `Serve` serves until the tool says `Bye` or the line closes.
pub fn serve(mode: Mode, engines: Engines, config: Config) -> ! {
    serve_with(mode, engines, config, None)
}

/// [`serve`], with a shell's door to its platform views' pictures: the
/// serving host turns it each tick and splices what it gives. The check
/// renders once and has no use for one.
pub fn serve_with(mode: Mode, engines: Engines, config: Config, pictures: Option<Box<dyn Pictures>>) -> ! {
    match mode {
        Mode::Check { idents } => check(idents, engines, config),
        Mode::Serve { port } => {
            let code = serve_port(port, engines, config, pictures);
            std::process::exit(code)
        }
    }
}

fn check(idents: Vec<String>, engines: Engines, config: Config) -> ! {
    let seen: Rc<RefCell<Vec<ToIde>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&seen);
    let mut host = Host::new(engines, config, Box::new(move |message| sink.borrow_mut().push(message)));
    let mut ok = true;
    for ident in idents {
        if let Err(why) = preview_entry(&ident) {
            println!("@@bunny-preview {ident} failed {why}");
            ok = false;
            continue;
        }
        host.handle(ToHost::Select(vec![wire::Selection { id: ident.clone(), ident: ident.clone(), traits: wire::Traits::default() }]));
        let began = Instant::now();
        host.tick();
        let ms = began.elapsed().as_secs_f64() * 1000.0;
        let mut found = false;
        for message in seen.borrow_mut().drain(..) {
            match message {
                ToIde::Frame(frame) if frame.preview == ident => {
                    println!("@@bunny-preview {ident} commands={} sites={} ms={ms:.1}", frame.commands.len(), frame.sites.len());
                    found = true;
                }
                ToIde::Panicked { preview, message } => {
                    println!("@@bunny-preview {preview} panicked {}", message.replace('\n', " "));
                    ok = false;
                    found = true;
                }
                _ => {}
            }
        }
        if !found {
            println!("@@bunny-preview {ident} failed no frame");
            ok = false;
        }
    }
    std::process::exit(if ok { 0 } else { 1 })
}

fn serve_port(port: u16, engines: Engines, config: Config, pictures: Option<Box<dyn Pictures>>) -> i32 {
    let Ok(stream) = std::net::TcpStream::connect(("127.0.0.1", port)) else {
        eprintln!("bunny preview: nothing listens on 127.0.0.1:{port}");
        return 2;
    };
    let Ok(reading) = stream.try_clone() else { return 2 };
    let writing = Arc::new(Mutex::new(stream));
    let (sender, receiver) = mpsc::channel::<Order>();
    // the line, read on a thread of its own
    let line = sender.clone();
    std::thread::spawn(move || {
        let mut reading = reading;
        let mut reader = Reader::new();
        let mut buf = [0u8; 64 * 1024];
        loop {
            let read = match std::io::Read::read(&mut reading, &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            reader.feed(&buf[..read]);
            loop {
                match reader.pull() {
                    Ok(Some(Received::Message(Message::ToHost(message)))) => {
                        if line.send(Order::Message(message)).is_err() {
                            return;
                        }
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(_) => {
                        let _ = line.send(Order::Closed);
                        return;
                    }
                }
            }
        }
        let _ = line.send(Order::Closed);
    });
    // a task woken elsewhere asks for a turn
    let wake = Mutex::new(sender);
    motor_wake(Arc::new(move || {
        if let Ok(sender) = wake.lock() {
            let _ = sender.send(Order::Wake);
        }
    }));
    let out = Arc::clone(&writing);
    let mut host = Host::new(
        engines,
        config,
        Box::new(move |message| {
            if let Ok(mut stream) = out.lock() {
                let _ = stream.write_all(&wire::encode(&Message::ToIde(message))).and_then(|()| stream.flush());
            }
        }),
    );
    if let Some(pictures) = pictures {
        host = host.with_pictures(pictures);
    }
    host.hello();
    loop {
        let wait = if host.needs_turn() { Duration::from_millis(16) } else { Duration::from_secs(1) };
        match receiver.recv_timeout(wait) {
            Ok(Order::Message(ToHost::Bye)) | Ok(Order::Closed) | Err(mpsc::RecvTimeoutError::Disconnected) => return 0,
            Ok(Order::Message(message)) => host.handle(message),
            Ok(Order::Wake) | Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        while let Ok(order) = receiver.try_recv() {
            match order {
                Order::Message(ToHost::Bye) | Order::Closed => return 0,
                Order::Message(message) => host.handle(message),
                Order::Wake => {}
            }
        }
        host.tick();
    }
}

fn motor_wake(hook: Arc<dyn Fn() + Send + Sync>) {
    bunny_ui_core::task::set_wake_hook(hook);
}

/// The scenes a host holds, by id and variant, with whether each is
/// poisoned — what a test reads after a panic.
impl Host {
    pub fn poisoned(&self) -> Vec<String> {
        self.scenes.iter().filter(|scene| scene.poisoned.is_some()).map(|scene| scene.id.clone()).collect()
    }

    /// The resources sent so far, by key.
    pub fn sent_resources(&self) -> &HashSet<u64> {
        &self.sent_resources
    }

    /// The overrides a scene holds.
    pub fn overrides_of(&self, id: &str) -> usize {
        self.scenes
            .iter()
            .filter(|scene| scene.instance().is_none() && scene.id == id)
            .map(|scene| scene.overrides.len())
            .sum()
    }

    /// The configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    #[doc(hidden)]
    pub fn scenes_map(&self) -> HashMap<String, u32> {
        self.scenes.iter().map(|scene| (scene.id.clone(), scene.variant)).collect()
    }
}
