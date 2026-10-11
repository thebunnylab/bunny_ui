//! Platform-neutral semantics for native assistive technology.
//!
//! ## Wiring
//! A shell enables collection on its runtime and reads a snapshot after a
//! frame. Placement supplies the real visible geometry; views supply names
//! and control roles. Actions return through the runtime's existing input
//! paths. Collection is demand driven and schedules no timer.
//!
//! This projection covers text, buttons, checkboxes, sliders and editable fields. It is not
//! a native accessibility adapter: custom controls, virtualized offscreen
//! navigation and platform protocols need their own integration.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use crate::bind::TextSource;
use crate::layout::Rect;

/// An opaque identity for one exposed element. Never reused on this UI thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId(u64);

impl NodeId {
    /// The stable numeric handle a native adapter can store with its element.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// The semantics implemented by the built-in controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// Read-only text.
    Text,
    /// A control with a press action.
    Button,
    /// A two-state control with a toggle action.
    Checkbox,
    /// A bounded numeric control.
    Slider,
    /// An editable text value.
    TextField,
    /// An editable secret whose value is never exported.
    PasswordField,
}

/// A request from assistive technology to a real control.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Invoke a button or toggle a checkbox through its existing callback.
    Activate,
    /// Give the keyboard to a focusable control.
    Focus,
    /// Replace a field's whole value using its existing editing path.
    SetText(String),
    /// Increase a range by one keyboard step, stopping at its upper endpoint.
    Increment,
    /// Decrease a range by one keyboard step, stopping at its lower endpoint.
    Decrement,
    /// Set a finite in-domain number, using the control's pointer snapping rule.
    SetValue(f64),
}

/// Why a native action could not be applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionError {
    /// The node is no longer exposed by this runtime.
    Unavailable,
    /// The role does not implement the requested action.
    Unsupported,
    /// The numeric request is nonfinite or outside the control's domain.
    InvalidValue,
}

impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "the accessible element is no longer available",
            Self::Unsupported => "the accessible element does not support this action",
            Self::InvalidValue => "the accessible range value must be finite and within its bounds",
        })
    }
}
impl std::error::Error for ActionError {}

/// An immutable numeric snapshot with finite bounds, value and keyboard steps.
/// Constructed from a validated slider domain; reading never changes its binding.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RangeValue {
    minimum: f64,
    maximum: f64,
    value: f64,
    small_change: f64,
    large_change: f64,
}
impl RangeValue {
    /// Projects a validated domain and the same bounded value the slider paints.
    /// NaN displays at the minimum; other out-of-domain values clamp to an endpoint.
    #[cfg(feature = "canvas")]
    #[must_use]
    pub fn new(range: crate::slider::SliderRange, value: f64) -> Self {
        let minimum = range.lower();
        let maximum = range.upper();
        let span = maximum - minimum;
        let small_change = range.step().unwrap_or(span / 100.0).min(span);
        Self {
            minimum,
            maximum,
            value: if value.is_nan() {
                minimum
            } else {
                value.clamp(minimum, maximum)
            },
            small_change,
            large_change: (small_change * 10.0).min(span),
        }
    }
    /// Inclusive lower endpoint.
    #[must_use]
    pub const fn minimum(self) -> f64 {
        self.minimum
    }
    /// Inclusive upper endpoint.
    #[must_use]
    pub const fn maximum(self) -> f64 {
        self.maximum
    }
    /// Current bounded visual value, which need not lie on a discrete step.
    #[must_use]
    pub const fn value(self) -> f64 {
        self.value
    }
    /// Nominal arrow-key change, capped to the span; endpoint steps can be shorter.
    #[must_use]
    pub const fn small_change(self) -> f64 {
        self.small_change
    }
    /// Nominal page-key change, capped to the span.
    #[must_use]
    pub const fn large_change(self) -> f64 {
        self.large_change
    }
    /// Whether a numeric set request is finite and inside the inclusive domain.
    #[must_use]
    pub fn contains(self, value: f64) -> bool {
        value.is_finite() && (self.minimum..=self.maximum).contains(&value)
    }
}

/// One exposed leaf. Containers are currently implicit in reading order.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// Identity retained while the element remains exposed.
    pub id: NodeId,
    /// The control's semantics.
    pub role: Role,
    /// Accessible name; a field defaults to its placeholder.
    pub label: Arc<str>,
    /// Current field value. Always absent for passwords and other roles.
    pub value: Option<Arc<str>>,
    /// Visible bounds in runtime layout coordinates (points, top-left origin).
    pub bounds: Rect,
    /// Overlay identity when this node belongs to a separate native surface.
    /// `None` identifies the main content view. Bounds remain in root layout coordinates.
    pub surface: Option<Rc<str>>,
    /// Whether this control currently owns keyboard focus.
    pub focused: bool,
    /// Whether the control can be operated.
    pub enabled: bool,
    /// The checkbox's value; absent for other roles.
    pub checked: Option<bool>,
    /// Numeric state for a slider; absent for other roles.
    pub range: Option<RangeValue>,
    /// Whether a text field accepts multiple lines.
    pub multiline: bool,
    pub(crate) path: Rc<str>,
}

impl Node {
    /// Whether this role supports an action. Availability is rechecked on use.
    pub fn supports(&self, action: &Action) -> bool {
        self.enabled
            && matches!(
                (self.role, action),
                (
                    Role::Button | Role::Checkbox,
                    Action::Activate | Action::Focus
                ) | (
                    Role::Slider,
                    Action::Focus | Action::Increment | Action::Decrement | Action::SetValue(_)
                ) | (
                    Role::TextField | Role::PasswordField,
                    Action::Focus | Action::SetText(_)
                )
        )
    }
}

/// Immutable snapshot of the visible leaves in reading order.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct Tree {
    nodes: Vec<Node>,
}

impl Tree {
    /// Exposed text and controls in layout order.
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }
    /// Looks up a handle without treating its numeric value as an index.
    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }
}

/// Semantic decorators used by the retained scene. They never affect layout.
#[derive(Clone, Debug)]
pub enum Semantics {
    /// A text leaf, with the same binding as its painted content.
    Text { path: Rc<str>, content: TextSource },
    /// A button whose default name comes from its text descendants.
    Button { path: Rc<str> },
    /// Built-in button or checkbox state. A checked value identifies a checkbox.
    Control { path: Rc<str>, enabled: bool, checked: Option<bool> },
    /// Overrides the name when exactly one semantic element is below it.
    Label(TextSource),
    /// Excludes decorative descendants from assistive technology.
    Hidden,
}

#[derive(Clone, Debug)]
pub(crate) struct Placed {
    pub enabled: bool,
    pub checked: Option<bool>,
    pub range: Option<RangeValue>,
    pub path: Rc<str>,
    pub role: Role,
    pub label: Arc<str>,
    pub value: Option<Arc<str>>,
    pub bounds: Option<Rect>,
    pub surface: Option<Rc<str>>,
    pub multiline: bool,
}

impl Semantics {
    pub(crate) fn collect(
        &self,
        start: usize,
        bounds: Option<Rect>,
        surface: &Option<Rc<str>>,
        nodes: &mut Vec<Placed>,
    ) {
        // A modal descendant may have removed what preceded it.
        let start = start.min(nodes.len());
        match self {
            Self::Hidden => nodes.truncate(start),
            Self::Label(label) => {
                if let [node] = &mut nodes[start..] {
                    node.label = label.get();
                }
            }
            Self::Text { path, content } => {
                nodes.push(Placed {
                    enabled: true,
                    checked: None,
                    range: None,
                    path: Rc::clone(path),
                    role: Role::Text,
                    label: content.get(),
                    value: None,
                    bounds,
                    surface: surface.clone(),
                    multiline: false,
                });
            }
            Self::Button { path } | Self::Control { path, .. } => {
                let (enabled, checked) = match self {
                    Self::Control { enabled, checked, .. } => (*enabled, *checked),
                    _ => (true, None),
                };
                let label = match &nodes[start..] {
                    [only] => Arc::clone(&only.label),
                    many => many
                        .iter()
                        .map(|node| node.label.as_ref())
                        .filter(|label| !label.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ")
                        .into(),
                };
                nodes.truncate(start);
                {
                    nodes.push(Placed {
                        enabled,
                        checked,
                        range: None,
                        path: Rc::clone(path),
                        role: if checked.is_some() { Role::Checkbox } else { Role::Button },
                        label,
                        value: None,
                        bounds,
                        surface: surface.clone(),
                        multiline: false,
                    });
                }
            }
        }
    }
}

thread_local! {
    static CAPTURING: Cell<bool> = const { Cell::new(false) };
    static NEXT_ID: Cell<u64> = const { Cell::new(1) };
}

#[derive(Default)]
pub(crate) struct State {
    pub enabled: bool,
    tree: Tree,
}

impl State {
    pub fn clear(&mut self) {
        self.tree.nodes.clear();
    }

    pub fn update(&mut self, placed: &[Placed]) {
        if !self.enabled {
            return;
        }
        let previous: motor::hash::FxHashMap<_, _> = self
            .tree
            .nodes
            .iter()
            .map(|node| ((node.path.as_ref(), node.role), node.id))
            .collect();
        let nodes = placed
            .iter()
            .filter_map(|placed| {
                let bounds = placed.bounds.filter(|bounds| !bounds.is_empty())?;
                let id = previous
                    .get(&(placed.path.as_ref(), placed.role))
                    .copied()
                    .unwrap_or_else(|| {
                        NEXT_ID.with(|next| {
                            let id = next.get();
                            // Exhausting 2^64 exposed lifetimes is unreachable in a process.
                            next.set(id.checked_add(1).expect("accessibility identity exhausted"));
                            NodeId(id)
                        })
                    });
                Some(Node {
                    enabled: placed.enabled,
                    checked: placed.checked,
                    range: placed.range,
                    id,
                    path: Rc::clone(&placed.path),
                    role: placed.role,
                    label: Arc::clone(&placed.label),
                    value: placed.value.clone(),
                    bounds,
                    surface: placed.surface.clone(),
                    focused: false,
                    multiline: placed.multiline,
                })
            })
            .collect();
        self.tree = Tree { nodes };
    }

    pub fn snapshot(&self, focused: Option<&str>) -> Tree {
        let mut tree = self.tree.clone();
        for node in &mut tree.nodes {
            node.focused = node.supports(&Action::Focus)
                && focused == Some(node.path.as_ref());
        }
        tree
    }
}

pub(crate) struct CaptureScope(bool);
impl CaptureScope {
    pub(crate) fn enter(enabled: bool) -> Self {
        Self(CAPTURING.with(|flag| flag.replace(enabled)))
    }
}
impl Drop for CaptureScope {
    fn drop(&mut self) {
        CAPTURING.with(|flag| flag.set(self.0));
    }
}
pub(crate) fn capturing() -> bool {
    CAPTURING.with(Cell::get)
}
