//! The wire — what a preview host sends the tool that shows its frames,
//! and what the tool sends back.
//!
//! Every message is one frame: a little-endian `u32` length, a `u8`
//! kind, then the payload. A reader that meets a kind it does not know
//! skips it by its length and carries on — that is how the wire grows
//! without a version bump; a payload that ends before its message does
//! is [`Malformed`], and so is a tag no variant answers to. The numbers
//! are little-endian throughout, text is UTF-8 behind a `u32` length, an
//! `Option` is a `u8` tag, a list a `u32` count.
//!
//! A [`Frame`] carries a display list the way the framework paints one —
//! [`Draw`] mirrors `DrawCommand` variant for variant — plus the sites of
//! its nodes and the boxes a platform surface occupies. Text and the
//! other repeated strings of a frame sit once in its string table, and
//! the draws name them by index. An image is named by key; its bytes
//! travel once, as a [`Resource`], before the first frame that draws it.
//!
//! [`WIRE_VERSION`] is 1. It is frozen: a change to an existing message
//! is a new kind, never a new meaning for an old one. The kinds that
//! joined after it froze — `Touch`, `Magnify` and `Place`, tool → host — are
//! skipped by a host that predates them, by their length; and `Glyph`, host
//! → tool, by a tool that predates it. The light weights (`Thin`,
//! `ExtraLight`, `Light`, tags 6–8) are the one value set that grew: no
//! old tag changed meaning and every stream without them is the bytes it
//! was, but a tool that predates them reads a frame that draws one as
//! malformed, naming the tag ([`Malformed::BadTag`]) — a host should not
//! send a light face to a tool older than the weights. A picture placed
//! between pixels (`Draw` tag 9) grew the draws the same way: a snapped
//! one keeps tag 6 and its bytes, and a tool that predates the tag reads a
//! frame that draws one as malformed.

use std::fmt;

mod instances;
mod views;
mod state;
pub use state::{DependencyKey, InvalidStateRequestId, MatchedState, StateConstructor, StateDependency, StateEvidence, StateMismatch, StateRead, StateRefusal, StateReply, StateRequestId, StateSnapshot, StateTarget, StateValue, STATE_DEPENDENCY_LIMIT, STATE_TEXT_LIMIT};
pub use views::{InvalidMountedViewId, MatchedViews, MountedViewId, ViewIdentity, ViewMismatch, ViewSnapshot};
pub use instances::{ExecutionMode, ExecutionSelection, InstanceCommand, InstanceSelection, InvalidSceneInstanceId, SceneInstanceId};

/// The version of this wire — what `Hello` announces.
pub const WIRE_VERSION: u32 = 1;

/// The whole-app scene, exported by `app!` in a preview build.
pub const APP_IDENT: &str = "app";

/// The first bytes of a `Hello`: a reader that sees anything else is on
/// the wrong socket.
pub const MAGIC: [u8; 4] = *b"BNYP";

/// The most a frame may carry; past it a reader is being fed garbage.
pub const MAX_FRAME: u32 = 256 * 1024 * 1024;

/// The kind byte of each message.
pub mod kind {
    pub const HELLO: u8 = 1;
    pub const READY: u8 = 2;
    pub const FRAME: u8 = 3;
    pub const RESOURCE: u8 = 4;
    pub const STATUS: u8 = 5;
    pub const PANICKED: u8 = 6;
    pub const PANIC: u8 = 7;
    pub const SNAPSHOT: u8 = 8;
    pub const PONG: u8 = 9;
    pub const INSTANCE_FRAME: u8 = 10;
    pub const INSTANCE_PANICKED: u8 = 11;
    pub const INSTANCE_SNAPSHOT: u8 = 12;
    pub const INSTANCE_ERROR: u8 = 13;
    pub const EXECUTIONS_SELECTED: u8 = 14;
    pub const VIEW_SNAPSHOT: u8 = 15;
    pub const STATE_OBSERVATION: u8 = 16;
    /// An app's own glyph that no single path draws, whole — a resource
    /// on a kind of its own, joined after v1 froze: a tool that predates
    /// it skips it by its length.
    pub const GLYPH: u8 = 17;

    pub const SELECT: u8 = 64;
    pub const RETRAIT: u8 = 65;
    pub const RESIZE: u8 = 66;
    pub const POINTER: u8 = 67;
    pub const WHEEL: u8 = 68;
    pub const KEY: u8 = 69;
    pub const TEXT: u8 = 70;
    pub const OVERRIDE: u8 = 71;
    pub const CLEAR_OVERRIDES: u8 = 72;
    pub const REQUEST_SNAPSHOT: u8 = 73;
    pub const FORGET: u8 = 74;
    pub const PING: u8 = 75;
    pub const BYE: u8 = 76;
    // joined after v1 froze: a host that predates them skips them
    pub const TOUCH: u8 = 77;
    pub const MAGNIFY: u8 = 78;
    pub const PLACE: u8 = 79;
    pub const SELECT_INSTANCES: u8 = 80;
    pub const INSTANCE: u8 = 81;
    pub const SET_EXECUTION: u8 = 82;
    pub const SELECT_EXECUTIONS: u8 = 83;
    pub const OBSERVE_VIEWS: u8 = 84;
    pub const OBSERVE_STATE: u8 = 85;
    /// Explicit text paste, negotiated independently of ordinary keys/text.
    pub const PASTE: u8 = 86;
}

/// Features a host explicitly offers in `Hello.caps`.
pub mod caps {
    /// Whole clipboard-text commits after custom shortcut handling.
    pub const CLIPBOARD_TEXT: u32 = 256;
    /// Selected read-only state observations through messages 16 and 85.
    pub const STATE_OBSERVATIONS: u32 = 128;
    /// Opt-in mounted-view snapshots accompanying exact frame publications.
    pub const VIEW_IDENTITIES: u32 = 64;
    /// Free form, grid and live Position/Span overrides through message 79.
    pub const PLACE: u32 = 1;
    /// Independently addressed preview instances through messages 10–13 and 80–81.
    pub const INSTANCES: u32 = 2;
    /// Ellipse builders and inherited foreground ink for basic shapes.
    pub const SHAPES: u32 = 4;
    /// The application framework provides once-per-identity on_mount handlers.
    pub const MOUNT: u32 = 8;
    /// Per-instance Design/Play execution contexts and lifetime notifications.
    pub const EXECUTION: u32 = 16;
    /// Atomic initial execution selection and remount on an intent change.
    pub const EXECUTION_SELECTION: u32 = 32;
}

// =============================================================================
// Geometry and paint, as plain data
// =============================================================================

/// A point in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// A size in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

/// A rectangle in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

/// An sRGB colour with straight alpha.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// The radius of each corner.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Corners {
    pub top_left: f64,
    pub top_right: f64,
    pub bottom_right: f64,
    pub bottom_left: f64,
}

/// Insets at each edge.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Edges {
    pub top: f64,
    pub trailing: f64,
    pub bottom: f64,
    pub leading: f64,
}

/// A gradient resolved to a rectangle — what a `Draw::Gradient` paints.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GradientPaint {
    Radial { center: Point, start: f64, end: f64, aspect: f64, inner: Color, outer: Color },
    Linear { start: Point, end: Point, from: Color, to: Color },
}

/// A gradient as an app wrote it, in unit coordinates — what a vector
/// icon's ink may hold.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gradient {
    Radial { center: Point, start: f64, end: Option<f64>, aspect: f64, inner: Color, outer: Color },
    Linear { start: Point, end: Point, from: Color, to: Color },
}

/// The liquid-glass pane's parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glass {
    pub blur: f64,
    pub tint: Color,
    pub refraction_band: f64,
    pub refraction_amount: f64,
    pub chromatic: f64,
    pub highlight: Color,
    pub highlight_band: f64,
    pub highlight_intensity: f64,
    pub saturation: f64,
    pub brightness: f64,
    pub sheen: f64,
    pub spot_center: Point,
    pub spot_radius: f64,
    pub spot_alpha: f64,
}

/// A font weight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Weight {
    Regular,
    Medium,
    Semibold,
    Bold,
    ExtraBold,
    Black,
    Thin,
    ExtraLight,
    Light,
}

/// A font design.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Design {
    Default,
    Mono,
}

/// A font slant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slant {
    Upright,
    Italic,
}

/// The font of a text line. `family` names a string of the frame's table;
/// `None` is the system face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Font {
    pub size: f64,
    pub weight: Weight,
    pub design: Design,
    pub slant: Slant,
    pub family: Option<u32>,
    pub tracking: f64,
}

/// An image, by the key of its resource, with the fades and flips the
/// app wrapped it in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageRef {
    pub key: u64,
    /// 255 is the image as it is.
    pub alpha: u8,
    pub mirrored: bool,
}

/// One draw command — `DrawCommand`, variant for variant.
#[derive(Clone, Debug, PartialEq)]
pub enum Draw {
    FillRect { rect: Rect, color: Color, corners: Corners },
    Gradient { rect: Rect, paint: GradientPaint, corners: Corners },
    Backdrop { rect: Rect, glass: Glass, corners: Corners },
    Shadow { rect: Rect, radius: f64, color: Color, corners: Corners },
    StrokeRect { rect: Rect, color: Color, width: f64, corners: Corners },
    /// One wrapped line: `text` names the string, `range` the byte span of it painted.
    TextLine { origin: Point, text: u32, range: (u32, u32), color: Color, font: Font },
    /// `subpixel`: the picture lands at its exact origin, between pixels
    /// when it falls there (`ImagePlacement::Subpixel`). It travels as a
    /// tag of its own (9), so a snapped picture is the bytes it was.
    Image { rect: Rect, image: ImageRef, subpixel: bool },
    PushClip { rect: Rect, corners: Corners },
    PopClip,
}

// =============================================================================
// Sites, hosts, metrics, the frame
// =============================================================================

/// Where one node of the frame was written, and its frame. The strings
/// are indices into the frame's table.
#[derive(Clone, Debug, PartialEq)]
pub struct SiteBox {
    pub index: u32,
    pub parent: Option<u32>,
    /// The node's kind (`text`, `vstack`, `modifier`, …).
    pub kind: u32,
    /// The modifier's name, for a modifier node.
    pub modifier: Option<u32>,
    /// The source file, for a site the compiler recorded; `None` for a
    /// node the framework named itself (`name` then says what).
    pub file: Option<u32>,
    /// The framework's name for a node with no source site.
    pub name: Option<u32>,
    pub line: u32,
    pub column: u32,
    pub frame: Rect,
    /// The component boundary the node sits under.
    pub path: Option<u32>,
}

/// A platform surface's place in the frame — a webview, a video — drawn
/// by the tool as a placeholder until a snapshot of it arrives.
#[derive(Clone, Debug, PartialEq)]
pub struct HostBox {
    pub index: u32,
    /// The surface's kind, a string of the table.
    pub kind: u32,
    pub frame: Rect,
}

/// What the frame cost.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Metrics {
    pub build_ms: f64,
    pub layout_ms: f64,
    pub body_passes: u32,
    pub commands: u32,
}

/// One rendered frame of one preview variant.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// The preview's ident.
    pub preview: String,
    /// Which of its variants (a `Both` scheme is two).
    pub variant: u32,
    /// Increases per preview; publications may skip numbers. Never reused by
    /// another mount in one host connection. Drop older publications.
    pub seq: u64,
    /// The generation that rendered it.
    pub generation: u32,
    pub size: Size,
    pub scale: f64,
    pub background: Color,
    /// The strings the draws, sites and hosts name by index.
    pub strings: Vec<String>,
    pub commands: Vec<Draw>,
    pub sites: Vec<SiteBox>,
    pub hosts: Vec<HostBox>,
    pub metrics: Metrics,
}

// =============================================================================
// Resources
// =============================================================================

/// One step of a vector path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Verb {
    Move(f32, f32),
    Line(f32, f32),
    Quad(f32, f32, f32, f32),
    Cubic(f32, f32, f32, f32, f32, f32),
    Close,
}

/// How a vector path is painted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Paint {
    Fill { even_odd: bool },
    Stroke { width: f32 },
}

/// What a vector path is painted with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ink {
    Solid(Color),
    Ramp(Gradient),
}

/// The bytes behind an image key.
#[derive(Clone, Debug, PartialEq)]
pub enum ResourceKind {
    /// Straight-alpha RGBA, row-major.
    Rgba { size: (u32, u32), rgba: Vec<u8> },
    /// An encoded image (PNG, JPEG) the tool decodes.
    Encoded { bytes: Vec<u8> },
    /// A vector path — a traced one, or an app's own symbol that one
    /// path draws.
    Vector { verbs: Vec<Verb>, paint: Paint, ink: Ink, box_size: (f32, f32) },
    /// A symbol by name, tinted. A tool draws the framework's own from
    /// the name; an app's own comes by name just before it comes whole
    /// ([`ResourceKind::Glyph`]), for a tool that predates the glyph.
    Symbol { name: String, color: Color, forced: bool },
    /// A file's icon, by path — the tool resolves it on its side.
    FileIcon { path: String },
    /// One generation of a live feed.
    Feed { generation: u64, size: (u32, u32), rgba: Vec<u8> },
    /// An app's own symbol whole, tinted: its draws in order, which a tool
    /// paints as the framework paints a glyph — one ink's draws piled in
    /// by MAX and blended once, an ink of their own painted over them.
    /// `forced` spends the draws' own tints. It rides its own message kind,
    /// [`kind::GLYPH`], never `RESOURCE`: a tool that predates it skips it,
    /// and reads the symbol's name sent just before ([`Resource::by_name`]).
    Glyph { name: String, color: Color, forced: bool, draws: Vec<GlyphDraw> },
}

/// One draw of a glyph: a path on the 24-unit icon grid, how it is
/// painted, and its own colour when it keeps one — `None` takes the
/// glyph's ink.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphDraw {
    pub paint: Paint,
    pub verbs: Vec<Verb>,
    pub tint: Option<Color>,
}

/// A resource, by key.
#[derive(Clone, Debug, PartialEq)]
pub struct Resource {
    pub key: u64,
    pub kind: ResourceKind,
}

impl Resource {
    /// What a tool that predates a kind reads in its place, under the same
    /// key: a glyph's symbol by name, which such a tool can at least name.
    /// `None` for every kind v1 reads.
    pub fn by_name(&self) -> Option<Resource> {
        match &self.kind {
            ResourceKind::Glyph { name, color, forced, .. } => Some(Resource {
                key: self.key,
                kind: ResourceKind::Symbol { name: name.clone(), color: *color, forced: *forced },
            }),
            ResourceKind::Rgba { .. }
            | ResourceKind::Encoded { .. }
            | ResourceKind::Vector { .. }
            | ResourceKind::Symbol { .. }
            | ResourceKind::FileIcon { .. }
            | ResourceKind::Feed { .. } => None,
        }
    }
}

// =============================================================================
// Traits and previews
// =============================================================================

/// The colour scheme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Light,
    Dark,
    Both,
}

/// A device frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    IPhone16,
    IPhone16ProMax,
    IPadPro11,
    Pixel9,
    MacWindow,
}

/// How a preview is sized.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sizing {
    Fixed(Size),
    Fits,
    Device(Device),
}

/// The reading direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    LeftToRight,
    RightToLeft,
}

/// The horizontal size class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeClass {
    Compact,
    Regular,
}

/// A preview's traits. A theme travels as its tokens by name, so a tool
/// built against another token set keeps what it knows.
#[derive(Clone, Debug, PartialEq)]
pub struct Traits {
    pub scheme: Scheme,
    pub theme: Option<Vec<(String, Color)>>,
    pub size: Sizing,
    pub locale: Option<String>,
    pub direction: Option<Direction>,
    pub size_class: Option<SizeClass>,
    pub scale: Option<f64>,
    pub safe_area: Option<Edges>,
}

impl Default for Traits {
    fn default() -> Self {
        Traits {
            scheme: Scheme::Light,
            theme: None,
            size: Sizing::Fits,
            locale: None,
            direction: None,
            size_class: None,
            scale: None,
            safe_area: None,
        }
    }
}

/// One preview the host can show.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewInfo {
    /// The tool's handle for it: the file and the ident, `file#ident`.
    pub id: String,
    pub ident: String,
    pub name: String,
    pub scope: String,
    pub example: String,
    pub module: String,
    pub file: String,
    pub line: u32,
    pub traits: Traits,
}

// =============================================================================
// Messages
// =============================================================================

/// What the host sends.
#[derive(Clone, Debug, PartialEq)]
pub enum ToIde {
    /// Mounted identities immediately preceding their exact frame.
    ViewSnapshot(ViewSnapshot),
    /// Selected state metadata, joined to an exact request and frame.
    StateObservation(StateReply),
    /// The complete execution selection is applied; following instance frames use it.
    ExecutionsSelected { request: u64 },
    /// The first message: who is talking.
    Hello {
        wire: u32,
        framework: String,
        pid: u32,
        generation: u32,
        workspace_root: String,
        package: String,
        caps: u32,
    },
    /// The previews of the generation that runs, after each load.
    Ready { generation: u32, previews: Vec<PreviewInfo>, missing: Vec<String> },
    Frame(Frame),
    /// A frame from one independent scene instance.
    InstanceFrame {
        instance: SceneInstanceId,
        frame: Box<Frame>,
    },
    /// A panic confined to one instance variant.
    InstancePanicked {
        instance: SceneInstanceId,
        preview: String,
        variant: u32,
        message: String,
    },
    /// Pixels requested from one instance variant.
    InstanceSnapshot {
        instance: SceneInstanceId,
        preview: String,
        variant: u32,
        size: (u32, u32),
        rgba: Vec<u8>,
    },
    /// An invalid instance selection or command; the previous state is retained.
    InstanceError {
        instance: SceneInstanceId,
        variant: Option<u32>,
        message: String,
    },
    /// The bytes behind an image key, before the first frame that draws it
    /// — on `RESOURCE`, or on `GLYPH` for a glyph, just after its name.
    Resource(Resource),
    Status { text: String },
    /// One preview's pass panicked; the host is alive.
    Panicked { preview: String, message: String },
    /// A panic outside a pass; the host is ending.
    Panic { message: String },
    Snapshot { preview: String, variant: u32, size: (u32, u32), rgba: Vec<u8> },
    Pong { nonce: u64 },
}

/// A preview the tool wants shown, under traits of its choosing.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    pub id: String,
    pub ident: String,
    pub traits: Traits,
}

/// A pointer event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerKind {
    Move,
    Down,
    Up,
    Leave,
}

/// A finger's step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchPhase {
    Began,
    Moved,
    Ended,
    Cancelled,
}

/// A source site a tool names an override by.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SiteKey {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

/// What an override does at its site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverrideOp {
    Replace,
    Append,
    Hide,
}

/// An edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
    Leading,
    Trailing,
}

/// An alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alignment {
    Leading,
    Center,
    Trailing,
    TopLeading,
    Top,
    TopTrailing,
    BottomLeading,
    Bottom,
    BottomTrailing,
}

/// A data-only modifier — what an override may put on a node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Patch {
    Padding,
    PaddingLength(f64),
    PaddingEdge(Edge, f64),
    Frame { width: f64, height: f64, alignment: Alignment },
    FrameMax { width: f64, height: f64, alignment: Alignment },
    FrameWidth(f64, Alignment),
    FrameHeight(f64),
    BackgroundColor(Color),
    ForegroundColor(Color),
    CornerRadius(Corners),
    FontSize(f64),
    Opacity(f64),
    Border(Color, f64),
    Shadow(f64, Color),
    Bold,
    Italic,
    Hidden,
    Blur(f64),
    LineHeight(f64),
    FontWeight(Weight),
    Monospaced,
    Clipped,
    HugHeight,
}

/// Placement modifiers travel separately from the frozen 23-tag `Patch` enum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlacePatch {
    /// Physical coordinates in the enclosing stack.
    Position(Point),
    /// Grid columns and rows; the core normalizes zero to one.
    Span(u8, u8),
}

/// What the tool sends.
#[derive(Clone, Debug, PartialEq)]
pub enum ToHost {
    /// Opt in after negotiating `caps::VIEW_IDENTITIES`; false stops metadata.
    ObserveViews { enabled: bool },
    /// Replace or cancel the selected state watch; requires view observation.
    ObserveState { request: StateRequestId, target: Option<StateTarget> },
    Select(Vec<Selection>),
    /// Replace instance selections after negotiating `caps::INSTANCES`.
    SelectInstances(Vec<InstanceSelection>),
    /// Select initial intent atomically after negotiating `caps::EXECUTION_SELECTION`.
    SelectExecutions {
        /// Opaque caller correlation echoed only after successful application.
        request: u64,
        /// Complete replacement selection for the instance lane.
        selections: Vec<ExecutionSelection>,
    },
    /// Set one scene's execution intent after negotiating `caps::EXECUTION`.
    SetExecution {
        instance: SceneInstanceId,
        variant: u32,
        mode: ExecutionMode,
    },
    /// Address one variant of an independent scene.
    Instance {
        instance: SceneInstanceId,
        variant: u32,
        command: InstanceCommand,
    },
    Retrait {
        id: String,
        traits: Traits,
    },
    Resize {
        id: String,
        size: Size,
    },
    Pointer {
        id: String,
        kind: PointerKind,
        x: f64,
        y: f64,
        modifiers: u32,
    },
    Wheel {
        id: String,
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
    },
    /// A finger on a scene: `touch` names it across its steps, `taps`
    /// is the platform's own count for it.
    Touch { id: String, phase: TouchPhase, touch: u64, x: f64, y: f64, taps: u8 },
    /// Two fingers, or a trackpad, changed their distance over a point
    /// by `scale`.
    Magnify { id: String, x: f64, y: f64, scale: f64 },
    Key { id: String, key: String, modifiers: u32 },
    Text { id: String, text: String },
    /// Explicit paste after negotiating `caps::CLIPBOARD_TEXT`.
    Paste {
        id: String,
        paste: ClipboardPaste,
    },
    Override {
        id: String,
        site: SiteKey,
        op: OverrideOp,
        patch: Patch,
    },
    /// Send only after the host announces `caps::PLACE`.
    Place { id: String, site: SiteKey, op: OverrideOp, patch: PlacePatch },
    ClearOverrides { id: String },
    RequestSnapshot { id: String },
    Forget { keys: Vec<u64> },
    Ping { nonce: u64 },
    Bye,
}

/// Text captured for one explicit paste gesture; never an OS clipboard handle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardPaste {
    /// Original shortcut key, retained for application handler precedence.
    pub key: String,
    /// Original shortcut modifiers using the wire's key bits.
    pub modifiers: u32,
    /// Whole committed UTF-8 text, including its line endings.
    pub text: String,
}
impl ClipboardPaste {
    /// Whether the key/modifier pair is the ordinary text paste shortcut.
    pub fn is_shortcut(key: &str, modifiers: u32) -> bool {
        matches!(key, "v" | "V") && matches!(modifiers, 2 | 8 | 10)
    }
    fn put(&self, out: &mut Vec<u8>) {
        out.string(&self.key);
        out.u32(self.modifiers);
        out.string(&self.text);
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(Self {
            key: c.string()?,
            modifiers: c.u32()?,
            text: c.string()?,
        })
    }
}

/// A message in either direction.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    ToIde(ToIde),
    ToHost(ToHost),
}

/// What a reader hands back: a message, or a kind it does not know,
/// skipped.
#[derive(Clone, Debug, PartialEq)]
pub enum Received {
    Message(Message),
    Unknown { kind: u8, len: u32 },
}

/// Why a frame could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Malformed {
    /// The payload ended before the message did.
    Truncated { kind: u8 },
    /// A tag no variant of `what` answers to.
    BadTag { what: &'static str, tag: u8 },
    /// A string that is not UTF-8.
    BadText,
    /// A scene instance name violates its identity contract.
    InvalidSceneInstance,
    /// Zero cannot identify a mounted view.
    InvalidMountedView,
    /// An observation contains an invalid request, value or duplicate dependency.
    InvalidStateObservation,
    /// Selected observation bounds were exceeded.
    StateObservationLimit,
    /// A length past [`MAX_FRAME`].
    Oversized { len: u32 },
    /// Bytes left over after the message — the payload was not this kind's.
    Trailing { kind: u8, left: usize },
}

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Malformed::Truncated { kind } => write!(f, "a message of kind {kind} ended before its payload did"),
            Malformed::BadTag { what, tag } => write!(f, "no {what} answers to the tag {tag}"),
            Malformed::InvalidStateObservation => f.write_str("invalid state observation"),
            Malformed::StateObservationLimit => f.write_str("state observation exceeds its bounds"),
            Malformed::InvalidMountedView => f.write_str("invalid mounted view identity"),
            Malformed::InvalidSceneInstance => f.write_str("invalid scene instance identity"),
            Malformed::BadText => f.write_str("a string on the wire is not UTF-8"),
            Malformed::Oversized { len } => write!(f, "a frame of {len} bytes is past the wire's limit"),
            Malformed::Trailing { kind, left } => write!(f, "a message of kind {kind} left {left} bytes unread"),
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
        Message::ToIde(message) => message.put(&mut payload),
        Message::ToHost(message) => message.put(&mut payload),
    };
    let mut out = Vec::with_capacity(payload.len() + 5);
    out.extend_from_slice(&(payload.len() as u32 + 1).to_le_bytes());
    out.push(kind);
    out.extend_from_slice(&payload);
    out
}

/// Reads frames out of a byte stream fed in any pieces.
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

    /// The next complete frame, if one has arrived whole.
    pub fn pull(&mut self) -> Result<Option<Received>, Malformed> {
        let rest = &self.buf[self.at..];
        if rest.len() < 5 {
            return Ok(None);
        }
        let len = u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]);
        if len > MAX_FRAME || len == 0 {
            return Err(Malformed::Oversized { len });
        }
        let len = len as usize;
        if rest.len() < 4 + len {
            return Ok(None);
        }
        let kind = rest[4];
        let payload = &rest[5..4 + len];
        let received = decode(kind, payload);
        self.at += 4 + len;
        if self.at == self.buf.len() {
            self.buf.clear();
            self.at = 0;
        }
        received.map(Some)
    }
}

/// One payload, by its kind.
pub fn decode(kind: u8, payload: &[u8]) -> Result<Received, Malformed> {
    let mut cursor = Cursor { bytes: payload, at: 0, kind };
    let message = match kind {
        kind::HELLO..=kind::GLYPH => Message::ToIde(ToIde::get(&mut cursor)?),
        kind::SELECT..=kind::PASTE => Message::ToHost(ToHost::get(&mut cursor)?),
        _ => return Ok(Received::Unknown { kind, len: payload.len() as u32 }),
    };
    if cursor.at != payload.len() {
        return Err(Malformed::Trailing { kind, left: payload.len() - cursor.at });
    }
    Ok(Received::Message(message))
}

/// Reads primitives off a payload.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
    kind: u8,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], Malformed> {
        let end = self.at.checked_add(n).ok_or(Malformed::Truncated { kind: self.kind })?;
        if end > self.bytes.len() {
            return Err(Malformed::Truncated { kind: self.kind });
        }
        let out = &self.bytes[self.at..end];
        self.at = end;
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

    fn u32(&mut self) -> Result<u32, Malformed> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Result<u64, Malformed> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }

    fn f32(&mut self) -> Result<f32, Malformed> {
        let b = self.take(4)?;
        Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn f64(&mut self) -> Result<f64, Malformed> {
        Ok(f64::from_bits(self.u64()?))
    }

    fn len(&mut self) -> Result<usize, Malformed> {
        let len = self.u32()? as usize;
        if len > self.bytes.len() - self.at {
            return Err(Malformed::Truncated { kind: self.kind });
        }
        Ok(len)
    }

    fn bytes(&mut self) -> Result<Vec<u8>, Malformed> {
        let len = self.len()?;
        Ok(self.take(len)?.to_vec())
    }

    fn string(&mut self) -> Result<String, Malformed> {
        let len = self.len()?;
        String::from_utf8(self.take(len)?.to_vec()).map_err(|_| Malformed::BadText)
    }

    fn option<T>(&mut self, get: impl FnOnce(&mut Self) -> Result<T, Malformed>) -> Result<Option<T>, Malformed> {
        match self.u8()? {
            0 => Ok(None),
            1 => get(self).map(Some),
            tag => Err(Malformed::BadTag { what: "option", tag }),
        }
    }

    fn list<T>(&mut self, mut get: impl FnMut(&mut Self) -> Result<T, Malformed>) -> Result<Vec<T>, Malformed> {
        let count = self.u32()? as usize;
        // a count is bounded by the bytes left: every item takes at least one
        if count > self.bytes.len() - self.at {
            return Err(Malformed::Truncated { kind: self.kind });
        }
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            out.push(get(self)?);
        }
        Ok(out)
    }
}

/// Writes primitives onto a payload.
trait Put {
    fn u8(&mut self, value: u8);
    fn bool(&mut self, value: bool);
    fn u32(&mut self, value: u32);
    fn u64(&mut self, value: u64);
    fn f32(&mut self, value: f32);
    fn f64(&mut self, value: f64);
    fn bytes(&mut self, value: &[u8]);
    fn string(&mut self, value: &str);
    fn option<T>(&mut self, value: Option<&T>, put: impl FnOnce(&mut Self, &T));
    fn list<T>(&mut self, items: &[T], put: impl FnMut(&mut Self, &T));
}

impl Put for Vec<u8> {
    fn u8(&mut self, value: u8) {
        self.push(value);
    }
    fn bool(&mut self, value: bool) {
        self.push(u8::from(value));
    }
    fn u32(&mut self, value: u32) {
        self.extend_from_slice(&value.to_le_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.extend_from_slice(&value.to_le_bytes());
    }
    fn f32(&mut self, value: f32) {
        self.extend_from_slice(&value.to_le_bytes());
    }
    fn f64(&mut self, value: f64) {
        self.extend_from_slice(&value.to_bits().to_le_bytes());
    }
    fn bytes(&mut self, value: &[u8]) {
        self.u32(value.len() as u32);
        self.extend_from_slice(value);
    }
    fn string(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }
    fn option<T>(&mut self, value: Option<&T>, put: impl FnOnce(&mut Self, &T)) {
        match value {
            None => self.push(0),
            Some(value) => {
                self.push(1);
                put(self, value);
            }
        }
    }
    fn list<T>(&mut self, items: &[T], mut put: impl FnMut(&mut Self, &T)) {
        self.u32(items.len() as u32);
        for item in items {
            put(self, item);
        }
    }
}

/// A type with a wire shape.
trait Wire: Sized {
    fn put(&self, out: &mut Vec<u8>);
    fn get(cursor: &mut Cursor<'_>) -> Result<Self, Malformed>;
}

macro_rules! tagged_enum {
    ($name:ident { $($variant:ident = $tag:expr),+ $(,)? }) => {
        impl Wire for $name {
            fn put(&self, out: &mut Vec<u8>) {
                out.u8(match self { $($name::$variant => $tag,)+ });
            }
            fn get(cursor: &mut Cursor<'_>) -> Result<Self, Malformed> {
                match cursor.u8()? {
                    $($tag => Ok($name::$variant),)+
                    tag => Err(Malformed::BadTag { what: stringify!($name), tag }),
                }
            }
        }
    };
}

// the light end joined after the freeze: tags 6–8 follow the six the
// wire was born with, and no tag moved
tagged_enum!(Weight {
    Regular = 0,
    Medium = 1,
    Semibold = 2,
    Bold = 3,
    ExtraBold = 4,
    Black = 5,
    Thin = 6,
    ExtraLight = 7,
    Light = 8,
});
tagged_enum!(Design { Default = 0, Mono = 1 });
tagged_enum!(Slant { Upright = 0, Italic = 1 });
tagged_enum!(Scheme { Light = 0, Dark = 1, Both = 2 });
tagged_enum!(Device { IPhone16 = 0, IPhone16ProMax = 1, IPadPro11 = 2, Pixel9 = 3, MacWindow = 4 });
tagged_enum!(Direction { LeftToRight = 0, RightToLeft = 1 });
tagged_enum!(SizeClass { Compact = 0, Regular = 1 });
tagged_enum!(PointerKind { Move = 0, Down = 1, Up = 2, Leave = 3 });
tagged_enum!(TouchPhase { Began = 0, Moved = 1, Ended = 2, Cancelled = 3 });
tagged_enum!(OverrideOp { Replace = 0, Append = 1, Hide = 2 });
tagged_enum!(Edge { Top = 0, Bottom = 1, Leading = 2, Trailing = 3 });
tagged_enum!(Alignment {
    Leading = 0,
    Center = 1,
    Trailing = 2,
    TopLeading = 3,
    Top = 4,
    TopTrailing = 5,
    BottomLeading = 6,
    Bottom = 7,
    BottomTrailing = 8,
});

impl Wire for Point {
    fn put(&self, out: &mut Vec<u8>) {
        out.f64(self.x);
        out.f64(self.y);
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(Point { x: c.f64()?, y: c.f64()? })
    }
}

impl Wire for Size {
    fn put(&self, out: &mut Vec<u8>) {
        out.f64(self.width);
        out.f64(self.height);
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(Size { width: c.f64()?, height: c.f64()? })
    }
}

impl Wire for Rect {
    fn put(&self, out: &mut Vec<u8>) {
        self.origin.put(out);
        self.size.put(out);
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(Rect { origin: Point::get(c)?, size: Size::get(c)? })
    }
}

impl Wire for Color {
    fn put(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&[self.r, self.g, self.b, self.a]);
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        let b = c.take(4)?;
        Ok(Color { r: b[0], g: b[1], b: b[2], a: b[3] })
    }
}

impl Wire for Corners {
    fn put(&self, out: &mut Vec<u8>) {
        out.f64(self.top_left);
        out.f64(self.top_right);
        out.f64(self.bottom_right);
        out.f64(self.bottom_left);
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(Corners { top_left: c.f64()?, top_right: c.f64()?, bottom_right: c.f64()?, bottom_left: c.f64()? })
    }
}

impl Wire for Edges {
    fn put(&self, out: &mut Vec<u8>) {
        out.f64(self.top);
        out.f64(self.trailing);
        out.f64(self.bottom);
        out.f64(self.leading);
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(Edges { top: c.f64()?, trailing: c.f64()?, bottom: c.f64()?, leading: c.f64()? })
    }
}

impl Wire for GradientPaint {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            GradientPaint::Radial { center, start, end, aspect, inner, outer } => {
                out.u8(0);
                center.put(out);
                out.f64(*start);
                out.f64(*end);
                out.f64(*aspect);
                inner.put(out);
                outer.put(out);
            }
            GradientPaint::Linear { start, end, from, to } => {
                out.u8(1);
                start.put(out);
                end.put(out);
                from.put(out);
                to.put(out);
            }
        }
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        match c.u8()? {
            0 => Ok(GradientPaint::Radial {
                center: Point::get(c)?,
                start: c.f64()?,
                end: c.f64()?,
                aspect: c.f64()?,
                inner: Color::get(c)?,
                outer: Color::get(c)?,
            }),
            1 => Ok(GradientPaint::Linear { start: Point::get(c)?, end: Point::get(c)?, from: Color::get(c)?, to: Color::get(c)? }),
            tag => Err(Malformed::BadTag { what: "GradientPaint", tag }),
        }
    }
}

impl Wire for Gradient {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            Gradient::Radial { center, start, end, aspect, inner, outer } => {
                out.u8(0);
                center.put(out);
                out.f64(*start);
                out.option(end.as_ref(), |out, end| out.f64(*end));
                out.f64(*aspect);
                inner.put(out);
                outer.put(out);
            }
            Gradient::Linear { start, end, from, to } => {
                out.u8(1);
                start.put(out);
                end.put(out);
                from.put(out);
                to.put(out);
            }
        }
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        match c.u8()? {
            0 => Ok(Gradient::Radial {
                center: Point::get(c)?,
                start: c.f64()?,
                end: c.option(|c| c.f64())?,
                aspect: c.f64()?,
                inner: Color::get(c)?,
                outer: Color::get(c)?,
            }),
            1 => Ok(Gradient::Linear { start: Point::get(c)?, end: Point::get(c)?, from: Color::get(c)?, to: Color::get(c)? }),
            tag => Err(Malformed::BadTag { what: "Gradient", tag }),
        }
    }
}

impl Wire for Glass {
    fn put(&self, out: &mut Vec<u8>) {
        out.f64(self.blur);
        self.tint.put(out);
        out.f64(self.refraction_band);
        out.f64(self.refraction_amount);
        out.f64(self.chromatic);
        self.highlight.put(out);
        out.f64(self.highlight_band);
        out.f64(self.highlight_intensity);
        out.f64(self.saturation);
        out.f64(self.brightness);
        out.f64(self.sheen);
        self.spot_center.put(out);
        out.f64(self.spot_radius);
        out.f64(self.spot_alpha);
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(Glass {
            blur: c.f64()?,
            tint: Color::get(c)?,
            refraction_band: c.f64()?,
            refraction_amount: c.f64()?,
            chromatic: c.f64()?,
            highlight: Color::get(c)?,
            highlight_band: c.f64()?,
            highlight_intensity: c.f64()?,
            saturation: c.f64()?,
            brightness: c.f64()?,
            sheen: c.f64()?,
            spot_center: Point::get(c)?,
            spot_radius: c.f64()?,
            spot_alpha: c.f64()?,
        })
    }
}

impl Wire for Font {
    fn put(&self, out: &mut Vec<u8>) {
        out.f64(self.size);
        self.weight.put(out);
        self.design.put(out);
        self.slant.put(out);
        out.option(self.family.as_ref(), |out, family| out.u32(*family));
        out.f64(self.tracking);
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(Font {
            size: c.f64()?,
            weight: Weight::get(c)?,
            design: Design::get(c)?,
            slant: Slant::get(c)?,
            family: c.option(|c| c.u32())?,
            tracking: c.f64()?,
        })
    }
}

impl Wire for ImageRef {
    fn put(&self, out: &mut Vec<u8>) {
        out.u64(self.key);
        out.u8(self.alpha);
        out.bool(self.mirrored);
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(ImageRef { key: c.u64()?, alpha: c.u8()?, mirrored: c.bool()? })
    }
}

impl Wire for Draw {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            Draw::FillRect { rect, color, corners } => {
                out.u8(0);
                rect.put(out);
                color.put(out);
                corners.put(out);
            }
            Draw::Gradient { rect, paint, corners } => {
                out.u8(1);
                rect.put(out);
                paint.put(out);
                corners.put(out);
            }
            Draw::Backdrop { rect, glass, corners } => {
                out.u8(2);
                rect.put(out);
                glass.put(out);
                corners.put(out);
            }
            Draw::Shadow { rect, radius, color, corners } => {
                out.u8(3);
                rect.put(out);
                out.f64(*radius);
                color.put(out);
                corners.put(out);
            }
            Draw::StrokeRect { rect, color, width, corners } => {
                out.u8(4);
                rect.put(out);
                color.put(out);
                out.f64(*width);
                corners.put(out);
            }
            Draw::TextLine { origin, text, range, color, font } => {
                out.u8(5);
                origin.put(out);
                out.u32(*text);
                out.u32(range.0);
                out.u32(range.1);
                color.put(out);
                font.put(out);
            }
            Draw::Image { rect, image, subpixel } => {
                out.u8(if *subpixel { 9 } else { 6 });
                rect.put(out);
                image.put(out);
            }
            Draw::PushClip { rect, corners } => {
                out.u8(7);
                rect.put(out);
                corners.put(out);
            }
            Draw::PopClip => out.u8(8),
        }
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(match c.u8()? {
            0 => Draw::FillRect { rect: Rect::get(c)?, color: Color::get(c)?, corners: Corners::get(c)? },
            1 => Draw::Gradient { rect: Rect::get(c)?, paint: GradientPaint::get(c)?, corners: Corners::get(c)? },
            2 => Draw::Backdrop { rect: Rect::get(c)?, glass: Glass::get(c)?, corners: Corners::get(c)? },
            3 => Draw::Shadow { rect: Rect::get(c)?, radius: c.f64()?, color: Color::get(c)?, corners: Corners::get(c)? },
            4 => Draw::StrokeRect { rect: Rect::get(c)?, color: Color::get(c)?, width: c.f64()?, corners: Corners::get(c)? },
            5 => Draw::TextLine {
                origin: Point::get(c)?,
                text: c.u32()?,
                range: (c.u32()?, c.u32()?),
                color: Color::get(c)?,
                font: Font::get(c)?,
            },
            6 => Draw::Image { rect: Rect::get(c)?, image: ImageRef::get(c)?, subpixel: false },
            7 => Draw::PushClip { rect: Rect::get(c)?, corners: Corners::get(c)? },
            8 => Draw::PopClip,
            9 => Draw::Image { rect: Rect::get(c)?, image: ImageRef::get(c)?, subpixel: true },
            tag => return Err(Malformed::BadTag { what: "Draw", tag }),
        })
    }
}

impl Wire for SiteBox {
    fn put(&self, out: &mut Vec<u8>) {
        out.u32(self.index);
        out.option(self.parent.as_ref(), |out, parent| out.u32(*parent));
        out.u32(self.kind);
        out.option(self.modifier.as_ref(), |out, modifier| out.u32(*modifier));
        out.option(self.file.as_ref(), |out, file| out.u32(*file));
        out.option(self.name.as_ref(), |out, name| out.u32(*name));
        out.u32(self.line);
        out.u32(self.column);
        self.frame.put(out);
        out.option(self.path.as_ref(), |out, path| out.u32(*path));
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(SiteBox {
            index: c.u32()?,
            parent: c.option(|c| c.u32())?,
            kind: c.u32()?,
            modifier: c.option(|c| c.u32())?,
            file: c.option(|c| c.u32())?,
            name: c.option(|c| c.u32())?,
            line: c.u32()?,
            column: c.u32()?,
            frame: Rect::get(c)?,
            path: c.option(|c| c.u32())?,
        })
    }
}

impl Wire for HostBox {
    fn put(&self, out: &mut Vec<u8>) {
        out.u32(self.index);
        out.u32(self.kind);
        self.frame.put(out);
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(HostBox { index: c.u32()?, kind: c.u32()?, frame: Rect::get(c)? })
    }
}

impl Wire for Metrics {
    fn put(&self, out: &mut Vec<u8>) {
        out.f64(self.build_ms);
        out.f64(self.layout_ms);
        out.u32(self.body_passes);
        out.u32(self.commands);
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(Metrics { build_ms: c.f64()?, layout_ms: c.f64()?, body_passes: c.u32()?, commands: c.u32()? })
    }
}

impl Wire for Frame {
    fn put(&self, out: &mut Vec<u8>) {
        out.string(&self.preview);
        out.u32(self.variant);
        out.u64(self.seq);
        out.u32(self.generation);
        self.size.put(out);
        out.f64(self.scale);
        self.background.put(out);
        out.list(&self.strings, |out, s| out.string(s));
        out.list(&self.commands, |out, d| d.put(out));
        out.list(&self.sites, |out, s| s.put(out));
        out.list(&self.hosts, |out, h| h.put(out));
        self.metrics.put(out);
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(Frame {
            preview: c.string()?,
            variant: c.u32()?,
            seq: c.u64()?,
            generation: c.u32()?,
            size: Size::get(c)?,
            scale: c.f64()?,
            background: Color::get(c)?,
            strings: c.list(|c| c.string())?,
            commands: c.list(Draw::get)?,
            sites: c.list(SiteBox::get)?,
            hosts: c.list(HostBox::get)?,
            metrics: Metrics::get(c)?,
        })
    }
}

impl Wire for Verb {
    fn put(&self, out: &mut Vec<u8>) {
        match *self {
            Verb::Move(x, y) => {
                out.u8(0);
                out.f32(x);
                out.f32(y);
            }
            Verb::Line(x, y) => {
                out.u8(1);
                out.f32(x);
                out.f32(y);
            }
            Verb::Quad(a, b, x, y) => {
                out.u8(2);
                out.f32(a);
                out.f32(b);
                out.f32(x);
                out.f32(y);
            }
            Verb::Cubic(a, b, c2, d, x, y) => {
                out.u8(3);
                out.f32(a);
                out.f32(b);
                out.f32(c2);
                out.f32(d);
                out.f32(x);
                out.f32(y);
            }
            Verb::Close => out.u8(4),
        }
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(match c.u8()? {
            0 => Verb::Move(c.f32()?, c.f32()?),
            1 => Verb::Line(c.f32()?, c.f32()?),
            2 => Verb::Quad(c.f32()?, c.f32()?, c.f32()?, c.f32()?),
            3 => Verb::Cubic(c.f32()?, c.f32()?, c.f32()?, c.f32()?, c.f32()?, c.f32()?),
            4 => Verb::Close,
            tag => return Err(Malformed::BadTag { what: "Verb", tag }),
        })
    }
}

impl Wire for Paint {
    fn put(&self, out: &mut Vec<u8>) {
        match *self {
            Paint::Fill { even_odd } => {
                out.u8(0);
                out.bool(even_odd);
            }
            Paint::Stroke { width } => {
                out.u8(1);
                out.f32(width);
            }
        }
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(match c.u8()? {
            0 => Paint::Fill { even_odd: c.bool()? },
            1 => Paint::Stroke { width: c.f32()? },
            tag => return Err(Malformed::BadTag { what: "Paint", tag }),
        })
    }
}

impl Wire for Ink {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            Ink::Solid(color) => {
                out.u8(0);
                color.put(out);
            }
            Ink::Ramp(gradient) => {
                out.u8(1);
                gradient.put(out);
            }
        }
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(match c.u8()? {
            0 => Ink::Solid(Color::get(c)?),
            1 => Ink::Ramp(Gradient::get(c)?),
            tag => return Err(Malformed::BadTag { what: "Ink", tag }),
        })
    }
}

impl Wire for Resource {
    fn put(&self, out: &mut Vec<u8>) {
        out.u64(self.key);
        match &self.kind {
            ResourceKind::Rgba { size, rgba } => {
                out.u8(0);
                out.u32(size.0);
                out.u32(size.1);
                out.bytes(rgba);
            }
            ResourceKind::Encoded { bytes } => {
                out.u8(1);
                out.bytes(bytes);
            }
            ResourceKind::Vector { verbs, paint, ink, box_size } => {
                out.u8(2);
                out.list(verbs, |out, v| v.put(out));
                paint.put(out);
                ink.put(out);
                out.f32(box_size.0);
                out.f32(box_size.1);
            }
            ResourceKind::Symbol { name, color, forced } => {
                out.u8(3);
                out.string(name);
                color.put(out);
                out.bool(*forced);
            }
            ResourceKind::FileIcon { path } => {
                out.u8(4);
                out.string(path);
            }
            ResourceKind::Feed { generation, size, rgba } => {
                out.u8(5);
                out.u64(*generation);
                out.u32(size.0);
                out.u32(size.1);
                out.bytes(rgba);
            }
            // its own kind says what it is: no tag, and never under RESOURCE
            ResourceKind::Glyph { name, color, forced, draws } => {
                out.string(name);
                color.put(out);
                out.bool(*forced);
                out.list(draws, |out, draw| draw.put(out));
            }
        }
    }
    /// The `RESOURCE` kind: v1's kinds, by tag. A glyph comes under its
    /// own kind ([`Resource::get_glyph`]).
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        let key = c.u64()?;
        let kind = match c.u8()? {
            0 => ResourceKind::Rgba { size: (c.u32()?, c.u32()?), rgba: c.bytes()? },
            1 => ResourceKind::Encoded { bytes: c.bytes()? },
            2 => ResourceKind::Vector { verbs: c.list(Verb::get)?, paint: Paint::get(c)?, ink: Ink::get(c)?, box_size: (c.f32()?, c.f32()?) },
            3 => ResourceKind::Symbol { name: c.string()?, color: Color::get(c)?, forced: c.bool()? },
            4 => ResourceKind::FileIcon { path: c.string()? },
            5 => ResourceKind::Feed { generation: c.u64()?, size: (c.u32()?, c.u32()?), rgba: c.bytes()? },
            tag => return Err(Malformed::BadTag { what: "ResourceKind", tag }),
        };
        Ok(Resource { key, kind })
    }
}

impl Resource {
    /// The `GLYPH` kind: a key and a glyph, untagged.
    fn get_glyph(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(Resource {
            key: c.u64()?,
            kind: ResourceKind::Glyph { name: c.string()?, color: Color::get(c)?, forced: c.bool()?, draws: c.list(GlyphDraw::get)? },
        })
    }
}

impl Wire for GlyphDraw {
    fn put(&self, out: &mut Vec<u8>) {
        self.paint.put(out);
        out.list(&self.verbs, |out, verb| verb.put(out));
        out.option(self.tint.as_ref(), |out, tint| tint.put(out));
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(GlyphDraw { paint: Paint::get(c)?, verbs: c.list(Verb::get)?, tint: c.option(Color::get)? })
    }
}

impl Wire for Sizing {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            Sizing::Fixed(size) => {
                out.u8(0);
                size.put(out);
            }
            Sizing::Fits => out.u8(1),
            Sizing::Device(device) => {
                out.u8(2);
                device.put(out);
            }
        }
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(match c.u8()? {
            0 => Sizing::Fixed(Size::get(c)?),
            1 => Sizing::Fits,
            2 => Sizing::Device(Device::get(c)?),
            tag => return Err(Malformed::BadTag { what: "Sizing", tag }),
        })
    }
}

impl Wire for Traits {
    fn put(&self, out: &mut Vec<u8>) {
        self.scheme.put(out);
        out.option(self.theme.as_ref(), |out, tokens| {
            out.list(tokens, |out, (name, color)| {
                out.string(name);
                color.put(out);
            });
        });
        self.size.put(out);
        out.option(self.locale.as_ref(), |out, locale| out.string(locale));
        out.option(self.direction.as_ref(), |out, direction| direction.put(out));
        out.option(self.size_class.as_ref(), |out, class| class.put(out));
        out.option(self.scale.as_ref(), |out, scale| out.f64(*scale));
        out.option(self.safe_area.as_ref(), |out, edges| edges.put(out));
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(Traits {
            scheme: Scheme::get(c)?,
            theme: c.option(|c| c.list(|c| Ok((c.string()?, Color::get(c)?))))?,
            size: Sizing::get(c)?,
            locale: c.option(|c| c.string())?,
            direction: c.option(Direction::get)?,
            size_class: c.option(SizeClass::get)?,
            scale: c.option(|c| c.f64())?,
            safe_area: c.option(Edges::get)?,
        })
    }
}

impl Wire for PreviewInfo {
    fn put(&self, out: &mut Vec<u8>) {
        out.string(&self.id);
        out.string(&self.ident);
        out.string(&self.name);
        out.string(&self.scope);
        out.string(&self.example);
        out.string(&self.module);
        out.string(&self.file);
        out.u32(self.line);
        self.traits.put(out);
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(PreviewInfo {
            id: c.string()?,
            ident: c.string()?,
            name: c.string()?,
            scope: c.string()?,
            example: c.string()?,
            module: c.string()?,
            file: c.string()?,
            line: c.u32()?,
            traits: Traits::get(c)?,
        })
    }
}

impl Wire for Selection {
    fn put(&self, out: &mut Vec<u8>) {
        out.string(&self.id);
        out.string(&self.ident);
        self.traits.put(out);
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(Selection { id: c.string()?, ident: c.string()?, traits: Traits::get(c)? })
    }
}

impl Wire for SiteKey {
    fn put(&self, out: &mut Vec<u8>) {
        out.string(&self.file);
        out.u32(self.line);
        out.u32(self.column);
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(SiteKey { file: c.string()?, line: c.u32()?, column: c.u32()? })
    }
}

impl Wire for PlacePatch {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            Self::Position(point) => {
                out.u8(0);
                point.put(out);
            }
            Self::Span(columns, rows) => {
                out.u8(1);
                out.u8(*columns);
                out.u8(*rows);
            }
        }
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(match c.u8()? {
            0 => Self::Position(Point::get(c)?),
            1 => Self::Span(c.u8()?, c.u8()?),
            tag => return Err(Malformed::BadTag { what: "PlacePatch", tag }),
        })
    }
}

impl Wire for Patch {
    fn put(&self, out: &mut Vec<u8>) {
        match *self {
            Patch::Padding => out.u8(0),
            Patch::PaddingLength(length) => {
                out.u8(1);
                out.f64(length);
            }
            Patch::PaddingEdge(edge, length) => {
                out.u8(2);
                edge.put(out);
                out.f64(length);
            }
            Patch::Frame { width, height, alignment } => {
                out.u8(3);
                out.f64(width);
                out.f64(height);
                alignment.put(out);
            }
            Patch::FrameMax { width, height, alignment } => {
                out.u8(4);
                out.f64(width);
                out.f64(height);
                alignment.put(out);
            }
            Patch::FrameWidth(width, alignment) => {
                out.u8(5);
                out.f64(width);
                alignment.put(out);
            }
            Patch::FrameHeight(height) => {
                out.u8(6);
                out.f64(height);
            }
            Patch::BackgroundColor(color) => {
                out.u8(7);
                color.put(out);
            }
            Patch::ForegroundColor(color) => {
                out.u8(8);
                color.put(out);
            }
            Patch::CornerRadius(corners) => {
                out.u8(9);
                corners.put(out);
            }
            Patch::FontSize(size) => {
                out.u8(10);
                out.f64(size);
            }
            Patch::Opacity(opacity) => {
                out.u8(11);
                out.f64(opacity);
            }
            Patch::Border(color, width) => {
                out.u8(12);
                color.put(out);
                out.f64(width);
            }
            Patch::Shadow(radius, color) => {
                out.u8(13);
                out.f64(radius);
                color.put(out);
            }
            Patch::Bold => out.u8(14),
            Patch::Italic => out.u8(15),
            Patch::Hidden => out.u8(16),
            Patch::Blur(radius) => {
                out.u8(17);
                out.f64(radius);
            }
            Patch::LineHeight(height) => {
                out.u8(18);
                out.f64(height);
            }
            Patch::FontWeight(weight) => {
                out.u8(19);
                weight.put(out);
            }
            Patch::Monospaced => out.u8(20),
            Patch::Clipped => out.u8(21),
            Patch::HugHeight => out.u8(22),
        }
    }
    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(match c.u8()? {
            0 => Patch::Padding,
            1 => Patch::PaddingLength(c.f64()?),
            2 => Patch::PaddingEdge(Edge::get(c)?, c.f64()?),
            3 => Patch::Frame { width: c.f64()?, height: c.f64()?, alignment: Alignment::get(c)? },
            4 => Patch::FrameMax { width: c.f64()?, height: c.f64()?, alignment: Alignment::get(c)? },
            5 => Patch::FrameWidth(c.f64()?, Alignment::get(c)?),
            6 => Patch::FrameHeight(c.f64()?),
            7 => Patch::BackgroundColor(Color::get(c)?),
            8 => Patch::ForegroundColor(Color::get(c)?),
            9 => Patch::CornerRadius(Corners::get(c)?),
            10 => Patch::FontSize(c.f64()?),
            11 => Patch::Opacity(c.f64()?),
            12 => Patch::Border(Color::get(c)?, c.f64()?),
            13 => Patch::Shadow(c.f64()?, Color::get(c)?),
            14 => Patch::Bold,
            15 => Patch::Italic,
            16 => Patch::Hidden,
            17 => Patch::Blur(c.f64()?),
            18 => Patch::LineHeight(c.f64()?),
            19 => Patch::FontWeight(Weight::get(c)?),
            20 => Patch::Monospaced,
            21 => Patch::Clipped,
            22 => Patch::HugHeight,
            tag => return Err(Malformed::BadTag { what: "Patch", tag }),
        })
    }
}

impl ToIde {
    /// Writes the payload; answers the kind.
    fn put(&self, out: &mut Vec<u8>) -> u8 {
        match self {
            ToIde::StateObservation(reply) => { reply.put(out); kind::STATE_OBSERVATION }
            ToIde::ViewSnapshot(snapshot) => {
                snapshot.put(out);
                kind::VIEW_SNAPSHOT
            }
            ToIde::ExecutionsSelected { request } => {
                out.u64(*request);
                kind::EXECUTIONS_SELECTED
            }
            ToIde::Hello { wire, framework, pid, generation, workspace_root, package, caps } => {
                out.extend_from_slice(&MAGIC);
                out.u32(*wire);
                out.string(framework);
                out.u32(*pid);
                out.u32(*generation);
                out.string(workspace_root);
                out.string(package);
                out.u32(*caps);
                kind::HELLO
            }
            ToIde::Ready { generation, previews, missing } => {
                out.u32(*generation);
                out.list(previews, |out, p| p.put(out));
                out.list(missing, |out, m| out.string(m));
                kind::READY
            }
            ToIde::Frame(frame) => {
                frame.put(out);
                kind::FRAME
            }
            ToIde::InstanceFrame { instance, frame } => {
                instance.put(out);
                frame.put(out);
                kind::INSTANCE_FRAME
            }
            ToIde::InstancePanicked {
                instance,
                preview,
                variant,
                message,
            } => {
                instance.put(out);
                out.string(preview);
                out.u32(*variant);
                out.string(message);
                kind::INSTANCE_PANICKED
            }
            ToIde::InstanceSnapshot {
                instance,
                preview,
                variant,
                size,
                rgba,
            } => {
                instance.put(out);
                out.string(preview);
                out.u32(*variant);
                out.u32(size.0);
                out.u32(size.1);
                out.bytes(rgba);
                kind::INSTANCE_SNAPSHOT
            }
            ToIde::InstanceError {
                instance,
                variant,
                message,
            } => {
                instance.put(out);
                out.option(variant.as_ref(), |out, v| out.u32(*v));
                out.string(message);
                kind::INSTANCE_ERROR
            }
            ToIde::Resource(resource) => {
                resource.put(out);
                match resource.kind {
                    // v1's kinds, which every tool reads
                    ResourceKind::Rgba { .. }
                    | ResourceKind::Encoded { .. }
                    | ResourceKind::Vector { .. }
                    | ResourceKind::Symbol { .. }
                    | ResourceKind::FileIcon { .. }
                    | ResourceKind::Feed { .. } => kind::RESOURCE,
                    // its own kind, which a tool that predates it skips: a
                    // new tag under RESOURCE would end that tool's connection
                    ResourceKind::Glyph { .. } => kind::GLYPH,
                }
            }
            ToIde::Status { text } => {
                out.string(text);
                kind::STATUS
            }
            ToIde::Panicked { preview, message } => {
                out.string(preview);
                out.string(message);
                kind::PANICKED
            }
            ToIde::Panic { message } => {
                out.string(message);
                kind::PANIC
            }
            ToIde::Snapshot { preview, variant, size, rgba } => {
                out.string(preview);
                out.u32(*variant);
                out.u32(size.0);
                out.u32(size.1);
                out.bytes(rgba);
                kind::SNAPSHOT
            }
            ToIde::Pong { nonce } => {
                out.u64(*nonce);
                kind::PONG
            }
        }
    }

    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(match c.kind {
            kind::STATE_OBSERVATION => ToIde::StateObservation(StateReply::get(c)?),
            kind::VIEW_SNAPSHOT => ToIde::ViewSnapshot(ViewSnapshot::get(c)?),
            kind::EXECUTIONS_SELECTED => ToIde::ExecutionsSelected { request: c.u64()? },
            kind::HELLO => {
                let magic = c.take(4)?;
                if magic != MAGIC {
                    return Err(Malformed::BadTag { what: "magic", tag: magic[0] });
                }
                ToIde::Hello {
                    wire: c.u32()?,
                    framework: c.string()?,
                    pid: c.u32()?,
                    generation: c.u32()?,
                    workspace_root: c.string()?,
                    package: c.string()?,
                    caps: c.u32()?,
                }
            }
            kind::READY => ToIde::Ready { generation: c.u32()?, previews: c.list(PreviewInfo::get)?, missing: c.list(|c| c.string())? },
            kind::FRAME => ToIde::Frame(Frame::get(c)?),
            kind::INSTANCE_FRAME => ToIde::InstanceFrame {
                instance: SceneInstanceId::get(c)?,
                frame: Box::new(Frame::get(c)?),
            },
            kind::INSTANCE_PANICKED => ToIde::InstancePanicked {
                instance: SceneInstanceId::get(c)?,
                preview: c.string()?,
                variant: c.u32()?,
                message: c.string()?,
            },
            kind::INSTANCE_SNAPSHOT => ToIde::InstanceSnapshot {
                instance: SceneInstanceId::get(c)?,
                preview: c.string()?,
                variant: c.u32()?,
                size: (c.u32()?, c.u32()?),
                rgba: c.bytes()?,
            },
            kind::INSTANCE_ERROR => ToIde::InstanceError {
                instance: SceneInstanceId::get(c)?,
                variant: c.option(|c| c.u32())?,
                message: c.string()?,
            },
            kind::RESOURCE => ToIde::Resource(Resource::get(c)?),
            kind::GLYPH => ToIde::Resource(Resource::get_glyph(c)?),
            kind::STATUS => ToIde::Status { text: c.string()? },
            kind::PANICKED => ToIde::Panicked { preview: c.string()?, message: c.string()? },
            kind::PANIC => ToIde::Panic { message: c.string()? },
            kind::SNAPSHOT => ToIde::Snapshot { preview: c.string()?, variant: c.u32()?, size: (c.u32()?, c.u32()?), rgba: c.bytes()? },
            kind::PONG => ToIde::Pong { nonce: c.u64()? },
            tag => return Err(Malformed::BadTag { what: "ToIde", tag }),
        })
    }
}

impl ToHost {
    /// Writes the payload; answers the kind.
    fn put(&self, out: &mut Vec<u8>) -> u8 {
        match self {
            ToHost::ObserveState { request, target } => {
                out.u64(request.get()); out.option(target.as_ref(), |out, target| target.put(out)); kind::OBSERVE_STATE
            }
            ToHost::ObserveViews { enabled } => {
                out.bool(*enabled);
                kind::OBSERVE_VIEWS
            }
            ToHost::SelectExecutions { request, selections } => {
                out.u64(*request);
                out.list(selections, |out, s| s.put(out));
                kind::SELECT_EXECUTIONS
            }
            ToHost::SetExecution { instance, variant, mode } => {
                instance.put(out);
                out.u32(*variant);
                mode.put(out);
                kind::SET_EXECUTION
            }
            ToHost::SelectInstances(selections) => {
                out.list(selections, |out, s| s.put(out));
                kind::SELECT_INSTANCES
            }
            ToHost::Instance {
                instance,
                variant,
                command,
            } => {
                instance.put(out);
                out.u32(*variant);
                command.put(out);
                kind::INSTANCE
            }
            ToHost::Select(selections) => {
                out.list(selections, |out, s| s.put(out));
                kind::SELECT
            }
            ToHost::Retrait { id, traits } => {
                out.string(id);
                traits.put(out);
                kind::RETRAIT
            }
            ToHost::Resize { id, size } => {
                out.string(id);
                size.put(out);
                kind::RESIZE
            }
            ToHost::Pointer { id, kind: pointer, x, y, modifiers } => {
                out.string(id);
                pointer.put(out);
                out.f64(*x);
                out.f64(*y);
                out.u32(*modifiers);
                kind::POINTER
            }
            ToHost::Wheel { id, x, y, dx, dy } => {
                out.string(id);
                out.f64(*x);
                out.f64(*y);
                out.f64(*dx);
                out.f64(*dy);
                kind::WHEEL
            }
            ToHost::Touch { id, phase, touch, x, y, taps } => {
                out.string(id);
                phase.put(out);
                out.u64(*touch);
                out.f64(*x);
                out.f64(*y);
                out.u8(*taps);
                kind::TOUCH
            }
            ToHost::Magnify { id, x, y, scale } => {
                out.string(id);
                out.f64(*x);
                out.f64(*y);
                out.f64(*scale);
                kind::MAGNIFY
            }
            ToHost::Key { id, key, modifiers } => {
                out.string(id);
                out.string(key);
                out.u32(*modifiers);
                kind::KEY
            }
            ToHost::Paste { id, paste } => {
                out.string(id);
                paste.put(out);
                kind::PASTE
            }
            ToHost::Text { id, text } => {
                out.string(id);
                out.string(text);
                kind::TEXT
            }
            ToHost::Override { id, site, op, patch } => {
                out.string(id);
                site.put(out);
                op.put(out);
                patch.put(out);
                kind::OVERRIDE
            }
            ToHost::Place { id, site, op, patch } => {
                out.string(id);
                site.put(out);
                op.put(out);
                patch.put(out);
                kind::PLACE
            }
            ToHost::ClearOverrides { id } => {
                out.string(id);
                kind::CLEAR_OVERRIDES
            }
            ToHost::RequestSnapshot { id } => {
                out.string(id);
                kind::REQUEST_SNAPSHOT
            }
            ToHost::Forget { keys } => {
                out.list(keys, |out, key| out.u64(*key));
                kind::FORGET
            }
            ToHost::Ping { nonce } => {
                out.u64(*nonce);
                kind::PING
            }
            ToHost::Bye => kind::BYE,
        }
    }

    fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(match c.kind {
            kind::OBSERVE_STATE => ToHost::ObserveState { request: StateRequestId::get_wire(c)?, target: c.option(StateTarget::get)? },
            kind::OBSERVE_VIEWS => ToHost::ObserveViews { enabled: c.bool()? },
            kind::SELECT_EXECUTIONS => ToHost::SelectExecutions { request: c.u64()?, selections: c.list(ExecutionSelection::get)? },
            kind::SET_EXECUTION => ToHost::SetExecution {
                instance: SceneInstanceId::get(c)?,
                variant: c.u32()?,
                mode: ExecutionMode::get(c)?,
            },
            kind::SELECT_INSTANCES => ToHost::SelectInstances(c.list(InstanceSelection::get)?),
            kind::INSTANCE => ToHost::Instance {
                instance: SceneInstanceId::get(c)?,
                variant: c.u32()?,
                command: InstanceCommand::get(c)?,
            },
            kind::SELECT => ToHost::Select(c.list(Selection::get)?),
            kind::RETRAIT => ToHost::Retrait {
                id: c.string()?,
                traits: Traits::get(c)?,
            },
            kind::RESIZE => ToHost::Resize {
                id: c.string()?,
                size: Size::get(c)?,
            },
            kind::POINTER => ToHost::Pointer {
                id: c.string()?,
                kind: PointerKind::get(c)?,
                x: c.f64()?,
                y: c.f64()?,
                modifiers: c.u32()?,
            },
            kind::WHEEL => ToHost::Wheel {
                id: c.string()?,
                x: c.f64()?,
                y: c.f64()?,
                dx: c.f64()?,
                dy: c.f64()?,
            },
            kind::TOUCH => ToHost::Touch {
                id: c.string()?,
                phase: TouchPhase::get(c)?,
                touch: c.u64()?,
                x: c.f64()?,
                y: c.f64()?,
                taps: c.u8()?,
            },
            kind::MAGNIFY => ToHost::Magnify {
                id: c.string()?,
                x: c.f64()?,
                y: c.f64()?,
                scale: c.f64()?,
            },
            kind::KEY => ToHost::Key {
                id: c.string()?,
                key: c.string()?,
                modifiers: c.u32()?,
            },
            kind::TEXT => ToHost::Text {
                id: c.string()?,
                text: c.string()?,
            },
            kind::PASTE => ToHost::Paste {
                id: c.string()?,
                paste: ClipboardPaste::get(c)?,
            },
            kind::OVERRIDE => ToHost::Override {
                id: c.string()?,
                site: SiteKey::get(c)?,
                op: OverrideOp::get(c)?,
                patch: Patch::get(c)?,
            },
            kind::PLACE => ToHost::Place {
                id: c.string()?,
                site: SiteKey::get(c)?,
                op: OverrideOp::get(c)?,
                patch: PlacePatch::get(c)?,
            },
            kind::CLEAR_OVERRIDES => ToHost::ClearOverrides { id: c.string()? },
            kind::REQUEST_SNAPSHOT => ToHost::RequestSnapshot { id: c.string()? },
            kind::FORGET => ToHost::Forget { keys: c.list(|c| c.u64())? },
            kind::PING => ToHost::Ping { nonce: c.u64()? },
            kind::BYE => ToHost::Bye,
            tag => return Err(Malformed::BadTag { what: "ToHost", tag }),
        })
    }
}

/// A string table under construction: each distinct string once.
#[derive(Debug, Default)]
pub struct Strings {
    table: Vec<String>,
    index: std::collections::HashMap<String, u32>,
}

impl Strings {
    pub fn new() -> Strings {
        Strings::default()
    }

    /// The index of `text`, interned.
    pub fn intern(&mut self, text: &str) -> u32 {
        if let Some(index) = self.index.get(text) {
            return *index;
        }
        let index = self.table.len() as u32;
        self.table.push(text.to_string());
        self.index.insert(text.to_string(), index);
        index
    }

    /// The table, in index order.
    pub fn into_table(self) -> Vec<String> {
        self.table
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_fed_in_pieces_comes_out_whole() {
        let message = Message::ToHost(ToHost::Ping { nonce: 7 });
        let bytes = encode(&message);
        let mut reader = Reader::new();
        for byte in &bytes[..bytes.len() - 1] {
            reader.feed(&[*byte]);
            assert_eq!(reader.pull().unwrap(), None);
        }
        reader.feed(&bytes[bytes.len() - 1..]);
        assert_eq!(reader.pull().unwrap(), Some(Received::Message(message)));
        assert_eq!(reader.pull().unwrap(), None);
    }

    #[test]
    fn an_unknown_kind_is_skipped_and_a_cut_payload_is_malformed() {
        let mut reader = Reader::new();
        reader.feed(&[3, 0, 0, 0, 200, 9, 9]);
        reader.feed(&encode(&Message::ToHost(ToHost::Bye)));
        assert_eq!(reader.pull().unwrap(), Some(Received::Unknown { kind: 200, len: 2 }));
        assert_eq!(reader.pull().unwrap(), Some(Received::Message(Message::ToHost(ToHost::Bye))));
        // a Ping whose payload is one byte: the kind says eight
        let mut cut = Reader::new();
        cut.feed(&[2, 0, 0, 0, kind::PING, 1]);
        assert_eq!(cut.pull(), Err(Malformed::Truncated { kind: kind::PING }));
        // a frame that claims more than the wire allows
        let mut big = Reader::new();
        big.feed(&[0xff, 0xff, 0xff, 0xff, 1]);
        assert!(matches!(big.pull(), Err(Malformed::Oversized { .. })));
    }

    #[test]
    fn the_light_weights_follow_the_six_the_wire_was_born_with() {
        let tag = |weight: Weight| {
            let mut out = Vec::new();
            weight.put(&mut out);
            out
        };
        // the six keep their bytes; the light end takes the next three
        let scale = [
            (Weight::Regular, 0),
            (Weight::Medium, 1),
            (Weight::Semibold, 2),
            (Weight::Bold, 3),
            (Weight::ExtraBold, 4),
            (Weight::Black, 5),
            (Weight::Thin, 6),
            (Weight::ExtraLight, 7),
            (Weight::Light, 8),
        ];
        for (weight, byte) in scale {
            assert_eq!(tag(weight), [byte], "{weight:?}");
            let bytes = [byte];
            let mut cursor = Cursor { bytes: &bytes, at: 0, kind: kind::FRAME };
            assert_eq!(Weight::get(&mut cursor), Ok(weight));
        }
        let unknown = [9];
        let mut cursor = Cursor { bytes: &unknown, at: 0, kind: kind::FRAME };
        assert_eq!(Weight::get(&mut cursor), Err(Malformed::BadTag { what: "Weight", tag: 9 }));
    }

    #[test]
    fn strings_intern_once() {
        let mut strings = Strings::new();
        assert_eq!(strings.intern("a"), 0);
        assert_eq!(strings.intern("b"), 1);
        assert_eq!(strings.intern("a"), 0);
        assert_eq!(strings.into_table(), vec!["a", "b"]);
    }
}
