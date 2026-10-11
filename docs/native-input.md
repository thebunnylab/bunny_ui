# Native input policies and external file drops

`TextField::editing_strategy` accepts an optional retained `Rc<dyn EditingStrategy>`.
The field still owns its binding, caret, selection, IME geometry and scrolling.
The policy interprets physical strokes and can decorate native edit commands;
it does not introduce a custom editor surface. Keep one policy per document,
rather than constructing it during every render.

`takes_text` decides whether printable keys enter the platform text/IME path.
`key` receives the layout's actual typed character before application bindings.
A consumed stroke updates both the binding and native caret and requests a frame,
including when only modal state changed. Declining preserves normal key routing.
`edit` defaults to native text editing; a policy can add grouped history or
replacement semantics. `Read` and `Copy` should remain observational.

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
