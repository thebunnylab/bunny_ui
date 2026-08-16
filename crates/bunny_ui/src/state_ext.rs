//! `wrappedValue` com cara de Rust — `state.get()` / `binding.get()`.

use motor::state::{Binding, State};

/// `State<T>::get()` — o `wrappedValue` do `@State`.
pub trait StateExt<T: Clone + 'static> {
    fn get(&self) -> T;
}

impl<T: Clone + 'static> StateExt<T> for State<T> {
    fn get(&self) -> T {
        self.wrappedValue()
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
