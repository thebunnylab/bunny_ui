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

use std::cell::Cell;
use std::rc::Rc;

pub use motor::runtime::Site;

use crate::layout::{LayoutNode, Rect};
use crate::view::NodeList;

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
            Self::Spacer => "spacer",
            Self::Rectangle => "rectangle",
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
pub(crate) fn seal(out: &mut NodeList, mark: usize, site: Site, kind: NodeKind, modifier: Option<&'static str>) {
    if !recording() {
        return;
    }
    out.wrap_layout_from(mark, |child| LayoutNode::Sited { site, kind, modifier, child: Box::new(child) });
}
