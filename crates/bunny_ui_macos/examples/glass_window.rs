//! Liquid glass, in a window.
//!
//! Glass is the one material a diff cannot review. Every assertion the
//! pane ships with — the layout, the batch order, the parity against
//! the rasterizer — still passes if the lens bends the wrong way, and a
//! pane that bends the wrong way looks *pinched* rather than broken.
//! So it gets a window.
//!
//! ```sh
//! cargo run -p bunny-ui-macos --example glass_window
//! ```
//!
//! Behind the panes: a saturated wash, soft blobs and a ruled grid.
//! Straight lines are what make refraction legible — a blur only
//! softens a line, a lens BENDS it, and the bend is unmistakable at a
//! rim. Click a card to press it (the touch lights), and use the row of
//! chips to change the material under all three.

#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

use std::rc::Rc;

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;

/// The recipes the chips walk through.
const RECIPES: [(&str, fn() -> Glass); 4] = [
    ("regular", || Glass::regular()),
    ("clear", || Glass::clear()),
    ("frosted", || Glass::frosted()),
    ("lens", || {
        Glass::regular()
            .blur(1.0)
            .tint(Color::rgba(255, 255, 255, 14))
            .refraction(26.0, 40.0)
            .chromatic(0.2)
    }),
];

#[derive(Clone, Copy)]
struct App {
    recipe: State<usize>,
    pressed: State<usize>,
}

impl Component for App {
    fn body(self, _ctx: &Context) -> impl View {
        zstack!(wallpaper(), self.panes(), self.chips())
    }
}

impl App {
    /// Three panes of the same material, over three different parts of
    /// the wash — the same glass reads differently over different
    /// scenes, which is the whole point of a material.
    fn panes(self) -> impl UnaryView {
        let glass = RECIPES[self.recipe.get() % RECIPES.len()].1();
        let pressed = self.pressed.get();
        vstack((
            self.card(0, "Liquid glass", "a lens over the scene", glass, pressed == 1),
            self.card(1, "Search", "type to filter the world", glass, pressed == 2),
            self.card(2, "Now playing", "the dock, the pill, the sheet", glass, pressed == 3),
        ))
        .spacing(22.0)
        .padding_length(40.0)
    }

    /// One card. A press adds the touch lights — a flat wash over the
    /// whole pane plus a pool of light in the middle — and both of them
    /// are ZERO until it does.
    fn card(
        self,
        index: usize,
        title: &'static str,
        detail: &'static str,
        glass: Glass,
        down: bool,
    ) -> impl UnaryView {
        let pressed = self.pressed;
        let material = match down {
            true => glass.sheen(0.06).spot(UnitPoint::CENTER, 0.9, 0.18),
            false => glass,
        };
        vstack((
            text(title).font(Font::Title).foreground_color(Color::WHITE),
            text(detail).font(Font::Callout).foreground_color(Color::rgba(255, 255, 255, 190)),
        ))
        .spacing(4.0)
        .alignment(HorizontalAlignment::Leading)
        .padding_length(22.0)
        .frame_width(360.0)
        .corner_radius(28.0)
        .glass(material)
        .on_click(move || pressed.set(if pressed.get() == index + 1 { 0 } else { index + 1 }))
    }

    /// The chips that change the recipe.
    fn chips(self) -> impl UnaryView {
        let recipe = self.recipe;
        let current = recipe.get() % RECIPES.len();
        let row = for_each(
            RECIPES.iter().enumerate().map(|(index, (name, _))| (index, *name)).collect::<Vec<_>>(),
            |(_, name)| name.to_string(),
            move |(index, name)| {
                let index = *index;
                text(*name)
                    .foreground_color(match index == current {
                        true => Color::WHITE,
                        false => Color::rgba(255, 255, 255, 170),
                    })
                    .padding_edge(Edge::Leading, 14.0)
                    .padding_edge(Edge::Trailing, 14.0)
                    .padding_edge(Edge::Top, 8.0)
                    .padding_edge(Edge::Bottom, 8.0)
                    .corner_radius(16.0)
                    // a chip is glass too, and a small one: a thin band
                    // and a light tint keep a label legible through it
                    .glass(match index == current {
                        true => Glass::regular().tint(Color::rgba(255, 255, 255, 64)),
                        false => Glass::clear().refraction(8.0, 12.0),
                    })
                    .on_click(move || recipe.set(index))
            },
        )
        .horizontal()
        .spacing(10.0);
        vstack((spacer(), row.padding_length(28.0)))
    }
}

/// A saturated wash with soft blobs, a ruled grid and a scatter of
/// specks. A pane over a flat colour proves nothing.
fn wallpaper() -> impl UnaryView {
    zstack!(
        spacer().background_gradient(
            Gradient::linear(Color::hex(0x141C5E), Color::hex(0x8E2E86))
                .direction(UnitPoint::TOP_LEADING, UnitPoint::BOTTOM_TRAILING),
        ),
        blobs(),
        grid(),
    )
}

/// The blobs are HALOS: a small transparent core carrying an enormous
/// soft shadow, which is a real falloff instead of a hard-edged disc.
fn blobs() -> impl UnaryView {
    let spots: Vec<(usize, f64, f64, f64, u32)> = vec![
        (0, 0.10, 0.16, 150.0, 0xFF6A3D),
        (1, 0.72, 0.24, 130.0, 0x1FA8C9),
        (2, 0.30, 0.78, 170.0, 0xF2960F),
        (3, 0.86, 0.70, 140.0, 0xE02F6E),
    ];
    for_each(
        spots,
        |(index, ..)| index.to_string(),
        |(_, x, y, reach, color)| {
            let (x, y, reach, color) = (*x, *y, *reach, *color);
            spacer().background_gradient(
                Gradient::radial(Color::hex_a((color << 8) | 0xB0), Color::hex_a(color << 8).fade())
                    .center(UnitPoint::new(x, y))
                    .radius(0.0, reach),
            )
        },
    )
    .vertical()
}

/// The ruled grid: what a lens bends and a blur only softens.
fn grid() -> impl UnaryView {
    let rows = for_each(
        (0..26).collect::<Vec<i32>>(),
        |index: &i32| index.to_string(),
        |_| {
            spacer()
                .frame_height(1.0)
                .background_color(Color::rgba(255, 255, 255, 46))
                .padding_edge(Edge::Bottom, 23.0)
        },
    )
    .vertical();
    let columns = for_each(
        (0..24).collect::<Vec<i32>>(),
        |index: &i32| index.to_string(),
        |_| {
            spacer()
                .frame_width(1.0)
                .background_color(Color::rgba(255, 255, 255, 34))
                .padding_edge(Edge::Trailing, 39.0)
        },
    )
    .horizontal();
    zstack!(rows, columns)
}

#[cfg(target_os = "macos")]
fn main() {
    theme::install(Theme::dark());
    let runtime = Runtime::new()
        .text_engine(Rc::new(bunny_ui_macos::CoreTextEngine::new()))
        .image_engine(Rc::new(bunny_ui_macos::CoreGraphicsImageEngine::new()));
    bunny_ui_macos::run_window_with(
        "bunny — liquid glass",
        Size { width: 900.0, height: 620.0 },
        runtime,
        App { recipe: State::new(0), pressed: State::new(0) },
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {} // this example is macOS-only
