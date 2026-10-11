# Accessibility semantics and macOS bridge

The core can project text, buttons and editable fields from the retained scene
into `bunny_ui::accessibility::Tree`. This is the data boundary for native
adapters. It does not by itself expose an application to VoiceOver, UIA or AT-SPI.

A shell opts in with `runtime.set_accessibility_enabled(true)` before its next
frame. After `display_frame`, `accessibility_tree()` returns the placed nodes in
reading order. Collection schedules no timer. Activating or deactivating it
rebuilds retained view metadata through the existing environment invalidation
path; application state remains in the identity arena. With collection disabled,
ordinary text and buttons allocate no semantic metadata.

Text supplies its displayed words. A button combines the names in its label
without exposing those words as duplicate children. A field defaults to its
placeholder as its name and exports its current value separately. A secret field
has role `PasswordField` and never exports its value. Names can be overridden
with a fixed or reactive source:

```rust,ignore
text_field("Description", expense.description.binding())
    .accessibility_label(expense.localized_description_label)
```

`accessibility_label` names exactly one semantic element. Label each control
inside a multi-control container separately. `accessibility_hidden()` removes
decorative content from this projection; it changes neither paint nor ordinary
input behavior. These modifiers preserve layout and do not create DOM elements.

Bounds use layout points with a top-left origin. Window and scroll clipping
exclude unreachable geometry. Modal content replaces the covered controls in
the projection. Overlays use their own placed content bounds. `Node.surface`
identifies the owning overlay; `None` means the main window. A partially
clipped button keeps its complete name even when some label text is outside
the visible area.

`NodeId` is opaque and scoped to an exposed lifetime. Keyed reorders preserve it;
an element that leaves the projection and later returns receives a new handle.
Disabling collection retires all exposed handles. An adapter must release its
removed native elements rather than retain them by position. Handles from a
different runtime cannot address the same control.

`accessibility_action(id, action)` validates the handle and supported action,
then uses the real control's callback or editing path. Buttons support
`Activate`; editable fields support `Focus` and `SetText`. Unsupported and
unavailable requests return distinct errors. Successful actions request a
frame. Snapshots report current keyboard focus without requesting a frame.

## macOS adapter

The AppKit shell exposes these controls through `NSAccessibilityElement` proxies
owned by each native view. Its first accessibility query enables semantic capture;
applications do not need to opt in manually. Overlay panels have their own roots,
including when the first query targets an already-open modal.

The adapter keeps each native object while its `NodeId` remains exposed, converts
layout bounds to screen coordinates, reports keyboard focus and forwards supported
press, focus and value edits through the shell's existing event queue. Secure
fields expose `AXSecureTextField` with no value. Removed controls and closed windows
reject actions even if a client still retains their former native objects.

After installing the new snapshot, the adapter sends AppKit notifications for
changed names, values, focus, membership and geometry. An unchanged scene sends no
notification and queries schedule no idle frames. Native views embedded inside
Bunny views keep their AppKit accessibility children, hit testing and focus,
including the owning control of AppKit's shared field editor. The adapter adds no crate
dependency and stays inside the existing macOS FFI boundary.

`cargo test -p bunny-ui-macos --test accessibility_native --locked` runs a
main-thread AppKit probe against real windows and selectors. It checks live edits,
identity, screen bounds, modals, password redaction, retired objects and
main-thread exit with an accessible window still open. This
in-process probe does not require permission to control other applications. It
is distinct from an external AX client dump or a human VoiceOver workflow.

## Remaining work

Windows UIA and Linux AT-SPI bridges are not included yet. The core projection
is a flat sequence of exposed leaves, not a complete document model.
It does not yet describe custom controls, checkboxes, sliders, read-only or
disabled states, rich-text ranges, selection APIs, validation messages or
virtualized offscreen navigation. It does not implement keyboard navigation
between buttons. Its snapshots are not evidence of screen-reader usability.
Native AX/UIA/AT-SPI dumps and separate human workflows remain necessary.
