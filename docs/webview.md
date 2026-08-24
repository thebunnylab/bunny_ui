# A webview

*Status: the native host, the macOS webview and its instrumentation
floor — user scripts, the message bus, navigation reports, eval with
the value coming back — are standing;
`cargo run -p bunny-ui-macos --example browser_window` is the proof.
Snapshot, devtools-by-handle, WebView2, WebKitGTK and the web's
iframe are open.*

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
clips it to the box, shows and hides it with the subtree — and paints
**around** it, never over it.

That last clause is the contract, and it is stated here rather than
discovered by the first popover: a native child view composites above
the scene. Nothing the framework draws can appear on top of it — not a
tooltip, not a menu, not a drag ghost. An overlay that would cross the
island repositions, or becomes a native view itself. The host reports
its rectangle so the app can make that call deliberately.

This is a different door from `canvas` and `custom`, which paint with
the framework's own commands and clip like everything else. The host
is for content that arrives with its own renderer: a webview first,
and whatever else the platform draws that we do not.

In the web lowering the "native view" is an `iframe`, and the island
contract is the one the DOM already enforces.

## The widget

Built on the host. The declarative half is a view like any other:

```rust
webview("https://docs.example.dev")
    .user_script(src)   // document-start, every navigation
    .on_navigate(move |url| history.update(|h| h.push(url.into())))
    .on_message(move |body| bus.send(body))   // window.bunny.post(…) in the page
```

The imperative half is a handle, because a page is a document with a
navigation stack, not a view tree, and a declarative API pretending
otherwise would be a fiction over `goBack()`:

```rust
let page = WebviewHandle::new();   // bound with .handle(&page)
page.navigate(url);
page.back();
page.eval("document.title", move |answer| { /* the value, serialized */ });
```

`eval` takes an expression and answers on a later frame: `Ok` is the
value as JSON, and a page that threw answers `Err` with the error's
name — never silence that looks like a slow page. A snapshot (the page
as an image) and opening the engine's own inspector are the handle's
open seats.

User scripts run at document start, so a page never renders before the
app's instrumentation is in place. The message bus is the same channel
discipline `.task` already uses: the page posts, the app receives —
and everything the page sends back rides that one channel, the eval
answers included.

## Capabilities

The three engines do not offer the same instrumentation, and the API
does not pretend they do. A backend declares what it serves; asking
for more is an error with a name, never an empty answer that looks
like a quiet page.

| | WKWebView | WebView2 | WebKitGTK |
| -- | -- | -- | -- |
| console messages | injected hook | native | native |
| network: requests observed | injected wrap (fetch/XHR) | native | native |
| network: response bodies | no | yes | yes |
| synthetic input | open question | native | open question |
| devtools | external inspector | built in | embeddable |

The table is the design's honest centre. An app that needs full
network capture on every OS needs a proxy or its own engine; this
widget is for showing the web and observing what an app's own pages
do — a dev tool watching the server it just started, a test harness
reading the console of the page it drives.

The declared set is a value the app can read, so a feature that needs
a capability can decide its own shape per platform instead of
half-working on two of three.

## What this is not

Not a way to write app UI in HTML — views are views, and a rounded
corner or a hover state belongs to the framework. Not an Electron: the
app owns the window, the scene and the event loop, and the webview is
one box in it. Not a DOM binding: the page is on the other side of a
message channel, and it stays there.

## Order of work

The macOS backend first, because WKWebView is the floor — the weakest
network story, the open input question — and an API grown on the
richest backend would be wrong on the other two. The capability table
above is the acceptance sheet: each cell either works or refuses by
name.
