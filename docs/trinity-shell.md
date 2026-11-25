# The Trinity shell target

A raw block (D723) is real bunny-ui, compiled for a shell whose platform is
a process boundary. The engine — layout and state — runs in a zero-import
Wasm sandbox (Trinity's Pure tier). It hands its host the **semantic
scene**, never pixels, and takes **events** back. Focus, keyboard, IME
and accessibility stay with the shell. What crosses is the **shell
protocol**: small, versioned, additive, and the one thing Trinity keeps
loadable forever. Each block bundles the engine it was built with, so a
bunny-ui change breaks nothing already shipped; source moves forward with
`bunny migrate` (`crates/bunny_cli/README.md`).

The crate is `bunny-ui-shell` (`crates/bunny_ui_shell`):

| Piece | What it is |
| --- | --- |
| `protocol` | The messages in both directions as bytes, the version handshake, the capabilities. Std only, always built. |
| `guest` (feature `engine`) | A view and its runtime behind the protocol: `Guest::call(bytes) -> bytes`; and `Fixture`, the preview harness. |
| `host` (feature `engine`) | The test host: a scene raised and painted with the framework's own painter, and `native`, the render a scene must equal. |
| `block!` | The zero-import exports that make a `cdylib` a block. |
| `wit/shell.wit` | The component-model world the exports lower: `export call: func(input: list<u8>) -> list<u8>`. |

The first block, `apps/hello_block`, is measured in
`docs/measures/trinity-shell-hello-2026-11-25.md`.

## The boundary

```
 Trinity shell (host)                         block (zero-import Wasm)
 ────────────────────                         ────────────────────────
 focus, keys, IME, a11y,  ── ToGuest bytes ─▶  Guest::call
 the clock, the renderer                       Runtime: settle, layout
                          ◀─ ToHost bytes ──   display list → Scene
                                               a11y tree    → Semantics
```

A block exports three functions and imports none:

- `cabi_realloc(old, old_len, align, len) -> ptr` — where the host writes
  a call's input;
- `call(ptr, len) -> ret` — one turn; `ret` points at the answer's
  `(ptr, len)`;
- `cabi_post_call(ret)` — the host has read the answer.

That is the canonical ABI of `call: func(list<u8>) -> list<u8>`, lowered by
hand so the block needs no binding generator: a host embeds
`wit/shell.wit` as the module's component metadata and assembles the
component (`wit_component::embed_component_metadata`, then the encoder),
which is what Trinity's `wasm-sandbox` does for its own guests. One call
carries whole messages; a message cut at the end of a call is a `Fault`.

## The protocol, version 1

Every message is a little-endian `u32` length (kind byte and payload), a
`u8` kind, the payload. Numbers are little-endian; text is UTF-8 behind a
`u32` length; an `Option` is a `u8` tag then the value; a list a `u32`
count. Kinds `0x00..=0x3f` go guest → host, `0x40..=0x7f` host → guest,
`0x80..` are reserved.

### Host → guest

| Kind | Message | Payload | Capability |
| --- | --- | --- | --- |
| `0x40` | `Offer` | `BNYS`, oldest `u32`, newest `u32`, capabilities `u64`, viewport | — |
| `0x41` | `Resize` | viewport: width, height (points, `f64`), scale `f64`, scheme `u8` (0 light, 1 dark) | — |
| `0x42` | `Pointer` | phase `u8` (move, down, up, exit, cancel), x, y, button `u8`, clicks `u8`, modifiers `u8` | `POINTER` |
| `0x43` | `Wheel` | x, y, dx, dy, modifiers | `POINTER` |
| `0x44` | `Key` | key (W3C `KeyboardEvent.key` for a named key; the unmodified character, lowercased, for a printable one), typed, modifiers | `KEYS` |
| `0x45` | `Text` | committed text (typing, a paste, the IME's result) | `TEXT` |
| `0x46` | `Focus` | focused `bool` | `KEYS` |
| `0x47` | `Accessibility` | node `u64`, action (activate, focus, set text, increment, decrement, set value) | `ACCESSIBILITY` |
| `0x48` | `Tick` | `dt_ms` `f64` — the block has no clock | `TIME` |
| `0x49` | `Context` | the page's filters (each behind its length: field, value), then the open record (`Option`) | `CONTEXT` |
| `0x4a` | `Rows` | a view the block reads, whole: its id, its columns, then its rows (each behind its length: record id, cells as `Option<string>`) | `READS` |
| `0x4b` | `Answer` | the request's number `u64`, the outcome `u8` (landed, held for approval, declined by the person, refused + a reason) | `REQUESTS` |

Modifiers: shift 1, command 2 (⌘ on a Mac, Ctrl elsewhere), option 4,
control 8 (the Mac's ⌃).

### Guest → host

| Kind | Message | Payload |
| --- | --- | --- |
| `0x01` | `Accept` | `BNYS`, version `u32`, capabilities `u64` (within the offer), engine (`bunny-ui 0.5.0`) |
| `0x02` | `Refuse` | the block's oldest and newest version, a reason |
| `0x03` | `Scene` | a preview-wire `Frame` message, whole: the draws mirror the display list command for command, text as text, fonts by name |
| `0x04` | `Resource` | a preview-wire `Resource` message: an image's bytes, once, before the first scene that draws it |
| `0x05` | `Semantics` | the scene's `seq`, then the nodes, each behind its own length: id, role (by name), label, value, bounds, focused, enabled, checked, expanded, level, multiline |
| `0x06` | `Pace` | animating `bool` — whether the block wants ticks |
| `0x07` | `Fault` | input the block could not read; it read nothing after it |
| `0x08` | `Request` | the block's number for it `u64`, a declared action's id, the record it acts on (`Option`) — under `REQUESTS` |

A scene, semantics and pace are sent only when they changed: a turn that
changed nothing answers nothing.

### Capabilities

`SCENE` 1, `POINTER` 2, `KEYS` 4, `TEXT` 8, `ACCESSIBILITY` 16, `TIME` 32,
`CONTEXT` 64, `READS` 128, `REQUESTS` 256.
The host offers what it grants; the block answers the intersection with
the ones it knows, and acts on no event of an ungranted kind. There is no
capability to write, to open the network or to read a file: a block has no
import to ask with.

### The signature

A raw block is a block inside a declarative page, with a **signature**
(D723): what it receives (the page's filters and the record open on it),
which views it reads, and which declared actions it may request. The shell
grants the three bits the signature names and no others:

- `CONTEXT` — the shell sends `Context` when the page's filters or its open
  record change.
- `READS` — the shell sends `Rows` for each view the signature reads, as the
  person looking may see them, whenever they change; each replaces the last
  for its view.
- `REQUESTS` — the block may send `Request`; the shell answers each with
  `Answer`. A request is a question: the shell refuses an action the
  signature does not declare without touching the ledger, runs a declared
  one through its own gate (asking the person first when the action asks),
  and says how it went. Nothing is written by asking.

A view reaches its signature through `bunny_ui_shell::guest::Page`
(`block!(page => make)`): `record()`, `filters()`, `rows(view)`,
`request(action, record)` and `answer(request)`, each read recorded like a
`State`'s. Under a shell that grants no `REQUESTS`, `request` is answered
`Refused` inside the guest, so the view can say so rather than wait.

### Previews

A block is previewed natively, before it is built for the shell:
`bunny_ui_shell::guest::Fixture` is the page it stands on, filled by hand —
`filter`, `record`, `rows` (whole, per view, as `Rows` replaces them) and
`answer`, how every request is answered (`Refused` unless it names
another outcome). `Fixture::page()` is a `Page` with no shell behind it, so
the block's `make(page)` runs wherever a view does, and a `#[preview]` over
it is what Studio's dock and `bunny preview` show on every save:

```rust
#[bunny_ui::preview(name = "Capacity", example = "On a record page")]
fn on_a_record_page() -> impl View {
    capacity(Fixture::new().record("task_7").rows(work()).page())
}
```

The block draws on a fixture page exactly what it draws in the sandbox on
the same page (`tests/preview.rs` holds the pixels equal). A fixture page
answers a request at once and sends nothing: a preview shows the block,
never the ledger.

### How it grows

1. **A new message is a new kind.** A reader skips an unknown kind by its
   length.
2. **A message grows only at its end**, and a reader leaves the bytes after
   the fields it knows unread. A list's elements sit behind their own
   lengths, so they grow the same way.
3. **A value set only grows.** A role travels by name and an unknown one
   reads as a group; an unknown tag elsewhere is refused by name.
4. **A capability is a bit**; an unknown bit survives the bytes and drops
   out of the answer.
5. **The version bumps only for a change of meaning** the rules above
   cannot say. The host offers a range, the block answers the newest
   version both speak or refuses with its own range.

The scene's draws are the preview wire's, frozen under the same additive
rule (`crates/bunny_ui_preview/src/wire.rs`): one scene vocabulary for the
ecosystem.

## Production gotchas

- **Text is measured inside the sandbox with the engine's own face**
  (`PixelFont`), because a Pure guest has no font to ask. The scene names
  the font it laid out with; a shell that paints the text with another face
  paints other widths. Real text needs the shell's metrics to reach the
  layout — a capability the protocol can add as a kind — or a face bundled
  in the block; until then the honest renderer of a scene is the
  framework's own painter.
- **The block has no clock.** Time is the host's ticks. An app that reads
  the wall clock (`Instant::now`) traps on `wasm32-unknown-unknown`.
- **A platform view (a webview, a video) has no place in a block**: the
  host box stays reserved and empty.
- **Clipboard is the shell's.** Copy and paste reach a block only as
  committed text; the protocol has no clipboard yet.
- **One turn holds the guest.** A host gives a call its fuel or its
  deadline; a block that spins is ended by the sandbox, not by the
  protocol.
