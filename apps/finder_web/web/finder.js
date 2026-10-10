// The finder's page, before the glue boots: which wasm it is, and the
// finder's own door to the network as an import module of the page's.
// The engine opens no socket; the answer goes back through the export
// the finder declared (`finder_fetched`). A failed fetch answers with an
// empty body — the task decides what that means.
window.BUNNY_WASM = "finder_web.wasm";
window.BUNNY_IMPORTS = {
  app: {
    js_fetch(pointer, length) {
      const wasm = window.__bunny;
      const url = new TextDecoder().decode(
        new Uint8Array(wasm.memory.buffer, pointer, length),
      );
      fetch(url)
        .then((response) => (response.ok ? response.text() : ""))
        .catch(() => "")
        .then((text) => {
          const bytes = new TextEncoder().encode(text);
          const out = wasm.bunny_alloc(bytes.length);
          new Uint8Array(wasm.memory.buffer, out, bytes.length).set(bytes);
          wasm.finder_fetched(out, bytes.length);
        });
    },
  },
};
