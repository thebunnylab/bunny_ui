//! `OneOf3`…`OneOf8` — the `_ConditionalContent` of an N-arm `match`.
//!
//! SwiftUI's `@ViewBuilder` hides the binary nesting in codegen; without
//! codegen, `Second(Second(First(…)))` leaks to the client — and
//! inserting an arm in the middle renumbers them all. Here every arm has
//! its own name:
//!
//! ```ignore
//! match self.details.get() {
//!     Loadable::NotRequested => OneOf4::A(self.default_view(ctx)),
//!     Loadable::IsLoading(..) => OneOf4::B(self.loading_view()),
//!     Loadable::Loaded(details) => OneOf4::C(self.loaded_view(ctx, details)),
//!     Loadable::Failed(error) => OneOf4::D(self.failed_view(ctx, error)),
//! }
//! ```
//!
//! As with [`Either`], all arms demand the same arity — the resulting
//! type stays monomorphic, only the discriminant decides at runtime.
//!
//! [`Either`]: crate::view::Either

use motor::state::Context;

use crate::view::{NodeList, View};

macro_rules! one_of {
    ($(#[$doc:meta])* $name:ident { $($variant:ident),+ }) => {
        $(#[$doc])*
        #[derive(Clone)]
        pub enum $name<$($variant),+> {
            $($variant($variant),)+
        }

        impl<Ar, $($variant: View<Arity = Ar>),+> View for $name<$($variant),+> {
            type Arity = Ar;

            fn render_into(&self, ctx: &Context, out: &mut NodeList) {
                // As in `Either`: the arm is identity — switching arms
                // unmounts what the previous one mounted.
                match self {
                    $($name::$variant(view) => {
                        let _frame =
                            motor::identity::enter(concat!("@", stringify!($variant)));
                        view.render_into(ctx, out);
                    })+
                }
            }
        }
    };
}

one_of!(
    /// A three-arm `match`.
    OneOf3 { A, B, C }
);
one_of!(
    /// A four-arm `match` — the shape of a `Loadable`.
    OneOf4 { A, B, C, D }
);
one_of!(
    /// A five-arm `match`.
    OneOf5 { A, B, C, D, E }
);
one_of!(
    /// A six-arm `match`.
    OneOf6 { A, B, C, D, E, F }
);
one_of!(
    /// A seven-arm `match`.
    OneOf7 { A, B, C, D, E, F, G }
);
one_of!(
    /// An eight-arm `match`.
    OneOf8 { A, B, C, D, E, F, G, H }
);
