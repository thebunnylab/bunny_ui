//! Liquid glass, in a window.
//!
//! Glass is the one material a diff cannot review. Every assertion the
//! pane ships with — the layout, the batch order, the parity against
//! the rasterizer — still passes if the lens bends the wrong way, and a
//! pane that bends the wrong way looks *pinched* rather than broken. So
//! it gets a window.
//!
//! ```sh
//! cargo run -p bunny-ui-macos --example glass_window
//! ```
//!
//! The scene is built to make the material LEGIBLE, which takes some
//! care: the tuned blur is sigma 8, and a hairline ruler simply
//! disappears under it. What is left then is a tinted box with a bright
//! rim — which is exactly how a fake looks. So the bands are wide, the
//! ruler under the lens is dense, and the rings give the bend a curve
//! to break.
//!
//! - **The sampler** (top): the same pane at three blurs over saturated
//!   bands. What changes is only how far the material reaches.
//! - **The lens** (bottom): a small blur and a violent bend, over a
//!   ruled field. At the rim the ruler FOLDS — the sample sweeps
//!   inward, turns, and comes back — which is what a thick edge of real
//!   glass does, and the loudest proof the lens is a lens.
//!
//! Click any pane to press it: the touch lights come up (a flat sheen
//! plus a pool of light), and both are zero until they do.

#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

use std::rc::Rc;

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;

#[derive(Clone, Copy)]
struct App {
    pressed: State<usize>,
}

impl Component for App {
    fn body(self, _ctx: &Context) -> impl View {
        vstack((self.sampler(), self.lens())).spacing(0.0)
    }
}

impl App {
    /// Three blurs of one material over saturated bands. The bands are
    /// wide on purpose: a blur of sigma 8 eats anything thinner than
    /// itself, and a pane over eaten content proves nothing.
    fn sampler(self) -> impl UnaryView {
        zstack!(
            bands(),
            hstack((
                self.pane(1, "thin", "blur 2", Glass::regular().blur(2.0), 26.0),
                self.pane(2, "regular", "blur 8", Glass::regular(), 26.0),
                self.pane(3, "thick", "blur 16", Glass::regular().blur(16.0), 26.0),
            ))
            .spacing(22.0)
            .padding_length(26.0),
        )
        .frame_height(250.0)
    }

    /// The lens: a small blur and a violent bend, over a ruled field.
    fn lens(self) -> impl UnaryView {
        zstack!(
            ruler(),
            self.pane(
                4,
                "lens",
                "blur 1 · refraction 24/44",
                Glass::regular().blur(1.0).refraction(24.0, 44.0).tint(Color::rgba(255, 255, 255, 16)),
                46.0,
            )
            .padding_length(46.0),
        )
    }

    fn pane(
        self,
        id: usize,
        title: &'static str,
        detail: &'static str,
        glass: Glass,
        radius: f64,
    ) -> impl UnaryView {
        let pressed = self.pressed;
        // the touch lights: a flat wash plus a pool under the finger,
        // and both are ZERO until the press
        let material = match pressed.get() == id {
            true => glass.sheen(0.06).spot(UnitPoint::CENTER, 0.9, 0.18),
            false => glass,
        };
        vstack((
            text(title).font(Font::Title).foreground_color(Color::WHITE),
            text(detail).font(Font::Caption).foreground_color(Color::rgba(255, 255, 255, 200)),
        ))
        .spacing(2.0)
        .padding_length(18.0)
        .frame_max(f64::INFINITY, f64::INFINITY, Alignment::Center)
        .corner_radius(radius)
        .glass(material)
        .on_click(move || pressed.set(if pressed.get() == id { 0 } else { id }))
    }
}

/// Saturated bands, wide enough to survive the material's own blur.
fn bands() -> impl UnaryView {
    let colors = [0xE84B4B, 0xE8A93C, 0x4BC17A, 0x3BA8D8, 0x8B5CF6, 0xE84B92];
    zstack!(
        spacer().background_color(Color::hex(0x14163A)),
        for_each(
            colors.into_iter().enumerate().collect::<Vec<_>>(),
            |(index, _)| index.to_string(),
            |(_, color)| spacer().background_color(Color::hex(*color)),
        )
        .horizontal(),
    )
}

/// A dense ruler and a few rings — the field the lens has to bend. A
/// straight line makes the bend legible; a curve makes it obvious where
/// the fold turns.
fn ruler() -> impl UnaryView {
    zstack!(
        spacer().background_gradient(
            Gradient::linear(Color::hex(0x2A1B5E), Color::hex(0xC2571F))
                .direction(UnitPoint::TOP_LEADING, UnitPoint::BOTTOM_TRAILING),
        ),
        rings(),
        for_each((0..40).collect::<Vec<i32>>(), |index: &i32| index.to_string(), |_| {
            spacer()
                .frame_width(2.0)
                .background_color(Color::rgba(255, 255, 255, 130))
                .padding_edge(Edge::Trailing, 16.0)
        })
        .horizontal(),
        for_each((0..22).collect::<Vec<i32>>(), |index: &i32| index.to_string(), |_| {
            spacer()
                .frame_height(2.0)
                .background_color(Color::rgba(255, 255, 255, 90))
                .padding_edge(Edge::Bottom, 16.0)
        })
        .vertical(),
    )
}

/// Concentric rings.
///
/// A ring is a CONTOUR, not a ramp: a two-stop radial gradient holds
/// its far colour for every distance past the end, so a ring built that
/// way is a filled disc with a soft edge. A box with a corner radius of
/// half its side and a border of two is the circle, and only the
/// circle.
fn rings() -> impl UnaryView {
    let ring = |side: f64| {
        spacer()
            .frame(side, side)
            .corner_radius(side / 2.0)
            .border(Color::rgba(255, 255, 255, 150), 2.0)
    };
    zstack!(ring(150.0), ring(280.0), ring(410.0), ring(540.0), ring(670.0))
}

#[cfg(target_os = "macos")]
fn main() {
    theme::install(Theme::dark());
    let runtime = Runtime::new()
        .text_engine(Rc::new(bunny_ui_macos::CoreTextEngine::new()))
        .image_engine(Rc::new(bunny_ui_macos::CoreGraphicsImageEngine::new()));
    bunny_ui_macos::run_window_with(
        "bunny — liquid glass",
        Size { width: 900.0, height: 660.0 },
        runtime,
        App { pressed: State::new(0) },
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {} // this example is macOS-only
