//! A window that stands on a blur of what is behind it.
//!
//! ```sh
//! cargo run -p bunny-ui-linux --example blur_window_linux
//! cargo run -p bunny-ui-linux --example blur_window_linux -- --clear
//! cargo run -p bunny-ui-linux --example blur_window_linux -- --opaque
//! cargo run -p bunny-ui-linux --example blur_window_linux -- --light
//! cargo run -p bunny-ui-linux --example blur_window_linux -- --drive
//! ```
//!
//! What to look at:
//! 1. The first line of the scene says what the desktop serves. A blur
//!    is the compositor's to give: KDE Plasma gives one, on Wayland and
//!    on X11. A desktop that cannot blur answers with an opaque window,
//!    never with a clear one: a scene drawn to stand on a blur cannot be
//!    read over a sharp desktop.
//! 2. Where the blur is served, the desktop shows through the window,
//!    blurred. Drag the window over something with sharp edges: the
//!    edges soften behind it, the text on it stays sharp.
//! 3. The canvas is the theme's own, thinned to 60 %. That alpha is the
//!    whole of what the app says: no view in the scene knows the window
//!    is not opaque.
//! 4. The two stacks are the same three shades, one inside the other,
//!    each at the same alpha. On the left each shade adds its alpha to
//!    the ones under it and mixes with their colour, so the blur fades
//!    with every step in: that is source-over, and it is the default.
//!    On the right each shade REPLACES what is under it
//!    (`.background_blend(BackgroundBlend::Replace)`): the colour
//!    changes from one to the next and the blur shows through all three
//!    at the one strength a shade names, however deep it sits.
//! 5. `--clear` is the same window with no blur: what is behind it shows
//!    sharp. Every Wayland compositor serves it; X11 serves it where a
//!    compositing manager runs. `--opaque` is the window every other
//!    example opens, and the alpha of the canvas is ignored.
//! 6. The count runs to eight and stops. Every step repaints its own
//!    small rect over the blur and leaves no ghost of the digit before
//!    it — on the GPU roads, and on the CPU road (`BUNNY_PRESENT=cpu`),
//!    where a damaged rect is cleared back to the translucent canvas
//!    before it is painted again.
//!
//! With `--drive` the window reads itself: it prints the background it
//! asked for and the one it was given, checks that the two agree with
//! what the desktop said it serves before the window opened, waits for
//! the eight repaints to be presented, and exits 0 when every line
//! holds. `--expect opaque|clear|blurred` adds the one answer a known
//! desktop must give.

#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

use std::cell::Cell;
use std::rc::Rc;

use bunny_ui::layout::Color;
use bunny_ui::prelude::*;
use bunny_ui::window::WindowBackground;

/// One panel of a stack: a veil, and whatever stands in it. `blend` says
/// whether the veil lies over what is under it or takes its place.
fn panel(label: &'static str, veil: Color, blend: BackgroundBlend, inside: impl View) -> impl View {
    vstack!(text(label).font(Font::Caption), inside)
        .spacing(8.0)
        .alignment(HorizontalAlignment::Leading)
        .padding_length(12.0)
        .background_color(veil)
        .corner_radius(12.0)
        .background_blend(blend)
}

/// Three shades of one window, each inside the one before.
fn stack(title: &'static str, shades: [Color; 3], blend: BackgroundBlend) -> impl View {
    let [outer, middle, inner] = shades;
    vstack!(
        text(title).font(Font::Headline),
        panel(
            "a shade",
            outer,
            blend,
            panel("a second, inside it", middle, blend, panel("a third", inner, blend, text("the blur"))),
        ),
    )
    .spacing(8.0)
    .alignment(HorizontalAlignment::Leading)
}

/// What the `--drive` sheet reads: the ask, what the desktop said it
/// serves before the window opened, what the open window was given, and
/// the answer `--expect` named.
#[derive(Clone)]
struct Drive {
    asked: WindowBackground,
    predicted: WindowBackground,
    given: Rc<Cell<Option<WindowBackground>>>,
    expected: Option<WindowBackground>,
}

#[derive(Clone)]
struct Scene {
    served: &'static str,
    dark: bool,
    count: State<u32>,
    drive: Option<Drive>,
}

impl Component for Scene {
    fn body(self) -> impl View {
        // three shades at ONE alpha: what differs from one to the next is
        // the colour, never how much of the blur it lets through
        let alpha = if self.dark { 64 } else { 56 };
        let shades = if self.dark {
            [Color::rgb(255, 255, 255), Color::rgb(110, 160, 255), Color::rgb(255, 140, 110)]
        } else {
            [Color::rgb(0, 0, 0), Color::rgb(20, 70, 190), Color::rgb(190, 60, 30)]
        }
        .map(|shade| shade.with_alpha(alpha));
        let stacks = hstack!(
            stack("over: the alphas add up", shades, BackgroundBlend::Over),
            stack("replace: each is its own", shades, BackgroundBlend::Replace),
        )
        .spacing(16.0)
        .alignment(VerticalAlignment::Top);
        let count = self.count;
        let drive = self.drive;
        vstack!(
            text!("The window stands on: {}", self.served).font(Font::Title),
            text("The canvas is the theme's, at 60 %. Nothing else in the scene knows."),
            stacks,
            text!("repainted {} times over the blur", count).font(Font::Title),
            spacer(),
        )
        .spacing(16.0)
        .alignment(HorizontalAlignment::Leading)
        .padding_length(24.0)
        .task(move || async move {
            for _ in 0..8 {
                task::sleep(std::time::Duration::from_millis(250)).await;
                count.add(1);
            }
        })
        .task(move || {
            let drive = drive.clone();
            async move {
                if let Some(drive) = drive {
                    // the window is up and the pump is turning
                    task::sleep(std::time::Duration::from_millis(800)).await;
                    the_sheet(drive, count).await;
                }
            }
        })
    }
}

/// The hand reads the window: what it asked for, what it was given, and
/// whether its repaints over the ground it stands on reach the glass.
#[cfg(target_os = "linux")]
async fn the_sheet(sheet: Drive, count: State<u32>) {
    use bunny_ui_linux::drive;
    let Drive { asked, predicted, given, expected } = sheet;
    let start = std::time::Instant::now();
    let stamp = move || format!("{:>6}ms", start.elapsed().as_millis());
    let mut passed = true;
    let mut check = |name: &str, held: bool| {
        println!("[{}] {} — {name}", stamp(), if held { "ok" } else { "FAILED" });
        passed &= held;
    };
    let before = drive::presents();
    check("the first frame reached the glass", before >= 1);
    let given = given.get();
    println!(
        "[{}] background: backend={} asked={asked:?} predicted={predicted:?} served={given:?}",
        stamp(),
        drive::backend()
    );
    check("the window is open and says what it was given", given.is_some());
    let served = given.unwrap_or(WindowBackground::Opaque);
    check("it was given what the desktop said it serves", served == predicted);
    check(
        "a blur that is not there is an opaque window, never a clear one",
        asked != WindowBackground::Blurred || served != WindowBackground::Clear,
    );
    check(
        "an opaque ask is an opaque window",
        asked != WindowBackground::Opaque || served == WindowBackground::Opaque,
    );
    if drive::backend() == "wayland" && asked == WindowBackground::Clear {
        check("a Wayland compositor serves a clear window", served == WindowBackground::Clear);
    }
    if let Some(expected) = expected {
        check(&format!("this desktop serves {expected:?}"), served == expected);
    }
    // the eight repaints: 250 ms apart, each one a damaged rect over the
    // ground the window stands on
    task::sleep(std::time::Duration::from_millis(2600)).await;
    check("the count ran to eight", count.wrappedValue() == 8);
    let after = drive::presents();
    check("and its repaints were presented", after > before);
    println!(
        "[{}] {} — backend={} asked={asked:?} served={served:?} presents={after}",
        stamp(),
        if passed { "the sheet holds" } else { "the sheet has a hole" },
        drive::backend()
    );
    std::process::exit(if passed { 0 } else { 1 });
}

#[cfg(not(target_os = "linux"))]
async fn the_sheet(_sheet: Drive, _count: State<u32>) {}

#[cfg(target_os = "linux")]
fn main() {
    use bunny_ui::theme::{self, Theme};
    use bunny_ui_linux::{App, FreeTypeEngine, LinuxImageEngine, WindowSpec};

    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str| args.iter().any(|arg| arg == name);
    let asked = if flag("--opaque") {
        WindowBackground::Opaque
    } else if flag("--clear") {
        WindowBackground::Clear
    } else {
        WindowBackground::Blurred
    };
    let driven = flag("--drive");
    // `--expect <name>`: the one answer a known desktop must give
    let expected = args.iter().position(|arg| arg == "--expect").map(|at| {
        match args.get(at + 1).map(String::as_str) {
            Some("opaque") => WindowBackground::Opaque,
            Some("clear") => WindowBackground::Clear,
            Some("blurred") => WindowBackground::Blurred,
            other => {
                eprintln!("--expect takes opaque, clear or blurred, not {other:?}");
                std::process::exit(2);
            }
        }
    });
    let dark = !flag("--light");
    let palette = if dark { Theme::dark() } else { Theme::light() };
    // the one line that makes the blur show: a canvas that is not solid
    theme::install(Theme { canvas: palette.canvas.with_alpha(153), ..palette });
    if driven {
        bunny_ui_linux::drive::watchdog(25);
    }

    let app = App::new();
    let runtime = app
        .runtime()
        .text_engine(Rc::new(FreeTypeEngine::new()))
        .image_engine(Rc::new(LinuxImageEngine::new()));
    // what the desktop says it serves, before the window exists
    let predicted = bunny_ui_linux::window_background(asked);
    let served = match predicted {
        WindowBackground::Opaque => "an opaque canvas",
        WindowBackground::Clear => "the desktop, sharp",
        WindowBackground::Blurred => "the desktop, blurred",
    };
    // what the window was given, for the sheet to read once it is open
    let given = Rc::new(Cell::new(None));
    let drive = driven.then(|| Drive { asked, predicted, given: Rc::clone(&given), expected });
    let window = app.open(
        WindowSpec::titled("blur_window").size(620.0, 460.0).background(asked),
        Rc::new(runtime),
        Scene { served, dark, count: State::new(0), drive },
    );
    given.set(app.background(window).ok());
    println!("asked {asked:?}, served {:?}", app.background(window));
    app.run();
}

#[cfg(not(target_os = "linux"))]
fn main() {} // this example is Linux-only
