# Desktop confirmations

`App::confirm(owner, spec)` asks a Yes/No question belonging to one live desktop
window. It returns a local `Confirmation` future. Await it on the UI executor;
only `ConfirmationResponse::Yes` authorizes the affirmative action.

```rust,ignore
use bunny_ui::window::{ConfirmationResponse, ConfirmationSpec};

let spec = ConfirmationSpec::new("Delete project?", "This removes the selected project.")?;
let confirmation = app.confirm(window, spec)?;
match confirmation.await? {
    ConfirmationResponse::Yes => model.delete_selected(),
    ConfirmationResponse::No | ConfirmationResponse::Dismissed => {}
}
```

Application question/detail text remains application-owned and should come from
its locale catalog. Neither string may contain NUL; the question must contain
visible text. The framework supplies Yes/No translations in sixteen languages.

| `ConfirmationKind` | Presentation and input scope | Button language |
| --- | --- | --- |
| `AppKitSheet` | An `NSAlert` sheet attached to the owner; other windows keep running | Owning scene at presentation |
| `Win32MessageBox` | A system message box disables its owner; its native loop starts after framework dispatch returns | Windows system language |
| `FrameworkAlert` | The Linux shell presents the framework's themed alert on a native X11/Wayland window; retained input isolation blocks the underlying scene | Owning scene |

The Linux presentation uses neither GTK nor a subprocess. Its framework input
scope does not claim Wayland `xdg_dialog` OS modality. It uses the existing
native alert surface and its existing rendering/connection failure behavior.
In particular, fatal display connection failure is not converted into “No.”

No is the Return default. AppKit also routes keypad Enter and Escape to No using
an event monitor restricted to this process and this exact sheet. Win32 routes
Escape to No with a message filter restricted to the current UI thread and the
owned message box. Linux Escape or a window dismissal yields `Dismissed`;
Return invokes No, and Tab visits the retained controls. A focused affirmative
button remains available through its normal activation and accessibility paths.

The future is not `Send` and cannot be cloned. Dropping it, or calling `cancel`,
requests dismissal. A presented operation finishes only after native teardown;
its owner remains reserved until then. Earlier cancellation wins over a later
native affirmative response. Hiding the owner reports `HiddenWindow`; closing
it reports `OwnerClosed`. A retired or foreign ID is `StaleWindow`. Another
outstanding confirmation or a conflicting modal presentation is `AlreadyOpen`.
Errors remain errors, never negative answers.

Windows queues requests for different owners while one message box is open.
Queued cancellation can complete while that first box remains open. AppKit
supports simultaneous sheets for different owners. Native callback tokens are
never reused, and a late response cannot answer a replacement window or request.

The adapters introduce no additional dependency. Native callbacks remain in the
existing platform FFI boundaries, release their registry borrows before native
operations or completion, and retire temporary hooks/monitors before delivering
answers. Linux explicitly acknowledges a native alert's retirement after the
event handler returns, including hide/close paths that skip drawing.

Verification includes public lifetime tests, retained alert keyboard/focus and
accessibility tests, own-process AppKit/Win32 protocol fixtures, and native
X11/Wayland surface geometry observations. These fixtures do not certify physical
keyboard input, human screen-reader usability, installed distribution, mixed-DPI
behavior, or a complete ecosystem qualification cell.
