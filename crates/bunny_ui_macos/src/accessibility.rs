//! NSAccessibility adapter inside the existing AppKit FFI boundary.
//!
//! ## Wiring
//! One scene owns its native surfaces and each surface owns stable proxy objects.
//! AppKit callbacks only read their snapshots or enqueue existing shell events;
//! they never borrow a running frame. The first query requests semantic capture.
//! Notifications are emitted after every changed snapshot has been installed.
//!
//! ## Production gotchas
//! Native callbacks and destructors can run during main-thread teardown after
//! the weak lookup maps have been destroyed. Missing maps then mean retired
//! objects; dropping native ownership must still release the AppKit objects.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{CStr, c_void};
use std::rc::{Rc, Weak};

use super::{
    AppEvent, CGPoint, CGRect, CGSize, Id, Sel, WindowHandle, class, class_addMethod, dispatch_to,
    msg_id, msg_rect, msg_void, ns_string, objc_allocateClassPair, objc_registerClassPair, sel,
    sel_getName,
};
use bunny_ui::accessibility::{Action, Node, Role};
use bunny_ui::layout::{Point, Rect};
use bunny_ui::runtime::Runtime;

#[allow(
    clashing_extern_declarations,
    reason = "Objective-C dispatch uses each selector's concrete ABI"
)]
#[link(name = "objc")]
unsafe extern "C" {
    #[link_name = "objc_msgSend"]
    fn object_at(object: Id, selector: Sel, index: usize) -> Id;
    #[link_name = "objc_msgSend"]
    fn count(object: Id, selector: Sel) -> usize;
    #[link_name = "objc_msgSend"]
    fn append(object: Id, selector: Sel, value: Id);
    #[link_name = "objc_msgSend"]
    fn predicate(object: Id, selector: Sel, value: Id) -> i8;
    #[link_name = "objc_msgSend"]
    fn boolean(object: Id, selector: Sel) -> i8;
    #[link_name = "objc_msgSendSuper"]
    fn super_object(object: *const Super, selector: Sel) -> Id;
    #[link_name = "objc_msgSend"]
    fn hit_object(object: Id, selector: Sel, point: CGPoint) -> Id;
}

#[repr(C)]
struct Super {
    receiver: Id,
    superclass: Id,
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    fn NSAccessibilityFrameInView(view: Id, frame: CGRect) -> CGRect;
    fn NSAccessibilityRoleDescription(role: Id, subrole: Id) -> Id;
    fn NSAccessibilityPostNotification(element: Id, notification: Id);
    static NSAccessibilityLayoutChangedNotification: Id;
    static NSAccessibilityTitleChangedNotification: Id;
    static NSAccessibilityValueChangedNotification: Id;
    static NSAccessibilityFocusedUIElementChangedNotification: Id;
    static NSAccessibilityUIElementDestroyedNotification: Id;
}

// =============================================================================
// Scene and native object ownership
// =============================================================================

thread_local! {
    static SURFACES: RefCell<HashMap<usize, Weak<Surface>>> = RefCell::new(HashMap::new());
    static ELEMENTS: RefCell<HashMap<usize, Weak<Element>>> = RefCell::new(HashMap::new());
}

/// Owns a window's lazily requested native accessibility projection.
pub(crate) struct Accessibility(Rc<Scene>);
struct Scene {
    owner: Id,
    requested: Cell<bool>,
    surfaces: RefCell<HashMap<usize, Rc<Surface>>>,
}
struct Surface {
    view: Id,
    scene: Weak<Scene>,
    origin: Cell<Point>,
    elements: RefCell<Vec<Rc<Element>>>,
}
struct Element {
    object: Id,
    surface: Weak<Surface>,
    node: RefCell<Node>,
    alive: Cell<bool>,
}

impl Accessibility {
    /// Registers the main surface without requesting semantic capture.
    pub(crate) fn new(window: WindowHandle) -> Self {
        let scene = Rc::new(Scene {
            owner: window.raw_window() as Id,
            requested: Cell::new(false),
            surfaces: RefCell::new(HashMap::new()),
        });
        let surface = Surface::new(window.view(), Rc::downgrade(&scene));
        scene
            .surfaces
            .borrow_mut()
            .insert(window.view() as usize, surface);
        Self(scene)
    }

    /// Whether AppKit has requested semantic children, focus or hit testing.
    pub(crate) fn requested(&self) -> bool {
        self.0.requested.get()
    }

    /// Overlay bounds and nodes use the same root layout coordinate system.
    pub(crate) fn update(
        &self,
        runtime: &Runtime,
        window: WindowHandle,
        overlays: &[(String, WindowHandle, Rect)],
    ) {
        let mut surfaces = self.0.surfaces.borrow_mut();
        surfaces.retain(|key, _| {
            *key == window.view() as usize
                || overlays
                    .iter()
                    .any(|(_, window, _)| *key == window.view() as usize)
        });
        for (_, handle, _) in overlays {
            surfaces
                .entry(handle.view() as usize)
                .or_insert_with(|| Surface::new(handle.view(), Rc::downgrade(&self.0)));
        }
        if !self.requested() {
            return;
        }
        let tree = runtime.accessibility_tree();
        // `new` installs the main surface and the retain predicate above keeps
        // it unconditionally; no other operation removes it while Scene lives.
        let main = Rc::clone(
            surfaces
                .get(&(window.view() as usize))
                .expect("the main surface lives for the entire scene"),
        );
        let mut updates = vec![(main, None, Point::default())];
        for (path, handle, frame) in overlays {
            let surface = surfaces
                .entry(handle.view() as usize)
                .or_insert_with(|| Surface::new(handle.view(), Rc::downgrade(&self.0)));
            updates.push((Rc::clone(surface), Some(path.as_str()), frame.origin));
        }
        drop(surfaces);
        let mut notifications = Vec::new();
        for (surface, path, origin) in updates {
            surface.origin.set(origin);
            surface.update(
                tree.nodes()
                    .iter()
                    .filter(|node| node.surface.as_deref() == path),
                &mut notifications,
            );
        }
        // No RefCell loan survives the call into AppKit. A notification can
        // synchronously ask for children, values or focus from the new tree.
        for (target, notification) in notifications {
            target.post(notification);
        }
    }
}

impl Surface {
    fn new(view: Id, scene: Weak<Scene>) -> Rc<Self> {
        // Own the view until callbacks can no longer resolve this surface.
        unsafe {
            msg_id(view, sel("retain"));
        }
        let surface = Rc::new(Self {
            view,
            scene,
            origin: Cell::new(Point::default()),
            elements: RefCell::new(Vec::new()),
        });
        SURFACES.with(|all| {
            all.borrow_mut()
                .insert(view as usize, Rc::downgrade(&surface));
        });
        surface
    }

    fn activate(&self) {
        if let Some(scene) = self.scene.upgrade()
            && !scene.requested.replace(true)
        {
            dispatch_to(scene.owner, AppEvent::AccessibilityEnable);
        }
    }

    fn update<'a>(
        self: &Rc<Self>,
        nodes: impl Iterator<Item = &'a Node>,
        notifications: &mut Vec<(Target, Notification)>,
    ) {
        let previous = std::mem::take(&mut *self.elements.borrow_mut());
        let old_order: Vec<_> = previous
            .iter()
            .map(|element| element.node.borrow().id)
            .collect();
        let had_focus = previous.iter().any(|element| element.node.borrow().focused);
        let mut previous: HashMap<_, _> = previous
            .into_iter()
            .map(|element| {
                let id = element.node.borrow().id;
                (id, element)
            })
            .collect();
        let mut geometry_changed = false;
        let current: Vec<_> = nodes
            .map(|node| {
                let kept = previous.remove(&node.id);
                let is_new = kept.is_none();
                let element =
                    kept.unwrap_or_else(|| Element::new(node.clone(), Rc::downgrade(self)));
                let mut held = element.node.borrow_mut();
                for notification in changes((!is_new).then_some(&*held), node) {
                    notifications.push((Target::Element(Rc::clone(&element)), notification));
                }
                geometry_changed |= held.bounds != node.bounds;
                *held = node.clone();
                drop(held);
                element
            })
            .collect();
        let order_changed = !old_order
            .iter()
            .copied()
            .eq(current.iter().map(|element| element.node.borrow().id));
        let has_focus = current.iter().any(|element| element.node.borrow().focused);
        *self.elements.borrow_mut() = current;
        if had_focus && !has_focus {
            notifications.push((Target::Surface(Rc::clone(self)), Notification::Focus));
        }
        for removed in previous.into_values() {
            removed.alive.set(false);
            notifications.push((Target::Element(removed), Notification::Destroyed));
        }
        if order_changed || geometry_changed {
            notifications.push((Target::Surface(Rc::clone(self)), Notification::Layout));
        }
    }

    fn children(&self) -> Id {
        let elements = self.elements.borrow().clone();
        unsafe {
            let array = msg_id(class("NSMutableArray"), sel("array"));
            for element in elements {
                append(array, sel("addObject:"), element.object);
            }
            // Native hosted views retain AppKit's own accessibility children.
            let native = super_object(
                &Super {
                    receiver: self.view,
                    superclass: class("NSView"),
                },
                sel("accessibilityChildren"),
            );
            for index in 0..count(native, sel("count")) {
                append(
                    array,
                    sel("addObject:"),
                    object_at(native, sel("objectAtIndex:"), index),
                );
            }
            array
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        let _ = SURFACES.try_with(|all| {
            all.borrow_mut().remove(&(self.view as usize));
        });
        for element in self.elements.get_mut() {
            element.alive.set(false);
        }
        unsafe {
            msg_void(self.view, sel("release"));
        }
    }
}

impl Element {
    fn new(node: Node, surface: Weak<Surface>) -> Rc<Self> {
        let object = unsafe {
            msg_id(
                msg_id(class("BunnyAccessibilityElement"), sel("alloc")),
                sel("init"),
            )
        };
        let element = Rc::new(Self {
            object,
            surface,
            node: RefCell::new(node),
            alive: Cell::new(true),
        });
        ELEMENTS.with(|all| {
            all.borrow_mut()
                .insert(object as usize, Rc::downgrade(&element));
        });
        element
    }

    fn action(&self, action: Action) -> bool {
        let node = self.node.borrow();
        if !self.alive.get() || !node.supports(&action) {
            return false;
        }
        let id = node.id;
        drop(node);
        let Some(surface) = self.surface.upgrade() else {
            return false;
        };
        let Some(scene) = surface.scene.upgrade() else {
            return false;
        };
        dispatch_to(scene.owner, AppEvent::AccessibilityAction { id, action });
        true
    }

    fn frame(&self) -> CGRect {
        let Some(surface) = self.surface.upgrade() else {
            return empty_frame();
        };
        let bounds = self.node.borrow().bounds;
        let origin = surface.origin.get();
        unsafe {
            let size = msg_rect(surface.view, sel("bounds")).size;
            NSAccessibilityFrameInView(
                surface.view,
                CGRect {
                    origin: CGPoint {
                        x: bounds.origin.x - origin.x,
                        y: size.height - (bounds.origin.y - origin.y) - bounds.size.height,
                    },
                    size: CGSize {
                        width: bounds.size.width,
                        height: bounds.size.height,
                    },
                },
            )
        }
    }
}
impl Drop for Element {
    fn drop(&mut self) {
        let _ = ELEMENTS.try_with(|all| {
            all.borrow_mut().remove(&(self.object as usize));
        });
        unsafe {
            msg_void(self.object, sel("release"));
        }
    }
}

// =============================================================================
// Snapshot change notifications
// =============================================================================

enum Target {
    Element(Rc<Element>),
    Surface(Rc<Surface>),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Notification {
    Layout,
    Title,
    Value,
    Focus,
    Destroyed,
}

fn value_of(node: &Node) -> Option<&str> {
    if node.role == Role::Text {
        Some(node.label.as_ref())
    } else {
        node.value.as_deref()
    }
}

fn changes(previous: Option<&Node>, current: &Node) -> impl Iterator<Item = Notification> {
    [
        previous
            .is_some_and(|old| old.label != current.label)
            .then_some(Notification::Title),
        previous
            .is_some_and(|old| value_of(old) != value_of(current))
            .then_some(Notification::Value),
        (current.focused && previous.is_none_or(|old| !old.focused)).then_some(Notification::Focus),
    ]
    .into_iter()
    .flatten()
}
impl Target {
    fn post(&self, notification: Notification) {
        let object = match self {
            Self::Element(element) => element.object,
            Self::Surface(surface) => surface.view,
        };
        unsafe {
            let name = match notification {
                Notification::Layout => NSAccessibilityLayoutChangedNotification,
                Notification::Title => NSAccessibilityTitleChangedNotification,
                Notification::Value => NSAccessibilityValueChangedNotification,
                Notification::Focus => NSAccessibilityFocusedUIElementChangedNotification,
                Notification::Destroyed => NSAccessibilityUIElementDestroyedNotification,
            };
            NSAccessibilityPostNotification(object, name);
        }
    }
}

// =============================================================================
// AppKit callbacks
// =============================================================================

fn surface(object: Id) -> Option<Rc<Surface>> {
    SURFACES
        .try_with(|all| all.borrow().get(&(object as usize)).and_then(Weak::upgrade))
        .ok()
        .flatten()
}
fn element(object: Id) -> Option<Rc<Element>> {
    ELEMENTS
        .try_with(|all| all.borrow().get(&(object as usize)).and_then(Weak::upgrade))
        .ok()
        .flatten()
        .filter(|element| element.alive.get())
}
fn empty() -> Id {
    std::ptr::null_mut()
}
fn empty_frame() -> CGRect {
    CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: 0.0,
            height: 0.0,
        },
    }
}

extern "C" fn view_children(this: Id, _: Sel) -> Id {
    let Some(surface) = surface(this) else {
        return empty();
    };
    surface.activate();
    surface.children()
}
extern "C" fn view_element(this: Id, _: Sel) -> i8 {
    i8::from(surface(this).is_some())
}
extern "C" fn view_role(_: Id, _: Sel) -> Id {
    unsafe { ns_string("AXGroup") }
}
extern "C" fn view_focused(this: Id, _: Sel) -> Id {
    let Some(surface) = surface(this) else {
        return this;
    };
    surface.activate();
    // A hosted native editor owns its first responder and its own AX focus.
    // Retained Bunny fields use this view as first responder instead.
    unsafe {
        let mut responder = msg_id(msg_id(this, sel("window")), sel("firstResponder"));
        // AppKit's shared field editor speaks for its owning text control.
        if predicate(responder, sel("isKindOfClass:"), class("NSText")) != 0
            && boolean(responder, sel("isFieldEditor")) != 0
        {
            let owner = msg_id(responder, sel("delegate"));
            if predicate(owner, sel("isKindOfClass:"), class("NSView")) != 0
                && predicate(owner, sel("isDescendantOf:"), this) != 0
            {
                responder = owner;
            }
        }
        if responder != this
            && predicate(responder, sel("isKindOfClass:"), class("NSView")) != 0
            && predicate(responder, sel("isDescendantOf:"), this) != 0
        {
            let focused = msg_id(responder, sel("accessibilityFocusedUIElement"));
            if !focused.is_null() {
                return focused;
            }
        }
    }

    surface
        .elements
        .borrow()
        .iter()
        .find(|element| element.node.borrow().focused)
        .map_or(this, |element| element.object)
}
fn contains(frame: CGRect, point: CGPoint) -> bool {
    point.x >= frame.origin.x
        && point.y >= frame.origin.y
        && point.x < frame.origin.x + frame.size.width
        && point.y < frame.origin.y + frame.size.height
}
extern "C" fn view_hit(this: Id, _: Sel, point: CGPoint) -> Id {
    let Some(surface) = surface(this) else {
        return this;
    };
    surface.activate();
    // Native hosted controls expose AppKit's own elements (for example an
    // NSButtonCell), which may perform deeper hit testing than their view.
    unsafe {
        let native = super_object(
            &Super {
                receiver: this,
                superclass: class("NSView"),
            },
            sel("accessibilityChildren"),
        );
        for index in (0..count(native, sel("count"))).rev() {
            let child = object_at(native, sel("objectAtIndex:"), index);
            if contains(msg_rect(child, sel("accessibilityFrame")), point) {
                let hit = hit_object(child, sel("accessibilityHitTest:"), point);
                return if hit.is_null() { child } else { hit };
            }
        }
    }
    let elements = surface.elements.borrow().clone();
    elements
        .iter()
        .rev()
        .find(|element| contains(element.frame(), point))
        .map_or(this, |element| element.object)
}
extern "C" fn element_alive(this: Id, _: Sel) -> i8 {
    i8::from(element(this).is_some())
}
extern "C" fn element_role(this: Id, _: Sel) -> Id {
    let Some(element) = element(this) else {
        return empty();
    };
    unsafe {
        ns_string(match element.node.borrow().role {
            Role::Text => "AXStaticText",
            Role::Button => "AXButton",
            Role::TextField | Role::PasswordField => "AXTextField",
        })
    }
}
extern "C" fn element_subrole(this: Id, _: Sel) -> Id {
    match element(this) {
        Some(element) if element.node.borrow().role == Role::PasswordField => unsafe {
            ns_string("AXSecureTextField")
        },
        _ => empty(),
    }
}
extern "C" fn element_description(this: Id, command: Sel) -> Id {
    if element(this).is_none() {
        return empty();
    }
    unsafe {
        NSAccessibilityRoleDescription(element_role(this, command), element_subrole(this, command))
    }
}
extern "C" fn element_label(this: Id, _: Sel) -> Id {
    element(this).map_or(empty(), |element| unsafe {
        ns_string(&element.node.borrow().label)
    })
}
extern "C" fn element_value(this: Id, _: Sel) -> Id {
    let Some(element) = element(this) else {
        return empty();
    };
    let node = element.node.borrow();
    value_of(&node).map_or(empty(), |value| unsafe { ns_string(value) })
}
extern "C" fn element_identifier(this: Id, _: Sel) -> Id {
    element(this).map_or(empty(), |element| unsafe {
        ns_string(&format!("bunny-{}", element.node.borrow().id.get()))
    })
}
extern "C" fn element_parent(this: Id, _: Sel) -> Id {
    element(this)
        .and_then(|element| element.surface.upgrade())
        .map_or(empty(), |surface| surface.view)
}
extern "C" fn element_window(this: Id, command: Sel) -> Id {
    unsafe { msg_id(element_parent(this, command), sel("window")) }
}
extern "C" fn element_frame(this: Id, _: Sel) -> CGRect {
    element(this).map_or_else(empty_frame, |element| element.frame())
}
extern "C" fn element_focused(this: Id, _: Sel) -> i8 {
    i8::from(element(this).is_some_and(|element| element.node.borrow().focused))
}
extern "C" fn element_press(this: Id, _: Sel) -> i8 {
    i8::from(element(this).is_some_and(|element| element.action(Action::Activate)))
}
extern "C" fn element_set_focus(this: Id, _: Sel, focused: i8) {
    if focused != 0
        && let Some(element) = element(this)
    {
        element.action(Action::Focus);
    }
}
extern "C" fn element_set_value(this: Id, _: Sel, value: Id) {
    if let Some(element) = element(this)
        && !value.is_null()
        && unsafe { predicate(value, sel("isKindOfClass:"), class("NSString")) } != 0
    {
        let text = unsafe { super::text_argument_to_string(value) };
        element.action(Action::SetText(text));
    }
}
extern "C" fn element_allowed(this: Id, _: Sel, selector: Sel) -> i8 {
    let Some(element) = element(this) else {
        return 0;
    };
    let node = element.node.borrow();
    if selector == unsafe { sel("accessibilityPerformPress") } {
        return i8::from(node.role == Role::Button);
    }
    if selector == unsafe { sel("setAccessibilityValue:") }
        || selector == unsafe { sel("setAccessibilityFocused:") }
    {
        return i8::from(matches!(node.role, Role::TextField | Role::PasswordField));
    }
    let name = unsafe { CStr::from_ptr(sel_getName(selector)) }.to_bytes();
    i8::from(!name.starts_with(b"setAccessibility") && !name.starts_with(b"accessibilityPerform"))
}

// =============================================================================
// Objective-C registration
// =============================================================================

/// Installs selectors while the shell is registering its BunnyView class.
pub(super) unsafe fn register_view(view: Id) {
    unsafe {
        for (name, method, types) in [
            (
                "accessibilityChildren",
                view_children as *const c_void,
                c"@@:",
            ),
            (
                "accessibilityChildrenInNavigationOrder",
                view_children as *const c_void,
                c"@@:",
            ),
            (
                "isAccessibilityElement",
                view_element as *const c_void,
                c"c@:",
            ),
            ("accessibilityRole", view_role as *const c_void, c"@@:"),
            (
                "accessibilityFocusedUIElement",
                view_focused as *const c_void,
                c"@@:",
            ),
            (
                "accessibilityHitTest:",
                view_hit as *const c_void,
                c"@@:{CGPoint=dd}",
            ),
        ] {
            class_addMethod(view, sel(name), method, types.as_ptr());
        }
        let class = objc_allocateClassPair(
            class("NSAccessibilityElement"),
            c"BunnyAccessibilityElement".as_ptr(),
            0,
        );
        for (name, method, types) in [
            (
                "isAccessibilityElement",
                element_alive as *const c_void,
                c"c@:",
            ),
            (
                "isAccessibilityEnabled",
                element_alive as *const c_void,
                c"c@:",
            ),
            ("accessibilityRole", element_role as *const c_void, c"@@:"),
            (
                "accessibilitySubrole",
                element_subrole as *const c_void,
                c"@@:",
            ),
            (
                "accessibilityRoleDescription",
                element_description as *const c_void,
                c"@@:",
            ),
            ("accessibilityLabel", element_label as *const c_void, c"@@:"),
            ("accessibilityValue", element_value as *const c_void, c"@@:"),
            (
                "accessibilityIdentifier",
                element_identifier as *const c_void,
                c"@@:",
            ),
            (
                "accessibilityParent",
                element_parent as *const c_void,
                c"@@:",
            ),
            (
                "accessibilityWindow",
                element_window as *const c_void,
                c"@@:",
            ),
            (
                "accessibilityTopLevelUIElement",
                element_window as *const c_void,
                c"@@:",
            ),
            (
                "accessibilityFrame",
                element_frame as *const c_void,
                c"{CGRect={CGPoint=dd}{CGSize=dd}}@:",
            ),
            (
                "isAccessibilityFocused",
                element_focused as *const c_void,
                c"c@:",
            ),
            (
                "accessibilityPerformPress",
                element_press as *const c_void,
                c"c@:",
            ),
            (
                "setAccessibilityFocused:",
                element_set_focus as *const c_void,
                c"v@:c",
            ),
            (
                "setAccessibilityValue:",
                element_set_value as *const c_void,
                c"v@:@",
            ),
            (
                "isAccessibilitySelectorAllowed:",
                element_allowed as *const c_void,
                c"c@::",
            ),
        ] {
            class_addMethod(class, sel(name), method, types.as_ptr());
        }
        objc_registerClassPair(class);
    }
}

#[cfg(test)]
mod tests {
    use super::{Notification, changes};
    use bunny_ui::accessibility::{Action, Role};
    use bunny_ui::layout::Size;
    use bunny_ui::prelude::*;

    #[derive(Clone, Copy)]
    struct Form {
        label: State<String>,
        value: State<String>,
    }
    impl Component for Form {
        fn body(self) -> impl View {
            vstack!(
                text(self.label),
                text_field("Description", self.value.binding())
            )
        }
    }

    #[test]
    fn notifications_follow_real_value_name_and_focus_changes_without_idle_chatter() {
        let app = Form {
            label: State::new("Before".into()),
            value: State::new("Lunch".into()),
        };
        let runtime = Runtime::new();
        runtime.set_accessibility_enabled(true);
        let size = Size {
            width: 480.0,
            height: 640.0,
        };
        runtime.display_frame(&app, size);
        let before = runtime.accessibility_tree();
        let field = before
            .nodes()
            .iter()
            .find(|node| node.role == Role::TextField)
            .unwrap()
            .id;
        app.label.set("After".into());
        app.value.set("Dinner".into());
        runtime.accessibility_action(field, Action::Focus).unwrap();
        runtime.display_frame(&app, size);
        let after = runtime.accessibility_tree();
        assert_eq!(
            changes(Some(&before.nodes()[0]), &after.nodes()[0]).collect::<Vec<_>>(),
            [Notification::Title, Notification::Value]
        );
        assert_eq!(
            changes(before.node(field), after.node(field).unwrap()).collect::<Vec<_>>(),
            [Notification::Value, Notification::Focus]
        );
        for node in after.nodes() {
            assert_eq!(changes(Some(node), node).count(), 0);
        }
        assert_eq!(
            changes(None, after.node(field).unwrap()).collect::<Vec<_>>(),
            [Notification::Focus]
        );
    }
}
