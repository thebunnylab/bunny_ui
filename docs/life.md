# The app's life

*Status: a second document window stands on macOS and Windows (the
`App`, one `Runtime` per window, the pump routing each message to the
window it arrived at) and is refused by name on Linux, which holds
one; `cargo run -p bunny-ui-macos --example two_windows -- --drive` is
the proof. The doors are standing in `bunny_ui::app` — the events
(sleep, wake, a notification activated, a second launch with its
arguments), the notifier, and the single instance — and each shell
answers them: macOS by the application delegate, the workspace's
notifications and UserNotifications; Windows by the power broadcast
and WinRT toasts over hand-written COM (type-checked, not run);
Linux by libdbus on one thread, `org.freedesktop.Notifications` and
`org.freedesktop.login1` (type-checked, not run). The single instance
is one mechanism on every platform, in core, with nothing but std.
`cargo run -p bunny-ui-macos --example life_window -- --drive` is the
proof of the spool and of the refusal by name; wrapped in an `.app`
it is the proof of the notification.*

A mail client is not only a window. It is one process — a second
launch, a link from another app, both must land in the one already
running. It wants to know the machine slept and woke, because the
mailbox went stale while it did. And it says "a letter arrived" in
the desktop's own notification, with a button on it, and wants to
hear which button was pressed. None of that is a view, and none of it
should be written three times by every app.

## The doors

```rust
use bunny_ui::app::{self, AppEvent, Instance, Notification};

let (sender, life) = task::channel::<AppEvent>();
app::subscribe(sender);                                    // first
if app::instance("trinity-mail", &arguments) == Instance::Secondary {
    return;                                                // the running one heard
}
// …in a task, on the app's own thread:
while let Some(event) = life.recv().await {
    match event {
        AppEvent::DidWake => sync_all(),
        AppEvent::WillSleep => save(),
        AppEvent::NotificationActivated { id, action } => open(id, action),
        AppEvent::Reopened { arguments } => route(arguments),
    }
}
// …when a letter arrives:
app::notify(&Notification::new(thread_id, from, subject)
    .action("reply", "Reply")
    .action("archive", "Archive"))?;
```

The events ride the channel discipline `.task` already uses: the app
owns a channel, hands the sender over, and reads on its own thread. A
send wakes the frame from whichever thread the platform speaks on — the
main thread on macOS, a thread-pool thread for a toast on Windows, the
bus thread on Linux — and the app never learns which.

**A notification** is posted by `id`, and posting again with the same
id REPLACES it: a thread that grew by one more letter is one
notification, updated. Its buttons come back as the `action`; the
notification itself, clicked, comes back as `None`. `notify` answers
`Err` with the platform's refusal by name — no shell running, an app
the desktop does not know, a person who said no — and never a quiet
nothing.

**The single instance** is a lock in the person's own runtime
directory (`XDG_RUNTIME_DIR`, `LOCALAPPDATA`, or the temp dir), held
for the life of the primary and released with its last breath. A
later launch writes its arguments into a spool beside the lock and
answers `Secondary`; the primary's own thread reads the spool four
times a second and reports `Reopened` with the arguments, whole. This
is the app's half of a deep link on every platform — the OS starts the
app with the url as an argument, and the argument crosses over.
Registering the url scheme is the platform's own configuration (the
bundle's `Info.plist`, the registry, a `.desktop` file), and stays
the app's to write.

## More than one window

A mail client detaches its composer; a workbench opens a second
project. That is not a popover and not a sheet: it is another
top-level window, with its own scene, its own keymap, its own focus —
and the app is still one process with one event road.

```rust
let app = App::new();                       // the shell's own App
let runtime = app.runtime().text_engine(…); // a scene of its own
let main = app.open(WindowSpec::titled("Trinity Mail").size(1080.0, 720.0),
                    Rc::new(runtime), mail);
// …later, from a "detach" action:
if MANY_WINDOWS {
    app.open(WindowSpec::titled("New message").size(720.0, 560.0),
             Rc::new(app.runtime().text_engine(…)), composer);
}
app.close(main);                            // the app stays up unless it was the last
app.run();                                  // returns when the last window closes
```

Every window has a `Runtime` of its own, so two windows showing the
SAME view are two trees and not one — the identity paths under them
are identical, and a click must not land in whichever window rendered
last. The shell routes by the window a message ARRIVED at: a click on
a popover belongs to the scene that opened it, a key to the window
holding the keyboard, and the shared beats — the frame tick, the caret
blink, a worker's wake — reach every window. What a window holds goes
with it when it closes: its swapchain, its backing, its panels, its
place in the frame beat, which moves house if it lived there. The app
quits when the LAST window closes, which is the single-window contract
said again.

`MANY_WINDOWS` is the shell's own answer, and an app that must run on
the three platforms asks it before it detaches:

| | macOS | Windows | Linux |
| -- | -- | -- | -- |
| `MANY_WINDOWS` | true | true | **false** |
| the road | AppKit's run loop, one `NSWindow` each | one pump, one `HWND` each, a swapchain each | one surface, one road |

On Linux both desktops — X11 and Wayland — are answered here by a
single surface with its own event road, and a second document window
is not built: `App::open` refuses the second by name rather than
half-serving it, and an app keeps its second view INSIDE the window
(a pane, a sheet). The refusal is loud on purpose: a silent
half-window would be worse.

## What each platform answers

| | macOS | Windows | Linux |
| -- | -- | -- | -- |
| sleep, wake | `NSWorkspace` will-sleep / did-wake | `WM_POWERBROADCAST` suspend / automatic resume | logind `PrepareForSleep` |
| notification | UserNotifications, a category per button set | a WinRT toast under the process's AppUserModelID | `org.freedesktop.Notifications` `Notify` |
| activation | the center's delegate, while running or launched by the click | the toast's `Activated` event, while running | `ActionInvoked` on the session bus |
| second launch | the spool; a BUNDLED app reopens through the delegate instead | the spool | the spool |
| a url handed over | `application:openURLs:` → `Reopened` | an argument → the spool | an argument → the spool |

Three honest edges. macOS shows notifications for a BUNDLE, never a
bare binary — the system's own center raises for a process with no
bundle identifier, so the framework refuses by name first; a dev
binary is shown by wrapping it in an `.app` with an `Info.plist`.
Windows shows a toast under the process's AppUserModelID
(`bunnylab.<executable>`, set at boot); a Start Menu shortcut carrying
that id gives it the app's name and icon, and a click on a toast of an
app that already EXITED needs a COM server the registry knows — both
the app's own setup. Linux announces sleep only where logind runs,
which is where a laptop runs.
