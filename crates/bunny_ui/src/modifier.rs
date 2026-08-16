//! Modificadores de view — o enum fechado no lugar do `Rc<dyn>` do motor.
//!
//! `Modifier` é um enum nosso (o conjunto é fechado por construção), então
//! `.font(.title)` não aloca nada: a variante mora inline no
//! [`Modified<C>`] e o dispatch é um jump table. As strings do `[.…]`
//! impresso são computadas no render, a partir da variante.
//!
//! Os behaviors (`onAppear`, efeitos, `sheet`, `EnvSet`, `Custom`) carregam
//! closures `Rc<dyn Fn>` — são as bordas de dinamismo declaradas: a fila de
//! efeitos e o conteúdo de sheet são heterogêneos por natureza.
//!
//! `Modified<C>` exige `C: View<Arity = Single>`: decorar uma tupla crua
//! (`(a, b).padding()`) aplicaria o modifier só no último nó — então não
//! compila. Quem quer decorar um grupo usa [`tuple`], que tem nó próprio.
//!
//! [`tuple`]: crate::views::tuple

use std::rc::Rc;

use motor::state::{Binding, Context, EffectFn, EnvironmentValues};
use motor::view::RenderNode;

use crate::erased::CustomModifier;
use crate::layout::{Color, CrossAlign, Edges, LayoutNode, VisualProps};
use crate::text_engine::{FontDesign, FontPatch, FontSpec, Weight};
use crate::state_ext::BindingExt;
use crate::view::{NodeList, Single, View};
use crate::views::{Alignment, wrap_layout};
use motor::views::{ContentMode, Edge, Font, ListStyle, ProgressViewStyle, TextAlignment};

/// Every modifier in the UI layer, as data.
#[derive(Clone)]
pub enum Modifier {
    // MARK: - Formatação (inert)
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

    // MARK: - Visuais (dados puros → `Styled` na cena)
    BackgroundColor(Color),
    ForegroundColor(Color),
    Border(Color, f64),
    CornerRadius(f64),
    Monospaced,
    BackgroundHovered(Color),
    BackgroundPressed(Color),

    // MARK: - Interação real (alvo de ponteiro sem chrome — o Button sem
    // a roupa; a ação dispara no up-inside como a dele)
    OnClick(Rc<dyn Fn()>),

    // MARK: - Interação (a ação dispara no render, como no motor headless)
    OnAppear(Rc<dyn Fn()>),
    OnTapGesture(Rc<dyn Fn()>),

    // MARK: - Efeitos (onChange / onReceive / query — drenados pelo pump)
    Effect {
        name: &'static str,
        detail: &'static str,
        effect: EffectFn,
    },

    // MARK: - Bordas dinâmicas
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

    // MARK: - Paridade inert
    Searchable,
    Refreshable,
    Toolbar,
    AttachEnvironmentOverrides,
    AttachEnvironmentOverridesOnChange,
    FlipsForRightToLeftLayoutDirection(bool),
    NavigationDestination,
}

impl Modifier {
    /// O `[.name(detail)]` appendo à linha do nó renderizado.
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
            Modifier::OnClick(_) => " [.onClick()]".into(),
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

/// Regra de merge dos modifiers visuais: estilos empilhados na MESMA view
/// fundem num único `Styled` — campo em conflito, o modifier mais PRÓXIMO
/// da view vence; campos distintos se acumulam
/// (`.background_color(a).corner_radius(r)` = UM nó, e o raio arredonda
/// ESTE fundo). Véu sobre véu na mesma view não compõe — camadas são do
/// `zstack`. O merge só acontece com `Styled` literal no topo:
/// `.background_color(a).padding().background_color(b)` aninha de verdade
/// (geometrias diferentes, os dois pintam).
fn wrap_styled(out: &mut NodeList, delta: VisualProps) {
    out.wrap_last_layout(|node| match node {
        LayoutNode::Styled { props, child } => {
            LayoutNode::Styled { props: props.or(delta), child }
        }
        other => LayoutNode::Styled { props: delta, child: Box::new(other) },
    });
}

/// A view modificada — `ModifiedContent` do Swift com o modifier inline.
#[derive(Clone)]
pub struct Modified<C> {
    pub(crate) base: C,
    pub(crate) modifier: Modifier,
}

impl<C: View<Arity = Single>> View for Modified<C> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        // `.modifier(…)` re-renderiza através do body(content:) do próprio
        // custom modifier, e marca o nó — é o caminho do blur recomputável.
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

        // `.inject()` / `.modelContainer()` regam a subárvore.
        let mut base_ctx = ctx.clone();
        if let Modifier::EnvSet { set, .. } = &self.modifier {
            set(&mut base_ctx.values);
        }

        self.base.render_into(&base_ctx, out);

        match &self.modifier {
            Modifier::OnAppear(action) | Modifier::OnTapGesture(action) => action(),
            Modifier::Effect { effect, .. } => {
                // O efeito vê o environment da subárvore — o pump só tem o
                // ctx do root em mãos.
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
                    // A sheet é uma sub-raiz com identidade própria: o que a
                    // closure constrói ancora aqui e morre quando ela fecha.
                    let _frame = motor::identity::enter("sheet");
                    content(ctx).render_into(ctx, &mut sheet_nodes);
                }
                let (sheet_prints, sheet_layouts) = sheet_nodes.into_parts();
                if let Some(node) = out.last_mut() {
                    node.children
                        .push(RenderNode::branch("Sheet", sheet_prints));
                }
                // no layout, a sheet sobrepõe a base
                out.wrap_last_layout(|base| LayoutNode::Layered {
                    children: vec![base, wrap_layout(sheet_layouts)],
                });
            }
            _ => {}
        }

        // modifiers de LAYOUT embrulham o nó da base — é aqui que a cadeia
        // tipada vira estrutura de proposta/resposta
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
            // fonte é propriedade herdada da cena — o mesmo Styled dos
            // visuais carrega o patch (o measure aplica por cima do env)
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
            Modifier::OnClick(action) => {
                // o mesmo registro do Button: ação retida no reconciler,
                // frame no hit-test pela identidade do cursor
                if let Some(path) = motor::identity::cursor_scope() {
                    crate::reconciler::attribute_action(path.clone(), action.clone());
                    out.wrap_last_layout(|node| LayoutNode::Interactive {
                        path,
                        hovered: false,
                        pressed: false,
                        child: Box::new(node),
                    });
                }
            }
            _ => {}
        }

        if let Some(node) = out.last_mut() {
            node.line.push_str(&self.modifier.suffix());
        }
    }
}
