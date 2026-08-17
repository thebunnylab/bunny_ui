//! View modifiers — the closed enum in place of the engine's `Rc<dyn>`.
//!
//! `Modifier` is our own enum (the set is closed by construction), so
//! `.font(.title)` allocates nothing: the variant lives inline in
//! [`Modified<C>`] and dispatch is a jump table. The strings of the
//! printed `[.…]` are computed at render, from the variant.
//!
//! The behaviors (`onAppear`, effects, `sheet`, `EnvSet`, `Custom`) carry
//! `Rc<dyn Fn>` closures — they are the declared borders of dynamism: the
//! effect queue and sheet content are heterogeneous by nature.
//!
//! `Modified<C>` demands `C: View<Arity = Single>`: decorating a raw tuple
//! (`(a, b).padding()`) would apply the modifier only to the last node — so
//! it does not compile. To decorate a group, use [`tuple`], which has its own node.
//!
//! [`tuple`]: crate::views::tuple

use std::rc::Rc;

use motor::state::{Binding, Context, EffectFn, EnvironmentValues};
use motor::view::RenderNode;

use crate::erased::CustomModifier;
use crate::layout::{Color, CrossAlign, Edges, LayoutNode, TextHighlight, Truncation, VisualProps};
use crate::text_engine::{FontDesign, FontPatch, FontSpec, Weight};
use crate::state_ext::BindingExt;
use crate::view::{NodeList, Single, View};
use crate::views::{Alignment, wrap_layout};
use motor::views::{ContentMode, Edge, Font, ListStyle, ProgressViewStyle, TextAlignment};

/// Every modifier in the UI layer, as data.
#[derive(Clone)]
pub enum Modifier {
    // MARK: - Formatting (inert)
    Font(Font),
    Bold,
    Padding,
    PaddingLength(f64),
    PaddingEdge(Edge, f64),
    FrameWH(f64, f64),
    FrameMax(f64, f64, Alignment),
    NavigationTitle(String),
    NavigationBarTitle(String),
    ListStyle(ListStyle),
    ProgressViewStyle(ProgressViewStyle),
    NavigationViewStyle,
    MultilineTextAlignment(TextAlignment),
    AspectRatio(ContentMode),
    Resizable,
    Blur(f64),
    IgnoresSafeArea,
    Background(String),
    Hidden,
    Equatable,

    // MARK: - Visuals (pure data → `Styled` in the scene)
    BackgroundColor(Color),
    ForegroundColor(Color),
    Border(Color, f64),
    CornerRadius(f64),
    Monospaced,
    BackgroundHovered(Color),
    BackgroundPressed(Color),
    Highlight(Rc<Vec<(usize, usize)>>, Color),
    TruncationMode(Truncation),
    /// The scroll region follows this item id when it changes.
    ScrollTarget(String),
    /// The field asks for focus on its first appearance.
    AutoFocus,

    // MARK: - Real interaction (a pointer target without chrome — the Button
    // without the outfit; the action fires on up-inside like the Button's)
    OnClick(Rc<dyn Fn()>),
    OnAction(crate::action::ActionId, Rc<dyn Fn()>),

    // MARK: - Interaction (the action fires at render, as in the headless engine)
    OnAppear(Rc<dyn Fn()>),
    OnTapGesture(Rc<dyn Fn()>),

    // MARK: - Effects (onChange / onReceive / query — drained by the pump)
    Effect {
        name: &'static str,
        detail: &'static str,
        effect: EffectFn,
    },

    // MARK: - Dynamic borders
    Sheet {
        is_presented: Binding<bool>,
        content: Rc<dyn Fn(&Context) -> crate::erased::Erased>,
    },
    EnvSet {
        name: &'static str,
        detail: String,
        set: Rc<dyn Fn(&mut EnvironmentValues)>,
    },
    Custom(Rc<dyn CustomModifier>),

    // MARK: - Inert parity
    Searchable,
    Refreshable,
    Toolbar,
    AttachEnvironmentOverrides,
    AttachEnvironmentOverridesOnChange,
    FlipsForRightToLeftLayoutDirection(bool),
    NavigationDestination,
}

impl Modifier {
    /// The `[.name(detail)]` appended to the rendered node's line.
    fn suffix(&self) -> String {
        match self {
            Modifier::Font(font) => format!(" [.font({font})]"),
            Modifier::Bold => " [.bold()]".into(),
            Modifier::Padding => " [.padding()]".into(),
            Modifier::PaddingLength(length) => format!(" [.padding({length})]"),
            Modifier::PaddingEdge(edge, length) => format!(" [.padding({edge}, {length})]"),
            Modifier::FrameWH(width, height) => {
                format!(" [.frame(width: {width}, height: {height})]")
            }
            Modifier::FrameMax(max_width, max_height, alignment) => format!(
                " [.frame(maxWidth: {max_width:?}, maxHeight: {max_height}, alignment: {alignment})]"
            ),
            Modifier::NavigationTitle(title) => format!(" [.navigationTitle({title:?})]"),
            Modifier::NavigationBarTitle(title) => format!(" [.navigationBarTitle({title:?})]"),
            Modifier::ListStyle(style) => format!(" [.listStyle({style})]"),
            Modifier::ProgressViewStyle(style) => format!(" [.progressViewStyle({style})]"),
            Modifier::NavigationViewStyle => " [.navigationViewStyle(stack)]".into(),
            Modifier::MultilineTextAlignment(alignment) => {
                format!(" [.multilineTextAlignment({alignment})]")
            }
            Modifier::AspectRatio(content_mode) => {
                format!(" [.aspectRatio(contentMode: .{content_mode:?})]")
            }
            Modifier::Resizable => " [.resizable()]".into(),
            Modifier::Blur(radius) => format!(" [.blur(radius: {radius})]"),
            Modifier::IgnoresSafeArea => " [.ignoresSafeArea()]".into(),
            Modifier::Background(line) => format!(" [.background {{ {line} }}]"),
            Modifier::Hidden => " [.hidden()]".into(),
            Modifier::Equatable => " [.equatable()]".into(),
            Modifier::BackgroundColor(color) => format!(" [.background({color})]"),
            Modifier::ForegroundColor(color) => format!(" [.foregroundColor({color})]"),
            Modifier::Border(color, width) => format!(" [.border({color}, width: {width})]"),
            Modifier::CornerRadius(radius) => format!(" [.cornerRadius({radius})]"),
            Modifier::Monospaced => " [.monospaced()]".into(),
            Modifier::BackgroundHovered(color) => format!(" [.backgroundHovered({color})]"),
            Modifier::BackgroundPressed(color) => format!(" [.backgroundPressed({color})]"),
            Modifier::Highlight(ranges, color) => {
                format!(" [.highlight({} ranges, {color})]", ranges.len())
            }
            Modifier::TruncationMode(mode) => format!(" [.truncationMode(.{mode:?})]"),
            Modifier::ScrollTarget(id) => format!(" [.scrollTarget({id:?})]"),
            Modifier::AutoFocus => " [.autoFocus()]".into(),
            Modifier::OnClick(_) => " [.onClick()]".into(),
            Modifier::OnAction(id, _) => format!(" [.onAction({id})]"),
            Modifier::OnAppear(_) => " [.onAppear()]".into(),
            Modifier::OnTapGesture(_) => " [.onTapGesture()]".into(),
            Modifier::Effect { name, detail, .. } => format!(" [.{name}{detail}]"),
            Modifier::Sheet { .. } => " [.sheet(isPresented: $…)]".into(),
            Modifier::EnvSet { name, detail, .. } => format!(" [.{name}{detail}]"),
            Modifier::Custom(custom) => format!(" [.modifier({})]", custom.name()),
            Modifier::Searchable => " [.searchable(text: $…)]".into(),
            Modifier::Refreshable => " [.refreshable { … }]".into(),
            Modifier::Toolbar => " [.toolbar { … }]".into(),
            Modifier::AttachEnvironmentOverrides => " [.attachEnvironmentOverrides()]".into(),
            Modifier::AttachEnvironmentOverridesOnChange => {
                " [.attachEnvironmentOverrides(onChange: …)]".into()
            }
            Modifier::FlipsForRightToLeftLayoutDirection(flips) => {
                format!(" [.flipsForRightToLeftLayoutDirection({flips})]")
            }
            Modifier::NavigationDestination => " [.navigationDestination(for: …)]".into(),
        }
    }
}

/// Merge rule for the visual modifiers: styles stacked on the SAME view
/// fuse into a single `Styled` — on a conflicting field, the modifier
/// NEAREST the view wins; distinct fields accumulate
/// (`.background_color(a).corner_radius(r)` = ONE node, and the radius
/// rounds THIS background). Veil over veil on the same view does not compose
/// — layers belong to `zstack`. The merge only happens with a literal
/// `Styled` on top: `.background_color(a).padding().background_color(b)`
/// truly nests (different geometries, both paint).
/// Rewrites the TEXT node under the `Styled` chain (if any) — the
/// `.highlight()`/`.truncationMode()` path, immune to the order of the
/// visual modifiers in the chain.
/// Descends through wrappers to the `Scroll` node and rewrites it —
/// modifier order stays irrelevant (`.scroll_target()` works before or
/// after visual modifiers). Without a scroll region it is a no-op.
fn rewrite_scroll_node(
    node: LayoutNode,
    rewrite: &impl Fn(Option<String>, Box<LayoutNode>) -> LayoutNode,
) -> LayoutNode {
    match node {
        LayoutNode::Scroll { path, child, .. } => rewrite(path, child),
        LayoutNode::Styled { props, child } => LayoutNode::Styled {
            props,
            child: Box::new(rewrite_scroll_node(*child, rewrite)),
        },
        LayoutNode::Padding { edges, child } => LayoutNode::Padding {
            edges,
            child: Box::new(rewrite_scroll_node(*child, rewrite)),
        },
        LayoutNode::Frame { width, height, child } => LayoutNode::Frame {
            width,
            height,
            child: Box::new(rewrite_scroll_node(*child, rewrite)),
        },
        LayoutNode::MaxFrame { max_width, max_height, align, child } => LayoutNode::MaxFrame {
            max_width,
            max_height,
            align,
            child: Box::new(rewrite_scroll_node(*child, rewrite)),
        },
        other => other,
    }
}

/// Descends through wrappers to the `Field` node and rewrites it —
/// same order-immunity as the text and scroll rewrites.
fn rewrite_field_node(
    node: LayoutNode,
    rewrite: &impl Fn(String, std::rc::Rc<str>, std::rc::Rc<str>) -> LayoutNode,
) -> LayoutNode {
    match node {
        LayoutNode::Field { path, content, placeholder, .. } => {
            rewrite(path, content, placeholder)
        }
        LayoutNode::Styled { props, child } => LayoutNode::Styled {
            props,
            child: Box::new(rewrite_field_node(*child, rewrite)),
        },
        LayoutNode::Padding { edges, child } => LayoutNode::Padding {
            edges,
            child: Box::new(rewrite_field_node(*child, rewrite)),
        },
        LayoutNode::Frame { width, height, child } => LayoutNode::Frame {
            width,
            height,
            child: Box::new(rewrite_field_node(*child, rewrite)),
        },
        LayoutNode::MaxFrame { max_width, max_height, align, child } => LayoutNode::MaxFrame {
            max_width,
            max_height,
            align,
            child: Box::new(rewrite_field_node(*child, rewrite)),
        },
        other => other,
    }
}

fn rewrite_text_node(
    node: LayoutNode,
    rewrite: &impl Fn(
        std::rc::Rc<str>,
        Option<TextHighlight>,
        Option<Truncation>,
    ) -> LayoutNode,
) -> LayoutNode {
    match node {
        LayoutNode::Text { content, highlights, truncation } => {
            rewrite(content, highlights, truncation)
        }
        LayoutNode::Styled { props, child } => LayoutNode::Styled {
            props,
            child: Box::new(rewrite_text_node(*child, rewrite)),
        },
        other => other,
    }
}

fn wrap_styled(out: &mut NodeList, delta: VisualProps) {
    out.wrap_last_layout(|node| match node {
        LayoutNode::Styled { props, child } => {
            LayoutNode::Styled { props: props.or(delta), child }
        }
        other => LayoutNode::Styled { props: delta, child: Box::new(other) },
    });
}

/// The modified view — Swift's `ModifiedContent` with the modifier inline.
#[derive(Clone)]
pub struct Modified<C> {
    pub(crate) base: C,
    pub(crate) modifier: Modifier,
}

impl<C: View<Arity = Single>> View for Modified<C> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        // `.modifier(…)` re-renders through the custom modifier's own
        // body(content:), and marks the node — the recomputable blur path.
        if let Modifier::Custom(custom) = &self.modifier {
            let applied = custom.apply(ctx, crate::erased::erased(self.base.clone()));
            let mut nodes = NodeList::new();
            applied.render_into(ctx, &mut nodes);
            if let Some(node) = nodes.last_mut() {
                node.line
                    .push_str(&format!(" [.modifier({})]", custom.name()));
            }
            out.extend(nodes);
            return;
        }

        // `.inject()` / `.modelContainer()` water the subtree.
        let mut base_ctx = ctx.clone();
        if let Modifier::EnvSet { set, .. } = &self.modifier {
            set(&mut base_ctx.values);
        }

        self.base.render_into(&base_ctx, out);

        match &self.modifier {
            Modifier::OnAppear(action) | Modifier::OnTapGesture(action) => action(),
            Modifier::Effect { effect, .. } => {
                // The effect sees the subtree's environment — the pump only
                // has the root ctx at hand.
                let effect = effect.clone();
                let subtree_ctx = base_ctx.clone();
                crate::effects::push(Rc::new(move |_: &Context| effect(&subtree_ctx)));
            }
            Modifier::Sheet {
                is_presented,
                content,
            } if is_presented.get() => {
                let mut sheet_nodes = NodeList::new();
                {
                    // The sheet is a sub-root with its own identity: what the
                    // closure builds anchors here and dies when it closes.
                    let _frame = motor::identity::enter("sheet");
                    content(ctx).render_into(ctx, &mut sheet_nodes);
                }
                let (sheet_prints, sheet_layouts) = sheet_nodes.into_parts();
                if let Some(node) = out.last_mut() {
                    node.children
                        .push(RenderNode::branch("Sheet", sheet_prints));
                }
                // in layout, the sheet overlays the base
                out.wrap_last_layout(|base| LayoutNode::Layered {
                    children: vec![base, wrap_layout(sheet_layouts)],
                });
            }
            _ => {}
        }

        // LAYOUT modifiers wrap the base's node — this is where the typed
        // chain becomes proposal/response structure
        match &self.modifier {
            Modifier::Padding => out.wrap_last_layout(|node| LayoutNode::Padding {
                edges: Edges::uniform(16.0),
                child: Box::new(node),
            }),
            Modifier::PaddingLength(length) => {
                let edges = Edges::uniform(*length);
                out.wrap_last_layout(|node| LayoutNode::Padding {
                    edges,
                    child: Box::new(node),
                });
            }
            Modifier::PaddingEdge(edge, length) => {
                let mut edges = Edges::default();
                match edge {
                    Edge::Top => edges.top = *length,
                    Edge::Bottom => edges.bottom = *length,
                    Edge::Leading => edges.leading = *length,
                    Edge::Trailing => edges.trailing = *length,
                }
                out.wrap_last_layout(|node| LayoutNode::Padding {
                    edges,
                    child: Box::new(node),
                });
            }
            Modifier::FrameWH(width, height) => {
                let (width, height) = (Some(*width), Some(*height));
                out.wrap_last_layout(|node| LayoutNode::Frame {
                    width,
                    height,
                    child: Box::new(node),
                });
            }
            Modifier::FrameMax(max_width, max_height, alignment) => {
                let (max_width, max_height) = (*max_width, *max_height);
                let align = match alignment {
                    Alignment::Leading => CrossAlign::Start,
                    Alignment::Center => CrossAlign::Center,
                    Alignment::Trailing => CrossAlign::End,
                };
                out.wrap_last_layout(|node| LayoutNode::MaxFrame {
                    max_width,
                    max_height,
                    align,
                    child: Box::new(node),
                });
            }
            Modifier::BackgroundColor(color) => wrap_styled(
                out,
                VisualProps { background: Some(*color), ..VisualProps::default() },
            ),
            Modifier::ForegroundColor(color) => wrap_styled(
                out,
                VisualProps { foreground: Some(*color), ..VisualProps::default() },
            ),
            Modifier::Border(color, width) => wrap_styled(
                out,
                VisualProps { border: Some((*color, *width)), ..VisualProps::default() },
            ),
            Modifier::CornerRadius(radius) => wrap_styled(
                out,
                VisualProps { corner_radius: Some(*radius), ..VisualProps::default() },
            ),
            // font is an inherited scene property — the same Styled as the
            // visuals carries the patch (measure applies it on top of the env)
            Modifier::Font(font) => wrap_styled(
                out,
                VisualProps {
                    font: FontPatch::full(FontSpec::resolve(*font)),
                    ..VisualProps::default()
                },
            ),
            Modifier::Bold => wrap_styled(
                out,
                VisualProps {
                    font: FontPatch { weight: Some(Weight::Bold), ..FontPatch::default() },
                    ..VisualProps::default()
                },
            ),
            Modifier::Monospaced => wrap_styled(
                out,
                VisualProps {
                    font: FontPatch { design: Some(FontDesign::Mono), ..FontPatch::default() },
                    ..VisualProps::default()
                },
            ),
            Modifier::BackgroundHovered(color) => wrap_styled(
                out,
                VisualProps { background_hovered: Some(*color), ..VisualProps::default() },
            ),
            Modifier::BackgroundPressed(color) => wrap_styled(
                out,
                VisualProps { background_pressed: Some(*color), ..VisualProps::default() },
            ),
            // the two below rewrite the TEXT NODE, descending through
            // `Styled` (`.font()`/`.foreground_color()` before or after, the
            // order does not matter) — on non-text they are no-ops on purpose
            // (SwiftUI parity: truncationMode outside text does nothing)
            Modifier::Highlight(ranges, color) => out.wrap_last_layout(|node| {
                rewrite_text_node(node, &|content, _, truncation| LayoutNode::Text {
                    content,
                    highlights: Some(TextHighlight { ranges: ranges.clone(), color: *color }),
                    truncation,
                })
            }),
            Modifier::TruncationMode(mode) => out.wrap_last_layout(|node| {
                rewrite_text_node(node, &|content, highlights, _| LayoutNode::Text {
                    content,
                    highlights,
                    truncation: Some(*mode),
                })
            }),
            Modifier::ScrollTarget(id) => out.wrap_last_layout(|node| {
                rewrite_scroll_node(node, &|path, child| LayoutNode::Scroll {
                    path,
                    target: Some(id.clone()),
                    child,
                })
            }),
            Modifier::AutoFocus => out.wrap_last_layout(|node| {
                rewrite_field_node(node, &|path, content, placeholder| LayoutNode::Field {
                    path,
                    content,
                    placeholder,
                    auto_focus: true,
                })
            }),
            Modifier::OnClick(action) => {
                // the same registration as the Button: action retained in the
                // reconciler, frame in the hit-test under the cursor identity
                if let Some(path) = motor::identity::cursor_scope() {
                    crate::reconciler::attribute_action(path.clone(), action.clone());
                    out.wrap_last_layout(|node| LayoutNode::Interactive {
                        path,
                        child: Box::new(node),
                    });
                }
            }
            Modifier::OnAction(id, handler) => {
                // pure registration: no pointer target, no layout node
                // — the action arrives by dispatch (keyboard), not hit-test
                if let Some(path) = motor::identity::cursor_scope() {
                    crate::reconciler::attribute_handler(path, *id, handler.clone());
                }
            }
            _ => {}
        }

        if let Some(node) = out.last_mut() {
            // frame does not print: the suffix is not even formatted
            if crate::view::print_enabled() {
                node.line.push_str(&self.modifier.suffix());
            }
        }
    }
}
