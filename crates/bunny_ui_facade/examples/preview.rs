//! A card with three previews — light, dark, and sized to fit — the way
//! an app writes them. Under `BUNNY_PREVIEW_CHECK=card_dark` the binary
//! renders that preview once and prints a line; under `BUNNY_PREVIEW_PORT`
//! it serves a tool; otherwise it opens its window on the card.
//!
//! ```sh
//! BUNNY_PREVIEW_CHECK=card_light,card_dark,card_fits \
//!   cargo run -p bunny-ui --features preview --example preview
//! ```

#![forbid(unsafe_code)]

use bunny_ui::prelude::*;

#[derive(Clone)]
struct Card {
    title: &'static str,
    count: State<i32>,
}

impl Component for Card {
    fn body(self) -> impl View {
        vstack!(
            text(self.title).font(Font::Title),
            text!("Tapped {} times", self.count),
            button(text("Tap"), move || self.count.add(1)),
        )
        .padding_length(16.0)
        .background_color(theme::panel())
        .corner_radius(10.0)
    }
}

fn card() -> impl View {
    Card { title: "Hello", count: State::new(0) }
}

/// The card under the light theme.
#[bunny_ui::preview(name = "Card", scope = "Cards", example = "Light")]
fn card_light() -> impl View {
    card()
}

/// The card under the dark theme.
#[bunny_ui::preview(name = "Card", scope = "Cards", example = "Dark")]
fn card_dark() -> Preview<impl View> {
    Preview::new(card()).dark()
}

/// The card at the size it asks for.
#[bunny_ui::preview(name = "Card", scope = "Cards", example = "Fits")]
fn card_fits() -> Preview<impl View> {
    Preview::new(card()).size_that_fits()
}

bunny_ui::app!(card, bunny_ui::AppConfig::new().title("Preview").size(320.0, 240.0));

fn main() {
    run()
}
