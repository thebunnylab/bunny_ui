//! `View` / `Component` / `AnyView` / `RenderNode` — the fake view tree.

use crate::state::Context;
use std::rc::Rc;

/// A node in the fake render tree (what a real SwiftUI would rasterize).
#[derive(Debug, Clone)]
pub struct RenderNode {
    pub line: String,
    pub children: Vec<RenderNode>,
}

impl RenderNode {
    pub fn leaf(line: impl Into<String>) -> Self {
        RenderNode { line: line.into(), children: Vec::new() }
    }

    pub fn branch(line: impl Into<String>, children: Vec<RenderNode>) -> Self {
        RenderNode { line: line.into(), children }
    }

    /// Pretty-prints the tree with box-drawing connectors.
    pub fn print(&self) -> String {
        let mut out = String::new();
        self.write_into(&mut out, "", "");
        out
    }

    fn write_into(&self, out: &mut String, prefix: &str, children_prefix: &str) {
        if self.line.is_empty() && self.children.is_empty() {
            return; // EmptyView / Optional(nil) render nothing
        }
        out.push_str(prefix);
        out.push_str(&self.line);
        out.push('\n');
        let n = self.children.len();
        for (i, child) in self.children.iter().enumerate() {
            let last = i + 1 == n;
            let connector = if last { "└─ " } else { "├─ " };
            let extension = if last { "   " } else { "│  " };
            child.write_into(
                out,
                &format!("{children_prefix}{connector}"),
                &format!("{children_prefix}{extension}"),
            );
        }
    }
}

/// The conformance the mirrored port writes for every `struct X: View`.
///
/// (`var body: some View` → `fn body(&self, ctx: &Context) -> AnyView`)
pub trait Component {
    fn body(&self, ctx: &Context) -> AnyView;
}

/// Internal object-safe equivalent of `View`.
pub trait ViewDyn {
    fn render_dyn(&self, ctx: &Context) -> RenderNode;
}

/// SwiftUI's `View`: a renderable blueprint. Built-ins implement `render`
/// directly; user views get it for free from [`Component::body`].
pub trait View: Clone + 'static {
    fn render(&self, ctx: &Context) -> RenderNode;
}

impl<T: Component + Clone + 'static> View for T {
    fn render(&self, ctx: &Context) -> RenderNode {
        RenderNode::branch(short_type_name::<T>(), vec![self.body(ctx).render(ctx)])
    }
}

impl<T: View> ViewDyn for T {
    fn render_dyn(&self, ctx: &Context) -> RenderNode {
        self.render(ctx)
    }
}

/// SwiftUI's optional-view handling: `country.flag.map { … }` renders the
/// content when the optional has a value, nothing when it doesn't.
impl<V: View> View for Option<V> {
    fn render(&self, ctx: &Context) -> RenderNode {
        match self {
            Some(view) => view.render(ctx),
            None => RenderNode::leaf(""),
        }
    }
}

/// SwiftUI's `AnyView` — type erasure over the fake view tree.
#[derive(Clone)]
pub struct AnyView {
    pub(crate) inner: Rc<dyn ViewDyn>,
}

impl AnyView {
    pub fn new<V: View>(view: V) -> Self {
        AnyView { inner: Rc::new(view) }
    }
}

impl View for AnyView {
    fn render(&self, ctx: &Context) -> RenderNode {
        self.inner.render_dyn(ctx)
    }
}

pub(crate) fn short_type_name<T: ?Sized>() -> String {
    let full = std::any::type_name::<T>();
    full.rsplit("::").next().unwrap_or(full).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Context;
    use crate::views::*;

    #[test]
    fn renders_a_nested_tree() {
        use crate::modifiers::ViewExt;
        struct CountryCell;

        impl Component for CountryCell {
            fn body(&self, _ctx: &Context) -> AnyView {
                VStack(Alignment::Leading, vec![
                    Text("United States").font(Font::Title),
                    Text("Population 125000000").font(Font::Caption),
                ])
                .padding()
            }
        }

        impl Clone for CountryCell {
            fn clone(&self) -> Self {
                CountryCell
            }
        }

        let ctx = Context::default();
        let printed = CountryCell.render(&ctx).print();
        assert!(printed.contains("CountryCell"));
        assert!(printed.contains("VStack"));
        assert!(printed.contains("Text(\"United States\") [.font(.title)]"));
        assert!(printed.contains("VStack (alignment: .leading) [.padding()]"));
        assert!(printed.contains("└─"));
    }
}
