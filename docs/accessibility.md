# Accessibility semantics and native bridges

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

## Linux adapter

The Linux shell registers an AT-SPI application on the system accessibility bus,
using the libdbus library already linked by the shell. Its descriptor participates
in both the X11 and Wayland event loops, including write readiness for queued
replies. Requests run on the UI thread between native dispatches. No accessibility
timer, Rust dependency or separate form model is introduced.

The first client query enables retained capture. Main windows and overlay surfaces
have distinct roots; leaves retain their object paths while their `NodeId` is
exposed. Accessible and Application properties describe the current objects.
Component exposes bounds, hit testing and field focus; Action invokes buttons;
Text reads Unicode scalar ranges; EditableText replaces a field's whole contents.
Unsupported operations return a D-Bus error. Text selection, caret offsets,
character geometry and partial edits are not implemented yet.

Passwords expose the password role and an empty readable text value and count.
No password text enters a bus reply or change event. Removed nodes and closed
windows reject requests. Name, text, focus, membership and visible-data events
follow snapshot installation. Embedded NUL scalars in labels are represented by
U+FFFD because D-Bus strings cannot carry NUL; the remaining text is preserved.

X11 screen coordinates come from the actual window or panel's server origin.
Both backends support window-relative coordinates. Wayland screen-coordinate
queries return NotSupported: its ordinary surface protocol supplies no global
window position. WPE content is currently a painted texture and does not yet
expose an embedded AT-SPI plug. A missing bus is reported at initialization;
a disconnected bus is reported and removed from the poll, without terminating
the app. Automatic reconnection is not yet implemented.

`bash scripts/check-linux-accessibility.sh` uses a separate libatspi client
(Python GI) against real Bunny windows, an isolated session bus, Xvfb and headless
Weston. It runs both backends in normal and paranoid modes. This is a native
protocol witness, not a human Orca workflow or a complete desktop qualification.

## Remaining work

The Windows UIA adapter is delivered in a separate change. The core projection
is a flat sequence of exposed leaves, not a complete document model.
It does not yet describe custom controls, checkboxes, sliders, read-only or
disabled states, rich-text ranges, selection APIs, validation messages or
virtualized offscreen navigation. It does not implement keyboard navigation
between buttons. Its snapshots are not evidence of screen-reader usability.
Native AX/UIA/AT-SPI dumps and separate human workflows remain necessary.
