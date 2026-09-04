//! The webview tenant: WKWebView behind the native host.
//!
//! The OS already ships a browser engine; this module mounts it in
//! the hole the layout keeps (`docs/webview.md`). The engine draws,
//! scrolls and reads input itself — the shell creates the view,
//! points it at a url, moves the box, and holds ONE return channel:
//! the script message bridge. Everything the page sends back rides
//! it — the app's bus (`window.bunny.post`) and the eval answers —
//! so the only Objective-C block this crate AUTHORS is the snapshot's.
//!
//! A DOCUMENT (`webview_html`) rides the same view by
//! `loadHTMLString:baseURL:`, sealed under its policy; the navigation
//! delegate then answers every question the engine asks with the
//! document's one rule — the app's own load goes through, a link
//! reports to the app, and nothing else moves.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::null_mut;

use bunny_ui::action::Modifiers;
use bunny_ui::host::{Document, HostSpec, MouseButton, WebviewInput};

use crate::ffi::{CGPoint, CGRect, CGSize, Id, NS_NOT_FOUND, NSRange, Sel, class, sel};

// The wheel is the one event AppKit has no constructor for: a scroll
// NSEvent is BORN as a CGEvent and crosses over.
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreateScrollWheelEvent2(
        source: Id,
        units: u32,
        wheels: u32,
        first: i32,
        second: i32,
        third: i32,
    ) -> Id;
    fn CGEventSetLocation(event: Id, location: CGPoint);
    fn CGEventSetIntegerValueField(event: Id, field: u32, value: i64);
}

// WebKit rides along — the classes resolve by name at runtime, and
// the link is what loads them.
#[link(name = "WebKit", kind = "framework")]
unsafe extern "C" {}

// The msgSend casts in the house pattern, local to the messages this
// module sends — plus the class-builder calls the bridge needs.
#[allow(clashing_extern_declarations)]
#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    #[link_name = "objc_msgSend"]
    fn msg_id(obj: Id, sel: Sel) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_id(obj: Id, sel: Sel, a: Id) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_id_id(obj: Id, sel: Sel, a: Id, b: Id) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_cstr(obj: Id, sel: Sel, a: *const c_char) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_void(obj: Id, sel: Sel);
    #[link_name = "objc_msgSend"]
    fn msg_void_id(obj: Id, sel: Sel, a: Id);
    #[link_name = "objc_msgSend"]
    fn msg_void_id_id(obj: Id, sel: Sel, a: Id, b: Id);
    #[link_name = "objc_msgSend"]
    fn msg_void_bool(obj: Id, sel: Sel, a: i8);
    #[link_name = "objc_msgSend"]
    fn msg_bool_id(obj: Id, sel: Sel, a: Id) -> i8;
    #[link_name = "objc_msgSend"]
    fn msg_i64(obj: Id, sel: Sel) -> i64;
    #[link_name = "objc_msgSend"]
    fn msg_bool_sel(obj: Id, sel: Sel, a: Sel) -> i8;
    #[link_name = "objc_msgSend"]
    fn msg_init_config(obj: Id, sel: Sel, frame: CGRect, config: Id) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_init_script(obj: Id, sel: Sel, source: Id, time: i64, main_only: i8) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_bool(obj: Id, sel: Sel) -> i8;
    #[link_name = "objc_msgSend"]
    fn msg_id_u64(obj: Id, sel: Sel, a: u64) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_f64(obj: Id, sel: Sel) -> f64;
    #[link_name = "objc_msgSend"]
    fn msg_rect(obj: Id, sel: Sel) -> CGRect;
    #[link_name = "objc_msgSend"]
    fn msg_point_point_id(obj: Id, sel: Sel, point: CGPoint, view: Id) -> CGPoint;
    #[link_name = "objc_msgSend"]
    fn msg_void_id_range(obj: Id, sel: Sel, a: Id, range: NSRange);
    /// `+[NSEvent mouseEventWithType:location:modifierFlags:timestamp:
    /// windowNumber:context:eventNumber:clickCount:pressure:]`
    #[link_name = "objc_msgSend"]
    fn msg_mouse_event(
        obj: Id,
        sel: Sel,
        kind: u64,
        location: CGPoint,
        flags: u64,
        timestamp: f64,
        window: i64,
        context: Id,
        number: i64,
        clicks: i64,
        pressure: f32,
    ) -> Id;
    /// `+[NSEvent keyEventWithType:location:modifierFlags:timestamp:
    /// windowNumber:context:characters:charactersIgnoringModifiers:
    /// isARepeat:keyCode:]`
    #[link_name = "objc_msgSend"]
    fn msg_key_event(
        obj: Id,
        sel: Sel,
        kind: u64,
        location: CGPoint,
        flags: u64,
        timestamp: f64,
        window: i64,
        context: Id,
        characters: Id,
        bare: Id,
        repeat: i8,
        code: u16,
    ) -> Id;

    fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra: usize) -> Id;
    fn objc_registerClassPair(class: Id);
    fn class_addMethod(class: Id, sel: Sel, imp: *const c_void, types: *const c_char) -> i8;
    fn objc_getProtocol(name: *const c_char) -> Id;
    fn class_addProtocol(class: Id, protocol: Id) -> i8;
}

/// What the page sent back over the bridge — delivered to the shell's
/// dispatch, on the main thread, from WebKit's own runloop callbacks.
#[derive(Clone)]
pub(crate) enum WebviewEvent {
    /// The engine committed a navigation — link clicks included.
    Navigated { view: Id, url: String },
    /// A link in a DOCUMENT was activated. The engine did not follow
    /// it: the document stays, and the app hears the url.
    Linked { view: Id, url: String },
    /// The engine REFUSED one: the url it tried, and why — the other
    /// leg of the same pair, so no load ends in silence.
    NavigationFailed { view: Id, url: String, why: String },
    /// The page called `window.bunny.post(…)`.
    Posted { view: Id, body: String },
    /// The page's console spoke — `"level: what it said"`.
    Console { view: Id, line: String },
    /// A request of the page's completed — `"METHOD url status"`.
    Requested { view: Id, line: String },
    /// An eval answered, by token — `Ok` is JSON, `Err` the thrown
    /// error's name.
    EvalDone { token: u64, result: Result<String, String> },
    /// A snapshot answered, by token — straight RGBA, tightly packed.
    SnapshotDone { token: u64, result: Result<(usize, usize, Vec<u8>), String> },
}

thread_local! {
    /// Where bridge callbacks land — the shell installs it at window
    /// start. Taken out while it runs, the way the app handler is.
    static DISPATCH: RefCell<Option<Box<dyn Fn(WebviewEvent)>>> = const { RefCell::new(None) };
    /// The ONE bridge instance — message handler and navigation
    /// delegate for every webview in the window (the events carry the
    /// view, so one listener serves all).
    static BRIDGE: Cell<Id> = const { Cell::new(null_mut()) };
    /// The DOCUMENTS mounted, by host path — what the policy delegate
    /// reads when the engine asks whether it may move. A page shown by
    /// url has no entry, and follows its own links.
    static LETTERS: RefCell<HashMap<String, Letter>> = RefCell::new(HashMap::new());
}

/// A mounted document's standing.
struct Letter {
    /// The fingerprint of what is loaded — `update` compares, so the
    /// same letter never reloads and a changed one always does.
    digest: u64,
    /// The app's own load is in flight: the ONE navigation the
    /// delegate lets through. Cleared when the delegate saw it, and
    /// again at the commit — whichever the engine says first — so a
    /// refresh the document asks for later finds the door shut.
    expected: bool,
}

/// `WKNavigationActionPolicy` — what the delegate answers with.
const POLICY_CANCEL: i64 = 0;
const POLICY_ALLOW: i64 = 1;
/// `WKNavigationTypeLinkActivated` — a link the person activated.
const NAVIGATION_LINK: i64 = 0;

/// The shell installs the landing spot for everything a page reports.
pub(crate) fn set_dispatch(dispatch: impl Fn(WebviewEvent) + 'static) {
    DISPATCH.with(|slot| *slot.borrow_mut() = Some(Box::new(dispatch)));
}

fn dispatch(event: WebviewEvent) {
    // taken out while it runs — a callback that re-enters finds the
    // slot empty instead of a borrow panic
    let Some(handler) = DISPATCH.with(|slot| slot.borrow_mut().take()) else {
        return;
    };
    handler(event);
    DISPATCH.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(handler);
        }
    });
}

/// The page's side of the bus, injected at document start on every
/// navigation, before anything the page runs.
const BOOT: &str = "window.bunny = { post: function(m) { \
    window.webkit.messageHandlers.bunny.postMessage(String(m)); } };";

/// The console hook — `WebviewCapability::ConsoleMessages` on this
/// backend is an injected wrap: each level forwards a line and then
/// speaks as before, and an uncaught error reports too. Injected only
/// when the app declared `on_console`: nothing is captured for a page
/// nobody watches.
const CONSOLE_HOOK: &str = "(function() { \
    function forward(line) { try { \
        window.webkit.messageHandlers.bunnyConsole.postMessage(line); } catch (e) {} } \
    var levels = ['log', 'info', 'warn', 'error']; \
    for (var i = 0; i < levels.length; i++) { (function(level) { \
        var original = console[level]; \
        console[level] = function() { \
            var parts = []; \
            for (var j = 0; j < arguments.length; j++) { var a = arguments[j]; \
                try { parts.push(typeof a === 'string' ? a : JSON.stringify(a)); } \
                catch (e) { parts.push(String(a)); } } \
            forward(level + ': ' + parts.join(' ')); \
            if (original) { original.apply(console, arguments); } \
        }; })(levels[i]); } \
    addEventListener('error', function(e) { forward('error: ' + e.message); }); \
})();";

/// The network wrap — `WebviewCapability::NetworkRequests` on this
/// backend: fetch and XHR report on completion, as
/// `METHOD url status`. BLIND to subresources by construction — an
/// image or a stylesheet never crosses fetch. Injected only when the
/// app declared `on_request`.
const NET_WRAP: &str = "(function() { \
    function forward(line) { try { \
        window.webkit.messageHandlers.bunnyNet.postMessage(line); } catch (e) {} } \
    var original = window.fetch; \
    if (original) { window.fetch = function(input, init) { \
        var method = (init && init.method) || (input && input.method) || 'GET'; \
        var url = (typeof input === 'string') ? input : ((input && input.url) || String(input)); \
        var pending = original.apply(this, arguments); \
        pending.then(function(response) { forward(method + ' ' + url + ' ' + response.status); }, \
                     function() { forward(method + ' ' + url + ' failed'); }); \
        return pending; }; } \
    var open = XMLHttpRequest.prototype.open; \
    XMLHttpRequest.prototype.open = function(method, url) { \
        this.__bunny = method + ' ' + url; return open.apply(this, arguments); }; \
    var send = XMLHttpRequest.prototype.send; \
    XMLHttpRequest.prototype.send = function() { var xhr = this; \
        xhr.addEventListener('loadend', function() { \
            forward((xhr.__bunny || '? ?') + ' ' + (xhr.status || 'failed')); }); \
        return send.apply(this, arguments); }; \
})();";

/// `userContentController:didReceiveScriptMessage:` — the one return
/// channel. `bunny` is the app's bus; `bunnyEval` carries eval
/// answers in a `token \t ok|err \t payload` envelope (stringify
/// escapes control characters, so the payload never contains a tab).
extern "C" fn bridge_message(_this: Id, _sel: Sel, _controller: Id, message: Id) {
    unsafe {
        let body = msg_id(message, sel("body"));
        if body.is_null() || msg_bool_id(body, sel("isKindOfClass:"), class("NSString")) == 0 {
            // the boot script and the eval wrapper send strings only —
            // anything else is a page poking the private channel
            return;
        }
        let body = to_string(body);
        let name = to_string(msg_id(message, sel("name")));
        match name.as_str() {
            "bunny" => {
                let view = msg_id(message, sel("webView"));
                dispatch(WebviewEvent::Posted { view, body });
            }
            "bunnyConsole" => {
                let view = msg_id(message, sel("webView"));
                dispatch(WebviewEvent::Console { view, line: body });
            }
            "bunnyNet" => {
                let view = msg_id(message, sel("webView"));
                dispatch(WebviewEvent::Requested { view, line: body });
            }
            "bunnyEval" => {
                let mut parts = body.splitn(3, '\t');
                let (Some(token), Some(verdict), Some(payload)) =
                    (parts.next(), parts.next(), parts.next())
                else {
                    return;
                };
                let Ok(token) = token.parse::<u64>() else {
                    return;
                };
                let result = match verdict {
                    "ok" => Ok(payload.to_string()),
                    _ => Err(payload.to_string()),
                };
                dispatch(WebviewEvent::EvalDone { token, result });
            }
            _ => {}
        }
    }
}

/// `webView:didCommitNavigation:` — the url is real from here on. A
/// document's commit also shuts the door its own load came through:
/// from here on nothing the document asks for moves it.
extern "C" fn bridge_committed(_this: Id, _sel: Sel, view: Id, _navigation: Id) {
    if let Some(path) = crate::ffi::host_key_of_child(view) {
        LETTERS.with(|letters| {
            if let Some(letter) = letters.borrow_mut().get_mut(&path) {
                letter.expected = false;
            }
        });
    }
    unsafe {
        let url = msg_id(view, sel("URL"));
        if url.is_null() {
            return;
        }
        let url = to_string(msg_id(url, sel("absoluteString")));
        dispatch(WebviewEvent::Navigated { view, url });
    }
}

/// The block the engine hands a policy delegate: called once, with
/// the answer. This crate never AUTHORS one of these — it only reads
/// the runtime's layout far enough to find `invoke` and call it.
#[repr(C)]
struct PolicyBlock {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: unsafe extern "C" fn(*mut PolicyBlock, i64),
}

/// `webView:decidePolicyForNavigationAction:decisionHandler:` — the
/// engine asks before it moves. A page shown by url is answered yes,
/// always: it follows its own links, as it did before this method
/// existed. A DOCUMENT is answered by its one rule: the app's own
/// load goes through, a link the person activated is CANCELLED and
/// reported to the app (the document never follows it), and every
/// other ask — a refresh the document wrote, a form, a subframe, the
/// engine's own reload (which would fetch the base url) — is
/// cancelled without a word. The handler is called exactly once, on
/// every road out: the engine throws when it is not.
extern "C" fn bridge_decide(_this: Id, _sel: Sel, view: Id, action: Id, handler: Id) {
    let policy = unsafe { decide(view, action) };
    unsafe {
        let block = handler as *mut PolicyBlock;
        if !block.is_null() {
            ((*block).invoke)(block, policy);
        }
    }
}

unsafe fn decide(view: Id, action: Id) -> i64 {
    let Some(path) = crate::ffi::host_key_of_child(view) else {
        return POLICY_ALLOW;
    };
    let expected = LETTERS.with(|letters| {
        letters
            .borrow_mut()
            .get_mut(&path)
            .map(|letter| std::mem::replace(&mut letter.expected, false))
    });
    let Some(expected) = expected else {
        // a page by url: its own business
        return POLICY_ALLOW;
    };
    unsafe {
        if msg_i64(action, sel("navigationType")) == NAVIGATION_LINK {
            report_link(view, action);
            return POLICY_CANCEL;
        }
    }
    if expected { POLICY_ALLOW } else { POLICY_CANCEL }
}

/// `webView:createWebViewWithConfiguration:forNavigationAction:
/// windowFeatures:` — a link that asks for a NEW window
/// (`target="_blank"`). No view is ever created here: a document's
/// link reports to the app like any other, and a page by url gets
/// what it always got from a window nobody opens — nothing.
extern "C" fn bridge_create_view(
    _this: Id,
    _sel: Sel,
    view: Id,
    _configuration: Id,
    action: Id,
    _features: Id,
) -> Id {
    let sealed = crate::ffi::host_key_of_child(view)
        .is_some_and(|path| LETTERS.with(|letters| letters.borrow().contains_key(&path)));
    if sealed {
        unsafe { report_link(view, action) };
    }
    null_mut()
}

/// The url a navigation action aims at, to the app — unless it is a
/// `javascript:` link, which is not a place and runs nowhere.
unsafe fn report_link(view: Id, action: Id) {
    unsafe {
        let request = msg_id(action, sel("request"));
        let url = if request.is_null() { null_mut() } else { msg_id(request, sel("URL")) };
        if url.is_null() {
            return;
        }
        let scheme = to_string(msg_id(url, sel("scheme")));
        if scheme.eq_ignore_ascii_case("javascript") {
            return;
        }
        let url = to_string(msg_id(url, sel("absoluteString")));
        if !url.is_empty() {
            dispatch(WebviewEvent::Linked { view, url });
        }
    }
}

/// `webView:didFailProvisionalNavigation:withError:` and
/// `webView:didFailNavigation:withError:` — the two ways a load ends
/// WITHOUT a commit: the first before any byte of the new page
/// arrived (a dead host, a bad certificate), the second after the
/// document started. The app hears one sentence for both, because the
/// app's question is the same one: it is not going to arrive.
///
/// A CANCELLED load is not a failure and never reports: it is what
/// the engine says when a newer navigation took the tab, and that one
/// answers for both. Reporting it would tell an app its live load
/// died at the moment it actually started.
extern "C" fn bridge_failed(_this: Id, _sel: Sel, view: Id, _navigation: Id, error: Id) {
    unsafe {
        if cancelled(error) {
            return;
        }
        let url = failing_url(view, error);
        dispatch(WebviewEvent::NavigationFailed { view, url, why: error_name(error) });
    }
}

/// Did the engine stop this load because another one replaced it?
/// `NSURLErrorCancelled`, in the domain's own numbering.
unsafe fn cancelled(error: Id) -> bool {
    const NS_URL_ERROR_CANCELLED: i64 = -999;
    if error.is_null() {
        return false;
    }
    unsafe {
        msg_i64(error, sel("code")) == NS_URL_ERROR_CANCELLED
            && to_string(msg_id(error, sel("domain"))) == "NSURLErrorDomain"
    }
}

/// The url a refused load was AIMING at. The view's own url is the
/// page still on screen — the one that did not go anywhere — so the
/// error's own record comes first: the loader files the target under
/// two keys, as a string and as an NSURL.
unsafe fn failing_url(view: Id, error: Id) -> String {
    unsafe {
        let info = if error.is_null() { null_mut() } else { msg_id(error, sel("userInfo")) };
        if !info.is_null() {
            let text = msg_id_id(info, sel("objectForKey:"), ns("NSErrorFailingURLStringKey"));
            if !text.is_null()
                && msg_bool_id(text, sel("isKindOfClass:"), class("NSString")) != 0
            {
                return to_string(text);
            }
            let url = msg_id_id(info, sel("objectForKey:"), ns("NSErrorFailingURLKey"));
            if !url.is_null() {
                let text = to_string(msg_id(url, sel("absoluteString")));
                if !text.is_empty() {
                    return text;
                }
            }
        }
        current_url(view).unwrap_or_default()
    }
}

/// The one bridge instance, built on first use: an NSObject that
/// answers the script messages and the navigation delegate calls.
fn bridge() -> Id {
    BRIDGE.with(|slot| {
        let existing = slot.get();
        if !existing.is_null() {
            return existing;
        }
        let instance = unsafe {
            let name = CString::new("BunnyWebBridge").expect("class name");
            let bridge = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
            let message_types = CString::new("v@:@@").expect("type encoding");
            class_addMethod(
                bridge,
                sel("userContentController:didReceiveScriptMessage:"),
                bridge_message as *const c_void,
                message_types.as_ptr(),
            );
            class_addMethod(
                bridge,
                sel("webView:didCommitNavigation:"),
                bridge_committed as *const c_void,
                message_types.as_ptr(),
            );
            let failure_types = CString::new("v@:@@@").expect("type encoding");
            for leg in [
                "webView:didFailProvisionalNavigation:withError:",
                "webView:didFailNavigation:withError:",
            ] {
                class_addMethod(
                    bridge,
                    sel(leg),
                    bridge_failed as *const c_void,
                    failure_types.as_ptr(),
                );
            }
            // the policy ask: three objects, the last a block (`@?`)
            let decide_types = CString::new("v@:@@@?").expect("type encoding");
            class_addMethod(
                bridge,
                sel("webView:decidePolicyForNavigationAction:decisionHandler:"),
                bridge_decide as *const c_void,
                decide_types.as_ptr(),
            );
            // the new-window ask answers with a view — or nil
            let create_types = CString::new("@@:@@@@").expect("type encoding");
            class_addMethod(
                bridge,
                sel("webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:"),
                bridge_create_view as *const c_void,
                create_types.as_ptr(),
            );
            for protocol in ["WKScriptMessageHandler", "WKNavigationDelegate", "WKUIDelegate"] {
                let protocol = CString::new(protocol).expect("protocol name");
                let protocol = objc_getProtocol(protocol.as_ptr());
                if !protocol.is_null() {
                    class_addProtocol(bridge, protocol);
                }
            }
            objc_registerClassPair(bridge);
            msg_id(msg_id(bridge, sel("alloc")), sel("init"))
        };
        slot.set(instance);
        instance
    })
}

/// Creates the engine's view, already instrumented and navigating to
/// the spec's url — or loading its document. The reference comes back
/// with ONE retain — the host's sweep releases it when the box leaves
/// the scene. `path` is the host's identity, what a document is filed
/// under for the delegate to find.
pub(crate) fn create(path: &str, spec: &HostSpec) -> Id {
    let HostSpec::Webview { url, document, .. } = spec;
    unsafe {
        let config =
            msg_id(msg_id(class("WKWebViewConfiguration"), sel("alloc")), sel("init"));
        install_bridge(msg_id(config, sel("userContentController")), spec);
        let view = msg_id(class("WKWebView"), sel("alloc"));
        let view = msg_init_config(
            view,
            sel("initWithFrame:configuration:"),
            CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize { width: 0.0, height: 0.0 },
            },
            config,
        );
        // the view copied what it needed from the configuration
        msg_void(config, sel("release"));
        // navigation reports come through the bridge (the delegate
        // reference is weak; the bridge outlives every view), and so
        // does the ask a new-window link makes
        msg_void_id(view, sel("setNavigationDelegate:"), bridge());
        msg_void_id(view, sel("setUIDelegate:"), bridge());
        // the engine's own inspector, where the OS offers the switch
        // (13.3+) — a webview here is a dev's window into a page, and
        // a devtool that cannot open is a quiet page with no name
        if msg_bool_sel(view, sel("respondsToSelector:"), sel("setInspectable:")) != 0 {
            msg_void_bool(view, sel("setInspectable:"), 1);
        }
        match document {
            Some(document) => load_document(path, view, document),
            None => navigate(view, url),
        }
        view
    }
}

/// Loads a document from MEMORY — `loadHTMLString:baseURL:`, the
/// sealed html the spec holds, the base the engine resolves relative
/// references by (nil for none). Filed first, loaded second: the
/// delegate is asked about this load, and must find the letter
/// expecting it.
fn load_document(path: &str, view: Id, document: &Document) {
    LETTERS.with(|letters| {
        letters.borrow_mut().insert(
            path.to_string(),
            Letter { digest: document.digest, expected: true },
        );
    });
    unsafe {
        let base = if document.base.is_empty() { null_mut() } else { ns_url(&document.base) };
        let _ = msg_id_id_id(view, sel("loadHTMLString:baseURL:"), ns(&document.html), base);
    }
}

/// Forgets the documents whose hosts left the scene — called beside
/// the host sweep, with the paths still standing.
pub(crate) fn sweep(alive: &[String]) {
    LETTERS.with(|letters| letters.borrow_mut().retain(|path, _| alive.contains(path)));
}

/// Hands the controller the bridge and the document-start scripts.
/// Every channel registers up front (a registered name that never
/// speaks costs nothing, and re-adding one throws); the scripts are
/// [`apply_scripts`]'s.
unsafe fn install_bridge(controller: Id, spec: &bunny_ui::host::HostSpec) {
    unsafe {
        let bridge = bridge();
        for channel in ["bunny", "bunnyConsole", "bunnyNet", "bunnyEval"] {
            msg_void_id_id(
                controller,
                sel("addScriptMessageHandler:name:"),
                bridge,
                ns(channel),
            );
        }
        apply_scripts(controller, spec);
    }
}

/// The document-start set, in a fixed order: the bus first (a user
/// script may want to post), then the hooks the app DECLARED — a page
/// nobody watches pays for no capture — then the app's own scripts,
/// in declaration order.
unsafe fn apply_scripts(controller: Id, spec: &bunny_ui::host::HostSpec) {
    let bunny_ui::host::HostSpec::Webview { scripts, console, requests, .. } = spec;
    unsafe {
        add_script(controller, BOOT);
        if *console {
            add_script(controller, CONSOLE_HOOK);
        }
        if *requests {
            add_script(controller, NET_WRAP);
        }
        for script in scripts.iter() {
            add_script(controller, script);
        }
    }
}

/// One WKUserScript at document start, main frame only.
unsafe fn add_script(controller: Id, source: &str) {
    unsafe {
        let script = msg_id(class("WKUserScript"), sel("alloc"));
        let script = msg_init_script(
            script,
            sel("initWithSource:injectionTime:forMainFrameOnly:"),
            ns(source),
            0, // WKUserScriptInjectionTimeAtDocumentStart
            1,
        );
        msg_void_id(controller, sel("addUserScript:"), script);
        // the controller holds it now
        msg_void(script, sel("release"));
    }
}

/// Re-instructs a MOUNTED view after its spec changed: the scripts
/// are replaced (they take effect on the next navigation) and the
/// page re-points — AFTER comparing with where the engine already
/// is. The comparison is the whole point: an app that folds the
/// committed url back into its spec must not reload the page the
/// engine just arrived at, and with the spec carrying the real url a
/// remount boards at the real page. The imperative
/// `WebviewHandle::navigate` never compares — asking again for the
/// page you are on is a reload, like the browser button it is.
///
/// A document compares by its fingerprint: the same letter under a
/// re-run body never reloads, a changed one always does. A view that
/// goes from a document back to a url closes the letter — the page
/// follows its own links again.
pub(crate) fn update(path: &str, view: Id, spec: &HostSpec) {
    let HostSpec::Webview { url, document, .. } = spec;
    unsafe {
        let config = msg_id(view, sel("configuration"));
        let controller = msg_id(config, sel("userContentController"));
        msg_void(controller, sel("removeAllUserScripts"));
        apply_scripts(controller, spec);
        match document {
            Some(document) => {
                let loaded = LETTERS
                    .with(|letters| letters.borrow().get(path).map(|letter| letter.digest));
                if loaded != Some(document.digest) {
                    load_document(path, view, document);
                }
            }
            None => {
                LETTERS.with(|letters| letters.borrow_mut().remove(path));
                if current_url(view).as_deref() != Some(&**url) {
                    navigate(view, url);
                }
            }
        }
    }
}

/// Where the engine is right now — the committed url, the same string
/// [`WebviewEvent::Navigated`] reported (so an app folding that
/// report into its spec compares equal, redirects included).
unsafe fn current_url(view: Id) -> Option<String> {
    unsafe {
        let url = msg_id(view, sel("URL"));
        if url.is_null() {
            return None;
        }
        Some(to_string(msg_id(url, sel("absoluteString"))))
    }
}

/// What this backend serves, as the value the app reads
/// (`docs/webview.md` — the capability table's WKWebView column):
/// console and requests by injected hook, synthetic input by real
/// NSEvent, and no response bodies — the one cell an injected wrap
/// cannot reach.
pub fn capabilities() -> &'static [bunny_ui::host::WebviewCapability] {
    use bunny_ui::host::WebviewCapability;
    &[
        WebviewCapability::ConsoleMessages,
        WebviewCapability::NetworkRequests,
        WebviewCapability::SyntheticInput,
    ]
}

/// Points the engine at `url` — the load is the engine's own affair,
/// asynchronous and cancellable by the next call.
pub(crate) fn navigate(view: Id, url: &str) {
    unsafe {
        let url = ns_url(url);
        if url.is_null() {
            // NSURL said no — an unparseable url loads nothing rather
            // than crashing the request builder
            return;
        }
        let request = msg_id_id(class("NSURLRequest"), sel("requestWithURL:"), url);
        let _ = msg_id_id(view, sel("loadRequest:"), request);
    }
}

/// An NSURL for `text` (autoreleased), or nil when it is not one — a
/// NUL inside is not a url, and NSURL has its own refusals.
unsafe fn ns_url(text: &str) -> Id {
    let Ok(text) = CString::new(text) else {
        return null_mut();
    };
    unsafe {
        let string = msg_id_cstr(class("NSString"), sel("stringWithUTF8String:"), text.as_ptr());
        msg_id_id(class("NSURL"), sel("URLWithString:"), string)
    }
}

/// One step back in the engine's own history — a no-op at the edge,
/// like the browser button it is.
pub(crate) fn back(view: Id) {
    unsafe {
        let _ = msg_id(view, sel("goBack"));
    }
}

/// And one forward.
pub(crate) fn forward(view: Id) {
    unsafe {
        let _ = msg_id(view, sel("goForward"));
    }
}

/// Evaluates `js` as an EXPRESSION in the page. The answer rides the
/// bridge (`bunnyEval`, by token) — the completion handler stays nil,
/// so no block ever crosses this border.
pub(crate) fn eval(view: Id, token: u64, js: &str) {
    let wrapped = format!(
        "(function() {{ try {{ \
           var __v = (function() {{ return ( {js} ); }})(); \
           var __s = JSON.stringify(__v); \
           window.webkit.messageHandlers.bunnyEval.postMessage(\
             \"{token}\\tok\\t\" + (__s === undefined ? \"null\" : __s)); \
         }} catch (e) {{ \
           window.webkit.messageHandlers.bunnyEval.postMessage(\
             \"{token}\\terr\\t\" + String(e)); \
         }} }})();"
    );
    unsafe {
        msg_void_id_id(
            view,
            sel("evaluateJavaScript:completionHandler:"),
            ns(&wrapped),
            null_mut(),
        );
    }
}

// The event types AppKit numbers, and the modifier bits it reads.
// Named here because a bare 25 in a call is nobody's idea of a middle
// button.
const LEFT_DOWN: u64 = 1;
const LEFT_UP: u64 = 2;
const RIGHT_DOWN: u64 = 3;
const RIGHT_UP: u64 = 4;
const MOUSE_MOVED: u64 = 5;
const LEFT_DRAGGED: u64 = 6;
const RIGHT_DRAGGED: u64 = 7;
const KEY_DOWN: u64 = 10;
const KEY_UP: u64 = 11;
const OTHER_DOWN: u64 = 25;
const OTHER_UP: u64 = 26;
const OTHER_DRAGGED: u64 = 27;
const FLAG_SHIFT: u64 = 1 << 17;
const FLAG_CONTROL: u64 = 1 << 18;
const FLAG_OPTION: u64 = 1 << 19;
const FLAG_COMMAND: u64 = 1 << 20;

/// One synthetic event into the page — the capability the table calls
/// `SyntheticInput`, served here by REAL NSEvents addressed at the
/// view.
///
/// This is why the mac can serve that cell at all: an event built by
/// `+[NSEvent mouseEventWithType:…]` and handed to the view is the
/// same object a hand produces, so the page reads `isTrusted` as true
/// and a button that guards on it works. The alternative is a
/// synthetic DOM event — `el.click()` — and real sites refuse those.
///
/// Fire-and-forget, the way nothing comes back from a hand. A page
/// with no window takes nothing: there are no coordinates to aim by.
pub(crate) fn input(view: Id, event: &WebviewInput) {
    unsafe {
        let window = msg_id(view, sel("window"));
        if window.is_null() {
            return;
        }
        crate::ffi::lend_hand(|| match event {
            WebviewInput::Click { x, y, clicks, button } => {
                let at = window_point(view, *x, *y);
                let (down, up) = press_kinds(*button);
                // a double click is two PAIRS, counted 1 then 2 — the
                // page reads the count off the second press, so one
                // press carrying a 2 is a lie a hand never tells
                for count in 1..=(*clicks).clamp(1, 3) {
                    send_mouse(view, window, down, at, Modifiers::NONE, count, 1.0);
                    send_mouse(view, window, up, at, Modifiers::NONE, count, 0.0);
                }
            }
            WebviewInput::Hover { x, y } => {
                let at = window_point(view, *x, *y);
                send_mouse(view, window, MOUSE_MOVED, at, Modifiers::NONE, 0, 0.0);
            }
            WebviewInput::Down { x, y, button, clicks, modifiers } => {
                let at = window_point(view, *x, *y);
                let (down, _) = press_kinds(*button);
                send_mouse(view, window, down, at, *modifiers, (*clicks).clamp(1, 3), 1.0);
            }
            WebviewInput::Up { x, y, button, clicks, modifiers } => {
                let at = window_point(view, *x, *y);
                let (_, up) = press_kinds(*button);
                send_mouse(view, window, up, at, *modifiers, (*clicks).clamp(1, 3), 0.0);
            }
            WebviewInput::Drag { x, y, button, modifiers } => {
                let at = window_point(view, *x, *y);
                let dragged = match button {
                    MouseButton::Left => LEFT_DRAGGED,
                    MouseButton::Right => RIGHT_DRAGGED,
                    MouseButton::Middle => OTHER_DRAGGED,
                };
                send_mouse(view, window, dragged, at, *modifiers, 1, 1.0);
            }
            WebviewInput::Scroll { x, y, dx, dy } => {
                send_wheel(view, window_point(view, *x, *y), *dx, *dy);
            }
            WebviewInput::Type { text } => send_text(view, window, text),
            WebviewInput::Key { key } => send_key(view, window, key),
        })
    }
}

/// The press and release types of one button.
const fn press_kinds(button: MouseButton) -> (u64, u64) {
    match button {
        MouseButton::Left => (LEFT_DOWN, LEFT_UP),
        MouseButton::Right => (RIGHT_DOWN, RIGHT_UP),
        // WebKit reads the middle button off the TYPE alone, so the
        // "other" pair is the middle one here
        MouseButton::Middle => (OTHER_DOWN, OTHER_UP),
    }
}

/// CSS pixels from the view's top-left corner, to the window
/// coordinates an NSEvent carries. The flip happens once, here: CSS
/// counts down from the top and AppKit counts up from the bottom,
/// unless the view says it is flipped (WKWebView does).
unsafe fn window_point(view: Id, x: f64, y: f64) -> CGPoint {
    unsafe {
        let bounds = msg_rect(view, sel("bounds"));
        let local = if msg_bool(view, sel("isFlipped")) != 0 {
            CGPoint { x: bounds.origin.x + x, y: bounds.origin.y + y }
        } else {
            CGPoint { x: bounds.origin.x + x, y: bounds.origin.y + bounds.size.height - y }
        };
        msg_point_point_id(view, sel("convertPoint:toView:"), local, null_mut())
    }
}

/// What the keyboard was holding, in AppKit's own bits.
const fn flags(modifiers: Modifiers) -> u64 {
    let mut bits = 0;
    if modifiers.shift {
        bits |= FLAG_SHIFT;
    }
    if modifiers.control {
        bits |= FLAG_CONTROL;
    }
    if modifiers.option {
        bits |= FLAG_OPTION;
    }
    if modifiers.command {
        bits |= FLAG_COMMAND;
    }
    bits
}

/// The clock an NSEvent is stamped with: seconds since the machine
/// woke, which is what AppKit puts there.
unsafe fn now() -> f64 {
    unsafe {
        let info = msg_id(class("NSProcessInfo"), sel("processInfo"));
        msg_f64(info, sel("systemUptime"))
    }
}

/// Builds one mouse event and hands it STRAIGHT to the view — not to
/// the window, and not to the queue the user's own hand shares. The
/// view is the address: the page takes what the app aimed at it, and
/// the pointer on the desk never moves.
unsafe fn send_mouse(
    view: Id,
    window: Id,
    kind: u64,
    at: CGPoint,
    modifiers: Modifiers,
    clicks: u32,
    pressure: f32,
) {
    unsafe {
        let event = msg_mouse_event(
            class("NSEvent"),
            sel(
                "mouseEventWithType:location:modifierFlags:timestamp:windowNumber:\
                 context:eventNumber:clickCount:pressure:",
            ),
            kind,
            at,
            flags(modifiers),
            now(),
            msg_i64(window, sel("windowNumber")),
            null_mut(),
            0,
            clicks as i64,
            pressure,
        );
        if !event.is_null() {
            let target = if kind == MOUSE_MOVED { move_door(view) } else { view };
            msg_void_id(target, sel(mouse_door(kind)), event);
        }
    }
}

/// Where a MOVE is heard. The engine watches the pointer through a
/// TRACKING AREA, and the area's owner — not the view — is what
/// answers `mouseMoved:`. Sent to the view, the message finds no
/// taker there and walks up the responder chain instead, which is the
/// app's own scene rather than the page.
unsafe fn move_door(view: Id) -> Id {
    unsafe {
        let areas = msg_id(view, sel("trackingAreas"));
        if areas.is_null() {
            return view;
        }
        for index in 0..msg_i64(areas, sel("count")).max(0) as u64 {
            let owner = msg_id(msg_id_u64(areas, sel("objectAtIndex:"), index), sel("owner"));
            if owner.is_null() || std::ptr::eq(owner, view) {
                continue;
            }
            if msg_bool_sel(owner, sel("respondsToSelector:"), sel("mouseMoved:")) != 0 {
                return owner;
            }
        }
        view
    }
}

/// The message a mouse event of this type is delivered by.
const fn mouse_door(kind: u64) -> &'static str {
    match kind {
        LEFT_DOWN => "mouseDown:",
        LEFT_UP => "mouseUp:",
        RIGHT_DOWN => "rightMouseDown:",
        RIGHT_UP => "rightMouseUp:",
        LEFT_DRAGGED => "mouseDragged:",
        RIGHT_DRAGGED => "rightMouseDragged:",
        OTHER_DOWN => "otherMouseDown:",
        OTHER_UP => "otherMouseUp:",
        OTHER_DRAGGED => "otherMouseDragged:",
        _ => "mouseMoved:",
    }
}

/// The wheel, at a point. AppKit's event constructor refuses this one
/// type, so the event is born in CoreGraphics and crosses over — and
/// an event that crossed over carries no window, which makes AppKit
/// read its location as SCREEN coordinates. The location is therefore
/// written as the window point flipped about the first screen, so the
/// page is asked about the point the app named.
///
/// The deltas arrive in the page's signs (`dy` counts down) and the
/// wheel's are the opposite: a wheel that turns away from the hand
/// moves the content up.
///
/// It goes as a GESTURE, in two beats: one that begins and carries
/// the whole delta, and one that ends and carries nothing. A precise
/// scroll is a gesture, and one that never ends leaves the engine
/// holding it open — with the end, the engine closes it and moves the
/// pointer over what is now under it, exactly as it does for a hand
/// on a trackpad (the page reports that move; it is the engine's own,
/// not one the app sent).
unsafe fn send_wheel(view: Id, at: CGPoint, dx: f64, dy: f64) {
    const PIXEL_UNITS: u32 = 0;
    const SCROLL_PHASE: u32 = 99;
    const PHASE_BEGAN: i64 = 1;
    const PHASE_ENDED: i64 = 4;
    unsafe {
        for (phase, x, y) in [(PHASE_BEGAN, -dx as i32, -dy as i32), (PHASE_ENDED, 0, 0)] {
            let event = CGEventCreateScrollWheelEvent2(null_mut(), PIXEL_UNITS, 2, y, x, 0);
            if event.is_null() {
                return;
            }
            CGEventSetIntegerValueField(event, SCROLL_PHASE, phase);
            if let Some(height) = main_screen_height() {
                CGEventSetLocation(event, CGPoint { x: at.x, y: height - at.y });
            }
            let carried = msg_id_id(class("NSEvent"), sel("eventWithCGEvent:"), event);
            if !carried.is_null() {
                msg_void_id(view, sel("scrollWheel:"), carried);
            }
            crate::ffi::CFRelease(event as *const c_void);
        }
    }
}

/// The height AppKit flips a screen-coordinate event about — the
/// first screen, which is the one the menu bar lives on.
unsafe fn main_screen_height() -> Option<f64> {
    unsafe {
        let screens = msg_id(class("NSScreen"), sel("screens"));
        if screens.is_null() {
            return None;
        }
        let screen = msg_id(screens, sel("firstObject"));
        if screen.is_null() {
            return None;
        }
        Some(msg_rect(screen, sel("frame")).size.height)
    }
}

/// Types into the page's own focus. This is a COMMIT, not a run of
/// keystrokes: `insertText:replacementRange:` is the door an input
/// method and a paste both land through, so the text arrives in a
/// field, in a `contenteditable`, and under a keyboard layout nobody
/// guessed. Where the engine does not answer that message, each
/// character goes as its own keystroke instead.
unsafe fn send_text(view: Id, window: Id, text: &str) {
    unsafe {
        let door = sel("insertText:replacementRange:");
        if msg_bool_sel(view, sel("respondsToSelector:"), door) != 0 {
            msg_void_id_range(view, door, ns(text), NSRange { location: NS_NOT_FOUND, length: 0 });
            return;
        }
        for character in text.chars() {
            stroke(view, window, 0, &character.to_string());
        }
    }
}

/// One named key, by the name the page uses. The mac numbers its keys
/// by POSITION on the board, and spells the ones with no character of
/// their own inside a private area of Unicode — the engine reads both
/// to name the key back to the page.
unsafe fn send_key(view: Id, window: Id, key: &str) {
    let (code, characters) = match key {
        "Enter" | "Return" => (36, "\r"),
        "Tab" => (48, "\t"),
        "Escape" | "Esc" => (53, "\u{1b}"),
        "Backspace" => (51, "\u{7f}"),
        "Delete" => (117, "\u{f728}"),
        "ArrowUp" => (126, "\u{f700}"),
        "ArrowDown" => (125, "\u{f701}"),
        "ArrowLeft" => (123, "\u{f702}"),
        "ArrowRight" => (124, "\u{f703}"),
        "Home" => (115, "\u{f729}"),
        "End" => (119, "\u{f72b}"),
        "PageUp" => (116, "\u{f72c}"),
        "PageDown" => (121, "\u{f72d}"),
        "Space" => (49, " "),
        // a single character IS its own key name, the way the page
        // spells it; a name nobody knows presses nothing
        other if other.chars().count() == 1 => (0, other),
        _ => return,
    };
    unsafe { stroke(view, window, code, characters) }
}

/// One press and release of a key, delivered to the view.
unsafe fn stroke(view: Id, window: Id, code: u16, characters: &str) {
    unsafe {
        let door = sel(
            "keyEventWithType:location:modifierFlags:timestamp:windowNumber:\
             context:characters:charactersIgnoringModifiers:isARepeat:keyCode:",
        );
        let number = msg_i64(window, sel("windowNumber"));
        let origin = CGPoint { x: 0.0, y: 0.0 };
        for (kind, deliver) in [(KEY_DOWN, "keyDown:"), (KEY_UP, "keyUp:")] {
            let text = ns(characters);
            let event = msg_key_event(
                class("NSEvent"),
                door,
                kind,
                origin,
                0,
                now(),
                number,
                null_mut(),
                text,
                text,
                0,
                code,
            );
            if !event.is_null() {
                msg_void_id(view, sel(deliver), event);
            }
        }
    }
}

/// The ONE Objective-C block in this crate. `takeSnapshotWithConfiguration:`
/// has no other door — there is no message-bus detour for pixels the
/// way there is for an eval's value — so the block literal is written
/// by hand: the layout the runtime documents, a POD capture (the
/// token), and no copy/dispose helpers, which tells `_Block_copy`
/// that a byte copy is the whole move.
#[repr(C)]
struct SnapshotBlock {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: extern "C" fn(*mut SnapshotBlock, Id, Id),
    descriptor: *const BlockDescriptor,
    /// The capture — plain data, safe to byte-copy.
    token: u64,
}

#[repr(C)]
struct BlockDescriptor {
    reserved: u64,
    size: u64,
}

static SNAPSHOT_DESCRIPTOR: BlockDescriptor = BlockDescriptor {
    reserved: 0,
    size: std::mem::size_of::<SnapshotBlock>() as u64,
};

#[link(name = "System", kind = "dylib")]
unsafe extern "C" {
    static _NSConcreteStackBlock: [*const c_void; 32];
}

/// The completion lands here, on the main thread: `(image, error)`.
extern "C" fn snapshot_landed(block: *mut SnapshotBlock, image: Id, error: Id) {
    let token = unsafe { (*block).token };
    let result = if image.is_null() {
        Err(unsafe { error_name(error) })
    } else {
        unsafe { image_rgba(image) }
    };
    dispatch(WebviewEvent::SnapshotDone { token, result });
}

/// The page as an image — the answer rides the dispatch, by token,
/// like an eval's. `WKSnapshotConfiguration` stays nil: the visible
/// viewport is the picture.
pub(crate) fn snapshot(view: Id, token: u64) {
    let block = SnapshotBlock {
        isa: (&raw const _NSConcreteStackBlock) as *const c_void,
        flags: 0,
        reserved: 0,
        invoke: snapshot_landed,
        descriptor: &SNAPSHOT_DESCRIPTOR,
        token,
    };
    unsafe {
        // the engine copies the block before this call returns — the
        // stack literal only has to live through the send
        msg_void_id_id(
            view,
            sel("takeSnapshotWithConfiguration:completionHandler:"),
            null_mut(),
            (&raw const block) as Id,
        );
    }
}

/// The error's own words, or a name for silence.
unsafe fn error_name(error: Id) -> String {
    if error.is_null() {
        return String::from("the engine answered nothing");
    }
    unsafe {
        let description = msg_id(error, sel("localizedDescription"));
        let text = to_string(description);
        if text.is_empty() { String::from("the engine refused unnamed") } else { text }
    }
}

/// NSImage → straight RGBA, tightly packed. The floor accepts what a
/// snapshot actually produces (8-bit RGBA or RGB); anything stranger
/// is refused by name rather than misread.
unsafe fn image_rgba(image: Id) -> Result<(usize, usize, Vec<u8>), String> {
    unsafe {
        let tiff = msg_id(image, sel("TIFFRepresentation"));
        if tiff.is_null() {
            return Err(String::from("the image had no representation"));
        }
        let rep = msg_id_id(class("NSBitmapImageRep"), sel("imageRepWithData:"), tiff);
        if rep.is_null() {
            return Err(String::from("the image did not decode"));
        }
        let width = msg_i64(rep, sel("pixelsWide")) as usize;
        let height = msg_i64(rep, sel("pixelsHigh")) as usize;
        let samples = msg_i64(rep, sel("samplesPerPixel")) as usize;
        let bits = msg_i64(rep, sel("bitsPerSample")) as usize;
        let stride = msg_i64(rep, sel("bytesPerRow")) as usize;
        if width == 0 || height == 0 {
            return Err(String::from("the image had no pixels"));
        }
        if bits != 8 || (samples != 4 && samples != 3) {
            return Err(format!(
                "unexpected pixel format: {samples} samples of {bits} bits"
            ));
        }
        let data = msg_id(rep, sel("bitmapData")) as *const u8;
        if data.is_null() {
            return Err(String::from("the image kept its bytes"));
        }
        let mut rgba = Vec::with_capacity(width * height * 4);
        for row in 0..height {
            let line = data.add(row * stride);
            for column in 0..width {
                let pixel = line.add(column * samples);
                rgba.push(*pixel);
                rgba.push(*pixel.add(1));
                rgba.push(*pixel.add(2));
                rgba.push(if samples == 4 { *pixel.add(3) } else { 255 });
            }
        }
        Ok((width, height, rgba))
    }
}

/// A borrowed NSString for the message being sent (autoreleased).
unsafe fn ns(text: &str) -> Id {
    let text = CString::new(text).unwrap_or_default();
    unsafe { msg_id_cstr(class("NSString"), sel("stringWithUTF8String:"), text.as_ptr()) }
}

/// The NSString's bytes, copied out.
unsafe fn to_string(ns: Id) -> String {
    if ns.is_null() {
        return String::new();
    }
    unsafe {
        let utf8 = msg_id(ns, sel("UTF8String")) as *const c_char;
        if utf8.is_null() {
            return String::new();
        }
        CStr::from_ptr(utf8).to_string_lossy().into_owned()
    }
}
