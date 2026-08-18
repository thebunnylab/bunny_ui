//! The scene draws its own window chrome: no system title bar, native
//! traffic lights at the corner, and a bar that belongs to the app —
//! tabs, actions, identity. Drag the window by the bar (buttons still
//! click); everything below is a normal scene.
//!
//! ```sh
//! cargo run -p bunny-ui-macos --example chrome_window
//! ```

#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

use std::rc::Rc;

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;
#[cfg(target_os = "macos")]
use bunny_ui_macos::Chrome;

const BAR_H: f64 = 44.0;
/// Room for the native traffic lights at the top-left corner.
const LIGHTS_W: f64 = 78.0;

#[derive(Clone, Copy)]
struct App {
    tab: State<usize>,
}

impl Component for App {
    fn body(self, _ctx: &Context) -> impl View {
        let tab = self.tab;
        let active = tab.get();
        let titles = ["Code", "Pareto", "Infra", "Atrium"];

        // the strip is a dynamic COLLECTION laid along the bar: same
        // identity contract as a list, other axis. A FLAT tab is a
        // click target without the button's outfit — `.on_click` arms
        // the press and the style is all ours; a `button(...)` here
        // would paint its own control chrome INSIDE this background
        // and the padding would read as a lopsided ring
        let tabs = for_each(
            titles.into_iter().enumerate().collect::<Vec<_>>(),
            |(_, title)| title.to_string(),
            move |(index, title)| {
                let index = *index;
                let on = index == active;
                text(*title)
                    .foreground_color(if on { Color::WHITE } else { theme::fg_secondary() })
                    // faint until the pointer arrives (the active one
                    // is already white and stays)
                    .foreground_hovered(Color::WHITE)
                    .padding_edge(Edge::Leading, 12.0)
                    .padding_edge(Edge::Trailing, 12.0)
                    .padding_edge(Edge::Top, 6.0)
                    .padding_edge(Edge::Bottom, 6.0)
                    .background_color(if on { theme::accent() } else { CLEAR })
                    .background_hovered(theme::row_hover())
                    .corner_radius(8.0)
                    .animated(Spring::snappy())
                    .on_click(move || tab.set(index))
            },
        )
        .horizontal()
        .spacing(8.0);

        // THE bar: the scene's own chrome. The whole strip drags the
        // window; the buttons on it still click (a press only drags
        // where nothing interactive wins).
        let bar = hstack!(
            spacer().frame(LIGHTS_W, 1.0),
            text("bunny").bold(),
            spacer(),
            tabs,
            spacer(),
            text("de")
                .foreground_color(Color::WHITE)
                .padding_length(6.0)
                .background_color(theme::accent())
                .corner_radius(12.0),
        )
        .spacing(8.0)
        .alignment(VerticalAlignment::Center)
        .padding_edge(Edge::Trailing, 12.0)
        .frame_max(f64::INFINITY, BAR_H, Alignment::Leading)
        .background_color(theme::panel())
        .window_drag_region();

        let body = vstack!(
            text(format!("the {} tab", ["code", "pareto", "infra", "atrium"][active]))
                .font(Font::Title)
                .foreground_color(theme::fg_secondary()),
            text("drag the window by the bar above")
                .foreground_color(theme::placeholder()),
        )
        .spacing(8.0);

        // the well carries a glow: a ramp declared in the box's own
        // proportions, so it follows every resize without a number
        // changing — and every rendering paints it (our rasterizer on
        // the desktop, a CSS gradient on the element lowering)
        let well = zstack!(
            spacer()
                .background_color(theme::canvas())
                .background_gradient(
                    Gradient::radial(theme::accent(), theme::accent().fade())
                        .center(UnitPoint::TOP)
                        .radius(0.0, 520.0),
                ),
            body,
        );

        vstack!(bar, spacer().frame(1.0, 1.0).background_color(theme::divider()), well)
    }
}

const CLEAR: Color = Color { r: 0, g: 0, b: 0, a: 0 };

#[cfg(target_os = "macos")]
fn main() {
    theme::install(Theme::dark());
    let runtime = Runtime::new()
        .text_engine(Rc::new(bunny_ui_macos::CoreTextEngine::new()))
        .image_engine(Rc::new(bunny_ui_macos::CoreGraphicsImageEngine::new()));
    bunny_ui_macos::run_window_chrome(
        "bunny — scene chrome",
        Size { width: 860.0, height: 560.0 },
        Chrome::Scene,
        runtime,
        App { tab: State::new(0) },
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {} // this example is macOS-only
