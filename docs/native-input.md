# Native input policies and external file drops

`TextField::editing_strategy` accepts an optional retained `Rc<dyn EditingStrategy>`.
The field still owns its binding, caret, selection, IME geometry and scrolling.
The policy interprets physical strokes and can decorate native edit commands;
it does not introduce a custom editor surface. Keep one policy per document,
rather than constructing it during every render.

`multiline_text_alignment` also aligns editable text in single-line fields and
each visual line of a multiline editor. Leading and trailing follow the scene's
layout direction; center uses the remaining padded width equally on each side.
Use trailing alignment with a monospaced font and fixed fractional digits for a
numeric column. Painting, pointer placement, selection, composition underlines
and native UTF-16 caret/range queries share that line origin. The placeholder
aligns independently of an empty editor's insertion point. Overflow still follows
the text's own reading direction and scrolls to reveal the caret. These rules do
not add visual BiDi navigation or OpenType tabular-number features.

`takes_text` decides whether printable keys enter the platform text/IME path.
`key` receives the layout's actual typed character before application bindings.
A consumed stroke updates both the binding and native caret and requests a frame,
including when only modal state changed. Declining preserves normal key routing.
`edit` defaults to native text editing; a policy can add grouped history or
replacement semantics. `Read` and `Copy` should remain observational.

## Independent native scenes

Each named `Runtime::scene` retains the root controls published by its last
render, including fields rendered directly without a `Component` wrapper.
Another scene may render or receive input without removing those controls.
Handlers and key contexts rebuild from the selected scene's retained entries
and root registrations; input-only activation never queues render effects.

A new render replaces only its own root registrations, including an empty
replacement when a control unmounts. Closing or resetting a named scene releases
its root input callbacks without removing a neighboring scene's controls.
`tests/scene_input.rs` covers typing, selection, independent history, rerender,
unmount and recreation through the public runtime API.

## Ordinary field history

A field without an editing strategy owns Undo/Redo through the standard keyboard
accelerators and native Edit menu. History restores both text and selection,
survives retained renders and unambiguous named moves, and stays local to its
mounted field. Unmounting or an external binding revision discards the history;
closure-backed bindings without revision identity detect changed text instead.
Custom strategies continue to own their editing commands and history.

Adjacent single-character typing and deletion coalesce. Selection, navigation,
paste, replacement and focus changes separate groups. One IME composition is one
group; Undo/Redo is unavailable while text is marked, and cancelling composition
preserves the previous redo. Read and Copy never create an edit. Secret fields
retain local history without exposing their contents through Copy or Cut.

History keeps changed bytes and caret states, sharing the current text's lending
cache. It retains at most 128 groups and a 1 MiB budget for replacements and patch
overhead. Old groups are evicted whole; an oversized edit leaves no partial Undo.
This contract covers the native/canvas runtime; browser flow DOM keeps its own
editing behavior. `tests/field_history.rs` exercises the public runtime boundary.

The runtime's `ExternalPaths` payload uses the same clipped `.on_drop` regions
as internal drag operations. `external_drag` previews without committing;
`external_drop` delivers only to the accepting target under the pointer;
`external_drag_exited` clears the preview. Empty payloads cannot commit.
The macOS shell registers native views for Finder file drops and routes them to
the owning window, including non-key windows. This change does not add native
file-drag adapters to the other platform shells.

Regression coverage lives in the field-strategy and external-file tests in
`crates/bunny_ui/src/lib.rs`. The application owns file classification and limits.

A multiline field can opt into `.submit_on_enter()`: plain Enter calls its
`on_submit` handler and Shift+Enter inserts a newline. Cmd+Enter remains an alias.
The default multiline editor still uses Enter for a newline. Editing strategies
receive the stroke before plain Enter submission, so Vim Normal mode can consume
it without sending.

Focused fields handle Option+Left/Right by word, Cmd+Left/Right by logical line,
and Cmd+Up/Down by document; Shift extends the selection for each motion. Word
movement uses the same Unicode character classes as double-click selection.
Logical line edges remain stable under soft wrapping and stop before newlines.
AppKit's corresponding text command selectors use the same edit commands.

Built-in field carets stay solid during accepted text, IME, navigation and
selection input. The first slow-clock beat after activity keeps them visible;
following quiet beats toggle at the existing half-period. This gives the caret
one complete quiet interval before it disappears, using the shell's existing
clock. Read/Copy queries preserve the phase. Custom elements keep ownership of
their blink behavior, and a window without caret or other pending work still
parks its clock.

Default Left/Right and Shift+Left/Right follow extended grapheme clusters, and
Backspace/Delete remove one complete cluster when no range is selected. A family
emoji, flag, skin-tone sequence or decomposed accent stays whole. An unselected
native caret inside a cluster removes that containing cluster. Explicit selection
and IME composition ranges retain their UTF-16/scalar contract; content is never
normalized. Word commands retain their existing character classes, and horizontal
movement remains logical rather than visual BiDi navigation.

The boundary rules are Unicode 17.0.0 / UAX #29 revision 47. The core has no new
dependency. All 766 official GraphemeBreakTest cases exercise the public edit API
forward, backward and from interior scalar positions. Generated property inputs
and BLAKE3 hashes are pinned in `scripts/unicode-grapheme-sources.json`; regeneration
uses `scripts/generate-grapheme-tables.py` with those downloaded files and the
build-time Python package `blake3==1.0.8`. The Unicode data license is retained
beside the test corpus. Ordinary ASCII edits inspect local boundaries; regional
indicator parity can require scanning the preceding indicator run.

## Click modifiers without pointer capture

`ViewExt::on_click_with_modifiers` receives `(click_count, Modifiers)` from the
initiating press. It is the same retained click target as `on_click_count`: a
cancelled press or lifted drag does not activate it, and `.on_drag` continues to
use the normal drag lane. Releasing a modifier before the pointer does not change
that press's meaning. Existing `on_click` and `on_click_count` callers keep their
signatures. Programmatic activation carries `Modifiers::NONE`.

Browser embeddings may forward their event modifiers through
`Runtime::dom_action_with_modifiers`; the original `dom_action` remains the
modifier-free compatibility entry point.
