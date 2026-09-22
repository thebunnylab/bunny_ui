# The Linux shell

`bunny-ui-linux` puts a bunny-ui scene in a window on a Linux desktop.
The shell opens one of two doors — Wayland (`xdg-shell`, through
`libwayland-client`) or X11 (through `xcb`) — with hand-written FFI
and not a single dependency, and it shares its Vulkan tier with the
Android shell through `bunny-ui-vulkan`. What is Linux's alone lives
here: the two doors, the EGL/GL presenter, the text engine
(fontconfig, FreeType, HarfBuzz), the image engine (the codecs of the
house), the session bus (notifications, sleep, the portal's theme and
file pickers), the secret store (`libsecret`), and the webview (WPE
WebKit, loaded at run time).

A window presents by the first tier that comes up, chosen once when
the window opens: Vulkan, then EGL/GL, then the CPU raster. Every tier
draws the same display list and the parity tests pin the pixels.

## Run it

```bash
cargo run -p bunny-ui-linux --example counter_window_linux
BUNNY_BACKEND=x11 BUNNY_PRESENT=cpu cargo run -p bunny-ui-linux --example counter_window_linux
cargo run -p bunny-ui-linux --example counter_window_linux -- --drive
```

The switches ride the environment. `BUNNY_BACKEND=wayland|x11` picks
the door (without it, `WAYLAND_DISPLAY` wins over `DISPLAY`).
`BUNNY_PRESENT=cpu|gl` asks for a lower tier than Vulkan.
`BUNNY_FRAME_STATS=1` prints a stage line per frame and a present line
per present, with the tier that presented. A refused tier is one line
on stderr, never a blank window.

An example that answers `--drive` drives itself: a sheet queues the
events a person would make, reads the state and counts the frames the
shell presented, prints one line per check and exits 0 when they all
hold. `crates/bunny_ui_linux/src/drive.rs` is the hand; the sheet is
the example's own.

## Prove it without a Linux desktop

```bash
crates/bunny_ui_linux/container/run.sh                 # everything
crates/bunny_ui_linux/container/run.sh test            # the crate tests
crates/bunny_ui_linux/container/run.sh drive counter_window_linux
crates/bunny_ui_linux/container/run.sh drive browser_window_linux --editor
crates/bunny_ui_linux/container/run.sh shell           # both displays up
```

The `--drive` sheets today: `counter_window_linux` (a click, the
manners with `--fixed`, the pacer with `--sleeper`),
`two_windows_linux`, `browser_window_linux` (the page, `--editor` for
the letter), `scroll_window_linux`, `compose_window_linux` and
`life_window_linux`.

The script builds a Debian image with the libraries the shell links,
raises a headless Weston (the Wayland door) and an Xvfb (the X11 door)
under a session bus, points Mesa at its software renderers (llvmpipe
for GL, lavapipe for Vulkan), and runs the tests of the core, the
Linux crate and the Vulkan tier, then every `--drive` example across
the matrix: two doors × three tiers. The table at the end names the
runs that did not hold. The repository is mounted read-write; the
cargo registry and the target directory live in named Docker volumes,
so the host's `target/` is never touched and the second run is fast.
`SCALE=2` gives the headless output an integer scale.

What the container cannot prove — a real GPU, a real compositor's
decorations, a trackpad or a touchscreen, a fractional scale — is
listed at the end.

## Two doors

The Wayland door speaks `xdg-shell` with hand-written opcode tables
(a test diffs them against the installed protocol XML), `wl_shm` for
the CPU tier, `wl_egl_window` for GL and a `VkSurfaceKHR` for Vulkan,
`zwp_text_input_v3` for composition, the compositor's own cursor
theme, and the portal over the session bus for the theme, the file
pickers and the secret store. The X11 door speaks core X through
`xcb`: MIT-SHM for the CPU tier, an EGL window surface for GL, an
`xcb` Vulkan surface, `xkbcommon-x11` for the keyboard, the core
cursor font, and `Xft.dpi` for the scale. The X11 scale is an integer
by law — `Xft.dpi: 144` is 2×, `Xft.dpi: 120` stays 1× — and there is
no composition road on that door (XIM is a fossil; dead keys still
compose client-side).

## The frame and its manners

`WindowSpec` says who draws the top edge (`Chrome::Native` or
`Chrome::Scene`) and how the window behaves under the hand:
`.fixed()` for one size, `.no_minimize()` for a window that cannot be
put away. The platform refuses the gesture, never the scene: on
Wayland the minimum and the maximum size are the one size, so the
compositor refuses the resize grab; on X11 `WM_NORMAL_HINTS` says the
same, and the Motif hints drop the resize, maximize and minimize
verbs from the window manager's frame.

A native window on Wayland asks the compositor for its frame through
`xdg-decoration`. KDE and the wlroots desktops answer with a
server-side frame. GNOME never does — Mutter speaks no
`xdg-decoration` and draws no frame for a Wayland window — and the
headless Weston of the container is the same. Where no server-side
frame answers, the shell stands a 32-point bar of its own on the
scene: the title, the minimize, maximize and close buttons, the whole
bar a drag region, and the crown answering its verbs, the resize
bands at the border and the rounded corners, exactly as a scene-chrome
window has them. One line on stderr says so. The X11 door keeps the
window manager's frame.

The pointer's shape follows the box under it: an I-beam over text, a
cross over a cell, a hand over anything that answers a press, the
resize arrows at a seam or a band. On Wayland the shapes come from
the cursor theme; on X11 from the core cursor font.

## Pacing

Every window has a pacer (`bunny_ui::pacing`), the shape the mac and
the web shells keep: a burst of pointer, wheel or wake events folds
into one frame a beat, and a wake with no news draws nothing. The
beat is the compositor's frame callback on Wayland while frames
present; a window that wants frames and presented nothing — a tick
that moved no pixel, a task asleep on a timer — keeps its beat on a
deadline of its own, at the display's interval or at the slower one
the animator asks for, and the next present retires it. The X11 door
has no callbacks and every window ticks on its own deadline. A
deadline keeps its phase across the events that re-sync the driver
(a blink every 500 ms would otherwise push an 800 ms sleep out
forever) and moves only when a shorter pace pulls it in; a slow beat
advances the engine's clock by the step it promised, as the mac's
slow timer does. No bare commit is ever involved. `BUNNY_PACING=off` draws every ask at once,
for a measurement that wants the raw count. The container proves it
with `counter_window_linux --drive --sleeper`: a task awake twenty
times a second that writes once a second presents its three writes
and nothing for the sixty wakes.

## Scale

The Wayland door climbs a ladder for the window's scale: the
compositor's exact preference in 120ths where it speaks
`wp_fractional_scale_v1` (KDE, the wlroots desktops, GNOME), the
whole number a v6 surface is told directly, and the outputs the
surface touches, which every compositor has. The raster draws on the
whole lattice — the ceiling of the exact scale, the law the Windows
shell keeps too — and the compositor scales the buffer to the glass,
as it does with a whole scale already; the `F` line prints both
numbers. A crisp 1:1 raster at a fractional scale needs the core's
raster to take a fraction, which it does not yet, so a 125 % desktop
draws the 2× lattice and lets the compositor bring it down. The X11
door reads `Xft.dpi` and rounds to a whole number (150 % is 2×, 125 %
stays 1×). The container's Weston speaks only whole scales; the
fractional tier is proven by its tables and its arithmetic.

## The hand on the glass

A wheel turns in detents: a compositor at `wl_pointer` v8 says them
in 120ths of a step (and sends no `axis_discrete` at all), an older
one in whole steps, and a finger on a pad says neither and stays
continuous — the shell reads all three and keeps the ×16 line every
platform shares. A pinch on a pad arrives through
`zwp_pointer_gestures_v1` where the compositor speaks it, as a ratio
per step for the box under the pointer. A touchscreen arrives through
`wl_touch`, asked for when the seat says it has one: each finger on
the main window reaches the runtime's own recognizer, which makes the
taps, the pans and the flings out of it, the same as on the phones.
The X11 door reads none of this — a touchscreen or a pad's pinch on
X11 needs XInput2, a road this shell does not walk. The container
proves the tables and the arithmetic; the hand itself is proven on a
laptop.

## More than one window

`App::open` raises as many windows as the app asks for — `MANY_WINDOWS`
is true on this shell. Each door keeps a list of windows: on Wayland
each is named by its `wl_surface`, on X11 by its xid, and every event
carries the surface it arrived at, so the pump routes it to that
window's runtime and nowhere else. The keyboard's keys go to the
window the compositor (or the server) said entered; a panel's events
go to the window it hangs from; the pointer's shape is set on the
window under it. Every window has its own GPU presenter — its own EGL
context or Vulkan device, so a second window costs a second atlas —
its own backing on the CPU tier, and its own frame callback on
Wayland; one deadline keeps a beat alive for every window that wants
one and presented nothing. A window opened from inside a handler (a
button that opens a window) takes only its own setup events off the
queue — its first configure, its frame's answer, its scale — without
a dispatch, so nothing re-enters the handler. The last window out
ends the road. `two_windows_linux --drive` is the proof: a third window
raised from a running app, the counts kept apart, the FIRST window
closed with the app still standing, then the survivors closed and the
road ending by itself.

## Fonts

The text engine is fontconfig, FreeType and HarfBuzz. A family the
app names is matched by fontconfig, weights on its own scale
(regular 80 … black 210), with a charset fallback for the glyphs the
matched face lacks. A face the app SHIPS registers with
`FreeTypeEngine::register_font(include_bytes!(…))`: the family name,
the weight and the slant are read out of the file's own tables
(`bunny_ui::font_file`), and a spec naming that family lands on the
registered face — the slant matched first, then the nearest weight —
before fontconfig is asked. The container proves it with the
machine's DejaVu Sans: registered, its bold file outranks the regular
the machine would have answered.

## Images

The image engine decodes with the codecs of the house —
`bunny_ui::codec`, behind the core's `codec` feature, which only this
shell turns on: PNG (every bit depth and color type, the palette, the
transparency chunk, the Adam7 interlace) and JPEG (baseline and
progressive, any sampling, restart intervals, JFIF and Adobe). The
JPEG road decodes the way libjpeg does — the same integer inverse DCT,
the same triangle filter for the chroma, the same fixed-point color
tables — and the fixtures in `crates/bunny_ui/tests/fixtures/codec/`
pin it to libjpeg-turbo's output within two steps per channel. What
the codec refuses, it refuses by name on stderr: arithmetic coding,
lossless, hierarchical, 12-bit samples, CMYK and YCCK. No decoder
here reads the EXIF orientation, and neither do the platform
decoders the other shells use. File icons come from the freedesktop
icon themes on disk.

## The webview

The page is WPE WebKit — WebKit with no GTK in it — loaded at run
time through six libraries the shell opens by name (`libwpe`,
`WPEBackend-fdo`, `WPEWebKit`, GLib, GObject, `libwayland-server`;
`wpe.rs`) and never links: an app without a webview never loads them,
and a box without them refuses the road with one line naming the
package. The engine renders out of process and hands every frame back
as a `wl_shm` buffer; the shell copies it once, straight RGBA, and
paints it as one image where the host stood in the display list
(`webview.rs`). That is the whole island contract on this shell — no
platform view, no sandwich: the scene painted after the host is above
the page by list order on the CPU raster, GL and Vulkan alike, and
the clip open at the mark cuts the page like anything else. The
engine is paced by the shell's own present: a frame is acknowledged
once it went up. The hand is routed by the shell — `Runtime::host_at`
names the page under the pointer — and lands as native libwpe events
the page trusts; the keyboard is the page's from a click in it until
a click outside; the engine's main context is pumped by the door's
loop (its file descriptors ride the same `poll`), so every report
lands on this thread and outside a frame. The container proves it
with `browser_window_linux --drive` (`--editor` for the composer) on
both doors and all three tiers: the bus, the console hook, the
network wrap, every step of the hand's vocabulary read back by the
page's own probe with `trusted=true`, a dead url refusing by name,
the eval and the snapshot; `--page <url>` points the same example at
any page. A still page costs nothing — no frame is exported until the
page changes — and a page that animates renders one frame per ack, at
the rate the software raster allows (a scrollable page shows the
engine's own overlay scrollbar for a few seconds after it loads, and
renders while it fades). The sandbox is WebKit's own (bubblewrap
around the web process); the container has no user namespace to
build one and says `WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1` for
itself. Not on this lane yet: the EGL zero-copy road (the frame as an
`EGLImage` straight into the GL tier) and a cursor the page chooses.
`docs/webview.md` has the capability table.

## Logs

Everything the shell says goes to stderr, one line per event: a tier
that came up or refused, the door that opened, a bus that could not
be reached. `BUNNY_FRAME_STATS=1` adds the `F` line (where the frame's
time went, before the present opened), the `P` line (which tier
presented, and how many presents so far) and the `W` line (a page
frame the engine exported: which host, its size in pixels, its
serial).

## What is not proven here

The container runs software GL and Vulkan on a headless compositor.
A real GPU, a real desktop's decorations, a trackpad's pinch, a
touchscreen, a fractional scale, and a web page at the display's own
rate under a real compositor (the webview's EGL lane, which would
spare the copy, does not exist yet) need a Linux machine with a
screen. The sections above say, item by item, what the container
proved and what waits for that machine.
