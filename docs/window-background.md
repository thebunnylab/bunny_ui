# A window's background

*Status: a window stands on a blur of what is behind it on macOS, seen
on macOS 26.5 on the Metal road and on the CPU road
(`cargo run -p bunny-ui-macos --example blur_window`). A background
that replaces what is under it is in the core, the CPU raster and every
GPU tier; Metal, GL and Vulkan are compared against the raster on a
solid canvas and on a clear ground, the last two on Mesa's software
renderers in the container. On Linux a clear window ran on both doors
and the three roads, and a blurred one ran only as the fall to opaque,
because no compositor with a blur was at hand. The Windows shell is
type-checked and has not run. The details are in
[What each platform serves](#what-each-platform-serves) and in
[Honest edges](#honest-edges).*

A window has always been opaque. The canvas is the colour a frame is
cleared to, and nothing behind the window shows. A desktop app often
wants the opposite: the whole window stands on a blur of the desktop,
and its panels are shades over that blur.

That needs two things. The window must tell the system that it is not
opaque. And the scene needs a rule for a panel inside a panel, because
the usual rule makes the blur disappear.

## The ask

```rust
use bunny_ui::theme::{self, Theme};
use bunny_ui_macos::{App, WindowBackground, WindowSpec};

// the canvas at 60 %: this is the tint over the blur
let palette = Theme::dark();
theme::install(Theme { canvas: palette.canvas.with_alpha(153), ..palette });

app.open(
    WindowSpec::titled("Trinity").size(1280.0, 800.0).background(WindowBackground::Blurred),
    runtime,
    root,
);
```

`WindowBackground` has three values.

| Value | What the window shows where the scene paints nothing |
| -- | -- |
| `Opaque` | The canvas at full strength. This is the default. The alpha of the canvas is ignored. |
| `Clear` | What is behind the window, sharp, under the canvas at the canvas's own alpha. |
| `Blurred` | The same, and the system blurs what is behind the window. |

In a window that is not opaque the alpha of the theme's canvas is real.
It is how strongly the canvas lies over what is behind the window. A
solid canvas hides the blur completely, so a scene that wants the blur
must thin its canvas. No view in the scene needs to know that the
window is not opaque.

A dialog and a popover always stand on the solid canvas.

### What the system serves

Not every system can serve every ask. The rule is one pure function,
`WindowBackground::resolve(alpha, blur)`, and each shell supplies the
two facts: `alpha`, the system composites a window by the window's own
alpha, and `blur`, the system can blur what is behind a window.

- `Clear` needs `alpha`.
- `Blurred` needs `alpha` and `blur`. Without both it resolves to
  `Opaque`, never to `Clear`. A scene drawn to stand on a blur cannot be
  read over a sharp desktop.

A shell answers in two places. `window_background(asked)` is the
background it will serve, before a window exists. `App::background(id)`
is the background an open window was given.

A window that resolved to `Opaque` paints exactly what an opaque window
paints: its canvas is forced to full strength. An app can install one
thinned theme and open the same spec on every system.

## A background that replaces

Every paint is source-over. An alpha adds to the alphas under it. In a
window that stands on a blur, with a canvas at 60 %:

| The scene | `BackgroundBlend::Over` (default) | `BackgroundBlend::Replace` |
| -- | -- | -- |
| A panel at 50 % | 80 % solid | 50 % |
| A second panel at 50 %, inside it | 90 % solid | 50 % |
| A pane of glass inside a pane of glass | the blur of a blur, both tints | one blur, the inner tint |
| The same panels in an opaque window | the alphas add up | each panel over the canvas alone |

```rust
sidebar
    .background_color(SHADE.with_alpha(128))
    .background_blend(BackgroundBlend::Replace)
```

With `Replace` the view's shape is first returned to the ground of the
surface. Its colour and its ramp are then painted on the ground alone.
The ground is what is behind the scene: what is behind the window when
the window is not opaque, and the canvas when it is.

- It is not the default. A scene that says nothing paints as before.
- It is the view's own word. It is not inherited: a hover veil inside a
  panel that replaces still lies over the panel.
- The halo the same view casts stays outside the shape. A translucent
  box is not darkened by its own shadow.
- `.opacity(…)` scales it. A view at zero takes nothing away.

On a pane of glass the word means one blur and one tint. A pane inside
a pane reads the scene the outer pane read, before the outer pane and
everything in it were painted. It shares that capture only while it is
the last one taken. A pane that captured in between left a newer scene,
and the next pane then reads the scene as it stands.

### How it travels

The scene carries one command, `DrawCommand::Knockout { rect,
corner_radius, opacity }`. It has no colour.

| Target | What a knockout is |
| -- | -- |
| A solid canvas (an opaque window, every page) | The canvas painted in the shape at that strength: an ordinary rect in the ordinary run. |
| A clear ground (a translucent window, an island over nothing) | Nothing is painted. The target keeps `1 - alpha` of itself, colour and alpha alike. |

The CPU raster keeps the ground beside its bitmap and does both. The
shared GPU walk is told the floor of its target and lowers the command
the same way: over a clear ground it makes a run of its own,
`RunKind::Knockout`, which a tier draws with the rect shaders under an
erase blend. In the element renderer the view paints the one solid
colour its background makes over the canvas.

A pane that shares carries `GlassPaint::shares`. A GPU tier builds no
blur pyramid for its batch and reads the one already built. The raster
keeps the pixels of that moment.

## A translucent target

A presenter is told once, when it is installed, that the system
composites its target by the target's own alpha.

1. The frame clears to the canvas multiplied by its own alpha.
   `raster::premultiply` is the rounding every tier uses.
2. The house blend does not change. Straight source over a
   premultiplied destination stays premultiplied.
3. A knockout is drawn with the erase blend.
4. A pane of glass takes the place of what it covers. The scene it
   reads may not be solid, and its pixel then has an alpha of its own.
   Where the scene is solid this is source-over, as before.
5. The alpha the scene leaves reaches the system.

An opaque window stays on the road it had, with no new cost.

## What each platform serves

| | macOS | Windows | Linux, Wayland | Linux, X11 | iOS, Android, web |
| -- | -- | -- | -- | -- | -- |
| `Clear` | yes | a composition swap chain¹ | buffer alpha² | an ARGB visual and a compositing manager² | no window spec: always opaque |
| `Blurred` | `NSVisualEffectView` behind the window | the DWM system backdrop, Windows 11 22H2 and later¹ | `ext-background-effect-v1`, or the KDE blur protocol³ | `_KDE_NET_WM_BLUR_BEHIND_REGION`³ | — |
| `BackgroundBlend::Replace` | yes | yes | yes | yes | yes, over the canvas |
| seen on a display | yes | no | no | no | — |

¹ Type-checked for `x86_64-pc-windows-msvc` from macOS. Its tests run
first in CI, on a headless runner.
² Ran in the container: on Weston by the CPU, GL and Vulkan roads, read
back from the compositor as the canvas at 60 % over black; under Xvfb
with a compositing manager on the three roads, and as `Opaque` without
one.
³ No compositor with a blur was available. What ran is `Blurred`
resolving to `Opaque` on both doors. The blur requests, the region and
the fall at runtime are type-checked and unit-tested: the request order
of both Wayland protocols is tested against their XML.

On macOS the blur is the system's own view, with the material the
system puts behind a whole interface (`FullScreenUI`). It is the
thinnest of the materials that follow the window's appearance: measured
against saturated bands it lets through about two and a half times the
colour `UnderWindowBackground` does. The window's appearance follows
the luma of the canvas, so the material follows the scene and not the
desktop. Where the reader asked the system to reduce transparency the
view answers with a solid colour.

A translucent window on macOS takes none of the native roads of the
window presenter: the bands, the sparse foreground, the patches and the
scroll bands all declare opaque pixels. Its first frame promotes to
Metal and each frame is presented whole.

On Windows the swap chain of an HWND ignores alpha. A window that is
not opaque is therefore created with no redirection bitmap and presents
through a composition swap chain with premultiplied alpha, which
DirectComposition puts on the window behind its child windows. Only
such a window takes that road. Its present is installed when the window
is created, because the extended style is read at creation alone and
nothing GDI paints on such a window is shown. Where the present or the
blur does not come up, the window is destroyed and an opaque one is
created in its place. The facts for the rule are: the GPU road is not
refused, the shader compiler and `dcomp.dll` are on the system, and for
a blur the build is 22621 or later. The acrylic is light or dark by the
dark-caption attribute, which follows the luma of the canvas.

On Wayland a clear window needs only a buffer with alpha. A blurred one
binds two optional globals, `ext_background_effect_manager_v1` first and
`org_kde_kwin_blur_manager` for an older Plasma, and sends an explicit
region before the commit that presents. The region follows the rounded
corners of a window whose bar the scene draws, so no blurred square
shows behind a transparent corner. A compositor that takes its blur away
makes the window opaque from the next frame. On X11 a window that is not
opaque takes the 32-bit visual, and is served only when a compositing
manager owns `_NET_WM_CM_S0`; the blur is asked for with the KDE
property, when the root window carries it. The Vulkan road asks for a
premultiplied composite alpha and steps down to GL, then to the CPU,
where the surface does not offer one.

## The proof

```bash
cargo test -p bunny-ui-core --all-targets --all-features   # the rule, the raster, the walk, the element renderer
cargo test -p bunny-ui-apple --all-targets                 # Metal against the raster, solid canvas and clear ground
cargo test -p bunny-ui-macos --all-targets                 # the spec, the dress
cargo test -p bunny-ui-preview --all-targets               # tags 11 and 12 on the wire
cargo run -p bunny-ui-macos --example blur_window          # and with --clear, --opaque, --light
BUNNY_PRESENT=cpu cargo run -p bunny-ui-macos --example blur_window
crates/bunny_ui_linux/container/run.sh test                # GL and Vulkan against the raster, on llvmpipe and lavapipe
crates/bunny_ui_linux/container/run.sh drive blur_window_linux   # and with --clear, --opaque
cargo check -p bunny-ui-windows --target x86_64-pc-windows-msvc --tests --examples
```

`blur_window` shows two stacks of three shades at one alpha. On the
left each shade adds to the ones under it. On the right each shade
replaces what is under it, and the blur shows through all three at the
same strength.

## Honest edges

- Only macOS has been seen on a display. The acrylic of Windows 11 in
  light and dark, an inactive window, a live resize under
  DirectComposition and a webview inside a composed window need a real
  machine. So does the blur of KWin, Mutter and niri.
- On Linux three facts are from memory and were never run: KWin reads a
  new blur region only when a new blur object is set on the surface;
  KWin announces its X11 blur by the property on the root window; and
  the property on a window is CARDINAL/32 rectangles in device pixels.
  X11 resolves the background when the window is created and does not
  watch for a compositor that leaves. No real GPU driver was tried.
- wayland-protocols 1.45, the first release with
  `ext-background-effect-v1`, names the blur capability 0. Release 1.46
  corrected it to 1, which is what a compositor sends and what the shell
  reads.
- On Windows the vtable slots of `IDCompositionDevice`,
  `IDCompositionTarget`, `IDCompositionVisual` and
  `CreateSwapChainForComposition` are the order of the headers, counted
  by hand and never run. A wrong slot is an access violation in CI, not
  a failed assertion.
- On Windows a bar the scene draws (`Chrome::Scene`) with `Blurred` is
  an open question: with the frame extended the compositor may show its
  caption tint or its buttons through the scene. The three switches
  that answer it are constants in `apply_backdrop`, all off, and
  `blur_window --scene` is the probe.
- Only the raster and the Metal tier make a pane of glass take the place
  of what it covers over a clear ground. On the D3D11, GL and Vulkan
  tiers a pane there is still laid over the ground, so it is more solid
  than its tint says and the sharp scene shows faintly through it. A
  background that replaces is exact on every tier.
- A window that is not opaque costs more to compose on macOS than an
  opaque one, because it gives up the native roads.
- `Blurred` where the system has no blur is `Opaque`: Windows 10, a
  Wayland compositor without either protocol, X11 without a compositing
  manager.
- In the element renderer a pane that shares has no `backdrop-filter`
  of its own, and the tint of the outer pane stays under it.
- A pane that shares with another pane capturing between it and the
  pane around it reads the scene as it stands.
- On the retained CPU surface a pane that shares and reaches outside
  the pane around it can read pixels painted after the capture.
- A popover is a panel of its own with nothing blurred behind it. Its
  material reads the window as an opaque one.
- Dialogs are opaque.
