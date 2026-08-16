//! Layout proposta/resposta — o protocolo e os algoritmos, headless.
//!
//! O contrato tem duas fases e três regras:
//!
//! 1. **O pai propõe** ([`Proposal`] — `None` = "você decide"), **o filho
//!    responde** um [`Size`], **o pai posiciona**. A resposta é uma função
//!    TOTAL da proposta: toda proposta tem resposta, não existe erro de
//!    layout.
//! 2. **Medir devolve um [`Fit`]** — o que a medição descobriu e a
//!    colocação reaproveita. `place` consome o `Fit` **por valor**: o
//!    sistema de tipos garante a ordem das fases (não há posicionar sem
//!    medir, nem posicionar duas vezes) e que nada se mede em dobro.
//! 3. **Encolher é decisão do container, na proposta.** Não existe "mínimo
//!    automático" que vaze do conteúdo por baixo do pano, nem propriedade
//!    visual mudando semântica de tamanho: uma [`LayoutNode::Scroll`]
//!    responde o que lhe foi oferecido no eixo de rolagem e guarda o
//!    tamanho do conteúdo para si — nenhum `min_h(0)` em lugar nenhum.
//!
//! A [`LayoutNode`] é a árvore de RUNTIME que o render produz: o body roda
//! UMA vez por pass e emite print e layout juntos (avaliar duas vezes
//! duplicaria âncoras de identidade). Depois da avaliação dos bodies, tudo
//! se reduz aos built-ins — um conjunto fechado, então enum. As métricas de
//! texto vêm do [`TextEngine`] do frame (o [`PixelFont`] determinístico da
//! casa por default — 8px por caractere, 16px por linha; CoreText no Mac);
//! os frames saem endereçados pelo caminho de identidade das fronteiras.
//!
//! [`PixelFont`]: crate::text_engine::PixelFont

use std::collections::HashMap;
use std::rc::Rc;

use crate::text_engine::{FontPatch, FontSpec, MeasureCache, PixelFont, TextEngine};

/// Pixels lógicos. O snapping para pixels de dispositivo é decisão do
/// backend real, num ponto único do pipeline — nunca espalhado.
pub type Px = f64;

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Size {
    pub width: Px,
    pub height: Px,
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Point {
    pub x: Px,
    pub y: Px,
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

/// A proposta do pai. `None` num eixo = "responda seu tamanho ideal".
///
/// Deliberadamente **sem `Default`**: um valor esquecido não pode degradar
/// silenciosamente para "mínimo" — quem propõe escolhe, sempre.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Proposal {
    pub width: Option<Px>,
    pub height: Option<Px>,
}

impl Proposal {
    /// Proposta exata nos dois eixos.
    pub fn exact(size: Size) -> Self {
        Proposal { width: Some(size.width), height: Some(size.height) }
    }

    /// "Você decide" nos dois eixos — o tamanho ideal do filho.
    pub fn unspecified() -> Self {
        Proposal { width: None, height: None }
    }
}

/// O eixo principal de um stack.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis {
    Vertical,
    Horizontal,
}

/// Alinhamento no eixo transversal, já em termos de layout (os tipos por
/// eixo da API — `HorizontalAlignment`/`VerticalAlignment` — convergem
/// para cá na construção do nó).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CrossAlign {
    Start,
    Center,
    End,
}

/// Insets de padding, por aresta.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Edges {
    pub top: Px,
    pub trailing: Px,
    pub bottom: Px,
    pub leading: Px,
}

impl Edges {
    pub fn uniform(amount: Px) -> Self {
        Edges { top: amount, trailing: amount, bottom: amount, leading: amount }
    }

    fn horizontal(&self) -> Px {
        self.leading + self.trailing
    }

    fn vertical(&self) -> Px {
        self.top + self.bottom
    }
}

/// Altura de linha do [`PixelFont`] — pública para os testes de frames.
///
/// [`PixelFont`]: crate::text_engine::PixelFont
pub const LINE_H: Px = 16.0;

/// O contexto que desce pelas duas fases: o engine de texto do frame, o
/// cache de medição, os offsets de rolagem (dono é o `Runtime`) e a fonte
/// HERDADA corrente — [`LayoutNode::Styled`] troca ao descer.
#[derive(Clone, Copy)]
pub struct LayoutEnv<'a> {
    pub text: &'a dyn TextEngine,
    pub cache: &'a MeasureCache,
    pub scroll_offsets: &'a HashMap<String, Point>,
    pub font: FontSpec,
}

/// A árvore de layout que um pass de render emite. Conjunto fechado (tudo
/// se reduz aos built-ins depois dos bodies), filhos em `Vec` — o dispatch
/// estático mora na árvore de VIEWS; esta é a estrutura de runtime.
#[derive(Clone, Debug)]
pub enum LayoutNode {
    /// Texto com métricas fake: `chars × 8px`, quebra pela proposta.
    Text { content: Rc<str> },
    /// Flexível no eixo principal do stack que o contém.
    Spacer,
    /// Caixa rígida (ProgressView, Image e afins, até existir de verdade).
    Leaf { size: Size },
    /// Preenche o que a proposta der (Rectangle).
    Fill,
    Stack { axis: Axis, spacing: Px, align: CrossAlign, children: Vec<LayoutNode> },
    /// Sobreposição: todos os filhos no mesmo frame (ZStack, sheet por cima).
    Layered { children: Vec<LayoutNode> },
    Padding { edges: Edges, child: Box<LayoutNode> },
    /// `.frame(width:height:)` — eixos `Some` sobrescrevem proposta e resposta.
    Frame { width: Option<Px>, height: Option<Px>, child: Box<LayoutNode> },
    /// `.frame(maxWidth:maxHeight:)` — `∞` = "preencha o proposto".
    MaxFrame { max_width: Px, max_height: Px, align: CrossAlign, child: Box<LayoutNode> },
    /// Região de rolagem vertical: responde o oferecido, mede o conteúdo
    /// sem restrição e guarda o excedente para si (o contrato de shrink).
    /// `path` é a identidade estrutural da região — o endereço do offset
    /// retido (rolagem restaura quando a lista remonta).
    Scroll { path: Option<String>, child: Box<LayoutNode> },
    /// Propriedade visual semântica: background atrás do filho, border por
    /// cima, foreground herdado. Transparente para a medida — por tipo.
    Styled { props: VisualProps, child: Box<LayoutNode> },
    /// Campo de texto de UMA linha — semântico de ponta a ponta (no Dom
    /// vira `<input>`; no Gpu, chrome + texto + caret + seleção daqui).
    /// `focused`/`caret`/`selection` são estampados POR FRAME na expansão
    /// (offsets de byte já clampados no conteúdo corrente); a retenção
    /// guarda o campo apagado.
    Field {
        path: String,
        content: Rc<str>,
        placeholder: Rc<str>,
        focused: bool,
        caret: Option<usize>,
        selection: Option<(usize, usize)>,
        /// Composição de IME viva — pinta sublinhada.
        marked: Option<(usize, usize)>,
    },
    /// Fronteira de view (`Component`): grava o frame no caminho de
    /// identidade — o endereço dos testes e, adiante, do hit-testing.
    Boundary { path: String, children: Vec<LayoutNode> },
    /// Alvo de interação (Button): o frame entra na lista de hit-test com
    /// o caminho que indexa a ação registrada no reconciler. `hovered`/
    /// `pressed` são estampados POR FRAME na expansão — a retenção guarda
    /// sempre `false` (estado de ponteiro nunca gruda no cache).
    Interactive { path: String, hovered: bool, pressed: bool, child: Box<LayoutNode> },
    /// Referência a uma fronteira retida (pulada pelo reconciler); a
    /// expansão resolve antes do measure — nunca chega ao algoritmo.
    BoundaryRef { path: String },
}

/// O handoff entre as fases — o espelho estrutural do [`LayoutNode`],
/// consumido por valor: não há posicionar sem medir, nem duas vezes.
#[derive(Debug)]
pub enum Fit {
    Leaf,
    /// Tamanhos e fits dos filhos, na ordem — medidos UMA vez.
    Children(Vec<(Size, Fit)>),
    Wrapped(Size, Box<Fit>),
    /// O tamanho real do conteúdo (pode exceder o frame — é o que rola).
    ScrollContent(Size, Box<Fit>),
}

/// Cor RGBA, sem drama. Estilização de verdade chega com os modifiers
/// visuais; por ora o pipeline usa os defaults do tema-de-um-lápis.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const BLACK: Color = Color { r: 20, g: 20, b: 25, a: 255 };
    pub const WHITE: Color = Color { r: 255, g: 255, b: 255, a: 255 };
    pub const FILL: Color = Color { r: 200, g: 205, b: 215, a: 255 };
    pub const OUTLINE: Color = Color { r: 150, g: 155, b: 165, a: 255 };
    /// Fundo padrão de janela — off-white frio, o chão do tema-de-um-lápis.
    pub const CANVAS: Color = Color::hex(0xF2F3F7);
    /// A thumb da scrollbar — véu translúcido (o blending é real).
    pub const SCROLLBAR: Color = Color { r: 0, g: 0, b: 0, a: 90 };
    /// Borda de campo focado.
    pub const FOCUS: Color = Color::hex(0x3B82F6);
    /// Texto de placeholder.
    pub const PLACEHOLDER: Color = Color::hex(0x9AA2B1);
    /// Véu de seleção de texto.
    pub const SELECTION: Color = Color::hex_a(0x3B82F640);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color {
        Color { r, g, b, a }
    }

    /// `0xRRGGBB`, alfa 255 — cor escrita do jeito que se lê.
    pub const fn hex(rgb: u32) -> Color {
        Color { r: (rgb >> 16) as u8, g: (rgb >> 8) as u8, b: rgb as u8, a: 255 }
    }

    /// `0xRRGGBBAA` — os véus do mundo real carregam alfa a sério.
    pub const fn hex_a(rgba: u32) -> Color {
        Color { r: (rgba >> 24) as u8, g: (rgba >> 16) as u8, b: (rgba >> 8) as u8, a: rgba as u8 }
    }
}

impl std::fmt::Display for Color {
    /// `#RRGGBB` (alfa só quando não é 255) — sufixos de print e mensagens
    /// de teste legíveis.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.a == 255 {
            write!(f, "#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
        } else {
            write!(f, "#{:02X}{:02X}{:02X}{:02X}", self.r, self.g, self.b, self.a)
        }
    }
}

/// Propriedades VISUAIS de um nó da cena — só pintura, por construção:
/// nenhum campo aqui altera medida (a LEI "hover não mexe em layout"
/// garantida pelo tipo). No modo Dom isto vira CSS do elemento; no Gpu,
/// comandos de desenho — a semântica nunca morre antes de o backend
/// escolher.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct VisualProps {
    pub background: Option<Color>,
    /// Herdado para baixo: o texto abaixo pinta com o foreground corrente.
    pub foreground: Option<Color>,
    pub border: Option<(Color, Px)>,
    pub corner_radius: Option<Px>,
    /// Fundo alternativo sob hover/pressed do `Interactive` ancestral —
    /// no Dom, `:hover`/`:active`. (Generalizar o estado para o
    /// `VisualProps` inteiro fica para o port do tema.)
    pub background_hovered: Option<Color>,
    pub background_pressed: Option<Color>,
    /// Patch de fonte herdado — a EXCEÇÃO da regra "props é só pintura":
    /// fonte muda medida, então desce pelo `LayoutEnv` já na medição (o
    /// estado de hover continua proibido de tocá-la).
    pub font: FontPatch,
}

impl VisualProps {
    /// Merge de modifiers empilhados na mesma view: o já definido (mais
    /// PRÓXIMO da view) vence; o de fora só preenche o que falta.
    pub fn or(self, outer: VisualProps) -> VisualProps {
        VisualProps {
            background: self.background.or(outer.background),
            foreground: self.foreground.or(outer.foreground),
            border: self.border.or(outer.border),
            corner_radius: self.corner_radius.or(outer.corner_radius),
            background_hovered: self.background_hovered.or(outer.background_hovered),
            background_pressed: self.background_pressed.or(outer.background_pressed),
            font: self.font.or(outer.font),
        }
    }
}

/// Estado de interação de um frame — resolvido ANTES do layout e estampado
/// na expansão (a LEI: hover troca pintura, nunca medida). O dono é o
/// `Runtime`; mora aqui por ser vocabulário da cena (caminhos + ponto).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Interaction {
    pub pointer: Option<Point>,
    pub hovered: Option<String>,
    pub pressed: Option<String>,
}

/// Um comando de desenho — a saída do passe de colocação, na ordem de
/// pintura (quem vem depois pinta por cima; `Layered` conta com isso).
/// É a interface do rasterizador e, adiante, de qualquer backend.
#[derive(Clone, Debug)]
pub enum DrawCommand {
    /// `corner_radius: 0.0` = retângulo puro (o caminho reto de sempre).
    FillRect { rect: Rect, color: Color, corner_radius: Px },
    /// Moldura pintada PARA DENTRO da aresta, `width` px lógicos.
    StrokeRect { rect: Rect, color: Color, width: Px },
    /// Uma linha de texto já quebrada. `origin` é o TOPO-esquerda da caixa
    /// de linha (o engine converte para baseline internamente); `font` é a
    /// fonte efetiva herdada no ponto da cena.
    TextLine { origin: Point, content: String, color: Color, font: FontSpec },
    /// Daqui até o [`DrawCommand::PopClip`] par, todo desenho intersecta
    /// este rect (o rect já chega intersectado com o clip de fora).
    PushClip { rect: Rect },
    PopClip,
}

/// A lista de desenho de um frame.
#[derive(Default, Debug)]
pub struct DisplayList {
    commands: Vec<DrawCommand>,
}

impl DisplayList {
    pub(crate) fn push(&mut self, command: DrawCommand) {
        self.commands.push(command);
    }

    pub fn iter(&self) -> impl Iterator<Item = &DrawCommand> {
        self.commands.iter()
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

/// As saídas do passe de colocação: frames por identidade (testes), a
/// lista de desenho (rasterizador/backends) e os alvos de interação (na
/// ordem de pintura — o hit-test varre de trás para frente, o de cima
/// ganha).
/// Uma região de rolagem colocada — o mapa do wheel. As regiões entram
/// filho-antes-do-pai: a mais interna sob o ponto decide primeiro.
#[derive(Clone, Debug)]
pub struct ScrollRegion {
    pub path: String,
    pub frame: Rect,
    pub content: Size,
}

/// Um campo de texto colocado: geometria + fonte EFETIVA no ponto da cena
/// — o clique-posiciona e a sincronização de IME medem por aqui.
#[derive(Clone, Debug)]
pub struct FieldPlacement {
    pub path: String,
    pub frame: Rect,
    pub text_origin: Point,
    pub font: FontSpec,
}

#[derive(Default, Debug)]
pub struct Placement {
    pub frames: Frames,
    pub display: DisplayList,
    pub hits: Vec<(String, Rect)>,
    pub scrolls: Vec<ScrollRegion>,
    pub fields: Vec<FieldPlacement>,
    /// Pilha do foreground herdado — o topo colore o texto.
    foreground: Vec<Color>,
    /// Pilha do `(hovered, pressed)` do `Interactive` mais próximo — o
    /// `Styled` escolhe o fundo por ela.
    pointer: Vec<(bool, bool)>,
    /// Pilha do clip corrente (interseções em coordenadas lógicas) — quem
    /// registra hit consulta; o raster refaz o corte em px físicos.
    clip: Vec<Rect>,
}

impl Placement {
    fn push_clip(&mut self, rect: Rect) {
        let clipped = match self.clip.last() {
            Some(top) => rect
                .intersection(*top)
                .unwrap_or(Rect { origin: rect.origin, size: Size::default() }),
            None => rect,
        };
        self.display.push(DrawCommand::PushClip { rect: clipped });
        self.clip.push(clipped);
    }

    fn pop_clip(&mut self) {
        self.display.push(DrawCommand::PopClip);
        self.clip.pop();
    }

    fn current_clip(&self) -> Option<Rect> {
        self.clip.last().copied()
    }
}

impl Rect {
    pub fn contains(&self, x: Px, y: Px) -> bool {
        x >= self.origin.x
            && y >= self.origin.y
            && x < self.origin.x + self.size.width
            && y < self.origin.y + self.size.height
    }

    /// `None` = interseção vazia.
    pub fn intersection(&self, other: Rect) -> Option<Rect> {
        let x0 = self.origin.x.max(other.origin.x);
        let y0 = self.origin.y.max(other.origin.y);
        let x1 = (self.origin.x + self.size.width).min(other.origin.x + other.size.width);
        let y1 = (self.origin.y + self.size.height).min(other.origin.y + other.size.height);
        (x1 > x0 && y1 > y0).then(|| Rect {
            origin: Point { x: x0, y: y0 },
            size: Size { width: x1 - x0, height: y1 - y0 },
        })
    }
}

/// O alvo mais ao topo sob o ponto — a chave da ação registrada.
pub fn hit_test(hits: &[(String, Rect)], x: Px, y: Px) -> Option<&str> {
    hits.iter()
        .rev()
        .find(|(_, rect)| rect.contains(x, y))
        .map(|(path, _)| path.as_str())
}

/// Os frames absolutos que o passe de colocação produz, endereçáveis pelo
/// caminho de identidade das fronteiras.
#[derive(Default, Debug)]
pub struct Frames {
    entries: Vec<(String, Rect)>,
}

impl Frames {
    fn record(&mut self, path: &str, frame: Rect) {
        self.entries.push((path.to_string(), frame));
    }

    /// O frame exato do caminho (o primeiro, se houver repetição).
    pub fn get(&self, path: &str) -> Option<Rect> {
        self.entries
            .iter()
            .find(|(entry, _)| entry == path)
            .map(|(_, frame)| *frame)
    }

    /// O primeiro frame cujo caminho termina no sufixo — endereço curto
    /// para testes (`"CountryCell"` em vez do caminho inteiro).
    pub fn find(&self, suffix: &str) -> Option<Rect> {
        self.entries
            .iter()
            .find(|(entry, _)| entry.ends_with(suffix))
            .map(|(_, frame)| *frame)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, Rect)> {
        self.entries.iter().map(|(path, frame)| (path.as_str(), *frame))
    }
}

/// Resultado do passe completo: o tamanho respondido pelo root, os frames,
/// a lista de desenho e os alvos de interação.
#[derive(Debug)]
pub struct LayoutResult {
    pub size: Size,
    pub frames: Frames,
    pub display: DisplayList,
    pub hits: Vec<(String, Rect)>,
    pub scrolls: Vec<ScrollRegion>,
    pub fields: Vec<FieldPlacement>,
}

/// Roda as duas fases a partir do root com o ambiente default — o
/// [`PixelFont`] determinístico, cache fresco, sem offsets de rolagem
/// (testes e uso direto; o `Runtime` monta o env real em
/// [`layout_with`]).
///
/// [`PixelFont`]: crate::text_engine::PixelFont
pub fn layout(root: &LayoutNode, proposal: Proposal) -> LayoutResult {
    let engine = PixelFont;
    let cache = MeasureCache::default();
    let offsets = HashMap::new();
    layout_with(
        root,
        proposal,
        LayoutEnv {
            text: &engine,
            cache: &cache,
            scroll_offsets: &offsets,
            font: FontSpec::DEFAULT,
        },
    )
}

/// Roda as duas fases com o ambiente do frame.
pub fn layout_with(root: &LayoutNode, proposal: Proposal, env: LayoutEnv) -> LayoutResult {
    let (size, fit) = root.measure(proposal, env);
    let mut out = Placement::default();
    root.place(Rect { origin: Point::default(), size }, fit, env, &mut out);
    LayoutResult {
        size,
        frames: out.frames,
        display: out.display,
        hits: out.hits,
        scrolls: out.scrolls,
        fields: out.fields,
    }
}

impl LayoutNode {
    /// Flexível = quer o espaço que sobrar no eixo (a base da distribuição
    /// dos stacks). Prioridade explícita, nunca efeito colateral de
    /// overflow.
    fn is_flexible(&self, axis: Axis) -> bool {
        match self {
            LayoutNode::Spacer | LayoutNode::Fill => true,
            LayoutNode::Scroll { .. } => axis == Axis::Vertical,
            // um campo toma a largura oferecida (como o TextField real)
            LayoutNode::Field { .. } => axis == Axis::Horizontal,
            LayoutNode::MaxFrame { max_width, max_height, child, .. } => match axis {
                Axis::Horizontal => max_width.is_infinite() || child.is_flexible(axis),
                Axis::Vertical => max_height.is_infinite() || child.is_flexible(axis),
            },
            LayoutNode::Frame { width, height, child } => match axis {
                Axis::Horizontal => width.is_none() && child.is_flexible(axis),
                Axis::Vertical => height.is_none() && child.is_flexible(axis),
            },
            LayoutNode::Padding { child, .. }
            | LayoutNode::Interactive { child, .. }
            | LayoutNode::Styled { child, .. } => child.is_flexible(axis),
            LayoutNode::Boundary { children, .. } => {
                children.len() == 1 && children[0].is_flexible(axis)
            }
            _ => false,
        }
    }

    pub(crate) fn measure(&self, proposal: Proposal, env: LayoutEnv) -> (Size, Fit) {
        match self {
            LayoutNode::Text { content } => {
                let metrics = env.cache.get_or_measure(content, &env.font, env.text);
                let natural = metrics.width;
                let line_h = metrics.height();
                // quebra REAL por palavra, com as medições do engine — a
                // largura entra na chave do cache (modo da sondagem)
                let size = match proposal.width {
                    Some(width) if width > 0.0 && width < natural => {
                        let lines =
                            env.cache.get_or_break(content, &env.font, width, env.text);
                        Size { width, height: lines.len() as Px * line_h }
                    }
                    _ => Size { width: natural, height: line_h },
                };
                (size, Fit::Leaf)
            }

            LayoutNode::Spacer | LayoutNode::Fill => {
                let size = Size {
                    width: proposal.width.unwrap_or(0.0),
                    height: proposal.height.unwrap_or(0.0),
                };
                (size, Fit::Leaf)
            }

            LayoutNode::Field { content, placeholder, .. } => {
                let sample: &str = if content.is_empty() { placeholder } else { content };
                let metrics = env.cache.get_or_measure(sample, &env.font, env.text);
                let natural = metrics.width + 2.0 * FIELD_PAD_H;
                let size = Size {
                    width: proposal.width.unwrap_or(natural),
                    height: metrics.height() + 2.0 * FIELD_PAD_V,
                };
                (size, Fit::Leaf)
            }

            LayoutNode::Leaf { size } => (*size, Fit::Leaf),

            LayoutNode::Stack { axis, spacing, children, .. } => {
                measure_stack(*axis, *spacing, children, proposal, env)
            }

            LayoutNode::Layered { children } => {
                let measured: Vec<(Size, Fit)> =
                    children.iter().map(|child| child.measure(proposal, env)).collect();
                let size = measured.iter().fold(Size::default(), |acc, (size, _)| Size {
                    width: acc.width.max(size.width),
                    height: acc.height.max(size.height),
                });
                (size, Fit::Children(measured))
            }

            LayoutNode::Padding { edges, child } => {
                let inset = |length: Option<Px>, total: Px| {
                    length.map(|length| (length - total).max(0.0))
                };
                let (child_size, fit) = child.measure(
                    Proposal {
                        width: inset(proposal.width, edges.horizontal()),
                        height: inset(proposal.height, edges.vertical()),
                    },
                    env,
                );
                let size = Size {
                    width: child_size.width + edges.horizontal(),
                    height: child_size.height + edges.vertical(),
                };
                (size, Fit::Wrapped(child_size, Box::new(fit)))
            }

            LayoutNode::Frame { width, height, child } => {
                let (child_size, fit) = child.measure(
                    Proposal {
                        width: width.or(proposal.width),
                        height: height.or(proposal.height),
                    },
                    env,
                );
                let size = Size {
                    width: width.unwrap_or(child_size.width),
                    height: height.unwrap_or(child_size.height),
                };
                (size, Fit::Wrapped(child_size, Box::new(fit)))
            }

            LayoutNode::MaxFrame { max_width, max_height, child, .. } => {
                let cap = |proposed: Option<Px>, max: Px| match proposed {
                    Some(length) => Some(length.min(max)),
                    None if max.is_finite() => Some(max),
                    None => None,
                };
                let (child_size, fit) = child.measure(
                    Proposal {
                        width: cap(proposal.width, *max_width),
                        height: cap(proposal.height, *max_height),
                    },
                    env,
                );
                // ∞ = preencha o proposto; finito = teto sobre o filho
                let resolve = |proposed: Option<Px>, max: Px, child_len: Px| {
                    if max.is_infinite() {
                        proposed.unwrap_or(child_len)
                    } else {
                        child_len.min(max)
                    }
                };
                let size = Size {
                    width: resolve(proposal.width, *max_width, child_size.width),
                    height: resolve(proposal.height, *max_height, child_size.height),
                };
                (size, Fit::Wrapped(child_size, Box::new(fit)))
            }

            LayoutNode::Scroll { child, .. } => {
                let (content, fit) = child.measure(
                    Proposal {
                        width: proposal.width,
                        height: None,
                    },
                    env,
                );
                let size = Size {
                    width: proposal.width.unwrap_or(content.width),
                    height: proposal.height.unwrap_or(content.height),
                };
                (size, Fit::ScrollContent(content, Box::new(fit)))
            }

            LayoutNode::Interactive { child, .. } => {
                let (size, fit) = child.measure(proposal, env);
                (size, Fit::Wrapped(size, Box::new(fit)))
            }

            LayoutNode::Styled { props, child } => {
                // a fonte herdada troca AQUI, já na medição — é a exceção
                // sancionada do VisualProps (fonte muda medida)
                let env = LayoutEnv { font: props.font.apply_over(env.font), ..env };
                let (size, fit) = child.measure(proposal, env);
                (size, Fit::Wrapped(size, Box::new(fit)))
            }

            LayoutNode::Boundary { children, .. } => {
                // fronteira com um filho é transparente; com vários, os
                // filhos empilham na vertical (o TupleView implícito)
                if children.len() == 1 {
                    let (size, fit) = children[0].measure(proposal, env);
                    (size, Fit::Children(vec![(size, fit)]))
                } else {
                    let (size, fit) =
                        measure_stack(Axis::Vertical, 0.0, children, proposal, env);
                    (size, fit)
                }
            }

            LayoutNode::BoundaryRef { path } => {
                debug_assert!(false, "BoundaryRef não expandida chegou ao measure: {path}");
                (Size::default(), Fit::Leaf)
            }
        }
    }

    pub(crate) fn place(&self, frame: Rect, fit: Fit, env: LayoutEnv, out: &mut Placement) {
        match (self, fit) {
            // folhas visuais: aqui nasce a lista de desenho
            (LayoutNode::Text { content }, Fit::Leaf) => {
                let color = out.foreground.last().copied().unwrap_or(Color::BLACK);
                if !content.is_empty() {
                    let metrics = env.cache.get_or_measure(content, &env.font, env.text);
                    let line_h = metrics.height();
                    if metrics.width <= frame.size.width {
                        out.display.push(DrawCommand::TextLine {
                            origin: frame.origin,
                            content: content.to_string(),
                            color,
                            font: env.font,
                        });
                    } else {
                        // as MESMAS quebras da medição (mesma chave, hit)
                        let lines = env.cache.get_or_break(
                            content,
                            &env.font,
                            frame.size.width,
                            env.text,
                        );
                        for (line_index, (start, end)) in lines.iter().enumerate() {
                            out.display.push(DrawCommand::TextLine {
                                origin: Point {
                                    x: frame.origin.x,
                                    y: frame.origin.y + line_index as Px * line_h,
                                },
                                content: content[*start..*end].to_string(),
                                color,
                                font: env.font,
                            });
                        }
                    }
                }
            }

            (LayoutNode::Fill, Fit::Leaf) => {
                out.display.push(DrawCommand::FillRect {
                    rect: frame,
                    color: Color::FILL,
                    corner_radius: 0.0,
                });
            }

            (
                LayoutNode::Field { path, content, placeholder, focused, caret, selection, marked },
                Fit::Leaf,
            ) => {
                // chrome: poço branco com borda (a borda acende no foco)
                out.display.push(DrawCommand::FillRect {
                    rect: frame,
                    color: Color::WHITE,
                    corner_radius: FIELD_RADIUS,
                });
                let text_origin = Point {
                    x: frame.origin.x + FIELD_PAD_H,
                    y: frame.origin.y + FIELD_PAD_V,
                };
                let metrics = env.cache.get_or_measure(
                    if content.is_empty() { placeholder } else { content },
                    &env.font,
                    env.text,
                );
                let prefix_width = |end: usize| {
                    env.cache.get_or_measure(&content[..end], &env.font, env.text).width
                };
                // seleção atrás do texto
                if let Some((start, end)) = selection {
                    let x0 = text_origin.x + prefix_width(*start);
                    let x1 = text_origin.x + prefix_width(*end);
                    out.display.push(DrawCommand::FillRect {
                        rect: Rect {
                            origin: Point { x: x0, y: text_origin.y },
                            size: Size { width: x1 - x0, height: metrics.height() },
                        },
                        color: Color::SELECTION,
                        corner_radius: 0.0,
                    });
                }
                if content.is_empty() {
                    if !placeholder.is_empty() {
                        // o placeholder anda pelo MESMO caminho do texto
                        // real: mesma origem, mesma fonte, só a cor cai
                        out.display.push(DrawCommand::TextLine {
                            origin: text_origin,
                            content: placeholder.to_string(),
                            color: Color::PLACEHOLDER,
                            font: env.font,
                        });
                    }
                } else {
                    let color = out.foreground.last().copied().unwrap_or(Color::BLACK);
                    out.display.push(DrawCommand::TextLine {
                        origin: text_origin,
                        content: content.to_string(),
                        color,
                        font: env.font,
                    });
                }
                // a composição viva ganha o sublinhado do IME
                if let Some((start, end)) = marked {
                    let x0 = text_origin.x + prefix_width(*start);
                    let x1 = text_origin.x + prefix_width(*end);
                    out.display.push(DrawCommand::FillRect {
                        rect: Rect {
                            origin: Point { x: x0, y: text_origin.y + metrics.height() - 1.0 },
                            size: Size { width: x1 - x0, height: 1.0 },
                        },
                        color: Color::BLACK,
                        corner_radius: 0.0,
                    });
                }
                // caret por cima (o blink alterna via estampa)
                if *focused && let Some(caret) = caret {
                    let x = text_origin.x + prefix_width(*caret);
                    out.display.push(DrawCommand::FillRect {
                        rect: Rect {
                            origin: Point { x, y: text_origin.y },
                            size: Size { width: FIELD_CARET_W, height: metrics.height() },
                        },
                        color: Color::BLACK,
                        corner_radius: FIELD_CARET_W / 2.0,
                    });
                }
                out.display.push(DrawCommand::StrokeRect {
                    rect: frame,
                    color: if *focused { Color::FOCUS } else { Color::OUTLINE },
                    width: 1.0,
                });
                // o campo é alvo de ponteiro (clicar foca) — clipado como
                // qualquer hit
                let visible = match out.current_clip() {
                    Some(clip) => frame.intersection(clip),
                    None => Some(frame),
                };
                if let Some(visible) = visible {
                    out.hits.push((path.clone(), visible));
                }
                out.fields.push(FieldPlacement {
                    path: path.clone(),
                    frame,
                    text_origin,
                    font: env.font,
                });
            }

            (LayoutNode::Leaf { .. }, Fit::Leaf) => {
                out.display.push(DrawCommand::StrokeRect {
                    rect: frame,
                    color: Color::OUTLINE,
                    width: 1.0,
                });
            }

            (LayoutNode::Stack { axis, spacing, align, children }, Fit::Children(fits)) => {
                place_stack(*axis, *spacing, *align, children, frame, fits, env, out);
            }

            (LayoutNode::Layered { children }, Fit::Children(fits)) => {
                for (child, (size, fit)) in children.iter().zip(fits) {
                    let origin = Point {
                        x: frame.origin.x + (frame.size.width - size.width) / 2.0,
                        y: frame.origin.y + (frame.size.height - size.height) / 2.0,
                    };
                    child.place(Rect { origin, size }, fit, env, out);
                }
            }

            (LayoutNode::Padding { edges, child }, Fit::Wrapped(child_size, fit)) => {
                let origin = Point {
                    x: frame.origin.x + edges.leading,
                    y: frame.origin.y + edges.top,
                };
                child.place(Rect { origin, size: child_size }, *fit, env, out);
            }

            (LayoutNode::Frame { child, .. }, Fit::Wrapped(child_size, fit)) => {
                let origin = Point {
                    x: frame.origin.x + (frame.size.width - child_size.width) / 2.0,
                    y: frame.origin.y + (frame.size.height - child_size.height) / 2.0,
                };
                child.place(Rect { origin, size: child_size }, *fit, env, out);
            }

            (LayoutNode::MaxFrame { align, child, .. }, Fit::Wrapped(child_size, fit)) => {
                let x = frame.origin.x
                    + match align {
                        CrossAlign::Start => 0.0,
                        CrossAlign::Center => (frame.size.width - child_size.width) / 2.0,
                        CrossAlign::End => frame.size.width - child_size.width,
                    };
                let y = frame.origin.y + (frame.size.height - child_size.height) / 2.0;
                child.place(Rect { origin: Point { x, y }, size: child_size }, *fit, env, out);
            }

            (LayoutNode::Scroll { path, child }, Fit::ScrollContent(content, fit)) => {
                // curso por eixo sobre valores SNAPADOS: "rolável por
                // 0.000001px" não existe aqui por construção
                let max_x = (content.width.round() - frame.size.width.round()).max(0.0);
                let max_y = (content.height.round() - frame.size.height.round()).max(0.0);
                let raw = path
                    .as_deref()
                    .and_then(|path| env.scroll_offsets.get(path))
                    .copied()
                    .unwrap_or_default();
                // conteúdo que encolheu re-clampa aqui — o offset retido
                // nunca deixa a região em terra de ninguém
                let offset =
                    Point { x: raw.x.clamp(0.0, max_x), y: raw.y.clamp(0.0, max_y) };
                out.push_clip(frame);
                child.place(
                    Rect {
                        origin: Point {
                            x: frame.origin.x - offset.x,
                            y: frame.origin.y - offset.y,
                        },
                        size: content,
                    },
                    *fit,
                    env,
                    out,
                );
                if max_y > 0.0 {
                    draw_scrollbar(frame, content.height, offset.y, max_y, out);
                }
                out.pop_clip();
                if let Some(path) = path {
                    // depois do filho: regiões internas ficam ANTES no vec
                    out.scrolls.push(ScrollRegion {
                        path: path.clone(),
                        frame,
                        content,
                    });
                }
            }

            (LayoutNode::Styled { props, child }, Fit::Wrapped(_, fit)) => {
                let env = LayoutEnv { font: props.font.apply_over(env.font), ..env };
                let (hovered, pressed) = out.pointer.last().copied().unwrap_or((false, false));
                // pressed > hovered > normal; estado sem fundo próprio cai
                // no fundo base — um botão sem hover definido não pisca
                let background = if pressed {
                    props.background_pressed.or(props.background)
                } else if hovered {
                    props.background_hovered.or(props.background)
                } else {
                    props.background
                };
                if let Some(color) = background {
                    out.display.push(DrawCommand::FillRect {
                        rect: frame,
                        color,
                        corner_radius: props.corner_radius.unwrap_or(0.0),
                    });
                }
                if let Some(color) = props.foreground {
                    out.foreground.push(color);
                }
                child.place(frame, *fit, env, out);
                if props.foreground.is_some() {
                    out.foreground.pop();
                }
                if let Some((color, width)) = props.border {
                    out.display.push(DrawCommand::StrokeRect { rect: frame, color, width });
                }
            }

            (LayoutNode::Interactive { path, hovered, pressed, child }, Fit::Wrapped(size, fit)) => {
                let _ = size;
                // fora do viewport o hit NÃO existe; row meio-visível
                // clica só na parte visível (o rect registrado é a
                // interseção com o clip corrente)
                let visible = match out.current_clip() {
                    Some(clip) => frame.intersection(clip),
                    None => Some(frame),
                };
                if let Some(visible) = visible {
                    out.hits.push((path.clone(), visible));
                }
                out.pointer.push((*hovered, *pressed));
                child.place(frame, *fit, env, out);
                out.pointer.pop();
            }

            (LayoutNode::Boundary { path, children }, Fit::Children(fits)) => {
                out.frames.record(path, frame);
                if children.len() == 1 {
                    let mut fits = fits;
                    let (size, fit) = fits.remove(0);
                    let _ = size;
                    children[0].place(frame, fit, env, out);
                } else {
                    place_stack(
                        Axis::Vertical,
                        0.0,
                        CrossAlign::Start,
                        children,
                        frame,
                        fits,
                        env,
                        out,
                    );
                }
            }

            (_, Fit::Leaf) => {}

            // o enum permite representar o par errado; a integração com Fit
            // associado por nó torna isso irrepresentável — aqui é bug nosso
            _ => unreachable!("fit de um nó usado em outro"),
        }
    }
}

/// O algoritmo dos stacks: mede todo mundo UMA vez sem restrição no eixo
/// principal (naturais + quem é flexível), divide o que sobrar entre os
/// flexíveis e só re-mede esses. Encolher nunca acontece por baixo do pano:
/// rígido fica com o natural.
fn measure_stack(
    axis: Axis,
    spacing: Px,
    children: &[LayoutNode],
    proposal: Proposal,
    env: LayoutEnv,
) -> (Size, Fit) {
    let cross_proposal = |main: Option<Px>| match axis {
        Axis::Vertical => Proposal { width: proposal.width, height: main },
        Axis::Horizontal => Proposal { width: main, height: proposal.height },
    };
    let main = |size: &Size| match axis {
        Axis::Vertical => size.height,
        Axis::Horizontal => size.width,
    };
    let cross = |size: &Size| match axis {
        Axis::Vertical => size.width,
        Axis::Horizontal => size.height,
    };
    let proposed_main = match axis {
        Axis::Vertical => proposal.height,
        Axis::Horizontal => proposal.width,
    };

    // fase 1: naturais (proposta irrestrita no eixo principal)
    let mut measured: Vec<(Size, Fit)> = children
        .iter()
        .map(|child| child.measure(cross_proposal(None), env))
        .collect();

    let spacing_total = spacing * children.len().saturating_sub(1) as Px;
    let flexible: Vec<usize> = children
        .iter()
        .enumerate()
        .filter(|(_, child)| child.is_flexible(axis))
        .map(|(index, _)| index)
        .collect();

    // fase 2: só os flexíveis re-medem, com a divisão do que sobrou
    if let Some(total) = proposed_main
        && !flexible.is_empty()
    {
        let rigid: Px = measured
            .iter()
            .enumerate()
            .filter(|(index, _)| !flexible.contains(index))
            .map(|(_, (size, _))| main(size))
            .sum();
        let share = ((total - rigid - spacing_total) / flexible.len() as Px).max(0.0);
        for &index in &flexible {
            measured[index] = children[index].measure(cross_proposal(Some(share)), env);
        }
    }

    let main_sum: Px = measured.iter().map(|(size, _)| main(size)).sum::<Px>() + spacing_total;
    let cross_max: Px = measured
        .iter()
        .map(|(size, _)| cross(size))
        .fold(0.0, Px::max);

    let size = match axis {
        Axis::Vertical => Size { width: cross_max, height: main_sum },
        Axis::Horizontal => Size { width: main_sum, height: cross_max },
    };
    (size, Fit::Children(measured))
}

const SCROLLBAR_W: Px = 4.0;
const SCROLLBAR_INSET: Px = 6.0;
const SCROLLBAR_MIN: Px = 24.0;

const FIELD_PAD_H: Px = 8.0;
const FIELD_PAD_V: Px = 5.0;
const FIELD_RADIUS: Px = 5.0;
const FIELD_CARET_W: Px = 1.5;

/// A thumb da região — draw-only nesta fase (drag chega com pointer
/// capture): 4px de largura a 6px da borda direita, trilho com inset 6,
/// piso de 24, proporcional ao viewport — e só existe quando há overflow
/// (conteúdo curto nunca ganha barra).
fn draw_scrollbar(frame: Rect, content_h: Px, offset_y: Px, max_y: Px, out: &mut Placement) {
    let track = frame.size.height - 2.0 * SCROLLBAR_INSET;
    if track <= 0.0 {
        return;
    }
    let thumb_h = ((frame.size.height / content_h) * track).max(SCROLLBAR_MIN).min(track);
    let travel = track - thumb_h;
    let thumb_y = frame.origin.y + SCROLLBAR_INSET + travel * (offset_y / max_y);
    out.display.push(DrawCommand::FillRect {
        rect: Rect {
            origin: Point {
                x: frame.origin.x + frame.size.width - SCROLLBAR_INSET - SCROLLBAR_W,
                y: thumb_y,
            },
            size: Size { width: SCROLLBAR_W, height: thumb_h },
        },
        color: Color::SCROLLBAR,
        corner_radius: SCROLLBAR_W / 2.0,
    });
}

#[allow(clippy::too_many_arguments)]
fn place_stack(
    axis: Axis,
    spacing: Px,
    align: CrossAlign,
    children: &[LayoutNode],
    frame: Rect,
    fits: Vec<(Size, Fit)>,
    env: LayoutEnv,
    out: &mut Placement,
) {
    let mut cursor = match axis {
        Axis::Vertical => frame.origin.y,
        Axis::Horizontal => frame.origin.x,
    };
    for (child, (size, fit)) in children.iter().zip(fits) {
        let cross_offset = |extent: Px, len: Px| match align {
            CrossAlign::Start => 0.0,
            CrossAlign::Center => (extent - len) / 2.0,
            CrossAlign::End => extent - len,
        };
        let origin = match axis {
            Axis::Vertical => Point {
                x: frame.origin.x + cross_offset(frame.size.width, size.width),
                y: cursor,
            },
            Axis::Horizontal => Point {
                x: cursor,
                y: frame.origin.y + cross_offset(frame.size.height, size.height),
            },
        };
        child.place(Rect { origin, size }, fit, env, out);
        cursor += match axis {
            Axis::Vertical => size.height,
            Axis::Horizontal => size.width,
        } + spacing;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(chars: usize) -> LayoutNode {
        LayoutNode::Text { content: Rc::from("x".repeat(chars)) }
    }

    fn boundary(path: &str, child: LayoutNode) -> LayoutNode {
        LayoutNode::Boundary { path: path.to_string(), children: vec![child] }
    }

    #[test]
    fn vstack_distributes_the_remainder_to_the_spacer() {
        let root = LayoutNode::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: CrossAlign::Start,
            children: vec![
                boundary("top", text(10)),
                boundary("gap", LayoutNode::Spacer),
                boundary("bottom", text(5)),
            ],
        };
        let result = layout(&root, Proposal { width: Some(200.0), height: Some(100.0) });

        assert_eq!(result.size.height, 100.0);
        assert_eq!(result.frames.get("top").unwrap().origin.y, 0.0);
        assert_eq!(result.frames.get("gap").unwrap().size.height, 68.0);
        assert_eq!(result.frames.get("bottom").unwrap().origin.y, 84.0);
    }

    #[test]
    fn scroll_region_never_propagates_the_content_minimum() {
        // A dor clássica de flexbox, morta por construção: header + scroll
        // de conteúdo gigante num viewport de 300 — o header fica natural,
        // a região responde o que sobrou e o conteúdo excede POR DENTRO.
        // Nenhum min_h(0), nenhum overflow mágico.
        let root = LayoutNode::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: CrossAlign::Start,
            children: vec![
                boundary("header", text(10)),
                boundary(
                    "region",
                    LayoutNode::Scroll {
                        path: None,
                        child: Box::new(boundary("content", text(1000))),
                    },
                ),
            ],
        };
        let result = layout(&root, Proposal { width: Some(400.0), height: Some(300.0) });

        let header = result.frames.get("header").unwrap();
        let region = result.frames.get("region").unwrap();
        let content = result.frames.get("content").unwrap();

        assert_eq!(header.size.height, LINE_H);
        assert_eq!(region.size.height, 300.0 - LINE_H, "a região pega o que sobrou");
        assert!(
            content.size.height > region.size.height,
            "o conteúdo excede por dentro — é isso que rola: {} > {}",
            content.size.height,
            region.size.height
        );
    }

    #[test]
    fn padding_shrinks_the_proposal_and_grows_the_answer() {
        let root = LayoutNode::Padding {
            edges: Edges::uniform(10.0),
            child: Box::new(boundary("inner", text(5))),
        };
        let result = layout(&root, Proposal::unspecified());

        assert_eq!(result.size, Size { width: 60.0, height: 36.0 });
        let inner = result.frames.get("inner").unwrap();
        assert_eq!(inner.origin, Point { x: 10.0, y: 10.0 });
    }

    #[test]
    fn frame_overrides_only_its_axes() {
        let root = LayoutNode::Frame {
            width: Some(100.0),
            height: None,
            child: Box::new(text(5)),
        };
        let result = layout(&root, Proposal::unspecified());
        assert_eq!(result.size, Size { width: 100.0, height: LINE_H });
    }

    /// Mede um nó com o ambiente default dos testes (PixelFont).
    fn measure_with_defaults(node: &LayoutNode, proposal: Proposal) -> Size {
        let engine = PixelFont;
        let cache = MeasureCache::default();
        let offsets = HashMap::new();
        let env = LayoutEnv {
            text: &engine,
            cache: &cache,
            scroll_offsets: &offsets,
            font: FontSpec::DEFAULT,
        };
        node.measure(proposal, env).0
    }

    #[test]
    fn max_frame_fills_when_infinite_and_caps_when_finite() {
        let fill = LayoutNode::MaxFrame {
            max_width: f64::INFINITY,
            max_height: 60.0,
            align: CrossAlign::Start,
            child: Box::new(text(5)),
        };
        let size =
            measure_with_defaults(&fill, Proposal { width: Some(300.0), height: Some(500.0) });
        assert_eq!(size, Size { width: 300.0, height: LINE_H });
    }

    #[test]
    fn text_wraps_against_the_proposed_width() {
        let size =
            measure_with_defaults(&text(100), Proposal { width: Some(100.0), height: None });
        // 100 chars sem espaço em 100px: hard-break por CHAR inteiro —
        // 12 por linha (96px ≤ 100) → 9 linhas (a quebra real não
        // fraciona caracteres como a média antiga fazia)
        assert_eq!(size, Size { width: 100.0, height: 144.0 });
    }

    #[test]
    fn words_wrap_at_spaces_never_mid_word() {
        let node = LayoutNode::Text { content: Rc::from("aa bb cc") };
        let result = layout(&node, Proposal { width: Some(40.0), height: None });

        // "aa bb" (40px) cabe; "cc" desce inteiro — nunca "c" órfão
        assert_eq!(result.size.height, 2.0 * LINE_H);
        let lines: Vec<String> = result
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::TextLine { content, .. } => Some(content.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(lines, vec!["aa bb ".to_string(), "cc".to_string()]);
    }

    #[test]
    fn a_word_longer_than_the_line_hard_breaks() {
        let node = LayoutNode::Text { content: Rc::from("aaaaaaaaaa") };
        let result = layout(&node, Proposal { width: Some(40.0), height: None });

        // 10 chars de 8px em 40px: 5 por linha
        assert_eq!(result.size.height, 2.0 * LINE_H);
        let lines: Vec<String> = result
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::TextLine { content, .. } => Some(content.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(lines, vec!["aaaaa".to_string(), "aaaaa".to_string()]);
    }

    fn styled(props: VisualProps, child: LayoutNode) -> LayoutNode {
        LayoutNode::Styled { props, child: Box::new(child) }
    }

    fn rows(count: usize) -> LayoutNode {
        LayoutNode::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: CrossAlign::Start,
            children: (0..count)
                .map(|index| boundary(&format!("row{index}"), text(4)))
                .collect(),
        }
    }

    #[test]
    fn scroll_offset_moves_content_under_the_clip() {
        let engine = PixelFont;
        let cache = MeasureCache::default();
        let mut offsets = HashMap::new();
        offsets.insert("lista".to_string(), Point { x: 0.0, y: 40.0 });
        let env = LayoutEnv {
            text: &engine,
            cache: &cache,
            scroll_offsets: &offsets,
            font: FontSpec::DEFAULT,
        };

        let root = LayoutNode::Scroll {
            path: Some("lista".to_string()),
            child: Box::new(rows(10)),
        };
        let result = layout_with(
            &root,
            Proposal::exact(Size { width: 100.0, height: 100.0 }),
            env,
        );

        assert_eq!(result.scrolls.len(), 1);
        assert_eq!(result.scrolls[0].content.height, 160.0, "o content real fica na região");
        assert_eq!(
            result.frames.get("row0").unwrap().origin.y,
            -40.0,
            "offset 40 empurra a row 0 para cima do viewport"
        );
        assert!(
            result.display.iter().any(|command| matches!(
                command,
                DrawCommand::PushClip { rect } if rect.size.height == 100.0
            )),
            "a região clipa no frame dela"
        );
    }

    #[test]
    fn hits_outside_the_viewport_do_not_exist() {
        let interactive = |path: &str| LayoutNode::Interactive {
            path: path.to_string(),
            hovered: false,
            pressed: false,
            child: Box::new(text(4)),
        };
        let root = LayoutNode::Scroll {
            path: Some("lista".to_string()),
            child: Box::new(LayoutNode::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: CrossAlign::Start,
                children: vec![
                    interactive("dentro"), // y [0, 16)
                    interactive("metade"), // y [16, 32) — o viewport corta em 24
                    interactive("fora"),   // y [32, 48) — invisível
                ],
            }),
        };
        let result = layout(&root, Proposal::exact(Size { width: 100.0, height: 24.0 }));

        assert!(result.hits.iter().any(|(path, _)| path == "dentro"));
        let metade = result
            .hits
            .iter()
            .find(|(path, _)| path == "metade")
            .map(|(_, rect)| *rect)
            .expect("meio-visível existe");
        assert_eq!(metade.size.height, 8.0, "o hit é só a parte visível");
        assert!(
            !result.hits.iter().any(|(path, _)| path == "fora"),
            "fora do viewport o hit NÃO existe"
        );
    }

    #[test]
    fn scrollbar_appears_only_with_overflow() {
        let scroll = |count: usize| LayoutNode::Scroll {
            path: Some("lista".to_string()),
            child: Box::new(rows(count)),
        };
        let viewport = Proposal::exact(Size { width: 100.0, height: 100.0 });
        let thumb_of = |result: &LayoutResult| {
            result.display.iter().find_map(|command| match command {
                DrawCommand::FillRect { rect, color, .. } if *color == Color::SCROLLBAR => {
                    Some(*rect)
                }
                _ => None,
            })
        };

        let fits = layout(&scroll(2), viewport);
        assert!(thumb_of(&fits).is_none(), "conteúdo curto nunca ganha barra");

        let over = layout(&scroll(10), viewport);
        let thumb = thumb_of(&over).expect("overflow ganha thumb");
        // trilho 88 (inset 6 dos dois lados), proporcional 100/160
        assert_eq!(thumb.size.height, (100.0 / 160.0_f64 * 88.0).max(24.0));
        assert_eq!(thumb.size.width, 4.0);
        assert_eq!(thumb.origin.x, 100.0 - 6.0 - 4.0);
    }

    #[test]
    fn styled_paints_background_behind_and_border_on_top() {
        let root = styled(
            VisualProps {
                background: Some(Color::hex(0x112233)),
                border: Some((Color::hex(0x445566), 2.0)),
                corner_radius: Some(4.0),
                ..VisualProps::default()
            },
            text(3),
        );
        let result = layout(&root, Proposal::unspecified());

        let commands: Vec<_> = result.display.iter().collect();
        assert_eq!(commands.len(), 3, "fundo, texto, borda — nesta ordem");
        assert!(matches!(
            commands[0],
            DrawCommand::FillRect { color, corner_radius, .. }
                if *color == Color::hex(0x112233) && *corner_radius == 4.0
        ));
        assert!(matches!(commands[1], DrawCommand::TextLine { .. }));
        assert!(matches!(
            commands[2],
            DrawCommand::StrokeRect { color, width, .. }
                if *color == Color::hex(0x445566) && *width == 2.0
        ));
    }

    #[test]
    fn styled_never_changes_measurement() {
        // a LEI no nível do nó: VisualProps é pintura pura
        let plain = layout(&text(7), Proposal::unspecified());
        let dressed = layout(
            &styled(
                VisualProps {
                    background: Some(Color::BLACK),
                    border: Some((Color::WHITE, 3.0)),
                    corner_radius: Some(8.0),
                    ..VisualProps::default()
                },
                text(7),
            ),
            Proposal::unspecified(),
        );
        assert_eq!(plain.size, dressed.size);
    }

    #[test]
    fn foreground_inherits_and_the_nearest_wins() {
        let outer = Color::hex(0x00AA00);
        let inner = Color::hex(0xAA0000);
        let root = styled(
            VisualProps { foreground: Some(outer), ..VisualProps::default() },
            LayoutNode::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: CrossAlign::Start,
                children: vec![
                    text(3),
                    styled(
                        VisualProps { foreground: Some(inner), ..VisualProps::default() },
                        text(3),
                    ),
                ],
            },
        );
        let result = layout(&root, Proposal::unspecified());

        let colors: Vec<Color> = result
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::TextLine { color, .. } => Some(*color),
                _ => None,
            })
            .collect();
        assert_eq!(colors, vec![outer, inner]);
    }

    #[test]
    fn hovered_swaps_paint_but_never_frames() {
        let node = |hovered: bool| LayoutNode::Interactive {
            path: "botao".to_string(),
            hovered,
            pressed: false,
            child: Box::new(styled(
                VisualProps {
                    background: Some(Color::hex(0x111111)),
                    background_hovered: Some(Color::hex(0x222222)),
                    ..VisualProps::default()
                },
                boundary("label", text(4)),
            )),
        };
        let cold = layout(&node(false), Proposal::unspecified());
        let hot = layout(&node(true), Proposal::unspecified());

        assert_eq!(cold.size, hot.size);
        assert_eq!(
            cold.frames.get("label"),
            hot.frames.get("label"),
            "a LEI: hover nunca mexe em frame"
        );
        let background = |result: &LayoutResult| {
            result
                .display
                .iter()
                .find_map(|command| match command {
                    DrawCommand::FillRect { color, .. } => Some(*color),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(background(&cold), Color::hex(0x111111));
        assert_eq!(background(&hot), Color::hex(0x222222));
    }

    #[test]
    fn pressed_beats_hovered() {
        let root = LayoutNode::Interactive {
            path: "botao".to_string(),
            hovered: true,
            pressed: true,
            child: Box::new(styled(
                VisualProps {
                    background: Some(Color::hex(0x111111)),
                    background_hovered: Some(Color::hex(0x222222)),
                    background_pressed: Some(Color::hex(0x333333)),
                    ..VisualProps::default()
                },
                text(2),
            )),
        };
        let result = layout(&root, Proposal::unspecified());

        let background = result
            .display
            .iter()
            .find_map(|command| match command {
                DrawCommand::FillRect { color, .. } => Some(*color),
                _ => None,
            })
            .unwrap();
        assert_eq!(background, Color::hex(0x333333));
    }
}
