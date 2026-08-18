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
    FrameWidth(f64),
    FrameHeight(f64),
    FrameMax(f64, f64, Alignment),
    NavigationTitle(String),
    NavigationBarTitle(String),
    ListStyle(ListStyle),
    ProgressViewStyle(ProgressViewStyle),
    NavigationViewStyle,
    MultilineTextAlignment(TextAlignment),
    Blur(f64),
    IgnoresSafeArea,
    Background(String),
    Hidden,
    Equatable,

    // MARK: - Visuals (pure data → `Styled` in the scene)
    BackgroundColor(Color),
    BackgroundGradient(crate::layout::Gradient),
    ForegroundColor(Color),
    Border(Color, f64),
    CornerRadius(f64),
    Monospaced,
    /// A size out of the preset scale — the rest of the font stays.
    FontSize(f64),
    BackgroundHovered(Color),
    BackgroundPressed(Color),
    ForegroundHovered(Color),
    ForegroundPressed(Color),
    Highlight(Rc<Vec<(usize, usize)>>, Color),
    TruncationMode(Truncation),
    /// The scroll region follows this item id when it changes.
    ScrollTarget(String),
    /// The field asks for focus on its first appearance.
    AutoFocus,
    /// A soft halo behind the view: (radius, color).
    Shadow(f64, Color),
    /// The image below negotiates size with the proposal.
    Resizable,
    /// How a resizable image maps into its box: contain or cover.
    AspectRatio(ContentMode),
    /// Colors under this view move through a spring when they change.
    Animated(crate::anim::Spring),
    /// Where this subtree renders when the scene lowers to elements.
    Rendering(crate::layout::Rendering),
    /// Declares a key context active while this view is mounted.
    KeyContext(&'static str),
    /// Pressing this view (where no interactive target wins) drags the
    /// WINDOW — the scene's own title bar on a chrome-less window.
    WindowDragRegion,

    // MARK: - Real interaction (a pointer target without chrome — the Button
    // without the outfit; the action fires on up-inside like the Button's)
    OnClick(Rc<dyn Fn()>),
    OnAction(crate::action::ActionId, Rc<dyn Fn()>),

    // MARK: - Interaction (the action fires at render, as in the headless engine)
    OnAppear(Rc<dyn Fn()>),
    OnTapGesture(Rc<dyn Fn()>),

    /// `.id("name")` — the view takes a NAME in the identity.
    Id(Rc<str>),

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
    /// An anchored popover: the modified view is the ANCHOR, `side`
    /// the preferred edge. Closes on Escape and on a press outside;
    /// `on_dismiss` runs after any of the framework's dismissal doors
    /// (never when the app clears the binding itself).
    Popover {
        is_presented: Binding<bool>,
        side: crate::layout::Side,
        on_dismiss: Option<Rc<dyn Fn()>>,
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
            Modifier::FrameWidth(width) => format!(" [.frame(width: {width})]"),
            Modifier::FrameHeight(height) => format!(" [.frame(height: {height})]"),
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
            Modifier::BackgroundGradient(gradient) => match gradient {
                crate::layout::Gradient::Radial { inner, outer, .. } => {
                    format!(" [.background(RadialGradient({inner} → {outer}))]")
                }
                crate::layout::Gradient::Linear { from, to, .. } => {
                    format!(" [.background(LinearGradient({from} → {to}))]")
                }
            },
            Modifier::ForegroundColor(color) => format!(" [.foregroundColor({color})]"),
            Modifier::Border(color, width) => format!(" [.border({color}, width: {width})]"),
            Modifier::CornerRadius(radius) => format!(" [.cornerRadius({radius})]"),
            Modifier::Monospaced => " [.monospaced()]".into(),
            Modifier::Id(name) => format!(" [.id({name:?})]"),
            Modifier::FontSize(size) => format!(" [.font(.system(size: {size}))]"),
            Modifier::BackgroundHovered(color) => format!(" [.backgroundHovered({color})]"),
            Modifier::ForegroundHovered(color) => format!(" [.foregroundHovered({color})]"),
            Modifier::ForegroundPressed(color) => format!(" [.foregroundPressed({color})]"),
            Modifier::BackgroundPressed(color) => format!(" [.backgroundPressed({color})]"),
            Modifier::Highlight(ranges, color) => {
                format!(" [.highlight({} ranges, {color})]", ranges.len())
            }
            Modifier::TruncationMode(mode) => format!(" [.truncationMode(.{mode:?})]"),
            Modifier::ScrollTarget(id) => format!(" [.scrollTarget({id:?})]"),
            Modifier::AutoFocus => " [.autoFocus()]".into(),
            Modifier::Shadow(radius, color) => format!(" [.shadow(radius: {radius}, {color})]"),
            Modifier::Animated(spec) => format!(
                " [.animated(response: {}, damping: {})]",
                spec.response, spec.damping
            ),
            Modifier::Rendering(mode) => format!(" [.rendering(.{mode:?})]"),
            Modifier::KeyContext(name) => format!(" [.keyContext({name})]"),
            Modifier::WindowDragRegion => " [.windowDragRegion()]".into(),
            Modifier::OnClick(_) => " [.onClick()]".into(),
            Modifier::OnAction(id, _) => format!(" [.onAction({id})]"),
            Modifier::OnAppear(_) => " [.onAppear()]".into(),
            Modifier::OnTapGesture(_) => " [.onTapGesture()]".into(),
            Modifier::Effect { name, detail, .. } => format!(" [.{name}{detail}]"),
            Modifier::Sheet { .. } => " [.sheet(isPresented: $…)]".into(),
            Modifier::Popover { side, .. } => format!(" [.popover(.{side:?})]"),
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
        LayoutNode::Animated { key, spec, child } => LayoutNode::Animated {
            key,
            spec,
            child: Box::new(rewrite_scroll_node(*child, rewrite)),
        },
        LayoutNode::Island { child } => LayoutNode::Island {
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
    rewrite: &impl Fn(String, std::sync::Arc<str>, std::sync::Arc<str>) -> LayoutNode,
) -> LayoutNode {
    match node {
        LayoutNode::Field { path, content, placeholder, .. } => {
            rewrite(path, content, placeholder)
        }
        LayoutNode::Styled { props, child } => LayoutNode::Styled {
            props,
            child: Box::new(rewrite_field_node(*child, rewrite)),
        },
        LayoutNode::Animated { key, spec, child } => LayoutNode::Animated {
            key,
            spec,
            child: Box::new(rewrite_field_node(*child, rewrite)),
        },
        LayoutNode::Island { child } => LayoutNode::Island {
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

/// Descends through wrappers to the pixel leaf — an `Image` or an
/// `Icon` — and rewrites it: `.resizable()`/`.aspect_ratio()` work
/// before or after the visual modifiers; on anything else they are
/// no-ops on purpose (SwiftUI parity, like truncationMode outside
/// text). Two closures because the two leaves carry different state.
fn rewrite_pixel_node(
    node: LayoutNode,
    rewrite: &impl Fn(
        Option<crate::image_engine::ImageSource>,
        bool,
        Option<ContentMode>,
    ) -> LayoutNode,
    icon: &impl Fn(crate::icon::Symbol, bool) -> LayoutNode,
) -> LayoutNode {
    match node {
        LayoutNode::Image { source, resizable, fit } => rewrite(source, resizable, fit),
        LayoutNode::Icon { symbol, resizable } => icon(symbol, resizable),
        LayoutNode::Styled { props, child } => LayoutNode::Styled {
            props,
            child: Box::new(rewrite_pixel_node(*child, rewrite, icon)),
        },
        LayoutNode::Animated { key, spec, child } => LayoutNode::Animated {
            key,
            spec,
            child: Box::new(rewrite_pixel_node(*child, rewrite, icon)),
        },
        LayoutNode::Island { child } => LayoutNode::Island {
            child: Box::new(rewrite_pixel_node(*child, rewrite, icon)),
        },
        LayoutNode::Padding { edges, child } => LayoutNode::Padding {
            edges,
            child: Box::new(rewrite_pixel_node(*child, rewrite, icon)),
        },
        LayoutNode::Frame { width, height, child } => LayoutNode::Frame {
            width,
            height,
            child: Box::new(rewrite_pixel_node(*child, rewrite, icon)),
        },
        LayoutNode::MaxFrame { max_width, max_height, align, child } => LayoutNode::MaxFrame {
            max_width,
            max_height,
            align,
            child: Box::new(rewrite_pixel_node(*child, rewrite, icon)),
        },
        other => other,
    }
}

fn rewrite_text_node(
    node: LayoutNode,
    rewrite: &impl Fn(
        std::sync::Arc<str>,
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
        LayoutNode::Animated { key, spec, child } => LayoutNode::Animated {
            key,
            spec,
            child: Box::new(rewrite_text_node(*child, rewrite)),
        },
        LayoutNode::Island { child } => LayoutNode::Island {
            child: Box::new(rewrite_text_node(*child, rewrite)),
        },
        other => other,
    }
}

/// Consecutive paddings ADD instead of nesting — shrinking a proposal by
/// `a` and then by `b` is shrinking it by `a + b`, so one node carries
/// the sum and the chain of `.padding_edge(...)` calls stops paying one
/// box per edge.
fn wrap_padding(out: &mut NodeList, edges: Edges) {
    out.wrap_last_layout(|node| match node {
        LayoutNode::Padding { edges: inner, child } => LayoutNode::Padding {
            edges: Edges {
                top: inner.top + edges.top,
                bottom: inner.bottom + edges.bottom,
                leading: inner.leading + edges.leading,
                trailing: inner.trailing + edges.trailing,
            },
            child,
        },
        node => LayoutNode::Padding { edges, child: Box::new(node) },
    });
}

fn wrap_styled(out: &mut NodeList, delta: VisualProps) {
    out.wrap_last_layout(|node| match node {
        LayoutNode::Styled { mut props, child } => {
            *props = (*props).or(delta);
            LayoutNode::Styled { props, child }
        }
        other => LayoutNode::Styled { props: Box::new(delta), child: Box::new(other) },
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

        // `.id("name")` wraps the base's RENDER: the identity cursor
        // must carry the name while the subtree declares its state,
        // its animations and its hit paths.
        if let Modifier::Id(name) = &self.modifier {
            let mut nodes = NodeList::new();
            {
                let _frame = motor::identity::enter(format!("[{name}]"));
                self.base.render_into(ctx, &mut nodes);
            }
            if let Some(node) = nodes.last_mut() {
                node.line.push_str(&format!(" [.id({name:?})]"));
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
                // in layout, the sheet overlays the base — centered, the
                // way a modal sits over what it covers
                out.wrap_last_layout(|base| LayoutNode::Layered {
                    align: CrossAlign::Center,
                    children: vec![base, wrap_layout(sheet_layouts)],
                });
            }
            Modifier::Popover {
                is_presented,
                side,
                on_dismiss,
                content,
            } if is_presented.get() => {
                let mut popover_nodes = NodeList::new();
                let path;
                {
                    // its own identity sub-root, like the sheet: what
                    // the closure builds anchors here and dies when it
                    // closes (auto-focus re-fires on every open)
                    let _frame = motor::identity::enter("popover");
                    path = motor::identity::cursor_scope();
                    content(ctx).render_into(ctx, &mut popover_nodes);
                }
                let (popover_prints, popover_layouts) = popover_nodes.into_parts();
                if let Some(node) = out.last_mut() {
                    node.children
                        .push(RenderNode::branch("Popover", popover_prints));
                }
                if let Some(path) = &path {
                    // both dismiss triggers close through ONE machine:
                    // the outside press fires the action; Escape
                    // arrives by dispatch (innermost handler wins, so
                    // nested popovers close from the inside out)
                    let close: Rc<dyn Fn()> = {
                        let is_presented = is_presented.clone();
                        let on_dismiss = on_dismiss.clone();
                        Rc::new(move || {
                            is_presented.set(false);
                            if let Some(on_dismiss) = &on_dismiss {
                                on_dismiss();
                            }
                        })
                    };
                    crate::reconciler::attribute_action(
                        format!("{path}/#dismiss"),
                        close.clone(),
                    );
                    crate::reconciler::attribute_handler(
                        path.clone(),
                        crate::action::OVERLAY_DISMISS,
                        close,
                    );
                    crate::reconciler::attribute_context(crate::action::OVERLAY_CONTEXT);
                }
                let side = *side;
                out.wrap_last_layout(|base| LayoutNode::Anchored {
                    path: path.unwrap_or_default(),
                    side,
                    overlay: Rc::new(wrap_layout(popover_layouts)),
                    child: Box::new(base),
                });
            }
            _ => {}
        }

        // LAYOUT modifiers wrap the base's node — this is where the typed
        // chain becomes proposal/response structure
        match &self.modifier {
            Modifier::Padding => wrap_padding(out, Edges::uniform(16.0)),
            Modifier::PaddingLength(length) => wrap_padding(out, Edges::uniform(*length)),
            Modifier::PaddingEdge(edge, length) => {
                let mut edges = Edges::default();
                match edge {
                    Edge::Top => edges.top = *length,
                    Edge::Bottom => edges.bottom = *length,
                    Edge::Leading => edges.leading = *length,
                    Edge::Trailing => edges.trailing = *length,
                }
                wrap_padding(out, edges);
            }
            Modifier::FrameWH(width, height) => {
                let (width, height) = (Some(*width), Some(*height));
                out.wrap_last_layout(|node| LayoutNode::Frame {
                    width,
                    height,
                    child: Box::new(node),
                });
            }
            Modifier::FrameWidth(width) => {
                let width = Some(*width);
                out.wrap_last_layout(|node| LayoutNode::Frame {
                    width,
                    height: None,
                    child: Box::new(node),
                });
            }
            Modifier::FrameHeight(height) => {
                let height = Some(*height);
                out.wrap_last_layout(|node| LayoutNode::Frame {
                    width: None,
                    height,
                    child: Box::new(node),
                });
            }
            Modifier::FrameMax(max_width, max_height, alignment) => {
                let (max_width, max_height) = (*max_width, *max_height);
                let align = crate::views::cross_align(*alignment);
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
            Modifier::BackgroundGradient(gradient) => wrap_styled(
                out,
                VisualProps { gradient: Some(*gradient), ..VisualProps::default() },
            ),
            Modifier::Shadow(radius, color) => wrap_styled(
                out,
                VisualProps { shadow: Some((*radius, *color)), ..VisualProps::default() },
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
            // only the size travels: a `.bold()` or a `.font(.title)`
            // around it keeps its weight and its design
            Modifier::FontSize(size) => wrap_styled(
                out,
                VisualProps {
                    font: FontPatch { size: Some(*size), ..FontPatch::default() },
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
            Modifier::ForegroundHovered(color) => wrap_styled(
                out,
                VisualProps { foreground_hovered: Some(*color), ..VisualProps::default() },
            ),
            Modifier::ForegroundPressed(color) => wrap_styled(
                out,
                VisualProps { foreground_pressed: Some(*color), ..VisualProps::default() },
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
            Modifier::Resizable => out.wrap_last_layout(|node| {
                rewrite_pixel_node(
                    node,
                    &|source, _, fit| LayoutNode::Image { source, resizable: true, fit },
                    &|symbol, _| LayoutNode::Icon { symbol, resizable: true },
                )
            }),
            Modifier::AspectRatio(mode) => out.wrap_last_layout(|node| {
                rewrite_pixel_node(
                    node,
                    &|source, resizable, _| LayoutNode::Image {
                        source,
                        resizable,
                        fit: Some(*mode),
                    },
                    // a glyph is a square — it has its one ratio already
                    &|symbol, resizable| LayoutNode::Icon { symbol, resizable },
                )
            }),
            Modifier::Animated(spec) => {
                // the key is captured NOW, at render — the cursor is
                // gone by place time. Sibling views sit in distinct
                // tuple scopes; two `.animated` stacked on one view
                // share the key and the outer spec wins (documented).
                let key = motor::identity::cursor_scope().map(Rc::from);
                let spec = *spec;
                out.wrap_last_layout(|node| LayoutNode::Animated {
                    key,
                    spec,
                    child: Box::new(node),
                });
            }
            Modifier::Rendering(mode) => {
                // Auto is the table's business (v1: everything lowers
                // to Dom); only an explicit Gpu claims an island node
                if *mode == crate::layout::Rendering::Gpu {
                    out.wrap_last_layout(|node| LayoutNode::Island {
                        child: Box::new(node),
                    });
                }
            }
            Modifier::AutoFocus => out.wrap_last_layout(|node| {
                rewrite_field_node(node, &|path, content, placeholder| LayoutNode::Field {
                    path,
                    content,
                    placeholder,
                    auto_focus: true,
                })
            }),
            Modifier::KeyContext(name) => {
                // declaration, not paint: retained with the entry — the
                // context deactivates when the view unmounts
                crate::reconciler::attribute_context(name);
            }
            Modifier::WindowDragRegion => {
                out.wrap_last_layout(|node| LayoutNode::DragRegion {
                    child: Box::new(node),
                });
            }
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
