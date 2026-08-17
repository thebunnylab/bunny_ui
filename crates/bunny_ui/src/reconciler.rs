//! O reconciler — a árvore retida por identidade.
//!
//! Cada fronteira de view (`Component`) deixa aqui uma [`Entry`]: o
//! VALOR da view (re-rodável, apagado atrás de um `Erased`), o `Context`
//! com que rendeu (environment de ancestrais já aplicado), a saída
//! impressa e os efeitos que o body registrou.
//!
//! No render, a fronteira decide: limpa e retida → PULA o body e emite uma
//! referência (a montagem final expande do cache); suja, nova, ou dentro
//! de um body que re-rodou (o pai construiu valores novos — config pode
//! ter mudado) → roda e re-retém. View suja atrás de pai pulado re-roda
//! ISOLADA a partir do valor retido, com o cursor re-semeado no caminho.
//!
//! Efeitos de views puladas continuam bombeando: a fila de cada pass é
//! remontada da retenção (elas são a subscription vigente). `onAppear` de
//! view pulada NÃO dispara — o que aproxima o fake da semântica real
//! (appear é mount, não frame).
//!
//! A saída referencia fronteiras por um marcador na própria linha
//! (`\u{1}caminho\u{1}sufixos…`) — interno à [`NodeList`] opaca; a
//! expansão resolve recursivamente contra a retenção, aplica sufixos de
//! modifier acumulados e re-appende filhos extra (o nó `Sheet`).

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use motor::state::{Context, EffectFn};
use motor::view::RenderNode;

use crate::erased::Erased;
use crate::layout::LayoutNode;
use crate::text_input::{CaretState, EditCommand};

/// Uma ação interativa registrada durante o render: (caminho do alvo, o
/// que o clique dispara).
pub(crate) type ActionEntry = (String, Rc<dyn Fn()>);

/// O editor de um campo de texto: aplica um comando ao par
/// (binding, caret) e devolve a saída de `Read`/`Copy`/`Cut`. Retido como
/// as ações — campo de view pulada edita.
pub(crate) type EditorFn = Rc<dyn Fn(EditCommand, &mut CaretState) -> Option<String>>;
pub(crate) type EditorEntry = (String, EditorFn);

/// Handler de ação NOMEADA registrado no render: (caminho do registro,
/// id, o que roda). Retido como as ações — handler de view pulada vive.
pub(crate) type HandlerFn = Rc<dyn Fn()>;
pub(crate) type HandlerEntry = (String, crate::action::ActionId, HandlerFn);

pub(crate) struct Entry {
    pub value: Erased,
    pub ctx: Context,
    pub node: RenderNode,
    /// A árvore de layout do body — retida junto com o print (as duas
    /// saídas do mesmo body-eval).
    pub layout: LayoutNode,
    pub effects: Vec<EffectFn>,
    /// As ações interativas do body — retidas como os efeitos: botão de
    /// view pulada continua clicável.
    pub actions: Vec<ActionEntry>,
    /// Os editores de campo do body — mesma retenção.
    pub editors: Vec<EditorEntry>,
    /// Os handlers de ação nomeada do body — mesma retenção.
    pub handlers: Vec<HandlerEntry>,
    /// Segmentos do caminho do PAI — a semente do cursor num re-run isolado.
    pub parent_segments: Vec<String>,
}

#[derive(Default)]
struct BuildingFrame {
    path: String,
    effects: Vec<EffectFn>,
    actions: Vec<ActionEntry>,
    editors: Vec<EditorEntry>,
    handlers: Vec<HandlerEntry>,
}

#[derive(Default)]
struct PassState {
    active: bool,
    /// Snapshot dos sujos no início do pass — decide quem re-roda.
    dirty: HashSet<String>,
    /// Pilha de entries em construção (o topo coleta efeitos e ações).
    building: Vec<BuildingFrame>,
    /// Efeitos da região do root (fora de qualquer fronteira) — re-rodam
    /// a cada walk.
    root_effects: Vec<EffectFn>,
    root_actions: Vec<ActionEntry>,
    root_editors: Vec<EditorEntry>,
    root_handlers: Vec<HandlerEntry>,
    /// Instrumentação: bodies que rodaram neste pass.
    body_runs: Vec<String>,
    /// Fronteiras PULADAS neste pass — a subtree de uma pulada sobrevive
    /// à varredura de entries (o walk não entrou nela de propósito).
    skipped: Vec<String>,
}

thread_local! {
    static RETAINED: RefCell<BTreeMap<String, Entry>> = const { RefCell::new(BTreeMap::new()) };
    static PASS: RefCell<PassState> = RefCell::new(PassState::default());
    static LAST_BODY_RUNS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// A árvore de layout retida de uma fronteira, emprestada no lugar — o
/// measure e o place resolvem `BoundaryRef` por aqui, SEM costurar cópia
/// expandida. Empréstimos aninham (ref dentro de ref = borrows
/// compartilhados do mesmo RefCell); nenhum body roda durante o layout,
/// então não há re-empréstimo mutável possível.
pub(crate) fn with_retained_layout<R>(
    path: &str,
    reader: impl FnOnce(Option<&LayoutNode>) -> R,
) -> R {
    RETAINED.with(|retained| {
        let retained = retained.borrow();
        reader(retained.get(path).map(|entry| &entry.layout))
    })
}

/// A fronteira está retida? (O guarda do frame estável do `Runtime`.)
pub(crate) fn is_retained(path: &str) -> bool {
    RETAINED.with(|retained| retained.borrow().contains_key(path))
}

/// Registra que o frame corrente foi servido SEM pass (raiz estável
/// sintetizada) — o contrato observável de `body_runs` continua: este
/// frame rodou zero bodies.
pub(crate) fn note_stable_frame() {
    LAST_BODY_RUNS.with(|last| last.borrow_mut().clear());
}

const REF_MARK: char = '\u{1}';

pub(crate) fn begin_pass(dirty: HashSet<String>) {
    PASS.with(|pass| {
        *pass.borrow_mut() = PassState {
            active: true,
            dirty,
            ..PassState::default()
        };
    });
}

pub(crate) enum Decision {
    Skip,
    Render,
}

/// Fronteira alcançada no walk: pula se está limpa, retida, e nenhum body
/// acima dela rodou neste pass (um pai que rodou construiu valores novos —
/// a config pode ter mudado sem passar por `State`).
pub(crate) fn decide(path: &str) -> Decision {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        if !pass.active {
            return Decision::Render;
        }
        let inside_rerun = !pass.building.is_empty();
        let retained = RETAINED.with(|retained| retained.borrow().contains_key(path));
        if !inside_rerun && retained && !pass.dirty.contains(path) {
            pass.skipped.push(path.to_string());
            Decision::Skip
        } else {
            Decision::Render
        }
    })
}

pub(crate) fn begin_entry(path: &str) {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        pass.body_runs.push(path.to_string());
        pass.building.push(BuildingFrame { path: path.to_string(), ..Default::default() });
    });
}

pub(crate) fn finish_entry(
    path: &str,
    value: Erased,
    ctx: Context,
    node: RenderNode,
    layout: LayoutNode,
) {
    let (effects, actions, editors, handlers) = PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        match pass.building.pop() {
            Some(frame) => {
                debug_assert_eq!(frame.path, path, "entries fecham na ordem que abrem");
                (frame.effects, frame.actions, frame.editors, frame.handlers)
            }
            None => (Vec::new(), Vec::new(), Vec::new(), Vec::new()),
        }
    });
    let parent_segments = motor::identity::current_path_segments()
        .split_last()
        .map(|(_, parents)| parents.to_vec())
        .unwrap_or_default();
    RETAINED.with(|retained| {
        retained.borrow_mut().insert(
            path.to_string(),
            Entry {
                value,
                ctx,
                node,
                layout,
                effects,
                actions,
                editors,
                handlers,
                parent_segments,
            },
        );
    });
}

/// Um efeito registrado durante o render: vai para a entry em construção,
/// ou para a região do root quando não há fronteira aberta.
pub(crate) fn attribute_effect(effect: EffectFn) {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        if let Some(frame) = pass.building.last_mut() {
            frame.effects.push(effect);
        } else {
            pass.root_effects.push(effect);
        }
    });
}

/// Uma ação interativa registrada durante o render — mesma atribuição.
pub(crate) fn attribute_action(path: String, action: Rc<dyn Fn()>) {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        if let Some(frame) = pass.building.last_mut() {
            frame.actions.push((path, action));
        } else {
            pass.root_actions.push((path, action));
        }
    });
}

/// Um editor de campo registrado durante o render — mesma atribuição.
pub(crate) fn attribute_editor(path: String, editor: EditorFn) {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        if let Some(frame) = pass.building.last_mut() {
            frame.editors.push((path, editor));
        } else {
            pass.root_editors.push((path, editor));
        }
    });
}

/// Um handler de ação nomeada registrado durante o render — mesma
/// atribuição das ações: entry em construção, ou região do root.
pub(crate) fn attribute_handler(path: String, id: crate::action::ActionId, handler: HandlerFn) {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        if let Some(frame) = pass.building.last_mut() {
            frame.handlers.push((path, id, handler));
        } else {
            pass.root_handlers.push((path, id, handler));
        }
    });
}

thread_local! {
    /// O mapa de handlers vigente: id → (profundidade do registro,
    /// handler). Remontado por pass, como ações e editores — o mapa é
    /// estampa do pass, nunca estado retido de interação.
    static HANDLERS: RefCell<HashMap<crate::action::ActionId, (usize, HandlerFn)>> =
        RefCell::new(HashMap::new());
}

/// Remonta o mapa de handlers da retenção sob o root + região do root.
/// Precedência: o caminho mais FUNDO vence (mais interno na árvore);
/// empate de profundidade → o montado por último (determinístico pela
/// ordem da retenção, documentado como NÃO-contratual — o desempate
/// semântico chega com key contexts).
pub(crate) fn assemble_handlers(root: &str) {
    let mut map: HashMap<crate::action::ActionId, (usize, HandlerFn)> = HashMap::new();
    let place = |map: &mut HashMap<crate::action::ActionId, (usize, HandlerFn)>,
                     path: &str,
                     id: crate::action::ActionId,
                     handler: HandlerFn| {
        let depth = path.split('/').count();
        match map.get(&id) {
            Some((existing, _)) if *existing > depth => {}
            _ => {
                map.insert(id, (depth, handler));
            }
        }
    };
    RETAINED.with(|retained| {
        for (path, entry) in retained.borrow().iter() {
            if covers(root, path) {
                for (key, id, handler) in &entry.handlers {
                    place(&mut map, key, *id, handler.clone());
                }
            }
        }
    });
    PASS.with(|pass| {
        for (key, id, handler) in std::mem::take(&mut pass.borrow_mut().root_handlers) {
            place(&mut map, &key, id, handler);
        }
    });
    HANDLERS.with(|handlers| *handlers.borrow_mut() = map);
}

/// Roda o handler mais interno do id. `false` = ninguém registrou — a
/// tecla NÃO é consumida (segue para o campo/sistema de input).
pub(crate) fn run_handler(id: crate::action::ActionId) -> bool {
    let handler = HANDLERS
        .with(|handlers| handlers.borrow().get(&id).map(|(_, handler)| handler.clone()));
    match handler {
        Some(handler) => {
            // fora do borrow: o handler pode escrever estado à vontade
            handler();
            true
        }
        None => false,
    }
}

/// Views sujas que o walk não alcançou (pai pulado): re-roda cada uma a
/// partir do valor retido, com o cursor semeado no caminho do pai —
/// ancestral primeiro, porque o re-run de um pai cobre os descendentes.
pub(crate) fn run_isolated(root: &str) {
    let mut pending: Vec<String> = PASS.with(|pass| {
        let pass = pass.borrow();
        pass.dirty
            .iter()
            .filter(|path| {
                covers(root, path) && !pass.body_runs.iter().any(|ran| covers(ran, path))
            })
            .cloned()
            .collect()
    });
    pending.sort_by_key(|path| path.len());

    for path in pending {
        let already_ran = PASS.with(|pass| {
            pass.borrow().body_runs.iter().any(|ran| covers(ran, &path))
        });
        if already_ran {
            continue;
        }
        let Some((value, ctx, parents)) = RETAINED.with(|retained| {
            retained.borrow().get(&path).map(|entry| {
                (entry.value.clone(), entry.ctx.clone(), entry.parent_segments.clone())
            })
        }) else {
            continue; // suja mas nunca montada (ou já varrida): nada a re-rodar
        };
        let _frames = motor::identity::seed(&parents);
        let mut scratch = crate::view::NodeList::new();
        use crate::view::View;
        // o valor retido re-renderiza pelo caminho normal do blanket: a
        // fronteira está no snapshot de sujos, então roda e re-retém
        value.render_into(&ctx, &mut scratch);
    }
}

fn covers(ancestor: &str, path: &str) -> bool {
    path == ancestor || path.starts_with(&format!("{ancestor}/"))
}

/// A fila de efeitos do pass: região do root + a retenção inteira sob o
/// root atual (pulada ou não — efeito retido é subscription vigente).
pub(crate) fn assemble_effects(root: &str) -> Vec<EffectFn> {
    let mut queue = PASS.with(|pass| std::mem::take(&mut pass.borrow_mut().root_effects));
    RETAINED.with(|retained| {
        for (path, entry) in retained.borrow().iter() {
            if covers(root, path) {
                queue.extend(entry.effects.iter().cloned());
            }
        }
    });
    queue
}

thread_local! {
    /// O mapa de cliques vigente: caminho do alvo → ação. Remontado a cada
    /// pass, como a fila de efeitos.
    static ACTIONS: RefCell<HashMap<String, Rc<dyn Fn()>>> = RefCell::new(HashMap::new());
}

/// Remonta o mapa de cliques da retenção sob o root (botão de view pulada
/// continua clicável) + região do root.
pub(crate) fn assemble_actions(root: &str) {
    let mut map: HashMap<String, Rc<dyn Fn()>> = HashMap::new();
    RETAINED.with(|retained| {
        for (path, entry) in retained.borrow().iter() {
            if covers(root, path) {
                for (key, action) in &entry.actions {
                    map.insert(key.clone(), action.clone());
                }
            }
        }
    });
    PASS.with(|pass| {
        for (key, action) in std::mem::take(&mut pass.borrow_mut().root_actions) {
            map.insert(key, action);
        }
    });
    ACTIONS.with(|actions| *actions.borrow_mut() = map);
}

/// Dispara a ação do alvo (a chave vem do hit-test). `false` = alvo não
/// registrado (identidade morreu entre o frame e o clique — inofensivo).
pub(crate) fn run_action(path: &str) -> bool {
    let action = ACTIONS.with(|actions| actions.borrow().get(path).cloned());
    match action {
        Some(action) => {
            action();
            true
        }
        None => false,
    }
}

thread_local! {
    /// O mapa de editores de campo vigente — remontado por pass, como as
    /// ações.
    static EDITORS: RefCell<HashMap<String, EditorFn>> = RefCell::new(HashMap::new());
}

/// Remonta o mapa de editores da retenção sob o root + região do root.
pub(crate) fn assemble_editors(root: &str) {
    let mut map: HashMap<String, EditorFn> = HashMap::new();
    RETAINED.with(|retained| {
        for (path, entry) in retained.borrow().iter() {
            if covers(root, path) {
                for (key, editor) in &entry.editors {
                    map.insert(key.clone(), editor.clone());
                }
            }
        }
    });
    PASS.with(|pass| {
        for (key, editor) in std::mem::take(&mut pass.borrow_mut().root_editors) {
            map.insert(key, editor);
        }
    });
    EDITORS.with(|editors| *editors.borrow_mut() = map);
}

/// O alvo é um campo de texto? (decide se um clique FOCA em vez de agir)
pub(crate) fn has_editor(path: &str) -> bool {
    EDITORS.with(|editors| editors.borrow().contains_key(path))
}

/// Aplica um comando ao campo — o closure retido é quem alcança o
/// binding. `None` externo = campo não registrado; o `Option` interno é a
/// saída do comando.
pub(crate) fn run_editor(
    path: &str,
    command: EditCommand,
    state: &mut CaretState,
) -> Option<Option<String>> {
    let editor = EDITORS.with(|editors| editors.borrow().get(path).cloned());
    editor.map(|editor| editor(command, state))
}

/// Identidades varridas pelo `end_pass`: as entries delas caem juntas.
pub(crate) fn forget(dead: &[String]) {
    RETAINED.with(|retained| {
        let mut retained = retained.borrow_mut();
        for path in dead {
            retained.remove(path);
        }
    });
}

/// A varredura-GÊMEA da de identidade, para views SEM estado próprio: a
/// varredura do motor só conhece fronteiras com slots/âncoras (owners);
/// uma view apátrida que desmonta deixaria a entry retida — e com ela
/// handlers/ações/editores ZUMBIS respondendo depois do desmonte. Regra:
/// sob o root, sobrevive quem re-rodou, quem foi pulado, ou quem vive sob
/// uma fronteira PULADA (o walk não entrou nela de propósito). Descendente
/// não-visitado de um pai que RE-RODOU está morto — o pai revisitou os
/// filhos vivos um a um.
pub(crate) fn sweep_stale(root: &str) {
    let (runs, skipped) = PASS.with(|pass| {
        let pass = pass.borrow();
        (
            pass.body_runs.iter().cloned().collect::<HashSet<String>>(),
            pass.skipped.clone(),
        )
    });
    RETAINED.with(|retained| {
        retained.borrow_mut().retain(|path, _| {
            if !covers(root, path) {
                return true; // outra árvore montada na mesma thread
            }
            runs.contains(path) || skipped.iter().any(|skip| covers(skip, path))
        });
    });
}

/// Descarta a retenção inteira — o próximo pass roda todos os bodies (o
/// `render_full` dos testes; o estado nas arenas de identidade fica).
pub(crate) fn clear() {
    RETAINED.with(|retained| retained.borrow_mut().clear());
}

pub(crate) fn end_pass() {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        pass.active = false;
        let runs = std::mem::take(&mut pass.body_runs);
        LAST_BODY_RUNS.with(|last| *last.borrow_mut() = runs);
    });
}

/// Instrumentação: os bodies que rodaram no último pass (caminhos de
/// identidade) — a prova de incrementalidade nos testes.
pub(crate) fn last_body_runs() -> Vec<String> {
    LAST_BODY_RUNS.with(|last| last.borrow().clone())
}

// MARK: - Referências e expansão

pub(crate) fn ref_line(path: &str) -> String {
    format!("{REF_MARK}{path}{REF_MARK}")
}

fn parse_ref(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix(REF_MARK)?;
    let end = rest.find(REF_MARK)?;
    Some((&rest[..end], &rest[end + REF_MARK.len_utf8()..]))
}

/// Resolve referências contra a retenção: expande o nó retido (recursivo —
/// o cache também referencia), re-aplica os sufixos de modifier acumulados
/// na linha da referência e re-appende filhos extra (o nó `Sheet` que o
/// modifier pendura na fronteira).
pub(crate) fn expand(node: &RenderNode) -> RenderNode {
    if let Some((path, suffix)) = parse_ref(&node.line) {
        let retained = RETAINED.with(|retained| {
            retained.borrow().get(path).map(|entry| entry.node.clone())
        });
        let Some(inner) = retained else {
            debug_assert!(false, "referência a fronteira sem retenção: {path}");
            return RenderNode::leaf("");
        };
        let mut expanded = expand(&inner);
        expanded.line.push_str(suffix);
        for child in &node.children {
            expanded.children.push(expand(child));
        }
        expanded
    } else {
        RenderNode {
            line: node.line.clone(),
            children: node.children.iter().map(expand).collect(),
        }
    }
}
