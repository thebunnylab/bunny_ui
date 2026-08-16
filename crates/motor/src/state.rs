//! Property-wrapper machinery: `@State`, `@Binding`, `@Environment`,
//! plus the `Context` / `EnvironmentValues` they resolve against.

use crate::combine::Store;
use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::rc::Rc;

/// A side effect collected while rendering (drained by `Runtime::pump`).
/// Returns whether it observed a state change (i.e. a re-render is due).
pub type EffectFn = Rc<dyn Fn(&Context) -> bool>;

/// Fake `Locale` (`import Foundation`).
#[derive(Clone, Debug, PartialEq)]
pub struct Locale {
    pub identifier: String,
}

impl Locale {
    pub fn new(identifier: impl Into<String>) -> Self {
        Locale { identifier: identifier.into() }
    }

    /// `Locale.shortIdentifier` — first two components.
    pub fn shortIdentifier(&self) -> String {
        self.identifier.chars().take(2).collect()
    }
}

impl Default for Locale {
    fn default() -> Self {
        Locale { identifier: "en".into() } // Locale.backendDefault
    }
}

/// Everything `@Environment(\.key)` can read. App-specific values (the DI
/// container, the SwiftData model container) ride along type-erased, exactly
/// like `@Entry` extensions do in real SwiftUI.
#[derive(Clone, Default)]
pub struct EnvironmentValues {
    pub locale: Locale,
    /// `\.injected` — `Rc<DIContainer>` in the app.
    pub injected: Option<Rc<dyn Any>>,
    /// `\.modelContext` stand-in: resolves `Query<T>` sources by type name.
    pub querySource: Option<Rc<dyn Fn(&'static str) -> Option<Rc<dyn Any>>>>,
}

/// Render-time context: environment values + collected effects.
#[derive(Clone, Default)]
pub struct Context {
    pub values: EnvironmentValues,
    pub(crate) effects: Rc<RefCell<Vec<EffectFn>>>,
}

impl Context {
    pub fn environment<T: FromEnvironment>(&self) -> T {
        T::from_environment(&self.values)
    }
}

/// `@Environment(\.key) var x: T` — resolvable from `EnvironmentValues`.
pub trait FromEnvironment: Clone + 'static {
    fn from_environment(values: &EnvironmentValues) -> Self;
}

impl FromEnvironment for Locale {
    fn from_environment(values: &EnvironmentValues) -> Self {
        values.locale.clone()
    }
}

/// `@Environment(\.key) private var x: T`
pub struct Environment<T: FromEnvironment> {
    _phantom: PhantomData<fn() -> T>,
}

impl<T: FromEnvironment> Environment<T> {
    pub fn new() -> Self {
        Environment { _phantom: PhantomData }
    }

    pub fn wrappedValue(&self, ctx: &Context) -> T {
        T::from_environment(&ctx.values)
    }
}

impl<T: FromEnvironment> Default for Environment<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: FromEnvironment> Clone for Environment<T> {
    fn clone(&self) -> Self {
        Self::new()
    }
}

/// `@State private var x = value` — value-type view, arena-backed storage.
///
/// O handle é `Copy` (índice + geração + id de dependência): closures
/// capturam cópias implicitamente, como structs Swift — sem o cortejo de
/// `let this = self.clone()` por closure.
///
/// A posse é do runtime, ancorada na identidade estrutural
/// ([`crate::identity`]): `new` DENTRO de um pass de render é declaração —
/// se a identidade já montou esse estado, o handle aponta para o slot vivo
/// e o valor inicial é descartado (o inicial só semeia o primeiro mount,
/// como o `@State` do Swift). Identidade que sai da árvore leva o slot
/// junto; a geração avança e um handle retido falha alto em vez de ler
/// slot reciclado. Fora de render (roots que o app segura), o slot é do
/// app e vive para sempre.
///
/// O storage é uma **arena por tipo** (`Vec<Slot<T>>` com valores inline):
/// o caminho quente indexa slots tipados sem downcast por valor nem caixa
/// por slot — o apagamento recua para a borda fria (o registro de arenas e
/// o ponteiro de função que a varredura usa para liberar sem conhecer `T`).
pub struct State<T> {
    index: usize,
    generation: u32,
    /// Identidade para o grafo de leitura — global, nunca reciclada.
    dep: u64,
    _marker: PhantomData<fn() -> T>,
}

struct TypedSlot<T> {
    generation: u32,
    value: Option<T>,
}

struct TypedArena<T> {
    slots: Vec<TypedSlot<T>>,
    free: Vec<usize>,
}

impl<T> TypedArena<T> {
    fn new() -> Self {
        TypedArena { slots: Vec::new(), free: Vec::new() }
    }

    fn alloc(&mut self, value: T) -> (usize, u32) {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index];
            slot.value = Some(value);
            (index, slot.generation)
        } else {
            self.slots.push(TypedSlot { generation: 0, value: Some(value) });
            (self.slots.len() - 1, 0)
        }
    }

    /// Geração avança (handle retido não enxerga o slot reciclado) e o
    /// valor morre agora.
    fn free(&mut self, index: usize) {
        if let Some(slot) = self.slots.get_mut(index) {
            slot.generation += 1;
            slot.value = None;
            self.free.push(index);
        }
    }
}

thread_local! {
    /// Uma arena por `TypeId` — o `dyn Any` embrulha a ARENA (borda fria,
    /// um downcast por acesso ao contêiner tipado), nunca o valor.
    static ARENAS: RefCell<HashMap<TypeId, Rc<dyn Any>>> = RefCell::new(HashMap::new());
    /// Como a varredura libera sem conhecer `T`: um ponteiro de função por
    /// tipo, registrado quando a arena nasce.
    static FREERS: RefCell<HashMap<TypeId, fn(usize)>> = RefCell::new(HashMap::new());
    static NEXT_DEP: Cell<u64> = const { Cell::new(0) };
}

fn with_arena<T: 'static, R>(f: impl FnOnce(&mut TypedArena<T>) -> R) -> R {
    let cell = ARENAS.with(|arenas| {
        let mut arenas = arenas.borrow_mut();
        arenas
            .entry(TypeId::of::<T>())
            .or_insert_with(|| {
                FREERS.with(|freers| {
                    freers.borrow_mut().insert(TypeId::of::<T>(), free_typed::<T>)
                });
                Rc::new(RefCell::new(TypedArena::<T>::new())) as Rc<dyn Any>
            })
            .clone()
            .downcast::<RefCell<TypedArena<T>>>()
            .expect("arena registrada por TypeId é sempre do próprio tipo")
    });
    let result = f(&mut cell.borrow_mut());
    result
}

fn free_typed<T: 'static>(index: usize) {
    with_arena::<T, _>(|arena| arena.free(index));
}

/// A varredura de identidades mortas passa por aqui.
pub(crate) fn free_slot(type_id: TypeId, index: usize) {
    let Some(freer) = FREERS.with(|freers| freers.borrow().get(&type_id).copied()) else {
        return;
    };
    freer(index);
}

const DEAD_STATE: &str = "State de uma identidade desmontada (ou de outra thread) — \
                          o slot morreu junto com a view";

impl<T> Clone for State<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for State<T> {}

impl<T: Clone + 'static> State<T> {
    pub fn new(value: T) -> Self {
        match crate::identity::claim_anchor(TypeId::of::<T>()) {
            crate::identity::Claim::Existing { index, generation, dep } => {
                // identidade já montada: revive o slot, descarta o inicial
                State { index, generation, dep, _marker: PhantomData }
            }
            crate::identity::Claim::Fresh(token) => {
                let dep = NEXT_DEP.with(|next| {
                    let dep = next.get();
                    next.set(dep + 1);
                    dep
                });
                let (index, generation) = with_arena::<T, _>(|arena| arena.alloc(value));
                crate::identity::fulfill_anchor(token, index, generation, dep);
                State { index, generation, dep, _marker: PhantomData }
            }
        }
    }

    pub fn wrappedValue(&self) -> T {
        crate::identity::record_read(crate::identity::DepKey::State(self.dep));
        with_arena::<T, _>(|arena| {
            arena
                .slots
                .get(self.index)
                .filter(|slot| slot.generation == self.generation)
                .and_then(|slot| slot.value.clone())
                .expect(DEAD_STATE)
        })
    }

    pub fn set(&self, value: T) {
        with_arena::<T, _>(|arena| {
            let slot = arena
                .slots
                .get_mut(self.index)
                .filter(|slot| slot.generation == self.generation)
                .expect(DEAD_STATE);
            slot.value = Some(value);
        });
        crate::identity::record_write(crate::identity::DepKey::State(self.dep));
    }

    /// Mutação composta. O valor sai da arena enquanto `f` roda (a arena
    /// não fica emprestada durante código do usuário — `f` pode ler OUTROS
    /// `State`s do mesmo tipo); acesso reentrante ao MESMO slot falha alto.
    pub fn update<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let mut value = with_arena::<T, _>(|arena| {
            arena
                .slots
                .get_mut(self.index)
                .filter(|slot| slot.generation == self.generation)
                .and_then(|slot| slot.value.take())
                .expect(DEAD_STATE)
        });
        let result = f(&mut value);
        with_arena::<T, _>(|arena| {
            let slot = arena
                .slots
                .get_mut(self.index)
                .filter(|slot| slot.generation == self.generation)
                .expect(DEAD_STATE);
            slot.value = Some(value);
        });
        crate::identity::record_write(crate::identity::DepKey::State(self.dep));
        result
    }

    /// `$x` — the binding projection.
    pub fn binding(&self) -> Binding<T> {
        let for_get = *self;
        let for_set = *self;
        Binding::new(move || for_get.wrappedValue(), move |value| for_set.set(value))
    }
}

/// `Binding<T>` — a get/set pair (`$x`, `@Binding`, `Binding.dispatched`).
pub struct Binding<T> {
    get: Rc<dyn Fn() -> T>,
    set: Rc<dyn Fn(T)>,
}

impl<T: Clone + 'static> Binding<T> {
    pub fn new(get: impl Fn() -> T + 'static, set: impl Fn(T) + 'static) -> Self {
        Binding { get: Rc::new(get), set: Rc::new(set) }
    }

    pub fn wrappedValue(&self) -> T {
        (self.get)()
    }

    pub fn set(&self, value: T) {
        (self.set)(value)
    }

    /// Compound mutation through the binding (`b.wrappedValue.field = v`).
    pub fn update<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let mut value = self.wrappedValue();
        let result = f(&mut value);
        self.set(value);
        result
    }

    /// `Binding.onSet { … }`
    pub fn onSet(self, perform: impl Fn(&T) + 'static) -> Self {
        let Binding { get, set } = self;
        let old_set = set;
        Binding { get, set: Rc::new(move |value| { old_set(value.clone()); perform(&value) }) }
    }

    /// `binding.field` — Swift's dynamic-member projection on `Binding<T>`
    /// (`routingBinding.detailsSheet`, a `Binding<Bool>` out of a
    /// `Binding<Routing>`).
    pub fn member<B: Clone + 'static>(
        &self,
        get: impl Fn(&T) -> B + 'static,
        set: impl Fn(&mut T, B) + 'static,
    ) -> Binding<B> {
        let old_get = self.get.clone();
        let old_get2 = self.get.clone();
        let old_set = self.set.clone();
        Binding::new(
            move || get(&(old_get)()),
            move |value| {
                let mut whole = (old_get2)();
                set(&mut whole, value);
                (old_set)(whole);
            },
        )
    }

    /// `Binding.dispatched(to: store, \.keyPath)`
    pub fn dispatched<S: Clone + 'static>(
        store: &Store<S>,
        get: impl Fn(&S) -> T + 'static,
        set: impl Fn(&mut S, T) + 'static,
    ) -> Self {
        let store_for_get = store.clone();
        let store_for_set = store.clone();
        Binding::new(
            move || get(&store_for_get.value()),
            move |value| store_for_set.update(|state| set(state, value)),
        )
    }
}

impl<T: Clone + 'static> Clone for Binding<T> {
    fn clone(&self) -> Self {
        Binding { get: self.get.clone(), set: self.set.clone() }
    }
}

/// What `.modelContainer(…)` needs to provide for `Query<T>` to fetch.
pub trait ProvidesQueries {
    fn querySource(&self) -> Rc<dyn Fn(&'static str) -> Option<Rc<dyn Any>>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_clones_share_storage() {
        let a = State::new(1);
        let b = a.clone();
        b.set(2);
        assert_eq!(a.wrappedValue(), 2);
    }

    #[test]
    fn binding_on_set_hooks() {
        let state = State::new(1);
        let fired = Rc::new(RefCell::new(false));
        let fired2 = fired.clone();
        let binding = state.binding().onSet(move |_| *fired2.borrow_mut() = true);
        binding.set(5);
        assert!( *fired.borrow());
        assert_eq!(state.wrappedValue(), 5);
    }
}
