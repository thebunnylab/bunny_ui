//! Recording sites must not change what a style paints.
//!
//! `.background_color(a).corner_radius(r)` fuses into ONE `Styled` node, so
//! the radius rounds that background. A recording pass wraps every modifier
//! in a `Sited` node; the next style must still reach the one beneath it,
//! or the radius lands on a node with nothing to round and the dash on a
//! border it never sees — what Studio and `bunny preview` then show.
#![cfg(feature = "canvas")]

use bunny_ui_core::BorderStyle;
use bunny_ui_core::layout::{Corners, DisplayList, DrawCommand, Size};
use bunny_ui_core::prelude::*;

const SIZE: Size = Size { width: 200.0, height: 100.0 };

fn painted(view: &impl View, sites: bool) -> DisplayList {
    let runtime = Runtime::scene("sited_styles");
    runtime.record_sites(sites);
    runtime.display_frame(view, SIZE)
}

fn rounded() -> impl UnaryView {
    text("A").frame(60.0, 60.0).background_color(Color::rgb(169, 121, 255)).corner_radius(12.0)
}

fn dashed() -> impl UnaryView {
    text("B")
        .frame(60.0, 60.0)
        .corner_radius(12.0)
        .border(Color::rgb(232, 227, 246), 2.0)
        .border_style(BorderStyle::Dashed)
}

#[test]
fn a_recorded_background_keeps_its_corner_radius() {
    let ordinary = painted(&rounded(), false);
    let recorded = painted(&rounded(), true);
    assert_eq!(ordinary.as_slice(), recorded.as_slice());
    let radius = recorded.as_slice().iter().find_map(|command| match command {
        DrawCommand::FillRect { corner_radius, .. } => Some(*corner_radius),
        _ => None,
    });
    assert_eq!(radius, Some(Corners::all(12.0)));
}

#[test]
fn a_recorded_border_keeps_its_style_and_radius() {
    assert_eq!(painted(&dashed(), false).as_slice(), painted(&dashed(), true).as_slice());
}

#[test]
fn every_fused_modifier_keeps_its_own_site() {
    let runtime = Runtime::scene("sited_styles_sites");
    runtime.record_sites(true);
    let _ = runtime.display_frame(&dashed(), SIZE);
    let sites = runtime.sites();
    for modifier in ["frame", "corner_radius", "border", "border_style"] {
        assert!(sites.iter().any(|site| site.modifier == Some(modifier)), "{modifier}");
    }
}
