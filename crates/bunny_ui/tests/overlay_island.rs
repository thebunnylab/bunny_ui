//! A box's overlay — a caret, a mark that moves on its own — is an island
//! of its own when the shell can layer it: placed beside the box, carved
//! out of the scene, and repainted alone when a write reaches only what
//! it read. Where no layer can, the box paints it inline as it always did.

use bunny_ui::custom::{CustomElement, PaintCtx, Painter, custom};
use bunny_ui::layout::{Color, DrawCommand, Point, Proposal, Rect, Size};
use bunny_ui::prelude::*;
use bunny_ui::runtime::Runtime;

const CARET: Rect = Rect { origin: Point { x: 10.0, y: 4.0 }, size: Size { width: 2.0, height: 16.0 } };

/// An editor's shape: a ground that never changes, and a caret that
/// follows a phase of its own.
struct Blinker {
    phase: State<u32>,
}

impl CustomElement for Blinker {
    fn paint(&self, ctx: &PaintCtx, painter: &mut Painter) {
        painter.fill(Rect { origin: Point::ZERO, size: ctx.frame.size }, Color::WHITE);
        // no layer for the caret: the box shows it itself
        if !ctx.overlay_layered {
            let _ = self.paint_overlay(ctx, painter);
        }
    }

    fn paint_overlay(&self, _ctx: &PaintCtx, painter: &mut Painter) -> Option<Rect> {
        if self.phase.get() % 2 == 0 {
            painter.fill(CARET, Color::BLACK);
        }
        Some(CARET)
    }
}

/// A box with no overlay that reads a value as it paints.
struct Plain {
    phase: State<u32>,
}

impl CustomElement for Plain {
    fn paint(&self, ctx: &PaintCtx, painter: &mut Painter) {
        let color = if self.phase.get() % 2 == 0 { Color::WHITE } else { Color::BLACK };
        painter.fill(Rect { origin: Point::ZERO, size: ctx.frame.size }, color);
    }
}

const SIZE: Size = Size { width: 200.0, height: 40.0 };

#[test]
fn an_overlay_is_an_island_when_the_shell_layers_it() {
    let phase = State::new(0u32);
    let runtime = Runtime::new();
    runtime.set_overlay_layers(true);
    let view = custom(Blinker { phase }).frame(SIZE.width, SIZE.height).id("editor");
    let result = runtime.settled_layout(&view, Proposal::exact(SIZE));

    let overlay = result.customs.iter().find(|placement| placement.overlay).expect("the overlay is placed");
    assert!(overlay.path.ends_with("/overlay"), "{}", overlay.path);
    assert_eq!(overlay.frame, CARET, "it covers what the overlay painted");
    assert_eq!(result.customs.iter().filter(|placement| !placement.overlay).count(), 1, "beside the box");
    assert_eq!(runtime.live_slices().len(), 1, "its commands are carved out of the scene");
    let (from, to) = overlay.slice;
    assert!(to > from, "the caret's fill is in the slice");
    assert!(
        result.display.iter().skip(from).take(to - from).any(|command| matches!(command, DrawCommand::FillRect { rect, .. } if *rect == CARET)),
        "the slice holds the caret"
    );

    // a frame paints everything and takes the dirty paints with it
    let _ = runtime.display_frame(&view, SIZE);
    assert!(!runtime.frame_need().paints);

    // a write that reaches only the overlay's paint: the island, not the scene
    phase.set(1);
    let need = runtime.frame_need();
    assert!(need.paints, "{need:?}");
    assert!(!need.wrote && !need.dirty, "no view read it: {need:?}");
    assert!(need.only_paints(), "{need:?}");
    let (blits, plain) = runtime.repaint_dirty_paints(1);
    assert!(!plain, "no plain box read it");
    assert_eq!(blits.len(), 1, "the overlay alone repaints");
    assert_eq!((blits[0].width, blits[0].height), (2, 16), "at the overlay's own size");
    assert_eq!(blits[0].frame, CARET, "at the overlay's place");
    assert!(!runtime.frame_need().paints, "taken");

    // the same picture twice is dropped by the ledger: phase 1 → 3 both hide the caret
    phase.set(3);
    let (blits, _) = runtime.repaint_dirty_paints(1);
    assert!(blits.is_empty(), "the ledger drops a step whose picture did not change");
}

#[test]
fn a_plain_box_that_reads_takes_the_frames_road() {
    let phase = State::new(0u32);
    let runtime = Runtime::new();
    runtime.set_overlay_layers(true);
    let view = custom(Plain { phase }).frame(SIZE.width, SIZE.height).id("meter");
    let result = runtime.settled_layout(&view, Proposal::exact(SIZE));
    assert!(result.customs.iter().all(|placement| !placement.overlay), "no overlay, no island");
    let _ = runtime.display_frame(&view, SIZE);

    phase.set(1);
    let need = runtime.frame_need();
    assert!(need.paints && need.only_paints(), "{need:?}");
    let (blits, plain) = runtime.repaint_dirty_paints(1);
    assert!(blits.is_empty());
    assert!(plain, "a box that is no island needs the frame");
}

#[test]
fn without_layers_the_overlay_paints_inline() {
    let phase = State::new(0u32);
    let runtime = Runtime::new();
    let view = custom(Blinker { phase }).frame(SIZE.width, SIZE.height).id("editor");
    let result = runtime.settled_layout(&view, Proposal::exact(SIZE));
    assert!(result.customs.iter().all(|placement| !placement.overlay), "no island without a layer");
    assert!(runtime.live_slices().is_empty());
    assert!(
        result.display.iter().any(|command| matches!(command, DrawCommand::FillRect { rect, .. } if *rect == CARET)),
        "the box painted its caret itself"
    );
}
