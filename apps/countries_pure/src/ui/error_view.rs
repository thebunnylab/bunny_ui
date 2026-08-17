//
//  ErrorView.swift — CountriesSwiftUI
//
//  The `retryAction` arrives as `Rc<dyn Fn()>` because the view keeps it in
//  a field across renders (it is the Swift design: an injected closure) —
//  the `Button` stays typed, with the action in an `F: Fn()` field.
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
            // `Rc<dyn Fn()>` does not implement `Fn` (only `&F` does) — the
            // action wrapped in a closure that dereferences it.
            button(text("Retry").bold(), move || retry_action()),
        ))
    }
}
