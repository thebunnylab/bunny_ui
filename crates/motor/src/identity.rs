//! Identidade estrutural + posse do estado pelo runtime.
//!
//! O cursor de render mantém o caminho até o ponto atual da árvore —
//! wrapper de view (`CountriesList`), posição na tupla (`#0`), braço de
//! condicional (`@First`), chave de row (`[USA]`), conteúdo de sheet
//! (`sheet`). Esse caminho é a identidade estrutural: é nele que o estado
//! ancora, é por ele que o estado morre, e é ele que o reconciler usa para
//! decidir qual body re-roda.
//!
//! Papéis, uma arena:
//!
//! - **Âncora**: `State::new` DENTRO de um pass de render não aloca às
//!   cegas — pergunta aqui por (escopo de construção, tipo, seq). Se a
//!   identidade já tem o slot, o handle novo aponta para ele e o valor
//!   inicial é descartado (o inicial só semeia o primeiro mount, como o
//!   `@State` do Swift). Fora de render, escopo do app: aloca uma vez e
//!   vive para sempre — o caso dos roots que o app segura.
//! - **Dono**: cada identidade tocada num pass fica viva; ao fim do pass,
//!   identidades do mesmo root que não apareceram são varridas — slots
//!   liberados (geração avança: handle velho falha alto, não lê slot
//!   reciclado), âncoras e slots de efeito removidos. Subárvores que o
//!   reconciler PULOU (cache limpo) contam como vivas sem serem visitadas.
//! - **Grafo de leitura**: `get()` durante render registra "esta view leu
//!   esta dependência" — `State` (slot) ou `Store` (id). O conjunto de
//!   leituras de uma view persiste até o próximo re-render DELA (views
//!   puladas não perdem dependências). `set()`/`send()` marca de sujo
//!   exatamente quem leu.
//!
//! Limite conhecido (documentado, não acidental): âncoras nascem no escopo
//! de CONSTRUÇÃO. Closures de row e de sheet rodam durante o render — com
//! o cursor já dentro da chave — então estado de row segue o item. Mas
//! braços de um mesmo `body` constroem tudo no mesmo escopo: dois braços
//! que construíssem `State` do MESMO tipo colidiriam na âncora. O motor
//! real, com metadados de campo por view, apura isso; o fake escolhe a
//! regra simples e verificável.

use std::any::TypeId;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::runtime::Site;

/// Uma dependência observável: um `State` (pelo id global do slot, nunca
/// reciclado) ou um `Store` inteiro (granularidade de objeto, como um
/// ObservableObject).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DepKey {
    State(u64),
    Store(u64),
}

#[derive(Default)]
struct Registry {
    pass_active: bool,
    /// Primeiro segmento empurrado no pass — define o root varrido.
    pass_root: Option<String>,
    path: Vec<String>,
    /// Só os wrappers de view — o alvo do read-tracking.
    views: Vec<String>,
    touched: HashSet<String>,
    /// Fronteiras que o reconciler pulou neste pass (cache limpo): a
    /// subárvore delas conta como viva na varredura.
    skipped: HashSet<String>,
    /// Fronteiras cujo body RODOU neste pass: dentro delas a varredura
    /// segue a regra normal (o que não apareceu, morreu).
    reran: HashSet<String>,
    /// Identidade → recursos que morrem com ela.
    owners: HashMap<String, OwnerRecord>,
    /// (escopo, tipo, seq) → (índice na arena do tipo, geração, dep-id).
    anchors: HashMap<AnchorKey, (usize, u32, u64)>,
    /// Contadores por pass: quantos `State::new` de cada tipo cada escopo já fez.
    seqs: HashMap<(String, TypeId), u32>,
    /// view → dependências lidas no ÚLTIMO body dela (persiste entre passes).
    reads_by_view: HashMap<String, HashSet<DepKey>>,
    /// índice invertido: dependência → views leitoras.
    readers: HashMap<DepKey, HashSet<String>>,
    dirty: HashSet<String>,
    /// Slots de efeito por (site, escopo) — a retenção de `on_change`/`on_receive`.
    effect_cells: HashMap<(Site, String), Rc<dyn std::any::Any>>,
    next_store_id: u64,
}

type AnchorKey = (String, TypeId, u32);

#[derive(Default)]
struct OwnerRecord {
    /// (tipo, índice na arena do tipo) — a varredura libera via o registro
    /// de arenas sem conhecer o tipo estaticamente.
    slots: Vec<(TypeId, usize)>,
    anchors: Vec<AnchorKey>,
    effect_sites: Vec<(Site, String)>,
}

thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry::default());
}

/// Escopo dos `State` criados fora de qualquer pass (roots do app).
const APP_SCOPE: &str = "@app";

/// Leituras fora de qualquer wrapper de view (a região do root — free fns,
/// custom modifiers no topo). Essa região re-roda em todo pass, então as
/// dependências dela zeram a cada begin.
const ROOT_READER: &str = "@root";

// MARK: - Pass

/// Abre um pass de render: zera contadores de âncora, marca de vivos e as
/// leituras da região do root (que sempre re-roda). As leituras das views
/// retidas FICAM — view pulada não perde dependência. Chamado pelo
/// `Runtime` da camada tipada — o motor espelhado nunca abre pass, então
/// mantém a semântica antiga intacta.
pub fn begin_pass() {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.pass_active = true;
        registry.pass_root = None;
        registry.path.clear();
        registry.views.clear();
        registry.touched.clear();
        registry.skipped.clear();
        registry.reran.clear();
        registry.seqs.clear();
        clear_view_reads(&mut registry, ROOT_READER);
    });
}

/// Fecha o pass e varre. Um dono morre se: está sob o root deste pass, não
/// foi tocado, e a fronteira retida mais próxima acima dele NÃO foi pulada
/// (prefixo mais longo vence: sob um pulo a subárvore vive; sob um body
/// que rodou, vale a regra normal). Devolve os caminhos mortos para o
/// reconciler descartar as entradas retidas correspondentes.
pub fn end_pass() -> Vec<String> {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.pass_active = false;
        // o root fica legível até o próximo begin_pass (o runtime consulta
        // para escopar dirty e efeitos)
        let Some(root) = registry.pass_root.clone() else {
            return Vec::new();
        };
        let prefix = format!("{root}/");
        let under_root =
            |owner: &str| owner == root || owner.starts_with(&prefix);
        let dead: Vec<String> = registry
            .owners
            .keys()
            .filter(|owner| {
                under_root(owner)
                    && !registry.touched.contains(*owner)
                    && !protected_by_skip(&registry, owner)
            })
            .cloned()
            .collect();
        for owner in &dead {
            let Some(record) = registry.owners.remove(owner) else {
                continue;
            };
            for (type_id, index) in record.slots {
                crate::state::free_slot(type_id, index);
            }
            for key in record.anchors {
                registry.anchors.remove(&key);
            }
            for site in record.effect_sites {
                registry.effect_cells.remove(&site);
            }
            clear_view_reads(&mut registry, owner);
            registry.dirty.remove(owner);
        }
        dead
    })
}

/// Prefixo mais longo entre pulados e re-rodados decide: pulado protege,
/// re-rodado (ou nenhum) deixa a regra normal valer.
fn protected_by_skip(registry: &Registry, owner: &str) -> bool {
    let mut best_len = 0usize;
    let mut best_is_skip = false;
    let covers = |candidate: &str| {
        owner == candidate || owner.starts_with(&format!("{candidate}/"))
    };
    for skip in &registry.skipped {
        if covers(skip) && skip.len() > best_len {
            best_len = skip.len();
            best_is_skip = true;
        }
    }
    for rerun in &registry.reran {
        if covers(rerun) && rerun.len() > best_len {
            best_len = rerun.len();
            best_is_skip = false;
        }
    }
    best_is_skip
}

/// O reconciler avisa: esta fronteira foi pulada (cache limpo) — a
/// subárvore dela conta como viva.
pub fn mark_skipped(path: &str) {
    REGISTRY.with(|registry| {
        registry.borrow_mut().skipped.insert(path.to_string());
    });
}

/// O reconciler avisa: o body desta fronteira rodou neste pass.
pub fn mark_reran(path: &str) {
    REGISTRY.with(|registry| {
        registry.borrow_mut().reran.insert(path.to_string());
    });
}

/// Views sujadas por escritas desde a última drenagem — a invalidação
/// fina, exposta para o loop de estabilidade e para os testes.
pub fn take_dirty() -> Vec<String> {
    REGISTRY.with(|registry| {
        let mut dirty: Vec<String> = registry.borrow_mut().dirty.drain().collect();
        dirty.sort();
        dirty
    })
}

/// Drena só a sujeira deste root (mais a região do root, que qualquer pass
/// consome). Sujeira de OUTRO root fica na fila para o render dele.
pub fn take_dirty_matching(root: &str) -> Vec<String> {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let prefix = format!("{root}/");
        let mut matching: Vec<String> = registry
            .dirty
            .iter()
            .filter(|path| *path == ROOT_READER || *path == root || path.starts_with(&prefix))
            .cloned()
            .collect();
        for path in &matching {
            registry.dirty.remove(path);
        }
        matching.sort();
        matching
    })
}

/// Cópia dos sujos agora — o snapshot que decide o pass, sem drenar
/// (escritas DURANTE o pass precisam sobreviver para o próximo ciclo).
pub fn dirty_snapshot() -> HashSet<String> {
    REGISTRY.with(|registry| registry.borrow().dirty.clone())
}

/// Há sujeira pendente deste root? Espia sem drenar — a condição de
/// estabilidade usa isto; quem CONSOME sujeira é o pass de render
/// (snapshot + consume), nunca o loop.
pub fn has_dirty_matching(root: &str) -> bool {
    REGISTRY.with(|registry| {
        let registry = registry.borrow();
        let prefix = format!("{root}/");
        registry
            .dirty
            .iter()
            .any(|path| path == ROOT_READER || path == root || path.starts_with(&prefix))
    })
}

/// Fim do pass: consome do registro a sujeira que este pass atendeu — a
/// interseção do snapshot com o root (e a região do root). O que veio de
/// escritas durante o render fica; o que é de outro root fica.
pub fn consume_dirty(root: &str, snapshot: &HashSet<String>) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let prefix = format!("{root}/");
        for path in snapshot {
            if path == ROOT_READER || path == root || path.starts_with(&prefix) {
                registry.dirty.remove(path);
            }
        }
    });
}

/// O primeiro segmento empurrado no pass corrente (ou no último encerrado).
pub fn current_pass_root() -> Option<String> {
    REGISTRY.with(|registry| registry.borrow().pass_root.clone())
}

/// Os segmentos do cursor agora — a entry retida guarda o caminho do pai
/// para semear re-runs isolados.
pub fn current_path_segments() -> Vec<String> {
    REGISTRY.with(|registry| registry.borrow().path.clone())
}

/// O caminho completo do cursor agora (`None` fora de pass) — a chave com
/// que nós interativos registram suas ações.
pub fn cursor_scope() -> Option<String> {
    REGISTRY.with(|registry| {
        let registry = registry.borrow();
        (registry.pass_active && !registry.path.is_empty()).then(|| registry.path.join("/"))
    })
}

// MARK: - Cursor

/// Um degrau do cursor — solta no drop, então o caminho sobrevive a early
/// returns e panics de debug_assert.
pub struct Frame {
    pops_view: bool,
    active: bool,
}

impl Drop for Frame {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        REGISTRY.with(|registry| {
            let mut registry = registry.borrow_mut();
            registry.path.pop();
            if self.pops_view {
                registry.views.pop();
            }
        });
    }
}

fn push(segment: String, is_view: bool) -> Frame {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        if !registry.pass_active {
            return Frame { pops_view: false, active: false };
        }
        if registry.pass_root.is_none() {
            registry.pass_root = Some(segment.clone());
        }
        registry.path.push(segment);
        let scope = registry.path.join("/");
        registry.touched.insert(scope.clone());
        if is_view {
            registry.views.push(scope);
        }
        Frame { pops_view: is_view, active: true }
    })
}

/// Desce um degrau estrutural: posição de tupla (`#0`), braço (`@First`),
/// chave de row (`[USA]`), conteúdo de sheet (`sheet`).
pub fn enter(segment: impl Into<String>) -> Frame {
    push(segment.into(), false)
}

/// Desce no wrapper de uma view (`Component`) — além do caminho, entra na
/// pilha de views que o read-tracking usa como alvo.
pub fn enter_view(name: impl Into<String>) -> Frame {
    push(name.into(), true)
}

/// O caminho da view mais interna em render — a chave do reconciler.
pub fn current_view_path() -> Option<String> {
    REGISTRY.with(|registry| registry.borrow().views.last().cloned())
}

/// Re-semeia o cursor com o caminho do PAI de uma fronteira retida, para o
/// reconciler re-rodar um body isolado (view suja atrás de pai pulado) com
/// âncoras e identidades corretas. Os frames devolvidos desfazem no drop.
pub fn seed(segments: &[String]) -> Vec<Frame> {
    segments.iter().map(|segment| enter(segment.clone())).collect()
}

fn current_scope(registry: &Registry) -> String {
    if registry.pass_active && !registry.path.is_empty() {
        registry.path.join("/")
    } else {
        APP_SCOPE.to_string()
    }
}

// MARK: - Âncoras de estado

/// O que `State::new` recebe de volta ao declarar estado.
pub(crate) enum Claim {
    /// A identidade já tem esse estado: reusa o slot, descarta o inicial.
    Existing { index: usize, generation: u32, dep: u64 },
    /// Primeiro mount (ou escopo do app): aloca e registra com o token.
    Fresh(AnchorToken),
}

pub(crate) struct AnchorToken {
    key: Option<AnchorKey>,
}

pub(crate) fn claim_anchor(type_id: TypeId) -> Claim {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        if !registry.pass_active {
            return Claim::Fresh(AnchorToken { key: None });
        }
        let scope = current_scope(&registry);
        let seq_key = (scope.clone(), type_id);
        let seq = *registry
            .seqs
            .entry(seq_key)
            .and_modify(|seq| *seq += 1)
            .or_insert(0);
        let key = (scope, type_id, seq);
        match registry.anchors.get(&key) {
            Some(&(index, generation, dep)) => Claim::Existing { index, generation, dep },
            None => Claim::Fresh(AnchorToken { key: Some(key) }),
        }
    })
}

pub(crate) fn fulfill_anchor(token: AnchorToken, index: usize, generation: u32, dep: u64) {
    let Some(key) = token.key else {
        return; // escopo do app: sem âncora, sem dono, vive para sempre
    };
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.anchors.insert(key.clone(), (index, generation, dep));
        let owner = registry.owners.entry(key.0.clone()).or_default();
        owner.slots.push((key.1, index));
        owner.anchors.push(key);
    });
}

// MARK: - Grafo de leitura

/// O body desta view vai (re)rodar: as leituras antigas dela caem — o
/// conjunto novo é o que o body registrar agora.
pub fn begin_view_reads(view: &str) {
    REGISTRY.with(|registry| {
        clear_view_reads(&mut registry.borrow_mut(), view);
    });
}

fn clear_view_reads(registry: &mut Registry, view: &str) {
    let Some(keys) = registry.reads_by_view.remove(view) else {
        return;
    };
    for key in keys {
        if let Some(readers) = registry.readers.get_mut(&key) {
            readers.remove(view);
            if readers.is_empty() {
                registry.readers.remove(&key);
            }
        }
    }
}

pub(crate) fn record_read(key: DepKey) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        if !registry.pass_active {
            return;
        }
        let view = registry
            .views
            .last()
            .cloned()
            .unwrap_or_else(|| ROOT_READER.to_string());
        registry.reads_by_view.entry(view.clone()).or_default().insert(key);
        registry.readers.entry(key).or_default().insert(view);
    });
}

pub(crate) fn record_write(key: DepKey) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let Some(readers) = registry.readers.get(&key).cloned() else {
            return;
        };
        registry.dirty.extend(readers);
    });
}

pub(crate) fn next_store_id() -> u64 {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.next_store_id += 1;
        registry.next_store_id
    })
}

// MARK: - Slots de efeito por identidade

/// O slot de `on_change`/`on_receive`, chaveado por (site, escopo atual):
/// duas instâncias da mesma view no mesmo callsite têm slots separados, e
/// o slot morre com a identidade. Fora de pass cai no escopo do app — o
/// comportamento global de antes.
pub fn scoped_effect_slot<V: 'static>(site: impl Into<Site>) -> Rc<RefCell<Option<V>>> {
    let site = site.into();
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let scope = current_scope(&registry);
        let key = (site, scope.clone());
        if let Some(any) = registry.effect_cells.get(&key).cloned()
            && let Ok(cell) = any.downcast::<RefCell<Option<V>>>()
        {
            return cell;
        }
        let cell: Rc<RefCell<Option<V>>> = Rc::new(RefCell::new(None));
        registry.effect_cells.insert(key.clone(), cell.clone());
        if scope != APP_SCOPE {
            registry.owners.entry(scope).or_default().effect_sites.push(key);
        }
        cell
    })
}
