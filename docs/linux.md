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
crates/bunny_ui_linux/container/run.sh shell           # both displays up
```

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

## Logs

Everything the shell says goes to stderr, one line per event: a tier
that came up or refused, the door that opened, a bus that could not
be reached. `BUNNY_FRAME_STATS=1` adds the `F` line (where the frame's
time went, before the present opened) and the `P` line (which tier
presented, and how many presents so far).

## What is not proven here

The container runs software GL and Vulkan on a headless compositor.
A real GPU, a real desktop's decorations, a trackpad's pinch, a
touchscreen and a fractional scale need a Linux machine with a screen.
The sections above say, item by item, what the container proved and
what waits for that machine.
