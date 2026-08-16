# bunny_ui

A declarative UI framework for Rust, inspired by SwiftUI.

Write views as value types. The framework finds the views that read changed state and runs only those.

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
cargo run -p countries-pure
```

The first demo prints a small interface to the terminal. The second opens a native macOS window. The third prints a full sample application.

## Design rules

- The framework crates use only the Rust standard library.
- Views are plain values. State lives in typed arenas behind small handles.
- A render pass runs only the views that read changed state.
- The layout protocol is a proposal from the parent and a response from the child.
