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
//! - Overrides arrive and are kept per scene; they take effect once the
//!   core's override road lands (the site-keyed patches of D424).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use bunny_ui_core::image_engine::ImageEngine;
use bunny_ui_core::layout::{Proposal, Size};
use bunny_ui_core::prelude::*;
use bunny_ui_core::preview::Traits;
use bunny_ui_core::text_engine::TextEngine;
use bunny_ui_hot::preview::preview_entry;

use crate::convert;
use crate::scan;
use crate::wire::{self, Message, Reader, Received, ToHost, ToIde};

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
    id: String,
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
    overrides: Vec<(wire::SiteKey, wire::OverrideOp, wire::Patch)>,
}

/// What a scene last sent — a frame is sent again only when one of these
/// moved.
struct Sent {
    size: Size,
    commands: Vec<wire::Draw>,
    sites: Vec<wire::SiteBox>,
    strings: Vec<String>,
}

impl Sent {
    fn same(&self, size: Size, commands: &[wire::Draw], sites: &[wire::SiteBox], strings: &[String]) -> bool {
        self.size == size && self.commands == commands && self.sites == sites && self.strings == strings
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
}

impl Host {
    /// A host that sends through `sink`. Installs the pass guard.
    pub fn new(engines: Engines, config: Config, sink: Box<dyn FnMut(ToIde)>) -> Host {
        guard_passes();
        Host { engines, config, scenes: Vec::new(), next_scene: 0, sent_resources: HashSet::new(), generation: bunny_ui_hot::generation(), sink }
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
            caps: 0,
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
        for found in &report.previews {
            let file = format!("src/{}", found.file);
            match preview_entry(&found.ident) {
                Ok(entry) => {
                    let output = (entry.make)();
                    previews.push(wire::PreviewInfo {
                        id: format!("{file}#{}", found.ident),
                        ident: found.ident.clone(),
                        name: entry.meta.name.to_string(),
                        scope: entry.meta.scope.to_string(),
                        example: entry.meta.example.to_string(),
                        module: entry.meta.module.to_string(),
                        file,
                        line: found.line,
                        traits: convert::traits(&output.traits),
                    });
                }
                Err(_) => missing.push(found.ident.clone()),
            }
        }
        (self.sink)(ToIde::Ready { generation: self.generation, previews, missing });
    }

    /// One message from the tool.
    pub fn handle(&mut self, message: ToHost) {
        match message {
            ToHost::Select(selections) => self.select(selections),
            ToHost::Retrait { id, traits } => {
                let traits = convert::core_traits(&traits);
                let variants = traits.variants();
                for scene in self.scenes.iter_mut().filter(|scene| scene.id == id) {
                    if let Some(variant) = variants.get(scene.variant as usize) {
                        variant.apply(&scene.runtime);
                        scene.traits = variant.clone();
                        scene.dirty = true;
                    }
                }
            }
            ToHost::Resize { id, size } => {
                for scene in self.scenes.iter_mut().filter(|scene| scene.id == id) {
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
                if let Some(scene) = self.scene(&id) {
                    scene.overrides.push((site, op, patch));
                    scene.dirty = true;
                }
            }
            ToHost::ClearOverrides { id } => {
                if let Some(scene) = self.scene(&id) {
                    scene.overrides.clear();
                    scene.dirty = true;
                }
            }
            ToHost::RequestSnapshot { id } => {
                (self.sink)(ToIde::Status { text: format!("{id}: snapshots arrive with a later version of the host") });
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
        self.scenes.iter_mut().find(|scene| scene.id == id)
    }

    /// The tool's selection replaces the scenes: one per variant of each
    /// selected preview; a scene already standing for the same id and
    /// variant under the same traits keeps its state.
    fn select(&mut self, selections: Vec<wire::Selection>) {
        let mut kept = Vec::new();
        for selection in selections {
            let traits = convert::core_traits(&selection.traits);
            for (variant, traits) in traits.variants().into_iter().enumerate() {
                let variant = variant as u32;
                let standing = self
                    .scenes
                    .iter()
                    .position(|scene| scene.id == selection.id && scene.variant == variant && scene.ident.as_ref() == selection.ident);
                if let Some(at) = standing {
                    let mut scene = self.scenes.remove(at);
                    if scene.traits != traits {
                        traits.apply(&scene.runtime);
                        scene.traits = traits;
                        scene.dirty = true;
                    }
                    kept.push(scene);
                } else {
                    kept.push(self.open(&selection.id, &selection.ident, variant, traits));
                }
            }
        }
        self.scenes = kept;
    }

    fn open(&mut self, id: &str, ident: &str, variant: u32, traits: Traits) -> Scene {
        let name: Rc<str> = Rc::from(format!("p{}", self.next_scene));
        self.next_scene += 1;
        let runtime = Runtime::scene(Rc::clone(&name))
            .text_engine(Rc::clone(&self.engines.text))
            .image_engine(Rc::clone(&self.engines.images));
        runtime.record_sites(true);
        traits.apply(&runtime);
        Scene {
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
        }
    }

    /// Whether any scene wants a frame without being asked.
    pub fn needs_turn(&self) -> bool {
        self.scenes.iter().any(|scene| scene.dirty || scene.runtime.needs_frame())
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
                scene.dirty = true;
            }
        }
        let mut sent = false;
        for index in 0..self.scenes.len() {
            if self.scenes[index].poisoned.is_some() {
                continue;
            }
            if !(self.scenes[index].dirty || self.scenes[index].runtime.needs_frame()) {
                continue;
            }
            sent |= self.render(index);
        }
        sent
    }

    /// Renders scene `index` and sends its frame when it changed.
    fn render(&mut self, index: usize) -> bool {
        let began = Instant::now();
        let scene = &mut self.scenes[index];
        scene.dirty = false;
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
            let stats = bunny_ui_core::stats::take();
            (size, display, sites, stats.body_passes)
        }));
        IN_PASS.with(|flag| flag.set(false));
        let (size, display, sites, body_passes) = match outcome {
            Ok(rendered) => rendered,
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
                scene.runtime = fresh;
                (self.sink)(ToIde::Panicked { preview: ident, message });
                return false;
            }
        };
        let mut strings = wire::Strings::new();
        let (commands, images) = convert::lower(&display, &mut strings);
        let sites: Vec<wire::SiteBox> = sites.iter().map(|placement| convert::site_box(placement, &mut strings)).collect();
        let table = strings.into_table();
        if scene.last.as_ref().is_some_and(|last| last.same(size, &commands, &sites, &table)) {
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
            hosts: Vec::new(),
            metrics: wire::Metrics {
                build_ms: 0.0,
                layout_ms: began.elapsed().as_secs_f64() * 1000.0,
                body_passes,
                commands: commands.len() as u32,
            },
        };
        scene.last = Some(Sent { size, commands, sites, strings: table });
        (self.sink)(ToIde::Frame(frame));
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
    match mode {
        Mode::Check { idents } => check(idents, engines, config),
        Mode::Serve { port } => {
            let code = serve_port(port, engines, config);
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

fn serve_port(port: u16, engines: Engines, config: Config) -> i32 {
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
        self.scenes.iter().filter(|scene| scene.id == id).map(|scene| scene.overrides.len()).sum()
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
