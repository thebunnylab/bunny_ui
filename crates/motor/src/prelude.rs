//! What the mirrored-port code implicitly imports.

pub use crate::cancel_bag::{CancelBag, Cancellable, TaskHandle};
pub use crate::combine::{AnyPublisher, IntoPublisher, PassthroughSubject, Store};
pub use crate::inspection::Inspection;
pub use crate::loadable::{Loadable, LoadableSubject, LoadError};
pub use crate::modifiers::{CustomModifier, ModifierBehavior, ModifiedView, Modifier, ViewExt};
pub use crate::runtime::Runtime;
pub use crate::state::{
    Binding, Context, Environment, EnvironmentValues, FromEnvironment, Locale, ProvidesQueries, State,
};
pub use std::rc::Rc;
pub use crate::view::{AnyView, RenderNode, Component, View};
pub use crate::views::*;

/// Swift's `Optional(x)` constructor — a promoção implícita `T` → `T?` dos
/// call sites do Swift não é verificável sem tipos no macro, então o port
/// escreve `Optional(x)` onde o Swift promove implicitamente.
pub fn Optional<T>(value: T) -> Option<T> {
    Some(value)
}

// Dotted-enum resolution (`case .notRequested` → `Loadable::NotRequested`)
// happens with qualified paths on the mirrored side, so the glob imports
// below stay unambiguous (core has its own `NotRequested` cases).
pub use crate::views::{Alignment::*, ContentMode::*, Font::*, ListStyle::*, ProgressViewStyle::*};
