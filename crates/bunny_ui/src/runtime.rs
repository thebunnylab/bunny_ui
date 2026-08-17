//! `Runtime` — o fake main-thread desta camada: renderiza a árvore tipada,
//! bombeia efeitos e estabiliza (re-render até a árvore impressa parar de
//! mudar — o stand-in do loop de frames).
//!
//! Cada `render` é um pass de identidade ([`motor::identity`]) dirigido
//! pelo reconciler: fronteiras limpas e retidas PULAM o body (o cache
//! responde), sujas re-rodam — mesmo atrás de pai pulado (re-run isolado a
//! partir do valor retido). A fila de efeitos é remontada da retenção, a
//! varredura desmonta o que saiu da árvore, e a montagem final expande as
//! referências. `set()` marca de sujo quem LEU — a invalidação fina entra
//! na condição de estabilidade e fica visível em [`Runtime::take_dirty`] e
//! [`Runtime::body_runs`].

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use motor::state::{Context, EnvironmentValues};

use crate::action::{ActionId, KeyPattern};
use crate::effects;
use crate::layout::{FieldPlacement, Interaction, LayoutEnv, Point, Px, Rect, ScrollRegion};
use crate::reconciler;
use crate::text_engine::{FontSpec, MeasureCache, PixelFont, TextEngine, caret_from_x};
use crate::text_input::{CaretState, EditCommand};
use crate::view::{NodeList, View};

/// O resultado de um comando de edição: `applied` = houve campo focado
/// para receber (o shell repinta); `output` = o texto que `Read`/`Copy`/
/// `Cut` extraem (a ponte do clipboard e da sincronização de IME).
pub struct Edited {
    pub applied: bool,
    pub output: Option<String>,
}

/// O que a plataforma pergunta ao campo focado (NSTextInputClient e
/// afins): texto, seleção e composição em UTF-16 — o vocabulário dela —
/// e o rect do caret em coordenadas de LAYOUT (o shell converte para
/// tela; é onde a janela de candidatos do IME aterrissa).
pub struct ImeSnapshot {
    pub text: String,
    /// (location, length) em UTF-16.
    pub selected: (usize, usize),
    /// Range marcado em UTF-16, se houver composição viva.
    pub marked: Option<(usize, usize)>,
    pub caret_rect: Rect,
}

pub struct Runtime {
    ctx: Context,
    /// O root do último pass — escopa `take_dirty` para não drenar sujeira
    /// de outra árvore montada na mesma thread.
    last_root: RefCell<Option<String>>,
    /// Os alvos do último layout, na ordem de pintura — o mapa do
    /// hit-test dos eventos de ponteiro.
    last_hits: RefCell<Vec<(String, Rect)>>,
    /// Estado de ponteiro do frame — resolvido ANTES do layout (a LEI:
    /// hover troca pintura, nunca medida) e estampado na expansão.
    interaction: RefCell<Interaction>,
    /// A borda de texto do frame — PixelFont por default (headless
    /// byte-estável); o shell instala o engine da plataforma.
    text: Rc<dyn TextEngine>,
    /// Cache de medição double-buffer, trocado a cada passada de layout.
    cache: MeasureCache,
    /// Offsets de rolagem por identidade — engine-owned (a posse dupla da
    /// premissa: no Dom o backend será o dono e observaremos). Não são
    /// podados quando a identidade some: a lista remontada RESTAURA a
    /// posição (GC atado à varredura fica anotado como futuro).
    scroll_offsets: RefCell<HashMap<String, Point>>,
    /// As regiões de rolagem do último layout — o mapa do wheel.
    last_scrolls: RefCell<Vec<ScrollRegion>>,
    /// O campo focado (caminho de identidade) — dono do teclado.
    focus: RefCell<Option<String>>,
    /// Caret + seleção por campo — sobrevivem a blur/refoco e a
    /// remontagem (restauração por identidade, como o scroll).
    carets: RefCell<HashMap<String, CaretState>>,
    /// Fase do blink: o caret some e volta no tick do shell; digitar ou
    /// focar volta para sólido (caret parado pisca, caret ativo não).
    caret_visible: Cell<bool>,
    /// Os campos do último layout (geometria + fonte efetiva) — o
    /// clique-posiciona e a sincronização de IME medem por aqui.
    last_fields: RefCell<Vec<FieldPlacement>>,
    /// A versão do tema vista pelo último pass — trocar tema reconstrói a
    /// retenção UMA vez (tokens lidos em body ficam gravados na cena).
    theme_version: Cell<u64>,
    /// O keymap do app: padrão de tecla → ação. Config do Runtime (como o
    /// engine de texto), não retenção — bind é declaração de intenção.
    keymap: RefCell<HashMap<KeyPattern, ActionId>>,
    /// O último pass viu a raiz virar UMA fronteira (`Boundary`/ref)? Só
    /// então o frame estável pode sintetizar a referência sem andar o
    /// pass — raiz sem fronteira sai fresca do walk a cada frame.
    root_is_boundary: Cell<bool>,
    /// A retenção pode conter entries SEM linhas de print (construídas no
    /// caminho de frame, que não formata) — imprimir de novo reconstrói
    /// uma vez, e o oráculo full == incremental segue byte a byte.
    printless: Cell<bool>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

impl Runtime {
    pub fn new() -> Self {
        Self::with_parts(Context::default(), Rc::new(PixelFont))
    }

    pub fn with_environment(values: EnvironmentValues) -> Self {
        let mut ctx = Context::default();
        ctx.values = values;
        Self::with_parts(ctx, Rc::new(PixelFont))
    }

    /// Troca o engine de texto (builder — compõe com `with_environment`):
    /// `Runtime::new().text_engine(Rc::new(CoreTextEngine::new()))`.
    pub fn text_engine(mut self, engine: Rc<dyn TextEngine>) -> Self {
        self.text = engine;
        self
    }

    fn with_parts(ctx: Context, text: Rc<dyn TextEngine>) -> Self {
        Runtime {
            ctx,
            last_root: RefCell::new(None),
            last_hits: RefCell::new(Vec::new()),
            interaction: RefCell::new(Interaction::default()),
            text,
            cache: MeasureCache::default(),
            scroll_offsets: RefCell::new(HashMap::new()),
            last_scrolls: RefCell::new(Vec::new()),
            focus: RefCell::new(None),
            carets: RefCell::new(HashMap::new()),
            caret_visible: Cell::new(true),
            last_fields: RefCell::new(Vec::new()),
            theme_version: Cell::new(crate::theme::version()),
            keymap: RefCell::new(HashMap::new()),
            root_is_boundary: Cell::new(false),
            printless: Cell::new(false),
        }
    }

    pub fn context(&self) -> Context {
        self.ctx.clone()
    }

    /// Um pass incremental: walk com pulos, re-runs isolados dos sujos que
    /// o walk não alcançou, remontagem da fila de efeitos e varredura.
    /// Devolve as duas saídas (print e layout) ainda com referências.
    fn render_pass(&self, root: &impl View) -> NodeList {
        // tema novo = retenção velha (bodies gravaram tokens antigos na
        // cena): reconstrói uma vez e segue incremental
        let theme_version = crate::theme::version();
        if self.theme_version.get() != theme_version {
            self.theme_version.set(theme_version);
            reconciler::clear();
        }
        effects::reset();
        let snapshot = motor::identity::dirty_snapshot();
        reconciler::begin_pass(snapshot.clone());
        motor::identity::begin_pass();

        let mut nodes = NodeList::new();
        root.render_into(&self.ctx, &mut nodes);

        let pass_root = motor::identity::current_pass_root();
        if let Some(pass_root) = &pass_root {
            reconciler::run_isolated(pass_root);
        }

        let dead = motor::identity::end_pass();
        reconciler::forget(&dead);
        if let Some(pass_root) = &pass_root {
            // a gêmea da varredura acima, para views sem estado próprio
            reconciler::sweep_stale(pass_root);
        }

        if let Some(pass_root) = &pass_root {
            effects::set_queue(reconciler::assemble_effects(pass_root));
            reconciler::assemble_actions(pass_root);
            reconciler::assemble_editors(pass_root);
            reconciler::assemble_handlers(pass_root);
            motor::identity::consume_dirty(pass_root, &snapshot);
            *self.last_root.borrow_mut() = Some(pass_root.clone());
        }
        reconciler::end_pass();
        nodes
    }

    /// Dispara a ação do alvo interativo (a chave vem do hit-test sobre
    /// `LayoutResult::hits`). `false` = alvo não registrado.
    pub fn activate(&self, path: &str) -> bool {
        reconciler::run_action(path)
    }

    // MARK: - Ponteiro (resolvido ANTES do layout — a LEI)

    /// O alvo sob o ponto, contra os hits do último layout.
    fn hover_target(&self, x: Px, y: Px) -> Option<String> {
        crate::layout::hit_test(&self.last_hits.borrow(), x, y).map(str::to_string)
    }

    /// Ponteiro moveu. `true` = o estado visível mudou (o shell repinta).
    /// Durante um press, o hover só re-resolve contra o alvo pressionado:
    /// arrastar para fora solta o visual, voltar re-arma (AppKit).
    pub fn pointer_moved(&self, x: Px, y: Px) -> bool {
        let target = self.hover_target(x, y);
        let mut interaction = self.interaction.borrow_mut();
        let hovered = match &interaction.pressed {
            Some(pressed) => target.filter(|candidate| candidate == pressed),
            None => target,
        };
        let changed = interaction.hovered != hovered;
        interaction.pointer = Some(Point { x, y });
        interaction.hovered = hovered;
        changed
    }

    /// Botão desceu: ARMA o pressed no alvo sob o ponto — a ação não
    /// dispara aqui (up-inside é a semântica de botão). `true` = repaint.
    pub fn pointer_pressed(&self, x: Px, y: Px) -> bool {
        let target = self.hover_target(x, y);
        let mut interaction = self.interaction.borrow_mut();
        interaction.pointer = Some(Point { x, y });
        let changed = interaction.pressed != target || interaction.hovered != target;
        interaction.hovered = target.clone();
        interaction.pressed = target;
        changed
    }

    /// Botão subiu: dispara a ação SE soltou dentro do alvo pressionado —
    /// ou FOCA, se o alvo é um campo de texto. Soltar fora de qualquer
    /// campo tira o foco (first responder segue o clique). Devolve o
    /// caminho disparado/focado; o visual de pressed limpa sempre.
    pub fn pointer_released(&self, x: Px, y: Px) -> Option<String> {
        let target = self.hover_target(x, y);
        let fired = {
            let mut interaction = self.interaction.borrow_mut();
            let pressed = interaction.pressed.take();
            interaction.pointer = Some(Point { x, y });
            interaction.hovered = target.clone();
            match (pressed, target) {
                (Some(pressed), Some(target)) if pressed == target => Some(pressed),
                _ => None,
            }
        };
        // fora do borrow: a ação pode escrever estado e re-entrar aqui
        match fired {
            Some(path) if reconciler::has_editor(&path) => {
                self.focus_at(&path, x);
                Some(path)
            }
            Some(path) if self.activate(&path) => {
                self.blur();
                Some(path)
            }
            _ => {
                self.blur();
                None
            }
        }
    }

    /// O ponteiro saiu da janela: limpa o hover (um press em andamento já
    /// teve o visual solto pelo `pointer_moved` do drag).
    pub fn pointer_exited(&self) -> bool {
        let mut interaction = self.interaction.borrow_mut();
        let changed = interaction.hovered.is_some();
        interaction.hovered = None;
        interaction.pointer = None;
        changed
    }

    /// Snapshot do estado de ponteiro — o cursor do shell e os asserts.
    pub fn interaction(&self) -> Interaction {
        self.interaction.borrow().clone()
    }

    // MARK: - Rolagem (offset é estado do ENGINE: nenhuma view invalida)

    /// Roteia o wheel para a região mais interna sob o ponto COM curso no
    /// eixo do delta. Convenção AppKit: delta positivo revela conteúdo
    /// acima — o offset diminui. `true` = moveu (o shell repinta; sem
    /// render: zero bodies).
    pub fn wheel(&self, x: Px, y: Px, dx: Px, dy: Px) -> bool {
        let scrolls = self.last_scrolls.borrow();
        let travel = |region: &ScrollRegion| {
            let max_x =
                (region.content.width.round() - region.frame.size.width.round()).max(0.0);
            let max_y =
                (region.content.height.round() - region.frame.size.height.round()).max(0.0);
            (max_x, max_y)
        };
        let Some(region) = scrolls.iter().find(|region| {
            region.frame.contains(x, y) && {
                let (max_x, max_y) = travel(region);
                (dx != 0.0 && max_x > 0.0) || (dy != 0.0 && max_y > 0.0)
            }
        }) else {
            return false;
        };
        let (max_x, max_y) = travel(region);
        let mut offsets = self.scroll_offsets.borrow_mut();
        let current = offsets.get(&region.path).copied().unwrap_or_default();
        let next = Point {
            x: (current.x - dx).clamp(0.0, max_x),
            y: (current.y - dy).clamp(0.0, max_y),
        };
        let moved = next != current;
        if moved {
            offsets.insert(region.path.clone(), next);
        }
        moved
    }

    /// Rolagem programática — o PRÓXIMO layout (mesmo frame) já aplica,
    /// com clamp no place.
    pub fn set_scroll_offset(&self, path: &str, offset: Point) {
        self.scroll_offsets.borrow_mut().insert(path.to_string(), offset);
    }

    pub fn scroll_offset(&self, path: &str) -> Point {
        self.scroll_offsets.borrow().get(path).copied().unwrap_or_default()
    }

    // MARK: - Ações nomeadas + keymap

    /// Liga um padrão de tecla a uma ação — rebind sobrescreve. Nenhum
    /// default: o app declara o mapa (os atalhos de edição do campo
    /// continuam sendo do shell). Casamento de modificadores é EXATO.
    pub fn bind(&self, pattern: KeyPattern, action: ActionId) {
        self.keymap.borrow_mut().insert(pattern, action);
    }

    /// O binding do padrão, se houver.
    pub fn match_key(&self, pattern: &KeyPattern) -> Option<ActionId> {
        self.keymap.borrow().get(pattern).copied()
    }

    /// Dispara o handler mais interno vigente. `false` = nenhum handler
    /// montado — quem chamou decide o fallback (o gate deixa a tecla
    /// seguir para o campo).
    pub fn dispatch_action(&self, id: ActionId) -> bool {
        reconciler::run_handler(id)
    }

    // MARK: - Foco e teclado (o campo focado é o dono do teclado)

    pub fn focused(&self) -> Option<String> {
        self.focus.borrow().clone()
    }

    /// Foca um campo. O caret vai para o FIM na primeira vez (o clamp da
    /// estampa resolve o `usize::MAX`); refocar restaura a posição retida.
    pub fn focus(&self, path: &str) {
        self.caret_visible.set(true);
        *self.focus.borrow_mut() = Some(path.to_string());
        self.carets
            .borrow_mut()
            .entry(path.to_string())
            .or_insert(CaretState { caret: usize::MAX, anchor: None, marked: None });
    }

    /// Foca posicionando o caret pelo X do clique — medição de prefixos
    /// com a FONTE efetiva do campo (retida do último layout).
    fn focus_at(&self, path: &str, x: Px) {
        self.caret_visible.set(true);
        *self.focus.borrow_mut() = Some(path.to_string());
        let mut probe = CaretState::default();
        let text = match reconciler::run_editor(path, EditCommand::Read, &mut probe) {
            Some(Some(text)) => text,
            _ => {
                self.carets
                    .borrow_mut()
                    .entry(path.to_string())
                    .or_insert(CaretState { caret: usize::MAX, anchor: None, marked: None });
                return;
            }
        };
        let placement = self
            .last_fields
            .borrow()
            .iter()
            .find(|field| field.path == path)
            .cloned();
        let caret = match placement {
            Some(field) => {
                caret_from_x(&text, x - field.text_origin.x, &field.font, &*self.text, &self.cache)
            }
            None => text.len(),
        };
        self.carets
            .borrow_mut()
            .insert(path.to_string(), CaretState { caret, anchor: None, marked: None });
    }

    pub fn blur(&self) -> bool {
        self.focus.borrow_mut().take().is_some()
    }

    /// Meio-período do blink (o shell chama num timer): alterna a
    /// visibilidade do caret. `true` = há campo focado — repaint.
    pub fn blink(&self) -> bool {
        if self.focus.borrow().is_none() {
            self.caret_visible.set(true);
            return false;
        }
        self.caret_visible.set(!self.caret_visible.get());
        true
    }

    /// O snapshot de IME do campo focado — `None` sem foco. Índices já em
    /// UTF-16 pela borda do framework; o caret rect sai da geometria
    /// retida do último layout.
    pub fn ime_snapshot(&self) -> Option<ImeSnapshot> {
        use crate::text_input::byte_to_utf16;

        let path = self.focus.borrow().clone()?;
        let mut probe = CaretState::default();
        let text = reconciler::run_editor(&path, EditCommand::Read, &mut probe)??;
        let state = self.carets.borrow().get(&path).copied().unwrap_or_default();

        let caret = crate::text_input::clamp_index(&text, state.caret);
        let (start, end) = state.selection().unwrap_or((caret, caret));
        let start_utf16 = byte_to_utf16(&text, start);
        let selected = (start_utf16, byte_to_utf16(&text, end) - start_utf16);
        let marked = state.marked.map(|(start, end)| {
            let start_utf16 = byte_to_utf16(&text, start);
            (start_utf16, byte_to_utf16(&text, end) - start_utf16)
        });

        let field = self
            .last_fields
            .borrow()
            .iter()
            .find(|field| field.path == path)
            .cloned()?;
        let metrics = self.cache.get_or_measure(&text, &field.font, &*self.text);
        let prefix = self.cache.get_or_measure(&text[..caret], &field.font, &*self.text).width;
        let caret_rect = Rect {
            origin: Point { x: field.text_origin.x + prefix, y: field.text_origin.y },
            size: crate::layout::Size { width: 1.5, height: metrics.height() },
        };

        Some(ImeSnapshot { text, selected, marked, caret_rect })
    }

    /// Aplica um comando de edição ao campo focado. A escrita no binding
    /// já sujou quem lê; digitar volta o caret para sólido.
    pub fn key(&self, command: EditCommand) -> Edited {
        let Some(path) = self.focus.borrow().clone() else {
            return Edited { applied: false, output: None };
        };
        let mut state = self.carets.borrow().get(&path).copied().unwrap_or_default();
        // fora do borrow do mapa: o editor escreve no binding e pode
        // re-entrar no runtime
        match reconciler::run_editor(&path, command, &mut state) {
            Some(output) => {
                self.carets.borrow_mut().insert(path, state);
                self.caret_visible.set(true);
                Edited { applied: true, output }
            }
            None => Edited { applied: false, output: None },
        }
    }

    /// Um frame completo para o shell: estabiliza, layout no viewport,
    /// raster no scale — os hits ficam retidos para os eventos. Se o
    /// conteúdo andou sob o ponteiro parado (uma ação inseriu/removeu), o
    /// hover re-resolve contra os hits novos e roda UMA passada extra —
    /// interação sempre resolvida ANTES da passada que a pinta.
    pub fn frame(
        &self,
        root: &impl View,
        size: crate::layout::Size,
        scale: usize,
        background: crate::layout::Color,
    ) -> crate::raster::Bitmap {
        self.settle(root);
        let mut result = self.layout(root, crate::layout::Proposal::exact(size));
        let pointer = self.interaction.borrow().pointer;
        if let Some(point) = pointer
            && self.pointer_moved(point.x, point.y)
        {
            result = self.layout(root, crate::layout::Proposal::exact(size));
        }
        crate::raster::rasterize_with(
            &result.display,
            (size.width.round() as usize) * scale,
            (size.height.round() as usize) * scale,
            scale,
            background,
            &*self.text,
        )
    }

    pub fn render(&self, root: &impl View) -> String {
        // retenção construída sem print não tem linha para expandir —
        // reconstrói uma vez e volta ao incremental normal
        if self.printless.get() {
            reconciler::clear();
            self.printless.set(false);
        }
        crate::view::set_print(true);
        self.render_pass(root)
            .into_nodes()
            .iter()
            .map(|node| reconciler::expand(node).print())
            .collect()
    }

    /// O pass do caminho de FRAME: idêntico ao de print, mas as linhas da
    /// árvore impressa nem são formatadas (imprimir é para gente; frame é
    /// para pixel).
    fn frame_pass(&self, root: &impl View) -> NodeList {
        crate::view::set_print(false);
        self.printless.set(true);
        let nodes = self.render_pass(root);
        crate::view::set_print(true);
        nodes
    }

    /// Layout do frame atual: roda um pass (incremental — árvore estável =
    /// zero bodies), expande a árvore de layout retida e responde à
    /// proposta com os frames por identidade.
    pub fn layout(
        &self,
        root: &impl View,
        proposal: crate::layout::Proposal,
    ) -> crate::layout::LayoutResult {
        // frame ESTÁVEL de raiz-fronteira (hover, wheel, blink, o layout
        // pós-settle): nada sujo, mesmo tema, raiz retida — o walk seria
        // all-skip e emitiria exatamente UMA referência; sintetiza a
        // referência e pula o pass inteiro. Qualquer outra situação anda
        // o pass de verdade.
        let stable_root = (self.root_is_boundary.get()
            && crate::theme::version() == self.theme_version.get()
            && !self.has_pending_dirty())
        .then(|| self.last_root.borrow().clone())
        .flatten()
        .filter(|path| reconciler::is_retained(path));
        let tree = match stable_root {
            Some(path) => {
                // o contrato observável fica: ESTE frame rodou zero bodies
                reconciler::note_stable_frame();
                crate::layout::LayoutNode::BoundaryRef { path }
            }
            None => {
                let mut nodes = self.frame_pass(root);
                let mut roots = nodes.take_layout();
                self.root_is_boundary.set(matches!(
                    roots.as_slice(),
                    [crate::layout::LayoutNode::Boundary { .. }]
                        | [crate::layout::LayoutNode::BoundaryRef { .. }]
                ));
                if roots.len() == 1 {
                    roots.remove(0)
                } else {
                    crate::layout::LayoutNode::Stack {
                        axis: crate::layout::Axis::Vertical,
                        spacing: 0.0,
                        align: crate::layout::CrossAlign::Start,
                        children: roots,
                    }
                }
            }
        };
        // o layout anda pela retenção NO LUGAR (`BoundaryRef` resolve
        // on-the-fly) e o estado de frame vai no ENV — frame nenhum clona
        // árvore nenhuma
        let interaction = self.interaction.borrow().clone();
        let focus = self.focus.borrow().clone();
        let carets = self.carets.borrow();
        let stamp = crate::layout::FrameStamp {
            interaction: &interaction,
            focus: focus.as_deref(),
            carets: &carets,
            caret_visible: self.caret_visible.get(),
        };
        self.cache.begin_frame();
        let offsets = self.scroll_offsets.borrow();
        let result = crate::layout::layout_with(
            &tree,
            proposal,
            LayoutEnv {
                text: &*self.text,
                cache: &self.cache,
                scroll_offsets: &offsets,
                font: FontSpec::DEFAULT,
                stamp,
            },
        );
        drop(offsets);
        drop(carets);
        *self.last_hits.borrow_mut() = result.hits.clone();
        *self.last_scrolls.borrow_mut() = result.scrolls.clone();
        *self.last_fields.borrow_mut() = result.fields.clone();
        result
    }

    /// Força todos os bodies (descarta a retenção antes do pass) — o
    /// oráculo dos testes: o incremental tem que imprimir byte a byte o
    /// que o full imprime.
    pub fn render_full(&self, root: &impl View) -> String {
        reconciler::clear();
        self.render(root)
    }

    /// Um frame completo até o bitmap: layout na proposta exata do
    /// viewport e rasterização da display list — o que o backend de
    /// plataforma blita na janela.
    pub fn paint(&self, root: &impl View, size: crate::layout::Size) -> crate::raster::Bitmap {
        self.paint_at_scale(root, size, 1)
    }

    /// [`Runtime::paint`] em retina: layout em pontos lógicos, bitmap em
    /// pixels físicos (`size × scale`).
    pub fn paint_at_scale(
        &self,
        root: &impl View,
        size: crate::layout::Size,
        scale: usize,
    ) -> crate::raster::Bitmap {
        let result = self.layout(root, crate::layout::Proposal::exact(size));
        crate::raster::rasterize_with(
            &result.display,
            (size.width.round() as usize) * scale,
            (size.height.round() as usize) * scale,
            scale,
            crate::layout::Color::WHITE,
            &*self.text,
        )
    }

    /// Drains registered effects (`onReceive`, `onChange`, `query`).
    /// Returns whether any of them observed a change.
    pub fn pump(&self) -> bool {
        effects::take().iter().any(|effect| effect(&self.ctx))
    }

    /// As views sujadas por `set()` desde a última drenagem — a invalidação
    /// fina (quem LEU a dependência escrita), por caminho de identidade,
    /// escopada ao root deste runtime.
    pub fn take_dirty(&self) -> Vec<String> {
        match self.last_root.borrow().as_deref() {
            Some(root) => motor::identity::take_dirty_matching(root),
            None => motor::identity::take_dirty(),
        }
    }

    /// Instrumentação: os bodies que o último [`Runtime::render`] rodou —
    /// a prova de incrementalidade (o resto veio do cache).
    pub fn body_runs(&self) -> Vec<String> {
        reconciler::last_body_runs()
    }

    /// Render → pump → re-render until the tree is stable (max 8 cycles).
    /// Estável = árvore impressa parada, nenhum efeito observou mudança e
    /// nenhuma view suja. A checagem de sujeira ESPIA sem drenar — a
    /// sujeira pendente é insumo do próximo pass, não do loop.
    pub fn render_stable(&self, root: &impl View) -> String {
        let mut previous = String::new();
        for _ in 0..8 {
            let printed = self.render(root);
            // pump first: side effects fired by THIS render's onAppear
            // nodes must be observed before declaring the tree stable
            let observed_change = self.pump();
            if printed == previous && !observed_change && !self.has_pending_dirty() {
                return printed;
            }
            previous = printed;
        }
        previous
    }

    /// O [`Runtime::render_stable`] do caminho de FRAME: estabiliza SEM
    /// montar a árvore impressa (imprimir é para gente; frame é para
    /// pixel). Estável = o pass não deixou NADA para trás: nenhum efeito
    /// observou mudança e nenhuma view suja — bodies terem rodado não
    /// pede confirmação (um pass sem sujeira nova produziu uma árvore
    /// consistente por definição; o pass seguinte seria all-skip).
    pub fn settle(&self, root: &impl View) {
        for _ in 0..8 {
            self.frame_pass(root);
            let observed_change = self.pump();
            if !observed_change && !self.has_pending_dirty() {
                return;
            }
        }
    }

    fn has_pending_dirty(&self) -> bool {
        match self.last_root.borrow().as_deref() {
            Some(root) => motor::identity::has_dirty_matching(root),
            None => false,
        }
    }
}
