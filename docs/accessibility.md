# Accessibility semantics and native bridges

The core can project text, buttons, checkboxes, sliders and editable fields from the retained scene
into `bunny_ui::accessibility::Tree`. This is the data boundary for native
adapters. It does not by itself expose an application to VoiceOver, UIA or AT-SPI.

A shell opts in with `runtime.set_accessibility_enabled(true)` before its next
frame. After `display_frame`, `accessibility_tree()` returns the placed nodes in
reading order. Collection schedules no timer. Activating or deactivating it
rebuilds retained view metadata through the existing environment invalidation
path; application state remains in the identity arena. With collection disabled,
ordinary text and buttons allocate no semantic metadata.
For named scenes, capture and environment invalidation rebuild only the owning
scene. Other windows keep their retained callbacks until their own next frame.

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
`Activate` and `Focus`; checkboxes support `Activate` (toggle) and `Focus`;
editable fields support `Focus` and `SetText`. Sliders support `Focus`,
`Increment`, `Decrement` and `SetValue(f64)`. Their `Node.range` snapshot has
finite bounds, bounded current value and nominal small/large keyboard changes.
A `SliderRange` validates construction; range properties are read-only. Numeric
sets outside the inclusive domain or nonfinite sets return `InvalidValue`;
in-domain sets snap using the pointer policy. Adjustments reuse the live binding
and do not steal keyboard focus. Use `accessibility_label` to name the control. Disabled controls retain their
semantic nodes with `enabled = false` and reject actions. Checkboxes expose
`checked: Some(bool)` separately from text values. Unsupported and
unavailable requests return distinct errors. Successful actions request a
frame. Snapshots report current keyboard focus without requesting a frame.

Successful `Activate` follows the pointer's keyboard-ownership rule after the
callback: a focusable control receives focus, including its containing editor's
standard edit commands. Explicit `.leaves_keyboard()` chrome preserves the
current owner. This also gives forms an invoking control to restore when they
close; it does not install a global edit-command fallback or change a focused
text field's own history. Rejected actions leave focus unchanged.

## macOS adapter

The AppKit shell exposes these controls through `NSAccessibilityElement` proxies
owned by each native view. Its first accessibility query enables semantic capture;
applications do not need to opt in manually. Overlay panels have their own roots,
including when the first query targets an already-open modal.

Popup choices remain children of their native overlay surface. The parent
window's focused-element query resolves the focused proxy across its own scene,
so a non-key popover can report its active choice through the parent that keeps
keyboard focus. Other windows retain independent focus. A hosted native editor
keeps its own focus when no Bunny overlay owns it. Inspecting only the parent's
children still omits overlay choices; clients must include child surfaces.

The adapter keeps each native object while its `NodeId` remains exposed, converts
layout bounds to screen coordinates, reports keyboard focus and forwards supported
press, focus and value edits through the shell's existing event queue. Secure
fields expose `AXSecureTextField` with no value. Removed controls and closed windows
reject actions even if a client still retains their former native objects.
Checkboxes expose `AXCheckBox` and an `NSNumber` value, with value-change
notifications. Sliders expose `AXSlider`, numeric value/minimum/maximum,
increment/decrement selectors and numeric value setting. Range changes emit
value notifications. These controls expose focus and enabled state.

After installing the new snapshot, the adapter sends AppKit notifications for
changed names, values, focus, membership and geometry. An unchanged scene sends no
notification and queries schedule no idle frames. Native views embedded inside
Bunny views keep their AppKit accessibility children, hit testing and focus,
including the owning control of AppKit's shared field editor. The adapter adds no crate
dependency and stays inside the existing macOS FFI boundary.

`cargo test -p bunny-ui-macos --test accessibility_native --locked` runs a
main-thread AppKit probe against real windows and selectors. It checks live edits,
identity, screen bounds, modals, password redaction, retired objects and
main-thread exit with an accessible window still open. Popover checks cover
native button roles, labels, actions, focus through the parent, hit testing,
repeated opening, retired choices and independent-window focus. This
in-process probe does not require permission to control other applications. It
is distinct from an external AX client dump or a human VoiceOver workflow.
`scripts/check-macos-accessibility.swift <accessibility_native-executable>` runs
an external AX range client. It exits 77 if the current process lacks the user's
Accessibility permission, without prompting or changing that permission.

## Windows adapter

The Win32 shell answers `WM_GETOBJECT` with a UI Automation fragment root for
each scene and overlay window. The first UIA request enables semantic collection,
including a request directed at an already-open modal. Text, buttons and fields
expose their native control types, names, physical screen bounds and stable
runtime IDs. Buttons expose Invoke; checkboxes expose Toggle and ToggleState;
single-line fields expose Value. Sliders expose RangeValue with numeric value,
minimum/maximum, small/large change and read-only state when disabled. Numeric
changes emit RangeValue property notifications. These controls expose keyboard
focus and enabled state. Toggle and enabled changes emit property notifications.
Multiline editors expose focus but require the still-unimplemented Text pattern
for text access; they do not claim Value pattern support.
Password fields set IsPassword and refuse value reads.

COM providers hold immutable owned snapshots behind synchronization; they never
borrow Runtime, views or application bindings. They support callbacks from an
MTA as well as a normal STA host. Action methods validate and queue requests to
the owning window's event loop; the handler rechecks the exposed identity before
using the real callback or editing path. A successful call means that the request
was accepted; clients observe the resulting frame or change event. A request
whose target disappears before dispatch is discarded. Removed elements and
closed windows disconnect their providers and refuse subsequent requests.

Snapshot publication follows every presentation path. Name, value, focus,
geometry and tree membership changes emit UIA notifications when clients are
listening; there is no accessibility polling timer. Native HWND children keep
their Windows providers, navigation, hit testing and focus. The adapter uses
system UIAutomationCore and OleAuto APIs inside the existing Win32 FFI boundary
and adds no Rust dependency.

`cargo test -p bunny-ui-windows --test accessibility_native --locked` runs a real
IUIAutomation client on a separate MTA thread while the app pumps its window
messages. Its fixture checks model edits, invocation, focus, dynamic names,
runtime identity, password redaction, moved bounds, native child interoperability,
modal exclusion and first-query activation, events, and retired/closed objects.
Subprocesses exercise an already-MTA host and first access to an open modal.
This protocol test is distinct from a human NVDA workflow.

## Linux adapter

The Linux shell registers an AT-SPI application on the system accessibility bus,
using the libdbus library already linked by the shell. Its descriptor participates
in both the X11 and Wayland event loops, including write readiness for queued
replies. Requests run on the UI thread between native dispatches. No accessibility
timer, Rust dependency or separate form model is introduced.

The first client query enables retained capture. Main windows and overlay surfaces
have distinct roots; leaves retain their object paths while their `NodeId` is
exposed. Accessible and Application properties describe the current objects.
Component exposes bounds, hit testing and control focus; Action invokes buttons
and toggles checkboxes. Sliders expose Value properties with a writable numeric
CurrentValue, plus increment/decrement actions. Enabled sliders are EDITABLE;
value changes emit `accessible-value` property events. Checkboxes expose CHECKABLE/CHECKED states and changes;
disabled controls omit ENABLED/SENSITIVE and reject actions.
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

The libatspi 2.52 client used by the Linux witness aborts in its Value setter
when a rejected write returns a D-Bus error. Valid queries and writes still run
through libatspi; invalid, disabled and retired writes are checked with direct
D-Bus calls so their exact error and unchanged value remain observable. The
adapter preserves rejection rather than returning a false success. This client
compatibility limit remains relevant to human assistive-technology acceptance.

## Remaining work

The core projection is a flat sequence of exposed leaves, not a complete document model.
Custom elements can opt into bounded range semantics; arbitrary custom control
roles, general read-only states, rich-text ranges, selection APIs, validation
messages and virtualized offscreen navigation remain unsupported. Its snapshots are not evidence of screen-reader usability.
Native AX/UIA/AT-SPI dumps and separate human workflows remain necessary.
