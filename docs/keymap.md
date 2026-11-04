# Keymap

A key reaches an app through one gate, `Runtime::offer_key`, which every shell
calls with the stroke its platform read. The gate keeps one order everywhere:

1. With nothing in the air, a stroke the platform types goes straight to a field
   or box that takes text. Typing is never stolen by a binding.
2. Whoever holds the keyboard hears it next (`Runtime::key_stroke`): an editor owns
   its arrows, its Enter and its Tab. A copy's text reaches the clipboard here.
3. A field of many lines owns the bare break and the bare vertical arrows.
4. The keymap (`Runtime::chord`) answers. An action runs if a handler answers it.
   A sequence, a dead end it unwound, or a disabled key is spent. A binding nobody
   answers does not consume, so the stroke walks on as if unbound.

Mid-sequence the keyboard belongs to the keymap, and steps 1 and 3 stand aside.
`KeyPlatform` tells the gate what only the shell knows. Alt and a letter type
nothing on Linux. A touch keyboard's return key arrives as text.

## Contexts and frames

`.key_context(name)` turns a context on while its view is mounted.
`.key_context_focused(name)` turns it on only while the keyboard is inside its
view. The `_with(ContextFrame)` forms declare facts beside the names:

```rust,ignore
editor.key_context_focused_with(
    ContextFrame::new("Editor").fact("mode", mode).fact("extension", ext),
)
```

A frame is part of the view's value. The next render declares the facts as they
are then.

`Runtime::context_stack` is the live set, innermost first. It holds the
declarations on the focus path, deepest first, then the mounted ones the keyboard
is not inside, deeper first and in tree order. Each level names its parent, so a
predicate can read the tree. Among contexts that bind the same key, the one
nearest the keyboard wins. The alert's and the popover's reserved keys come
before all of them.

## The resolver

`Runtime::set_key_resolver` installs a Strategy: `KeyResolver::resolve(strokes,
contexts)` answers one of four ways:

- `Action(id)`: the strokes are bound to this action.
- `Pending(fallback)`: a longer binding starts with these strokes.
- `Disabled`: a `null`. The stroke is consumed and nothing runs.
- `None`: nothing is bound.

The `bind`/`bind_in`/`bind_sequence` tables stay beneath an installed resolver as
the floor, and are asked only when it answers `None`.

## Sequences

A sequence matches each stroke by the key it is, or by the character it typed. A
binding spelled `g ~` completes on a layout where `~` is shift and a key, and the
key's own spelling wins a tie.

When the next stroke leads nowhere, or the slow clock ends the wait (two ticks),
the held strokes are unwound, not dropped:

- If some prefix is bound on its own, the longest one fires. A key held for both
  `j` and `j k` still means `j` alone.
- If none is bound, the first held stroke goes to the text door, as if no binding
  had seen it.

What came after is then offered to the whole gate again. Escape lets a sequence go
and is spent. A press of the pointer drops it.

## A lone modifier

A modifier pressed with nothing else held, and let go with no key, text or click
in between, becomes a stroke of its own: `Key::Shift`, `Key::Command`,
`Key::Option` or `Key::Control`. The runtime makes it from the shell's modifier
reports (`Runtime::modifiers_changed`), and the keymap is the only one that reads
it: no box hears it as a key.

`ModifierChange::tap_taken` tells a shell with its own use for the tap, such as
Linux's menu-bar Alt, that a binding took it.
