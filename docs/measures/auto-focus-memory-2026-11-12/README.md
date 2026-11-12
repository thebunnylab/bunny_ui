# A first appearance that forgot it had happened

`.auto_focus()` takes a free keyboard on a field's first appearance. A blur by the reader is final. For a field with no name at a position in a stack, it was not: after a blur, the next pass of any kind gave the keyboard back to the field.

The runtime kept one set of strings for three kinds of memory: `path` (a first appearance), `path#beat` (a beat a field or a box answered) and `path#alert` (an alert's opening). To forget the memory of an input that left, it split each string at the last `#`. A path also ends in `#` and digits when the input stands at a position: `Form/#0` was read as beat `0` of an input at `Form/`. No such input exists, so the memory was dropped on every pass.

The three kinds are now kept apart, each under the input's own path, and nothing is parsed. A pass that asks about a beat already answered no longer builds a string to ask it.

| File | What it holds |
|---|---|
| `failing-before.log` | `tests/auto_focus_memory.rs` on the old memory: after a blur, pass 1 hands the keyboard back to `Form/#0` |

The same file has two controls that passed before and still pass: a field that leaves and comes back is a first appearance again, and a beat at a position is answered once.
