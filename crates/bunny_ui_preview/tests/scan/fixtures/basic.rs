use bunny_ui::prelude::*;

/// A card with a short title.
#[preview(name = "Card", scope = "Data Display", example = "Short")]
fn card_short() -> impl View {
    card("Short")
}

/// A card with a title that wraps.
///
/// Two paragraphs of docs.
#[preview(name = "Card", scope = "Data Display", example = "Long title")]
pub fn card_long() -> impl View {
    card("A much longer title that wraps")
}

#[preview]
fn plain_button() -> impl View {
    button("Go", || {})
}

mod nested {
    #[bunny_ui::preview(scope = "Forms & Input")]
    pub(crate) fn nested_field() -> impl View {
        text_field("")
    }
}
