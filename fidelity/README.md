# Fidelity

A deterministic harness that holds bunny's two web lowerings against
pages written by hand.

Each case is a pair: `case.html`, a page as a web developer would write
it, and `case.rs`, the bunny view meant to look exactly the same. One
headless Chrome captures three renderings of each case at a fixed
viewport and pixel ratio:

| | What | How |
|---|---|---|
| **A** | `case.html` | Chrome's own layout and paint — the truth |
| **B** | the bunny view on elements | `start_dom`: the flow lowering, CSS the browser lays out |
| **C** | the bunny view on WebGL | `start` with `?present=gpu`: bunny's layout, drawn by the GPU tier |

Three comparisons say who disagrees with whom:

- **A×B** — is the element lowering faithful to the page?
- **A×C** — is the WebGL tier faithful to the page?
- **B×C** — do bunny's two lowerings agree with each other? A mistake both
  share (a measured word, a layout rule) shows in A×B and A×C but not
  here; one backend's own shows here.

## Running

```sh
cargo run -p bunny-cli --release -- fidelity sweep
```

| Option | |
|---|---|
| `--only CASE` | Capture this case alone; may repeat |
| `--bless` | Write the numbers that shrank, and new cases, into `baseline.tsv` |
| `--check-determinism` | Capture everything twice, in two browsers, and require the same bytes |
| `--chrome PATH` | The browser to capture with |

The sweep builds `fidelity/corpus` for `wasm32-unknown-unknown` (release),
lays a site out under `target/fidelity/site`, serves it on the loopback,
captures, compares, and writes:

- `target/fidelity/report.md` — the table and, per case and pair, every
  box that disagrees ("`share` moved +2.00, +1.00 px") and the largest
  runs of differing pixels;
- `target/fidelity/cases/<case>/{A,B,C}.png` — the three captures;
- `target/fidelity/cases/<case>/diff-{AxB,AxC,BxC}.png` — the first capture
  faded to gray, the differing pixels outside text in red, those under
  text in blue, each disagreeing box outlined where the first side put it
  (green) and where the second did (magenta).

It exits non-zero when a number grew against the baseline, or when
`--check-determinism` found two runs that differ.

### The browser

The baseline names the browser it was measured with
(`# chrome` in `baseline.tsv`); a sweep with another version warns,
because the numbers move with Chrome's rasterizer. Today that is Chrome
for Testing 150.0.7871.24:

```sh
npx @puppeteer/browsers install chrome@150.0.7871.24 --path gitignore/chrome
```

The browser is found in this order: `--chrome`, `BUNNY_CHROME`, a Chrome
for Testing under `gitignore/chrome` (this checkout's, or the main
checkout's from a worktree), then the system's Chrome or Chromium.

## The numbers

Each pair keeps three numbers:

1. **geometry** — the named boxes that disagree. A box is named by
   `data-fid="x"` in the page and `.test_id("x")` in the view. A and B
   read `getBoundingClientRect`; C reads the frame bunny's layout gave
   the view. Two boxes agree within a sixty-fourth of a CSS pixel, the
   unit the browser keeps its geometry in; a name one side lacks
   disagrees.
2. **pixels outside text** — screenshot pixels (`Page.captureScreenshot`)
   that differ in any channel, outside every text box.
3. **pixels in text** — the same, inside a text box: the client rects of
   every text node in A and B, every text frame in C, each grown by one
   CSS pixel for antialiasing and overhangs. Two glyph rasterizers rarely
   agree byte for byte; this bucket keeps them from hiding everything
   else.

**Pixel perfect** is zero geometry and zero pixels outside text.

## The ratchet

`baseline.tsv` holds the numbers, one line per case and pair:

```
case	pair	geometry	pixels_outside_text	pixels_in_text
card	AxC	7	250993	15047
```

A sweep fails when any number grows. `--bless` writes the current numbers
when none grew — a better number becomes the new floor, a new case gets
its first line. A number that grows is never blessed: fix it, or, when
the change is deliberate, delete the case's lines and bless again, and
say why in the commit.

## Determinism

Nothing is compared before it is repeatable:

- Chrome runs with `--headless=new --disable-gpu
  --disable-gpu-rasterization --disable-gpu-compositing
  --force-color-profile=srgb --disable-lcd-text
  --font-render-hinting=none --hide-scrollbars`: Skia on the CPU paints
  the page, colors stay sRGB, text is grayscale and unhinted.
- WebGL on SwiftShader needs `--enable-unsafe-swiftshader`: since Chrome
  137 a WebGL context on the software device is refused without it, and
  Chrome 150 has no WebGL2 at all under `--disable-gpu` otherwise.
  bunny's GPU tier refuses a software context on its own pages
  (`failIfMajorPerformanceCaveat`); `?present=gpu` forces it.
- The pixel ratio is the device's, 2, from `--force-device-scale-factor`:
  Chrome then lays out and snaps boxes in device pixels, as on a HiDPI
  screen. The ratio `Emulation.setDeviceMetricsOverride` emulates lays
  out and snaps in CSS pixels and scales afterwards — a 94.67 px column
  ends on device pixel 190 there, and on 189 on a real screen.
- Each capture opens a fresh tab at the case's viewport
  (`Emulation.setDeviceMetricsOverride`, ratio left to the device), light
  scheme, reduced motion.
- Before the page's own scripts, `Date` reads one instant
  (2026-01-01T00:00:00Z) and `Math.random` is a seeded sequence.
- The load runs on the CDP virtual clock: paused at that instant while
  the navigation starts, then given a 5 s budget that stands still while
  a fetch is pending (`pauseIfNetworkFetchesPending`). A budget granted
  before the navigation would be spent at once on the blank page.
- The capture waits for the page to draw (`window.__bunnyBoot` for B, the
  layout export for C), `document.fonts.ready`, every picture decoded,
  no request in flight, two `requestAnimationFrame`s, and then two
  consecutive screenshots with the same bytes.
- `--check-determinism` captures everything twice, each time in a new
  browser with a new profile, and fails on any byte or box that moved.

## The corpus

```
fidelity/
  cases/<case>/case.html   the page
  cases/<case>/case.rs     the view: `pub fn view() -> impl View`
  corpus/                  the crate the views build into
  fonts/                   the one face both sides use, and its license
  assets/                  pictures both sides use
  baseline.tsv             the ratchet
```

The corpus crate's build script turns every folder with a `case.rs` into
a module and two wasm entries, `start_dom_<case>` and `start_gpu_<case>`;
adding a case is adding its folder. `cargo test -p fidelity-corpus` lays
every case out natively and checks that each names its page and at
least one box, each name once.

### Writing the page

The page is the reference, so it must not borrow from bunny: write the
CSS a web developer would write for the design, never what bunny's
lowering emits. A translation mistake that the reference copies agrees
with itself.

- Link `/fonts/fonts.css` and use `font: 16px "Fid Sans"` on the body.
  Both sides read the same file (Lato Medium, `fonts/OFL.txt`); no system
  face takes part. The face is declared for weights 100–900 so no side
  ever synthesizes a bold.
- Start from the shared rules: `*, *::before, *::after { box-sizing:
  border-box }`, `html, body { margin: 0 }`, and a `.page` with the
  viewport's size that holds the content at its top-left corner
  (`display: flex; flex-direction: column; align-items: flex-start`).
- Name a size other than 400 × 300 with
  `<meta name="fidelity" content="width=400, height=340">`.
- Name every box worth comparing with `data-fid`. A paragraph is a
  block (`<p>`): its box is as wide as its container, which is the box a
  bunny text that wraps takes. A short label in a flex row or column is
  any element around the words: a flex item fits its content.
- Say the design, not bunny's model of it. bunny draws a border over the
  box's edge and lays nothing out around it; a field whose words stand
  12 px from the outer edge under a 1 px stroke is `padding-left: 11px`
  in CSS (`cases/form`).

### Writing the view

```rust
use crate::prelude::*;

#[must_use]
pub fn view() -> impl View {
    page(400.0, 300.0, vstack!(block(48.0, 24.0, BLUE, "a")).spacing(8.0))
}
```

`crate::prelude` holds the corpus's face, its inks, `page` (the `.page`
rule: white, the content at the top-left, the face at 16 px, named
`page`) and `block` (a filled box of a fixed size). Name each box with
`.test_id("x")`, the same name the page gave it.

## The test-id hook

`.test_id("x")` is part of bunny-ui, not of the harness: any tool can find
a view by it.

- On the element lowering it is the element's `data-testid` attribute
  (wire ABI 27; the served page writes it too).
- On the pixel pipelines `Runtime::test_frames()` answers every named
  view's frame in the window's coordinates;
  `bunny_ui::layout::record_text_frames(true)` also keeps every text's
  frame (`Runtime::text_frames()`).
- A web shell built with the `probe` feature answers both as JSON from
  `bunny_test_frames_json()`, readable at `bunny_probe_ptr()`.
