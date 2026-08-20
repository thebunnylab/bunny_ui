# finder on the web

The same finder scene, rendered three ways from one wasm binary. The
scene stays semantic in the engine; each page picks how it reaches the
screen. No JavaScript framework, no bindgen — the FFI border is written
by hand, and each glue file IS the platform layer of its mode.

**Canvas mode** (`index.html` + `glue.js`): the engine rasterizes each
frame and the glue blits it whole. Every pixel is ours — the same
compositor that runs on the desktop. Text measures and rasters through
the browser's own fonts (`measureText`, `fillText`) behind the engine's
text border.

**Dom mode** (`dom.html` + `glue_dom.js`): the engine lowers the scene
to element patches — a fixed little-endian stream the glue walks with
one `DataView`. Text selects natively, scroll carries real momentum,
the input owns the editing — a field of many lines mounts as a
`<textarea>`, so the browser wraps, breaks and scrolls it at home.
Hover and pressed states are `:hover` and
`:active` rules — including the INK a subtree inherits, so the row's
path text brightens under the pointer without one patch crossing;
animation specs become CSS transitions. The browser renders at home;
the engine still owns every position.

**The GPU tier**: both pages present through WebGL2 when the browser
gives it. The engine walks the same display list the desktop walks, into
the same wire structs, and the shaders are the same source the OpenGL
tier compiles — only the prelude differs, because a browser speaks GLSL
ES 3.00 and must name its precisions. The CPU rasterizer stays as the
oracle, the fallback, and the answer for a device that refuses.

Add `?present=cpu` to any URL to refuse the tier and compare the two
pictures side by side. A lost context falls back on its own: the tier
rebuilds once in silence, and a second loss hands the page to the
rasterizer for as long as it lives.

The tier is measured against the rasterizer, in the browser, on the
device that runs it:

| scene | worst channel | beyond one step |
| --- | --- | --- |
| flat opaque rects | 0 | 0.0000% |
| a translucent veil | 0 | 0.0000% |
| a rounded fill | 1 | 0.0000% |
| a stroke ring | 1 | 0.0000% |
| a rounded clip | 1 | 0.0000% |
| a shadow's falloff | 1 | 0.0000% |
| a pixel-font run | 0 | 0.0000% |
| one pane of glass | 2 | 0.0234% |
| two panes stacked | 3 | 0.0938% |

Flat colour and pixel-font text are byte-exact. The anti-aliased shapes
stay within one step, where the gate allows two. Glass allows three for
one pane and six for two, and this comes in at two and three. The
numbers are from Chrome on ANGLE over Metal; another browser or another
GPU must say so again for itself.

What it costs to present a full window, on the same machine, medians
of five interleaved rounds of eleven samples with a cooldown between
rounds. Every sample moves one command, because both roads skip a frame
that did not change and the skip is not the thing being measured.

| physical | gpu submit | cpu, fresh surface | cpu, damage only |
| --- | --- | --- | --- |
| 1520x1280 | &lt;0.1 | 8.2 | 4.2 |
| 2560x1440 | &lt;0.1 | 13.6 | 7.6 |
| 3024x1964 | &lt;0.1 | 22.2 | 12.6 |
| 3840x2160 | &lt;0.1 | 31.0 | 17.5 |
| 5120x2880 | &lt;0.1 | 54.9 | 31.0 |

The GPU column INCLUDES the display-list walk, the instance and atlas
upload, and the draw submission through `glFlush`. It EXCLUDES layout
and settle, the GPU's own execution, and the browser's compositing. The
CPU columns include `Surface::frame` and the RGBA mirror, and exclude
the blit.

The GPU numbers are written as "under a tenth" because that is all this
clock can say: `performance.now()` on this page steps in 0.1 ms, and an
empty scene reports the same figure as a five-thousand-pixel one. What
the measurement does establish is the shape — the submit does not grow
with the pixel count over a twelve-fold range, while the rasterizer's
cost grows with it, from four milliseconds to thirty-one.

**Islands**: inside the Dom page, `.rendering(Rendering::Gpu)` claims a
canvas island for one subtree — our layout positions the element, our
rasterizer fills it, and it redraws only when its content changes. The
finder's header shows one: the match count drawn as digit bars, live
while the filter types.

Build and run:

```
cargo build --profile web -p finder-web --target wasm32-unknown-unknown
cp ../../target/wasm32-unknown-unknown/web/finder_web.wasm web/
python3 -m http.server 8871 --directory web
```

Then open http://localhost:8871 (canvas) or
http://localhost:8871/dom.html (dom + island).

One WebGL2 context serves every island on the page. A context for each
would meet the browser's ceiling, and an island element that claimed
WebGL2 could never take `putImageData` again — so a lost context would
leave the whole page dark. The island keeps its own 2d road; the tier
keeps the GL, and copies each island into its element.

The tier costs 57 KB of the binary, 22 KB compressed. A page that does
not want it builds without:

```
cargo build --profile web -p finder-web --target wasm32-unknown-unknown --no-default-features
```

One binary carries both shells and the tier (653 KB through the `web`
profile — `opt-level = "z"`, one codegen unit, lto, and the name table
stripped; 597 KB without the tier). Ten thousand rows
stay virtualized in both modes: the scroll geometry is honest to the
full extent, and only the visible window exists.

Images ride the same premise. The header's bunny mark is a real PNG
written by hand in the demo; the browser decodes it asynchronously
(one crossing per identity, `createImageBitmap` off the main thread)
and reports back — the first paint ships without it, the ready event
reflows. On the canvas page the decoded pixels come back once and our
compositor blends them; on the dom page the mark is a native `<img>`
on a blob URL and not one pixel crosses the border.

Popovers too: click the selected row and its details card anchors at
the row's trailing edge, clamped inside the viewport — the web
fallback of a scene that, on the desktop shell, steps OUTSIDE the
window on a child panel. Escape closes it; a press outside closes it
and is consumed. In dom mode the card mounts as the root's last child
(the portal), so no scroll container ever clips it.
