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
custom(CodeSurface { document, state })
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

## Status

Early development. The API is not stable.

## Build and test

```bash
cargo build
cargo test
```

## Demos

```bash
cargo run -p bunny-ui --example counter_headless
cargo run -p bunny-ui-macos --example counter_window
cargo run -p bunny-ui-macos --example git_window
cargo run -p countries-pure
```

The first demo prints a small interface to the terminal. The second opens a native macOS window. The third reads this repository's own `git log` from a worker thread and fills the window while it scrolls. The fourth prints a full sample application.

## Design rules

- The framework crates use only the Rust standard library.
- Views are plain values. State lives in typed arenas behind small handles.
- A render pass runs only the views that read changed state.
- The layout protocol is a proposal from the parent and a response from the child.
