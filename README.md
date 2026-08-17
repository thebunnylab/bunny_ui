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
