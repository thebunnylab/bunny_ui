# bunny_ui

A declarative UI framework for Rust, inspired by SwiftUI.

Write views as value types. The framework finds the views that read changed state and runs only those.

## Quick look

```rust
#[derive(Clone, Copy)]
struct Counter {
    count: State<i32>,
}

impl Component for Counter {
    fn body(self, _ctx: &Context) -> impl View {
        vstack!(
            text!("Count: {}", self.count),
            button(text("Tap"), move || self.count.add(1)),
        )
    }
}
```

The display of `count` records a read. A tap changes the state, and the framework runs only this view again.

## Work that waits

A view can own asynchronous work. `.task` starts it on the view's first
appearance and ends it when the view leaves the tree.

```rust
row.task(move || async move {
    let (lines, reader) = task::channel();
    std::thread::spawn(move || read_the_log(lines));
    while let Some(line) = reader.recv().await {
        log.update(|all| all.push(line));
    }
})
```

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
or a gradient belongs in the framework.

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
cargo run -p bunny-ui --features svg --example svg2icon -- icons/*.svg
```

The tool prints Rust const data to paste into the app — the default
build carries no parser. The same parser opens at runtime behind the
`svg` feature (`Symbol::from_svg`) for the app that accepts the cost.

## Status

Early development. The API is not stable.

## Build and test

```bash
cargo build
cargo test
cargo test --features svg   # the icon converter's parser rides the flag
```

## Demos

```bash
cargo run -p bunny-ui --example counter_headless
cargo run -p bunny-ui-macos --example counter_window
cargo run -p bunny-ui-macos --example git_window
cargo run -p bunny-ui-macos --example sketch_window
cargo run -p bunny-ui-macos --example icon_window
cargo run -p countries-pure
```

The first demo prints a small interface to the terminal. The second opens a native macOS window. The third reads this repository's own `git log` from a worker thread and fills the window while it scrolls. The fourth is one box the application owns: it draws its own ink with the pointer, sizes its brush with the wheel, and types into a caption of its own — composition included. The fifth shows the sixteen house glyphs across fonts and inks. The sixth prints a full sample application.

## Design rules

- The framework crates use only the Rust standard library.
- Views are plain values. State lives in typed arenas behind small handles.
- A render pass runs only the views that read changed state.
- The layout protocol is a proposal from the parent and a response from the child.
