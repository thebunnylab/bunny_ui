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
