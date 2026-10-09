//! MVVM presentation state stays ordinary Rust, with explicit ownership.
//!
//! ## Wiring
//! Put independent `State` fields and command methods on a small presentation
//! struct and pass that model into the view's `vm` field. The body composes
//! the view from those properties and commands. Models constructed outside
//! rendering use `State`'s application lifetime. For component-owned state,
//! [`view_model`] remains available inside a body: it initializes once per
//! mounted identity and can supply a child view's field. Moving a model into
//! a field does not transfer its state ownership or change its subscriptions.
//! Domain services are normal constructor arguments; commands test headlessly.
//!
//! ## Production gotchas
//! A `view_model` initializer runs once. Changing its captured arguments does not
//! recreate it: give the owning view a new identity when its model must change.
//! Call this once at each declaration site; repeated rows need keyed identity.
//! Retained `State` handles become invalid when the owning view is unmounted.

use std::panic::Location;
use std::rc::Rc;

/// Initializes and retains a presentation model at this component callsite.
///
/// The model is an ordinary `Clone` struct; `State`-only models can be `Copy`.
/// A parent rerender keeps the model and its properties. Unmounting its owner
/// releases the model and all state created by its initializer.
///
/// # Panics
/// Panics outside a runtime render pass. Application-owned models may instead
/// be constructed explicitly before rendering, under `State`'s app lifetime.
#[track_caller]
pub fn view_model<M: Clone + 'static>(initialize: impl FnOnce() -> M) -> M {
    assert!(
        motor::identity::cursor_scope_rc().is_some(),
        "view_model() must be declared inside a component rendered by a Runtime"
    );
    let site = Location::caller();
    let _scope = motor::identity::enter(format!("@model({site})"));
    let slot = motor::identity::scoped_effect_slot::<M>(site);
    let existing = slot.borrow().clone();
    if let Some(model) = existing {
        // Its initializer did not run, so the state's nested owners were not
        // visited. This retained scope protects them just as a kept view does.
        if let Some(path) = motor::identity::cursor_scope_rc() {
            motor::identity::mark_skipped(&path);
        }
        return model;
    }
    let model = initialize();
    *slot.borrow_mut() = Some(model.clone());
    model
}

/// A read-only derived property, evaluated in the subscribing reader's scope.
///
/// This describes a computation; it does not create an eager effect or a
/// whole-model subscription. A text node caches its result through the same
/// binding mechanism used by `text!`.
pub struct Derived<T>(Rc<dyn Fn() -> T>);

impl<T> Clone for Derived<T> {
    fn clone(&self) -> Self {
        Self(Rc::clone(&self.0))
    }
}

impl<T> Derived<T> {
    /// Evaluates the property and records dependencies on its current reader.
    pub fn get(&self) -> T {
        (self.0)()
    }
}

/// Declares a derived presentation property without evaluating it.
pub fn derived<T>(read: impl Fn() -> T + 'static) -> Derived<T> {
    Derived(Rc::new(read))
}

impl<T: std::fmt::Display + 'static> crate::text_value::IntoText for Derived<T> {
    fn into_text(self) -> crate::views::Text {
        crate::views::text_with(move || crate::bind::shared_text(format_args!("{}", self.get())))
    }
}
