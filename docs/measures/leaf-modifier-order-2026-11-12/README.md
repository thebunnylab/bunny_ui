# Leaf modifiers through every wrapper of the same view

A find bar wrote `text_field(..).hug_height().select_on_beat(beat)` and the field never took the keyboard. The same chain with the two modifiers swapped worked. `.hug_height()` wraps the field in a node that the field rewrite did not know, so the ask was dropped without a word.

The cause was wider than one wrapper. Four rewrites (scroll, field, pixel, text) each kept their own list of the wrappers they crossed, and each list was different. The retained failing-before run shows what was lost on the base revision `9e4e27b1`:

- a field ask written after `.hug_height()`, `.ignores_safe_area()`, `.tooltip(..)`, `.on_measure(..)`, `.hover_group()`, `.window_drag_region()`, `.layout(Exact)`, `.context_menu(..)` or `.on_drop(..)`;
- a text modifier written after `.padding()` or a frame (`.truncation_mode`, `.highlight`);
- every field and text modifier while source sites are recorded (preview);
- `.scroll_target(..)` on a region made with `.hugging()`, in any order.

`LayoutNode::seen_through_mut` now names, in one exhaustive match, the wrappers that are still the same view. `rewrite_leaf` walks them and changes the leaf where it stands. A new node kind does not compile until it is classified there. The four rewrites are gone.

A node that carries an action or element hints is still a target of its own and still stops a leaf modifier, as before. A field ask that reaches no field is reported once for its source line in debug builds (`diagnostics::take_unreached_fields`).

| File | What it holds |
|---|---|
| `failing-before.log` | `tests/leaf_modifier_order.rs` on the base sources: 16 of 18 tests fail |
| `failing-before-exit.txt` | the exit code of that run |

After the change the same file passes 18 of 18. The core suite passes with default features, with all features, and with `BUNNY_PARANOID=all`.
