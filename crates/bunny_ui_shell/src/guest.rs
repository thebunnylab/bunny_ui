//! The guest: a view and its runtime behind the protocol. What a block
//! compiled for the Trinity shell is, inside its sandbox — bytes in,
//! bytes out, nothing else in reach.
//!
//! One [`Guest::call`] is one turn: whole messages in, then every answer
//! they caused out — the handshake's, and the scene when the view now
//! shows something it did not. A turn that changed nothing answers
//! nothing.
//!
//! ## Production gotchas
//!
//! - **A call carries whole messages.** A message cut at the end of a
//!   call is a [`ToHost::Fault`], never a wait for the next call: the
//!   host hands the guest its bytes in one piece.
//! - **Nothing is acted on before the handshake**, and nothing the host
//!   did not grant afterwards: an event of an ungranted kind is dropped
//!   as if it never came.
//! - **The guest has no clock.** Time is what the host's
//!   [`ToGuest::Tick`]s say it is; a block that animates says so with
//!   [`ToHost::Pace`].
//! - **A request is a question.** [`Page::request`] queues it; the turn
//!   hands it to the host after the scene, and only under
//!   [`Capabilities::REQUESTS`]. A shell that grants none answers every
//!   request [`Outcome::Refused`] inside the guest, so the block can say
//!   so instead of waiting.
//! - **Text is measured with the engine's own face** (`PixelFont`, the
//!   house's headless face), because a sandbox has no font to ask. The
//!   scene names the font it laid out with; a host that paints the text
//!   with another face paints other widths.

use std::collections::HashSet;

use bunny_ui_core::accessibility::{self, Action};
use bunny_ui_core::action::{Key, KeyPattern, Stroke};
use bunny_ui_core::custom;
use bunny_ui_core::layout::Size;
use bunny_ui_core::prelude::{EditCommand, Runtime, State};
use bunny_ui_core::theme::Theme;
use bunny_ui_core::view::View;
use bunny_ui_preview::convert;
use bunny_ui_preview::wire::{self, Frame};

use crate::protocol::{
    self, AccessAction, Capabilities, Context, Filter, Message, Modifiers, Outcome, PointerButton, PointerPhase, Reader,
    Received, Requested, Role, Rows, Scheme, SemanticNode, ToGuest, ToHost, Versions, Viewport,
};

/// What the guest says it is, in its answer to the offer.
pub const ENGINE: &str = concat!("bunny-ui ", env!("CARGO_PKG_VERSION"));

/// What a handshake agreed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Session {
    pub version: u32,
    pub capabilities: Capabilities,
}

/// What a block's view holds of the page it stands on — its signature,
/// from the inside: the page's filters and open record, the rows of the
/// views it reads, and the declared actions it asks the shell to run.
///
/// Every read records itself like a `State`'s, so a view that shows the
/// open record draws again when the shell opens another.
#[derive(Clone, Copy)]
pub struct Page {
    context: State<Context>,
    reads: State<Vec<Rows>>,
    asked: State<Vec<Requested>>,
    answers: State<Vec<(u64, Outcome)>>,
    next: State<u64>,
}

impl Page {
    fn new() -> Page {
        Page {
            context: State::new(Context::default()),
            reads: State::new(Vec::new()),
            asked: State::new(Vec::new()),
            answers: State::new(Vec::new()),
            next: State::new(0),
        }
    }

    /// The page as the shell last said it.
    pub fn context(&self) -> Context {
        self.context.with(Context::clone)
    }

    /// The page's filters.
    pub fn filters(&self) -> Vec<Filter> {
        self.context.with(|context| context.filters.clone())
    }

    /// The record open on the page, on a record page.
    pub fn record(&self) -> Option<String> {
        self.context.with(|context| context.record.clone())
    }

    /// The rows of `view` as the shell last sent them; `None` before the
    /// first, or for a view the block's signature does not read.
    pub fn rows(&self, view: &str) -> Option<Rows> {
        self.reads.with(|reads| reads.iter().find(|rows| rows.view == view).cloned())
    }

    /// Asks the shell to run the declared action `action`, on `record` when
    /// it acts on one, and answers the request's number: what
    /// [`Page::answer`] looks it up by. The shell runs it through its own
    /// gate; nothing is written by asking.
    pub fn request(&self, action: &str, record: Option<&str>) -> u64 {
        let request = self.next.update(|next| {
            *next += 1;
            *next
        });
        self.asked.update(|asked| {
            asked.push(Requested { request, action: action.to_string(), record: record.map(str::to_string) });
        });
        request
    }

    /// How the shell answered `request`, once it has.
    pub fn answer(&self, request: u64) -> Option<Outcome> {
        self.answers.with(|answers| answers.iter().find(|(asked, _)| *asked == request).map(|(_, outcome)| outcome.clone()))
    }

    /// The requests waiting to be handed over, taken.
    fn take_asked(&self) -> Vec<Requested> {
        self.asked.update_if(|asked| (!asked.is_empty(), std::mem::take(asked)))
    }

    fn answered(&self, request: u64, outcome: Outcome) {
        self.answers.update(|answers| answers.push((request, outcome)));
    }

    fn read(&self, rows: Rows) -> bool {
        self.reads.update_if(|reads| match reads.iter_mut().find(|held| held.view == rows.view) {
            Some(held) if *held == rows => (false, false),
            Some(held) => {
                *held = rows;
                (true, true)
            }
            None => {
                reads.push(rows);
                (true, true)
            }
        })
    }
}

/// A view, its runtime, and what the host has already been told.
pub struct Guest<V> {
    runtime: Runtime,
    root: V,
    page: Page,
    session: Option<Session>,
    viewport: Viewport,
    seq: u64,
    /// The image keys the host holds.
    sent: HashSet<u64>,
    last: Option<Painted>,
    semantics: Option<Vec<SemanticNode>>,
    animating: Option<bool>,
    touched: bool,
}

/// The scene the host holds, to tell a new one from it.
#[derive(PartialEq)]
struct Painted {
    size: (f64, f64),
    scale: f64,
    background: wire::Color,
    strings: Vec<String>,
    commands: Vec<wire::Draw>,
}

impl<V: View> Guest<V> {
    /// A guest over `root`, with a runtime of its own.
    pub fn new(root: V) -> Self {
        Guest::with_runtime(Runtime::new(), root)
    }

    /// A guest over the view `make` builds from the page it stands on —
    /// a block with a signature.
    pub fn over(make: impl FnOnce(Page) -> V) -> Self {
        let runtime = Runtime::new();
        let page = Page::new();
        let root = make(page);
        Guest::assemble(runtime, root, page)
    }

    /// A guest over `root` on a runtime the caller built.
    pub fn with_runtime(runtime: Runtime, root: V) -> Self {
        Guest::assemble(runtime, root, Page::new())
    }

    fn assemble(runtime: Runtime, root: V, page: Page) -> Self {
        Guest {
            runtime,
            root,
            page,
            session: None,
            viewport: Viewport { width: 0.0, height: 0.0, scale: 1.0, scheme: Scheme::Light },
            seq: 0,
            sent: HashSet::new(),
            last: None,
            semantics: None,
            animating: None,
            touched: false,
        }
    }

    /// What the handshake agreed, once it has.
    pub fn session(&self) -> Option<Session> {
        self.session
    }

    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    /// One turn: the host's messages in, the guest's answers out.
    pub fn call(&mut self, input: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut reader = Reader::new();
        reader.feed(input);
        let mut read = 0usize;
        loop {
            match reader.pull() {
                Ok(Some(Received::Message(Message::ToGuest(message)))) => {
                    read += 1;
                    self.take(message, &mut out);
                }
                // a host's next kinds, and anything not meant for a guest
                Ok(Some(Received::Message(Message::ToHost(_)) | Received::Unknown { .. })) => read += 1,
                Ok(None) => {
                    if reader.pending() > 0 {
                        say(&mut out, ToHost::Fault { reason: format!("a message was cut at the end of the call, after {read} whole ones") });
                    }
                    break;
                }
                Err(why) => {
                    say(&mut out, ToHost::Fault { reason: why.to_string() });
                    break;
                }
            }
        }
        if self.session.is_some() && (self.touched || self.runtime.needs_frame()) {
            self.touched = false;
            self.present(&mut out);
        }
        self.hand_over(&mut out);
        out
    }

    /// The requests the view made this turn: to the host when it grants
    /// them, else answered as refused here, so the view can say so.
    fn hand_over(&mut self, out: &mut Vec<u8>) {
        let asked = self.page.take_asked();
        if asked.is_empty() {
            return;
        }
        if self.granted(Capabilities::REQUESTS) {
            for requested in asked {
                say(out, ToHost::Request(requested));
            }
        } else {
            for requested in asked {
                self.page.answered(requested.request, Outcome::Refused("this shell grants this block no requests".to_string()));
            }
            // the refusals are the view's to show
            if self.session.is_some() {
                self.present(out);
            }
        }
    }

    fn granted(&self, capability: Capabilities) -> bool {
        self.session.is_some_and(|session| session.capabilities.contains(capability))
    }

    fn take(&mut self, message: ToGuest, out: &mut Vec<u8>) {
        if let ToGuest::Offer { versions, capabilities, viewport } = message {
            self.handshake(versions, capabilities, viewport, out);
            return;
        }
        if self.session.is_none() {
            return;
        }
        let runtime = &self.runtime;
        let moved = match message {
            ToGuest::Offer { .. } => false,
            ToGuest::Resize(viewport) => {
                self.viewport = viewport;
                apply(runtime, viewport);
                true
            }
            ToGuest::Pointer { phase, x, y, button, clicks, modifiers } if self.granted(Capabilities::POINTER) => {
                pointer(runtime, phase, x, y, button, clicks, modifiers)
            }
            ToGuest::Wheel { x, y, dx, dy, modifiers: _ } if self.granted(Capabilities::POINTER) => runtime.wheel(x, y, dx, dy),
            ToGuest::Key { key, typed, modifiers } if self.granted(Capabilities::KEYS) => self.key(&key, &typed, modifiers),
            ToGuest::Text { text } if self.granted(Capabilities::TEXT) => {
                !text.is_empty() && runtime.key(EditCommand::Insert(text)).applied
            }
            ToGuest::Focus { focused } if self.granted(Capabilities::KEYS) => !focused && runtime.blur(),
            ToGuest::Accessibility { node, action } if self.granted(Capabilities::ACCESSIBILITY) => {
                let tree = runtime.accessibility_tree();
                match tree.nodes().iter().find(|exposed| exposed.id.get() == node) {
                    Some(exposed) => runtime.accessibility_action(exposed.id, access_action(action)).is_ok(),
                    None => false,
                }
            }
            ToGuest::Tick { dt_ms } if self.granted(Capabilities::TIME) => {
                let ticked = runtime.tick(dt_ms / 1000.0);
                ticked.scene || ticked.islands
            }
            ToGuest::Context(context) if self.granted(Capabilities::CONTEXT) => self.page.context.set_if_changed(context),
            ToGuest::Rows(rows) if self.granted(Capabilities::READS) => self.page.read(rows),
            ToGuest::Answer { request, outcome } if self.granted(Capabilities::REQUESTS) => {
                self.page.answered(request, outcome);
                true
            }
            // an event of a kind the host did not grant
            ToGuest::Pointer { .. }
            | ToGuest::Wheel { .. }
            | ToGuest::Key { .. }
            | ToGuest::Text { .. }
            | ToGuest::Focus { .. }
            | ToGuest::Accessibility { .. }
            | ToGuest::Tick { .. }
            | ToGuest::Context(_)
            | ToGuest::Rows(_)
            | ToGuest::Answer { .. } => false,
        };
        self.touched |= moved;
    }

    fn handshake(&mut self, versions: Versions, offered: Capabilities, viewport: Viewport, out: &mut Vec<u8>) {
        let Some(version) = Versions::THIS.negotiate(versions) else {
            self.session = None;
            say(
                out,
                ToHost::Refuse {
                    versions: Versions::THIS,
                    reason: format!(
                        "this block speaks the shell protocol {}..={}, the host {}..={}",
                        Versions::THIS.oldest,
                        Versions::THIS.newest,
                        versions.oldest,
                        versions.newest
                    ),
                },
            );
            return;
        };
        let capabilities = offered & Capabilities::KNOWN;
        self.session = Some(Session { version, capabilities });
        // a new handshake starts the host over: it holds nothing yet
        self.sent.clear();
        self.last = None;
        self.semantics = None;
        self.animating = None;
        self.viewport = viewport;
        apply(&self.runtime, viewport);
        self.runtime.set_accessibility_enabled(capabilities.contains(Capabilities::ACCESSIBILITY));
        say(out, ToHost::Accept { version, capabilities, engine: ENGINE.to_string() });
        self.touched = true;
    }

    fn key(&self, key: &str, typed: &str, modifiers: Modifiers) -> bool {
        let runtime = &self.runtime;
        let Some(named) = key_of(key) else { return false };
        let pattern = KeyPattern {
            key: named,
            shift: modifiers.contains(Modifiers::SHIFT),
            command: modifiers.contains(Modifiers::COMMAND),
            option: modifiers.contains(Modifiers::OPTION),
            control: modifiers.contains(Modifiers::CONTROL),
        };
        let mut chars = typed.chars();
        let typed = match (chars.next(), chars.next()) {
            (Some(one), None) => Some(one),
            _ => None,
        };
        let stroke = Stroke::new(pattern, typed);
        // a stroke that types is the text road's (the host sends its
        // `Text`), as on every shell
        if stroke.typed.is_some() {
            return false;
        }
        if runtime.offer_key(stroke) {
            return true;
        }
        let shift = pattern.shift;
        let edit = match named {
            Key::Backspace => EditCommand::Backspace,
            Key::Delete => EditCommand::Delete,
            Key::Left => EditCommand::Left(shift),
            Key::Right => EditCommand::Right(shift),
            Key::Home => EditCommand::Home(shift),
            Key::End => EditCommand::End(shift),
            Key::Char('a') if pattern.command => EditCommand::SelectAll,
            Key::Escape => return runtime.blur(),
            _ => return false,
        };
        runtime.key(edit).applied
    }

    /// The scene, its images and its semantics, where they changed.
    fn present(&mut self, out: &mut Vec<u8>) {
        let viewport = self.viewport;
        let size = Size { width: viewport.width, height: viewport.height };
        let scale = pixel_scale(viewport.scale);
        let display = self.runtime.display_frame(&self.root, size);
        let display = convert::turned_as_pictures(display, &*self.runtime.text(), scale);
        let mut strings = wire::Strings::new();
        let (commands, images) = convert::lower(&display, &mut strings);
        let background = background(&self.runtime).into();
        let painted = Painted {
            size: (viewport.width, viewport.height),
            scale: viewport.scale,
            background,
            strings: strings.into_table(),
            commands,
        };
        if self.last.as_ref() != Some(&painted) {
            for image in &images {
                if let Some(resource) = convert::resource(image)
                    && self.sent.insert(resource.key)
                {
                    say(out, ToHost::Resource(resource));
                }
            }
            self.seq += 1;
            say(
                out,
                ToHost::Scene(Frame {
                    preview: String::new(),
                    variant: 0,
                    seq: self.seq,
                    generation: 0,
                    size: wire::Size { width: viewport.width, height: viewport.height },
                    scale: viewport.scale,
                    background,
                    strings: painted.strings.clone(),
                    commands: painted.commands.clone(),
                    sites: Vec::new(),
                    hosts: Vec::new(),
                    metrics: wire::Metrics {
                        build_ms: 0.0,
                        layout_ms: 0.0,
                        body_passes: 0,
                        commands: painted.commands.len() as u32,
                    },
                }),
            );
            self.last = Some(painted);
        }
        if self.granted(Capabilities::ACCESSIBILITY) {
            let nodes = semantics(&self.runtime.accessibility_tree());
            if self.semantics.as_ref() != Some(&nodes) {
                say(out, ToHost::Semantics { seq: self.seq, nodes: nodes.clone() });
                self.semantics = Some(nodes);
            }
        }
        if self.granted(Capabilities::TIME) {
            let animating = self.runtime.wants_frame();
            if self.animating != Some(animating) {
                say(out, ToHost::Pace { animating });
                self.animating = Some(animating);
            }
        }
    }
}

fn say(out: &mut Vec<u8>, message: ToHost) {
    out.extend(protocol::encode(&Message::ToHost(message)));
}

/// The whole device pixels a point paints at.
pub(crate) fn pixel_scale(scale: f64) -> usize {
    scale.round().max(1.0) as usize
}

/// The viewport's appearance and density, on a runtime.
pub(crate) fn apply(runtime: &Runtime, viewport: Viewport) {
    runtime.set_theme(Some(match viewport.scheme {
        Scheme::Light => Theme::light(),
        Scheme::Dark => Theme::dark(),
    }));
    runtime.set_device_scale(viewport.scale);
}

/// The colour under the scene.
pub(crate) fn background(runtime: &Runtime) -> bunny_ui_core::layout::Color {
    runtime.scene_theme().unwrap_or_else(Theme::light).canvas
}

/// A pointer event, as a native shell hands it to the runtime.
pub(crate) fn pointer(
    runtime: &Runtime,
    phase: PointerPhase,
    x: f64,
    y: f64,
    button: PointerButton,
    clicks: u8,
    modifiers: Modifiers,
) -> bool {
    let held = bunny_ui_core::action::Modifiers {
        shift: modifiers.contains(Modifiers::SHIFT),
        command: modifiers.contains(Modifiers::COMMAND),
        option: modifiers.contains(Modifiers::OPTION),
        control: modifiers.contains(Modifiers::CONTROL),
    };
    let other = match button {
        PointerButton::Primary => None,
        PointerButton::Secondary => Some(custom::PointerButton::Secondary),
        PointerButton::Middle => Some(custom::PointerButton::Middle),
    };
    match (phase, other) {
        (PointerPhase::Move, _) => runtime.pointer_moved(x, y, held),
        (PointerPhase::Down, None) => runtime.pointer_clicked(x, y, clicks.max(1), held),
        (PointerPhase::Down, Some(button)) => runtime.button_pressed(x, y, button, held),
        // the release always clears a pressed look, so it always repaints
        (PointerPhase::Up, None) => {
            let _ = runtime.pointer_released(x, y);
            true
        }
        (PointerPhase::Up, Some(button)) => runtime.button_released(x, y, button),
        (PointerPhase::Exit, _) => runtime.pointer_exited(),
        (PointerPhase::Cancel, _) => runtime.pointer_cancelled(),
    }
}

/// A key's name on the wire, as the framework's key.
fn key_of(name: &str) -> Option<Key> {
    Some(match name {
        "ArrowDown" => Key::Down,
        "ArrowUp" => Key::Up,
        "ArrowLeft" => Key::Left,
        "ArrowRight" => Key::Right,
        "Enter" => Key::Enter,
        "Escape" => Key::Escape,
        "Tab" => Key::Tab,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        "Backspace" => Key::Backspace,
        "Delete" => Key::Delete,
        "Home" => Key::Home,
        "End" => Key::End,
        "Insert" => Key::Insert,
        _ => {
            if let Some(number) = name.strip_prefix('F').and_then(|digits| digits.parse::<u8>().ok())
                && (1..=24).contains(&number)
            {
                return Some(Key::F(number));
            }
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                (Some(one), None) if !one.is_control() => Key::Char(one.to_ascii_lowercase()),
                _ => return None,
            }
        }
    })
}

fn access_action(action: AccessAction) -> Action {
    match action {
        AccessAction::Activate => Action::Activate,
        AccessAction::Focus => Action::Focus,
        AccessAction::SetText(text) => Action::SetText(text),
        AccessAction::Increment => Action::Increment,
        AccessAction::Decrement => Action::Decrement,
        AccessAction::SetValue(value) => Action::SetValue(value),
    }
}

/// The runtime's semantic tree, as the protocol says it.
pub(crate) fn semantics(tree: &accessibility::Tree) -> Vec<SemanticNode> {
    tree.nodes()
        .iter()
        .map(|node| SemanticNode {
            id: node.id.get(),
            role: role(node.role),
            label: node.label.to_string(),
            value: node.value.as_deref().map(str::to_string),
            bounds: (node.bounds.origin.x, node.bounds.origin.y, node.bounds.size.width, node.bounds.size.height),
            focused: node.focused,
            enabled: node.enabled,
            checked: node.checked,
            expanded: node.expanded,
            level: node.level,
            multiline: node.multiline,
        })
        .collect()
}

fn role(role: accessibility::Role) -> Role {
    match role {
        accessibility::Role::Text => Role::Text,
        accessibility::Role::Button => Role::Button,
        accessibility::Role::Checkbox => Role::Checkbox,
        accessibility::Role::Switch => Role::Switch,
        accessibility::Role::RadioButton => Role::RadioButton,
        accessibility::Role::Tab => Role::Tab,
        accessibility::Role::PopUpButton => Role::PopUpButton,
        accessibility::Role::Slider => Role::Slider,
        accessibility::Role::Stepper => Role::Stepper,
        accessibility::Role::TextField => Role::TextField,
        accessibility::Role::PasswordField => Role::PasswordField,
        accessibility::Role::Link => Role::Link,
        accessibility::Role::Heading => Role::Heading,
    }
}
