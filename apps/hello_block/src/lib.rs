//! The hello block — the smallest raw block worth measuring: a heading,
//! a line that counts, a button and a switch. Built for the Trinity
//! shell with `bunny_ui_shell::block!`, it is a zero-import Wasm module:
//!
//! ```sh
//! cargo build -p hello-block --target wasm32-unknown-unknown --profile web
//! ```
//!
//! What it weighs and what it costs to start are in
//! `docs/measures/trinity-shell-hello-2026-11-25.md`.

use bunny_ui::prelude::*;

#[derive(Clone, Copy)]
pub struct Hello {
    count: State<i32>,
    compact: State<bool>,
}

impl Component for Hello {
    fn body(self) -> impl View {
        vstack!(
            text("Hello from a raw block").font(Font::Headline),
            text!("Pressed {} times", self.count),
            hstack!(
                button(text("Add one"), move || self.count.add(1)),
                toggle(text("Compact"), self.compact.binding()),
            ),
        )
        .alignment(HorizontalAlignment::Leading)
        .padding()
    }
}

/// A fresh hello block.
pub fn hello() -> Hello {
    Hello { count: State::new(0), compact: State::new(false) }
}

bunny_ui_shell::block!(hello);
