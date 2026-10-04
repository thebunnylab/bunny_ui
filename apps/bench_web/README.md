# bench_web

A 200-row stateful table on the element lowering, with a driver that
measures it in a real browser.

Every row owns a toggle. One chip toggles all rows. One chip filters
the table to ten rows and back. The driver dispatches real pointer
events and times the full path: input, state, patches, elements.

## Run

```sh
cargo build --profile web -p bench-web --target wasm32-unknown-unknown
cp ../../target/wasm32-unknown-unknown/web/bench_web.wasm web/
python3 -m http.server 8872 --directory web
```

Open `http://localhost:8872/bench.html`. Then, in the console:

```js
await __bench.all()
```

The driver runs each operation in interleaved rounds with a cooldown
between rounds, so no operation heats the machine for the next one.
It prints one table: p50 / p95 / max per operation, sustained
toggles per second, and the boot cost (instantiate, first frame).

Add `?stats` to the URL to make the glue accumulate its apply-side
wall time in `window.__bunnyApply`.

## What the numbers mean

- **toggle 1 row** — one state flip. The cost must follow the CHANGE,
  not the table.
- **toggle all 200** — a bulk update. It must fit a frame budget.
- **filter 200 → 10 / 10 → 200** — removals and creations.
- **sustained toggles/sec** — complete input-to-element frames in one
  second.

The headless twin of this fixture is
`crates/bunny_ui/examples/bench_dom.rs` — the same operations with
per-stage timing inside the engine.

## The keyed page, three ways

`krausest.html` is the keyed benchmark page on the element lowering:
six chips, a table of rows, every row its own component. The same
page comes two more ways, from the same crate built with the `gpu`
feature:

- `hybrid.html` — the chips are real elements, the table is one
  canvas island our layout positions and the engine paints.
- `gpu.html` — the whole page through the pixel pipeline: the WebGL
  tier where the page has it, the CPU surface where it does not.

```sh
cargo build --profile web -p bench-web --no-default-features --target wasm32-unknown-unknown
cp ../../target/wasm32-unknown-unknown/web/bench_web.wasm web/
cargo build --profile web -p bench-web --features gpu --target wasm32-unknown-unknown
cp ../../target/wasm32-unknown-unknown/web/bench_web.wasm web/bench_web_px.wasm
cp ../../crates/bunny_ui_web/glue/{glue.js,glue_gl.js,surface.js} web/
```

The `gpu` feature also turns on the probe: `window.__bunnyHits()`
answers the hit rectangles of the last layout (and the islands'
frames), which is how a runner finds a row on a canvas.

`tools/three_modes.mjs` times the nine keyed operations on any set
of pages the official way: a fresh page per sample, the page's own
warm-ups, the CPU throttle per operation, a trace from the click to
the last paint. It takes `--page name=url[:kind]` with kind `dom`,
`hybrid` or `gpu`, a `--chrome` binary, and `--count`. Given a
`--harness` checkout it computes the durations with that harness's
own timeline code; without one it uses a port of the same rule.
