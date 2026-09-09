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
//! The same engine also shows a document the app already HOLDS — a
//! letter, a rendered preview — from memory, under a network policy
//! the engine enforces on every fetch, with its links handed to the
//! app instead of followed:
//!
//! ```ignore
//! webview_html(&letter, "https://mail.example/", NetworkPolicy::Deny)
//!     .on_link(move |url| open_in_browser(url))
//! ```
//!
//! And a document the app holds can be EDITED in place — the composer
//! of a mail client — with the engine's own editing, commands from an
//! allowlist the framework composes, and every change reported back:
//!
//! ```ignore
//! webview_html(&draft, "", NetworkPolicy::Deny)
//!     .editable()
//!     .on_html_change(move |html| body.set(html.into()))
//!     .handle(&editor)
//! // …from a toolbar button:
//! editor.exec(EditorCommand::Bold);
//! ```
//!
//! This is a different door from `canvas` and `custom`, which paint
//! with the framework's own commands and clip like everything else.
//! The host is for content that arrives with its own renderer.

use std::cell::RefCell;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;

use motor::state::Context;
use motor::view::RenderNode;

use crate::action::Modifiers;
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
        /// Where the engine is pointed — or `about:blank` while a
        /// `document` rides, so a backend that does not serve
        /// documents shows an empty page rather than a network load
        /// the policy forbade.
        url: Rc<str>,
        /// A page from MEMORY, sealed under its network policy — the
        /// engine loads it instead of `url`, and follows none of its
        /// links (they report through `on_link`).
        document: Option<Document>,
        scripts: Rc<[Rc<str>]>,
        /// The app listens to the page's console — a backend that
        /// serves it by injected hook only pays the hook when this is
        /// on (nothing is captured for a page nobody watches).
        console: bool,
        /// The app observes the page's requests — same rule. The
        /// injected wrap sees `fetch` and XHR, never subresources; a
        /// backend with native capture sees everything.
        requests: bool,
        /// The page sees `prefers-reduced-motion: no-preference` even
        /// where the OS asks for calm — a testing surface shows the
        /// site as most visitors meet it. Off, the OS preference
        /// passes through, like any browser's.
        full_motion: bool,
    },
}

/// The colours a document is shown in — sealed into its head as the
/// page's own `color-scheme`, so its default text and canvas follow
/// the app's theme instead of the engine's white. Unset, the engine
/// shows the document as any browser would.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum ColorScheme {
    Light,
    Dark,
}

impl ColorScheme {
    /// The keyword the page reads.
    pub fn keyword(self) -> &'static str {
        match self {
            ColorScheme::Light => "light",
            ColorScheme::Dark => "dark",
        }
    }
}

/// What a document shown from memory may reach over the network —
/// the policy [`webview_html`] takes. It is enforced by the ENGINE, on
/// every fetch, before a byte leaves the machine: the policy rides at
/// the document's head as its Content-Security-Policy, which every
/// engine this framework hosts honours for images, stylesheets, fonts,
/// frames, media, scripts and the fetches a script would make. A
/// document under a policy runs no script of its own; the app's user
/// scripts, injected by the engine, still run.
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
pub enum NetworkPolicy {
    /// Nothing leaves the machine. What the document carries inline —
    /// a `data:` image, its own styles — still shows. The default,
    /// and the one a letter from a stranger deserves.
    #[default]
    Deny,
    /// Remote IMAGES load, over http and https; everything else stays
    /// denied. The "load remote content" switch a mail reader flips
    /// per message or per sender.
    RemoteImages,
}

impl NetworkPolicy {
    /// The policy as the Content-Security-Policy the engine enforces.
    /// `form-action 'none'` rides along: a document is read, not
    /// filled in and sent.
    pub fn csp(self) -> &'static str {
        match self {
            NetworkPolicy::Deny => {
                "default-src 'none'; img-src data:; media-src data:; font-src data:; \
                 style-src 'unsafe-inline'; form-action 'none'"
            }
            NetworkPolicy::RemoteImages => {
                "default-src 'none'; img-src data: http: https:; media-src data:; \
                 font-src data:; style-src 'unsafe-inline'; form-action 'none'"
            }
        }
    }

    /// Seals `html` under this policy: the policy's CSP, then `base`
    /// (when there is one), then the colour `scheme` (when one is
    /// named), stand at the document's head, AHEAD of anything the
    /// document brought — so no element of the document is parsed
    /// before the policy is. A leading doctype keeps its
    /// place (standards mode stays the document's to choose; the
    /// tokenizer ends a doctype at the first `>`, so nothing that
    /// loads can hide in one). A byte-order mark is dropped. The
    /// document's own CSP, if it carries one, can only tighten this
    /// one — policies combine, they never loosen.
    pub fn seal(self, html: &str, base: &str, scheme: Option<ColorScheme>) -> String {
        let html = html.strip_prefix('\u{feff}').unwrap_or(html);
        let mut head = String::from("<meta http-equiv=\"Content-Security-Policy\" content=\"");
        head.push_str(self.csp());
        head.push_str("\">");
        if !base.is_empty() {
            head.push_str("<base href=\"");
            for character in base.chars() {
                match character {
                    '&' => head.push_str("&amp;"),
                    '"' => head.push_str("&quot;"),
                    '<' => head.push_str("&lt;"),
                    '>' => head.push_str("&gt;"),
                    other => head.push(other),
                }
            }
            head.push_str("\">");
        }
        if let Some(scheme) = scheme {
            let keyword = scheme.keyword();
            head.push_str("<meta name=\"color-scheme\" content=\"");
            head.push_str(keyword);
            head.push_str("\"><style>:root{color-scheme:");
            head.push_str(keyword);
            head.push_str("}</style>");
        }
        let split = leading_doctype_end(html);
        let mut sealed = String::with_capacity(head.len() + html.len());
        sealed.push_str(&html[..split]);
        sealed.push_str(&head);
        sealed.push_str(&html[split..]);
        sealed
    }
}

/// Where a doctype that opens `html` ends — the byte after its `>` —
/// or 0 when the document does not open with one. Whitespace before
/// it is the document's, and stays.
fn leading_doctype_end(html: &str) -> usize {
    let trimmed = html.trim_start_matches(['\t', '\n', '\x0C', '\r', ' ']);
    let lead = html.len() - trimmed.len();
    let opens = trimmed.get(..9).is_some_and(|open| open.eq_ignore_ascii_case("<!doctype"));
    match trimmed.find('>') {
        Some(close) if opens => lead + close + 1,
        _ => 0,
    }
}

/// A page shown from MEMORY — what [`webview_html`] puts in the spec.
/// The letter as the app gave it, with what the app asked of it; the
/// shells load [`Document::sealed`], and stamp by [`Document::digest`].
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    /// The html as the app gave it.
    pub html: Rc<str>,
    /// Where the document's relative references resolve; empty for
    /// none. Sealed into the head as well, for the engine that has no
    /// door of its own for a base.
    pub base: Rc<str>,
    pub policy: NetworkPolicy,
    /// The colours the document is shown in, when the app names them.
    pub scheme: Option<ColorScheme>,
    /// The document is EDITED in place: the engine's own editing on
    /// the body, the framework's editor script riding at document
    /// start, every change reported through `on_html_change`.
    pub editable: bool,
    /// The app OWNS the paste (`on_paste` is declared): a paste is
    /// intercepted and reported, never inserted by the engine.
    pub paste: bool,
    /// The editor takes the keyboard as soon as it appears.
    pub focus: bool,
    /// A fingerprint of the letter and everything asked of it — what a
    /// shell's stamp compares, so a body that runs again with the same
    /// letter never reloads it, and a changed one always does.
    pub digest: u64,
}

impl Document {
    /// A document under `policy`, shown as the engine would show it.
    pub fn new(
        html: impl Into<Rc<str>>,
        base: impl Into<Rc<str>>,
        policy: NetworkPolicy,
    ) -> Document {
        let mut document = Document {
            html: html.into(),
            base: base.into(),
            policy,
            scheme: None,
            editable: false,
            paste: false,
            focus: false,
            digest: 0,
        };
        document.restamp();
        document
    }

    /// The document as the engine loads it — sealed under its policy,
    /// with its base and its colours at its head
    /// ([`NetworkPolicy::seal`]). Built at the load, once.
    pub fn sealed(&self) -> String {
        self.policy.seal(&self.html, &self.base, self.scheme)
    }

    /// Shown in these colours.
    pub fn color_scheme(mut self, scheme: ColorScheme) -> Document {
        self.scheme = Some(scheme);
        self.restamp();
        self
    }

    /// Edited in place — see [`WebviewView::editable`].
    pub fn editable(mut self) -> Document {
        self.editable = true;
        self.restamp();
        self
    }

    /// The paste is the app's — see [`WebviewView::on_paste`].
    pub fn paste_owned(mut self) -> Document {
        self.paste = true;
        self.restamp();
        self
    }

    /// Takes the keyboard on appearing — see
    /// [`WebviewView::focus_on_appear`].
    pub fn focus_on_appear(mut self) -> Document {
        self.focus = true;
        self.restamp();
        self
    }

    fn restamp(&mut self) {
        let mut hasher = DefaultHasher::new();
        self.html.hash(&mut hasher);
        self.base.hash(&mut hasher);
        self.policy.hash(&mut hasher);
        self.scheme.hash(&mut hasher);
        (self.editable, self.paste, self.focus).hash(&mut hasher);
        self.digest = hasher.finish();
    }
}

/// One editing command, by name — the ALLOWLIST. Each one is the
/// engine's own editing command (`execCommand`), composed by the
/// framework: an app never hands the engine JavaScript.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorCommand {
    Bold,
    Italic,
    Underline,
    Strikethrough,
    Superscript,
    Subscript,
    OrderedList,
    UnorderedList,
    Indent,
    Outdent,
    /// The current block becomes a paragraph.
    Paragraph,
    /// …a quotation.
    Blockquote,
    /// …a heading of this level, 1 to 6.
    Heading(u8),
    /// …preformatted text.
    Preformatted,
    AlignLeft,
    AlignCenter,
    AlignRight,
    HorizontalRule,
    /// The link under the selection is removed; the text stays.
    Unlink,
    /// The selection's inline formatting is removed.
    RemoveFormat,
    SelectAll,
    Undo,
    Redo,
}

impl EditorCommand {
    /// The engine's own name for the command, and the argument the
    /// block commands take.
    fn exec(self) -> (&'static str, Option<String>) {
        match self {
            EditorCommand::Bold => ("bold", None),
            EditorCommand::Italic => ("italic", None),
            EditorCommand::Underline => ("underline", None),
            EditorCommand::Strikethrough => ("strikeThrough", None),
            EditorCommand::Superscript => ("superscript", None),
            EditorCommand::Subscript => ("subscript", None),
            EditorCommand::OrderedList => ("insertOrderedList", None),
            EditorCommand::UnorderedList => ("insertUnorderedList", None),
            EditorCommand::Indent => ("indent", None),
            EditorCommand::Outdent => ("outdent", None),
            EditorCommand::Paragraph => ("formatBlock", Some(String::from("<p>"))),
            EditorCommand::Blockquote => ("formatBlock", Some(String::from("<blockquote>"))),
            EditorCommand::Heading(level) => {
                ("formatBlock", Some(format!("<h{}>", level.clamp(1, 6))))
            }
            EditorCommand::Preformatted => ("formatBlock", Some(String::from("<pre>"))),
            EditorCommand::AlignLeft => ("justifyLeft", None),
            EditorCommand::AlignCenter => ("justifyCenter", None),
            EditorCommand::AlignRight => ("justifyRight", None),
            EditorCommand::HorizontalRule => ("insertHorizontalRule", None),
            EditorCommand::Unlink => ("unlink", None),
            EditorCommand::RemoveFormat => ("removeFormat", None),
            EditorCommand::SelectAll => ("selectAll", None),
            EditorCommand::Undo => ("undo", None),
            EditorCommand::Redo => ("redo", None),
        }
    }
}

/// What a handle asks of an editable document — spent by the shell on
/// the engine as the script [`EditorAction::script`] composes.
#[derive(Clone, Debug, PartialEq)]
pub enum EditorAction {
    /// One command from the allowlist, on the selection.
    Exec(EditorCommand),
    /// The selection becomes a link to `url`.
    Link(Rc<str>),
    /// The body becomes this html — the app's own write, so it is
    /// not reported back as a change.
    SetHtml(Rc<str>),
    /// This html lands at the caret, replacing the selection — what an
    /// app inserts after it decided a paste.
    InsertHtml(Rc<str>),
    /// The editor takes the keyboard.
    Focus,
}

impl EditorAction {
    /// The script the shell runs for this action — composed HERE, from
    /// the allowlist and the app's strings as JavaScript literals, so
    /// the only JavaScript that ever reaches the engine is the
    /// framework's own. A link to a `javascript:` url is not a link to
    /// anywhere, and composes to nothing.
    pub fn script(&self) -> String {
        match self {
            EditorAction::Exec(command) => match command.exec() {
                (name, Some(value)) => format!(
                    "document.execCommand({}, false, {});",
                    js_string(name),
                    js_string(&value)
                ),
                (name, None) => format!("document.execCommand({}, false, null);", js_string(name)),
            },
            EditorAction::Link(url) => {
                let scheme = url.split(':').next().unwrap_or_default();
                if scheme.eq_ignore_ascii_case("javascript") {
                    return String::new();
                }
                format!("document.execCommand(\"createLink\", false, {});", js_string(url))
            }
            EditorAction::SetHtml(html) => {
                format!("document.body.innerHTML = {};", js_string(html))
            }
            EditorAction::InsertHtml(html) => {
                format!("document.execCommand(\"insertHTML\", false, {});", js_string(html))
            }
            EditorAction::Focus => String::from("document.body.focus();"),
        }
    }
}

/// `text` as a JavaScript string literal: quotes, backslashes, line
/// breaks and the two line separators escaped, every control character
/// spelled out, and `<` escaped too — so a `</script>` inside a literal
/// never closes anything.
pub fn js_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '<' => out.push_str("\\u003c"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            control if (control as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// The editor, injected at document start when a document is
/// editable — the same text on every engine. It expects the shell to
/// have defined `window.__bunnyEditor` first: `paste` and `focus` as
/// the spec says them, and `send(line)`, the shell's own road for a
/// report line. The body becomes editable when the document is
/// parsed; every `input` reports the body's html on the next turn
/// (a burst of keys is one report); with `paste` on, a paste is
/// intercepted and reported — html and text, each with its
/// backslashes and tabs escaped — instead of inserted.
pub const EDITOR_SCRIPT: &str = r#"(function() {
  var editor = window.__bunnyEditor || {};
  var send = editor.send || function() {};
  var pending = false;
  function report() { pending = false; send('change\t' + document.body.innerHTML); }
  function esc(text) { return String(text || '').replace(/\\/g, '\\\\').replace(/\t/g, '\\t'); }
  addEventListener('DOMContentLoaded', function() {
    document.body.contentEditable = 'true';
    try { document.execCommand('styleWithCSS', false, false); } catch (e) {}
    document.addEventListener('input', function() {
      if (!pending) { pending = true; setTimeout(report, 0); }
    });
    if (editor.paste) {
      document.addEventListener('paste', function(event) {
        event.preventDefault();
        var data = event.clipboardData;
        send('paste\t' + esc(data && data.getData('text/html')) + '\t'
             + esc(data && data.getData('text/plain')));
      });
    }
    if (editor.focus) { document.body.focus(); }
  });
})();"#;

/// What the editor reported — decoded from one line of the editor's
/// channel by [`editor_report`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorReport {
    /// The body changed under the person's hand; this is its html.
    Changed(String),
    /// A paste the app owns: what the clipboard held, as html and as
    /// text (either may be empty).
    Pasted { html: String, text: String },
}

/// Decodes one line the editor sent — `change\t<html>` or
/// `paste\t<html>\t<text>` with the two halves escaped as the editor
/// escapes them. Anything else is not the editor's, and is dropped.
pub fn editor_report(line: &str) -> Option<EditorReport> {
    let (kind, payload) = line.split_once('\t')?;
    match kind {
        "change" => Some(EditorReport::Changed(payload.to_string())),
        "paste" => {
            let (html, text) = payload.split_once('\t')?;
            Some(EditorReport::Pasted { html: unescape_half(html), text: unescape_half(text) })
        }
        _ => None,
    }
}

/// The editor's escape, undone: `\\` is a backslash, `\t` a tab.
fn unescape_half(half: &str) -> String {
    let mut out = String::with_capacity(half.len());
    let mut characters = half.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        match characters.next() {
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
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
    /// The page's media environment can be emulated — `full_motion`
    /// holds where this is declared; elsewhere the OS preference is
    /// the only truth the engine will tell.
    MediaEmulation,
    /// A document can be EDITED in place — `.editable()`, the editor
    /// commands and the change reports are served. Where this is not
    /// declared an editable document is shown, read-only.
    HtmlEditor,
}

/// What the page sends back through the one return channel. The engine
/// serializes the value; a page that threw answers with the error's
/// name — never an empty answer that looks like a quiet page.
pub type EvalResult = Result<String, String>;

/// Which button a synthetic press uses. A page reads the three by
/// number, and the middle one is the wheel pressed down.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseButton {
    /// The primary button.
    #[default]
    Left,
    /// The secondary button — it opens the PAGE's own context menu.
    Right,
    /// The wheel, pressed.
    Middle,
}

/// One synthetic event, in CSS pixels from the view's own top-left
/// corner — the hand an app lends the page it holds.
///
/// The vocabulary has two halves. `Click`, `Hover`, `Scroll`, `Type`
/// and `Key` are what a tool does: each one is complete in itself. The
/// other three — `Down`, `Drag`, `Up` — are what a HAND does: a
/// button that stays held between events, which is the only way to
/// spell a drag or a selection.
///
/// Served where the backend declares
/// [`WebviewCapability::SyntheticInput`]; a backend without the
/// capability drops what it cannot send (the door is
/// fire-and-forget — there is no answer to refuse in).
#[derive(Clone, Debug, PartialEq)]
pub enum WebviewInput {
    /// Press and release at a point: `clicks` is 1, 2 or 3, and a 2
    /// gets the press-release pair the page counts a double-click by.
    Click { x: f64, y: f64, clicks: u32, button: MouseButton },
    /// Move the pointer with nothing held — what makes a hover state
    /// appear.
    Hover { x: f64, y: f64 },
    /// The wheel at a point. The deltas are the PAGE's own signs: `dy`
    /// counts DOWN and `dx` counts RIGHT, the way the page reads them
    /// in a wheel event.
    Scroll { x: f64, y: f64, dx: f64, dy: f64 },
    /// Insert text where the page's focus is — a commit, the way a
    /// paste or an input method lands, not one key after another. It
    /// works in every field, a `contenteditable` included.
    Type { text: Rc<str> },
    /// One named key: `Enter`, `Tab`, `Escape`, `Backspace`,
    /// `Delete`, `ArrowUp`, `ArrowDown`, `ArrowLeft`, `ArrowRight`,
    /// `Home`, `End`, `PageUp`, `PageDown`, `Space` — the names the
    /// page itself uses. A single character is that character's key.
    Key { key: Rc<str> },
    /// Press without releasing — the start of a drag or a selection.
    Down { x: f64, y: f64, button: MouseButton, clicks: u32, modifiers: Modifiers },
    /// Move with the button still held — the middle of one.
    Drag { x: f64, y: f64, button: MouseButton, modifiers: Modifiers },
    /// Release — the end of one. `clicks` is the count the press
    /// carried, so a page counting double-clicks sees the same number
    /// twice.
    Up { x: f64, y: f64, button: MouseButton, clicks: u32, modifiers: Modifiers },
}

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
    /// Evaluate `js` as an EXPRESSION in the page, and hand the value
    /// back — serialized as JSON, or `raw` as the string it is —
    /// `then` fires on the app's own thread, on a later frame.
    Eval { js: Rc<str>, raw: bool, then: Box<dyn FnOnce(EvalResult)> },
    /// One editing action on an editable document — fire-and-forget,
    /// like the hand.
    Edit(EditorAction),
    /// The page as an image — what the engine shows right now.
    Snapshot { then: SnapshotSink },
    /// One synthetic event into the page — fire-and-forget, like the
    /// hand it stands for.
    Input(WebviewInput),
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
    Eval { path: String, token: u64, js: Rc<str>, raw: bool },
    /// The answer goes back through
    /// [`crate::runtime::Runtime::webview_snapshot_done`], by token —
    /// the same law as the eval's.
    Snapshot { path: String, token: u64 },
    /// No token and no answer: the shell runs the action's script on
    /// the editable document, or the document is not there and the
    /// action is spent on nothing.
    Edit { path: String, action: EditorAction },
    /// No token and no answer: the shell spends it on the engine, or
    /// the page is not there and the event is spent on nothing — the
    /// same as a hand that moves over a window which just closed.
    Input { path: String, event: WebviewInput },
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
        self.queue.borrow_mut().push(WebviewCommand::Eval {
            js: js.into(),
            raw: false,
            then: Box::new(then),
        });
    }

    /// The editable document's body, as html — `then` fires on the
    /// app's own thread, on a later frame, with the html as it is
    /// (not as JSON), or the engine's refusal by name. The same value
    /// `on_html_change` reports, asked for instead of waited for.
    pub fn get_html(&self, then: impl FnOnce(EvalResult) + 'static) {
        self.queue.borrow_mut().push(WebviewCommand::Eval {
            js: Rc::from("document.body.innerHTML"),
            raw: true,
            then: Box::new(then),
        });
    }

    /// One command from the allowlist, on the editable document's
    /// selection — bold, a list, undo. Served where the backend
    /// declares [`WebviewCapability::HtmlEditor`]; the editor takes
    /// the keyboard back first, so a toolbar click that took it away
    /// still finds the selection.
    pub fn exec(&self, command: EditorCommand) {
        self.edit(EditorAction::Exec(command));
    }

    /// The selection becomes a link to `url` — the app asked the
    /// person for it in its own dialog. A `javascript:` url is no
    /// link, and nothing happens.
    pub fn exec_link(&self, url: impl Into<Rc<str>>) {
        self.edit(EditorAction::Link(url.into()));
    }

    /// The body becomes `html` — a reply's quotation, a signature, a
    /// draft restored. The app's own write: it is not reported back.
    pub fn set_html(&self, html: impl Into<Rc<str>>) {
        self.edit(EditorAction::SetHtml(html.into()));
    }

    /// `html` lands at the caret, replacing the selection — what an
    /// app inserts once it decided a paste. Reported back as the
    /// change it is.
    pub fn insert_html(&self, html: impl Into<Rc<str>>) {
        self.edit(EditorAction::InsertHtml(html.into()));
    }

    /// The editor takes the keyboard.
    pub fn focus(&self) {
        self.edit(EditorAction::Focus);
    }

    fn edit(&self, action: EditorAction) {
        self.queue.borrow_mut().push(WebviewCommand::Edit(action));
    }

    /// The page as an image, as the engine shows it right now —
    /// `then` fires on the app's own thread, on a later frame, with
    /// the pixels or the engine's refusal by name.
    pub fn snapshot(&self, then: impl FnOnce(SnapshotResult) + 'static) {
        self.queue.borrow_mut().push(WebviewCommand::Snapshot { then: Box::new(then) });
    }

    /// One synthetic event into the page — the whole vocabulary, for
    /// the drag and the right press the short doors below do not
    /// spell. Fire-and-forget: a hand gets no receipt either.
    ///
    /// ```ignore
    /// page.input(WebviewInput::Down {
    ///     x: 40.0,
    ///     y: 120.0,
    ///     button: MouseButton::Left,
    ///     clicks: 1,
    ///     modifiers: Modifiers::NONE,
    /// });
    /// ```
    pub fn input(&self, event: WebviewInput) {
        self.queue.borrow_mut().push(WebviewCommand::Input(event));
    }

    /// One press and release at a point, with the primary button —
    /// the event nine calls in ten want. CSS pixels, from the view's
    /// own top-left corner.
    pub fn click(&self, x: f64, y: f64) {
        self.input(WebviewInput::Click { x, y, clicks: 1, button: MouseButton::Left });
    }

    /// The pointer moves there, holding nothing — what makes a hover
    /// state appear.
    pub fn hover(&self, x: f64, y: f64) {
        self.input(WebviewInput::Hover { x, y });
    }

    /// The wheel at a point, in the page's own signs: `dy` counts
    /// DOWN and `dx` counts RIGHT.
    pub fn scroll(&self, x: f64, y: f64, dx: f64, dy: f64) {
        self.input(WebviewInput::Scroll { x, y, dx, dy });
    }

    /// Types into whatever the page has focused — a commit, the way a
    /// paste lands, not one key after another.
    pub fn type_text(&self, text: impl Into<Rc<str>>) {
        self.input(WebviewInput::Type { text: text.into() });
    }

    /// One named key, by the name the page itself uses: `Enter`,
    /// `Tab`, `Escape`, `ArrowDown` — see [`WebviewInput::Key`].
    pub fn key(&self, key: impl Into<Rc<str>>) {
        self.input(WebviewInput::Key { key: key.into() });
    }

    pub(crate) fn share_queue(&self) -> CommandQueue {
        Rc::clone(&self.queue)
    }
}

/// The view a webview enters the scene as — see [`webview`].
#[derive(Clone)]
pub struct WebviewView {
    url: Rc<str>,
    document: Option<Document>,
    scripts: Vec<Rc<str>>,
    on_link: Option<crate::reconciler::WebviewReport>,
    on_html_change: Option<crate::reconciler::WebviewReport>,
    on_paste: Option<crate::reconciler::WebviewPaste>,
    on_navigate: Option<crate::reconciler::WebviewReport>,
    on_navigate_failed: Option<crate::reconciler::WebviewFailure>,
    on_message: Option<crate::reconciler::WebviewReport>,
    on_console: Option<crate::reconciler::WebviewReport>,
    on_request: Option<crate::reconciler::WebviewReport>,
    handle: Option<WebviewHandle>,
    full_motion: bool,
}

impl WebviewView {
    /// A script the engine runs at DOCUMENT START, on every
    /// navigation — the page never renders before the app's
    /// instrumentation is in place. Main frame only.
    pub fn user_script(mut self, src: impl Into<Rc<str>>) -> WebviewView {
        self.scripts.push(src.into());
        self
    }

    /// A link in a DOCUMENT was activated — see [`webview_html`]. The
    /// document never follows it: the click lands here, with the
    /// link's url, and the app decides (open it in the browser, look
    /// at it first, refuse it). A `target="_blank"` link lands here
    /// too. Without this hook a link in a document does nothing. A
    /// page shown by url follows its own links, and this never fires.
    ///
    /// ```ignore
    /// webview_html(&letter, base, NetworkPolicy::Deny)
    ///     .on_link(move |url| open_in_browser(url))
    /// ```
    pub fn on_link(mut self, action: impl Fn(&str) + 'static) -> WebviewView {
        self.on_link = Some(Rc::new(action));
        self
    }

    /// The DOCUMENT is edited in place — the composer of a mail client.
    /// The engine's own editing works the body (the caret, the
    /// selection, typing, undo), the commands come from the allowlist
    /// through the handle ([`WebviewHandle::exec`]), and every change
    /// reports through [`WebviewView::on_html_change`]. The policy
    /// holds while editing: a pasted or inserted image from the web
    /// stays blocked under `Deny`. Served where the backend declares
    /// [`WebviewCapability::HtmlEditor`]; elsewhere the document is
    /// shown, read-only. A page shown by url is never editable.
    ///
    /// ```ignore
    /// webview_html(&draft, "", NetworkPolicy::Deny)
    ///     .editable()
    ///     .focus_on_appear()
    ///     .on_html_change(move |html| draft.set(html.into()))
    ///     .handle(&editor)
    /// ```
    pub fn editable(mut self) -> WebviewView {
        self.document = self.document.map(Document::editable);
        self
    }

    /// The body changed under the person's hand: fires with the body's
    /// html, once per turn — a burst of keys is one report. The app's
    /// own writes ([`WebviewHandle::set_html`]) do not fire it; an
    /// insert does, being the change it is.
    pub fn on_html_change(mut self, action: impl Fn(&str) + 'static) -> WebviewView {
        self.on_html_change = Some(Rc::new(action));
        self
    }

    /// The app OWNS the paste. Declared, a paste into the editable
    /// document is intercepted and lands here — what the clipboard
    /// held as html and as text, either possibly empty — and nothing
    /// is inserted until the app says what
    /// ([`WebviewHandle::insert_html`]): the app's sanitizer meets the
    /// clipboard before the document does. Undeclared, the engine
    /// pastes as any browser would, under the policy.
    pub fn on_paste(mut self, action: impl Fn(&str, &str) + 'static) -> WebviewView {
        self.on_paste = Some(Rc::new(action));
        self.document = self.document.map(Document::paste_owned);
        self
    }

    /// The document is shown in these colours — its own default text
    /// and canvas follow the app's theme instead of the engine's
    /// white. Sealed into the head; a document that styles itself
    /// keeps its styles.
    pub fn color_scheme(mut self, scheme: ColorScheme) -> WebviewView {
        self.document = self.document.map(|document| document.color_scheme(scheme));
        self
    }

    /// The editor takes the keyboard as soon as the document appears —
    /// a composer that opens ready to type into.
    pub fn focus_on_appear(mut self) -> WebviewView {
        self.document = self.document.map(Document::focus_on_appear);
        self
    }

    /// The page moved: fires with the committed url — the engine's own
    /// navigations included (a link click is a navigation the app
    /// never asked for, and history wants it anyway). A document
    /// commits once, at its base.
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

    /// The page sees `prefers-reduced-motion: no-preference` even
    /// where the OS asks for calm — a TESTING surface shows the site
    /// as most visitors meet it, not as the tester's accessibility
    /// settings ask. Served where the backend declares
    /// [`WebviewCapability::MediaEmulation`]; elsewhere the OS
    /// preference stays the only truth the engine will tell.
    pub fn full_motion(mut self) -> WebviewView {
        self.full_motion = true;
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
        let listening = self.on_link.is_some()
            || self.on_html_change.is_some()
            || self.on_paste.is_some()
            || self.on_navigate.is_some()
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
                    linked: self.on_link.clone(),
                    changed: self.on_html_change.clone(),
                    pasted: self.on_paste.clone(),
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
                document: self.document.clone(),
                scripts: self.scripts.clone().into(),
                console: self.on_console.is_some(),
                requests: self.on_request.is_some(),
                full_motion: self.full_motion,
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
        document: None,
        scripts: Vec::new(),
        on_link: None,
        on_html_change: None,
        on_paste: None,
        on_navigate: None,
        on_navigate_failed: None,
        on_message: None,
        on_console: None,
        on_request: None,
        handle: None,
        full_motion: false,
    }
}

/// A document the app already holds, shown from MEMORY by the OS's
/// own engine — no file written, no url fetched — under a network
/// policy the engine enforces on every fetch. The document is an
/// island: it runs no script of its own, sends no form, and follows
/// none of its links — a link reports through
/// [`WebviewView::on_link`] and the app decides. `base` is where its
/// relative references resolve (empty for none). The reader of a
/// letter from a stranger:
///
/// ```ignore
/// webview_html(&message.html, &message.base, NetworkPolicy::Deny)
///     .on_link(move |url| app.open(url))
/// ```
///
/// A message from a trusted sender loads its remote images with
/// `NetworkPolicy::RemoteImages` — the app's call, per message. Every
/// other door of the widget serves the document too: user scripts,
/// the bus, eval, the snapshot, the hand.
pub fn webview_html(
    html: impl AsRef<str>,
    base: impl Into<Rc<str>>,
    policy: NetworkPolicy,
) -> WebviewView {
    let mut view = webview("about:blank");
    view.document = Some(Document::new(Rc::from(html.as_ref()), base, policy));
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seal stands AHEAD of the document: nothing the document
    /// brought is parsed before the policy is, and a doctype that
    /// opens it keeps its place.
    #[test]
    fn a_sealed_document_opens_with_its_policy() {
        let sealed = NetworkPolicy::Deny.seal("<p>hi</p>", "", None);
        assert!(sealed.starts_with("<meta http-equiv=\"Content-Security-Policy\" content=\""));
        assert!(sealed.ends_with("\"><p>hi</p>"), "{sealed}");
        assert!(!sealed.contains("<base"), "no base, no base tag");

        let sealed = NetworkPolicy::Deny.seal(
            "  <!DOCTYPE html>\n<html>",
            "https://a.test/?x=1&y=\"2\"",
            None,
        );
        assert!(
            sealed.starts_with("  <!DOCTYPE html><meta http-equiv="),
            "the doctype keeps its place, the seal follows it: {sealed}"
        );
        assert!(
            sealed.contains("<base href=\"https://a.test/?x=1&amp;y=&quot;2&quot;\">\n<html>"),
            "the base is escaped and stands before the document: {sealed}"
        );

        // a byte-order mark is dropped; a doctype that never closes is
        // the whole document, and the seal goes first
        assert!(NetworkPolicy::Deny.seal("\u{feff}<b>", "", None).starts_with("<meta"));
        assert!(NetworkPolicy::Deny.seal("<!doctype html", "", None).starts_with("<meta"));
        // a comment before the doctype is not a doctype: the seal
        // goes first, ahead of it
        assert!(
            NetworkPolicy::Deny.seal("<!-- x --><!DOCTYPE html>", "", None).starts_with("<meta")
        );
        // the colours ride the head too, after the base
        let dark = NetworkPolicy::Deny.seal("<p>", "https://a.test/", Some(ColorScheme::Dark));
        assert!(dark.contains(
            "<base href=\"https://a.test/\"><meta name=\"color-scheme\" content=\"dark\">\
             <style>:root{color-scheme:dark}</style><p>"
        ), "{dark}");
    }

    /// Deny lets nothing out; RemoteImages opens exactly the image
    /// sources over the web, and nothing else.
    #[test]
    fn the_policy_names_what_may_leave() {
        let deny = NetworkPolicy::Deny.csp();
        assert!(deny.starts_with("default-src 'none'"));
        assert!(deny.contains("img-src data:;"), "inline images stay: {deny}");
        assert!(!deny.contains("http"), "nothing over the web: {deny}");
        assert!(deny.contains("form-action 'none'"));

        let images = NetworkPolicy::RemoteImages.csp();
        assert!(images.contains("img-src data: http: https:;"), "{images}");
        assert_eq!(
            images.matches("http").count(),
            2,
            "only the image source names the web: {images}"
        );
        assert_eq!(NetworkPolicy::default(), NetworkPolicy::Deny);
    }

    /// The fingerprint follows the letter, not the allocation: the
    /// same letter built twice stamps the same, a changed one differs.
    #[test]
    fn a_document_is_fingerprinted_by_its_letter() {
        let one = Document::new("<p>a</p>", "https://a.test/", NetworkPolicy::Deny);
        let two = Document::new("<p>a</p>", "https://a.test/", NetworkPolicy::Deny);
        assert_eq!(one.digest, two.digest);
        assert_eq!(one, two);
        let other = Document::new("<p>b</p>", "https://a.test/", NetworkPolicy::Deny);
        assert_ne!(one.digest, other.digest);
        let looser = Document::new("<p>a</p>", "https://a.test/", NetworkPolicy::RemoteImages);
        assert_ne!(one.digest, looser.digest, "the policy is part of the letter");
        assert!(one.sealed().contains("<base href=\"https://a.test/\">"));
        assert_eq!(&*one.html, "<p>a</p>", "the letter itself stays as given");
        // everything asked of the document is part of its fingerprint
        assert_ne!(one.digest, one.clone().editable().digest);
        assert_ne!(one.digest, one.clone().paste_owned().digest);
        assert_ne!(one.digest, one.clone().focus_on_appear().digest);
        assert_ne!(one.digest, one.clone().color_scheme(ColorScheme::Dark).digest);
    }

    /// The allowlist composes the engine's own commands, the app's
    /// strings ride as literals, and a `javascript:` link composes to
    /// nothing.
    #[test]
    fn an_editor_action_composes_the_only_script_the_engine_gets() {
        assert_eq!(
            EditorAction::Exec(EditorCommand::Bold).script(),
            "document.execCommand(\"bold\", false, null);"
        );
        assert_eq!(
            EditorAction::Exec(EditorCommand::Heading(9)).script(),
            "document.execCommand(\"formatBlock\", false, \"\\u003ch6>\");"
        );
        assert_eq!(
            EditorAction::Link(Rc::from("https://a.test/?q=\"x\"")).script(),
            "document.execCommand(\"createLink\", false, \"https://a.test/?q=\\\"x\\\"\");"
        );
        assert_eq!(EditorAction::Link(Rc::from("JavaScript:alert(1)")).script(), "");
        assert_eq!(
            EditorAction::InsertHtml(Rc::from("<i>x</i>\n</script>")).script(),
            "document.execCommand(\"insertHTML\", false, \
             \"\\u003ci>x\\u003c/i>\\n\\u003c/script>\");"
        );
        assert_eq!(
            EditorAction::SetHtml(Rc::from("a\u{2028}b\u{1}c")).script(),
            "document.body.innerHTML = \"a\\u2028b\\u0001c\";"
        );
        assert_eq!(EditorAction::Focus.script(), "document.body.focus();");
    }

    /// The editor's two lines decode, the paste's halves with their
    /// tabs and backslashes intact, and a stranger's line is dropped.
    #[test]
    fn an_editor_report_decodes_its_two_lines() {
        assert_eq!(
            editor_report("change\t<p>a\tb</p>"),
            Some(EditorReport::Changed(String::from("<p>a\tb</p>")))
        );
        assert_eq!(
            editor_report("paste\t<b>x\\ty</b>\\\\\tplain\\tone\\\\two"),
            Some(EditorReport::Pasted {
                html: String::from("<b>x\ty</b>\\"),
                text: String::from("plain\tone\\two"),
            })
        );
        assert_eq!(
            editor_report("paste\t\t"),
            Some(EditorReport::Pasted { html: String::new(), text: String::new() })
        );
        assert_eq!(editor_report("paste\tno second half"), None);
        assert_eq!(editor_report("noise"), None);
        // the script escapes exactly what the decoder undoes
        assert!(EDITOR_SCRIPT.contains("replace(/\\\\/g, '\\\\\\\\').replace(/\\t/g, '\\\\t')"));
        assert!(EDITOR_SCRIPT.contains("send('change\\t' + document.body.innerHTML)"));
    }
}
