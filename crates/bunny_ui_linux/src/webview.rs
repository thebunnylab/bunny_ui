//! The webview on Linux: WPE WebKit as a tenant of the scene.
//!
//! The engine renders the page OUT of process and hands every frame
//! over as a `wl_shm` buffer (WPEBackend-fdo's SHM lane); the shell
//! copies it once, straight RGBA, and paints it as ONE image where the
//! host stood in the display list. That is the whole island contract
//! here — no platform view, no sandwich: the scene painted after the
//! host is above the page by list order on the CPU raster, GL and
//! Vulkan alike, and the clip open at the mark cuts the page like
//! anything else. The hand is routed by the shell itself:
//! `Runtime::host_at` says which page is under the pointer, and
//! libwpe's dispatch doors take the pointer, the wheel and the
//! keyboard as native events the page trusts.
//!
//! The page-side transport is the WebKit one the Apple tenant speaks
//! (`bunny_ui::host::WEBKIT_*`): the same `messageHandlers` door, the
//! same five channels, the same envelope for an eval's answer. The
//! engine's own main context is pumped by the shell's loops — its
//! file descriptors ride the same `poll` — so no thread of the engine
//! ever touches the scene, and every report lands outside a frame.
//!
//! Not on this lane yet: the EGL zero-copy road (an `EGLImage` straight
//! into the GL tier), and a cursor the page chooses. Both wait for a
//! real GPU to prove them on.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::rc::Rc;

use bunny_ui::action::Modifiers;
use bunny_ui::host::{
    Document, EDITOR_SCRIPT, EditorAction, EditorReport, HostSpec, MouseButton, WEBKIT_BOOT,
    WEBKIT_CONSOLE_HOOK, WEBKIT_NET_WRAP, WebviewCapability, WebviewInput, editor_report,
    js_string, webkit_editor_prelude, webkit_eval_wrap,
};
use bunny_ui::image_engine::ImageSource;
use bunny_ui::layout::{DisplayList, HostPlacement, Rect};

use crate::wpe::{self, Stack};

/// What this backend serves (`docs/webview.md`, the WPE column):
/// console and requests by injected hook, synthetic input as native
/// libwpe events, the editor. Nothing when the stack is not on the
/// box — a capability nothing can serve is not declared.
pub fn capabilities() -> &'static [WebviewCapability] {
    if wpe::loader().is_some() {
        &[
            WebviewCapability::ConsoleMessages,
            WebviewCapability::NetworkRequests,
            WebviewCapability::SyntheticInput,
            WebviewCapability::HtmlEditor,
        ]
    } else {
        &[]
    }
}

/// What a page reports back to the shell — the runtime door each one
/// runs is named by its variant.
#[derive(Clone, Debug)]
pub enum WebviewEvent {
    /// A navigation committed: the url is real from here on.
    Navigated { path: String, url: String },
    /// A document's link was activated — reported, never followed.
    Linked { path: String, url: String },
    /// The editable document's body changed under the hand.
    Changed { path: String, html: String },
    /// A paste the app owns, intercepted.
    Pasted { path: String, html: String, text: String },
    /// A load that refused, by name.
    NavigationFailed { path: String, url: String, why: String },
    /// The page posted on the bus.
    Posted { path: String, body: String },
    /// The page spoke on its console.
    Console { path: String, line: String },
    /// The page made a request (fetch or XHR).
    Requested { path: String, line: String },
    /// An eval's answer, by token.
    EvalDone { token: u64, result: Result<String, String> },
    /// A snapshot's pixels, by token.
    SnapshotDone { token: u64, result: Result<(usize, usize, Vec<u8>), String> },
}

/// A key as the door delivers it — what the page holding the keyboard
/// hears, before the scene's own road.
pub(crate) struct RawKey {
    /// The window holding the keyboard.
    pub window: usize,
    /// The xkb keysym.
    pub keysym: u32,
    /// The X keycode (evdev plus eight) — the engine's `code`.
    pub keycode: u32,
    pub pressed: bool,
    pub modifiers: Modifiers,
}

/// The C side's handle on a host: the two halves of its key.
struct Tag {
    window: usize,
    path: String,
}

/// A mounted document's standing (the Apple tenant's rules, kept).
struct Letter {
    digest: u64,
    expected: bool,
    focus: bool,
}

/// The last frame the page exported, straight RGBA at the physical size.
struct Frame {
    size: (u32, u32),
    rgba: Rc<[u8]>,
}

struct Host {
    window: usize,
    tag: *mut Tag,
    exportable: *mut c_void,
    backend: *mut c_void,
    view: *mut c_void,
    ucm: *mut c_void,
    stamp: String,
    /// The host's box, in the window's layout points.
    frame: Rect,
    scale: f64,
    /// The logical size the engine was told.
    size: (u32, u32),
    shown: bool,
    letter: Option<Letter>,
    latest: Option<Frame>,
    /// A frame arrived and the engine waits for the shell's present.
    pending_complete: bool,
    identity: u64,
    serial: u64,
    /// The buttons held on the page, as libwpe's modifier bits.
    held: u32,
}

type Key = (usize, String);

thread_local! {
    static HOSTS: RefCell<HashMap<Key, Host>> = RefCell::new(HashMap::new());
    /// Where a page's reports land, per window — cloned out to run, so
    /// a report that opens a frame that reports again never re-borrows.
    static DISPATCHERS: RefCell<Vec<(usize, Rc<dyn Fn(WebviewEvent)>)>> = RefCell::new(Vec::new());
    static FOCUSED: RefCell<Option<Key>> = const { RefCell::new(None) };
    static GRAB: RefCell<Option<Key>> = const { RefCell::new(None) };
    /// Answers that must not land inside the frame that asked.
    static LATER: RefCell<Vec<WebviewEvent>> = const { RefCell::new(Vec::new()) };
    /// Windows with a page frame not yet painted.
    static FRESH: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
    static GRAVEYARD: RefCell<Vec<*mut Tag>> = const { RefCell::new(Vec::new()) };
    static ENGINE: Cell<bool> = const { Cell::new(false) };
    static IDENTITIES: Cell<u64> = const { Cell::new(1) };
}

fn ms() -> u32 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u32
}

/// The shell installs the landing spot for a window's pages.
pub(crate) fn add_dispatch(window: usize, dispatch: Rc<dyn Fn(WebviewEvent)>) {
    DISPATCHERS.with(|list| list.borrow_mut().push((window, dispatch)));
}

fn dispatch(event: WebviewEvent) {
    let listeners: Vec<Rc<dyn Fn(WebviewEvent)>> =
        DISPATCHERS.with(|list| list.borrow().iter().map(|(_, d)| Rc::clone(d)).collect());
    for listener in listeners {
        listener(event.clone());
    }
}

/// Delivers the answers held back from inside a frame. Both pumps
/// call it once a turn, outside any dispatch.
pub(crate) fn deliver_pending() {
    loop {
        let next = LATER.with(|later| {
            let mut later = later.borrow_mut();
            if later.is_empty() { None } else { Some(later.remove(0)) }
        });
        match next {
            Some(event) => dispatch(event),
            None => break,
        }
    }
}

/// Is the engine up? The pumps ask before lending their poll.
pub(crate) fn pump_active() -> bool {
    ENGINE.get()
}

pub(crate) fn pump_prepare(fds: &mut Vec<crate::ffi::PollFd>) -> Option<c_int> {
    if !pump_active() {
        return None;
    }
    wpe::pump_prepare(fds)
}

pub(crate) fn pump_after(fds: &mut [crate::ffi::PollFd]) {
    if pump_active() {
        wpe::pump_after(fds);
    }
}

/// A page frame waits to be painted in this window.
pub(crate) fn fresh(window: usize) -> bool {
    FRESH.with(|fresh| fresh.borrow().contains(&window))
}

fn with_host<R>(window: usize, path: &str, body: impl FnOnce(&mut Host) -> R) -> Option<R> {
    HOSTS.with(|hosts| hosts.borrow_mut().get_mut(&(window, path.to_string())).map(body))
}

/// The fdo client table: the SHM lane alone.
static SHM_CLIENT: wpe::ExportableClient = wpe::ExportableClient {
    export_buffer_resource: None,
    export_dmabuf_resource: None,
    export_shm_buffer: Some(export_shm),
    reserved0: None,
    reserved1: None,
};

/// The engine, once: fdo's SHM export. The sandbox is WebKit's own
/// business (bubblewrap around the web process); a box that cannot
/// stand one says `WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1` itself,
/// as the container harness does.
fn ensure_engine(stack: &Stack) -> bool {
    if ENGINE.get() {
        return true;
    }
    if !unsafe { (stack.fdo.initialize_shm)() } {
        eprintln!("bunny_ui_linux: WPEBackend-fdo refused its SHM lane — the webview mounts nothing");
        return false;
    }
    ENGINE.set(true);
    true
}

// MARK: - the reconcile: mount, re-instruct, place, sweep

/// The stamp fingerprints the whole spec — a change re-instructs the
/// mounted view, never re-creates it. A document stamps by its digest.
fn stamp_of(spec: &HostSpec) -> String {
    let HostSpec::Webview { url, document, scripts, console, requests, full_motion } = spec;
    let mut stamp = String::with_capacity(url.len() + 22);
    stamp.push_str(url);
    if let Some(document) = document {
        stamp.push('\u{3}');
        stamp.push_str(&format!("{:016x}", document.digest));
    }
    stamp.push('\u{2}');
    stamp.push(if *console { 'c' } else { '-' });
    stamp.push(if *requests { 'r' } else { '-' });
    stamp.push(if *full_motion { 'm' } else { '-' });
    for script in scripts.iter() {
        stamp.push('\u{1}');
        stamp.push_str(script);
    }
    stamp
}

/// The hosts of one layout, reconciled with the pages mounted for the
/// window: a new box mounts a page, a changed spec re-instructs it,
/// every box places it, and a box that left takes its page with it.
pub(crate) fn reconcile(window: usize, hosts: &[HostPlacement], scale: f64) {
    for host in hosts {
        let stamp = stamp_of(&host.spec);
        let shown = !host.visible.is_empty();
        let standing = HOSTS.with(|all| {
            all.borrow().get(&(window, host.path.clone())).map(|mounted| mounted.stamp == stamp)
        });
        match standing {
            None => create(window, &host.path, &host.spec, stamp, host.frame, scale, shown),
            Some(false) => update(window, &host.path, &host.spec, stamp),
            Some(true) => {}
        }
        place(window, &host.path, host.frame, shown, scale);
    }
    sweep(window, hosts);
}

fn logical_size(frame: Rect) -> (u32, u32) {
    (frame.size.width.round().max(1.0) as u32, frame.size.height.round().max(1.0) as u32)
}

type LoadChanged = extern "C" fn(*mut c_void, c_int, *mut c_void);
type LoadFailed = extern "C" fn(*mut c_void, c_int, *const c_char, *mut wpe::GError, *mut c_void) -> c_int;
type DecidePolicy = extern "C" fn(*mut c_void, *mut c_void, c_int, *mut c_void) -> c_int;
type Create = extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void;
type Terminated = extern "C" fn(*mut c_void, c_int, *mut c_void);
type Message = extern "C" fn(*mut c_void, *mut c_void, *mut c_void);

/// The five channels and their handlers — one function each, so the
/// engine's detail-less callback still knows which channel spoke.
const CHANNELS: [(&CStr, &CStr, Message); 5] = [
    (c"bunny", c"script-message-received::bunny", on_message_bunny),
    (c"bunnyEval", c"script-message-received::bunnyEval", on_message_eval),
    (c"bunnyConsole", c"script-message-received::bunnyConsole", on_message_console),
    (c"bunnyNet", c"script-message-received::bunnyNet", on_message_net),
    (c"bunnyEdit", c"script-message-received::bunnyEdit", on_message_edit),
];

fn create(
    window: usize,
    path: &str,
    spec: &HostSpec,
    stamp: String,
    frame: Rect,
    scale: f64,
    shown: bool,
) {
    let Some(stack) = wpe::loader() else { return };
    if !ensure_engine(stack) {
        return;
    }
    let HostSpec::Webview { url, document, .. } = spec;
    let size = logical_size(frame);
    let tag = Box::into_raw(Box::new(Tag { window, path: path.to_string() }));
    let (exportable, backend, view, ucm) = unsafe {
        let exportable = (stack.fdo.exportable_create)(&SHM_CLIENT, tag.cast(), size.0, size.1);
        if exportable.is_null() {
            eprintln!("bunny_ui_linux: WPEBackend-fdo gave no backend — the webview mounts nothing");
            drop(Box::from_raw(tag));
            return;
        }
        let backend = (stack.fdo.exportable_get_view_backend)(exportable);
        // the engine owns the exportable from here: its destroy is the
        // notify the WebKit backend runs when the view goes
        let view_backend = (stack.webkit.web_view_backend_new)(
            backend,
            Some(stack.fdo.exportable_destroy),
            exportable,
        );
        let view = (stack.webkit.web_view_new)(view_backend);
        if view.is_null() {
            eprintln!("bunny_ui_linux: WPEWebKit gave no view — the webview mounts nothing");
            GRAVEYARD.with(|yard| yard.borrow_mut().push(tag));
            return;
        }
        let ucm = (stack.webkit.web_view_get_user_content_manager)(view);
        let data = tag.cast::<c_void>();
        wpe::connect(stack, view, c"load-changed", on_load_changed as LoadChanged as *const c_void, data);
        wpe::connect(stack, view, c"load-failed", on_load_failed as LoadFailed as *const c_void, data);
        wpe::connect(stack, view, c"decide-policy", on_decide_policy as DecidePolicy as *const c_void, data);
        wpe::connect(stack, view, c"create", on_create as Create as *const c_void, data);
        wpe::connect(
            stack,
            view,
            c"web-process-terminated",
            on_terminated as Terminated as *const c_void,
            data,
        );
        // the channels: connected BEFORE they register, the order the
        // engine documents against a race
        for (name, signal, handler) in CHANNELS {
            wpe::connect(stack, ucm, signal, handler as *const c_void, data);
            (stack.webkit.user_content_manager_register_script_message_handler)(
                ucm,
                name.as_ptr(),
                std::ptr::null(),
            );
        }
        (stack.webkit.settings_set_enable_developer_extras)(
            (stack.webkit.web_view_get_settings)(view),
            1,
        );
        apply_scripts(stack, ucm, spec);
        (stack.wpe.dispatch_set_device_scale_factor)(backend, scale as f32);
        (stack.wpe.dispatch_set_size)(backend, size.0, size.1);
        let mut activity = wpe::ACTIVITY_IN_WINDOW;
        if shown {
            activity |= wpe::ACTIVITY_VISIBLE;
        }
        (stack.wpe.add_activity_state)(backend, activity);
        (exportable, backend, view, ucm)
    };
    let identity = IDENTITIES.with(|next| {
        let id = next.get();
        next.set(id + 1);
        id
    });
    let mut host = Host {
        window,
        tag,
        exportable,
        backend,
        view,
        ucm,
        stamp,
        frame,
        scale,
        size,
        shown,
        letter: None,
        latest: None,
        pending_complete: false,
        identity,
        serial: 0,
        held: 0,
    };
    match document {
        Some(document) => load_document(stack, &mut host, document),
        None => unsafe { load_uri(stack, view, url) },
    }
    HOSTS.with(|hosts| hosts.borrow_mut().insert((window, path.to_string()), host));
}

/// The document-start set, in the Apple tenant's fixed order: the bus
/// first, then the hooks the app DECLARED, then the editor for an
/// editable document, then the app's own scripts.
unsafe fn apply_scripts(stack: &Stack, ucm: *mut c_void, spec: &HostSpec) {
    let HostSpec::Webview { scripts, console, requests, document, .. } = spec;
    unsafe {
        (stack.webkit.user_content_manager_remove_all_scripts)(ucm);
        add_script(stack, ucm, WEBKIT_BOOT);
        if *console {
            add_script(stack, ucm, WEBKIT_CONSOLE_HOOK);
        }
        if *requests {
            add_script(stack, ucm, WEBKIT_NET_WRAP);
        }
        if let Some(document) = document
            && document.editable
        {
            add_script(stack, ucm, &webkit_editor_prelude(document));
            add_script(stack, ucm, EDITOR_SCRIPT);
        }
        for script in scripts.iter() {
            add_script(stack, ucm, script);
        }
    }
}

/// One user script at document start, main frame only.
unsafe fn add_script(stack: &Stack, ucm: *mut c_void, source: &str) {
    let Ok(source) = CString::new(source) else { return };
    unsafe {
        let script = (stack.webkit.user_script_new)(
            source.as_ptr(),
            wpe::INJECT_TOP_FRAME,
            wpe::INJECT_AT_DOCUMENT_START,
            std::ptr::null(),
            std::ptr::null(),
        );
        if script.is_null() {
            return;
        }
        (stack.webkit.user_content_manager_add_script)(ucm, script);
        (stack.webkit.user_script_unref)(script);
    }
}

/// Loads a document from MEMORY — the sealed html the spec holds, the
/// base the engine resolves relative references by. Filed first,
/// loaded second: the policy asks about this load, and finds the
/// letter expecting it.
fn load_document(stack: &Stack, host: &mut Host, document: &Document) {
    host.letter = Some(Letter { digest: document.digest, expected: true, focus: document.focus });
    let Ok(content) = CString::new(document.sealed()) else { return };
    let base = (!document.base.is_empty()).then(|| CString::new(&*document.base).ok()).flatten();
    unsafe {
        (stack.webkit.web_view_load_html)(
            host.view,
            content.as_ptr(),
            base.as_ref().map_or(std::ptr::null(), |base| base.as_ptr()),
        );
    }
}

unsafe fn load_uri(stack: &Stack, view: *mut c_void, url: &str) {
    let Ok(url) = CString::new(url) else { return };
    unsafe { (stack.webkit.web_view_load_uri)(view, url.as_ptr()) }
}

/// Where the engine is right now — the committed url, the same string
/// `Navigated` reported.
unsafe fn current_uri(stack: &Stack, view: *mut c_void) -> Option<String> {
    let uri = unsafe { (stack.webkit.web_view_get_uri)(view) };
    if uri.is_null() {
        return None;
    }
    Some(unsafe { wpe::text_of(uri) })
}

/// Re-instructs a MOUNTED page after its spec changed — the Apple
/// tenant's rules: the scripts are replaced, a document compares by
/// its digest, a url by where the engine already is.
fn update(window: usize, path: &str, spec: &HostSpec, stamp: String) {
    let Some(stack) = wpe::loader() else { return };
    let HostSpec::Webview { url, document, .. } = spec;
    with_host(window, path, |host| {
        host.stamp = stamp;
        unsafe { apply_scripts(stack, host.ucm, spec) };
        match document {
            Some(document) => {
                if host.letter.as_ref().map(|letter| letter.digest) != Some(document.digest) {
                    load_document(stack, host, document);
                }
            }
            None => {
                host.letter = None;
                let current = unsafe { current_uri(stack, host.view) };
                if current.as_deref() != Some(&**url) {
                    unsafe { load_uri(stack, host.view, url) };
                }
            }
        }
    });
}

/// Places a page: the engine learns a new logical size, a new scale,
/// and whether the box shows at all.
fn place(window: usize, path: &str, frame: Rect, shown: bool, scale: f64) {
    let Some(stack) = wpe::loader() else { return };
    with_host(window, path, |host| {
        host.frame = frame;
        let size = logical_size(frame);
        unsafe {
            if (host.scale - scale).abs() > f64::EPSILON {
                host.scale = scale;
                (stack.wpe.dispatch_set_device_scale_factor)(host.backend, scale as f32);
            }
            if host.size != size {
                host.size = size;
                (stack.wpe.dispatch_set_size)(host.backend, size.0, size.1);
            }
            if host.shown != shown {
                host.shown = shown;
                if shown {
                    (stack.wpe.add_activity_state)(host.backend, wpe::ACTIVITY_VISIBLE);
                } else {
                    (stack.wpe.remove_activity_state)(host.backend, wpe::ACTIVITY_VISIBLE);
                }
            }
        }
    });
}

/// Forgets the pages whose hosts left this window's scene.
fn sweep(window: usize, alive: &[HostPlacement]) {
    let gone: Vec<Key> = HOSTS.with(|hosts| {
        hosts
            .borrow()
            .keys()
            .filter(|(owner, path)| {
                *owner == window && !alive.iter().any(|host| host.path == *path)
            })
            .cloned()
            .collect()
    });
    for key in gone {
        destroy(&key);
    }
}

/// Unmounts one page: the view goes, and the backend and the
/// exportable go with it (the notify). The tag outlives them — a
/// callback in flight may still read it — and is freed at the end.
fn destroy(key: &Key) {
    let Some(host) = HOSTS.with(|hosts| hosts.borrow_mut().remove(key)) else { return };
    FOCUSED.with(|focused| {
        if focused.borrow().as_ref() == Some(key) {
            *focused.borrow_mut() = None;
        }
    });
    GRAB.with(|grab| {
        if grab.borrow().as_ref() == Some(key) {
            *grab.borrow_mut() = None;
        }
    });
    if let Some(stack) = wpe::loader() {
        unsafe { (stack.gobject.object_unref)(host.view) };
    }
    GRAVEYARD.with(|yard| yard.borrow_mut().push(host.tag));
}

/// A window closed: its pages go, and its landing spot with them.
pub(crate) fn teardown(window: usize) {
    let mine: Vec<Key> =
        HOSTS.with(|hosts| hosts.borrow().keys().filter(|(owner, _)| *owner == window).cloned().collect());
    for key in mine {
        destroy(&key);
    }
    DISPATCHERS.with(|list| list.borrow_mut().retain(|(owner, _)| *owner != window));
    FRESH.with(|fresh| fresh.borrow_mut().remove(&window));
}

/// The pump is leaving: every page goes, the engine reaps what it
/// started, and the tags are freed at last.
pub(crate) fn teardown_all() {
    let all: Vec<Key> = HOSTS.with(|hosts| hosts.borrow().keys().cloned().collect());
    for key in all {
        destroy(&key);
    }
    DISPATCHERS.with(|list| list.borrow_mut().clear());
    if pump_active() {
        wpe::pump_settle(32);
        wpe::pump_release();
    }
    GRAVEYARD.with(|yard| {
        for tag in yard.borrow_mut().drain(..) {
            unsafe { drop(Box::from_raw(tag)) };
        }
    });
}

// MARK: - the frames

/// fdo exported a frame: copied out at once, straight RGBA, and the
/// buffer goes back to the engine before this returns. The window is
/// marked fresh and the pump woken — the next present paints it.
unsafe extern "C" fn export_shm(data: *mut c_void, buffer: *mut c_void) {
    let Some(stack) = wpe::loader() else { return };
    let tag = unsafe { &*(data as *const Tag) };
    let key: Key = (tag.window, tag.path.clone());
    let exportable = HOSTS.with(|hosts| hosts.borrow().get(&key).map(|host| host.exportable));
    let Some(exportable) = exportable else { return };
    let frame = unsafe { read_shm(stack, buffer) };
    unsafe { (stack.fdo.exportable_dispatch_release_shm_exported_buffer)(exportable, buffer) };
    let Some(frame) = frame else { return };
    HOSTS.with(|hosts| {
        if let Some(host) = hosts.borrow_mut().get_mut(&key) {
            if crate::trace::active() {
                crate::trace::mark(
                    "W",
                    format_args!("page={} {}x{} serial={}", tag.path, frame.size.0, frame.size.1, host.serial + 1),
                );
            }
            host.latest = Some(frame);
            host.serial += 1;
            host.pending_complete = true;
        }
    });
    FRESH.with(|fresh| fresh.borrow_mut().insert(tag.window));
    crate::ffi::wake_from_any_thread();
}

/// The buffer's pixels as the raster wants them: B G R A premultiplied
/// words become R G B A straight bytes; an XRGB buffer is opaque.
unsafe fn read_shm(stack: &Stack, buffer: *mut c_void) -> Option<Frame> {
    unsafe {
        let shm = (stack.fdo.shm_exported_buffer_get_shm_buffer)(buffer);
        if shm.is_null() {
            return None;
        }
        (stack.wl.shm_buffer_begin_access)(shm);
        let data = (stack.wl.shm_buffer_get_data)(shm) as *const u8;
        let stride = (stack.wl.shm_buffer_get_stride)(shm);
        let width = (stack.wl.shm_buffer_get_width)(shm);
        let height = (stack.wl.shm_buffer_get_height)(shm);
        let format = (stack.wl.shm_buffer_get_format)(shm);
        let frame = if data.is_null() || width <= 0 || height <= 0 || stride < width * 4 {
            None
        } else {
            let (width, height, stride) = (width as usize, height as usize, stride as usize);
            let mut rgba = vec![0u8; width * height * 4];
            // ARGB carries alpha, premultiplied; XRGB (and any other
            // word) is opaque
            let opaque = format != wpe::SHM_ARGB8888;
            for row in 0..height {
                let source = data.add(row * stride);
                let line = &mut rgba[row * width * 4..][..width * 4];
                for (column, pixel) in line.chunks_exact_mut(4).enumerate() {
                    let word = source.add(column * 4);
                    let (b, g, r, a) = (*word, *word.add(1), *word.add(2), *word.add(3));
                    let a = if opaque { 255 } else { a };
                    if a == 0 || a == 255 {
                        pixel.copy_from_slice(&[r, g, b, a]);
                    } else {
                        // premultiplied → straight, the raster's law
                        let straight = |c: u8| ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
                        pixel.copy_from_slice(&[straight(r), straight(g), straight(b), a]);
                    }
                }
            }
            Some(Frame { size: (width as u32, height as u32), rgba: Rc::from(rgba) })
        };
        (stack.wl.shm_buffer_end_access)(shm);
        frame
    }
}

/// The display list with every page painted in at its host's mark —
/// the picture is the host's box, at the identity of the frame it
/// shows (a still page uploads nothing twice).
pub(crate) fn paint_into(window: usize, display: &DisplayList, hosts: &[HostPlacement]) -> DisplayList {
    FRESH.with(|fresh| fresh.borrow_mut().remove(&window));
    let pictures: Vec<(usize, Rect, ImageSource)> = HOSTS.with(|all| {
        let all = all.borrow();
        hosts
            .iter()
            .filter_map(|placement| {
                let host = all.get(&(window, placement.path.clone()))?;
                let frame = host.latest.as_ref()?;
                Some((
                    placement.mark,
                    placement.frame,
                    ImageSource::rgba(
                        (host.identity << 40) ^ host.serial,
                        frame.size,
                        Rc::clone(&frame.rgba),
                    ),
                ))
            })
            .collect()
    });
    if pictures.is_empty() { display.clone() } else { display.with_host_pixels(&pictures) }
}

/// The shell presented: every page whose frame went up is told, and
/// renders its next — the engine is paced by the shell's own present.
pub(crate) fn frame_presented(window: usize) {
    let Some(stack) = wpe::loader() else { return };
    HOSTS.with(|hosts| {
        for host in hosts.borrow_mut().values_mut() {
            if host.window == window && host.pending_complete {
                host.pending_complete = false;
                unsafe { (stack.fdo.exportable_dispatch_frame_complete)(host.exportable) };
            }
        }
    });
}

// MARK: - the hand

fn wpe_modifiers(modifiers: Modifiers) -> u32 {
    let mut bits = 0;
    if modifiers.shift {
        bits |= wpe::MOD_SHIFT;
    }
    // the shell maps the Control key to `command` (the chord law)
    if modifiers.command {
        bits |= wpe::MOD_CONTROL;
    }
    if modifiers.option {
        bits |= wpe::MOD_ALT;
    }
    if modifiers.control {
        bits |= wpe::MOD_META;
    }
    bits
}

/// libwpe's button number and its held bit.
fn button_of(button: MouseButton) -> (u32, u32) {
    match button {
        MouseButton::Left => (1, wpe::BUTTON1),
        MouseButton::Right => (2, wpe::BUTTON2),
        MouseButton::Middle => (3, wpe::BUTTON3),
    }
}

/// A point in the window's layout points, in the page's own physical
/// pixels — what libwpe takes.
fn local(host: &Host, x: f64, y: f64) -> (c_int, c_int) {
    (
        ((x - host.frame.origin.x) * host.scale).round() as c_int,
        ((y - host.frame.origin.y) * host.scale).round() as c_int,
    )
}

/// The page holding a press, else the one under the point.
pub(crate) fn grab_or(window: usize, hit: Option<String>) -> Option<String> {
    GRAB.with(|grab| {
        grab.borrow()
            .as_ref()
            .filter(|(owner, _)| *owner == window)
            .map(|(_, path)| path.clone())
            .or(hit)
    })
}

/// The press the page holds, taken back at the release.
pub(crate) fn take_grab(window: usize) -> Option<String> {
    GRAB.with(|grab| {
        let held = grab.borrow().as_ref().is_some_and(|(owner, _)| *owner == window);
        if held { grab.borrow_mut().take().map(|(_, path)| path) } else { None }
    })
}

unsafe fn pointer(stack: &Stack, host: &Host, kind: c_int, x: c_int, y: c_int, button: u32, state: u32, modifiers: u32) {
    let mut event = wpe::PointerEvent { kind, time: ms(), x, y, button, state, modifiers };
    unsafe { (stack.wpe.dispatch_pointer_event)(host.backend, &mut event) };
}

pub(crate) fn pointer_motion(window: usize, path: &str, x: f64, y: f64, modifiers: Modifiers) {
    let Some(stack) = wpe::loader() else { return };
    with_host(window, path, |host| {
        let (px, py) = local(host, x, y);
        unsafe { pointer(stack, host, wpe::POINTER_MOTION, px, py, 0, 0, wpe_modifiers(modifiers) | host.held) };
    });
}

/// A press on the page: it holds the pointer until the release.
pub(crate) fn press(window: usize, path: &str, x: f64, y: f64, button: MouseButton, modifiers: Modifiers) {
    let Some(stack) = wpe::loader() else { return };
    let (number, bit) = button_of(button);
    let landed = with_host(window, path, |host| {
        host.held |= bit;
        let (px, py) = local(host, x, y);
        unsafe { pointer(stack, host, wpe::POINTER_BUTTON, px, py, number, 1, wpe_modifiers(modifiers) | host.held) };
    });
    if landed.is_some() {
        GRAB.with(|grab| *grab.borrow_mut() = Some((window, path.to_string())));
    }
}

pub(crate) fn release(window: usize, path: &str, x: f64, y: f64, button: MouseButton, modifiers: Modifiers) {
    let Some(stack) = wpe::loader() else { return };
    let (number, bit) = button_of(button);
    with_host(window, path, |host| {
        host.held &= !bit;
        let (px, py) = local(host, x, y);
        unsafe { pointer(stack, host, wpe::POINTER_BUTTON, px, py, number, 0, wpe_modifiers(modifiers) | host.held) };
    });
}

/// The wheel, in the engine's own sign (positive `dy` is up — what
/// libwpe's smooth axis takes too).
pub(crate) fn wheel(window: usize, path: &str, x: f64, y: f64, dx: f64, dy: f64) {
    let Some(stack) = wpe::loader() else { return };
    with_host(window, path, |host| {
        let (px, py) = local(host, x, y);
        let mut event = wpe::Axis2dEvent {
            base: wpe::AxisEvent {
                kind: wpe::AXIS_MOTION_SMOOTH | wpe::AXIS_MASK_2D,
                time: ms(),
                x: px,
                y: py,
                axis: 0,
                value: 0,
                modifiers: host.held,
            },
            x_axis: dx,
            y_axis: dy,
        };
        unsafe { (stack.wpe.dispatch_axis_event)(host.backend, &mut event.base) };
    });
}

/// The keyboard is the page's — a click landed in it, or its editable
/// document asked at the commit.
pub(crate) fn focus(window: usize, path: &str) {
    let key: Key = (window, path.to_string());
    if FOCUSED.with(|focused| focused.borrow().as_ref() == Some(&key)) {
        return;
    }
    unfocus();
    let Some(stack) = wpe::loader() else { return };
    let landed = with_host(window, path, |host| unsafe {
        (stack.wpe.add_activity_state)(host.backend, wpe::ACTIVITY_FOCUSED);
    });
    if landed.is_some() {
        FOCUSED.with(|focused| *focused.borrow_mut() = Some(key));
    }
}

/// The keyboard goes back to the scene.
pub(crate) fn unfocus() {
    let Some(key) = FOCUSED.with(|focused| focused.borrow_mut().take()) else { return };
    let Some(stack) = wpe::loader() else { return };
    with_host(key.0, &key.1, |host| unsafe {
        (stack.wpe.remove_activity_state)(host.backend, wpe::ACTIVITY_FOCUSED);
    });
}

/// A key at the door: the page holding the keyboard in that window
/// takes it as a native event, and the scene's road stays shut.
pub(crate) fn takes_key(raw: &RawKey) -> bool {
    let Some(key) = FOCUSED.with(|focused| focused.borrow().clone()) else { return false };
    if key.0 != raw.window {
        return false;
    }
    let Some(stack) = wpe::loader() else { return false };
    with_host(key.0, &key.1, |host| {
        let mut event = wpe::KeyboardEvent {
            time: ms(),
            key_code: raw.keysym,
            hardware_key_code: raw.keycode,
            pressed: raw.pressed,
            modifiers: wpe_modifiers(raw.modifiers) | host.held,
        };
        unsafe { (stack.wpe.dispatch_keyboard_event)(host.backend, &mut event) };
    })
    .is_some()
}

/// The keysym for a key the vocabulary names — the names a page uses,
/// and a single character as itself.
fn keysym_named(name: &str) -> Option<u32> {
    Some(match name {
        "Enter" => 0xff0d,
        "Tab" => 0xff09,
        "Escape" => 0xff1b,
        "Backspace" => 0xff08,
        "Delete" => 0xffff,
        "ArrowUp" => 0xff52,
        "ArrowDown" => 0xff54,
        "ArrowLeft" => 0xff51,
        "ArrowRight" => 0xff53,
        "Home" => 0xff50,
        "End" => 0xff57,
        "PageUp" => 0xff55,
        "PageDown" => 0xff56,
        "Space" => 0x20,
        _ => {
            let mut characters = name.chars();
            let (character, rest) = (characters.next()?, characters.next());
            if rest.is_some() {
                return None;
            }
            let code = character as u32;
            // xkb: Latin-1 is its own keysym, the rest ride 0x0100_0000
            if code < 0x100 { code } else { 0x0100_0000 | code }
        }
    })
}

/// A synthetic event: the pointer and the keys as native libwpe events
/// the page trusts; the one text insert by script, the way a paste
/// lands (libwpe has no text door).
pub(crate) fn input(window: usize, path: &str, event: &WebviewInput) {
    let Some(stack) = wpe::loader() else { return };
    let css = |host: &Host, x: f64, y: f64| -> (c_int, c_int) {
        ((x * host.scale).round() as c_int, (y * host.scale).round() as c_int)
    };
    match event {
        WebviewInput::Click { x, y, clicks, button } => {
            let (number, bit) = button_of(*button);
            with_host(window, path, |host| {
                let (px, py) = css(host, *x, *y);
                for _ in 0..(*clicks).clamp(1, 3) {
                    unsafe {
                        pointer(stack, host, wpe::POINTER_BUTTON, px, py, number, 1, host.held | bit);
                        pointer(stack, host, wpe::POINTER_BUTTON, px, py, number, 0, host.held);
                    }
                }
            });
        }
        WebviewInput::Hover { x, y } => {
            with_host(window, path, |host| {
                let (px, py) = css(host, *x, *y);
                unsafe { pointer(stack, host, wpe::POINTER_MOTION, px, py, 0, 0, host.held) };
            });
        }
        WebviewInput::Down { x, y, button, modifiers, .. } => {
            let (number, bit) = button_of(*button);
            with_host(window, path, |host| {
                host.held |= bit;
                let (px, py) = css(host, *x, *y);
                unsafe { pointer(stack, host, wpe::POINTER_BUTTON, px, py, number, 1, wpe_modifiers(*modifiers) | host.held) };
            });
        }
        WebviewInput::Drag { x, y, button, modifiers } => {
            let (_, bit) = button_of(*button);
            with_host(window, path, |host| {
                let (px, py) = css(host, *x, *y);
                unsafe { pointer(stack, host, wpe::POINTER_MOTION, px, py, 0, 0, wpe_modifiers(*modifiers) | host.held | bit) };
            });
        }
        WebviewInput::Up { x, y, button, modifiers, .. } => {
            let (number, bit) = button_of(*button);
            with_host(window, path, |host| {
                host.held &= !bit;
                let (px, py) = css(host, *x, *y);
                unsafe { pointer(stack, host, wpe::POINTER_BUTTON, px, py, number, 0, wpe_modifiers(*modifiers) | host.held) };
            });
        }
        WebviewInput::Scroll { x, y, dx, dy } => {
            // the page's signs (down and right count up) → the axis's
            with_host(window, path, |host| {
                let (px, py) = css(host, *x, *y);
                let mut axis = wpe::Axis2dEvent {
                    base: wpe::AxisEvent {
                        kind: wpe::AXIS_MOTION_SMOOTH | wpe::AXIS_MASK_2D,
                        time: ms(),
                        x: px,
                        y: py,
                        axis: 0,
                        value: 0,
                        modifiers: host.held,
                    },
                    x_axis: -dx,
                    y_axis: -dy,
                };
                unsafe { (stack.wpe.dispatch_axis_event)(host.backend, &mut axis.base) };
            });
        }
        WebviewInput::Type { text } => {
            let script = format!("document.execCommand('insertText', false, {});", js_string(text));
            with_host(window, path, |host| unsafe { run_script(stack, host.view, &script) });
        }
        WebviewInput::Key { key } => {
            let Some(keysym) = keysym_named(key) else { return };
            with_host(window, path, |host| {
                for pressed in [true, false] {
                    let mut event = wpe::KeyboardEvent {
                        time: ms(),
                        key_code: keysym,
                        hardware_key_code: 0,
                        pressed,
                        modifiers: host.held,
                    };
                    unsafe { (stack.wpe.dispatch_keyboard_event)(host.backend, &mut event) };
                }
            });
        }
    }
}

// MARK: - the doors the app's handle opens

unsafe fn run_script(stack: &Stack, view: *mut c_void, js: &str) {
    let Ok(js) = CString::new(js) else { return };
    unsafe {
        (stack.webkit.web_view_evaluate_javascript)(
            view,
            js.as_ptr(),
            -1,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null_mut(),
        );
    }
}

pub(crate) fn navigate(window: usize, path: &str, url: &str) {
    let Some(stack) = wpe::loader() else { return };
    with_host(window, path, |host| unsafe { load_uri(stack, host.view, url) });
}

pub(crate) fn back(window: usize, path: &str) {
    let Some(stack) = wpe::loader() else { return };
    with_host(window, path, |host| unsafe { (stack.webkit.web_view_go_back)(host.view) });
}

pub(crate) fn forward(window: usize, path: &str) {
    let Some(stack) = wpe::loader() else { return };
    with_host(window, path, |host| unsafe { (stack.webkit.web_view_go_forward)(host.view) });
}

/// One editing action on the document — the allowlist's script. The
/// editor takes the keyboard back first, except for the app's own
/// write of the whole body.
pub(crate) fn edit(window: usize, path: &str, action: &EditorAction) {
    let Some(stack) = wpe::loader() else { return };
    let script = action.script();
    if script.is_empty() {
        return;
    }
    if !matches!(action, EditorAction::SetHtml(_)) {
        focus(window, path);
    }
    with_host(window, path, |host| unsafe { run_script(stack, host.view, &script) });
}

/// Evaluates `js` as an EXPRESSION; the answer rides `bunnyEval`.
pub(crate) fn eval(window: usize, path: &str, token: u64, js: &str, raw: bool) -> Result<(), String> {
    let Some(stack) = wpe::loader() else { return Err(String::from("no WPE WebKit on this box")) };
    let wrapped = webkit_eval_wrap(token, js, raw);
    with_host(window, path, |host| unsafe { run_script(stack, host.view, &wrapped) })
        .ok_or_else(|| String::from("no page mounted at that path"))
}

/// The last frame the page exported, as the snapshot — answered on
/// the next turn of the pump, never inside the frame that asked.
pub(crate) fn snapshot(window: usize, path: &str, token: u64) -> Result<(), String> {
    let frame = with_host(window, path, |host| {
        host.latest.as_ref().map(|frame| (frame.size, frame.rgba.to_vec()))
    })
    .ok_or_else(|| String::from("no page mounted at that path"))?;
    let (size, rgba) = frame.ok_or_else(|| String::from("no frame exported yet"))?;
    LATER.with(|later| {
        later.borrow_mut().push(WebviewEvent::SnapshotDone {
            token,
            result: Ok((size.0 as usize, size.1 as usize, rgba)),
        });
    });
    Ok(())
}

// MARK: - what the engine reports

fn key_of(data: *mut c_void) -> Key {
    let tag = unsafe { &*(data as *const Tag) };
    (tag.window, tag.path.clone())
}

extern "C" fn on_load_changed(view: *mut c_void, event: c_int, data: *mut c_void) {
    if event != wpe::LOAD_COMMITTED {
        return;
    }
    let Some(stack) = wpe::loader() else { return };
    let key = key_of(data);
    let url = unsafe { current_uri(stack, view) }.unwrap_or_default();
    // a document's commit shuts the door its own load came through,
    // and its editor takes the keyboard — once
    let wants_keyboard = with_host(key.0, &key.1, |host| {
        host.letter.as_mut().is_some_and(|letter| {
            letter.expected = false;
            std::mem::replace(&mut letter.focus, false)
        })
    })
    .unwrap_or(false);
    if wants_keyboard {
        focus(key.0, &key.1);
    }
    dispatch(WebviewEvent::Navigated { path: key.1, url });
}

extern "C" fn on_load_failed(
    view: *mut c_void,
    _event: c_int,
    uri: *const c_char,
    error: *mut wpe::GError,
    data: *mut c_void,
) -> c_int {
    let Some(stack) = wpe::loader() else { return 1 };
    let (domain, code, message) = unsafe {
        if error.is_null() {
            (String::new(), 0, String::from("the engine answered nothing"))
        } else {
            (
                wpe::text_of((stack.glib.quark_to_string)((*error).domain)),
                (*error).code,
                wpe::text_of((*error).message),
            )
        }
    };
    // a load the policy refused, or one replaced by another, is not a
    // refusal the app hears about
    const NETWORK_CANCELLED: c_int = 302;
    const POLICY_INTERRUPTED: c_int = 102;
    let quiet = (domain == "WebKitNetworkError" && code == NETWORK_CANCELLED)
        || (domain == "WebKitPolicyError" && code == POLICY_INTERRUPTED);
    if quiet {
        return 1;
    }
    let key = key_of(data);
    let mut url = unsafe { wpe::text_of(uri) };
    if url.is_empty() {
        url = unsafe { current_uri(stack, view) }.unwrap_or_default();
    }
    let why = if message.is_empty() { String::from("the engine refused unnamed") } else { message };
    dispatch(WebviewEvent::NavigationFailed { path: key.1, url, why });
    // handled: the page on screen stays, no error page replaces it
    1
}

/// A link the page activated, reported to the app — a document
/// follows none of its own.
unsafe fn report_link(stack: &Stack, key: &Key, action: *mut c_void) {
    unsafe {
        let request = (stack.webkit.navigation_action_get_request)(action);
        if request.is_null() {
            return;
        }
        let url = wpe::text_of((stack.webkit.uri_request_get_uri)(request));
        if url.is_empty() || url.get(..11).is_some_and(|head| head.eq_ignore_ascii_case("javascript:")) {
            return;
        }
        dispatch(WebviewEvent::Linked { path: key.1.clone(), url });
    }
}

extern "C" fn on_decide_policy(_view: *mut c_void, decision: *mut c_void, kind: c_int, data: *mut c_void) -> c_int {
    let Some(stack) = wpe::loader() else { return 0 };
    let key = key_of(data);
    // a page shown by url follows its own links: the engine decides
    let expected = with_host(key.0, &key.1, |host| {
        host.letter.as_mut().map(|letter| std::mem::replace(&mut letter.expected, false))
    })
    .flatten();
    let Some(expected) = expected else { return 0 };
    unsafe {
        match kind {
            wpe::POLICY_NAVIGATION_ACTION => {
                let action = (stack.webkit.navigation_policy_decision_get_navigation_action)(decision);
                let link = !action.is_null()
                    && (stack.webkit.navigation_action_get_navigation_type)(action)
                        == wpe::NAVIGATION_LINK_CLICKED;
                if link {
                    report_link(stack, &key, action);
                    (stack.webkit.policy_decision_ignore)(decision);
                } else if expected {
                    (stack.webkit.policy_decision_use)(decision);
                } else {
                    (stack.webkit.policy_decision_ignore)(decision);
                }
                1
            }
            wpe::POLICY_NEW_WINDOW_ACTION => {
                let action = (stack.webkit.navigation_policy_decision_get_navigation_action)(decision);
                if !action.is_null() {
                    report_link(stack, &key, action);
                }
                (stack.webkit.policy_decision_ignore)(decision);
                1
            }
            _ => 0,
        }
    }
}

extern "C" fn on_create(_view: *mut c_void, action: *mut c_void, data: *mut c_void) -> *mut c_void {
    let Some(stack) = wpe::loader() else { return std::ptr::null_mut() };
    let key = key_of(data);
    let sealed = with_host(key.0, &key.1, |host| host.letter.is_some()).unwrap_or(false);
    if sealed && !action.is_null() {
        unsafe { report_link(stack, &key, action) };
    }
    std::ptr::null_mut()
}

extern "C" fn on_terminated(view: *mut c_void, reason: c_int, data: *mut c_void) {
    let Some(stack) = wpe::loader() else { return };
    let key = key_of(data);
    let url = unsafe { current_uri(stack, view) }.unwrap_or_default();
    dispatch(WebviewEvent::NavigationFailed {
        path: key.1,
        url,
        why: format!("the web process died (reason {reason})"),
    });
}

/// One line on one channel: the boot script and the eval wrapper send
/// strings only — anything else is a page poking the private channel.
fn message(channel: &str, value: *mut c_void, data: *mut c_void) {
    let Some(stack) = wpe::loader() else { return };
    let body = unsafe {
        if value.is_null() || (stack.webkit.jsc_value_is_string)(value) == 0 {
            return;
        }
        let text = (stack.webkit.jsc_value_to_string)(value);
        let body = wpe::text_of(text);
        (stack.glib.free)(text.cast());
        body
    };
    let key = key_of(data);
    let path = key.1;
    match channel {
        "bunny" => dispatch(WebviewEvent::Posted { path, body }),
        "bunnyConsole" => dispatch(WebviewEvent::Console { path, line: body }),
        "bunnyNet" => dispatch(WebviewEvent::Requested { path, line: body }),
        "bunnyEdit" => match editor_report(&body) {
            Some(EditorReport::Changed(html)) => dispatch(WebviewEvent::Changed { path, html }),
            Some(EditorReport::Pasted { html, text }) => {
                dispatch(WebviewEvent::Pasted { path, html, text });
            }
            None => {}
        },
        "bunnyEval" => {
            let mut parts = body.splitn(3, '\t');
            let (Some(token), Some(verdict), Some(payload)) = (parts.next(), parts.next(), parts.next())
            else {
                return;
            };
            let Ok(token) = token.parse::<u64>() else { return };
            let result = match verdict {
                "ok" => Ok(payload.to_string()),
                _ => Err(payload.to_string()),
            };
            dispatch(WebviewEvent::EvalDone { token, result });
        }
        _ => {}
    }
}

extern "C" fn on_message_bunny(_ucm: *mut c_void, value: *mut c_void, data: *mut c_void) {
    message("bunny", value, data);
}
extern "C" fn on_message_eval(_ucm: *mut c_void, value: *mut c_void, data: *mut c_void) {
    message("bunnyEval", value, data);
}
extern "C" fn on_message_console(_ucm: *mut c_void, value: *mut c_void, data: *mut c_void) {
    message("bunnyConsole", value, data);
}
extern "C" fn on_message_net(_ucm: *mut c_void, value: *mut c_void, data: *mut c_void) {
    message("bunnyNet", value, data);
}
extern "C" fn on_message_edit(_ucm: *mut c_void, value: *mut c_void, data: *mut c_void) {
    message("bunnyEdit", value, data);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names a page uses map to the keysyms libwpe reads; a single
    /// character is itself; anything longer is nothing.
    #[test]
    fn the_key_names_map_to_keysyms() {
        assert_eq!(keysym_named("Enter"), Some(0xff0d));
        assert_eq!(keysym_named("Escape"), Some(0xff1b));
        assert_eq!(keysym_named("ArrowDown"), Some(0xff54));
        assert_eq!(keysym_named("Space"), Some(0x20));
        assert_eq!(keysym_named("a"), Some(0x61));
        assert_eq!(keysym_named("é"), Some(0xe9));
        assert_eq!(keysym_named("→"), Some(0x0100_0000 | 0x2192));
        assert_eq!(keysym_named("Enterprise"), None);
    }

    /// The shell's modifiers become libwpe's bits: Control rides
    /// `command` on this shell, Alt rides `option`.
    #[test]
    fn the_modifiers_become_wpe_bits() {
        let all = Modifiers { shift: true, command: true, option: true, control: true };
        assert_eq!(
            wpe_modifiers(all),
            wpe::MOD_SHIFT | wpe::MOD_CONTROL | wpe::MOD_ALT | wpe::MOD_META
        );
        assert_eq!(wpe_modifiers(Modifiers::NONE), 0);
        assert_eq!(button_of(MouseButton::Right), (2, wpe::BUTTON2));
    }

    /// The stamp folds the whole spec, and a document by its digest.
    #[test]
    fn the_stamp_fingerprints_the_spec() {
        let plain = HostSpec::Webview {
            url: "https://example.test/".into(),
            document: None,
            scripts: Vec::new().into(),
            console: false,
            requests: true,
            full_motion: false,
        };
        let stamp = stamp_of(&plain);
        assert!(stamp.starts_with("https://example.test/"));
        assert!(stamp.ends_with("-r-"));
        let HostSpec::Webview { console, .. } = &plain;
        assert!(!console);
    }
}
