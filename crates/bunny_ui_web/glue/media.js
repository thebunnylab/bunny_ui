// The media registry: the page's door for a MediaStream (`docs/video.md`).
//
// A stream never crosses into wasm — there is no number for it, and the
// engine has no use for one. The page registers it HERE, hands the
// engine the integer this answers through an export of the app's own,
// and the engine names it back by that integer when it places a video
// host: `js_host_video` on the canvas road, a set-video patch on the
// element road. The glue then finds the stream in this table and hands
// it to the element. Zero is no stream. Loaded before glue.js (or
// glue_dom.js), the way surface.js is; a page that never shows a video
// may leave it out, and the glue plays nothing.
//
// The page stays the stream's owner throughout. Releasing a handle
// forgets the name; it stops no track, and an element still showing the
// stream keeps it until its host leaves the scene.

const bunnyMedia = (() => {
  const streams = new Map();
  let next = 1;
  return {
    // Registers `stream` and answers its handle — the integer the app's
    // `MediaHandle` wraps.
    register(stream) {
      const handle = next++;
      streams.set(handle, stream);
      return handle;
    },
    // Forgets a handle.
    release(handle) {
      streams.delete(handle >>> 0);
    },
    // The stream behind a handle, or null.
    get(handle) {
      return streams.get(handle >>> 0) ?? null;
    },
  };
})();
