# The hello block in the sandbox

T2-APP-60 / D723 / FR-APP-23. The plan's risk note asked for the engine's
size and its cost in the sandbox to be measured before anything is
promised. This is that measurement, for the first block: `apps/hello_block`
— a heading, a line that counts, a button and a switch, built with
`bunny_ui_shell::block!`.

Branch `task/T2-APP-60-trinity-shell`, based on bunny_ui main `46c640d9`.
rustc 1.100.0, Apple M5 Max, macOS 27.0. The block was built with

```sh
cargo build -p hello-block --target wasm32-unknown-unknown --profile web
```

(`opt-level = "z"`, fat LTO, one codegen unit, stripped, `panic = "abort"`):
`hello_block.wasm`, sha256 `7a1ef513…69fcdb3837`.

## Size

| | bytes |
| --- | ---: |
| module | 1 276 487 |
| gzip -9 | 430 882 |
| brotli 11 | 330 607 |
| imports | **0** |
| exports | `memory`, `cabi_realloc`, `call`, `cabi_post_call` |
| linear memory after two turns | 1 245 184 (19 pages) |

Where the bytes are (the same build with its name section,
`breakdown.mjs`): code is 1 189 553 bytes, data 78 364. By the crate that
defines each function — generic instances counted where they are defined,
so `core`, `alloc` and `hashbrown` hold much of the engine's own
monomorphized code:

| crate | code bytes |
| --- | ---: |
| `bunny_ui_core` (layout, modifiers, runtime) | 481 641 |
| `core` | 201 614 |
| `hashbrown` | 148 804 |
| `alloc` | 115 454 |
| `std` | 85 098 |
| `bunny_ui_preview` (the scene's draws, convert and wire) | 79 016 |
| `bunny_ui_motor` | 21 495 |
| `bunny_ui_shell` (the protocol and the guest) | 21 230 |
| `compiler_builtins`, `dlmalloc`, the rest | ~26 000 |

The largest single functions are the layout's `place_node` (57 KB),
`modifier::apply` (34 KB), the preview wire's `decode` (23 KB — a guest
never decodes a scene; see follow-ups) and `measure` (16 KB). The view
itself is 391 bytes: what a block costs is the engine, paid once per block.

## Start and turns

### Trinity's Pure recipe (wasmtime 36.0.12)

`wasmtime-harness.rs`, built against `foundation/wasm-sandbox` as it
stands on Trinity `origin/dev` `e005ac62`: the core module given
`crates/bunny_ui_shell/wit/shell.wit` as its component metadata, assembled
by the sandbox's own `build::componentize` (so the hand-lowered ABI *is*
the `call: func(list<u8>) -> list<u8>` component), compiled by
`fueled_component` (bulk instructions metered), checked import-free by
`imports`, instantiated with an empty linker in a `fueled_store` (10¹⁰
fuel, never near spent; 64 MiB memory cap). Six fresh loads:

| step | ms |
| --- | ---: |
| componentize (once) | 6.6 |
| compile (Cranelift, per load) | 108 – 156 |
| instantiate | 0.033 – 0.045 |
| first turn: offer → accept, first scene, semantics, pace | 0.60 – 0.90 |
| a press (down and up) → new scene, semantics | 0.080 – 0.113 |

| fuel (exact, every run) | units |
| --- | ---: |
| first turn | 393 352 |
| a press | 303 436 |

The compiled artifact serializes to 6.7 MB. **Compilation is the cost**:
two orders of magnitude over everything else. Since the workspace forbids
`deserialize`, a host should compile a block once per process and keep the
`Component`; instantiating it again is tens of microseconds.

### V8 (node 26.10.0)

`scripts/measure-hello-block.mjs`, the same module with an empty import
object; `v8.json` is its output.

| step | cold (first run), ms | warm (median of 30), ms |
| --- | ---: | ---: |
| compile | 3.27 | 0.24 |
| instantiate | 0.36 | 0.10 |
| first turn | 10.70 | 0.28 |
| a press | 1.14 | 0.10 |

V8 compiles lazily, so its cold first turn carries the compilation
wasmtime paid up front; the warm medians reuse V8's cache of the module.

### What crosses

The first turn's answer is 958 bytes (`Accept`, `Scene`, `Semantics`,
`Pace`); a press's is 913 (`Scene`, `Semantics`). The answers of the V8
run and the wasmtime run are byte-identical.

## Identical to the native render

- `apps/hello_block/tests/identical.rs` (always run): the block behind the
  protocol, in a test host, paints the same RGBA as the same view rendered
  natively — light at 2×, dark at 1× — before and after a press, and
  negotiates version 1 from an offer of 1..=7.
- `apps/hello_block/tests/sandboxed.rs` (ignored; needs the wasm build): the
  answers the **wasm** block gave, from V8 and from wasmtime, painted by the
  test host, equal the native render before and after the press. Both
  passed on the build above.

The native render is the CPU raster every shell keeps as its oracle,
measuring text with `PixelFont`. A Trinity shell that paints the scene with
its own text engine will not match it until the shell's metrics reach the
block's layout; that is the first thing this measurement does not promise.

## What this does not promise

- Real text: the block lays text out with the house's headless face.
- Images, platform views, the clipboard, IME composition inside the block.
- That 1.3 MB per block is acceptable at scale. Every block bundles its
  engine (D723), so a page of N blocks holds N engines; the size is
  dominated by layout and the standard collections, not by the protocol.
