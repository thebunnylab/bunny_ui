//! `wrappedValue` that looks like Rust — `state.get()` / `binding.get()`.

use motor::state::{Binding, State};

/// `State<T>::get()` — the `wrappedValue` of `@State`.
pub trait StateExt<T: Clone + 'static> {
    fn get(&self) -> T;

    /// `state.add(1)` — the `+=` that works inside `Fn` closures: the
    /// mutation is interior, the handle does not even need `&mut`. (A
    /// literal `+=` would demand `FnMut` and would not compile in a
    /// `button(…)`.)
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

/// `Binding<T>::get()` — the `wrappedValue` of `@Binding`/`$x`.
pub trait BindingExt<T: Clone + 'static> {
    fn get(&self) -> T;
}

impl<T: Clone + 'static> BindingExt<T> for Binding<T> {
    fn get(&self) -> T {
        self.wrappedValue()
    }
}
