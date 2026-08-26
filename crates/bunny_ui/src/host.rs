//! The native host: a box a PLATFORM view owns.
//!
//! Every other box in the scene is painted by the framework. This one
//! is not: the framework measures it, places it, clips it and shows or
//! hides it with its subtree — and the platform composites its own
//! view above the scene, in the hole the layout keeps. Nothing the
//! framework draws can appear on top of it; an overlay that would
//! cross the island repositions, or becomes a native view itself. The
//! box reports its rectangle so an app can make that call
//! deliberately. The full contract is `docs/webview.md`.
//!
//! The first tenant is the webview — the engine the OS already ships
//! (WKWebView, WebView2, WebKitGTK), at zero bytes of bundled browser:
//!
//! ```ignore
//! webview("https://docs.example.dev")
//! ```
//!
//! This is a different door from `canvas` and `custom`, which paint
//! with the framework's own commands and clip like everything else.
//! The host is for content that arrives with its own renderer.

use std::cell::RefCell;
use std::rc::Rc;

use motor::state::Context;
use motor::view::RenderNode;

use crate::layout::LayoutNode;
use crate::view::{NodeList, Single, View};

/// What a host's box holds — the value that rides in the node, the way
/// a scroll region carries its offset. The shell reads it from the
/// placement and mounts the platform view; a spec that changes
/// re-instructs the mounted view, it never re-creates the box.
#[derive(Clone, Debug, PartialEq)]
pub enum HostSpec {
    /// The OS webview: `url` to show, and the app's user scripts —
    /// injected at document start, on every navigation, so a page
    /// never renders before the instrumentation is in place.
    Webview {
        url: Rc<str>,
        scripts: Rc<[Rc<str>]>,
        /// The app listens to the page's console — a backend that
        /// serves it by injected hook only pays the hook when this is
        /// on (nothing is captured for a page nobody watches).
        console: bool,
        /// The app observes the page's requests — same rule. The
        /// injected wrap sees `fetch` and XHR, never subresources; a
        /// backend with native capture sees everything.
        requests: bool,
    },
}

/// What a webview backend can serve — the capability table of
/// `docs/webview.md` as a value. The three engines do not offer the
/// same instrumentation and the API does not pretend they do: an app
/// reads the shell's declared set and decides its own shape per
/// platform, instead of half-working on two of three.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebviewCapability {
    /// The page's console reaches `on_console`.
    ConsoleMessages,
    /// The page's fetch/XHR traffic reaches `on_request`.
    NetworkRequests,
    /// Response BODIES can be read — no injected wrap serves this.
    NetworkBodies,
    /// Input can be synthesized into the page.
    SyntheticInput,
}

/// What the page sends back through the one return channel. The engine
/// serializes the value; a page that threw answers with the error's
/// name — never an empty answer that looks like a quiet page.
pub type EvalResult = Result<String, String>;

/// A handle's queue, shared between the app's clone and the retained
/// registration — the runtime drains it once per frame.
pub(crate) type CommandQueue = Rc<RefCell<Vec<WebviewCommand>>>;

/// An eval's `then`, parked in the runtime until the shell answers.
pub(crate) type EvalSink = Box<dyn FnOnce(EvalResult)>;

/// The page as an image: straight RGBA, row-major, tightly packed —
/// the same bytes `ImageSource::Rgba` takes, so a snapshot can go
/// straight back into a scene.
#[derive(Clone, Debug, PartialEq)]
pub struct WebviewSnapshot {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// A snapshot's answer — the image, or the engine's refusal by name.
pub type SnapshotResult = Result<WebviewSnapshot, String>;

/// A snapshot's `then`, parked like an eval's.
pub(crate) type SnapshotSink = Box<dyn FnOnce(SnapshotResult)>;

/// A command the app queued on a [`WebviewHandle`] — drained by the
/// shell each frame ([`crate::runtime::Runtime::webview_commands`])
/// and spent on the mounted engine.
pub enum WebviewCommand {
    /// Point the engine at a url (the declarative `webview(url)` stays
    /// the mount-time truth; this is the imperative door).
    Navigate(Rc<str>),
    /// One step back in the engine's own navigation stack.
    Back,
    /// And one forward.
    Forward,
    /// Evaluate `js` as an EXPRESSION in the page, and hand the
    /// serialized value back — `then` fires on the app's own thread,
    /// on a later frame.
    Eval { js: Rc<str>, then: Box<dyn FnOnce(EvalResult)> },
    /// The page as an image — what the engine shows right now.
    Snapshot { then: SnapshotSink },
}

/// A drained command, addressed — what a shell spends on the mounted
/// engine, once per frame
/// ([`crate::runtime::Runtime::webview_commands`]).
pub enum WebviewOp {
    Navigate { path: String, url: Rc<str> },
    Back { path: String },
    Forward { path: String },
    /// The answer goes back through
    /// [`crate::runtime::Runtime::webview_eval_done`], by token — an
    /// op whose page is not mounted deserves an `Err` with a name,
    /// never silence.
    Eval { path: String, token: u64, js: Rc<str> },
    /// The answer goes back through
    /// [`crate::runtime::Runtime::webview_snapshot_done`], by token —
    /// the same law as the eval's.
    Snapshot { path: String, token: u64 },
}

/// The imperative half of the webview, because a page is a document
/// with a navigation stack, not a view tree — a declarative API
/// pretending otherwise would be a fiction over `goBack()`.
///
/// Cheap to clone, bound to a view with [`WebviewView::handle`]. The
/// commands queue here and the shell spends them on the mounted
/// engine each frame; a handle bound to nothing queues into the void
/// (an eval's answer then names it: the webview is not mounted).
///
/// ```ignore
/// let page = WebviewHandle::new();
/// webview(url).handle(&page)
/// // …later, from a click:
/// page.eval("document.title", move |title| tab.set(title));
/// ```
#[derive(Clone, Default)]
pub struct WebviewHandle {
    queue: CommandQueue,
}

impl WebviewHandle {
    pub fn new() -> WebviewHandle {
        WebviewHandle::default()
    }

    /// Points the page at `url` — the engine's own load, asynchronous
    /// and cancellable by the next call.
    pub fn navigate(&self, url: impl Into<Rc<str>>) {
        self.queue.borrow_mut().push(WebviewCommand::Navigate(url.into()));
    }

    /// One step back in the page's own history.
    pub fn back(&self) {
        self.queue.borrow_mut().push(WebviewCommand::Back);
    }

    /// And one forward.
    pub fn forward(&self) {
        self.queue.borrow_mut().push(WebviewCommand::Forward);
    }

    /// Evaluates `js` as an EXPRESSION in the page and hands the
    /// value back, serialized (`Ok` is JSON; a page that threw
    /// answers `Err` with the error's name). `then` fires on the
    /// app's own thread, on a later frame — never re-entrantly.
    pub fn eval(&self, js: impl Into<Rc<str>>, then: impl FnOnce(EvalResult) + 'static) {
        self.queue
            .borrow_mut()
            .push(WebviewCommand::Eval { js: js.into(), then: Box::new(then) });
    }

    /// The page as an image, as the engine shows it right now —
    /// `then` fires on the app's own thread, on a later frame, with
    /// the pixels or the engine's refusal by name.
    pub fn snapshot(&self, then: impl FnOnce(SnapshotResult) + 'static) {
        self.queue.borrow_mut().push(WebviewCommand::Snapshot { then: Box::new(then) });
    }

    pub(crate) fn share_queue(&self) -> CommandQueue {
        Rc::clone(&self.queue)
    }
}

/// The view a webview enters the scene as — see [`webview`].
#[derive(Clone)]
pub struct WebviewView {
    url: Rc<str>,
    scripts: Vec<Rc<str>>,
    on_navigate: Option<crate::reconciler::WebviewReport>,
    on_navigate_failed: Option<crate::reconciler::WebviewFailure>,
    on_message: Option<crate::reconciler::WebviewReport>,
    on_console: Option<crate::reconciler::WebviewReport>,
    on_request: Option<crate::reconciler::WebviewReport>,
    handle: Option<WebviewHandle>,
}

impl WebviewView {
    /// A script the engine runs at DOCUMENT START, on every
    /// navigation — the page never renders before the app's
    /// instrumentation is in place. Main frame only.
    pub fn user_script(mut self, src: impl Into<Rc<str>>) -> WebviewView {
        self.scripts.push(src.into());
        self
    }

    /// The page moved: fires with the committed url — the engine's own
    /// navigations included (a link click is a navigation the app
    /// never asked for, and history wants it anyway).
    pub fn on_navigate(mut self, action: impl Fn(&str) + 'static) -> WebviewView {
        self.on_navigate = Some(Rc::new(action));
        self
    }

    /// The page did NOT move: a load the engine refused fires with the
    /// url it tried and the reason in the engine's own words — a name
    /// a person can read, never a number.
    ///
    /// The two hooks are one pair, and every load answers in exactly
    /// one of them: an app that waits for `on_navigate` before it
    /// calls a page ready waits for ever on a dead host, a bad
    /// certificate or a server that is down. A load that ANOTHER
    /// navigation replaced is not a failure — the one that replaced it
    /// reports for both.
    ///
    /// ```ignore
    /// webview(url)
    ///     .on_navigate(move |url| bar.set(url.into()))
    ///     .on_navigate_failed(move |url, why| bar.set(format!("{url} — {why}")))
    /// ```
    pub fn on_navigate_failed(mut self, action: impl Fn(&str, &str) + 'static) -> WebviewView {
        self.on_navigate_failed = Some(Rc::new(action));
        self
    }

    /// The page posted: `window.bunny.post(…)` in the page lands here,
    /// as the string the page sent. The same channel discipline
    /// `.task` uses — the page posts, the app receives.
    pub fn on_message(mut self, action: impl Fn(&str) + 'static) -> WebviewView {
        self.on_message = Some(Rc::new(action));
        self
    }

    /// The page's console, one line per call: `"level: what it said"`
    /// (uncaught errors included). Served where the backend declares
    /// [`WebviewCapability::ConsoleMessages`] — on WKWebView by an
    /// injected hook, which only rides when this is declared.
    pub fn on_console(mut self, action: impl Fn(&str) + 'static) -> WebviewView {
        self.on_console = Some(Rc::new(action));
        self
    }

    /// The page's requests, one line per completion:
    /// `"METHOD url status"`. Served where the backend declares
    /// [`WebviewCapability::NetworkRequests`] — on WKWebView by an
    /// injected wrap of fetch and XHR, which is BLIND to
    /// subresources (an image, a stylesheet); a backend with native
    /// capture sees everything.
    pub fn on_request(mut self, action: impl Fn(&str) + 'static) -> WebviewView {
        self.on_request = Some(Rc::new(action));
        self
    }

    /// Binds the imperative half — see [`WebviewHandle`].
    pub fn handle(mut self, handle: &WebviewHandle) -> WebviewView {
        self.handle = Some(handle.clone());
        self
    }
}

impl View for WebviewView {
    type Arity = Single;

    fn render_into(&self, _ctx: &Context, out: &mut NodeList) {
        out.push(RenderNode::leaf(if crate::view::print_enabled() {
            format!("Webview({})", self.url)
        } else {
            String::new()
        }));
        // outside a pass (a decorative render) there is no identity to
        // key the platform view by — the box still holds its space, it
        // just mounts nothing
        let path = motor::identity::cursor_scope().unwrap_or_default();
        let listening = self.on_navigate.is_some()
            || self.on_navigate_failed.is_some()
            || self.on_message.is_some()
            || self.on_console.is_some()
            || self.on_request.is_some()
            || self.handle.is_some();
        if !path.is_empty() && listening {
            // the writers are retained beside the node, like a scroll
            // binding's — a skipped body's page keeps reporting, and
            // its handle keeps commanding
            crate::reconciler::attribute_webview(
                path.clone(),
                crate::reconciler::WebviewHooks {
                    navigated: self.on_navigate.clone(),
                    failed: self.on_navigate_failed.clone(),
                    posted: self.on_message.clone(),
                    console: self.on_console.clone(),
                    requested: self.on_request.clone(),
                    commands: self.handle.as_ref().map(WebviewHandle::share_queue),
                },
            );
        }
        out.push_layout(LayoutNode::Host {
            path,
            spec: HostSpec::Webview {
                url: self.url.clone(),
                scripts: self.scripts.clone().into(),
                console: self.on_console.is_some(),
                requests: self.on_request.is_some(),
            },
        });
    }
}

/// A page in a box: the OS's own browser engine, held by the layout
/// like any other view. It fills what the parent proposes; `.frame(…)`
/// pins it. The page's scroll, text selection and input are the
/// engine's — native, and free.
///
/// ```ignore
/// webview("https://docs.example.dev")
///     .user_script("window.bunny.post('early')")
///     .on_message(move |body| log.update(|l| l.push(body.into())))
///     .on_navigate(move |url| history.update(|h| h.push(url.into())))
/// ```
pub fn webview(url: impl Into<Rc<str>>) -> WebviewView {
    WebviewView {
        url: url.into(),
        scripts: Vec::new(),
        on_navigate: None,
        on_navigate_failed: None,
        on_message: None,
        on_console: None,
        on_request: None,
        handle: None,
    }
}
