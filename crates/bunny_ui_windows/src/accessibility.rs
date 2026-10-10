//! Demand-driven UI Automation projection. COM sees owned, synchronized data;
//! only the window's event handler can touch Runtime or an application binding.
#[path = "accessibility_abi.rs"]
mod abi;
use super::{Guid, Hresult, Hwnd, UnknownVtbl, WindowHandle};
use abi::*;
use bunny_ui::accessibility::{Action, NodeId, Role};
use bunny_ui::{layout::Rect, runtime::Runtime};
use std::{
    cell::RefCell,
    collections::HashMap,
    ptr,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
};

pub(super) const MESSAGE: u32 = 0x8004;
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Root,
    Node(NodeId),
}
#[derive(Clone)]
struct Node {
    id: NodeId,
    role: Role,
    label: Arc<str>,
    value: Option<Arc<str>>,
    bounds: Rect,
    focused: bool,
    multiline: bool,
}
#[derive(Clone, Default)]
struct Snapshot {
    nodes: Vec<Node>,
    origin: (f64, f64),
    factor: f64,
}
struct Queue {
    owner: Hwnd,
    requested: AtomicBool,
    alive: AtomicBool,
    requests: Mutex<Vec<Request>>,
}
struct Surface {
    window: Hwnd,
    queue: Arc<Queue>,
    alive: AtomicBool,
    snapshot: Mutex<Arc<Snapshot>>,
    providers: Mutex<HashMap<Key, Owned>>,
}
thread_local! {
    static SURFACES: RefCell<HashMap<Hwnd, Weak<Surface>>> = RefCell::new(HashMap::new());
}
fn surface(window: Hwnd) -> Option<Arc<Surface>> {
    SURFACES
        .try_with(|all| all.borrow().get(&window).and_then(Weak::upgrade))
        .ok()
        .flatten()
}
/// Window-thread owner. No Runtime, view, Rc or binding crosses into COM.
pub(crate) struct Accessibility {
    queue: Arc<Queue>,
    surfaces: RefCell<HashMap<Hwnd, Arc<Surface>>>,
}
impl Accessibility {
    pub fn new(window: WindowHandle) -> Self {
        let this = Self {
            queue: Arc::new(Queue {
                owner: window.hwnd,
                requested: AtomicBool::new(false),
                alive: AtomicBool::new(true),
                requests: Mutex::new(Vec::new()),
            }),
            surfaces: RefCell::new(HashMap::new()),
        };
        this.register(window.hwnd);
        this
    }
    fn register(&self, window: Hwnd) -> Arc<Surface> {
        let mut all = self.surfaces.borrow_mut();
        Arc::clone(all.entry(window).or_insert_with(|| {
            let surface = Arc::new(Surface {
                window,
                queue: Arc::clone(&self.queue),
                alive: AtomicBool::new(true),
                snapshot: Mutex::new(Arc::new(Snapshot::default())),
                providers: Mutex::new(HashMap::new()),
            });
            SURFACES.with(|all| all.borrow_mut().insert(window, Arc::downgrade(&surface)));
            surface
        }))
    }
    pub fn requested(&self) -> bool {
        self.queue.requested.load(Ordering::Acquire)
    }
    pub fn update(
        &self,
        runtime: &Runtime,
        window: WindowHandle,
        overlays: &[(String, WindowHandle)],
    ) {
        let mut surfaces = vec![(None, self.register(window.hwnd))];
        for (path, window) in overlays {
            surfaces.push((Some(path.as_str()), self.register(window.hwnd)));
        }
        let retired: Vec<_> = {
            let mut all = self.surfaces.borrow_mut();
            let keys: Vec<_> = all
                .keys()
                .filter(|key| !surfaces.iter().any(|(_, surface)| surface.window == **key))
                .copied()
                .collect();
            keys.into_iter()
                .filter_map(|key| all.remove(&key))
                .collect()
        };
        for surface in retired {
            surface.retire();
        }
        // Overlay registrations precede demand: the first query may target a modal.
        if !self.requested() {
            return;
        }
        let tree = runtime.accessibility_tree();
        let mut changes = Vec::new();
        for (path, surface) in surfaces {
            let snapshot = Snapshot {
                nodes: tree
                    .nodes()
                    .iter()
                    .filter(|node| node.surface.as_deref() == path)
                    .map(|node| Node {
                        id: node.id,
                        role: node.role,
                        label: Arc::clone(&node.label),
                        value: node.value.clone(),
                        bounds: node.bounds,
                        focused: node.focused,
                        multiline: node.multiline,
                    })
                    .collect(),
                origin: super::scene_origin(surface.window),
                factor: super::shared_factor_for(surface.window),
            };
            match surface.replace(snapshot) {
                Ok(previous) => changes.push((surface, previous)),
                Err(error) => eprintln!("bunny_ui: UIA snapshot refused: {error:#x}"),
            }
        }
        // UIA can call back while an event is raised. Publish every surface
        // before notifying, and never hold a provider/snapshot lock across COM.
        for (surface, previous) in changes {
            surface.notify(previous);
        }
    }
}
impl Drop for Accessibility {
    fn drop(&mut self) {
        self.queue.alive.store(false, Ordering::Release);
        for surface in self.surfaces.get_mut().values() {
            surface.retire();
        }
    }
}

/// An accepted request is queued without waiting for the UI thread (COM may
/// itself be dispatched on that thread). Both the adapter and core revalidate it.
#[derive(Clone)]
pub(crate) struct Request {
    surface: Weak<Surface>,
    key: Key,
    action: Action,
}
impl Request {
    pub fn apply(&self, runtime: &Runtime) -> bool {
        let Some(surface) = self.surface.upgrade() else {
            return false;
        };
        if surface.read(self.key).is_err() {
            return false;
        }
        if let Key::Node(id) = self.key
            && runtime
                .accessibility_action(id, self.action.clone())
                .is_err()
        {
            return false;
        }
        if matches!(self.action, Action::Focus | Action::SetText(_)) {
            // UI thread only. SetFocus gives the real window keyboard input;
            // the core already chose the field and the next frame mirrors IME.
            unsafe {
                super::SetFocus(surface.window);
            }
        }
        true
    }
}
pub(super) fn drain(window: Hwnd) {
    let Some(surface) = surface(window) else {
        return;
    };
    let requests = match surface.queue.requests.lock() {
        Ok(mut requests) => std::mem::take(&mut *requests),
        Err(_) => {
            eprintln!("bunny_ui: UIA action queue poisoned");
            return;
        }
    };
    for request in requests {
        super::dispatch_at(window, super::AppEvent::AccessibilityRequest(request));
    }
}
pub(super) fn destroy(window: Hwnd) {
    if let Some(surface) = surface(window) {
        surface.retire();
    }
}
pub(super) fn get_object(window: Hwnd, wparam: usize, lparam: isize) -> Option<isize> {
    // Win32 does not sign extend object IDs in every caller/architecture.
    if lparam as i32 != -25 {
        return None;
    } // UiaRootObjectId
    let surface = surface(window)?;
    if !surface.queue.requested.swap(true, Ordering::AcqRel) {
        super::dispatch_at(surface.queue.owner, super::AppEvent::AccessibilityEnable);
    }
    let provider = surface.provider(Key::Root).ok()?;
    Some(unsafe { UiaReturnRawElementProvider(window, wparam, lparam, provider.simple()) })
}
impl Surface {
    fn read(&self, key: Key) -> Result<(Arc<Snapshot>, Option<Node>), Hresult> {
        let snapshot = self.snapshot.lock().map_err(|_| FAILED)?;
        if !self.alive.load(Ordering::Acquire) {
            return Err(UNAVAILABLE);
        }
        let node = match key {
            Key::Root => None,
            Key::Node(id) => Some(
                snapshot
                    .nodes
                    .iter()
                    .find(|node| node.id == id)
                    .cloned()
                    .ok_or(UNAVAILABLE)?,
            ),
        };
        Ok((snapshot.clone(), node))
    }
    fn provider(self: &Arc<Self>, key: Key) -> Result<Owned, Hresult> {
        let (_, node) = self.read(key)?;
        let mut all = self.providers.lock().map_err(|_| FAILED)?;
        if !self.alive.load(Ordering::Acquire) {
            return Err(UNAVAILABLE);
        }
        Ok(all
            .entry(key)
            .or_insert_with(|| Owned::new(self, key, node.map(|node| node.role)))
            .clone())
    }
    fn replace(&self, snapshot: Snapshot) -> Result<Arc<Snapshot>, Hresult> {
        let previous = {
            let mut current = self.snapshot.lock().map_err(|_| FAILED)?;
            std::mem::replace(&mut *current, Arc::new(snapshot))
        };
        let retired = {
            let snapshot = self.snapshot.lock().map_err(|_| FAILED)?;
            let mut all = self.providers.lock().map_err(|_| FAILED)?;
            let keys: Vec<_> = all
                .keys()
                .filter(|key| match key {
                    Key::Root => false,
                    Key::Node(id) => !snapshot.nodes.iter().any(|node| node.id == *id),
                })
                .copied()
                .collect();
            keys.into_iter()
                .filter_map(|key| all.remove(&key))
                .collect::<Vec<_>>()
        };
        for provider in retired {
            unsafe {
                UiaDisconnectProvider(provider.simple());
            }
        }
        Ok(previous)
    }
    fn retire(&self) {
        if !self.alive.swap(false, Ordering::AcqRel) {
            return;
        }
        let _ = SURFACES.try_with(|all| {
            all.borrow_mut().remove(&self.window);
        });
        let providers = self
            .providers
            .lock()
            .map(|mut all| std::mem::take(&mut *all))
            .unwrap_or_default();
        for provider in providers.into_values() {
            unsafe {
                UiaDisconnectProvider(provider.simple());
            }
        }
    }
    fn notify(self: &Arc<Self>, previous: Arc<Snapshot>) {
        if unsafe { UiaClientsAreListening() } == 0 {
            return;
        }
        let Ok((current, _)) = self.read(Key::Root) else {
            return;
        };
        let changed = previous
            .nodes
            .iter()
            .map(|n| n.id)
            .ne(current.nodes.iter().map(|n| n.id));
        if changed && let Ok(root) = self.provider(Key::Root) {
            unsafe {
                UiaRaiseStructureChangedEvent(root.simple(), 2, ptr::null(), 0);
            }
        }
        for node in &current.nodes {
            let Ok(provider) = self.provider(Key::Node(node.id)) else {
                continue;
            };
            let old = previous.nodes.iter().find(|old| old.id == node.id);
            if node.focused && old.is_none_or(|old| !old.focused) {
                unsafe {
                    UiaRaiseAutomationEvent(provider.simple(), FOCUS_EVENT);
                }
            }
            if let Some(old) = old {
                if old.label != node.label {
                    notify_property(
                        &provider,
                        NAME,
                        Variant::string(&old.label),
                        Variant::string(&node.label),
                    );
                }
                if old.value != node.value && node.role != Role::PasswordField {
                    notify_property(
                        &provider,
                        VALUE_VALUE,
                        Variant::string(old.value.as_deref().unwrap_or("")),
                        Variant::string(node.value.as_deref().unwrap_or("")),
                    );
                }
                if old.bounds != node.bounds
                    || previous.origin != current.origin
                    || previous.factor != current.factor
                {
                    notify_property(
                        &provider,
                        BOUNDS,
                        self.bounds(&previous, Some(old)).and_then(Variant::bounds),
                        self.bounds(&current, Some(node)).and_then(Variant::bounds),
                    );
                }
                if old.focused != node.focused {
                    notify_property(
                        &provider,
                        FOCUSED,
                        Ok(Variant::boolean(old.focused)),
                        Ok(Variant::boolean(node.focused)),
                    );
                }
            }
        }
    }
    fn bounds(&self, snapshot: &Snapshot, node: Option<&Node>) -> Result<UiaRect, Hresult> {
        if let Some(node) = node {
            let mut origin = super::Point { x: 0, y: 0 };
            if unsafe { super::ClientToScreen(self.window, &mut origin) } == 0 {
                return Err(UNAVAILABLE);
            }
            Ok(UiaRect {
                left: f64::from(origin.x)
                    + (node.bounds.origin.x - snapshot.origin.0) * snapshot.factor,
                top: f64::from(origin.y)
                    + (node.bounds.origin.y - snapshot.origin.1) * snapshot.factor,
                width: node.bounds.size.width * snapshot.factor,
                height: node.bounds.size.height * snapshot.factor,
            })
        } else {
            let mut rect = super::Rect::default();
            if unsafe { super::GetWindowRect(self.window, &mut rect) } == 0 {
                return Err(UNAVAILABLE);
            }
            Ok(UiaRect {
                left: f64::from(rect.left),
                top: f64::from(rect.top),
                width: f64::from(rect.width()),
                height: f64::from(rect.height()),
            })
        }
    }
}
fn notify_property(
    provider: &Owned,
    property: i32,
    old: Result<Variant, Hresult>,
    new: Result<Variant, Hresult>,
) {
    match (old, new) {
        (Ok(mut old), Ok(mut new)) => unsafe {
            UiaRaiseAutomationPropertyChangedEvent(provider.simple(), property, old, new);
            VariantClear(&mut old);
            VariantClear(&mut new);
        },
        (Ok(mut value), Err(_)) | (Err(_), Ok(mut value)) => unsafe {
            VariantClear(&mut value);
        },
        _ => {}
    }
}

// Every interface has the same two-word prefix. Its identity and interface set
// are immutable for the COM lifetime, even after its element/window retires.
#[repr(C)]
struct Port<V> {
    table: *const V,
    owner: *mut Provider,
}
#[repr(C)]
struct Provider {
    simple: Port<SimpleVtbl>,
    fragment: Port<FragmentVtbl>,
    root: Port<RootVtbl>,
    invoke: Port<InvokeVtbl>,
    value: Port<ValueVtbl>,
    refs: AtomicU32,
    surface: Weak<Surface>,
    key: Key,
    role: Option<Role>,
}
struct Owned(*mut Provider);
// SAFETY: ports/key/role/Weak are immutable after publication, refs are atomic,
// and Surface contains only synchronized owned data. No COM call touches Rc or
// Runtime. The cache owns a reference; every returned interface owns another.
unsafe impl Send for Owned {}
unsafe impl Sync for Owned {}
impl Owned {
    fn new(surface: &Arc<Surface>, key: Key, role: Option<Role>) -> Self {
        let mut value = Box::new(Provider {
            simple: Port {
                table: &SIMPLE_VTABLE,
                owner: ptr::null_mut(),
            },
            fragment: Port {
                table: &FRAGMENT_VTABLE,
                owner: ptr::null_mut(),
            },
            root: Port {
                table: &ROOT_VTABLE,
                owner: ptr::null_mut(),
            },
            invoke: Port {
                table: &INVOKE_VTABLE,
                owner: ptr::null_mut(),
            },
            value: Port {
                table: &VALUE_VTABLE,
                owner: ptr::null_mut(),
            },
            refs: AtomicU32::new(1),
            surface: Arc::downgrade(surface),
            key,
            role,
        });
        let owner = &mut *value as *mut Provider;
        value.simple.owner = owner;
        value.fragment.owner = owner;
        value.root.owner = owner;
        value.invoke.owner = owner;
        value.value.owner = owner;
        Self(Box::into_raw(value))
    }
    fn simple(&self) -> Object {
        self.0.cast()
    }
    fn interface(&self, iid: &Guid) -> Result<Object, Hresult> {
        let mut out = ptr::null_mut();
        let result = unsafe { query(self.simple(), iid, &mut out) };
        if result < 0 { Err(result) } else { Ok(out) }
    }
}
impl Clone for Owned {
    fn clone(&self) -> Self {
        unsafe {
            retain(self.simple());
        }
        Self(self.0)
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe {
            release(self.simple());
        }
    }
}
unsafe fn provider<'a>(this: Object) -> &'a Provider {
    unsafe { &*(*(this as *const Port<UnknownVtbl>)).owner }
}
unsafe extern "system" fn query(this: Object, iid: *const Guid, out: *mut Object) -> Hresult {
    if out.is_null() {
        return POINTER;
    }
    unsafe {
        *out = ptr::null_mut();
    }
    if iid.is_null() {
        return POINTER;
    }
    let value = unsafe { provider(this) };
    let iid = unsafe { *iid };
    let port: Object = if iid == UNKNOWN || iid == SIMPLE {
        ptr::from_ref(&value.simple).cast_mut().cast()
    } else if iid == FRAGMENT {
        ptr::from_ref(&value.fragment).cast_mut().cast()
    } else if iid == ROOT && value.key == Key::Root {
        ptr::from_ref(&value.root).cast_mut().cast()
    } else if iid == INVOKE && value.role == Some(Role::Button) {
        ptr::from_ref(&value.invoke).cast_mut().cast()
    } else if iid == VALUE && matches!(value.role, Some(Role::TextField | Role::PasswordField)) {
        ptr::from_ref(&value.value).cast_mut().cast()
    } else {
        return NO_INTERFACE;
    };
    unsafe {
        retain(this);
        *out = port;
    }
    OK
}
unsafe extern "system" fn retain(this: Object) -> u32 {
    let old = unsafe { provider(this) }
        .refs
        .fetch_add(1, Ordering::Relaxed);
    if old >= u32::MAX / 2 {
        std::process::abort();
    }
    old + 1
}
unsafe extern "system" fn release(this: Object) -> u32 {
    let value = unsafe { provider(this) };
    let previous = value.refs.fetch_sub(1, Ordering::Release);
    if previous == 1 {
        std::sync::atomic::fence(Ordering::Acquire);
        unsafe {
            drop(Box::from_raw(ptr::from_ref(value).cast_mut()));
        }
    }
    previous - 1
}
fn output<T>(out: *mut T, empty: T, action: impl FnOnce() -> Result<T, Hresult>) -> Hresult {
    if out.is_null() {
        return POINTER;
    }
    // SAFETY: each COM entry point supplies its caller-owned ABI out pointer.
    unsafe {
        *out = empty;
    }
    match action() {
        Ok(value) => {
            unsafe {
                *out = value;
            }
            OK
        }
        Err(error) => error,
    }
}
type Available = (Arc<Surface>, Arc<Snapshot>, Option<Node>);
fn state(value: &Provider) -> Result<Available, Hresult> {
    let surface = value.surface.upgrade().ok_or(UNAVAILABLE)?;
    let (snapshot, node) = surface.read(value.key)?;
    Ok((surface, snapshot, node))
}
unsafe extern "system" fn options(_this: Object, out: *mut i32) -> Hresult {
    // Immutable COM metadata remains available during disconnect/retirement,
    // just like QueryInterface. Only element data and actions become unavailable.
    output(out, 0, || Ok(2 | 32)) // ServerSideProvider | UseComThreading
}
unsafe extern "system" fn pattern(this: Object, pattern: i32, out: *mut Object) -> Hresult {
    output(out, ptr::null_mut(), || {
        let value = unsafe { provider(this) };
        let (_, _, node) = state(value)?;
        let iid = match pattern {
            INVOKE_PATTERN if value.role == Some(Role::Button) => &INVOKE,
            VALUE_PATTERN
                if matches!(value.role, Some(Role::TextField | Role::PasswordField))
                    && node.is_some_and(|node| !node.multiline) =>
            {
                &VALUE
            }
            _ => return Ok(ptr::null_mut()),
        };
        let mut result = ptr::null_mut();
        let hr = unsafe { query(this, iid, &mut result) };
        if hr < 0 { Err(hr) } else { Ok(result) }
    })
}
unsafe extern "system" fn property(this: Object, property: i32, out: *mut Variant) -> Hresult {
    output(out, Variant::empty(), || {
        let (surface, snapshot, node) = state(unsafe { provider(this) })?;
        match property {
            BOUNDS => Variant::bounds(surface.bounds(&snapshot, node.as_ref())?),
            FRAMEWORK => Variant::string("Bunny UI"),
            CONTROL | CONTENT => Ok(Variant::boolean(true)),
            _ => {
                let Some(node) = node else {
                    return Ok(Variant::empty());
                };
                match property {
                    NAME => Variant::string(&node.label),
                    ENABLED => Ok(Variant::boolean(unsafe {
                        abi::IsWindowEnabled(surface.window) != 0
                    })),
                    OFFSCREEN => Ok(Variant::boolean(unsafe {
                        abi::IsIconic(surface.window) != 0
                            || super::IsWindowVisible(surface.window) == 0
                    })),
                    CONTROL_TYPE => Ok(Variant::integer(match node.role {
                        Role::Text => 50020,
                        Role::Button => 50000,
                        Role::TextField | Role::PasswordField => 50004,
                    })),
                    AUTOMATION_ID => Variant::string(&format!("bunny:{}", node.id.get())),
                    FOCUSED => Ok(Variant::boolean(
                        node.focused && window_has_focus(surface.window),
                    )),
                    FOCUSABLE => Ok(Variant::boolean(matches!(
                        node.role,
                        Role::TextField | Role::PasswordField
                    ))),
                    PASSWORD => Ok(Variant::boolean(node.role == Role::PasswordField)),
                    VALUE_VALUE if node.role == Role::PasswordField => Err(DENIED),
                    VALUE_VALUE if node.role == Role::TextField && !node.multiline => {
                        Variant::string(node.value.as_deref().unwrap_or(""))
                    }
                    VALUE_READ_ONLY
                        if matches!(node.role, Role::TextField | Role::PasswordField)
                            && !node.multiline =>
                    {
                        Ok(Variant::boolean(false))
                    }
                    _ => Ok(Variant::empty()),
                }
            }
        }
    })
}
unsafe extern "system" fn host(this: Object, out: *mut Object) -> Hresult {
    output(out, ptr::null_mut(), || {
        let value = unsafe { provider(this) };
        let (surface, _, _) = state(value)?;
        if value.key != Key::Root {
            return Ok(ptr::null_mut());
        }
        let mut result = ptr::null_mut();
        let hr = unsafe { UiaHostProviderFromHwnd(surface.window, &mut result) };
        if hr < 0 { Err(hr) } else { Ok(result) }
    })
}
unsafe extern "system" fn navigate(this: Object, direction: i32, out: *mut Object) -> Hresult {
    output(out, ptr::null_mut(), || {
        let value = unsafe { provider(this) };
        let (surface, snapshot, _) = state(value)?;
        let target = match (value.key, direction) {
            (Key::Root, 0..=2) => None,
            (Key::Root, 3) => snapshot.nodes.first().map(|node| Key::Node(node.id)),
            (Key::Root, 4) => snapshot.nodes.last().map(|node| Key::Node(node.id)),
            (Key::Node(_), 0) => Some(Key::Root),
            (Key::Node(id), 1 | 2) => {
                let index = snapshot
                    .nodes
                    .iter()
                    .position(|node| node.id == id)
                    .ok_or(UNAVAILABLE)?;
                let index = if direction == 1 {
                    index.checked_add(1)
                } else {
                    index.checked_sub(1)
                };
                index
                    .and_then(|index| snapshot.nodes.get(index))
                    .map(|node| Key::Node(node.id))
            }
            (Key::Node(_), 3 | 4) => None,
            _ => return Err(INVALID),
        };
        match target {
            Some(key) => surface.provider(key)?.interface(&FRAGMENT),
            None => Ok(ptr::null_mut()),
        }
    })
}
unsafe extern "system" fn runtime_id(this: Object, out: *mut *mut SafeArray) -> Hresult {
    output(out, ptr::null_mut(), || {
        let value = unsafe { provider(this) };
        state(value)?;
        let Key::Node(id) = value.key else {
            return Ok(ptr::null_mut());
        };
        let array = unsafe { SafeArrayCreateVector(3, 0, 3) }; // VT_I4
        if array.is_null() {
            return Err(NO_MEMORY);
        }
        for (index, mut value) in [3, (id.get() >> 32) as i32, id.get() as i32]
            .into_iter()
            .enumerate()
        {
            let hr = unsafe {
                SafeArrayPutElement(array, &(index as i32), ptr::from_mut(&mut value).cast())
            };
            if hr < 0 {
                unsafe {
                    SafeArrayDestroy(array);
                }
                return Err(hr);
            }
        }
        Ok(array)
    })
}
unsafe extern "system" fn bounds(this: Object, out: *mut UiaRect) -> Hresult {
    output(out, UiaRect::default(), || {
        let (surface, snapshot, node) = state(unsafe { provider(this) })?;
        surface.bounds(&snapshot, node.as_ref())
    })
}
unsafe extern "system" fn embedded(this: Object, out: *mut *mut SafeArray) -> Hresult {
    output(out, ptr::null_mut(), || {
        state(unsafe { provider(this) })?;
        Ok(ptr::null_mut())
    })
}
fn enqueue(value: &Provider, action: Action) -> Hresult {
    let Ok((surface, _, node)) = state(value) else {
        return UNAVAILABLE;
    };
    if matches!(action, Action::SetText(_)) && node.is_some_and(|node| node.multiline) {
        return UNSUPPORTED;
    }
    let supported = matches!(
        (value.role, &action),
        (Some(Role::Button), Action::Activate)
            | (
                Some(Role::TextField | Role::PasswordField),
                Action::Focus | Action::SetText(_)
            )
            | (None, Action::Focus)
    );
    if !supported {
        return UNSUPPORTED;
    }
    let Ok(mut requests) = surface.queue.requests.lock() else {
        return FAILED;
    };
    if !surface.queue.alive.load(Ordering::Acquire) || !surface.alive.load(Ordering::Acquire) {
        return UNAVAILABLE;
    }
    requests.push(Request {
        surface: Arc::downgrade(&surface),
        key: value.key,
        action,
    });
    if unsafe { super::PostMessageW(surface.queue.owner, MESSAGE, 0, 0) } == 0 {
        requests.pop();
        return UNAVAILABLE;
    }
    OK
}
unsafe extern "system" fn focus(this: Object) -> Hresult {
    enqueue(unsafe { provider(this) }, Action::Focus)
}
unsafe extern "system" fn invoke(this: Object) -> Hresult {
    enqueue(unsafe { provider(this) }, Action::Activate)
}
unsafe extern "system" fn fragment_root(this: Object, out: *mut Object) -> Hresult {
    output(out, ptr::null_mut(), || {
        let (surface, _, _) = state(unsafe { provider(this) })?;
        surface.provider(Key::Root)?.interface(&ROOT)
    })
}
unsafe extern "system" fn at_point(this: Object, x: f64, y: f64, out: *mut Object) -> Hresult {
    output(out, ptr::null_mut(), || {
        if !x.is_finite() || !y.is_finite() {
            return Err(INVALID);
        }
        let (surface, snapshot, _) = state(unsafe { provider(this) })?;
        let mut key = Key::Root;
        for node in snapshot.nodes.iter().rev() {
            let bounds = surface.bounds(&snapshot, Some(node))?;
            if x >= bounds.left
                && x < bounds.left + bounds.width
                && y >= bounds.top
                && y < bounds.top + bounds.height
            {
                key = Key::Node(node.id);
                break;
            }
        }
        surface.provider(key)?.interface(&FRAGMENT)
    })
}
unsafe extern "system" fn focused(this: Object, out: *mut Object) -> Hresult {
    output(out, ptr::null_mut(), || {
        let (surface, snapshot, _) = state(unsafe { provider(this) })?;
        if !window_has_focus(surface.window) {
            return Ok(ptr::null_mut());
        }
        match snapshot.nodes.iter().find(|node| node.focused) {
            Some(node) => surface.provider(Key::Node(node.id))?.interface(&FRAGMENT),
            None => Ok(ptr::null_mut()),
        }
    })
}
unsafe extern "system" fn set_value(this: Object, text: *const u16) -> Hresult {
    if text.is_null() {
        return INVALID;
    }
    // IValueProvider takes a caller-owned, NUL-terminated LPCWSTR.
    let mut length = 0;
    unsafe {
        while *text.add(length) != 0 {
            length += 1;
        }
    }
    let text = match String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) }) {
        Ok(text) => text,
        Err(_) => return INVALID,
    };
    enqueue(unsafe { provider(this) }, Action::SetText(text))
}
unsafe extern "system" fn get_value(this: Object, out: *mut *mut u16) -> Hresult {
    output(out, ptr::null_mut(), || {
        let (_, _, node) = state(unsafe { provider(this) })?;
        let node = node.ok_or(UNSUPPORTED)?;
        if node.multiline {
            return Err(UNSUPPORTED);
        }
        if node.role == Role::PasswordField {
            return Err(DENIED);
        }
        let value = Variant::string(node.value.as_deref().unwrap_or(""))?;
        Ok(unsafe { value.data.pointer.cast() }) // ownership of BSTR passes to UIA
    })
}
unsafe extern "system" fn read_only(this: Object, out: *mut i32) -> Hresult {
    output(out, 0, || {
        state(unsafe { provider(this) })?;
        Ok(0)
    })
}
const fn unknown() -> UnknownVtbl {
    UnknownVtbl {
        query_interface: query,
        add_ref: retain,
        release,
    }
}
static SIMPLE_VTABLE: SimpleVtbl = SimpleVtbl {
    unknown: unknown(),
    options,
    pattern,
    property,
    host,
};
static FRAGMENT_VTABLE: FragmentVtbl = FragmentVtbl {
    unknown: unknown(),
    navigate,
    runtime_id,
    bounds,
    embedded,
    focus,
    root: fragment_root,
};
static ROOT_VTABLE: RootVtbl = RootVtbl {
    unknown: unknown(),
    at_point,
    focused,
};
static INVOKE_VTABLE: InvokeVtbl = InvokeVtbl {
    unknown: unknown(),
    invoke,
};
static VALUE_VTABLE: ValueVtbl = ValueVtbl {
    unknown: unknown(),
    set: set_value,
    get: get_value,
    read_only,
};

#[cfg(test)]
mod tests {
    use super::*;
    use bunny_ui::prelude::{State, button, text, text_editor, text_field, vstack};

    fn fixture() -> (Arc<Surface>, Owned, Owned, Owned) {
        let runtime = Runtime::new();
        runtime.set_accessibility_enabled(true);
        let value = State::new("Lunch".to_string());
        let root = vstack!(
            button(text("Save"), || {}),
            text_field("Description", value.binding()),
            text("Read only"),
            text_field("Password", value.binding()).secret(true),
            text_editor("Notes", value.binding())
        );
        let _ = runtime.display_frame(
            &root,
            bunny_ui::layout::Size {
                width: 480.0,
                height: 480.0,
            },
        );
        let tree = runtime.accessibility_tree();
        let nodes: Vec<_> = tree
            .nodes()
            .iter()
            .map(|node| Node {
                id: node.id,
                role: node.role,
                label: Arc::clone(&node.label),
                value: node.value.clone(),
                bounds: node.bounds,
                focused: node.focused,
                multiline: node.multiline,
            })
            .collect();
        let surface = Arc::new(Surface {
            window: 0,
            queue: Arc::new(Queue {
                owner: 0,
                requested: AtomicBool::new(true),
                alive: AtomicBool::new(true),
                requests: Mutex::new(Vec::new()),
            }),
            alive: AtomicBool::new(true),
            snapshot: Mutex::new(Arc::new(Snapshot {
                nodes,
                origin: (0.0, 0.0),
                factor: 1.0,
            })),
            providers: Mutex::new(HashMap::new()),
        });
        let button = surface.provider(Key::Node(tree.nodes()[0].id)).unwrap();
        let field = surface.provider(Key::Node(tree.nodes()[1].id)).unwrap();
        let text = surface.provider(Key::Node(tree.nodes()[2].id)).unwrap();
        (surface, button, field, text)
    }

    #[test]
    fn com_identity_and_refcounts_survive_concurrent_queries_and_retirement() {
        let (surface, button, field, _) = fixture();
        let canonical = field.simple() as usize;
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let field = field.clone();
                std::thread::spawn(move || {
                    for _ in 0..1000 {
                        let port = field.interface(&VALUE).unwrap();
                        let mut identity = ptr::null_mut();
                        unsafe {
                            assert_eq!(query(port, &UNKNOWN, &mut identity), OK);
                            assert_eq!(identity as usize, canonical);
                            release(identity);
                            release(port);
                        }
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(
            unsafe { provider(field.simple()) }
                .refs
                .load(Ordering::Acquire),
            2
        ); // cache + caller
        surface.alive.store(false, Ordering::Release);
        surface.providers.lock().unwrap().clear();
        drop(surface);
        let still_queryable = field.interface(&VALUE).unwrap();
        unsafe {
            let mut result = ptr::dangling_mut::<u16>();
            assert_eq!(get_value(still_queryable, &mut result), UNAVAILABLE);
            assert!(result.is_null());
            release(still_queryable);
            assert_eq!(invoke(button.simple()), UNAVAILABLE);
            let mut flags = 0;
            assert_eq!(options(button.simple(), &mut flags), OK);
            assert_eq!(flags, 2 | 32);
        }
        assert_eq!(
            unsafe { provider(field.simple()) }
                .refs
                .load(Ordering::Acquire),
            1
        );
    }

    #[test]
    fn native_outputs_and_capabilities_follow_com_contract() {
        let (_surface, button, field, text) = fixture();
        unsafe {
            assert_eq!(query(field.simple(), &VALUE, ptr::null_mut()), POINTER);
            let mut result = ptr::dangling_mut::<std::ffi::c_void>();
            assert_eq!(query(field.simple(), &INVOKE, &mut result), NO_INTERFACE);
            assert!(result.is_null());
            assert_eq!(query(field.simple(), ptr::null(), &mut result), POINTER);
            assert_eq!(pattern(text.simple(), VALUE_PATTERN, &mut result), OK);
            assert!(result.is_null());
            assert_eq!(pattern(button.simple(), VALUE_PATTERN, &mut result), OK);
            assert!(result.is_null());
            assert_eq!(navigate(field.simple(), 99, &mut result), INVALID);
            assert!(result.is_null());
            assert_eq!(invoke(text.simple()), UNSUPPORTED);
            assert_eq!(set_value(field.simple(), ptr::null()), INVALID);
            let invalid_utf16 = [0xd800, 0];
            assert_eq!(set_value(field.simple(), invalid_utf16.as_ptr()), INVALID);
            let mut array = ptr::null_mut();
            assert_eq!(runtime_id(field.simple(), &mut array), OK);
            assert!(!array.is_null());
            assert_eq!(SafeArrayDestroy(array), OK);
            let mut value = Variant::empty();
            assert_eq!(property(field.simple(), VALUE_VALUE, &mut value), OK);
            assert_eq!(value.kind, 8);
            assert_eq!(VariantClear(&mut value), OK);
        }
    }

    #[test]
    fn secret_values_are_denied_and_multiline_editors_do_not_claim_value_pattern() {
        let (surface, _, _, _) = fixture();
        let (snapshot, _) = surface.read(Key::Root).unwrap();
        let password = snapshot
            .nodes
            .iter()
            .find(|node| node.role == Role::PasswordField)
            .unwrap();
        let password = surface.provider(Key::Node(password.id)).unwrap();
        let multiline = snapshot.nodes.iter().find(|node| node.multiline).unwrap();
        let multiline = surface.provider(Key::Node(multiline.id)).unwrap();
        unsafe {
            let mut value = ptr::dangling_mut::<u16>();
            assert_eq!(get_value(password.simple(), &mut value), DENIED);
            assert!(value.is_null());
            let mut property_value = Variant::empty();
            assert_eq!(
                property(password.simple(), VALUE_VALUE, &mut property_value),
                DENIED
            );
            assert_eq!(property_value.kind, 0);
            let mut pattern_value = ptr::dangling_mut::<std::ffi::c_void>();
            assert_eq!(
                pattern(multiline.simple(), VALUE_PATTERN, &mut pattern_value),
                OK
            );
            assert!(pattern_value.is_null());
            assert_eq!(get_value(multiline.simple(), &mut value), UNSUPPORTED);
            assert!(value.is_null());
        }
    }
    #[test]
    fn retirement_is_checked_again_before_an_accepted_action_reaches_runtime() {
        let (surface, button, _, _) = fixture();
        let request = Request {
            surface: Arc::downgrade(&surface),
            key: unsafe { provider(button.simple()) }.key,
            action: Action::Activate,
        };
        surface.alive.store(false, Ordering::Release);
        assert!(!request.apply(&Runtime::new()));
        let mut out = Variant::empty();
        assert_eq!(
            unsafe { property(button.simple(), NAME, &mut out) },
            UNAVAILABLE
        );
        assert_eq!(out.kind, 0);
    }
}
