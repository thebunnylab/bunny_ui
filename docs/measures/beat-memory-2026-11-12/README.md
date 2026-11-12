# A ring answered once, also by a field that returns

A find bar lives in a tab and wears a beat: `.select_on_beat(n)` with a number the app raises on the shortcut. The reader summons the bar, clicks back into the document, changes tab and returns. The bar took the keyboard again on the way back.

That is the documented rule of a beat given as a number. The field remembers the numbers it answered while it is on screen, and a field that leaves forgets them. A popup whose field is made new on each opening relies on it. The rule is wrong for a field that leaves and returns under the same intent, and the runtime has no place to keep such a memory: a table keyed by the field's name would hold one record for each name for the life of the window, with nothing to say when a name will never return.

The state that carries the beat is the one owner that outlives the field and has a defined end. `Beat` is that state, as a type. It holds how many times it rang and the newest ring a field answered. `text_field(..).focus_on(beat)` and `.select_on(beat)` make the field ask the beat before it takes the keyboard.

The failing-before run is the scenario with the type and the two methods in place and the answered mark not yet read, which is the old rule.

| File | What it holds |
|---|---|
| `failing-before.log` | `tests/beat_memory.rs` without the mark: the query takes the keyboard again when its tab returns, and two fields that wear one beat both answer a ring |

With the mark the file passes 5 of 5. One of the five makes the bar leave and return 1000 times and counts live memory blocks: nothing is kept per return.
