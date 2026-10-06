//! A looping decoration at rest: a dial that turns on its own layer while
//! the window around it does nothing.
use arena::{Args, Step, WINDOW, scripted};
use bunny_ui::anim::Loop;
use bunny_ui::layout::{Color, Point, Rect, Size};
use bunny_ui::prelude::*;

const OVER: (f64, f64) = (WINDOW.0 * 0.5, WINDOW.1 * 0.5);

#[derive(Clone)]
struct Dial;

impl Component for Dial {
    fn body(self, _ctx: &Context) -> impl View {
        vstack!(
            text!("the arena · a loop at rest").font_size(13.0).padding_length(10.0),
            canvas(|ctx, painter| {
                let phase = ctx.phase;
                let size = ctx.frame.size;
                let centre = Point { x: size.width / 2.0, y: size.height / 2.0 };
                let radius = size.width.min(size.height) * 0.3;
                let angle = phase * std::f64::consts::TAU;
                let hand = Point { x: centre.x + radius * angle.cos(), y: centre.y + radius * angle.sin() };
                painter.fill(Rect { origin: Point { x: hand.x - 6.0, y: hand.y - 6.0 }, size: Size { width: 12.0, height: 12.0 } }, Color::hex(0xE69933));
            })
            .looping(Loop::secs(2.0).fps(60.0))
            .frame(240.0, 240.0),
            spacer(),
        )
    }
}

fn main() {
    let args = Args::parse();
    bunny_ui_macos::run_window("arena — canvas", Size { width: WINDOW.0, height: WINDOW.1 }, scripted(Dial, args, OVER, |_: Step| {}));
}
