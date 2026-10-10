# bunny_ui

A declarative UI framework for Rust with fine-grained reactivity, inspired by SwiftUI.

Write views as value types and let reads establish their reactive dependencies.
`State<T>` is Bunny UI's signal primitive: changing a value invalidates its
subscribers. A text node can subscribe directly, so a counter update can change
its label without rerunning the component body.

- **Fine-grained signals:** `State`, two-way `Binding` projections and read-only
  `Derived` values share one dependency system.
- **Plain Rust MVVM:** presentation structs hold independent reactive properties
  and command methods; views receive them through ordinary fields.
- **Explicit composition:** typed components declare `body(self)` and request
  ambient values only when needed with `environment::<T>()`.
- **Scoped work:** views can own state and asynchronous tasks, with cleanup when
  their mounted identity leaves the tree.

## Install

```bash
cargo add bunny-ui
```

`bunny-ui` is the one crate an application adds: the core and the shell of the
target it compiles for — macOS, iOS, Windows, Linux, Android or the web — with
nothing to pick by hand. A crate that only builds views, such as a component
library or a theme, leaves the shell to the application:

```toml
bunny-ui = { version = "0.2", default-features = false }
```

To start an app with every platform's files already in place, the `bunny`
command creates one:

```bash
cargo install bunny-cli
bunny new my_app
```

## Quick look

```rust
// src/main.rs
use bunny_ui::prelude::*;

// A component: its state, and the body that shows it.
#[derive(Clone, Copy)]
struct Counter {
    count: State<i32>,
}

impl Component for Counter {
    fn body(self) -> impl View {
        vstack!(
            text!("Count: {}", self.count),
            button(text("Tap"), move || self.count.add(1)),
        )
    }
}

// The first view the app shows.
fn home() -> impl View {
    Counter { count: State::new(0) }
}

// Writes `run()`: it opens the window and puts `home()` in it.
bunny_ui::app!(home, bunny_ui::AppConfig::new().size(280.0, 180.0));

fn main() {
    run()
}
```

`main` calls `run()`, `run()` opens the window, the window shows `home()`, and
`home()` builds the `Counter`. The text node records the read: a tap changes the
state and updates that node without running the body again. `text(self.count)`
also binds directly; `text!("Count: {}", self.count)` adds formatting.

That file runs on macOS, Windows, Linux and iOS. A project from `bunny new`
keeps the same code in `src/lib.rs`, and `src/main.rs` is the one line
`my_app::run()` — the package's name, and the `run` that `app!` wrote. Android
and the web start from the library, not from a `main`: `app!` writes their
entries too, the Android activity and the page's `start` export. `bunny run`
reloads that library hot.

## Signals and subscriptions

The API calls its signal `State<T>`. The subscription belongs to the place that
reads it, which is what determines the scope of an update.

| API | Role | Typical use |
| --- | --- | --- |
| `State<T>` | A reactive value behind a small typed handle | Independent presentation properties |
| `Binding<T>` | A read/write projection, including application getters and setters | Two-way fields and child controls |
| `Derived<T>` | A lazy, read-only computation | Computed presentation values |
| `text(state)` / `text(binding)` / `text(derived)` | A read deferred to the text node | Update a label without rerunning its component body |
| `text!("Count: {}", state)` | Formatting with reads at the text node | Reactive formatted labels |

Reading `state.get()` in `body` subscribes that component to the value. That is
useful when state controls structure, such as whether a panel exists. For a
label, passing the reactive value directly to `text` keeps the subscription on
the label. `text(name.get())` instead takes a string snapshot while the body is
running. A state change does not automatically rebuild the whole application.

Bindings preserve the dependencies of their getter; a binding around ordinary
nonreactive data does not create change notifications by itself. Derived values
compute when read, and subscribe to the state read by that computation. They are
not global memos; text nodes retain their normal caching. See the
[public authoring tests](crates/bunny_ui/tests/authoring.rs) for body and node
execution counters that verify update granularity.

## MVVM with ordinary Rust structs

Keep presentation properties and commands on a model and supply it through a
`vm` field. Each property remains independently reactive.

```rust
use bunny_ui::prelude::*;

#[derive(Clone, Copy)]
struct CounterModel {
    count: State<i32>,
}

impl CounterModel {
    fn increment(self) {
        self.count.add(1);
    }
}

#[derive(Clone, Copy)]
struct Counter {
    vm: CounterModel,
}

impl Component for Counter {
    fn body(self) -> impl View {
        vstack!(
            text(self.vm.count),
            button(text("Increment"), move || self.vm.increment()),
        )
    }
}
```

The model's commands can be tested without a window. Domain models and services
remain independent of Bunny UI. Prefer separate `State` properties when fields
should update independently; wrapping a whole model in one `State` gives its
readers a shared dependency.

Both a model supplied through a field and a body-local `view_model(Model::new)`
are supported. Initialization determines ownership: an app can create a model
outside rendering, or `view_model` can retain it once per mounted identity and
release its state on unmount. A field stores the supplied model; it does not
change its lifetime. The [MVVM guide](docs/mvvm.md) explains both forms, derived
properties and two-way editing. Run the complete examples without a window:

```bash
cargo run -p bunny-ui-core --example counter_mvvm
cargo run -p bunny-ui-core --example profile_mvvm
```

## Work that waits

A view can own asynchronous work. `.task` starts it on the view's first
appearance and ends it when the view leaves the tree.

```rust
row.task(async move || {
    let (lines, reader) = task::channel();
    std::thread::spawn(move || read_the_log(lines));
    while let Some(line) = reader.recv().await {
        log.update(|all| all.push(line));
    }
})
```

Tasks accept async closures that borrow their owned captures across `.await`.
The running task keeps those captures alive. Ordinary `move || async move { … }`
factories remain supported, with the same identity and cancellation behavior.

The framework reads no file and opens no socket. The application does
that on its own thread — or through its own browser callback — and
hands the results over a channel. The sender is the only part that
crosses a thread boundary, and it carries a signal, not a scene: the
shell answers with the frame it already knows how to draw.

Cancellation is a drop. A view that leaves the tree ends its task, the
reader dies, and the next `send` answers `Err` — the sign for the
worker to stop. `.task_id(id)` restarts the work when the id moves, so
a details panel that switches files cancels the read in flight.

## A box the application owns

Some content has no interface vocabulary: a code editor, a terminal
grid, a waveform. It gets a box of its own, painted with the same
commands every built-in view emits.

```rust
// the short door: a box that only draws
canvas(|ctx, p| p.fill_rounded(ctx.bounds(), ink, 6.0))

// the full one: it measures, paints, and answers the pointer,
// the keyboard and the input system
custom(SketchPad { strokes, caption })
```

The box paints in its own coordinates and cannot escape them — the
clip around it is the framework's. It hears how much of it the clip
lets through, so a long document costs one screen, and it inherits the
ink and the font of the scope above it.

Nothing forks: the desktop composites the box on the GPU, the web
canvas mode on the CPU, and the element mode turns it into a canvas
island. A box that asks for the keyboard takes it on a click; the
strokes reach it before the key bindings, text arrives as text
(typing, a paste, the commit of a composition), and on the desktop the
input system asks it directly where the caret is.

Use it for content that has no views. A rounded corner, a hover state
or a gradient belongs in the framework. Content that arrives with its
own renderer — a web page, a camera's video — takes the native host
instead (`docs/webview.md`, `docs/video.md`).

## Gradients

A two-stop ramp is a property of a view, declared in the box's own
proportions so it survives every resize.

```rust
panel.background_gradient(
    Gradient::radial(violet, violet.fade())
        .center(UnitPoint::TOP)
        .radius(0.0, 420.0),
)
bar.background_gradient(Gradient::linear(top_ink, bottom_ink))
```

The placement resolves it to pixels once; the rasterizers only
evaluate. On the desktop the ramp rides the same GPU instance a fill
does, and the CPU oracle agrees with it. On the element lowering it
becomes a CSS gradient — the geometry ours, the pixels the browser's.

`Color::fade()` is the end of a glow: interpolation is straight, so a
ramp that fades to a transparent black drags itself through grey.

## Clipping

`.clipped()` cuts the subtree to the box — and the cut follows the
`.corner_radius(…)` already on it. There is no radius to repeat and no
order to remember: the two fuse into one node.

```rust
vstack((toolbar(), panels()))
    .background_color(surface)
    .border(outline, 1.0)
    .corner_radius(6.0)
    .clipped()
```

A child that paints its own background dies at the curve; the border
paints over the cut child. On the pixel backends one coverage multiply
serves every primitive — fills, text, images, icons. In element mode
the browser does it natively (`overflow:hidden` beside the radius).
An inner clip with no radius of its own inherits the curve above it,
so a scroll region inside a rounded card keeps the card's corners.

## Icons

A glyph is a recipe, never pixels: verbs on a fixed 24 grid, plus the
paint that turns contours into ink. The house rasterizes the recipe at
the exact physical size a frame asks for — crisp at sixteen, crisp at
sixty-four.

```rust
icon(symbol::CHEVRON_RIGHT)                          // sizes with the font, takes the ink
icon(symbol::SEARCH).font(Font::Title)               // a symbol scales like a character
icon(symbol::FOLDER).resizable().frame(24.0, 24.0)   // the exact-box idiom
icon(acme::LOGO)                                     // an app's own glyph: the same type
```

One glyph, four renderings. The CPU rasterizes it once — a scanline
fill and a distance-field pen with round caps. The desktop GPU blits
those same bytes from the sprite atlas, so the two pipelines agree
byte for byte. The web canvas mode is the CPU rasterizer. The web
element mode emits a real `<svg>` that draws with `currentColor`, so a
hover re-tints with zero patches.

Sixteen symbols ship with the framework (`bunny_ui::symbol`). An app
converts its own icon files offline:

```bash
cargo run -p bunny-ui-core --features svg --example svg2icon -- icons/*.svg
```

The tool prints Rust const data to paste into the app — the default
build carries no parser. The same parser opens at runtime behind the
`svg` feature (`Symbol::from_svg`) for the app that accepts the cost.

## Languages and direction

The shell reports the languages the system prefers, as a list, before
the first frame. A view reads `Locale` from its environment and picks
the table that serves it best; the framework's own words — the mac's
app menu, the Edit items, a notification's button — speak sixteen
languages on their own. A right-to-left locale mirrors the scene:
stacks, padding, splits, scroll bars, popovers and the caret read from
the right, and a subtree may read the other way (`docs/i18n.md`).

## Status

Early development. The API is not stable.

## Build and test

```bash
cargo build
cargo test
cargo test --features svg   # the icon converter's parser rides the flag
```

The core lives in `crates/bunny_ui` and publishes as `bunny-ui-core`;
`crates/bunny_ui_facade` publishes as `bunny-ui`, the crate an application
adds. Each platform shell is its own crate, and the facade picks it by target.
`crates/bunny_cli` is the `bunny` command (`bunny-cli`).

## Demos

```bash
cargo run -p bunny-ui-core --example counter_headless
cargo run -p bunny-ui --example counter
cargo run -p bunny-ui-macos --example git_window
cargo run -p bunny-ui-macos --example sketch_window
cargo run -p bunny-ui-macos --example icon_window
cargo run -p countries-pure
crates/bunny_ui_ios/simulator/run-sim.sh touch_window_ios
crates/bunny_ui_android/android/run-emu.sh touch_window_android
cargo run -p bunny-ui-linux --example browser_window_linux
```

The first demo prints a small interface to the terminal. The second opens the same counter in a native window, on whichever desktop runs it. The third reads this repository's own `git log` from a worker thread and fills the window while it scrolls. The fourth is one box the application owns: it draws its own ink with the pointer, sizes its brush with the wheel, and types into a caption of its own — composition included. The fifth shows the sixteen house glyphs across fonts and inks. The sixth prints a full sample application. The seventh runs on the iOS Simulator: a list that pans and flings, a field that raises the keyboard, a canvas that draws under one finger and zooms under two (`docs/ios.md`). The eighth is the same screen on the Android emulator, presented by Vulkan (`docs/android.md`). The last one opens a web page on Linux — WPE WebKit held as one box of the scene, its pixels painted by the shell itself (`docs/linux.md`, `docs/webview.md`).

## Design rules

- The framework crates use only the Rust standard library.
- Views are plain values. State lives in typed arenas behind small handles.
- A render pass runs only the views that read changed state.
- The layout protocol is a proposal from the parent and a response from the child.

## License

Bunny UI is source-available under the [PolyForm Perimeter License 1.0.1](LICENSE.md) with two additional permissions. In short:

- **Build with it freely.** Applications, commercial ones included, can use Bunny UI without asking. Development tools such as IDEs count as applications.
- **Sell what you build on top.** Component libraries, themes, templates and plugins that depend on Bunny UI from crates.io may be sold, as long as they don't copy its source or present themselves as a replacement for it.
- **Fork it for free.** Forks and ports are welcome when nobody earns money from them: no sale, no paid support, hosting or premium features, no bundling into a paid product.
- **Don't sell it as a competitor.** Offering a fork, port or rewrite built from this code as a commercial alternative to Bunny UI is not allowed.

Extensions may say they are "for Bunny UI"; names that suggest an official product, such as "Bunny UI Pro", are reserved. [LICENSE.md](LICENSE.md) holds the binding terms; this summary does not replace them.
