//! `OneOf3`…`OneOf8` — o `_ConditionalContent` de um `match` com N braços.
//!
//! O `@ViewBuilder` do SwiftUI esconde o aninhamento binário no codegen;
//! sem codegen, `Second(Second(First(…)))` vaza para o cliente — e inserir
//! um braço no meio renumera todos. Aqui cada braço tem nome próprio:
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
//! Como no [`Either`], todos os braços exigem a mesma aridade — o tipo
//! resultante continua monomórfico, só o discriminante decide em runtime.
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
                // Como no `Either`: o braço é identidade — trocar de braço
                // desmonta o que o anterior montou.
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
    /// `match` de três braços.
    OneOf3 { A, B, C }
);
one_of!(
    /// `match` de quatro braços — o formato de um `Loadable`.
    OneOf4 { A, B, C, D }
);
one_of!(
    /// `match` de cinco braços.
    OneOf5 { A, B, C, D, E }
);
one_of!(
    /// `match` de seis braços.
    OneOf6 { A, B, C, D, E, F }
);
one_of!(
    /// `match` de sete braços.
    OneOf7 { A, B, C, D, E, F, G }
);
one_of!(
    /// `match` de oito braços.
    OneOf8 { A, B, C, D, E, F, G, H }
);
