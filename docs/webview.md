# A webview

*Status: the native host, the macOS webview and its instrumentation
floor — user scripts, the message bus, navigation reports (both legs:
the commit and the refusal), eval with the value coming back, console
and request capture by injected hook, the snapshot, and synthetic
input as real NSEvents the page trusts — are standing;
`cargo run -p bunny-ui-macos --example browser_window` is the proof,
the web's own lowering — the iframe — is standing beside it, and the
scene interleaves with the island: what paints after a host
composites above it. WebView2 stands too, the same floor behind the
same API — the Evergreen runtime found by registry at zero bundled
bytes, console and requests served NATIVELY (the DevTools Protocol
and the response-received event, richer than the injected wraps),
synthetic input by protocol with `isTrusted` true, and the sandwich
riding owned per-pixel-alpha popups;
`cargo run -p bunny-ui-windows --example browser_window` is that
proof, `--drive` its witness. A document from MEMORY under a network
policy — `webview_html`, the reader of a letter from a stranger —
stands on both engines and in the web lowering;
`cargo run -p bunny-ui-macos --example letter_window -- --drive` is
its proof, measured against a witness on the loopback. WebKitGTK is
open.*

An app sometimes has to show a web page — the preview of the thing it
is building, a documentation site, an OAuth dance. Bundling a browser
engine to do it costs a few hundred megabytes and a CVE stream that
never ends. Meanwhile every OS this framework targets already ships an
engine: WKWebView on macOS, WebView2 on Windows, WebKitGTK on Linux.
The framework should be able to hold that engine the way it holds any
other view — at zero bytes of bundled browser.

Two pieces, and the first is more general than the second.

## The native host

A box owned by a platform view. The framework positions it, sizes it,
clips it to the box, shows and hides it with the subtree — and keeps
**paint order the truth**, island or no island.

The platform's law is that a native child view composites above the
scene, and the framework answers it on two roads rather than letting
the law leak into the app. Whatever the scene paints AFTER the host —
a toast in the corner, a veil, anything a z-stack puts over the pane —
leaves the window's own present and composites on a **segment
surface** above the platform view: its painted pixels claim the
pointer, its clear ones let the click fall through to the page. An
overlay — a popover, a sheet — presents on a surface of its own, the
road it already travels. A scene with nothing painted after its hosts
mints no segment and pays nothing.

One thing stays honestly out of reach: the page's pixels are the
engine's, so a material above the island tints — it does not blur.
The host reports its rectangle for the calls that remain deliberate.

This is a different door from `canvas` and `custom`, which paint with
the framework's own commands and clip like everything else. The host
is for content that arrives with its own renderer: a webview first,
and whatever else the platform draws that we do not.

In the web lowering the "native view" is an `iframe`, and the island
contract is the one the DOM already enforces. This half is built: the
host lowers to an `iframe` that grows and stretches like the filler
it is, and a changed url is ONE patch — the element (and the page's
state in it) survives the navigation. The instrumentation channels do
not cross this border: a cross-origin page is the browser's island,
sealed by the browser's own rules.

## The widget

Built on the host. The declarative half is a view like any other:

```rust
webview("https://docs.example.dev")
    .user_script(src)   // document-start, every navigation
    .full_motion()      // the page sees prefers-reduced-motion: no-preference
    .on_navigate(move |url| history.update(|h| h.push(url.into())))
    .on_navigate_failed(move |url, why| status.set(format!("{url} — {why}")))
    .on_message(move |body| bus.send(body))   // window.bunny.post(…) in the page
    .on_console(move |line| log.push(line))   // "level: what it said"
    .on_request(move |line| net.push(line))   // "METHOD url status"
```

The two navigation hooks are ONE pair, and every load answers in
exactly one of them: an app that waits for the commit before it calls
a page ready waits for ever on a dead host, a bad certificate or a
server that is down. A load another navigation replaced is not a
failure — the one that replaced it reports for both.

The two hooks are capability-gated (the table below), and a backend
that serves them by injection only pays when they are declared —
nothing is captured for a page nobody watches.

`.full_motion()` is for the TESTING surface: an embedded browser whose
job is to show a site as most visitors meet it must not inherit the
tester's OS accessibility settings — a developer who keeps reduced
motion on their machine still needs to see the product move. Off (the
default), the OS preference passes through, like any browser's.
Capability-gated as `MediaEmulation`; where the backend cannot serve
it, the OS stays the only truth the engine will tell.

The imperative half is a handle, because a page is a document with a
navigation stack, not a view tree, and a declarative API pretending
otherwise would be a fiction over `goBack()`:

```rust
let page = WebviewHandle::new();   // bound with .handle(&page)
page.navigate(url);
page.back();
page.eval("document.title", move |answer| { /* the value, serialized */ });
page.snapshot(move |answer| { /* the page as straight RGBA */ });

page.click(120.0, 48.0);          // CSS px, from the view's top-left
page.type_text("a query");        // into whatever the page has focused
page.key("Enter");
page.hover(x, y);
page.scroll(x, y, 0.0, 240.0);    // the page's own signs: dy counts down
page.input(WebviewInput::Down {   // and the whole vocabulary, for a drag
    x, y, button: MouseButton::Right, clicks: 1, modifiers: Modifiers::NONE,
});
```

`eval` takes an expression and answers on a later frame: `Ok` is the
value as JSON, and a page that threw answers `Err` with the error's
name — never silence that looks like a slow page. `snapshot` answers
the same way, with the pixels the engine shows right now. Opening the
engine's own inspector stays the OS's door (on macOS the view is
inspectable: right-click, Inspect Element).

The input door is the hand an app lends the page. Its vocabulary has
two halves: `click`, `hover`, `scroll`, `type_text` and `key` are what
a tool does — each one complete in itself — while `Down`, `Drag` and
`Up` are what a hand does, a button that stays held between events,
which is the only way to spell a drag or a selection. Fire and
forget: a hand gets no receipt.

Where the backend serves it by REAL platform events — on macOS,
NSEvents addressed at the view — the page cannot tell the app's hand
from the one on the desk: `isTrusted` is true, and a control that
guards on it works. That is the whole reason the door exists, because
the alternative an app reaches for is a synthetic DOM event
(`el.click()`), which real sites refuse. Typing is a COMMIT rather
than a run of keystrokes, so it lands in a field, in a
`contenteditable`, and under a keyboard layout nobody guessed.

Two truths a driver has to know. A right press opens the PAGE's own
menu, and a menu takes the machine until a person closes it — so it
is the last event of a sequence, never the middle one. And what the
engine does not consume walks back up the responder chain: while the
app is lending the hand the scene's ears are closed, but a key the
page declines can still reach the app afterwards, exactly as it does
when a person types into a focused page.

User scripts run at document start, so a page never renders before the
app's instrumentation is in place. The message bus is the same channel
discipline `.task` already uses: the page posts, the app receives —
and everything the page sends back rides that one channel, the eval
answers included.

## A document from memory

The same box shows a page the app already HOLDS — a letter, a
rendered preview — from memory, with no file written and no url
fetched:

```rust
webview_html(&letter, "https://mail.example/", NetworkPolicy::Deny)
    .on_link(move |url| open_in_browser(url))
```

Three things the url door does not promise, and this one does.

The document is under a **network policy** the engine enforces on
every fetch, before a byte leaves the machine. `Deny` lets nothing
out — no image, stylesheet, font, frame, media or script of the
document's own — while what it carries inline (a `data:` image, its
own styles) still shows. `RemoteImages` opens exactly the remote
images, over http and https, and nothing else: the "load remote
content" switch a mail reader flips per message or per sender. The
policy rides at the document's head as its Content-Security-Policy,
which every engine this framework hosts honours, ahead of anything
the document brought; a policy the document carries of its own can
only tighten it. A document runs no script of its own and sends no
form. The app's user scripts, injected by the engine, still run — and
so do eval, the bus, the snapshot and the hand.

The document **never moves**. A link the person activates — a
`target="_blank"` one included — is cancelled and reported through
`on_link` with its url, and the app decides; a refresh the document
wrote, a form, a subframe, the engine's own reload (which would fetch
the base) are cancelled without a word. `base` is where the
document's relative references resolve; `on_navigate` reports it
once, at the commit. A body that runs again with the same letter
never reloads it — the spec carries a fingerprint, not a comparison
of pages — and a changed letter always does.

One thing stays honestly outside. The policy governs fetches; a hint
that is not one — a DNS prefetch — is the sanitizer's to strip before
the letter arrives here. A reader shows a stranger's html sanitized,
and the policy is the belt under it.

On macOS the document loads by `loadHTMLString:baseURL:`, and the
navigation delegate answers the engine's every ask with the rule
above. On Windows it loads by `NavigateToString` — a two-megabyte
door; a larger letter is refused by name on `on_navigate_failed` —
with the base sealed into its head, the starting leg cancelling what
the rule forbids and the new-window ask handled. In the web lowering
the frame holds the document as `srcdoc` inside a sandbox with no
powers, where a link is inert: the web leg has no road to hand it
back, and says so here rather than opening it in the pane.

## Capabilities

The three engines do not offer the same instrumentation, and the API
does not pretend they do. A backend declares what it serves; asking
for more is an error with a name, never an empty answer that looks
like a quiet page.

| | WKWebView | WebView2 | WebKitGTK |
| -- | -- | -- | -- |
| console messages | injected hook | native | native |
| network: requests observed | injected wrap (fetch/XHR) | native | native |
| network: response bodies | no | yes¹ | yes |
| synthetic input | NSEvent, trusted | native | open question |
| media emulation (full motion) | no² | CDP `Emulation.setEmulatedMedia` | open question |
| devtools | external inspector | built in | embeddable |
| a document under a policy | `loadHTMLString`, CSP | `NavigateToString`, CSP | open |

¹ Engine-ready, core door open: the engine can hand a response body
over, but no hook of this API carries bytes yet — so the backend does
not DECLARE `NetworkBodies`, because a declared capability nothing can
ask through would be an empty answer with a checkmark on it.

² WKWebView offers no public override — a document-start `matchMedia`
shim could lie to scripts but never to a CSS `@media` block, and a
half-truth is worse than the honest cell. On the mac the OS setting is
the only lever.

The table is the design's honest centre. An app that needs full
network capture on every OS needs a proxy or its own engine; this
widget is for showing the web and observing what an app's own pages
do — a dev tool watching the server it just started, a test harness
reading the console of the page it drives.

The declared set is a value the app can read — on macOS,
`bunny_ui_macos::webview::capabilities()` — so a feature that needs a
capability can decide its own shape per platform instead of
half-working on two of three.

## What this is not

Not a way to write app UI in HTML — views are views, and a rounded
corner or a hover state belongs to the framework. Not an Electron: the
app owns the window, the scene and the event loop, and the webview is
one box in it. Not a DOM binding: the page is on the other side of a
message channel, and it stays there.

## Order of work

The macOS backend first, because WKWebView is the floor — the weakest
network story, and the one where synthetic input had no answer at
all — and an API grown on the richest backend would be wrong on the
other two. The capability table
above is the acceptance sheet: each cell either works or refuses by
name.
