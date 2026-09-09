//! The webview tenant: WKWebView behind the native host, on every
//! Apple platform.
//!
//! The OS already ships a browser engine; this module mounts it in
//! the hole the layout keeps (`docs/webview.md`). The engine draws,
//! scrolls and reads input itself — the shell creates the view,
//! points it at a url, moves the box, and holds ONE return channel:
//! the script message bridge. Everything the page sends back rides
//! it — the app's bus (`window.bunny.post`) and the eval answers —
//! so the only Objective-C block this crate AUTHORS is the snapshot's.
//!
//! The pages are filed here by the host's path, so a report that
//! arrives holding the VIEW finds its box's identity without asking
//! the shell. What differs per platform is small and said where it
//! differs: how the keyboard is taken, how a snapshot's image reads
//! out, and what the backend serves — the Mac alone types synthetic
//! events into a page (its own `input`, in its own crate).
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

use bunny_ui::host::{Document, EDITOR_SCRIPT, EditorAction, EditorReport, HostSpec, editor_report};

use crate::ffi::{CGPoint, CGRect, CGSize, Id, Sel, class, sel};

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
    #[cfg(not(target_os = "macos"))]
    #[link_name = "objc_msgSend"]
    fn msg_bool(obj: Id, sel: Sel) -> i8;
    fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra: usize) -> Id;
    fn objc_registerClassPair(class: Id);
    fn class_addMethod(class: Id, sel: Sel, imp: *const c_void, types: *const c_char) -> i8;
    fn objc_getProtocol(name: *const c_char) -> Id;
    fn class_addProtocol(class: Id, protocol: Id) -> i8;
}

/// What the page sent back over the bridge — delivered to the shell's
/// dispatch, on the main thread, from WebKit's own runloop callbacks.
#[derive(Clone)]
pub enum WebviewEvent {
    /// The engine committed a navigation — link clicks included.
    Navigated { path: String, url: String },
    /// A link in a DOCUMENT was activated. The engine did not follow
    /// it: the document stays, and the app hears the url.
    Linked { path: String, url: String },
    /// An editable document's body changed under the person's hand.
    Changed { path: String, html: String },
    /// A paste the app owns: the clipboard's html and text, nothing
    /// inserted.
    Pasted { path: String, html: String, text: String },
    /// The engine REFUSED one: the url it tried, and why — the other
    /// leg of the same pair, so no load ends in silence.
    NavigationFailed { path: String, url: String, why: String },
    /// The page called `window.bunny.post(…)`.
    Posted { path: String, body: String },
    /// The page's console spoke — `"level: what it said"`.
    Console { path: String, line: String },
    /// A request of the page's completed — `"METHOD url status"`.
    Requested { path: String, line: String },
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
    /// Every page mounted, by host path — how a report holding the VIEW
    /// finds the box's identity. Filed at `create`, forgotten at `sweep`.
    static PAGES: RefCell<HashMap<String, Id>> = RefCell::new(HashMap::new());
}

/// The host path a page is mounted under, if it still is.
fn key_of(view: Id) -> Option<String> {
    PAGES.with(|pages| {
        pages.borrow().iter().find(|(_, page)| std::ptr::eq(**page, view)).map(|(key, _)| key.clone())
    })
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
    /// The editor takes the keyboard at the commit — once. The view
    /// has no window at its creation (the host adds it after), so the
    /// commit is the first beat the keyboard can be taken at.
    focus: bool,
}

/// `WKNavigationActionPolicy` — what the delegate answers with.
const POLICY_CANCEL: i64 = 0;
const POLICY_ALLOW: i64 = 1;
/// `WKNavigationTypeLinkActivated` — a link the person activated.
const NAVIGATION_LINK: i64 = 0;

/// The shell installs the landing spot for everything a page reports.
pub fn set_dispatch(dispatch: impl Fn(WebviewEvent) + 'static) {
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
                if let Some(path) = key_of(msg_id(message, sel("webView"))) {
                    dispatch(WebviewEvent::Posted { path, body });
                }
            }
            "bunnyConsole" => {
                if let Some(path) = key_of(msg_id(message, sel("webView"))) {
                    dispatch(WebviewEvent::Console { path, line: body });
                }
            }
            "bunnyNet" => {
                if let Some(path) = key_of(msg_id(message, sel("webView"))) {
                    dispatch(WebviewEvent::Requested { path, line: body });
                }
            }
            "bunnyEdit" => {
                let Some(path) = key_of(msg_id(message, sel("webView"))) else {
                    return;
                };
                match editor_report(&body) {
                    Some(EditorReport::Changed(html)) => {
                        dispatch(WebviewEvent::Changed { path, html });
                    }
                    Some(EditorReport::Pasted { html, text }) => {
                        dispatch(WebviewEvent::Pasted { path, html, text });
                    }
                    None => {}
                }
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
    let Some(path) = key_of(view) else {
        return;
    };
    let wants_keyboard = LETTERS.with(|letters| {
        let mut letters = letters.borrow_mut();
        let Some(letter) = letters.get_mut(&path) else {
            return false;
        };
        letter.expected = false;
        std::mem::replace(&mut letter.focus, false)
    });
    if wants_keyboard {
        unsafe { take_keyboard(view) };
    }
    unsafe {
        let url = msg_id(view, sel("URL"));
        if url.is_null() {
            return;
        }
        let url = to_string(msg_id(url, sel("absoluteString")));
        dispatch(WebviewEvent::Navigated { path, url });
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
    let Some(path) = key_of(view) else {
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
    let sealed =
        key_of(view).is_some_and(|path| LETTERS.with(|letters| letters.borrow().contains_key(&path)));
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
        if let Some(path) = key_of(view)
            && !url.is_empty()
        {
            dispatch(WebviewEvent::Linked { path, url });
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
        if let Some(path) = key_of(view) {
            dispatch(WebviewEvent::NavigationFailed { path, url, why: error_name(error) });
        }
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
pub fn create(path: &str, spec: &HostSpec) -> Id {
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
        PAGES.with(|pages| pages.borrow_mut().insert(path.to_string(), view));
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
            Letter { digest: document.digest, expected: true, focus: document.focus },
        );
    });
    unsafe {
        let base = if document.base.is_empty() { null_mut() } else { ns_url(&document.base) };
        let _ = msg_id_id_id(
            view,
            sel("loadHTMLString:baseURL:"),
            ns(&document.sealed()),
            base,
        );
    }
}

/// The view becomes the window's first responder — the keyboard is
/// the page's. A view with no window yet takes nothing.
#[cfg(target_os = "macos")]
unsafe fn take_keyboard(view: Id) {
    unsafe {
        let window = msg_id(view, sel("window"));
        if !window.is_null() {
            let _ = msg_bool_id(window, sel("makeFirstResponder:"), view);
        }
    }
}

/// The same, where a responder claims the keyboard itself.
#[cfg(not(target_os = "macos"))]
unsafe fn take_keyboard(view: Id) {
    unsafe {
        let window = msg_id(view, sel("window"));
        if !window.is_null() {
            let _ = msg_bool(view, sel("becomeFirstResponder"));
        }
    }
}

/// One editing action on the document — the allowlist's script, run
/// on the engine. The editor takes the keyboard back first (a toolbar
/// click took it), except for the app's own write of the whole body,
/// which needs no selection. Fire-and-forget, like the hand.
pub fn edit(view: Id, action: &EditorAction) {
    let script = action.script();
    if script.is_empty() {
        return;
    }
    unsafe {
        if !matches!(action, EditorAction::SetHtml(_)) {
            take_keyboard(view);
        }
        run_script(view, &script);
    }
}

/// Runs `js` on the page, answer discarded — the completion handler
/// stays nil, so no block crosses this border.
unsafe fn run_script(view: Id, js: &str) {
    unsafe {
        msg_void_id_id(view, sel("evaluateJavaScript:completionHandler:"), ns(js), null_mut());
    }
}

/// Forgets the pages and the documents whose hosts left the scene —
/// called beside the host sweep, with the paths still standing.
pub fn sweep(alive: &[String]) {
    LETTERS.with(|letters| letters.borrow_mut().retain(|path, _| alive.contains(path)));
    PAGES.with(|pages| pages.borrow_mut().retain(|path, _| alive.contains(path)));
}

/// Hands the controller the bridge and the document-start scripts.
/// Every channel registers up front (a registered name that never
/// speaks costs nothing, and re-adding one throws); the scripts are
/// [`apply_scripts`]'s.
unsafe fn install_bridge(controller: Id, spec: &bunny_ui::host::HostSpec) {
    unsafe {
        let bridge = bridge();
        for channel in ["bunny", "bunnyConsole", "bunnyNet", "bunnyEval", "bunnyEdit"] {
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
/// nobody watches pays for no capture — then the editor for an
/// editable document (its transport first, the framework's script
/// after), then the app's own scripts, in declaration order.
unsafe fn apply_scripts(controller: Id, spec: &HostSpec) {
    let HostSpec::Webview { scripts, console, requests, document, .. } = spec;
    unsafe {
        add_script(controller, BOOT);
        if *console {
            add_script(controller, CONSOLE_HOOK);
        }
        if *requests {
            add_script(controller, NET_WRAP);
        }
        if let Some(document) = document
            && document.editable
        {
            add_script(controller, &editor_prelude(document));
            add_script(controller, EDITOR_SCRIPT);
        }
        for script in scripts.iter() {
            add_script(controller, script);
        }
    }
}

/// What the editor script expects to find: the document's asks, and
/// this backend's road for a report line — the `bunnyEdit` channel.
fn editor_prelude(document: &Document) -> String {
    format!(
        "window.__bunnyEditor = {{ paste: {}, focus: {}, send: function(line) {{ \
         window.webkit.messageHandlers.bunnyEdit.postMessage(line); }} }};",
        document.paste, document.focus
    )
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
pub fn update(path: &str, view: Id, spec: &HostSpec) {
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
/// (`docs/webview.md` — the capability table's WKWebView columns):
/// console and requests by injected hook, the editor, and — on the
/// Mac alone — synthetic input by real NSEvent. No response bodies:
/// the one cell an injected wrap cannot reach. The phone has no
/// event constructor a page trusts, so it does not claim the cell.
pub fn capabilities() -> &'static [bunny_ui::host::WebviewCapability] {
    use bunny_ui::host::WebviewCapability;
    #[cfg(target_os = "macos")]
    {
        &[
            WebviewCapability::ConsoleMessages,
            WebviewCapability::NetworkRequests,
            WebviewCapability::SyntheticInput,
            WebviewCapability::HtmlEditor,
        ]
    }
    #[cfg(not(target_os = "macos"))]
    {
        &[
            WebviewCapability::ConsoleMessages,
            WebviewCapability::NetworkRequests,
            WebviewCapability::HtmlEditor,
        ]
    }
}

/// Points the engine at `url` — the load is the engine's own affair,
/// asynchronous and cancellable by the next call.
pub fn navigate(view: Id, url: &str) {
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
pub fn back(view: Id) {
    unsafe {
        let _ = msg_id(view, sel("goBack"));
    }
}

/// And one forward.
pub fn forward(view: Id) {
    unsafe {
        let _ = msg_id(view, sel("goForward"));
    }
}

/// Evaluates `js` as an EXPRESSION in the page. The answer rides the
/// bridge (`bunnyEval`, by token) — the completion handler stays nil,
/// so no block ever crosses this border. `raw` hands the value back as
/// the string it is (a null answers empty); otherwise it rides as JSON.
pub fn eval(view: Id, token: u64, js: &str, raw: bool) {
    let serialize = if raw {
        "(__v === undefined || __v === null) ? \"\" : String(__v)"
    } else {
        "JSON.stringify(__v)"
    };
    let wrapped = format!(
        "(function() {{ try {{ \
           var __v = (function() {{ return ( {js} ); }})(); \
           var __s = {serialize}; \
           window.webkit.messageHandlers.bunnyEval.postMessage(\
             \"{token}\\tok\\t\" + (__s === undefined ? \"null\" : __s)); \
         }} catch (e) {{ \
           window.webkit.messageHandlers.bunnyEval.postMessage(\
             \"{token}\\terr\\t\" + String(e)); \
         }} }})();"
    );
    unsafe { run_script(view, &wrapped) }
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
pub fn snapshot(view: Id, token: u64) {
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
#[cfg(target_os = "macos")]
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

/// UIImage → straight RGBA, tightly packed: the image's CGImage is
/// drawn once into a context over our own buffer (the context only
/// draws premultiplied, so the rows are unpremultiplied on the way
/// out — the text engine's own pass).
#[cfg(not(target_os = "macos"))]
unsafe fn image_rgba(image: Id) -> Result<(usize, usize, Vec<u8>), String> {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGImageGetWidth(image: Id) -> usize;
        fn CGImageGetHeight(image: Id) -> usize;
        fn CGBitmapContextCreate(
            data: *mut c_void,
            width: usize,
            height: usize,
            bits_per_component: usize,
            bytes_per_row: usize,
            space: *mut c_void,
            bitmap_info: u32,
        ) -> *mut c_void;
        fn CGContextRelease(context: *mut c_void);
    }
    /// `kCGImageAlphaPremultipliedLast` — the only RGBA layout a
    /// drawing context accepts.
    const ALPHA_PREMULTIPLIED_LAST: u32 = 1;
    unsafe {
        let cg = msg_id(image, sel("CGImage"));
        if cg.is_null() {
            return Err(String::from("the image had no bitmap"));
        }
        let width = CGImageGetWidth(cg);
        let height = CGImageGetHeight(cg);
        if width == 0 || height == 0 {
            return Err(String::from("the image had no pixels"));
        }
        let mut rgba = vec![0u8; width * height * 4];
        let space = crate::ffi::CGColorSpaceCreateDeviceRGB();
        let context = CGBitmapContextCreate(
            rgba.as_mut_ptr() as *mut c_void,
            width,
            height,
            8,
            width * 4,
            space,
            ALPHA_PREMULTIPLIED_LAST,
        );
        if context.is_null() {
            crate::ffi::CGColorSpaceRelease(space);
            return Err(String::from("no drawing context for the image"));
        }
        crate::ffi::CGContextDrawImage(
            context as Id,
            CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize { width: width as f64, height: height as f64 },
            },
            cg,
        );
        CGContextRelease(context);
        crate::ffi::CGColorSpaceRelease(space);
        for pixel in rgba.chunks_exact_mut(4) {
            let alpha = pixel[3] as u32;
            if alpha != 0 && alpha != 255 {
                for channel in &mut pixel[..3] {
                    *channel = ((*channel as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
                }
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
