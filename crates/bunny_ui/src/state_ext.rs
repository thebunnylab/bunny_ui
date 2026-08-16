//! `wrappedValue` com cara de Rust — `state.get()` / `binding.get()`.

use motor::state::{Binding, State};

/// `State<T>::get()` — o `wrappedValue` do `@State`.
pub trait StateExt<T: Clone + 'static> {
    fn get(&self) -> T;

    /// `state.add(1)` — o `+=` que funciona dentro de closures `Fn`: a
    /// mutação é interior, o handle nem precisa de `&mut`. (Um `+=`
    /// literal exigiria `FnMut` e não compilaria num `button(…)`.)
    fn add(&self, delta: T)
    where
        T: std::ops::AddAssign<T>;
}

impl<T: Clone + 'static> StateExt<T> for State<T> {
    fn get(&self) -> T {
        self.wrappedValue()
    }

    fn add(&self, delta: T)
    where
        T: std::ops::AddAssign<T>,
    {
        self.update(|value| *value += delta);
    }
}

/// `Binding<T>::get()` — o `wrappedValue` do `@Binding`/`$x`.
pub trait BindingExt<T: Clone + 'static> {
    fn get(&self) -> T;
}

impl<T: Clone + 'static> BindingExt<T> for Binding<T> {
    fn get(&self) -> T {
        self.wrappedValue()
    }
}
