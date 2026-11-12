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
//! 3. The two stacks are the same three shades, one inside the other,
//!    each at the same alpha. On the left each shade adds its alpha to
//!    the ones under it and mixes with their colour, so the blur fades
//!    with every step in: that is source-over, and it is the default.
//!    On the right each shade REPLACES what is under it
//!    (`.background_blend(BackgroundBlend::Replace)`): the colour
//!    changes from one to the next and the blur shows through all three
//!    at the one strength a shade names, however deep it sits.
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

/// One panel of a stack: a veil, and whatever stands in it. `blend` says
/// whether the veil lies over what is under it or takes its place.
fn panel(label: &'static str, veil: Color, blend: BackgroundBlend, inside: impl View) -> impl View {
    vstack!(text(label).font(Font::Caption), inside)
        .spacing(8.0)
        .alignment(HorizontalAlignment::Leading)
        .padding_length(12.0)
        .background_color(veil)
        .corner_radius(12.0)
        .background_blend(blend)
}

/// Three shades of one window, each inside the one before.
fn stack(title: &'static str, shades: [Color; 3], blend: BackgroundBlend) -> impl View {
    let [outer, middle, inner] = shades;
    vstack!(
        text(title).font(Font::Headline),
        panel(
            "a shade",
            outer,
            blend,
            panel("a second, inside it", middle, blend, panel("a third", inner, blend, text("the blur"))),
        ),
    )
    .spacing(8.0)
    .alignment(HorizontalAlignment::Leading)
}

#[derive(Clone)]
struct Scene {
    served: &'static str,
    dark: bool,
    count: State<u32>,
}

impl Component for Scene {
    fn body(self) -> impl View {
        // three shades at ONE alpha: what differs from one to the next is
        // the colour, never how much of the blur it lets through
        let alpha = if self.dark { 64 } else { 56 };
        let shades = if self.dark {
            [Color::rgb(255, 255, 255), Color::rgb(110, 160, 255), Color::rgb(255, 140, 110)]
        } else {
            [Color::rgb(0, 0, 0), Color::rgb(20, 70, 190), Color::rgb(190, 60, 30)]
        }
        .map(|shade| shade.with_alpha(alpha));
        let stacks = hstack!(
            stack("over: the alphas add up", shades, BackgroundBlend::Over),
            stack("replace: each is its own", shades, BackgroundBlend::Replace),
        )
        .spacing(16.0)
        .alignment(VerticalAlignment::Top);
        let count = self.count;
        vstack!(
            text!("The window stands on: {}", self.served).font(Font::Title),
            text("The canvas is the theme's, at 60 %. Nothing else in the scene knows."),
            stacks,
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
        WindowSpec::titled("blur_window").size(620.0, 460.0).background(asked),
        Rc::new(runtime),
        Scene { served, dark, count: State::new(0) },
    );
    println!("asked {asked:?}, served {:?}", app.background(window));
    app.run();
}

#[cfg(not(target_os = "macos"))]
fn main() {}
