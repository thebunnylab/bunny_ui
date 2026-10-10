//! The counter from one line on every platform: `app!` writes the entry
//! each target expects, and the application never names a shell.
//!
//! cargo run -p bunny-ui --example counter

use bunny_ui::prelude::*;

#[derive(Clone, Copy)]
struct Counter {
    count: State<i32>,
}

impl Component for Counter {
    fn body(self) -> impl View {
        vstack!(
            text!("Count: {}", self.count).font(Font::Title),
            spacer(),
            button(text("Tap me!"), move || self.count.add(1)),
        )
        .alignment(HorizontalAlignment::Leading)
        .padding()
    }
}

fn home() -> impl View {
    Counter { count: State::new(0) }
}

bunny_ui::app!(home, bunny_ui::AppConfig::new().title("bunny_ui").size(280.0, 180.0));

fn main() {
    run()
}
