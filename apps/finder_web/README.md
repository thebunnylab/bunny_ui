# finder on the web

The same finder scene, in a browser tab. The engine rasterizes the
frame; a small `glue.js` blits it to a canvas and forwards the events.
No JavaScript framework, no bindgen — the FFI border is written by
hand.

Build and run:

```
cargo build --release -p finder-web --target wasm32-unknown-unknown
cp ../../target/wasm32-unknown-unknown/release/finder_web.wasm web/
python3 -m http.server 8871 --directory web
```

Then open http://localhost:8871.
