//! Where a node was written — the call site of the builder or the modifier
//! that made it, recorded into the layout so a tool can map a frame on
//! screen back to a line of source and a line of source to a frame.
//!
//! This is the framework's half of Studio's two-way canvas (Trinity,
//! FR-STUDIO-5): click a box, land on the `.padding(…)` that made it; move
//! the caret, see the box light up. Every builder in [`crate::views`] and
//! every method of [`crate::ext::ViewExt`] is `#[track_caller]`, so the
//! [`Site`] a view carries is the line the app wrote, through `vstack!`
//! and `text!` alike (a `#[track_caller]` reached from a macro reports the
//! macro's invocation).
//!
//! ## Wiring
//!
//! Off by default and free when off: a view carries its site (one pointer
//! to a `'static` location), and nothing else happens until a runtime
//! asks for sites with [`crate::runtime::Runtime::record_sites`]. Then
//! [`seal`] wraps each sited node in a transparent [`LayoutNode::Sited`]
//! as the body renders, the layout walk records a [`SitePlacement`] per
//! wrapper with the frame it was placed at, and the runtime hands the list
//! out through [`crate::runtime::Runtime::sites`]. The display list is the
//! same with the wrappers as without: a sited node measures and places
//! exactly as its child does.
//!
//! ## Production gotchas
//!
//! - **Turn recording on before the first pass.** The reconciler retains
//!   a clean boundary's nodes, and nodes built without wrappers stay
//!   without them until their body runs again — a preview host sets
//!   recording at start, which is the only place a tool needs it.
//! - **A site is where the BUILDER was called**, not where the view is
//!   mounted: a helper that builds a row in one place and is called from
//!   ten reports its own line ten times. That is the right answer for a
//!   designer (the code to edit is the helper's) and the wrong one for a
//!   tool that wanted the ten call sites — which is what the identity
//!   path is for.
//! - **A folded modifier is still two sites.** `.padding().padding()`
//!   folds into one `Padding` node when recording is off; on, each keeps
//!   its wrapper and the geometry stays the same.
//!
//! ## Overrides
//!
//! The other direction of the canvas: a tool puts a modifier on a site
//! before the code says so ([`set_overrides`], keyed by [`SiteKey`]), and
//! the next pass shows it — a `Replace` stands in for the modifier written
//! at the site, an `Append` goes on top of whatever the site renders, a
//! `Hide` keeps the node's place and paints nothing. The table is the
//! thread's, versioned like the theme: a moved table rebuilds every
//! runtime's retention once (the nodes baked the modifiers they were built
//! with), and [`clear_overrides`] puts the code back in charge. Cost when
//! empty: one `Cell` read per seal.
//! Independent preview instances use [`OverrideSet::with`] to enter their own
//! retained context without invalidating another runtime.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

pub use motor::identity::MountedId;
pub use motor::runtime::Site;
use motor::state::Context;

use crate::layout::{LayoutNode, Rect};
use crate::modifier::Modifier;
use crate::view::NodeList;

// =============================================================================
// Overrides — a modifier put on a site before the code says so
// =============================================================================

/// Where a node was written, as a key a tool names it by: the file, the
/// line and the column of its call site.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SiteKey {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

impl SiteKey {
    /// The key of a site the compiler recorded; a site the framework
    /// named itself has none.
    pub fn of(site: Site) -> Option<SiteKey> {
        match site {
            Site::Caller(location) => Some(SiteKey { file: location.file().to_string(), line: location.line(), column: location.column() }),
            Site::Named(_) => None,
        }
    }
}

/// What an override does at its site — the designer's gesture on the
/// picture before the real edit is compiled.
#[derive(Clone)]
pub enum Override {
    /// The modifier written at the site stands aside for this one (a
    /// modifier site only; on a builder's site it appends).
    Replace(Modifier),
    /// This modifier goes on top of what the site renders.
    Append(Modifier),
    /// The node keeps its place and paints nothing.
    Hide,
}

impl std::fmt::Debug for Override {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Override::Replace(modifier) => write!(f, "Replace({})", modifier.name()),
            Override::Append(modifier) => write!(f, "Append({})", modifier.name()),
            Override::Hide => f.write_str("Hide"),
        }
    }
}

thread_local! {
    static OVERRIDES: RefCell<Rc<HashMap<SiteKey, Vec<Override>>>> = RefCell::new(Rc::new(HashMap::new()));
    static OVERRIDES_CLOCK: Cell<u64> = const { Cell::new(0) };
    static OVERRIDES_VERSION: Cell<u64> = const { Cell::new(0) };
    static HAS_OVERRIDES: Cell<bool> = const { Cell::new(false) };
}

/// Replaces the thread's overrides with these, and bumps the version
/// every runtime compares — the next pass rebuilds the retention once,
/// the way an installed theme does, since the nodes baked the modifiers
/// they were built with. Several overrides on one site apply in order.
pub fn set_overrides(overrides: impl IntoIterator<Item = (SiteKey, Override)>) {
    OverrideSet::new(overrides).install();
}

/// An immutable, versioned set of one scene's provisional source modifiers.
/// Re-entering a set keeps its version, so other scenes do not invalidate this
/// runtime's retained layout. The default thread table remains the legacy lane.
#[derive(Clone)]
pub struct OverrideSet {
    table: Rc<HashMap<SiteKey, Vec<Override>>>,
    version: u64,
}

impl OverrideSet {
    /// Capture overrides in application order with a fresh retention identity.
    pub fn new(overrides: impl IntoIterator<Item = (SiteKey, Override)>) -> Self {
        let mut table: HashMap<SiteKey, Vec<Override>> = HashMap::new();
        for (site, value) in overrides {
            table.entry(site).or_default().push(value);
        }
        let version = OVERRIDES_CLOCK.with(|clock| {
            let next = clock.get() + 1;
            clock.set(next);
            next
        });
        Self {
            table: Rc::new(table),
            version,
        }
    }

    /// Render or deliver input under this scene's overrides. Restore the outer
    /// set even when the scene panics; nested scopes remain independent.
    pub fn with<T>(&self, run: impl FnOnce() -> T) -> T {
        let previous = Self {
            table: OVERRIDES.with(|table| Rc::clone(&table.borrow())),
            version: overrides_version(),
        };
        let _restore = RestoreOverrides(previous);
        self.install();
        run()
    }

    fn install(&self) {
        HAS_OVERRIDES.with(|has| has.set(!self.table.is_empty()));
        OVERRIDES.with(|current| *current.borrow_mut() = Rc::clone(&self.table));
        OVERRIDES_VERSION.with(|version| version.set(self.version));
    }
}

struct RestoreOverrides(OverrideSet);
impl Drop for RestoreOverrides {
    fn drop(&mut self) {
        self.0.install();
    }
}

/// Takes every override away; the code says what the picture shows again.
pub fn clear_overrides() {
    set_overrides(std::iter::empty());
}

/// The overrides' version — whoever retains what they built compares it.
pub fn overrides_version() -> u64 {
    OVERRIDES_VERSION.with(Cell::get)
}

/// The overrides at `site`, when the table has any.
pub(crate) fn overrides_at(site: Site) -> Option<Vec<Override>> {
    if !HAS_OVERRIDES.with(Cell::get) {
        return None;
    }
    let key = SiteKey::of(site)?;
    OVERRIDES.with(|table| table.borrow().get(&key).cloned())
}

/// The modifier a `Replace` at `site` puts in place of the one written
/// there — the first, when several.
pub(crate) fn replacement_at(site: Site) -> Option<Modifier> {
    overrides_at(site)?.into_iter().find_map(|override_| match override_ {
        Override::Replace(modifier) => Some(modifier),
        Override::Append(_) | Override::Hide => None,
    })
}

/// What kind of node a site made — the word a tool shows beside the line.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(u8)]
pub enum NodeKind {
    /// `text(…)` / `text!`.
    Text,
    /// `button(…)`.
    Button,
    /// `text_field(…)` / `text_editor(…)`.
    Field,
    /// `vstack(…)` / `vstack!`.
    VStack,
    /// `hstack(…)` / `hstack!`.
    HStack,
    /// `zstack(…)` / `zstack!`.
    ZStack,
    /// `spacer()`.
    Spacer,
    /// `rectangle()`.
    Rectangle,
    /// `image(…)`.
    Image,
    /// `icon(…)`.
    Icon,
    /// `scroll(…)`.
    Scroll,
    /// `tuple(…)`.
    Tuple,
    /// `section(…)`.
    Section,
    /// `list(…)`.
    List,
    /// `virtual_list(…)`.
    VirtualList,
    /// A modifier applied by a `ViewExt` method — `modifier` names it.
    Modifier,
    /// `grid(...)` / `grid!`.
    Grid,
    /// `ellipse()`.
    Ellipse,
}

impl NodeKind {
    /// The builder's name, as the app wrote it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Button => "button",
            Self::Field => "text_field",
            Self::VStack => "vstack",
            Self::HStack => "hstack",
            Self::ZStack => "zstack",
            Self::Grid => "grid",
            Self::Spacer => "spacer",
            Self::Rectangle => "rectangle",
            Self::Ellipse => "ellipse",
            Self::Image => "image",
            Self::Icon => "icon",
            Self::Scroll => "scroll",
            Self::Tuple => "tuple",
            Self::Section => "section",
            Self::List => "list",
            Self::VirtualList => "virtual_list",
            Self::Modifier => "modifier",
        }
    }
}

/// One sited node as the last layout placed it.
#[derive(Clone, Debug, PartialEq)]
pub struct SitePlacement {
    /// This placement's index in the list — what `parent` refers to.
    pub index: u32,
    /// Mounted declaration identity, independent of this layout's index.
    pub mounted: Option<MountedId>,
    /// The enclosing sited node, when there is one.
    pub parent: Option<u32>,
    /// Where the builder or the modifier was called.
    pub site: Site,
    /// What the site made.
    pub kind: NodeKind,
    /// The modifier's name, for a [`NodeKind::Modifier`].
    pub modifier: Option<&'static str>,
    /// The frame the node was placed at, in the scene's coordinates.
    pub frame: Rect,
    /// The identity path of the nearest component boundary above the
    /// node, when the layout knew one.
    pub path: Option<Rc<str>>,
}

thread_local! {
    static RECORDING: Cell<bool> = const { Cell::new(false) };
}

/// Whether sites are being wrapped into the layout on this thread.
#[must_use]
pub fn recording() -> bool {
    RECORDING.with(Cell::get)
}

/// Turn site recording on or off for this thread — see the module docs for
/// why it must be on before the first pass.
pub fn set_recording(on: bool) {
    RECORDING.with(|recording| recording.set(on));
}

/// The site of whoever called the `#[track_caller]` function that called
/// this — a builder's or a modifier's own caller.
#[track_caller]
#[must_use]
pub(crate) fn here() -> Site {
    Site::Caller(std::panic::Location::caller())
}

/// Wrap the node a builder or a modifier left at `mark` in its site, when
/// recording; nothing otherwise, so a pass that nobody is watching is the
/// pass it always was.
pub(crate) fn seal(ctx: &Context, out: &mut NodeList, mark: usize, site: Site, kind: NodeKind, modifier: Option<&'static str>) {
    // the overrides on this site go on top of what it rendered: an
    // appended modifier, or the node hidden. A `Replace` on a modifier's
    // site was taken by the modifier's own render; on a builder's it is
    // an append, since there is nothing to stand aside.
    if let Some(overrides) = overrides_at(site) {
        for override_ in &overrides {
            match override_ {
                Override::Append(added) => crate::modifier::apply(added, ctx, ctx, out, mark),
                Override::Replace(added) if modifier.is_none() => crate::modifier::apply(added, ctx, ctx, out, mark),
                Override::Replace(_) => {}
                Override::Hide => crate::modifier::apply(&Modifier::Hidden, ctx, ctx, out, mark),
            }
        }
    }
    if !recording() {
        return;
    }
    let mounted = motor::identity::mounted_site(site, modifier.unwrap_or_else(|| kind.name()));
    out.wrap_layout_from(mark, |child| LayoutNode::Sited { site, mounted, kind, modifier, child: Box::new(child) });
}
