//! A window that stands on a blur of what is behind it.
//!
//! ```sh
//! cargo run -p bunny-ui-windows --example blur_window
//! cargo run -p bunny-ui-windows --example blur_window -- --clear
//! cargo run -p bunny-ui-windows --example blur_window -- --opaque
//! cargo run -p bunny-ui-windows --example blur_window -- --light
//! cargo run -p bunny-ui-windows --example blur_window -- --scene
//! ```
//!
//! The probe script this example exists to run. The road it opens was
//! type-checked and not yet run on a machine when this was written, so
//! every line is something to confirm, on Windows 11 version 22H2 or
//! later:
//! 1. The console prints what was asked and what the system served. The
//!    two agree, or the second is `Opaque`: a blur the system cannot give
//!    is never served as a clear window. Before Windows 11 22H2 a blurred
//!    ask is served opaque.
//! 2. The desktop shows through the window, blurred by the system's
//!    acrylic material. Drag the window over something with sharp edges:
//!    the edges soften behind it, the text on it stays sharp.
//! 3. The canvas is the theme's own, thinned to 60 %. That alpha is the
//!    whole of what the app says: no view in the scene knows the window
//!    is not opaque.
//! 4. The two stacks are the same three shades, one inside the other,
//!    each at the same alpha. On the left each shade adds its alpha to
//!    the ones under it and mixes with their colour, so the blur fades
//!    with every step in: that is source-over, and it is the default.
//!    On the right each shade REPLACES what is under it
//!    (`.background_blend(BackgroundBlend::Replace)`): the colour
//!    changes from one to the next and the blur shows through all three
//!    at the one strength a shade names, however deep it sits.
//! 5. `--clear` is the same window with no blur: what is behind it shows
//!    sharp. `--opaque` is the window every other example opens, and the
//!    alpha of the canvas is ignored.
//! 6. `--light` is the light theme. The material follows the scene and
//!    not the desktop: light under a light canvas, dark under a dark one.
//! 7. The count runs to eight and stops. Every step repaints over the
//!    blur and leaves no ghost of the digit before it. Resize the window
//!    while it counts: the blur follows the frame.
//! 8. `BUNNY_PRESENT=cpu` refuses the GPU road, and a window that is not
//!    opaque has no other: the console says it was served `Opaque`, and
//!    the window is the opaque one.
//! 9. `--scene` is the same window with the bar drawn by the scene and
//!    no system title bar (drag it by its first line; Alt+F4 closes it).
//!    This is the open question of the backdrop: look at the top edge
//!    for a caption tint, or for caption buttons of the system showing
//!    through the scene. `ffi::apply_backdrop` holds the three switches
//!    that answer whatever shows.

#![cfg_attr(not(target_os = "windows"), allow(dead_code, unused_imports))]

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
            // under `--scene` this line is the bar: the window drags by
            // it. A window with the system's bar never asks
            text!("The window stands on: {}", self.served).font(Font::Title).window_drag_region(),
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

#[cfg(target_os = "windows")]
fn main() {
    use std::rc::Rc;

    use bunny_ui::theme::{self, Theme};
    use bunny_ui_windows::{
        App, Chrome, DirectWriteEngine, WicImageEngine, WindowBackground, WindowSpec,
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
    let chrome = if flag("--scene") { Chrome::Scene } else { Chrome::Native };
    let palette = if dark { Theme::dark() } else { Theme::light() };
    // the one line that makes the blur show: a canvas that is not solid
    theme::install(Theme { canvas: palette.canvas.with_alpha(153), ..palette });

    let app = App::new();
    let runtime = app
        .runtime()
        .text_engine(Rc::new(DirectWriteEngine::new()))
        .image_engine(Rc::new(WicImageEngine::new()));
    // the answer before the window exists; the console has the answer
    // after it opens, which is the one that counts
    let served = match bunny_ui_windows::window_background(asked) {
        WindowBackground::Opaque => "an opaque canvas",
        WindowBackground::Clear => "the desktop, sharp",
        WindowBackground::Blurred => "the desktop, blurred",
    };
    let window = app.open(
        WindowSpec::titled("blur_window").size(620.0, 460.0).chrome(chrome).background(asked),
        Rc::new(runtime),
        Scene { served, dark, count: State::new(0) },
    );
    println!("asked {asked:?}, served {:?}", app.background(window));
    app.run();
}

#[cfg(not(target_os = "windows"))]
fn main() {} // this example is Windows-only
