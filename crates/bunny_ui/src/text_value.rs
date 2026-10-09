//! Conversions into text preserve whether the input is fixed or reactive.
//!
//! ## Wiring
//! `text(state)` defers the read to the text node. Fixed strings are converted
//! once; callers choosing an eager snapshot can still write `text(state.get())`.

use std::borrow::Cow;
use std::fmt::Display;
use std::sync::Arc;

use motor::state::{Binding, State};

use crate::views::{Text, text_with};

/// A fixed string or a reactive source accepted by [`crate::views::text`].
///
/// Implement this trait for application-specific text sources. Reactive
/// implementations must defer reads with [`text_with`].
pub trait IntoText {
    /// Converts this input without eagerly reading reactive state.
    fn into_text(self) -> Text;
}

macro_rules! fixed_text {
    ($($ty:ty),+ $(,)?) => {$(
        impl IntoText for $ty {
            fn into_text(self) -> Text {
                Text(crate::bind::TextSource::Fixed(Arc::from(self)))
            }
        }
    )+};
}

fixed_text!(&str, &mut str, String, Box<str>, Arc<str>, Cow<'_, str>);

impl<T: Clone + Display + 'static> IntoText for State<T> {
    fn into_text(self) -> Text {
        text_with(move || crate::bind::shared_text(format_args!("{self}")))
    }
}

impl<T: Clone + Display + 'static> IntoText for Binding<T> {
    fn into_text(self) -> Text {
        text_with(move || self.with(|value| crate::bind::shared_text(format_args!("{value}"))))
    }
}

impl IntoText for &String {
    fn into_text(self) -> Text {
        self.as_str().into_text()
    }
}
