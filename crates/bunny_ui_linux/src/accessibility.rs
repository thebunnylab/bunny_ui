//! AT-SPI Adapter over the runtime's retained semantic projection.
//!
//! ## Wiring
//! Each mounted window keeps an owner and a weak refresh callback. One private
//! accessibility-bus connection joins the existing display poll. Queries and
//! actions run between native dispatches on the UI thread; no model or binding
//! crosses threads. The first client query enables semantic capture. Publishing
//! installs the current tree before emitting notifications.
//!
//! ## Production limits
//! Built-in text, buttons and fields are supported. Detailed text selection,
//! character geometry and virtualized offscreen navigation remain unsupported.
//! Wayland exposes window-relative geometry, never invented screen positions.
//! WPE pixels do not yet expose an embedded accessibility plug.

#[path = "accessibility_bus.rs"]
mod bus;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::CStr;
use std::rc::{Rc, Weak};

use bunny_ui::accessibility::{Action, Node, NodeId, Role};
use bunny_ui::layout::{Point, Rect, Size};
use bunny_ui::prelude::Runtime;
use bus::{Connection, Message, Value};

const ROOT: &str = "/org/a11y/atspi/accessible/root";
const NULL: &str = "/org/a11y/atspi/null";
const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const APPLICATION: &str = "org.a11y.atspi.Application";
const COMPONENT: &str = "org.a11y.atspi.Component";
const ACTION: &str = "org.a11y.atspi.Action";
const TEXT: &str = "org.a11y.atspi.Text";
const EDITABLE: &str = "org.a11y.atspi.EditableText";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

thread_local! {
    static SERVICE: RefCell<Option<Result<Rc<Service>, ()>>> = const { RefCell::new(None) };
    static SEQUENCE: Cell<u64> = const { Cell::new(0) };
}

fn path(prefix: char) -> String {
    SEQUENCE.with(|sequence| {
        let next = sequence
            .get()
            .checked_add(1)
            .expect("accessibility surface identities exhausted");
        sequence.set(next);
        format!("/org/a11y/atspi/accessible/{prefix}{next}")
    })
}
fn node_path(id: NodeId) -> String {
    format!("/org/a11y/atspi/accessible/n{}", id.get())
}

/// Native surface information from the same pool used by presentation.
pub(crate) struct Surface {
    pub(crate) key: Option<String>,
    pub(crate) handle: super::WindowHandle,
    pub(crate) origin: Point,
    pub(crate) size: Size,
    pub(crate) title: String,
}

#[derive(Clone)]
struct Entry {
    path: String,
    parent: String,
    children: Vec<String>,
    name: String,
    kind: Kind,
    bounds: Rect,
    handle: super::WindowHandle,
    origin: Point,
}
#[derive(Clone)]
enum Kind {
    Window,
    Dialog,
    Control(Node),
}
impl Entry {
    fn role(&self) -> (u32, &'static str) {
        match &self.kind {
            Kind::Window => (23, "frame"),
            Kind::Dialog => (16, "dialog"),
            Kind::Control(node) => match node.role {
                Role::Text => (29, "label"),
                Role::Button => (43, "push button"),
                Role::TextField => (79, "entry"),
                Role::PasswordField => (40, "password text"),
            },
        }
    }
    fn interfaces(&self) -> Vec<&'static str> {
        let mut result = vec![ACCESSIBLE, COMPONENT];
        if let Kind::Control(node) = &self.kind {
            match node.role {
                Role::Button => result.push(ACTION),
                Role::Text => result.push(TEXT),
                Role::TextField | Role::PasswordField => result.extend([TEXT, EDITABLE]),
            }
        }
        result
    }
    fn text(&self) -> &str {
        match &self.kind {
            Kind::Control(node) if node.role == Role::Text => &node.label,
            Kind::Control(node) => node.value.as_deref().unwrap_or(""),
            _ => "",
        }
    }
    fn state(&self) -> u32 {
        let mut bits = (1 << 8) | (1 << 24) | (1 << 25) | (1 << 30);
        if let Kind::Control(node) = &self.kind {
            if node.supports(&Action::Focus) {
                bits |= (1 << 7) | (1 << 11) | (1 << if node.multiline { 17 } else { 26 });
            }
            if node.focused {
                bits |= 1 << 12;
            }
        }
        bits
    }
    fn extents(&self, coordinates: u32) -> Result<[i32; 4], Failure> {
        let scale = self.handle.scale_factor();
        let x = (self.bounds.origin.x - self.origin.x) * scale;
        let y = (self.bounds.origin.y - self.origin.y) * scale;
        let (x, y) = match coordinates {
            0 => {
                if !super::is_x11() {
                    return Err(Failure::Unsupported);
                }
                let (dx, dy) = crate::x11::accessibility_origin(
                    self.handle.window as u32,
                    self.handle.panel_slot(),
                )
                .ok_or(Failure::Unavailable)?;
                (x + f64::from(dx), y + f64::from(dy))
            }
            1 | 2 => (x, y),
            _ => return Err(Failure::Arguments),
        };
        Ok([
            x.round() as i32,
            y.round() as i32,
            (self.bounds.size.width * scale).round() as i32,
            (self.bounds.size.height * scale).round() as i32,
        ])
    }
}

/// Mounted window ownership. A weak callback avoids a presentation-owner cycle.
pub(crate) struct Accessibility {
    service: Option<Rc<Service>>,
    runtime: Rc<Runtime>,
    window: usize,
    root: String,
    capture: Cell<bool>,
    retired: Cell<bool>,
    refresh: RefCell<Option<Weak<dyn Fn()>>>,
    surfaces: RefCell<HashMap<Option<String>, String>>,
    entries: RefCell<Vec<Entry>>,
}
impl Accessibility {
    pub(crate) fn new(runtime: Rc<Runtime>, window: usize) -> Rc<Self> {
        let service = SERVICE.with(|slot| {
            let mut slot = slot.borrow_mut();
            slot.get_or_insert_with(|| {
                Service::new().map_err(|reason| {
                    eprintln!("bunny_ui_linux: AT-SPI unavailable: {reason}");
                })
            })
            .as_ref()
            .ok()
            .cloned()
        });
        let result = Rc::new(Self {
            service,
            runtime,
            window,
            root: path('w'),
            capture: Cell::new(false),
            retired: Cell::new(false),
            refresh: RefCell::new(None),
            surfaces: RefCell::new(HashMap::new()),
            entries: RefCell::new(Vec::new()),
        });
        if let Some(service) = &result.service {
            service.windows.borrow_mut().push(Rc::downgrade(&result));
        }
        result
    }
    pub(crate) fn set_refresh(&self, refresh: &Rc<dyn Fn()>) {
        *self.refresh.borrow_mut() = Some(Rc::downgrade(refresh));
    }
    fn redraw(&self) {
        let callback = self.refresh.borrow().as_ref().and_then(Weak::upgrade);
        if !self.retired.get()
            && let Some(callback) = callback
        {
            callback();
        }
    }
    fn activate(&self) {
        if !self.capture.replace(true) {
            self.runtime.set_accessibility_enabled(true);
            self.redraw();
        }
    }
    pub(crate) fn publish(&self, surfaces: Vec<Surface>) {
        if self.retired.get() || self.service.is_none() {
            return;
        }
        let tree = self.runtime.accessibility_tree();
        let mut paths = self.surfaces.borrow_mut();
        paths.retain(|key, _| surfaces.iter().any(|surface| &surface.key == key));
        let mut entries = Vec::new();
        for surface in surfaces {
            let root = paths
                .entry(surface.key.clone())
                .or_insert_with(|| {
                    if surface.key.is_none() {
                        self.root.clone()
                    } else {
                        path('s')
                    }
                })
                .clone();
            let children: Vec<Node> = tree
                .nodes()
                .iter()
                .filter(|node| node.surface.as_deref() == surface.key.as_deref())
                .cloned()
                .collect();
            entries.push(Entry {
                path: root.clone(),
                parent: ROOT.into(),
                children: children.iter().map(|node| node_path(node.id)).collect(),
                name: surface.title,
                kind: if surface.key.is_none() {
                    Kind::Window
                } else {
                    Kind::Dialog
                },
                bounds: Rect {
                    origin: surface.origin,
                    size: surface.size,
                },
                handle: surface.handle,
                origin: surface.origin,
            });
            entries.extend(children.into_iter().map(|node| Entry {
                path: node_path(node.id),
                parent: root.clone(),
                children: Vec::new(),
                name: node.label.to_string(),
                bounds: node.bounds,
                kind: Kind::Control(node),
                handle: surface.handle,
                origin: surface.origin,
            }));
        }
        drop(paths);
        let old = self.entries.replace(entries.clone());
        if let Some(service) = &self.service {
            service.changed(&old, &entries);
        }
    }
    fn retire(&self) {
        if self.retired.replace(true) {
            return;
        }
        let old = self.entries.take();
        if let Some(service) = &self.service {
            service.changed(&old, &[]);
            service
                .windows
                .borrow_mut()
                .retain(|owner| !std::ptr::eq(owner.as_ptr(), self));
        }
    }
}
impl Drop for Accessibility {
    fn drop(&mut self) {
        self.retire();
    }
}

struct Service {
    connection: Connection,
    name: String,
    windows: RefCell<Vec<Weak<Accessibility>>>,
    registration: u32,
    parent: RefCell<Value>,
    id: Cell<i32>,
    connected: Cell<bool>,
    root_children: RefCell<Vec<String>>,
}
impl Service {
    fn new() -> Result<Rc<Self>, String> {
        let connection = Connection::accessibility()?;
        let name = connection.name();
        let request = Message::call(
            c"org.a11y.atspi.Registry",
            c"/org/a11y/atspi/accessible/root",
            c"org.a11y.atspi.Socket",
            c"Embed",
        )?;
        request.append(&[Value::reference(&name, ROOT)])?;
        // The registry calls Application.Id during this handshake. Waiting
        // synchronously for Embed here would deadlock that incoming request.
        let registration = connection.send(&request)?;
        Ok(Rc::new(Self {
            connection,
            name,
            windows: RefCell::new(Vec::new()),
            registration,
            parent: RefCell::new(Value::reference("org.a11y.atspi.Registry", ROOT)),
            id: Cell::new(0),
            connected: Cell::new(true),
            root_children: RefCell::new(Vec::new()),
        }))
    }
    fn live(&self) -> Vec<Rc<Accessibility>> {
        self.windows
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|window| !window.retired.get())
            .collect()
    }
    fn all_entries(&self) -> Vec<Entry> {
        self.live()
            .iter()
            .flat_map(|window| window.entries.borrow().clone())
            .collect()
    }
    fn reference(&self, path: &str) -> Value {
        Value::reference(&self.name, path)
    }
    fn null(&self) -> Value {
        Value::reference("", NULL)
    }
    fn send(&self, message: Result<Message, String>) {
        if let Err(reason) = message.and_then(|message| self.connection.send(&message)) {
            eprintln!("bunny_ui_linux: AT-SPI reply failed: {reason}");
        }
    }
    fn event(&self, path: &str, member: &CStr, detail: &str, first: i32, second: i32, data: Value) {
        if !self.connected.get() {
            return;
        }
        self.send(
            Message::signal(path, c"org.a11y.atspi.Event.Object", member).and_then(|message| {
                message.append(&[
                    Value::text(detail),
                    Value::I32(first),
                    Value::I32(second),
                    Value::Variant(Box::new(data)),
                    Value::Array(c"{sv}", Vec::new()),
                ])?;
                Ok(message)
            }),
        );
    }
    fn changed(&self, old: &[Entry], new: &[Entry]) {
        for previous in old {
            match new.iter().find(|entry| entry.path == previous.path) {
                None => {
                    self.event(
                        &previous.path,
                        c"StateChanged",
                        "defunct",
                        1,
                        0,
                        Value::I32(0),
                    );
                }
                Some(current) => {
                    if previous.name != current.name {
                        self.event(
                            &current.path,
                            c"PropertyChange",
                            "accessible-name",
                            0,
                            0,
                            Value::text(&current.name),
                        );
                    }
                    if previous.state() != current.state()
                        && (previous.state() ^ current.state()) & (1 << 12) != 0
                    {
                        self.event(
                            &current.path,
                            c"StateChanged",
                            "focused",
                            i32::from(current.state() & (1 << 12) != 0),
                            0,
                            Value::I32(0),
                        );
                    }
                    if previous.text() != current.text() {
                        self.event(
                            &current.path,
                            c"TextChanged",
                            "delete",
                            0,
                            text_len(previous.text()),
                            Value::text(previous.text()),
                        );
                        self.event(
                            &current.path,
                            c"TextChanged",
                            "insert",
                            0,
                            text_len(current.text()),
                            Value::text(current.text()),
                        );
                    }
                    if previous.bounds != current.bounds {
                        // Window-relative geometry is available on both display
                        // protocols; screen geometry may not be, so invalidate.
                        self.event(
                            &current.path,
                            c"VisibleDataChanged",
                            "",
                            0,
                            0,
                            Value::I32(0),
                        );
                    }
                }
            }
        }
        let roots = self.children(None);
        let prior_roots = self.root_children.replace(roots.clone());
        self.children_changed(ROOT, &prior_roots, &roots);
        for parent in old
            .iter()
            .chain(new)
            .filter(|entry| !matches!(entry.kind, Kind::Control(_)))
        {
            if old.iter().any(|entry| std::ptr::eq(entry, parent))
                || !old.iter().any(|entry| entry.path == parent.path)
            {
                let previous = old
                    .iter()
                    .find(|entry| entry.path == parent.path)
                    .map_or(&[][..], |entry| entry.children.as_slice());
                let current = new
                    .iter()
                    .find(|entry| entry.path == parent.path)
                    .map_or(&[][..], |entry| entry.children.as_slice());
                self.children_changed(&parent.path, previous, current);
            }
        }
    }
    fn children_changed(&self, parent: &str, old: &[String], new: &[String]) {
        for edit in child_edits(old, new) {
            self.event(
                parent,
                c"ChildrenChanged",
                if edit.add { "add" } else { "remove" },
                edit.index as i32,
                0,
                self.reference(&edit.path),
            );
        }
    }
    fn pump(&self) {
        if !self.connected.get() {
            return;
        }
        if !self.connection.pump() {
            self.connected.set(false);
            eprintln!("bunny_ui_linux: AT-SPI bus disconnected");
            return;
        }
        while let Some(message) = self.connection.pop() {
            if message.reply_serial() == self.registration {
                if let Ok(values) = message.args()
                    && let [value @ Value::Struct(_)] = values.as_slice()
                {
                    *self.parent.borrow_mut() = value.clone();
                } else {
                    eprintln!("bunny_ui_linux: AT-SPI registry refused registration");
                }
                continue;
            }
            if message.kind() != 1 {
                continue;
            }
            // Metadata writes during registration do not count as an AT query.
            if !(message.interface() == PROPERTIES && message.member() == "Set") {
                for window in self.live() {
                    window.activate();
                }
            }
            let response = match self.answer(&message) {
                Ok(values) => message.reply().and_then(|reply| {
                    reply.append(&values)?;
                    Ok(reply)
                }),
                Err(failure) => {
                    if std::env::var_os("BUNNY_ATSPI_TRACE").is_some() {
                        eprintln!(
                            "AT-SPI rejected {} {}.{}: {failure:?}",
                            message.path(),
                            message.interface(),
                            message.member()
                        );
                    }
                    let (name, text) = failure.details();
                    message.error(name, text)
                }
            };
            self.send(response);
        }
    }
    fn answer(&self, request: &Message) -> Result<Vec<Value>, Failure> {
        let path = request.path();
        let interface = request.interface();
        let member = request.member();
        let args = request.args().map_err(|_| Failure::Arguments)?;
        if path == "/org/a11y/atspi/cache"
            && interface == "org.a11y.atspi.Cache"
            && member == "GetItems"
            && args.is_empty()
        {
            // No speculative full-cache payload: clients query the retained
            // objects, and ordinary object events invalidate their caches.
            return Ok(vec![Value::Array(c"((so)(so)(so)iiassusau)", Vec::new())]);
        }
        let owner_entry = self.live().into_iter().find_map(|owner| {
            let entry = owner
                .entries
                .borrow()
                .iter()
                .find(|entry| entry.path == path)
                .cloned()?;
            Some((owner, entry))
        });
        let entry = owner_entry.as_ref().map(|(_, entry)| entry);
        if path != ROOT && entry.is_none() {
            return Err(Failure::Unavailable);
        }
        if interface == PROPERTIES {
            return self.property_call(&path, entry, &member, &args);
        }
        let supported = entry.map_or_else(|| vec![ACCESSIBLE, APPLICATION], Entry::interfaces);
        if !supported.contains(&interface.as_str()) {
            return Err(Failure::Unsupported);
        }
        if interface == ACCESSIBLE {
            let children = self.children(entry);
            return match (member.as_str(), args.as_slice()) {
                ("GetChildren", []) => Ok(vec![Value::Array(
                    c"(so)",
                    children.iter().map(|path| self.reference(path)).collect(),
                )]),
                ("GetChildAtIndex", [Value::I32(index)]) => Ok(vec![
                    children
                        .get(usize::try_from(*index).map_err(|_| Failure::Arguments)?)
                        .map(|path| self.reference(path))
                        .ok_or(Failure::Arguments)?,
                ]),
                ("GetIndexInParent", []) => {
                    Ok(vec![Value::I32(entry.map_or(-1, |entry| {
                        child_index(&self.all_entries(), entry)
                    }))])
                }
                ("GetRole", []) => Ok(vec![Value::U32(entry.map_or(75, |entry| entry.role().0))]),
                ("GetRoleName" | "GetLocalizedRoleName", []) => Ok(vec![Value::text(
                    entry.map_or("application", |entry| entry.role().1),
                )]),
                ("GetState", []) => Ok(vec![Value::Array(
                    c"u",
                    vec![Value::U32(entry.map_or(0, Entry::state)), Value::U32(0)],
                )]),
                ("GetRelationSet", []) => Ok(vec![Value::Array(c"(ua(so))", Vec::new())]),
                ("GetAttributes", []) => Ok(vec![Value::Array(c"{ss}", Vec::new())]),
                ("GetApplication", []) => Ok(vec![self.reference(ROOT)]),
                ("GetInterfaces", []) => Ok(vec![Value::Array(
                    c"s",
                    supported.into_iter().map(Value::text).collect(),
                )]),
                _ => Err(Failure::Unsupported),
            };
        }
        if interface == APPLICATION {
            return match (member.as_str(), args.as_slice()) {
                ("GetLocale", [Value::U32(_)]) => Ok(vec![Value::text(locale())]),
                ("GetApplicationBusAddress", []) => Ok(vec![Value::text("")]),
                _ => Err(Failure::Unsupported),
            };
        }
        let (owner, entry) = owner_entry.ok_or(Failure::Unavailable)?;
        let perform = |action| {
            let Kind::Control(node) = &entry.kind else {
                return Err(Failure::Unsupported);
            };
            owner
                .runtime
                .accessibility_action(node.id, action)
                .map_err(|error| {
                    if std::env::var_os("BUNNY_ATSPI_TRACE").is_some() {
                        eprintln!("AT-SPI runtime refused {}: {error}", node.id.get());
                    }
                    match error {
                        bunny_ui::accessibility::ActionError::Unavailable => Failure::Unavailable,
                        bunny_ui::accessibility::ActionError::Unsupported => Failure::Unsupported,
                    }
                })?;
            owner.redraw();
            Ok(vec![Value::Bool(true)])
        };
        match (interface.as_str(), member.as_str(), args.as_slice()) {
            (ACTION, "DoAction", [Value::I32(0)]) => perform(Action::Activate),
            (ACTION, "GetName" | "GetLocalizedName", [Value::I32(0)]) => {
                Ok(vec![Value::text("click")])
            }
            (ACTION, "GetDescription" | "GetKeyBinding", [Value::I32(0)]) => {
                Ok(vec![Value::text("")])
            }
            (ACTION, "GetActions", []) => Ok(vec![Value::Array(
                c"(sss)",
                vec![Value::Struct(vec![
                    Value::text("click"),
                    Value::text(""),
                    Value::text(""),
                ])],
            )]),
            (EDITABLE, "SetTextContents", [Value::String(text)]) => {
                perform(Action::SetText(text.clone()))
            }
            (TEXT, "GetText", [Value::I32(start), Value::I32(end)]) => {
                Ok(vec![Value::text(text_range(entry.text(), *start, *end)?)])
            }
            (TEXT, "GetCharacterAtOffset", [Value::I32(offset)]) => {
                let ch = entry
                    .text()
                    .chars()
                    .nth(usize::try_from(*offset).map_err(|_| Failure::Arguments)?)
                    .ok_or(Failure::Arguments)?;
                Ok(vec![Value::I32(ch as i32)])
            }
            (COMPONENT, "GrabFocus", []) => perform(Action::Focus),
            (COMPONENT, "GetExtents", [Value::U32(coordinates)]) => Ok(vec![Value::Struct(
                entry
                    .extents(*coordinates)?
                    .into_iter()
                    .map(Value::I32)
                    .collect(),
            )]),
            (COMPONENT, "GetPosition", [Value::U32(coordinates)]) => {
                let rect = entry.extents(*coordinates)?;
                Ok(vec![Value::I32(rect[0]), Value::I32(rect[1])])
            }
            (COMPONENT, "GetSize", []) => {
                let rect = entry.extents(1)?;
                Ok(vec![Value::I32(rect[2]), Value::I32(rect[3])])
            }
            (COMPONENT, "GetLayer", []) => Ok(vec![Value::U32(3)]),
            (COMPONENT, "GetMDIZOrder", []) => Ok(vec![Value::I16(0)]),
            (COMPONENT, "GetAlpha", []) => Ok(vec![Value::F64(1.0)]),
            (COMPONENT, "Contains", [Value::I32(x), Value::I32(y), Value::U32(coordinates)]) => Ok(
                vec![Value::Bool(contains(entry.extents(*coordinates)?, *x, *y))],
            ),
            (
                COMPONENT,
                "GetAccessibleAtPoint",
                [Value::I32(x), Value::I32(y), Value::U32(coordinates)],
            ) => {
                let entries = owner.entries.borrow();
                let hit = entries
                    .iter()
                    .rev()
                    .filter(|candidate| {
                        candidate.path == entry.path || candidate.parent == entry.path
                    })
                    .find(|candidate| {
                        candidate
                            .extents(*coordinates)
                            .is_ok_and(|rect| contains(rect, *x, *y))
                    });
                Ok(vec![hit.map_or_else(
                    || self.null(),
                    |entry| self.reference(&entry.path),
                )])
            }
            _ => Err(Failure::Unsupported),
        }
    }
    fn children(&self, entry: Option<&Entry>) -> Vec<String> {
        entry.map_or_else(
            || {
                self.all_entries()
                    .into_iter()
                    .filter(|entry| entry.parent == ROOT)
                    .map(|entry| entry.path)
                    .collect()
            },
            |entry| entry.children.clone(),
        )
    }
    fn property_call(
        &self,
        path: &str,
        entry: Option<&Entry>,
        member: &str,
        args: &[Value],
    ) -> Result<Vec<Value>, Failure> {
        match (member, args) {
            ("GetAll", [Value::String(interface)]) => {
                let names: &[&str] = match interface.as_str() {
                    ACCESSIBLE => &[
                        "Name",
                        "Description",
                        "Parent",
                        "ChildCount",
                        "Locale",
                        "AccessibleId",
                        "HelpText",
                        "version",
                    ],
                    APPLICATION => &[
                        "ToolkitName",
                        "Version",
                        "ToolkitVersion",
                        "AtspiVersion",
                        "InterfaceVersion",
                        "Id",
                    ],
                    ACTION => &["NActions", "version"],
                    TEXT => &["CharacterCount", "version"],
                    COMPONENT | EDITABLE => &["version"],
                    _ => return Err(Failure::Unsupported),
                };
                let values = names
                    .iter()
                    .map(|name| {
                        self.property(path, entry, interface, name).map(|value| {
                            Value::DictEntry(vec![
                                Value::text(*name),
                                Value::Variant(Box::new(value)),
                            ])
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(vec![Value::Array(c"{sv}", values)])
            }
            ("Get", [Value::String(interface), Value::String(name)]) => Ok(vec![Value::Variant(
                Box::new(self.property(path, entry, interface, name)?),
            )]),
            (
                "Set",
                [
                    Value::String(interface),
                    Value::String(name),
                    Value::Variant(value),
                ],
            ) if path == ROOT && interface == APPLICATION && name == "Id" => {
                let Value::I32(id) = &**value else {
                    return Err(Failure::Arguments);
                };
                self.id.set(*id);
                Ok(Vec::new())
            }
            _ => Err(Failure::Unsupported),
        }
    }
    fn property(
        &self,
        path: &str,
        entry: Option<&Entry>,
        interface: &str,
        name: &str,
    ) -> Result<Value, Failure> {
        let supported = entry.map_or_else(|| vec![ACCESSIBLE, APPLICATION], Entry::interfaces);
        if !supported.contains(&interface) {
            return Err(Failure::Unsupported);
        }
        match (interface, name) {
            (ACCESSIBLE, "Name") => Ok(Value::text(entry.map_or_else(
                || {
                    std::env::current_exe()
                        .ok()
                        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                        .unwrap_or_default()
                },
                |entry| entry.name.clone(),
            ))),
            (ACCESSIBLE, "Description" | "HelpText") => Ok(Value::text("")),
            (ACCESSIBLE, "Parent") => Ok(entry.map_or_else(
                || self.parent.borrow().clone(),
                |entry| self.reference(&entry.parent),
            )),
            (ACCESSIBLE, "ChildCount") => Ok(Value::I32(self.children(entry).len() as i32)),
            (ACCESSIBLE, "AccessibleId") => Ok(Value::text(path)),
            (ACCESSIBLE, "Locale") => Ok(Value::text(locale())),
            (APPLICATION, "ToolkitName") => Ok(Value::text("bunny-ui")),
            (APPLICATION, "Version" | "ToolkitVersion") => {
                Ok(Value::text(env!("CARGO_PKG_VERSION")))
            }
            (APPLICATION, "AtspiVersion") => Ok(Value::text("2.1")),
            (APPLICATION, "Id") => Ok(Value::I32(self.id.get())),
            (APPLICATION, "InterfaceVersion") | (_, "version") => Ok(Value::U32(1)),
            (ACTION, "NActions") => Ok(Value::I32(1)),
            (TEXT, "CharacterCount") => Ok(Value::I32(text_len(
                entry.ok_or(Failure::Unavailable)?.text(),
            ))),
            _ => Err(Failure::Unsupported),
        }
    }
}

struct ChildEdit {
    add: bool,
    index: usize,
    path: String,
}

/// Apply removals and moves to the client's prior child order. A move never
/// retires the object: only its position changes, preserving native identity.
fn child_edits(old: &[String], new: &[String]) -> Vec<ChildEdit> {
    if old == new {
        return Vec::new();
    }
    let wanted: std::collections::HashSet<&str> = new.iter().map(String::as_str).collect();
    let mut current = old.to_vec();
    let mut edits = Vec::new();
    for index in (0..current.len()).rev() {
        if !wanted.contains(current[index].as_str()) {
            edits.push(ChildEdit {
                add: false,
                index,
                path: current.remove(index),
            });
        }
    }
    for (index, path) in new.iter().enumerate() {
        if current.get(index) == Some(path) {
            continue;
        }
        if let Some(previous) = current.iter().position(|candidate| candidate == path) {
            edits.push(ChildEdit {
                add: false,
                index: previous,
                path: current.remove(previous),
            });
        }
        current.insert(index, path.clone());
        edits.push(ChildEdit {
            add: true,
            index,
            path: path.clone(),
        });
    }
    edits
}

fn child_index(entries: &[Entry], entry: &Entry) -> i32 {
    entries
        .iter()
        .filter(|candidate| candidate.parent == entry.parent)
        .position(|candidate| candidate.path == entry.path)
        .map_or(-1, |index| index as i32)
}
fn text_len(text: &str) -> i32 {
    i32::try_from(text.chars().count()).unwrap_or(i32::MAX)
}
fn text_range(text: &str, start: i32, end: i32) -> Result<String, Failure> {
    let length = text_len(text);
    let end = if end == -1 { length } else { end };
    if start < 0 || end < start || end > length {
        return Err(Failure::Arguments);
    }
    Ok(text
        .chars()
        .skip(start as usize)
        .take((end - start) as usize)
        .collect())
}
fn contains(rect: [i32; 4], x: i32, y: i32) -> bool {
    x >= rect[0]
        && y >= rect[1]
        && i64::from(x) < i64::from(rect[0]) + i64::from(rect[2])
        && i64::from(y) < i64::from(rect[1]) + i64::from(rect[3])
}
fn locale() -> String {
    std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_else(|_| "C".into())
}

#[derive(Debug, PartialEq)]
enum Failure {
    Unavailable,
    Unsupported,
    Arguments,
}
impl Failure {
    fn details(&self) -> (&'static CStr, &'static CStr) {
        match self {
            Self::Unavailable => (
                c"org.freedesktop.DBus.Error.UnknownObject",
                c"The accessible object is no longer available",
            ),
            Self::Unsupported => (
                c"org.freedesktop.DBus.Error.NotSupported",
                c"The accessible object does not support this operation",
            ),
            Self::Arguments => (
                c"org.freedesktop.DBus.Error.InvalidArgs",
                c"Invalid accessible operation arguments",
            ),
        }
    }
}

fn service() -> Option<Rc<Service>> {
    SERVICE
        .try_with(|slot| {
            slot.borrow()
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .cloned()
        })
        .ok()
        .flatten()
}

/// Adds the bus descriptor, including write readiness for queued replies.
pub(crate) fn prepare(fds: &mut Vec<super::PollFd>) {
    if let Some(service) = service()
        && service.connected.get()
        && let Some(fd) = service.connection.fd()
    {
        fds.push(super::PollFd {
            fd,
            events: super::POLLIN
                | if service.connection.wants_write() {
                    4
                } else {
                    0
                },
            revents: 0,
        });
    }
}
/// Runs outside display dispatch, with no borrowed runtime or platform client.
pub(crate) fn pump() {
    if let Some(service) = service() {
        service.pump();
    }
}
/// Retires before the native handle can be destroyed or reused.
pub(crate) fn retire_window(window: usize) {
    if let Some(service) = service() {
        for owner in service
            .live()
            .into_iter()
            .filter(|owner| owner.window == window)
        {
            owner.retire();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn text_ranges_use_unicode_scalars_and_validate_bounds() {
        assert_eq!(text_range("a👩‍🚀z", 1, 4), Ok("👩‍🚀".into()));
        assert_eq!(text_range("a👩‍🚀z", 4, -1), Ok("z".into()));
        for (start, end) in [(-1, 0), (3, 2), (0, 6)] {
            assert_eq!(text_range("a👩‍🚀z", start, end), Err(Failure::Arguments));
        }
    }
    #[test]
    fn child_events_preserve_order_through_moves_removals_and_insertions() {
        let old = ["main", "one", "two", "keeper"].map(str::to_owned).to_vec();
        let new = ["keeper", "main", "new", "two"].map(str::to_owned).to_vec();
        let mut replay = old.clone();
        for edit in child_edits(&old, &new) {
            if edit.add {
                replay.insert(edit.index, edit.path);
            } else {
                assert_eq!(replay.remove(edit.index), edit.path);
            }
        }
        assert_eq!(replay, new);
        assert!(child_edits(&new, &new).is_empty());
    }
    #[test]
    fn hit_testing_is_half_open_and_does_not_overflow() {
        assert!(contains([i32::MAX - 1, 0, 10, 10], i32::MAX, 0));
        assert!(!contains([0, 0, 10, 10], 10, 5));
        assert!(!contains([0, 0, 10, 10], 5, -1));
    }
}
