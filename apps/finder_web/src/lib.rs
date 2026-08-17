//! The finder, in a browser tab — the same scene the desktop runs:
//! ten thousand rows behind the virtual window, a live filter, spring
//! animations, all rasterized by the engine and blitted to a canvas.
#![cfg(target_arch = "wasm32")]

use std::rc::Rc;

use bunny_ui::prelude::*;

fn matches(dir: &str, name: &str, needle: &str) -> bool {
    let mut haystack = dir.chars().chain(name.chars()).map(|c| c.to_ascii_lowercase());
    needle.chars().map(|c| c.to_ascii_lowercase()).all(|wanted| haystack.any(|c| c == wanted))
}

#[derive(Clone)]
struct Finder {
    query: State<String>,
    selected: State<usize>,
    visible: State<Rc<Vec<usize>>>,
    files: Rc<Vec<(Rc<str>, Rc<str>)>>,
}

impl Component for Finder {
    fn body(self, _ctx: &Context) -> impl View {
        let files = Rc::clone(&self.files);
        let visible = self.visible.get();
        let count = visible.len();
        let selected = self.selected;
        let selected_index = selected.get().min(count.saturating_sub(1));
        let id_files = Rc::clone(&files);
        let id_visible = Rc::clone(&visible);
        vstack!(
            hstack!(
                text("›").foreground_color(theme::accent()),
                text_field("Search ten thousand files…", self.query.binding()).monospaced(),
                count_meter(count),
            )
            .spacing(10.0)
            .alignment(VerticalAlignment::Center)
            .padding_length(10.0),
            virtual_list(
                count,
                move |row| {
                    let (name, dir) = &id_files[id_visible[row]];
                    format!("{dir}{name}")
                },
                move |row| {
                    let (name, dir) = &files[visible[row]];
                    let on = row == selected_index;
                    hstack!(
                        text(name.clone()).foreground_color(theme::fg()),
                        text(dir.clone())
                            .monospaced()
                            .foreground_color(theme::fg_secondary()),
                        spacer(),
                    )
                    .spacing(8.0)
                    .alignment(VerticalAlignment::Center)
                    .padding_edge(Edge::Leading, 12.0)
                    .padding_edge(Edge::Trailing, 12.0)
                    .padding_edge(Edge::Top, 6.0)
                    .padding_edge(Edge::Bottom, 6.0)
                    .background_color(if on { theme::row_pressed() } else { CLEAR })
                    .background_hovered(theme::row_hover())
                    .animated(Spring::snappy())
                    .on_click(move || selected.set(row))
                },
            )
            .reveal(selected_index),
        )
        .alignment(HorizontalAlignment::Leading)
        .background_color(theme::panel())
        .on_change(
            {
                let query = self.query;
                move || query.get()
            },
            false,
            {
                let files = Rc::clone(&self.files);
                let cache = self.visible;
                move |_, query: &String| {
                    cache.set(Rc::new(
                        (0..files.len())
                            .filter(|index| {
                                let (name, dir) = &files[*index];
                                query.is_empty() || matches(dir, name, query)
                            })
                            .collect(),
                    ));
                }
            },
        )
    }
}

const CLEAR: Color = Color { r: 0, g: 0, b: 0, a: 0 };

/// The visible count, drawn as five digit bars — custom pixels. On the
/// Dom page this subtree claims a CANVAS ISLAND (`.rendering(Gpu)`):
/// our layout positions the element, our rasterizer fills it, and the
/// bars redraw live as the filter types. On the canvas page the
/// modifier dissolves — everything is pixels there already.
fn count_meter(count: usize) -> impl View {
    let digit = |place: u32| ((count / 10usize.pow(place)) % 10) as f64;
    let bar = |place: u32| {
        spacer()
            .frame(6.0, 6.0 + digit(place) * 2.0)
            .background_color(theme::accent())
            .corner_radius(2.0)
    };
    hstack!(bar(4), bar(3), bar(2), bar(1), bar(0))
        .spacing(2.0)
        .alignment(VerticalAlignment::Bottom)
        .rendering(Rendering::Gpu)
}

fn finder() -> Finder {
    let files: Rc<Vec<(Rc<str>, Rc<str>)>> = Rc::new(
        (0..10_000)
            .map(|index| {
                (
                    Rc::from(format!("file_{index:04}.rs")),
                    Rc::from(format!("src/mod_{:02}/", index % 100)),
                )
            })
            .collect(),
    );
    Finder {
        query: State::new(String::new()),
        selected: State::new(0),
        visible: State::new(Rc::new((0..10_000).collect())),
        files,
    }
}

/// The glue calls this once, with the canvas geometry.
#[unsafe(no_mangle)]
pub extern "C" fn start(width: f64, height: f64, scale: f64) {
    bunny_ui_web::start(width, height, scale, finder());
}

/// The Dom page calls this one — the SAME scene, rendered at home.
/// `scale` rasters the canvas islands at the device's density.
#[unsafe(no_mangle)]
pub extern "C" fn start_dom(width: f64, height: f64, scale: f64) {
    bunny_ui_web::start_dom(width, height, scale, finder());
}
