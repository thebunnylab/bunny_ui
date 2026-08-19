//! A living decoration on a resting app.
//!
//! The ring in the bar breathes on a loop clock: `.looping(...)` gives
//! its paint a phase, and each step of the clock repaints the ring's
//! own layer — the window behind it never redraws, the GPU never
//! re-encodes the scene, and the frame driver beats five times a
//! second instead of sixty. Springs still take the display's own pace:
//! press the chip and watch the two drivers trade places.
//!
//! What to check by hand:
//! - the ring breathes slowly; nothing else repaints (turn on the
//!   damage overlay of your choice, or just trust the pace);
//! - hovering the chip animates its color by spring — the display
//!   link runs while it settles, then the slow beat returns;
//! - switch to another app: the ring freezes mid-breath, and resumes
//!   exactly there when the window is key again;
//! - with reduce motion on, the ring holds its resting frame forever.

#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

use bunny_ui::prelude::*;
#[cfg(target_os = "macos")]
use bunny_ui_macos::CoreTextEngine;
#[cfg(target_os = "macos")]
use std::rc::Rc;

const BAR_H: f64 = 44.0;

#[derive(Clone, Copy)]
struct LiveDemo;

impl Component for LiveDemo {
    fn body(self, _ctx: &Context) -> impl View {
        let ring = canvas(|ctx, painter| {
            // one slow breath per loop: the phase turns into a radius
            let size = ctx.size();
            let breathe = 1.0 + 0.10 * (ctx.phase * std::f64::consts::TAU).sin();
            let diameter = size.width.min(size.height) * 0.68 * breathe;
            let ring = Rect {
                origin: Point {
                    x: (size.width - diameter) / 2.0,
                    y: (size.height - diameter) / 2.0,
                },
                size: Size { width: diameter, height: diameter },
            };
            painter.stroke(ring, painter.ink(), 2.5, diameter / 2.0);
        })
        .looping(Loop::secs(4.8).fps(5.0))
        .frame(26.0, 26.0)
        .foreground_color(theme::accent());

        let chip = text("a spring beside a clock")
            .padding_length(8.0)
            .background_color(theme::control())
            .background_hovered(theme::control_hovered())
            .corner_radius(6.0)
            .animated(Spring::smooth());

        let bar = hstack!(ring, chip, spacer())
            .spacing(12.0)
            .alignment(VerticalAlignment::Center)
            .padding_edge(Edge::Leading, 16.0)
            .frame_height(BAR_H)
            .background_color(theme::panel());

        vstack!(
            bar,
            text("the ring repaints alone, five frames a second")
                .foreground_color(theme::fg_secondary())
                .padding_length(24.0),
            spacer()
        )
        .alignment(HorizontalAlignment::Leading)
    }
}

#[cfg(target_os = "macos")]
fn main() {
    theme::install(Theme::dark());
    let runtime = Runtime::new().text_engine(Rc::new(CoreTextEngine::new()));
    bunny_ui_macos::run_window_with(
        "live",
        Size { width: 560.0, height: 240.0 },
        runtime,
        LiveDemo,
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {} // this example is macOS-only
