use bunny_ui::layout::DrawCommand;
use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;

const SIZE: Size = Size { width: 200.0, height: 200.0 };
const INK: Color = Color::rgba(0, 0, 0, 90);

fn frame(runtime: &Runtime, name: &str) -> Rect {
    runtime.test_frames().into_iter().find(|(id, _)| &**id == name).map(|(_, rect)| rect).expect(name)
}

fn grown(rect: Rect, x: f64, y: f64, by: f64) -> Rect {
    Rect {
        origin: Point { x: rect.origin.x + x - by, y: rect.origin.y + y - by },
        size: Size { width: rect.size.width + 2.0 * by, height: rect.size.height + 2.0 * by },
    }
}

/// The pixel pipelines cast a layer as the house halo, `blur` wide,
/// round the box's outline moved by the offset and grown by the spread
/// — a negative one shrinks it, corners and all.
#[test]
fn a_layer_casts_the_halo_round_its_moved_and_grown_outline() {
    let runtime = Runtime::new();
    let view = rectangle()
        .frame(40.0, 20.0)
        .corner_radius(4.0)
        .box_shadow(BoxShadow::new(INK).offset(0.0, 10.0).blur(8.0).spread(-2.0))
        .test_id("card");
    let display = runtime.display_frame(&view, SIZE);
    let card = frame(&runtime, "card");
    let halos: Vec<_> = display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::Shadow { rect, radius, color, corner_radius } => Some((*rect, *radius, *color, *corner_radius)),
            _ => None,
        })
        .collect();
    assert_eq!(halos, [(grown(card, 0.0, 10.0, -2.0), 8.0, INK, Corners::all(2.0))]);
    assert!(!display.iter().any(|command| matches!(command, DrawCommand::StrokeRect { .. })), "a shrunk outline adds no band");
}

/// A spread with no blur is a solid ring round the box, exact; an inset
/// layer lies inward over the ground, under what the box holds.
#[test]
fn a_spread_is_a_solid_ring_and_an_inset_layer_lies_inward() {
    let runtime = Runtime::new();
    let view = vstack!(
        spacer()
            .frame(8.0, 8.0)
            .background_color(Color::WHITE)
            .corner_radius(4.0)
            .box_shadow(BoxShadow::new(INK).spread(4.0))
            .test_id("dot"),
        text("pressed").background_color(Color::WHITE).box_shadow(BoxShadow::new(INK).spread(2.0).inset()).test_id("key"),
    )
    .spacing(20.0);
    let display = runtime.display_frame(&view, SIZE);
    let dot = frame(&runtime, "dot");
    let key = frame(&runtime, "key");
    let rings: Vec<_> = display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::StrokeRect { rect, width, corner_radius, color } => Some((*rect, *width, *corner_radius, *color)),
            _ => None,
        })
        .collect();
    assert_eq!(rings, [(grown(dot, 0.0, 0.0, 4.0), 4.0, Corners::all(8.0), INK), (key, 2.0, Corners::ZERO, INK)]);
    assert!(!display.iter().any(|command| matches!(command, DrawCommand::Shadow { .. })), "no blur, no halo");
    let at = |wanted: fn(&DrawCommand, Rect) -> bool| display.iter().position(|command| wanted(command, key)).expect("drawn");
    let ground = at(|command, key| matches!(command, DrawCommand::FillRect { rect, .. } if *rect == key));
    let inward = at(|command, key| matches!(command, DrawCommand::StrokeRect { rect, .. } if *rect == key));
    let words = at(|command, _| matches!(command, DrawCommand::TextLine { .. }));
    assert!(ground < inward && inward < words, "over the ground, under the text");
}

/// The layers paint bottom first, as CSS stacks them: the one written
/// first lands on top.
#[test]
fn the_first_layer_written_paints_on_top() {
    let runtime = Runtime::new();
    let near = Color::rgba(255, 0, 0, 255);
    let far = Color::rgba(0, 0, 255, 255);
    let view = rectangle().frame(40.0, 20.0).box_shadow(BoxShadow::new(near).blur(2.0)).box_shadow(BoxShadow::new(far).blur(9.0));
    let display = runtime.display_frame(&view, SIZE);
    let inks: Vec<Color> = display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::Shadow { color, .. } => Some(*color),
            _ => None,
        })
        .collect();
    assert_eq!(inks, [far, near]);
}

/// Four layers fit one box; a fifth nests the box in another, so the
/// page writes two shadow lists and the printed tree names every layer.
#[test]
fn a_fifth_layer_nests_a_second_box() {
    let layer = |spread: f64| BoxShadow::new(INK).spread(spread);
    let view = rectangle()
        .frame(40.0, 20.0)
        .box_shadow(layer(1.0))
        .box_shadow(layer(2.0))
        .box_shadow(layer(3.0))
        .box_shadow(layer(4.0))
        .box_shadow(layer(5.0));
    let page = bunny_ui::ssr::render(&view, SIZE);
    assert_eq!(page.css.matches("box-shadow:").count(), 2, "{}", page.css);
    assert!(page.css.contains("box-shadow:0px 0px 0px 1px rgba(0, 0, 0, 0.35294117647058826),0px 0px 0px 2px"), "{}", page.css);
    let printed = Runtime::new().render_stable(&view);
    assert_eq!(printed.matches("[.boxShadow(").count(), 5, "{printed}");
    assert!(Runtime::new().render_stable(&rectangle().box_shadow(layer(1.0).inset())).contains("[.boxShadow(inset, x: 0"));
}

/// A line along one edge paints a bar of its width inward along that
/// edge; the leading one stands on the right in a right-to-left scene.
#[test]
fn an_edge_paints_a_bar_along_its_side() {
    let rule = Color::rgb(1, 2, 3);
    let bars = |runtime: &Runtime, edge: Edge| {
        let view = spacer().frame(40.0, 20.0).border_edge(edge, rule, 2.0).test_id("bar");
        let display = runtime.display_frame(&view, SIZE);
        let bar = frame(runtime, "bar");
        let fills: Vec<Rect> = display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::FillRect { rect, color, .. } if *color == rule => Some(*rect),
                _ => None,
            })
            .collect();
        (bar, fills)
    };
    let at = |bar: Rect, x: f64, y: f64, width: f64, height: f64| Rect {
        origin: Point { x: bar.origin.x + x, y: bar.origin.y + y },
        size: Size { width, height },
    };
    let (bar, fills) = bars(&Runtime::new(), Edge::Bottom);
    assert_eq!(fills, [at(bar, 0.0, 18.0, 40.0, 2.0)]);
    let (bar, fills) = bars(&Runtime::new(), Edge::Leading);
    assert_eq!(fills, [at(bar, 0.0, 0.0, 2.0, 20.0)]);
    let rtl = Runtime::new();
    rtl.set_layout_direction(Some(LayoutDirection::RightToLeft));
    let (bar, fills) = bars(&rtl, Edge::Leading);
    assert_eq!(fills, [at(bar, 38.0, 0.0, 2.0, 20.0)]);
    assert!(Runtime::new().render_stable(&spacer().border_edge(Edge::Bottom, rule, 2.0)).contains("[.border(.bottom, "));
}
