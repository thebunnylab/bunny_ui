//! O counter numa janela de verdade — o primeiro elemento na tela.
//!
//! ```sh
//! cargo run -p bunny-ui-macos --example counter_window
//! ```

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;

#[derive(Clone, Copy)]
struct Counter {
    count: State<i32>,
}

impl Component for Counter {
    fn body(&self, _ctx: &Context) -> impl View {
        let this = *self;
        vstack((
            text(format!("count: {}", self.count.get())),
            spacer(),
            button(text("tap!"), move || this.count.update(|n| *n += 1)),
        ))
        .alignment(HorizontalAlignment::Leading)
        .padding()
    }
}

fn main() {
    let counter = Counter { count: State::new(0) };
    bunny_ui_macos::run_window(
        "bunny_ui",
        Size { width: 280.0, height: 180.0 },
        counter,
    );
}
