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
