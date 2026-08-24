//! The webview tenant: WKWebView behind the native host.
//!
//! The OS already ships a browser engine; this module mounts it in
//! the hole the layout keeps (`docs/webview.md`). The engine draws,
//! scrolls and reads input itself — the shell creates the view,
//! points it at a url, moves the box, and holds ONE return channel:
//! the script message bridge. Everything the page sends back rides
//! it — the app's bus (`window.bunny.post`) and the eval answers —
//! so there is no Objective-C block ABI anywhere in this crate.

use std::cell::{Cell, RefCell};
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::null_mut;

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
    fn msg_bool_sel(obj: Id, sel: Sel, a: Sel) -> i8;
    #[link_name = "objc_msgSend"]
    fn msg_init_config(obj: Id, sel: Sel, frame: CGRect, config: Id) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_init_script(obj: Id, sel: Sel, source: Id, time: i64, main_only: i8) -> Id;

    fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra: usize) -> Id;
    fn objc_registerClassPair(class: Id);
    fn class_addMethod(class: Id, sel: Sel, imp: *const c_void, types: *const c_char) -> i8;
    fn objc_getProtocol(name: *const c_char) -> Id;
    fn class_addProtocol(class: Id, protocol: Id) -> i8;
}

/// What the page sent back over the bridge — delivered to the shell's
/// dispatch, on the main thread, from WebKit's own runloop callbacks.
pub(crate) enum WebviewEvent {
    /// The engine committed a navigation — link clicks included.
    Navigated { view: Id, url: String },
    /// The page called `window.bunny.post(…)`.
    Posted { view: Id, body: String },
    /// The page's console spoke — `"level: what it said"`.
    Console { view: Id, line: String },
    /// A request of the page's completed — `"METHOD url status"`.
    Requested { view: Id, line: String },
    /// An eval answered, by token — `Ok` is JSON, `Err` the thrown
    /// error's name.
    EvalDone { token: u64, result: Result<String, String> },
}

thread_local! {
    /// Where bridge callbacks land — the shell installs it at window
    /// start. Taken out while it runs, the way the app handler is.
    static DISPATCH: RefCell<Option<Box<dyn Fn(WebviewEvent)>>> = const { RefCell::new(None) };
    /// The ONE bridge instance — message handler and navigation
    /// delegate for every webview in the window (the events carry the
    /// view, so one listener serves all).
    static BRIDGE: Cell<Id> = const { Cell::new(null_mut()) };
}

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

/// `webView:didCommitNavigation:` — the url is real from here on.
extern "C" fn bridge_committed(_this: Id, _sel: Sel, view: Id, _navigation: Id) {
    unsafe {
        let url = msg_id(view, sel("URL"));
        if url.is_null() {
            return;
        }
        let url = to_string(msg_id(url, sel("absoluteString")));
        dispatch(WebviewEvent::Navigated { view, url });
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
            for protocol in ["WKScriptMessageHandler", "WKNavigationDelegate"] {
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
/// the spec's url. The reference comes back with ONE retain — the
/// host's sweep releases it when the box leaves the scene.
pub(crate) fn create(spec: &bunny_ui::host::HostSpec) -> Id {
    let bunny_ui::host::HostSpec::Webview { url, .. } = spec;
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
        // reference is weak; the bridge outlives every view)
        msg_void_id(view, sel("setNavigationDelegate:"), bridge());
        // the engine's own inspector, where the OS offers the switch
        // (13.3+) — a webview here is a dev's window into a page, and
        // a devtool that cannot open is a quiet page with no name
        if msg_bool_sel(view, sel("respondsToSelector:"), sel("setInspectable:")) != 0 {
            msg_void_bool(view, sel("setInspectable:"), 1);
        }
        navigate(view, url);
        view
    }
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
/// are replaced (they take effect on the next navigation, which the
/// closing `navigate` provides) and the page re-points.
pub(crate) fn update(view: Id, spec: &bunny_ui::host::HostSpec) {
    let bunny_ui::host::HostSpec::Webview { url, .. } = spec;
    unsafe {
        let config = msg_id(view, sel("configuration"));
        let controller = msg_id(config, sel("userContentController"));
        msg_void(controller, sel("removeAllUserScripts"));
        apply_scripts(controller, spec);
        navigate(view, url);
    }
}

/// What this backend serves, as the value the app reads
/// (`docs/webview.md` — the capability table's WKWebView column):
/// console and requests by injected hook, no response bodies, and
/// synthetic input is the open question the table names.
pub fn capabilities() -> &'static [bunny_ui::host::WebviewCapability] {
    use bunny_ui::host::WebviewCapability;
    &[WebviewCapability::ConsoleMessages, WebviewCapability::NetworkRequests]
}

/// Points the engine at `url` — the load is the engine's own affair,
/// asynchronous and cancellable by the next call.
pub(crate) fn navigate(view: Id, url: &str) {
    let Ok(url) = CString::new(url) else {
        // a NUL inside a url is not a url; nothing to load
        return;
    };
    unsafe {
        let string =
            msg_id_cstr(class("NSString"), sel("stringWithUTF8String:"), url.as_ptr());
        let url = msg_id_id(class("NSURL"), sel("URLWithString:"), string);
        if url.is_null() {
            // NSURL said no — an unparseable url loads nothing rather
            // than crashing the request builder
            return;
        }
        let request = msg_id_id(class("NSURLRequest"), sel("requestWithURL:"), url);
        let _ = msg_id_id(view, sel("loadRequest:"), request);
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
