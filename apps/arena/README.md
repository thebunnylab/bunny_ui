# The arena

Four scenes, scripted, in a real window: what every UI framework is measured on, on the same
machine, with the same content and the same hand. These are bunny_ui's own; the method and the
numbers are in `docs/arena.md`.

| bin | scene | scripts |
|---|---|---|
| `arena_table` | ten thousand rows of six columns, 24 pt each, in a virtual list | `rest`, `wheel` (240 steps a second, down then up), `soak` |
| `arena_editor` | a text editor over 400 or 30 000 lines of source, the keyboard in it | `type` (a character and a backspace, ten a second), `wheel` |
| `arena_stream` | a chat that grows by one message every 33 ms | `stream` |
| `arena_canvas` | a looping decoration at rest, on its own layer | `rest` (the loop runs by itself) |

Every app prints `FIRST_FRAME <unix ms>` after its first frame and exits when the script ends.
`ARENA_FIXTURES` names the fixtures directory (`rows-10k.tsv`, `lines-400.txt`, `lines-30k.txt`);
`BUNNY_PRESENT_TRACE` and `BUNNY_FRAME_STATS=1` put the frames on the tape.

```sh
ARENA_FIXTURES=… cargo run --release -p arena --bin arena_table -- --script wheel --rows 10000 --secs 3
```
