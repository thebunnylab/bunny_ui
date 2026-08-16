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

use std::cell::RefCell;

use motor::state::{Context, EnvironmentValues};

use crate::effects;
use crate::layout::{Interaction, Point, Px, Rect};
use crate::reconciler;
use crate::view::{NodeList, View};

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
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

impl Runtime {
    pub fn new() -> Self {
        Runtime {
            ctx: Context::default(),
            last_root: RefCell::new(None),
            last_hits: RefCell::new(Vec::new()),
            interaction: RefCell::new(Interaction::default()),
        }
    }

    pub fn with_environment(values: EnvironmentValues) -> Self {
        let mut ctx = Context::default();
        ctx.values = values;
        Runtime {
            ctx,
            last_root: RefCell::new(None),
            last_hits: RefCell::new(Vec::new()),
            interaction: RefCell::new(Interaction::default()),
        }
    }

    pub fn context(&self) -> Context {
        self.ctx.clone()
    }

    /// Um pass incremental: walk com pulos, re-runs isolados dos sujos que
    /// o walk não alcançou, remontagem da fila de efeitos e varredura.
    /// Devolve as duas saídas (print e layout) ainda com referências.
    fn render_pass(&self, root: &impl View) -> NodeList {
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
            effects::set_queue(reconciler::assemble_effects(pass_root));
            reconciler::assemble_actions(pass_root);
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

    /// Botão subiu: dispara a ação SE soltou dentro do alvo pressionado.
    /// Devolve o caminho disparado; o visual de pressed limpa sempre.
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
            Some(path) if self.activate(&path) => Some(path),
            _ => None,
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
        self.render_stable(root);
        let mut result = self.layout(root, crate::layout::Proposal::exact(size));
        let pointer = self.interaction.borrow().pointer;
        if let Some(point) = pointer
            && self.pointer_moved(point.x, point.y)
        {
            result = self.layout(root, crate::layout::Proposal::exact(size));
        }
        crate::raster::rasterize_scaled(
            &result.display,
            (size.width.round() as usize) * scale,
            (size.height.round() as usize) * scale,
            scale,
            background,
        )
    }

    pub fn render(&self, root: &impl View) -> String {
        self.render_pass(root)
            .into_nodes()
            .iter()
            .map(|node| reconciler::expand(node).print())
            .collect()
    }

    /// Layout do frame atual: roda um pass (incremental — árvore estável =
    /// zero bodies), expande a árvore de layout retida e responde à
    /// proposta com os frames por identidade.
    pub fn layout(
        &self,
        root: &impl View,
        proposal: crate::layout::Proposal,
    ) -> crate::layout::LayoutResult {
        let mut nodes = self.render_pass(root);
        // a interação é estampada na CÓPIA expandida — a retenção nunca
        // guarda estado de ponteiro
        let interaction = self.interaction.borrow().clone();
        let mut roots: Vec<crate::layout::LayoutNode> = nodes
            .take_layout()
            .iter()
            .map(|node| reconciler::expand_layout(node, &interaction))
            .collect();
        let tree = if roots.len() == 1 {
            roots.remove(0)
        } else {
            crate::layout::LayoutNode::Stack {
                axis: crate::layout::Axis::Vertical,
                spacing: 0.0,
                align: crate::layout::CrossAlign::Start,
                children: roots,
            }
        };
        let result = crate::layout::layout(&tree, proposal);
        *self.last_hits.borrow_mut() = result.hits.clone();
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
        crate::raster::rasterize_scaled(
            &result.display,
            (size.width.round() as usize) * scale,
            (size.height.round() as usize) * scale,
            scale,
            crate::layout::Color::WHITE,
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

    fn has_pending_dirty(&self) -> bool {
        match self.last_root.borrow().as_deref() {
            Some(root) => motor::identity::has_dirty_matching(root),
            None => false,
        }
    }
}
