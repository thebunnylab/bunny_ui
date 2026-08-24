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
    /// The OS webview, showing `url`.
    Webview { url: Rc<str> },
}

/// The view a webview enters the scene as — see [`webview`].
#[derive(Clone)]
pub struct WebviewView {
    url: Rc<str>,
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
        out.push_layout(LayoutNode::Host {
            path,
            spec: HostSpec::Webview { url: self.url.clone() },
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
/// ```
pub fn webview(url: impl Into<Rc<str>>) -> WebviewView {
    WebviewView { url: url.into() }
}
