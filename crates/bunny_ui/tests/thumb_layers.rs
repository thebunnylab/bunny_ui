//! A scrollbar's thumb on a layer of its own. When the shell can show one
//! there, a thumb that nothing covers and that stands on one colour leaves
//! the scene's list, and the runtime hands it to the shell instead — with
//! the opaque colour its pixels would have had — so a list that grows below
//! the fold is the same list, and the frame is the thumb moving. A thumb
//! that something covers, that a rounded clip cuts, that stands on more than
//! one colour, or that a popover carries, stays where the scene drew it; and
//! every range the scene keeps into its list still points at the commands it
//! named.

use bunny_ui::custom::{CustomElement, PaintCtx, Painter, custom};
use bunny_ui::layout::{Color, DisplayList, DrawCommand, Point, Proposal, Rect, Side, Size};
use bunny_ui::prelude::*;
use bunny_ui::runtime::Runtime;

const WINDOW: Size = Size { width: 300.0, height: 200.0 };

/// A chat: twenty-point lines in a region two hundred points tall.
#[derive(Clone, Copy)]
struct Chat {
    lines: State<usize>,
}

impl Component for Chat {
    fn body(self) -> impl View {
        let lines: Vec<usize> = (0..self.lines.get()).collect();
        scroll(for_each(lines, |line| line.to_string(), |line| {
            text(format!("line {line}")).frame_height(20.0)
        }))
        .id("chat")
    }
}

/// The shell's runtime: the unseen cut on, the thumb layers as asked.
fn shell(layers: bool) -> Runtime {
    let runtime = Runtime::new();
    runtime.drop_unseen();
    runtime.set_thumb_layers(layers);
    runtime
}

/// A fill over a ground, the way the GPU blends it.
fn over(ground: Color, ink: Color) -> Color {
    let alpha = ink.a as u32;
    let mix = |under: u8, top: u8| ((top as u32 * alpha + under as u32 * (255 - alpha) + 127) / 255) as u8;
    Color { r: mix(ground.r, ink.r), g: mix(ground.g, ink.g), b: mix(ground.b, ink.b), a: 255 }
}

fn fills(display: &DisplayList, rect: Rect) -> usize {
    display
        .iter()
        .filter(|command| matches!(command, DrawCommand::FillRect { rect: at, .. } if *at == rect))
        .count()
}

#[test]
fn a_thumb_nothing_covers_leaves_the_list_for_a_layer() {
    let chat = Chat { lines: State::new(40) };
    let drawn = shell(false);
    let painted = drawn.display_frame(&chat, WINDOW);
    assert!(drawn.thumbs().is_empty(), "no layers asked, none given");

    let layered = shell(true);
    let lifted = layered.display_frame(&chat, WINDOW);
    let thumbs = layered.thumbs();
    assert_eq!(thumbs.len(), 1, "one region, one thumb");
    let thumb = thumbs[0];
    assert!(thumb.rect.origin.x > WINDOW.width - 12.0, "it hugs the trailing edge: {thumb:?}");
    let theme = bunny_ui::theme::current();
    assert_eq!(thumb.color, over(theme.canvas, theme.scrollbar), "the thumb over the window's floor, opaque");
    assert_eq!(fills(&painted, thumb.rect), 1, "the scene paints it where no layer can");
    assert_eq!(fills(&lifted, thumb.rect), 0, "and leaves it to the layer where one can");
    assert_eq!(lifted.len() + 1, painted.len(), "the thumb is the only command that left");
}

#[test]
fn a_list_that_grows_below_the_fold_is_the_same_list() {
    let lines = State::new(40);
    let chat = Chat { lines };
    let runtime = shell(true);
    let before = runtime.display_frame(&chat, WINDOW);
    let thumb = runtime.thumbs();

    lines.set(41);
    let after = runtime.display_frame(&chat, WINDOW);
    assert_eq!(before.as_slice(), after.as_slice(), "nothing on the glass changed but the thumb");
    let moved = runtime.thumbs();
    assert_eq!(moved.len(), 1);
    assert!(moved[0].rect.size.height < thumb[0].rect.size.height, "the thumb shrank: {moved:?} {thumb:?}");

    // without the layer the same growth is a new list
    let lines = State::new(40);
    let chat = Chat { lines };
    let runtime = shell(false);
    let before = runtime.display_frame(&chat, WINDOW);
    lines.set(41);
    let after = runtime.display_frame(&chat, WINDOW);
    assert_ne!(before.as_slice(), after.as_slice(), "the drawn thumb changed the list");
}

#[test]
fn a_thumb_under_later_paint_stays_in_the_scene() {
    #[derive(Clone, Copy)]
    struct Veiled {
        lines: State<usize>,
    }

    impl Component for Veiled {
        fn body(self) -> impl View {
            zstack!(
                Chat { lines: self.lines },
                rectangle().foreground_color(Color::rgba(255, 0, 0, 40)).frame(WINDOW.width, WINDOW.height),
            )
        }
    }

    let runtime = shell(true);
    let display = runtime.display_frame(&Veiled { lines: State::new(40) }, WINDOW);
    assert!(runtime.thumbs().is_empty(), "a veil painted after it covers it");
    let reference = shell(true);
    let _ = reference.display_frame(&Chat { lines: State::new(40) }, WINDOW);
    let rect = reference.thumbs()[0].rect;
    assert_eq!(fills(&display, rect), 1, "so the scene paints it, under the veil");
}

#[test]
fn a_thumb_in_a_rounded_corner_stays_in_the_scene() {
    #[derive(Clone, Copy)]
    struct Card {
        lines: State<usize>,
        radius: f64,
    }

    impl Component for Card {
        fn body(self) -> impl View {
            Chat { lines: self.lines }.corner_radius(self.radius).clipped()
        }
    }

    // the thumb starts six points down the trailing edge: a curve of
    // twelve cuts into it, a curve of four does not reach it
    let round = shell(true);
    let display = round.display_frame(&Card { lines: State::new(40), radius: 12.0 }, WINDOW);
    assert!(round.thumbs().is_empty(), "the curve may take its pixels");
    assert!(
        display.iter().any(|command| matches!(command, DrawCommand::FillRect { rect, .. } if rect.size.width == 4.0)),
        "it is painted under the curve"
    );
    let gentle = shell(true);
    let _ = gentle.display_frame(&Card { lines: State::new(40), radius: 4.0 }, WINDOW);
    assert_eq!(gentle.thumbs().len(), 1, "clear of the curve, it lifts");
}

#[test]
fn a_popover_keeps_its_thumb_and_its_slice() {
    #[derive(Clone, Copy)]
    struct Anchored {
        open: State<bool>,
    }

    impl Component for Anchored {
        fn body(self) -> impl View {
            scroll(text("page").frame(400.0, 4000.0)).id("page").popover(
                self.open.binding(),
                Side::Trailing,
                move |_| erased(scroll(text("list").frame(160.0, 900.0)).id("pop").frame(180.0, 120.0)),
            )
        }
    }

    let viewport = Proposal::exact(Size { width: 400.0, height: 300.0 });
    let slice_of = |layers: bool| {
        let anchored = Anchored { open: State::new(true) };
        let runtime = shell(layers);
        runtime.render_stable(&anchored);
        let result = runtime.layout(&anchored, viewport);
        let overlay = result.overlays.first().expect("the popover is placed").clone();
        let slice = result.display.as_slice()[overlay.display.0..overlay.display.1].to_vec();
        (slice, runtime.thumbs(), result.display.len())
    };
    let (drawn, none, painted) = slice_of(false);
    let (moved, thumbs, lifted) = slice_of(true);
    assert!(none.is_empty());
    assert_eq!(thumbs.len(), 1, "the page's thumb lifts; the popover's presents on the popover's surface");
    assert!(thumbs[0].rect.origin.x > 380.0, "it is the page's: {thumbs:?}");
    assert_eq!(lifted + 1, painted);
    assert_eq!(moved, drawn, "the popover's slice names the same commands, its own thumb among them");
}

#[test]
fn a_live_box_after_a_lifted_thumb_keeps_its_slice() {
    struct Swatch;

    impl CustomElement for Swatch {
        fn paint(&self, ctx: &PaintCtx, painter: &mut Painter) {
            painter.fill(Rect { origin: Point::ZERO, size: ctx.frame.size }, Color::rgb(0, 128, 255));
        }
    }

    #[derive(Clone, Copy)]
    struct Beside {
        lines: State<usize>,
    }

    impl Component for Beside {
        fn body(self) -> impl View {
            hstack!(Chat { lines: self.lines }.frame(200.0, 200.0), custom(Swatch).frame(100.0, 200.0).id("swatch"))
        }
    }

    let viewport = Proposal::exact(WINDOW);
    let slice_of = |layers: bool| {
        let view = Beside { lines: State::new(40) };
        let runtime = shell(layers);
        runtime.render_stable(&view);
        let result = runtime.layout(&view, viewport);
        let placement = result.customs.first().expect("the box is placed").clone();
        (result.display.as_slice()[placement.slice.0..placement.slice.1].to_vec(), runtime.thumbs())
    };
    let (drawn, _) = slice_of(false);
    let (moved, thumbs) = slice_of(true);
    assert_eq!(thumbs.len(), 1, "the thumb before the box lifts");
    assert!(!drawn.is_empty());
    assert_eq!(moved, drawn, "the box's slice still names its own commands");
}

#[test]
fn a_thumb_on_one_fill_takes_its_colour_and_on_many_stays() {
    #[derive(Clone, Copy)]
    struct Rows {
        lines: State<usize>,
        striped: bool,
    }

    impl Component for Rows {
        fn body(self) -> impl View {
            let lines: Vec<usize> = (0..self.lines.get()).collect();
            let striped = self.striped;
            scroll(for_each(lines, |line| line.to_string(), move |line| {
                let shade = if striped && line % 2 == 0 { Color::hex(0x1C1C21) } else { Color::hex(0x17171C) };
                text(format!("line {line}")).frame(WINDOW.width, 20.0).background_color(shade)
            }))
            .id("rows")
        }
    }

    // every row the same colour, the whole width: one ground
    let plain = shell(true);
    let _ = plain.display_frame(&Rows { lines: State::new(40), striped: false }, WINDOW);
    let thumbs = plain.thumbs();
    assert_eq!(thumbs.len(), 1, "one ground under it");
    let theme = bunny_ui::theme::current();
    assert_eq!(thumbs[0].color, over(Color::hex(0x17171C), theme.scrollbar), "the thumb over the rows' colour");

    // stripes: the thumb stands on two colours, and the scene paints it
    let striped = shell(true);
    let display = striped.display_frame(&Rows { lines: State::new(40), striped: true }, WINDOW);
    assert!(striped.thumbs().is_empty(), "two colours under it");
    assert!(
        display.iter().any(|command| matches!(command, DrawCommand::FillRect { color, .. } if *color == theme.scrollbar)),
        "painted in the scene"
    );
}

#[test]
fn a_line_that_runs_under_the_thumb_keeps_it_in_the_scene() {
    #[derive(Clone, Copy)]
    struct Long {
        lines: State<usize>,
        width: usize,
    }

    impl Component for Long {
        fn body(self) -> impl View {
            let lines: Vec<usize> = (0..self.lines.get()).collect();
            let word = "w".repeat(self.width);
            scroll(for_each(lines, |line| line.to_string(), move |_| text(word.clone()).frame_height(20.0)))
                .id("long")
        }
    }

    let short = shell(true);
    let _ = short.display_frame(&Long { lines: State::new(40), width: 4 }, WINDOW);
    assert_eq!(short.thumbs().len(), 1, "short lines end long before the thumb");
    let long = shell(true);
    let _ = long.display_frame(&Long { lines: State::new(40), width: 400 }, WINDOW);
    assert!(long.thumbs().is_empty(), "a line under the thumb mixes what it stands on");
}
