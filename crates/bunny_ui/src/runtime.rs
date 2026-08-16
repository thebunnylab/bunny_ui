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
use crate::reconciler;
use crate::view::{NodeList, View};

pub struct Runtime {
    ctx: Context,
    /// O root do último pass — escopa `take_dirty` para não drenar sujeira
    /// de outra árvore montada na mesma thread.
    last_root: RefCell<Option<String>>,
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
        }
    }

    pub fn with_environment(values: EnvironmentValues) -> Self {
        let mut ctx = Context::default();
        ctx.values = values;
        Runtime {
            ctx,
            last_root: RefCell::new(None),
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
        let mut roots: Vec<crate::layout::LayoutNode> = nodes
            .take_layout()
            .iter()
            .map(reconciler::expand_layout)
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
        crate::layout::layout(&tree, proposal)
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
