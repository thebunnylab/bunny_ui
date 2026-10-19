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

/// What the dial painted. The diagnostic prints its first seconds; on
/// Windows the last paint also answers every observation boundary, so the
/// producer can check that the loop actually moved on its trajectory.
struct PaintProbe {
    started: std::time::Instant,
    frames: std::cell::Cell<u64>,
    slot: std::cell::Cell<u128>,
    last: std::cell::Cell<Option<Paint>>,
    diagnostic: bool,
}

/// One painted frame of the dial.
#[derive(Clone, Copy)]
struct Paint {
    frame: u64,
    phase: f64,
    drawing: Rect,
    marker: Rect,
}

impl Paint {
    fn value(self) -> serde_json::Value {
        let rect = |r: Rect| [r.origin.x, r.origin.y, r.size.width, r.size.height];
        serde_json::json!({"frame": self.frame, "phase": self.phase,
            "drawing_rect": rect(self.drawing), "marker": rect(self.marker)})
    }
}

impl Component for Dial {
    fn body(self) -> impl View {
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
                    let paint = Paint {
                        frame,
                        phase,
                        drawing: ctx.frame,
                        marker,
                    };
                    probe.last.set(Some(paint));
                    let slot = probe.started.elapsed().as_millis() / 500;
                    if probe.diagnostic && slot <= 3 && (frame == 1 || slot > probe.slot.get()) {
                        probe.slot.set(slot);
                        println!("CANVAS_FRAME {} {}", arena::unix_ms(), paint.value());
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
    let diagnostic = std::env::var_os("ARENA_SCENE_DIAGNOSTIC").is_some();
    // Windows answers its producer's boundaries with the last paint, so the
    // probe is part of every Windows run; macOS keeps it diagnostic-only.
    let probe = (diagnostic || cfg!(target_os = "windows")).then(|| {
        std::rc::Rc::new(PaintProbe {
            started: std::time::Instant::now(),
            frames: Default::default(),
            slot: Default::default(),
            last: Default::default(),
            diagnostic,
        })
    });
    let witness = probe.clone();
    arena::scene::run(
        "arena — canvas",
        "canvas",
        Dial { probe },
        args,
        |_| {},
        move || {
            witness.as_ref().map_or_else(
                || serde_json::json!({}),
                |probe| {
                    serde_json::json!({"frames": probe.frames.get(),
                        "last_paint": probe.last.get().map(Paint::value)})
                },
            )
        },
    );
}
