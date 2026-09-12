# The Android shell

`bunny-ui-android` puts a bunny-ui scene on a phone or a tablet. The
shell is `android.app.NativeActivity` through hand-written FFI and JNI,
with not a single dependency, and it shares its Vulkan tier — the
swapchain presenter, the atlas, the parity oracle — with the Linux
shell through `bunny-ui-vulkan`. What is Android's alone lives here:
the activity's callbacks, the main looper the clocks and the input
queue ride, the window a frame presents into, the text engine
(`android.graphics`), the image engine (`AImageDecoder`), the insets,
the soft keyboard and the secret store.

## Run it

```bash
crates/bunny_ui_android/android/run-emu.sh counter_window_android
crates/bunny_ui_android/android/run-emu.sh touch_window_android
EMU_SHOT=1 crates/bunny_ui_android/android/run-emu.sh countries_window_android
```

The lane builds the example as a shared object for
`aarch64-linux-android` with the NDK's own linker, wraps it in an APK
over a pure `NativeActivity` (Gradle, zero Java), boots the emulator if
none is up, stops the app it may already be running — installing over a
running app relaunches the old binary — installs, launches and follows
logcat. `EMU_SHOT=1` takes a screenshot into `target/android/` instead.

An app has no environment on Android, so the switches ride as system
properties: `adb shell setprop debug.bunny.trace 1` prints every event
the shell delivers, `adb shell setprop debug.bunny.present cpu` asks for
the CPU floor instead of Vulkan. Logcat is the activity's only voice:
the shell's lines ride under the tag `bunny_ui`, a panic is a FATAL
line, and stdout and stderr flow to the same place — a refused tier is
a line, never a blank screen.

An app of its own is a cdylib that exports the entry the system looks
for, through the crate's macro:

```rust
fn main() {
    bunny_ui_android::run_window("Counter", Size { width: 280.0, height: 180.0 }, counter);
}
#[cfg(target_os = "android")]
bunny_ui_android::activity!(main);
```

`run_window` returns at once — the activity was running before the app
was built — and the window is raised when the system hands one over.

## One thread

Everything runs on the UI thread: the system calls the activity there,
the main looper lives there, and the runtime is single-threaded by
design. So there is no glue thread, no command pipe and no
acknowledgement for the main thread to wait on — a window that is going
away is let go inside its own callback, and a frame the system asks for
is drawn before the callback returns. The clocks are file descriptors
on the main looper: a timerfd for the caret's blink and the slow beat,
a pipe for a worker's knock, and the choreographer for the display's
pace, posted only while something moves and never while paused.

## What a finger means

The core decides. Every pointer of a motion event reaches
`Runtime::touch_began`, `touch_moved`, `touch_ended` and
`touch_cancelled` in points, and `bunny_ui::touch` turns it into the
pointer's vocabulary: a tap is a press and a release; a pan over
something that scrolls is the wheel, anchored where the finger landed;
a lift at speed flings, and the fling dies at the clamp; a press held
still for half a second over a menu opens the menu; a second finger
makes the pair a zoom the app's box hears as `ElementEvent::Magnify`.
The platform carries no tap count, so the shell counts: a press within
300 ms and 25 points of the last lift continues the series. Nothing
hovers on a touch surface.

## The safe area and the keyboard

The window is laid out edge to edge, and the decor view's own insets —
the status bar, the navigation bar, a cutout — become the core's safe
area (`Runtime::set_safe_area`); the keyboard's inset joins the bottom
when it rises, so the scene stands above the keys. A view wearing
`.ignores_safe_area()` reclaims the bands its frame touches; the root
reclaims the whole window.

The keyboard follows the focus: a field that takes it asks the input
method for the keyboard, in the name of the view the platform serves;
a scene with no such field asks for it back. The input method keeps
the back key while the keyboard is up, so a keyboard the person sent
away is noticed by the window's insets, and the field lets go — or the
next frame would raise it again.

Keys arrive as key codes, not text. A table of the latin layout says
what each types, with shift and caps lock; a return is the break a
field of many lines takes; the back key is the escape, closing what is
open before it leaves the app. A composed character, a script the
table does not know, an emoji need an `InputConnection`, which is
Java, and do not arrive — the honest ceiling of a `NativeActivity`.

## The secret store

Android has no keychain that a native app can open by that name. It has
the two halves one is made of, and `credentials` puts them together.

`AndroidKeyStore` mints an AES key under an alias. The key stays in the
secure element where the phone has one, and the app receives a handle
that can encrypt and decrypt but cannot read the key. The ciphertext
goes to `SharedPreferences`, a file under the app's own uid. A secret is
named by a pair, the service and the account, like on every other shell.

```rust
bunny_ui_android::credentials::write("api.example.com", "default", token);
let found = bunny_ui_android::credentials::read("api.example.com", "default");
bunny_ui_android::credentials::delete("api.example.com", "default");
```

This is what `EncryptedSharedPreferences` is under its own cover, without
the AndroidX dependency. The example shows the one thing a store must
prove, which is that a secret survives the app:

```sh
crates/bunny_ui_android/android/run-emu.sh credentials_window_android
crates/bunny_ui_android/android/run-emu.sh credentials_window_android
```

**CAUTION: a refusal is never a plaintext fallback.** If the keystore
refuses a key, or a cipher refuses to run, `write` answers `false` and
stores nothing. The app must then tell the person that the secret is
lost at the end of the run.

The key belongs to the app's data. Erasing the app's storage, or an
uninstall, erases the alias. Every secret is then unreadable, `read`
answers nothing, and the app asks again.

## What the shell answers

| | Android |
| -- | -- |
| `MANY_WINDOWS` | **false** — the screen is the window |
| the road | the activity's UI thread and its looper; `App::run` hands the window to the activity and returns |
| present | the shared Vulkan tier over the `ANativeWindow`, or the CPU floor through `ANativeWindow_lock` — chosen once per mount, `debug.bunny.present=cpu` asks for the floor |
| frames | the choreographer, posted only while something moves — never while paused |
| the window | comes and goes with the background and a rotation; the surface renews on the same device, the atlas stays |
| scale | the density over 160, rounded to a whole number: a 420 dpi phone is 3×, 360 points wide |
| dark, size class | the configuration (`uiMode`, the width in dp), mirrored into the theme (while the app has not chosen one) and `SizeClass`; a change is an event, not a new activity |
| text, images | `android.graphics` and `AImageDecoder`; a face the app ships goes through `register_font`, which reads the family out of the file |
| the clipboard, reduce motion | the system's, through JNI |
| the modality | every gesture sets it: `Runtime::last_input_was_touch`, and `touch` on a custom box's event and paint |
| the secret store | `AndroidKeyStore` for the key, `SharedPreferences` for the ciphertext — see above |
| a directory to write | `data_dir()` — the activity's private files directory, which nothing in the environment names |
| hosts | none: a web view is Java |
| a chrome, a cursor, a live resize | none: the phone has no window frame and no pointer |
| an IME road | keys only — see above |
| notifications | not yet: `bunny_ui::app::notify` refuses by name |
| a url handed over | not yet: a `NativeActivity` hears no new intent |
| background, foreground | `AppEvent::WillSleep` / `DidWake` on pause and resume, and the loop clocks rest |

## Honest edges

The emulator's Vulkan is SwiftShader, which offers FIFO only: a present
waits for the vertical blank on the UI thread, which the shell accepts
as it accepts the phone's own display link. The scale is a whole
number by the tiers' contract, so a 2.625× phone lays out at 3× and a
little smaller than its dp would say. A pinch is proven headless in
the core and by hand on the emulator (`adb shell input` has no second
finger). The countries example's root wears `.ignores_safe_area()`, as
the app it ports does, so its list starts under the clock — that is the
app's choice, faithfully kept. The Linux shell is type-checked here and
run on a Linux box; the parity suite of the shared tier wants a device.
