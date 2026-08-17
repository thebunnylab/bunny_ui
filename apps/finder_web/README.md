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
the input owns the editing. Hover and pressed states are `:hover` and
`:active` rules; animation specs become CSS transitions. The browser
renders at home; the engine still owns every position.

**Islands**: inside the Dom page, `.rendering(Rendering::Gpu)` claims a
canvas island for one subtree — our layout positions the element, our
rasterizer fills it, and it redraws only when its content changes. The
finder's header shows one: the match count drawn as digit bars, live
while the filter types.

Build and run:

```
cargo build --release -p finder-web --target wasm32-unknown-unknown
cp ../../target/wasm32-unknown-unknown/release/finder_web.wasm web/
python3 -m http.server 8871 --directory web
```

Then open http://localhost:8871 (canvas) or
http://localhost:8871/dom.html (dom + island).

One binary carries both shells (~590 KB, `opt-level = "z"` + lto). Ten
thousand rows stay virtualized in both modes: the scroll geometry is
honest to the full extent, and only the visible window exists.
