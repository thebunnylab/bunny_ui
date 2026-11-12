//! A window that stands on a blur of what is behind it.
//!
//! ```sh
//! cargo run -p bunny-ui-macos --example blur_window
//! cargo run -p bunny-ui-macos --example blur_window -- --clear
//! cargo run -p bunny-ui-macos --example blur_window -- --opaque
//! cargo run -p bunny-ui-macos --example blur_window -- --light
//! ```
//!
//! What to look at:
//! 1. The desktop shows through the window, blurred. Drag the window over
//!    something with sharp edges: the edges soften behind it, the text on
//!    it stays sharp.
//! 2. The canvas is the theme's own, thinned to 60 %. That alpha is the
//!    whole of what the app says: no view in the scene knows the window
//!    is not opaque.
//! 3. The panels are the same colour at the same alpha, one inside the
//!    other. Each one adds its alpha to the one under it, so the blur
//!    fades with every step in: that is source-over, and it is the
//!    default.
//! 4. `--clear` is the same window with no blur: what is behind it shows
//!    sharp. `--opaque` is the window every other example opens, and the
//!    alpha of the canvas is ignored.
//! 5. The count runs to eight and stops. Every step repaints its own
//!    small rect over the blur and leaves no ghost of the digit before
//!    it — on the GPU road, and on the CPU road (`BUNNY_PRESENT=cpu`),
//!    where a damaged rect is cleared back to the translucent canvas
//!    before it is painted again.
//! 6. System Settings → Accessibility → Display → Reduce transparency
//!    turns the blur into a solid colour. That is the system's answer to
//!    that reader, and the window keeps it.

#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

use bunny_ui::layout::Color;
use bunny_ui::prelude::*;

/// One panel of the stack: a veil at `alpha`, and whatever stands in it.
fn panel(label: &'static str, veil: Color, inside: impl View) -> impl View {
    vstack!(text(label).font(Font::Caption), inside)
        .spacing(8.0)
        .alignment(HorizontalAlignment::Leading)
        .padding_length(12.0)
        .background_color(veil)
        .corner_radius(12.0)
}

#[derive(Clone)]
struct Scene {
    served: &'static str,
    dark: bool,
    count: State<u32>,
}

impl Component for Scene {
    fn body(self) -> impl View {
        // one veil, three deep: white over a dark scene, black over a light
        let veil = if self.dark { Color::rgba(255, 255, 255, 46) } else { Color::rgba(0, 0, 0, 30) };
        let stack = panel(
            "one veil",
            veil,
            panel(
                "two veils",
                veil,
                panel("three veils", veil, text("each step in hides more of the blur")),
            ),
        );
        let count = self.count;
        vstack!(
            text!("The window stands on: {}", self.served).font(Font::Title),
            text("The canvas is the theme's, at 60 %. Nothing else in the scene knows."),
            stack,
            text!("repainted {} times over the blur", count).font(Font::Title),
            spacer(),
        )
        .spacing(16.0)
        .alignment(HorizontalAlignment::Leading)
        .padding_length(24.0)
        .task(move || async move {
            for _ in 0..8 {
                task::sleep(std::time::Duration::from_millis(250)).await;
                count.add(1);
            }
        })
    }
}

#[cfg(target_os = "macos")]
fn main() {
    use std::rc::Rc;

    use bunny_ui::theme::{self, Theme};
    use bunny_ui_macos::{
        App, CoreGraphicsImageEngine, CoreTextEngine, WindowBackground, WindowSpec,
    };

    let flag = |name: &str| std::env::args().any(|arg| arg == name);
    let asked = if flag("--opaque") {
        WindowBackground::Opaque
    } else if flag("--clear") {
        WindowBackground::Clear
    } else {
        WindowBackground::Blurred
    };
    let dark = !flag("--light");
    let palette = if dark { Theme::dark() } else { Theme::light() };
    // the one line that makes the blur show: a canvas that is not solid
    theme::install(Theme { canvas: palette.canvas.with_alpha(153), ..palette });

    let app = App::new();
    let runtime = app
        .runtime()
        .text_engine(Rc::new(CoreTextEngine::new()))
        .image_engine(Rc::new(CoreGraphicsImageEngine::new()));
    let served = match bunny_ui_macos::window_background(asked) {
        WindowBackground::Opaque => "an opaque canvas",
        WindowBackground::Clear => "the desktop, sharp",
        WindowBackground::Blurred => "the desktop, blurred",
    };
    let window = app.open(
        WindowSpec::titled("blur_window").size(560.0, 420.0).background(asked),
        Rc::new(runtime),
        Scene { served, dark, count: State::new(0) },
    );
    println!("asked {asked:?}, served {:?}", app.background(window));
    app.run();
}

#[cfg(not(target_os = "macos"))]
fn main() {}
