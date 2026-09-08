# The iOS shell

`bunny-ui-ios` puts a bunny-ui scene on a phone or a tablet. The shell
is UIKit through hand-written FFI, with not a single dependency, and it
shares its Apple half — the text engine (CoreText), the image engine
(ImageIO), the credentials (the keychain), the Metal presenter and the
webview tenant — with the macOS shell through `bunny-ui-apple`. What is
UIKit's alone lives here: the application delegate, the view whose
backing layer IS the drawable, the touches, the responder the keyboard
types into, the safe area and the traits.

## Run it

```bash
crates/bunny_ui_ios/simulator/run-sim.sh counter_window_ios
crates/bunny_ui_ios/simulator/run-sim.sh touch_window_ios
SIM_DEVICE="iPad Pro 11-inch (M5)" crates/bunny_ui_ios/simulator/run-sim.sh countries_window_ios
```

The lane builds the example for `aarch64-apple-ios-sim`, assembles an
UNSIGNED `.app` (the simulator installs those), terminates the app it
may already be running — installing over a running app relaunches the
old binary — installs, and launches with the console attached. No Xcode
project is involved. `SIMCTL_CHILD_BUNNY_IOS_TRACE=1` prints every event
the shell delivers; `SIMCTL_CHILD_BUNNY_PRESENT_TRACE=1` writes the
present tape.

A real device needs signing, which the simulator does not. That lane is
an Xcode project with a stub `main` that the last build phase overwrites
with cargo's binary, so that Xcode signs what is actually there; it is
documented here and not gated by this repository.

## What a finger means

The core decides. Every touch reaches `Runtime::touch_began`,
`touch_moved`, `touch_ended` and `touch_cancelled` with the platform's
own tap count, and `bunny_ui::touch` turns it into the pointer's
vocabulary: a tap is a press and a release; a pan over something that
scrolls is the wheel, anchored where the finger landed; a lift at speed
flings, and the fling dies at the clamp; a press held still for half a
second over a menu opens the menu; a second finger makes the pair a
zoom the app's box hears as `ElementEvent::Magnify`. A box that wants
the drag on a touch surface says so with `CustomElement::takes_drag`.
Nothing hovers on a touch surface: the pressed paint is the interactive
paint, and a tooltip never arms.

## The safe area and the keyboard

The root lays out INSIDE the safe area — `Runtime::set_safe_area`
mirrors `safeAreaInsets` — and the keyboard's height joins the bottom
inset when it rises, so the scene stands above the keys. A view wearing
`.ignores_safe_area()` reclaims the bands its frame touches; the root
reclaims the whole window. The keyboard follows the focus: a field that
takes it makes the view the first responder, and the view types through
`UIKeyInput` with the traits answered by hand (no autocorrection, no
sentence capitals, unless the app asks). A hardware keyboard is asked of
the keymap first; a letter arrives once, through the keyboard's road.

## What the shell answers

| | iOS |
| -- | -- |
| `MANY_WINDOWS` | **false** — the screen is the window |
| the road | UIKit's main run loop; `App::run` hands the process over and never returns |
| present | Metal, on the view's own `CAMetalLayer`; no CPU road |
| frames | `CADisplayLink`, born paused, running only while something moves — never in the background |
| dark, size class | the traits, mirrored into the theme (while the app has not chosen one) and `SizeClass` |
| hosts | a webview rides the shared tenant, under the same sandwich the desktops keep |
| synthetic input into a page | not claimed — the phone has no event constructor a page trusts |
| a chrome, a cursor, a live resize | none: the phone has no window frame and no pointer |
| an IME mirror | not yet: a composition arrives committed |
| notifications | not yet: `bunny_ui::app::notify` refuses by name |
| a url handed over | `application:openURL:options:` → `AppEvent::Reopened` |
| background, foreground | `AppEvent::WillSleep` / `DidWake`, and the loop clocks rest |

## Honest edges

The simulator's Metal is the host's, paravirtualized; the atlas and the
offscreen target use shared-storage textures, which the simulator
accepts today — if a device refuses one, the ground that mints textures
is the one place to answer with a private texture and a blit. The tap
count comes from `UITouch.tapCount`, so a double tap is the platform's
word. A rubber band past a scroll region's edge is not drawn. The
countries example's root wears `.ignores_safe_area()`, as the app it
ports does, so its list starts under the clock — that is the app's
choice, faithfully kept.
