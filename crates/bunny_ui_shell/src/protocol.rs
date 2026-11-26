//! The shell protocol — what a Trinity shell and a block speak, in both
//! directions, as bytes. This is the contract the shell keeps loadable
//! forever (D723): a block bundles the engine it was built with, so the
//! engine may change under it; these bytes may not.
//!
//! ## Framing
//!
//! Every message is a little-endian `u32` length (the kind byte and the
//! payload), a `u8` kind, then the payload — the framing of the preview
//! wire. Numbers are little-endian, text is UTF-8 behind a `u32` length,
//! an `Option` is a `u8` tag then the value, a list a `u32` count.
//!
//! Kinds `0x00..=0x3f` travel from the guest to the host ([`ToHost`]),
//! `0x40..=0x7f` from the host to the guest ([`ToGuest`]); `0x80..` are
//! reserved.
//!
//! ## How it grows (the additive rule)
//!
//! - **A new message is a new kind.** A reader skips a kind it does not
//!   know by its length ([`Received::Unknown`]) and carries on, so a host
//!   can send an old block an event it has never heard of.
//! - **A message only ever grows at its end.** A reader takes the fields
//!   it knows and leaves the rest unread, so a newer peer may append a
//!   field to a message an older one already reads. The elements of a
//!   list ([`SemanticNode`]) each sit behind their own length and grow the
//!   same way.
//! - **A value set only ever grows**, and a value a reader does not know
//!   is said by name where it can be ([`Role::from_name`] reads an unknown
//!   role as a group) and refused by name where it cannot
//!   ([`Malformed::BadTag`]).
//! - **A capability is a bit.** The host offers the ones it grants, the
//!   guest answers with the ones it will use — the intersection — and
//!   neither ever sends what the other did not agree to. A bit this build
//!   has no name for survives the bytes and drops out of the answer.
//! - **The version is negotiated, and bumps only for what the rules above
//!   cannot say**: a change of meaning. The host offers its range, the
//!   guest answers the newest version both speak ([`Versions::negotiate`])
//!   or refuses with its own range.
//!
//! ## What a block can never ask for
//!
//! There is no capability to write, to open a socket or to read a file:
//! a block is a zero-import Wasm module, so it has nothing to ask with.
//! What its **signature** lets it have joins as kinds and bits the host
//! grants or withholds: the page it stands on ([`Context`], under
//! [`Capabilities::CONTEXT`]), the rows of the views it reads ([`Rows`],
//! under [`Capabilities::READS`]) and the declared actions it may ask the
//! shell to run ([`Requested`], under [`Capabilities::REQUESTS`]). A
//! request is a question, never a write: the shell judges it against the
//! signature, runs it through its own gate, and answers with an
//! [`Outcome`].
//!
//! ## The Effect level: a fetch is a question too
//!
//! A block is born Pure. At the Effect level it may read the network —
//! still without a socket: it asks the host to fetch a URL on its behalf
//! ([`Fetch`], under [`Capabilities::FETCHES`]), and the host answers
//! what came back ([`ToGuest::Fetched`]). The host performs only a `GET`
//! over `https` to a host the block's signature declares **and** its org
//! grants, under a size and a time limit of its own; anything else is
//! answered [`Fetched::Refused`], by name, and never leaves the machine.
//! The block cannot tell a host the shell refuses from one that does not
//! exist, and needs to: it shows the refusal's sentence.
//!
//! ## The scene
//!
//! A [`ToHost::Scene`] carries a preview-wire [`Frame`]: the draws mirror
//! the framework's display list command for command, with text as text
//! and fonts by name — a semantic scene, never pixels. The bytes behind
//! an image travel once, as a [`ToHost::Resource`], before the first
//! scene that draws it. The preview wire is frozen under the same
//! additive rule, so there is one scene vocabulary in the ecosystem.

use std::fmt;
use std::ops::{BitAnd, BitOr};

use bunny_ui_preview::wire::{self, Frame, Resource};

/// The version this build speaks, and the newest it knows.
pub const PROTOCOL_VERSION: u32 = 1;

/// The first bytes of an [`ToGuest::Offer`] and a [`ToHost::Accept`]: a
/// peer that sees anything else is not talking to a block.
pub const MAGIC: [u8; 4] = *b"BNYS";

/// The most one message may carry; past it a reader is being fed garbage.
pub const MAX_MESSAGE: u32 = 64 * 1024 * 1024;

mod kind {
    // guest → host
    pub const ACCEPT: u8 = 0x01;
    pub const REFUSE: u8 = 0x02;
    pub const SCENE: u8 = 0x03;
    pub const RESOURCE: u8 = 0x04;
    pub const SEMANTICS: u8 = 0x05;
    pub const PACE: u8 = 0x06;
    pub const FAULT: u8 = 0x07;
    pub const REQUEST: u8 = 0x08;
    pub const FETCH: u8 = 0x09;
    // host → guest
    pub const OFFER: u8 = 0x40;
    pub const RESIZE: u8 = 0x41;
    pub const POINTER: u8 = 0x42;
    pub const WHEEL: u8 = 0x43;
    pub const KEY: u8 = 0x44;
    pub const TEXT: u8 = 0x45;
    pub const FOCUS: u8 = 0x46;
    pub const ACCESSIBILITY: u8 = 0x47;
    pub const TICK: u8 = 0x48;
    pub const CONTEXT: u8 = 0x49;
    pub const ROWS: u8 = 0x4a;
    pub const ANSWER: u8 = 0x4b;
    pub const FETCHED: u8 = 0x4c;
}

// =============================================================================
// Versions and capabilities
// =============================================================================

/// The protocol versions a peer speaks, oldest to newest, both included.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Versions {
    pub oldest: u32,
    pub newest: u32,
}

impl Versions {
    /// What this build speaks.
    pub const THIS: Versions = Versions { oldest: 1, newest: PROTOCOL_VERSION };

    /// The newest version both ranges hold, or `None` when they share
    /// none.
    pub fn negotiate(self, other: Versions) -> Option<u32> {
        let newest = self.newest.min(other.newest);
        let oldest = self.oldest.max(other.oldest);
        (oldest <= newest).then_some(newest)
    }
}

/// What a host grants a block, and what the block answers it will use.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Capabilities(u64);

impl Capabilities {
    /// Nothing.
    pub const NONE: Capabilities = Capabilities(0);
    /// The block hands the host its scene. Without it a block draws
    /// nothing; every shell grants it.
    pub const SCENE: Capabilities = Capabilities(1);
    /// Pointer presses, moves and the wheel reach the block.
    pub const POINTER: Capabilities = Capabilities(1 << 1);
    /// Key strokes reach the block while it holds the shell's focus.
    pub const KEYS: Capabilities = Capabilities(1 << 2);
    /// Committed text (typing, the IME's result) reaches the block.
    pub const TEXT: Capabilities = Capabilities(1 << 3);
    /// The block hands the host its semantics, and the host's assistive
    /// requests reach it.
    pub const ACCESSIBILITY: Capabilities = Capabilities(1 << 4);
    /// The host is the block's clock: ticks reach it, and it says when it
    /// wants them.
    pub const TIME: Capabilities = Capabilities(1 << 5);
    /// The page the block stands on reaches it: its filters and the record
    /// open on it ([`ToGuest::Context`]).
    pub const CONTEXT: Capabilities = Capabilities(1 << 6);
    /// The rows of the views the block's signature reads reach it
    /// ([`ToGuest::Rows`]), as the person looking may see them.
    pub const READS: Capabilities = Capabilities(1 << 7);
    /// The block may ask the shell to run a declared action
    /// ([`ToHost::Request`]), and hears how it went ([`ToGuest::Answer`]).
    pub const REQUESTS: Capabilities = Capabilities(1 << 8);
    /// The block may ask the host to fetch a URL on its behalf
    /// ([`ToHost::Fetch`]), and hears what came back
    /// ([`ToGuest::Fetched`]) — the Effect level. The host still judges
    /// every fetch: the bit opens the conversation, never a host.
    pub const FETCHES: Capabilities = Capabilities(1 << 9);
    /// Every capability this build has a name for.
    pub const KNOWN: Capabilities = Capabilities(0b11_1111_1111);

    /// The capabilities a peer sent, unknown bits and all.
    pub const fn from_bits(bits: u64) -> Capabilities {
        Capabilities(bits)
    }

    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Whether every capability of `other` is here.
    pub const fn contains(self, other: Capabilities) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for Capabilities {
    type Output = Capabilities;
    fn bitor(self, other: Capabilities) -> Capabilities {
        Capabilities(self.0 | other.0)
    }
}

impl BitAnd for Capabilities {
    type Output = Capabilities;
    fn bitand(self, other: Capabilities) -> Capabilities {
        Capabilities(self.0 & other.0)
    }
}

impl fmt::Debug for Capabilities {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const NAMES: [(Capabilities, &str); 10] = [
            (Capabilities::SCENE, "SCENE"),
            (Capabilities::POINTER, "POINTER"),
            (Capabilities::KEYS, "KEYS"),
            (Capabilities::TEXT, "TEXT"),
            (Capabilities::ACCESSIBILITY, "ACCESSIBILITY"),
            (Capabilities::TIME, "TIME"),
            (Capabilities::CONTEXT, "CONTEXT"),
            (Capabilities::READS, "READS"),
            (Capabilities::REQUESTS, "REQUESTS"),
            (Capabilities::FETCHES, "FETCHES"),
        ];
        let mut list = f.debug_set();
        for (capability, name) in NAMES {
            if self.contains(capability) {
                list.entry(&format_args!("{name}"));
            }
        }
        let unknown = self.0 & !Capabilities::KNOWN.0;
        if unknown != 0 {
            list.entry(&format_args!("{unknown:#x}"));
        }
        list.finish()
    }
}

// =============================================================================
// Values
// =============================================================================

/// The box a block is given, and how it is shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// In points.
    pub width: f64,
    pub height: f64,
    /// Device pixels per point.
    pub scale: f64,
    pub scheme: Scheme,
}

/// The shell's appearance; the block follows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scheme {
    Light,
    Dark,
}

/// Where a pointer is in its press.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PointerPhase {
    Move,
    Down,
    Up,
    /// The pointer left the block's box.
    Exit,
    /// The shell took the press away (a drag left for the shell's own).
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PointerButton {
    Primary,
    Secondary,
    Middle,
}

/// The modifiers held. `COMMAND` is the primary one: ⌘ on a Mac, Ctrl
/// elsewhere; `CONTROL` is the Mac's ⌃ alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Modifiers(pub u8);

impl Modifiers {
    pub const NONE: Modifiers = Modifiers(0);
    pub const SHIFT: Modifiers = Modifiers(1);
    pub const COMMAND: Modifiers = Modifiers(1 << 1);
    pub const OPTION: Modifiers = Modifiers(1 << 2);
    pub const CONTROL: Modifiers = Modifiers(1 << 3);

    pub const fn contains(self, other: Modifiers) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for Modifiers {
    type Output = Modifiers;
    fn bitor(self, other: Modifiers) -> Modifiers {
        Modifiers(self.0 | other.0)
    }
}

/// A request from the shell's assistive technology to a node of the
/// block's semantics.
#[derive(Clone, Debug, PartialEq)]
pub enum AccessAction {
    Activate,
    Focus,
    SetText(String),
    Increment,
    Decrement,
    SetValue(f64),
}

/// What a semantic node is. Travels by name, so a role a newer block
/// knows reads as a [`Role::Group`] to an older shell instead of a
/// refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// Heard through its children; also what an unknown role reads as.
    Group,
    Text,
    Button,
    Checkbox,
    Switch,
    RadioButton,
    Tab,
    PopUpButton,
    Slider,
    Stepper,
    TextField,
    PasswordField,
    Link,
    Heading,
}

impl Role {
    const ALL: [(Role, &'static str); 14] = [
        (Role::Group, "group"),
        (Role::Text, "text"),
        (Role::Button, "button"),
        (Role::Checkbox, "checkbox"),
        (Role::Switch, "switch"),
        (Role::RadioButton, "radio"),
        (Role::Tab, "tab"),
        (Role::PopUpButton, "popup"),
        (Role::Slider, "slider"),
        (Role::Stepper, "stepper"),
        (Role::TextField, "textfield"),
        (Role::PasswordField, "password"),
        (Role::Link, "link"),
        (Role::Heading, "heading"),
    ];

    /// The role's name on the wire.
    pub fn name(self) -> &'static str {
        Role::ALL.iter().find(|(role, _)| *role == self).map_or("group", |(_, name)| name)
    }

    /// The role a name says; a name this build does not know is a group.
    pub fn from_name(name: &str) -> Role {
        Role::ALL.iter().find(|(_, known)| *known == name).map_or(Role::Group, |(role, _)| *role)
    }
}

/// One exposed element of the block, in reading order.
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticNode {
    /// Stable while the element stays exposed; what an
    /// [`ToGuest::Accessibility`] names.
    pub id: u64,
    pub role: Role,
    pub label: String,
    /// A field's value; never a password's.
    pub value: Option<String>,
    /// `(x, y, width, height)` in points, in the block's box.
    pub bounds: (f64, f64, f64, f64),
    pub focused: bool,
    pub enabled: bool,
    pub checked: Option<bool>,
    pub expanded: Option<bool>,
    pub level: Option<u32>,
    pub multiline: bool,
}

/// One term of the page's filter bar: a field and the value it is held
/// to, as the page shows them.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Filter {
    pub field: String,
    pub value: String,
}

/// The page a block stands on — what its signature says it receives.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct Context {
    /// The page's filters, in the order its bar holds them.
    pub filters: Vec<Filter>,
    /// The record open on the page, on a record page.
    pub record: Option<String>,
}

/// One record of a view the block reads.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Row {
    /// The record's id.
    pub record: String,
    /// Its cells, one per [`Rows::columns`], as the ledger shows them;
    /// `None` where the record holds nothing.
    pub cells: Vec<Option<String>>,
}

/// The rows of one view the block's signature reads, as the person
/// looking may see them, whole: each [`ToGuest::Rows`] replaces the last
/// for its view.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Rows {
    /// The view's id, as the signature names it.
    pub view: String,
    /// The fields of each row's cells, in order.
    pub columns: Vec<String>,
    pub rows: Vec<Row>,
}

impl Rows {
    /// The cell of `row` under the column `field`.
    pub fn cell<'a>(&self, row: &'a Row, field: &str) -> Option<&'a str> {
        let at = self.columns.iter().position(|column| column == field)?;
        row.cells.get(at)?.as_deref()
    }
}

/// A block asking the shell to run a declared action.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Requested {
    /// The block's own number for the request; the [`ToGuest::Answer`]
    /// names it.
    pub request: u64,
    /// The action's id, as the signature names it.
    pub action: String,
    /// The record it acts on, when it acts on one.
    pub record: Option<String>,
}

/// How the shell answered a request.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Outcome {
    /// The action ran and its write is on the ledger.
    Landed,
    /// The action ran and waits for a person's approval.
    Held,
    /// The person was asked and said no.
    Declined,
    /// The shell refused it, and why: an action the signature does not
    /// declare, one the person may not run, or the gate's own refusal.
    Refused(String),
}

/// A block asking the host to fetch a URL on its behalf: a `GET`, whose
/// body comes back whole in a [`ToGuest::Fetched`]. A method other than
/// `GET`, and headers, would grow this message at its end.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Fetch {
    /// The block's own number for the fetch; the [`ToGuest::Fetched`]
    /// names it. One counter with [`Requested::request`], so a number
    /// names one question.
    pub fetch: u64,
    /// The URL, whole — `https://<host>/<path>?<query>`.
    pub url: String,
}

/// What a host that performed a fetch heard back.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Response {
    /// The HTTP status. A redirect is answered as it came — the host never
    /// follows one to a host the block was not granted.
    pub status: u16,
    /// The `Content-Type` it named, when it named one.
    pub content_type: Option<String>,
    /// The body, whole, within the host's size limit.
    pub body: Vec<u8>,
}

/// How the host answered a fetch.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Fetched {
    /// The host performed it, and this came back.
    Answered(Response),
    /// The host refused it, and why — a host the signature does not
    /// declare, one the org does not grant, a scheme other than `https`,
    /// a shell that grants no network: it never left the machine.
    Refused(String),
    /// The host performed it and it did not come back whole, and why: the
    /// network failed, the time limit passed, the body outgrew the size
    /// limit.
    Failed(String),
}

// =============================================================================
// Messages
// =============================================================================

/// What the host sends the block.
#[derive(Clone, Debug, PartialEq)]
pub enum ToGuest {
    /// The first message: the versions the host speaks, what it grants,
    /// and the box. Answered by [`ToHost::Accept`] and the first scene,
    /// or by [`ToHost::Refuse`].
    Offer { versions: Versions, capabilities: Capabilities, viewport: Viewport },
    /// The box or its appearance changed.
    Resize(Viewport),
    /// `x`, `y` in points in the block's box. `clicks` counts a press in a
    /// run of them (a double click's second press is 2).
    Pointer { phase: PointerPhase, x: f64, y: f64, button: PointerButton, clicks: u8, modifiers: Modifiers },
    Wheel { x: f64, y: f64, dx: f64, dy: f64, modifiers: Modifiers },
    /// A key, while the block holds the shell's focus. `key` is the W3C
    /// `KeyboardEvent.key` name of a named key (`Enter`, `ArrowLeft`,
    /// `F5`…) and, for a printable key, the character it types with no
    /// modifier applied, lowercased. `typed` is what it typed with them
    /// (empty for nothing).
    Key { key: String, typed: String, modifiers: Modifiers },
    /// Committed text: typing, a paste of characters, the IME's result.
    /// The composition itself stays with the shell.
    Text { text: String },
    /// The block gained or lost the shell's keyboard focus.
    Focus { focused: bool },
    Accessibility { node: u64, action: AccessAction },
    /// Time passed. The block has no clock of its own.
    Tick { dt_ms: f64 },
    /// The page the block stands on, whole, when it changed.
    Context(Context),
    /// The rows of a view the block reads, whole, when they changed.
    Rows(Rows),
    /// How the shell answered the block's request `request`.
    Answer { request: u64, outcome: Outcome },
    /// How the host answered the block's fetch `fetch`.
    Fetched { fetch: u64, fetched: Fetched },
}

/// What the block sends the host.
#[derive(Clone, Debug, PartialEq)]
pub enum ToHost {
    /// The handshake's answer: the version both speak, the capabilities the
    /// block will use (within the offer), and the engine it was built with.
    Accept { version: u32, capabilities: Capabilities, engine: String },
    /// No common version: the block's own range, and why.
    Refuse { versions: Versions, reason: String },
    /// What the block shows now. Sent when it changed; `seq` grows.
    Scene(Frame),
    /// The bytes behind an image key, before the first scene that draws it.
    Resource(Resource),
    /// The block's semantics as of the scene `seq`.
    Semantics { seq: u64, nodes: Vec<SemanticNode> },
    /// Whether the block wants ticks (an animation runs).
    Pace { animating: bool },
    /// Input the block could not read; it read nothing after it.
    Fault { reason: String },
    /// The block asks the shell to run a declared action.
    Request(Requested),
    /// The block asks the host to fetch a URL on its behalf.
    Fetch(Fetch),
}

/// A message in either direction.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    ToGuest(ToGuest),
    ToHost(ToHost),
}

/// What a reader hands back.
#[derive(Clone, Debug, PartialEq)]
pub enum Received {
    Message(Message),
    /// A kind this build does not know, skipped by its length.
    Unknown { kind: u8, len: u32 },
}

/// Why bytes could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Malformed {
    /// The payload ended before the message did.
    Truncated { kind: u8 },
    /// A tag no variant of `what` answers to.
    BadTag { what: &'static str, tag: u8 },
    /// A string that is not UTF-8.
    BadText { kind: u8 },
    /// An offer or an answer that does not start with [`MAGIC`].
    BadMagic,
    /// A length past [`MAX_MESSAGE`], or a length of zero.
    Oversized { len: u32 },
    /// The scene or the resource inside could not be read.
    Scene(wire::Malformed),
}

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Malformed::Truncated { kind } => write!(f, "a message of kind {kind:#04x} ended before its payload did"),
            Malformed::BadTag { what, tag } => write!(f, "no {what} answers to the tag {tag}"),
            Malformed::BadText { kind } => write!(f, "a string in a message of kind {kind:#04x} is not UTF-8"),
            Malformed::BadMagic => f.write_str("the handshake does not start with BNYS"),
            Malformed::Oversized { len } => write!(f, "a message of {len} bytes is past the protocol's limit"),
            Malformed::Scene(why) => write!(f, "the scene inside: {why}"),
        }
    }
}

impl std::error::Error for Malformed {}

// =============================================================================
// Encoding
// =============================================================================

/// The bytes of one message, framed.
pub fn encode(message: &Message) -> Vec<u8> {
    let mut payload = Vec::new();
    let kind = match message {
        Message::ToGuest(message) => put_to_guest(message, &mut payload),
        Message::ToHost(message) => put_to_host(message, &mut payload),
    };
    let mut out = Vec::with_capacity(payload.len() + 5);
    out.extend_from_slice(&(payload.len() as u32 + 1).to_le_bytes());
    out.push(kind);
    out.extend_from_slice(&payload);
    out
}

struct Out<'a>(&'a mut Vec<u8>);

impl Out<'_> {
    fn u8(&mut self, value: u8) {
        self.0.push(value);
    }
    fn bool(&mut self, value: bool) {
        self.u8(u8::from(value));
    }
    fn u16(&mut self, value: u16) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn f64(&mut self, value: f64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn bytes(&mut self, value: &[u8]) {
        self.u32(value.len() as u32);
        self.0.extend_from_slice(value);
    }
    fn string(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }
    fn option<T>(&mut self, value: Option<T>, put: impl FnOnce(&mut Self, T)) {
        match value {
            None => self.u8(0),
            Some(value) => {
                self.u8(1);
                put(self, value);
            }
        }
    }
    fn viewport(&mut self, viewport: &Viewport) {
        self.f64(viewport.width);
        self.f64(viewport.height);
        self.f64(viewport.scale);
        self.u8(match viewport.scheme {
            Scheme::Light => 0,
            Scheme::Dark => 1,
        });
    }
    fn versions(&mut self, versions: Versions) {
        self.u32(versions.oldest);
        self.u32(versions.newest);
    }
    /// A value behind its own length, so it may grow at its end.
    fn sized(&mut self, put: impl FnOnce(&mut Out<'_>)) {
        let mut inner = Vec::new();
        put(&mut Out(&mut inner));
        self.bytes(&inner);
    }
}

fn put_to_guest(message: &ToGuest, payload: &mut Vec<u8>) -> u8 {
    let mut out = Out(payload);
    match message {
        ToGuest::Offer { versions, capabilities, viewport } => {
            out.0.extend_from_slice(&MAGIC);
            out.versions(*versions);
            out.u64(capabilities.bits());
            out.viewport(viewport);
            kind::OFFER
        }
        ToGuest::Resize(viewport) => {
            out.viewport(viewport);
            kind::RESIZE
        }
        ToGuest::Pointer { phase, x, y, button, clicks, modifiers } => {
            out.u8(match phase {
                PointerPhase::Move => 0,
                PointerPhase::Down => 1,
                PointerPhase::Up => 2,
                PointerPhase::Exit => 3,
                PointerPhase::Cancel => 4,
            });
            out.f64(*x);
            out.f64(*y);
            out.u8(match button {
                PointerButton::Primary => 0,
                PointerButton::Secondary => 1,
                PointerButton::Middle => 2,
            });
            out.u8(*clicks);
            out.u8(modifiers.0);
            kind::POINTER
        }
        ToGuest::Wheel { x, y, dx, dy, modifiers } => {
            out.f64(*x);
            out.f64(*y);
            out.f64(*dx);
            out.f64(*dy);
            out.u8(modifiers.0);
            kind::WHEEL
        }
        ToGuest::Key { key, typed, modifiers } => {
            out.string(key);
            out.string(typed);
            out.u8(modifiers.0);
            kind::KEY
        }
        ToGuest::Text { text } => {
            out.string(text);
            kind::TEXT
        }
        ToGuest::Focus { focused } => {
            out.bool(*focused);
            kind::FOCUS
        }
        ToGuest::Accessibility { node, action } => {
            out.u64(*node);
            match action {
                AccessAction::Activate => out.u8(0),
                AccessAction::Focus => out.u8(1),
                AccessAction::SetText(text) => {
                    out.u8(2);
                    out.string(text);
                }
                AccessAction::Increment => out.u8(3),
                AccessAction::Decrement => out.u8(4),
                AccessAction::SetValue(value) => {
                    out.u8(5);
                    out.f64(*value);
                }
            }
            kind::ACCESSIBILITY
        }
        ToGuest::Tick { dt_ms } => {
            out.f64(*dt_ms);
            kind::TICK
        }
        ToGuest::Context(context) => {
            out.u32(context.filters.len() as u32);
            for filter in &context.filters {
                out.sized(|out| {
                    out.string(&filter.field);
                    out.string(&filter.value);
                });
            }
            out.option(context.record.as_deref(), Out::string);
            kind::CONTEXT
        }
        ToGuest::Rows(rows) => {
            out.string(&rows.view);
            out.u32(rows.columns.len() as u32);
            for column in &rows.columns {
                out.string(column);
            }
            out.u32(rows.rows.len() as u32);
            for row in &rows.rows {
                out.sized(|out| {
                    out.string(&row.record);
                    out.u32(row.cells.len() as u32);
                    for cell in &row.cells {
                        out.option(cell.as_deref(), Out::string);
                    }
                });
            }
            kind::ROWS
        }
        ToGuest::Answer { request, outcome } => {
            out.u64(*request);
            match outcome {
                Outcome::Landed => out.u8(0),
                Outcome::Held => out.u8(1),
                Outcome::Declined => out.u8(2),
                Outcome::Refused(reason) => {
                    out.u8(3);
                    out.string(reason);
                }
            }
            kind::ANSWER
        }
        ToGuest::Fetched { fetch, fetched } => {
            out.u64(*fetch);
            match fetched {
                Fetched::Answered(response) => {
                    out.u8(0);
                    out.u16(response.status);
                    out.option(response.content_type.as_deref(), Out::string);
                    out.bytes(&response.body);
                }
                Fetched::Refused(reason) => {
                    out.u8(1);
                    out.string(reason);
                }
                Fetched::Failed(reason) => {
                    out.u8(2);
                    out.string(reason);
                }
            }
            kind::FETCHED
        }
    }
}

fn put_to_host(message: &ToHost, payload: &mut Vec<u8>) -> u8 {
    let mut out = Out(payload);
    match message {
        ToHost::Accept { version, capabilities, engine } => {
            out.0.extend_from_slice(&MAGIC);
            out.u32(*version);
            out.u64(capabilities.bits());
            out.string(engine);
            kind::ACCEPT
        }
        ToHost::Refuse { versions, reason } => {
            out.versions(*versions);
            out.string(reason);
            kind::REFUSE
        }
        ToHost::Scene(frame) => {
            out.bytes(&wire::encode(&wire::Message::ToIde(wire::ToIde::Frame(frame.clone()))));
            kind::SCENE
        }
        ToHost::Resource(resource) => {
            out.bytes(&wire::encode(&wire::Message::ToIde(wire::ToIde::Resource(resource.clone()))));
            kind::RESOURCE
        }
        ToHost::Semantics { seq, nodes } => {
            out.u64(*seq);
            out.u32(nodes.len() as u32);
            for node in nodes {
                out.sized(|out| {
                    out.u64(node.id);
                    out.string(node.role.name());
                    out.string(&node.label);
                    out.option(node.value.as_deref(), Out::string);
                    out.f64(node.bounds.0);
                    out.f64(node.bounds.1);
                    out.f64(node.bounds.2);
                    out.f64(node.bounds.3);
                    out.bool(node.focused);
                    out.bool(node.enabled);
                    out.option(node.checked, Out::bool);
                    out.option(node.expanded, Out::bool);
                    out.option(node.level, Out::u32);
                    out.bool(node.multiline);
                });
            }
            kind::SEMANTICS
        }
        ToHost::Pace { animating } => {
            out.bool(*animating);
            kind::PACE
        }
        ToHost::Fault { reason } => {
            out.string(reason);
            kind::FAULT
        }
        ToHost::Request(requested) => {
            out.u64(requested.request);
            out.string(&requested.action);
            out.option(requested.record.as_deref(), Out::string);
            kind::REQUEST
        }
        ToHost::Fetch(fetch) => {
            out.u64(fetch.fetch);
            out.string(&fetch.url);
            kind::FETCH
        }
    }
}

// =============================================================================
// Decoding
// =============================================================================

/// Reads messages out of bytes fed in any pieces.
#[derive(Debug, Default)]
pub struct Reader {
    buf: Vec<u8>,
    at: usize,
}

impl Reader {
    pub fn new() -> Reader {
        Reader::default()
    }

    /// More bytes, as they arrived.
    pub fn feed(&mut self, bytes: &[u8]) {
        if self.at > 0 && self.at == self.buf.len() {
            self.buf.clear();
            self.at = 0;
        }
        self.buf.extend_from_slice(bytes);
    }

    /// The bytes fed that are not yet a whole message.
    pub fn pending(&self) -> usize {
        self.buf.len() - self.at
    }

    /// The next whole message, if one has arrived. After an error the
    /// reader is lost: the stream is no longer framed where it thinks.
    pub fn pull(&mut self) -> Result<Option<Received>, Malformed> {
        let rest = &self.buf[self.at..];
        if rest.len() < 5 {
            return Ok(None);
        }
        let len = u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]);
        if len > MAX_MESSAGE || len == 0 {
            return Err(Malformed::Oversized { len });
        }
        let len = len as usize;
        if rest.len() < 4 + len {
            return Ok(None);
        }
        let received = decode(rest[4], &rest[5..4 + len]);
        self.at += 4 + len;
        received.map(Some)
    }
}

/// One payload, by its kind. Bytes after the fields this build knows are
/// a newer peer's, and are left unread.
pub fn decode(kind: u8, payload: &[u8]) -> Result<Received, Malformed> {
    let mut c = Cursor { bytes: payload, at: 0, kind };
    let message = match kind {
        kind::OFFER => {
            c.magic()?;
            Message::ToGuest(ToGuest::Offer {
                versions: c.versions()?,
                capabilities: Capabilities::from_bits(c.u64()?),
                viewport: c.viewport()?,
            })
        }
        kind::RESIZE => Message::ToGuest(ToGuest::Resize(c.viewport()?)),
        kind::POINTER => Message::ToGuest(ToGuest::Pointer {
            phase: match c.u8()? {
                0 => PointerPhase::Move,
                1 => PointerPhase::Down,
                2 => PointerPhase::Up,
                3 => PointerPhase::Exit,
                4 => PointerPhase::Cancel,
                tag => return Err(Malformed::BadTag { what: "PointerPhase", tag }),
            },
            x: c.f64()?,
            y: c.f64()?,
            button: match c.u8()? {
                0 => PointerButton::Primary,
                1 => PointerButton::Secondary,
                2 => PointerButton::Middle,
                tag => return Err(Malformed::BadTag { what: "PointerButton", tag }),
            },
            clicks: c.u8()?,
            modifiers: Modifiers(c.u8()?),
        }),
        kind::WHEEL => Message::ToGuest(ToGuest::Wheel {
            x: c.f64()?,
            y: c.f64()?,
            dx: c.f64()?,
            dy: c.f64()?,
            modifiers: Modifiers(c.u8()?),
        }),
        kind::KEY => Message::ToGuest(ToGuest::Key { key: c.string()?, typed: c.string()?, modifiers: Modifiers(c.u8()?) }),
        kind::TEXT => Message::ToGuest(ToGuest::Text { text: c.string()? }),
        kind::FOCUS => Message::ToGuest(ToGuest::Focus { focused: c.bool()? }),
        kind::ACCESSIBILITY => {
            let node = c.u64()?;
            let action = match c.u8()? {
                0 => AccessAction::Activate,
                1 => AccessAction::Focus,
                2 => AccessAction::SetText(c.string()?),
                3 => AccessAction::Increment,
                4 => AccessAction::Decrement,
                5 => AccessAction::SetValue(c.f64()?),
                tag => return Err(Malformed::BadTag { what: "AccessAction", tag }),
            };
            Message::ToGuest(ToGuest::Accessibility { node, action })
        }
        kind::TICK => Message::ToGuest(ToGuest::Tick { dt_ms: c.f64()? }),
        kind::CONTEXT => {
            let count = c.u32()? as usize;
            let mut filters = Vec::with_capacity(count.min(payload.len()));
            for _ in 0..count {
                let mut f = Cursor { bytes: c.bytes()?, at: 0, kind };
                filters.push(Filter { field: f.string()?, value: f.string()? });
            }
            Message::ToGuest(ToGuest::Context(Context { filters, record: c.option(Cursor::string)? }))
        }
        kind::ROWS => {
            let view = c.string()?;
            let count = c.u32()? as usize;
            let mut columns = Vec::with_capacity(count.min(payload.len()));
            for _ in 0..count {
                columns.push(c.string()?);
            }
            let count = c.u32()? as usize;
            let mut rows = Vec::with_capacity(count.min(payload.len()));
            for _ in 0..count {
                let mut r = Cursor { bytes: c.bytes()?, at: 0, kind };
                let record = r.string()?;
                let cells = r.u32()? as usize;
                let mut row = Row { record, cells: Vec::with_capacity(cells.min(payload.len())) };
                for _ in 0..cells {
                    row.cells.push(r.option(Cursor::string)?);
                }
                rows.push(row);
            }
            Message::ToGuest(ToGuest::Rows(Rows { view, columns, rows }))
        }
        kind::ANSWER => {
            let request = c.u64()?;
            let outcome = match c.u8()? {
                0 => Outcome::Landed,
                1 => Outcome::Held,
                2 => Outcome::Declined,
                3 => Outcome::Refused(c.string()?),
                tag => return Err(Malformed::BadTag { what: "Outcome", tag }),
            };
            Message::ToGuest(ToGuest::Answer { request, outcome })
        }
        kind::FETCHED => {
            let fetch = c.u64()?;
            let fetched = match c.u8()? {
                0 => Fetched::Answered(Response {
                    status: c.u16()?,
                    content_type: c.option(Cursor::string)?,
                    body: c.bytes()?.to_vec(),
                }),
                1 => Fetched::Refused(c.string()?),
                2 => Fetched::Failed(c.string()?),
                tag => return Err(Malformed::BadTag { what: "Fetched", tag }),
            };
            Message::ToGuest(ToGuest::Fetched { fetch, fetched })
        }
        kind::ACCEPT => {
            c.magic()?;
            Message::ToHost(ToHost::Accept {
                version: c.u32()?,
                capabilities: Capabilities::from_bits(c.u64()?),
                engine: c.string()?,
            })
        }
        kind::REFUSE => Message::ToHost(ToHost::Refuse { versions: c.versions()?, reason: c.string()? }),
        kind::SCENE => match c.nested()? {
            wire::ToIde::Frame(frame) => Message::ToHost(ToHost::Scene(frame)),
            _ => return Err(Malformed::BadTag { what: "Scene", tag: kind }),
        },
        kind::RESOURCE => match c.nested()? {
            wire::ToIde::Resource(resource) => Message::ToHost(ToHost::Resource(resource)),
            _ => return Err(Malformed::BadTag { what: "Resource", tag: kind }),
        },
        kind::SEMANTICS => {
            let seq = c.u64()?;
            let count = c.u32()? as usize;
            let mut nodes = Vec::with_capacity(count.min(payload.len()));
            for _ in 0..count {
                let element = c.bytes()?;
                let mut n = Cursor { bytes: element, at: 0, kind };
                nodes.push(SemanticNode {
                    id: n.u64()?,
                    role: Role::from_name(&n.string()?),
                    label: n.string()?,
                    value: n.option(Cursor::string)?,
                    bounds: (n.f64()?, n.f64()?, n.f64()?, n.f64()?),
                    focused: n.bool()?,
                    enabled: n.bool()?,
                    checked: n.option(Cursor::bool)?,
                    expanded: n.option(Cursor::bool)?,
                    level: n.option(Cursor::u32)?,
                    multiline: n.bool()?,
                });
            }
            Message::ToHost(ToHost::Semantics { seq, nodes })
        }
        kind::PACE => Message::ToHost(ToHost::Pace { animating: c.bool()? }),
        kind::FAULT => Message::ToHost(ToHost::Fault { reason: c.string()? }),
        kind::REQUEST => Message::ToHost(ToHost::Request(Requested {
            request: c.u64()?,
            action: c.string()?,
            record: c.option(Cursor::string)?,
        })),
        kind::FETCH => Message::ToHost(ToHost::Fetch(Fetch { fetch: c.u64()?, url: c.string()? })),
        _ => return Ok(Received::Unknown { kind, len: payload.len() as u32 }),
    };
    Ok(Received::Message(message))
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
    kind: u8,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Malformed> {
        let end = self.at.checked_add(n).ok_or(Malformed::Truncated { kind: self.kind })?;
        let taken = self.bytes.get(self.at..end).ok_or(Malformed::Truncated { kind: self.kind })?;
        self.at = end;
        Ok(taken)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], Malformed> {
        let mut out = [0; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8, Malformed> {
        Ok(self.take(1)?[0])
    }
    fn bool(&mut self) -> Result<bool, Malformed> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            tag => Err(Malformed::BadTag { what: "bool", tag }),
        }
    }
    fn u16(&mut self) -> Result<u16, Malformed> {
        Ok(u16::from_le_bytes(self.array()?))
    }
    fn u32(&mut self) -> Result<u32, Malformed> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    fn u64(&mut self) -> Result<u64, Malformed> {
        Ok(u64::from_le_bytes(self.array()?))
    }
    fn f64(&mut self) -> Result<f64, Malformed> {
        Ok(f64::from_le_bytes(self.array()?))
    }
    fn bytes(&mut self) -> Result<&'a [u8], Malformed> {
        let len = self.u32()? as usize;
        self.take(len)
    }
    fn string(&mut self) -> Result<String, Malformed> {
        let kind = self.kind;
        let bytes = self.bytes()?;
        String::from_utf8(bytes.to_vec()).map_err(|_| Malformed::BadText { kind })
    }
    fn option<T>(&mut self, get: impl FnOnce(&mut Self) -> Result<T, Malformed>) -> Result<Option<T>, Malformed> {
        match self.u8()? {
            0 => Ok(None),
            1 => get(self).map(Some),
            tag => Err(Malformed::BadTag { what: "Option", tag }),
        }
    }
    fn magic(&mut self) -> Result<(), Malformed> {
        if self.array::<4>().map_err(|_| Malformed::BadMagic)? == MAGIC { Ok(()) } else { Err(Malformed::BadMagic) }
    }
    fn versions(&mut self) -> Result<Versions, Malformed> {
        Ok(Versions { oldest: self.u32()?, newest: self.u32()? })
    }
    fn viewport(&mut self) -> Result<Viewport, Malformed> {
        Ok(Viewport {
            width: self.f64()?,
            height: self.f64()?,
            scale: self.f64()?,
            scheme: match self.u8()? {
                0 => Scheme::Light,
                1 => Scheme::Dark,
                tag => return Err(Malformed::BadTag { what: "Scheme", tag }),
            },
        })
    }
    /// A preview-wire message carried whole.
    fn nested(&mut self) -> Result<wire::ToIde, Malformed> {
        let bytes = self.bytes()?;
        let mut reader = wire::Reader::new();
        reader.feed(bytes);
        match reader.pull().map_err(Malformed::Scene)? {
            Some(wire::Received::Message(wire::Message::ToIde(message))) => Ok(message),
            _ => Err(Malformed::Truncated { kind: self.kind }),
        }
    }
}
