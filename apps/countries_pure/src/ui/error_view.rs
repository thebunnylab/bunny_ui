//
//  ErrorView.swift — CountriesSwiftUI
//
//  O `retryAction` chega como `Rc<dyn Fn()>` porque a view o guarda num
//  campo entre renders (é o design do Swift: closure injetada) — o
//  `Button` continua tipado, com a ação num campo `F: Fn()`.
//

use bunny_ui::prelude::*;

#[derive(Clone)]
pub struct ErrorView {
    error: LoadError,
    retry_action: Rc<dyn Fn()>,
}

impl ErrorView {
    /// `ErrorView(error:retryAction:)`
    pub fn new(error: LoadError, retry_action: Rc<dyn Fn()>) -> Self {
        Self {
            error,
            retry_action,
        }
    }
}

impl Component for ErrorView {
    fn body(self, _ctx: &Context) -> impl View {
        let retry_action = self.retry_action.clone();
        vstack((
            text("An Error Occured").font(Font::Title),
            text(self.error.message())
                .font(Font::Callout)
                .multiline_text_alignment(TextAlignment::Center)
                .padding_edge(Edge::Bottom, 40.0)
                .padding(),
            // `Rc<dyn Fn()>` não implementa `Fn` (só `&F` o faz) — a
            // ação embrulhada numa closure que a desreferencia.
            button(text("Retry").bold(), move || retry_action()),
        ))
    }
}
