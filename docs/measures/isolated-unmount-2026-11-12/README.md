# A component that leaves inside a kept row

A find bar is a component with no state of its own. It opens over an editor and borrows the keyboard. The report said: the bar closes, its parent body re-runs alone, and the dead field keeps the keyboard.

That exact shape was already correct on the base revision `9e4e27b1` (the sweep was rewritten on 2026-10-02). `a_bar_closed_while_its_parent_re_runs_alone_gives_the_keys_back` passes there, and now pins it.

A narrower shape of the same class was still wrong. A list re-runs and keeps a clean row by its key. A body inside that row is dirty and runs after the walk. That body no longer makes a component it made before. The sweep asked "does ANY skipped boundary above cover this entry?", found the kept row, and sheltered the entry. The component stayed in the retention with its editor, its handlers and its key contexts. The keyboard was released to nobody.

The sweep now asks the NEAREST boundary the pass met. A skipped boundary shelters what is under it. A boundary that ran answers for its own children, also when a kept row stands above it.

`BUNNY_PARANOID=sweep` is new. It asks the same rule of every retained entry on its own (no ordered walk, no stack, no range) and compares the answer with what the sweep took out. It is part of `BUNNY_PARANOID=all`, which CI runs.

| File | What it holds |
|---|---|
| `failing-before-retention.log` | the reconciler unit test on the base sweep: the component stays retained |
| `failing-before-keyboard.log` | `tests/isolated_unmount.rs` on the base sweep: the keyboard goes to nobody |

The identity sweep already took out a component that owns state. Only a component with no state of its own was held, which is why the unit test uses one.
