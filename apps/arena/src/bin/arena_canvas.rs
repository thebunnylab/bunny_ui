//! A looping decoration at rest: a dial that turns on its own layer while
//! the window around it does nothing.
use arena::{Args, FONT_FAMILY};
use bunny_ui::anim::Loop;
use bunny_ui::layout::{Color, Point, Rect, Size};
use bunny_ui::prelude::*;

#[derive(Clone)]
struct Dial {
    probe: Option<std::rc::Rc<PaintProbe>>,
}

struct PaintProbe {
    started: std::time::Instant,
    frames: std::cell::Cell<u64>,
    slot: std::cell::Cell<u128>,
}

impl Component for Dial {
    fn body(self, _ctx: &Context) -> impl View {
        let probe = self.probe.clone();
        vstack!(
            hstack!(
                text!("the arena · a loop at rest").font_size(13.0),
                spacer(),
            )
            .padding_edge(Edge::Leading, 12.0)
            .frame_height(40.0),
            canvas(move |ctx, painter| {
                let phase = ctx.phase;
                let size = ctx.frame.size;
                let centre = Point {
                    x: size.width / 2.0,
                    y: size.height / 2.0,
                };
                let radius = size.width.min(size.height) * 0.3;
                let angle = phase * std::f64::consts::TAU;
                let hand = Point {
                    x: centre.x + radius * angle.cos(),
                    y: centre.y + radius * angle.sin(),
                };
                let marker = Rect {
                        origin: Point {
                            x: hand.x - 6.0,
                            y: hand.y - 6.0,
                        },
                        size: Size {
                            width: 12.0,
                            height: 12.0,
                        },
                    };
                painter.fill(marker, Color::hex(0xE69933));
                if let Some(probe) = &probe {
                    let frame = probe.frames.get() + 1;
                    probe.frames.set(frame);
                    let slot = probe.started.elapsed().as_millis() / 500;
                    if slot <= 3 && (frame == 1 || slot > probe.slot.get()) {
                        probe.slot.set(slot);
                        let rect = ctx.frame;
                        let state = serde_json::json!({"frame": frame, "phase": phase,
                            "drawing_rect": [rect.origin.x, rect.origin.y, rect.size.width, rect.size.height],
                            "marker": [marker.origin.x, marker.origin.y, marker.size.width, marker.size.height]});
                        println!("CANVAS_FRAME {} {state}", arena::unix_ms());
                    }
                }
            })
            .looping(Loop::secs(2.0).fps(60.0))
            .frame(240.0, 240.0),
            spacer(),
        )
        .spacing(0.0)
        .alignment(HorizontalAlignment::Leading)
        .font_family(FONT_FAMILY)
        .foreground_color(Color::hex(0xE6E6EA))
        .background_color(Color::hex(0x17171C))
    }
}

fn main() {
    let args = Args::parse();
    let probe = std::env::var_os("ARENA_SCENE_DIAGNOSTIC").map(|_| {
        std::rc::Rc::new(PaintProbe {
            started: std::time::Instant::now(),
            frames: Default::default(),
            slot: Default::default(),
        })
    });
    arena::scene::run(
        "arena — canvas",
        "canvas",
        Dial { probe },
        args,
        |_| {},
        || serde_json::json!({}),
    );
}
