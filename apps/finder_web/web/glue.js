// The glue: the browser side of the hand-written FFI border.
// It forwards DOM events into the wasm exports and paints the RGBA
// frames the engine hands back. No frameworks, no bindgen output —
// this file IS the web platform layer.

const canvas = document.getElementById("app");
const context = canvas.getContext("2d");

let wasm = null;
let frameArmed = false;
let lastTick = 0;

// The hidden ink canvas: the engine's TextEngine measures and rasters
// through it. Grow-only, read often (getImageData every raster).
const decoder = new TextDecoder();
let inkCanvas = null;
let inkContext = null;

function inkSurface(width, height) {
  if (!inkCanvas) {
    inkCanvas = document.createElement("canvas");
    inkCanvas.width = 256;
    inkCanvas.height = 64;
    inkContext = inkCanvas.getContext("2d", { willReadFrequently: true });
  }
  if (inkCanvas.width < width) inkCanvas.width = width;
  if (inkCanvas.height < height) inkCanvas.height = height;
  return inkContext;
}

function cssFont(size, weight, mono) {
  const family = mono
    ? 'ui-monospace, Menlo, Consolas, monospace'
    : 'system-ui, -apple-system, "Segoe UI", sans-serif';
  return `${weight} ${size}px ${family}`;
}

function requestFrame() {
  if (frameArmed) return;
  frameArmed = true;
  requestAnimationFrame((timestamp) => {
    frameArmed = false;
    const dt = lastTick ? (timestamp - lastTick) / 1000 : 1 / 60;
    lastTick = timestamp;
    wasm.bunny_frame(dt);
  });
}

const imports = {
  bunny: {
    js_blit(pointer, width, height) {
      if (canvas.width !== width || canvas.height !== height) {
        canvas.width = width;
        canvas.height = height;
      }
      const pixels = new Uint8ClampedArray(
        wasm.memory.buffer,
        pointer,
        width * height * 4,
      );
      context.putImageData(new ImageData(pixels, width, height), 0, 0);
    },
    js_request_frame() {
      requestFrame();
    },
    // dom-mode imports — the single binary carries both shells, and
    // this page only ever drives the canvas one
    js_apply_patches() {},
    js_island() {},
    // Writes [width, ascent, descent] as three f64 at `out` — logical
    // px. Ascent/descent come from the FONT's bounding box (stable per
    // font); an empty string keeps the metrics and reports width 0.
    js_measure_text(pointer, length, size, weight, mono, out) {
      const text = decoder.decode(
        new Uint8Array(wasm.memory.buffer, pointer, length),
      );
      const ink = inkSurface(1, 1);
      ink.font = cssFont(size, weight, mono);
      const probe = ink.measureText(text || "Mg");
      const metrics = new Float64Array(wasm.memory.buffer, out, 3);
      metrics[0] = text ? probe.width : 0;
      metrics[1] = probe.fontBoundingBoxAscent ?? size * 0.8;
      metrics[2] = probe.fontBoundingBoxDescent ?? size * 0.25;
    },
    // Draws one line into a width×height physical rectangle and copies
    // the RGBA into wasm memory at `out`. getImageData hands back
    // straight alpha — the compositor's contract, no conversion here.
    js_raster_text(
      pointer,
      length,
      size,
      weight,
      mono,
      scale,
      width,
      height,
      descent,
      color,
      out,
    ) {
      const text = decoder.decode(
        new Uint8Array(wasm.memory.buffer, pointer, length),
      );
      const ink = inkSurface(width, height);
      ink.setTransform(1, 0, 0, 1, 0, 0);
      ink.clearRect(0, 0, width, height);
      ink.setTransform(scale, 0, 0, scale, 0, 0);
      ink.font = cssFont(size, weight, mono);
      ink.textBaseline = "alphabetic";
      const r = (color >>> 24) & 0xff;
      const g = (color >>> 16) & 0xff;
      const b = (color >>> 8) & 0xff;
      const a = color & 0xff;
      ink.fillStyle = `rgba(${r}, ${g}, ${b}, ${a / 255})`;
      // baseline sits `descent` above the box bottom — the ceil slack
      // stays on top, mirroring the desktop engine
      ink.fillText(text, 0, height / scale - descent);
      const pixels = ink.getImageData(0, 0, width, height).data;
      new Uint8Array(wasm.memory.buffer, out, width * height * 4).set(pixels);
    },
  },
};

function sendText(text) {
  const bytes = new TextEncoder().encode(text);
  const pointer = wasm.bunny_alloc(bytes.length);
  new Uint8Array(wasm.memory.buffer, pointer, bytes.length).set(bytes);
  wasm.bunny_text(pointer, bytes.length);
}

const KEYS = {
  Backspace: 1,
  Delete: 2,
  ArrowLeft: 3,
  ArrowRight: 4,
  Home: 5,
  End: 6,
  Escape: 7,
};

WebAssembly.instantiateStreaming(fetch("finder_web.wasm"), imports).then(
  ({ instance }) => {
    wasm = instance.exports;
    window.__bunny = wasm;
    const scale = window.devicePixelRatio || 1;
    const width = canvas.clientWidth;
    const height = canvas.clientHeight;
    wasm.start(width, height, scale);

    const point = (event) => {
      const rect = canvas.getBoundingClientRect();
      return [event.clientX - rect.left, event.clientY - rect.top];
    };
    canvas.addEventListener("pointermove", (event) => {
      const [x, y] = point(event);
      wasm.bunny_pointer_move(x, y);
    });
    canvas.addEventListener("pointerdown", (event) => {
      const [x, y] = point(event);
      wasm.bunny_pointer_down(x, y);
    });
    canvas.addEventListener("pointerup", (event) => {
      const [x, y] = point(event);
      wasm.bunny_pointer_up(x, y);
    });
    canvas.addEventListener(
      "wheel",
      (event) => {
        event.preventDefault();
        const [x, y] = point(event);
        wasm.bunny_wheel(x, y, event.deltaX, event.deltaY);
      },
      { passive: false },
    );
    window.addEventListener("keydown", (event) => {
      const code = KEYS[event.key];
      if (code !== undefined) {
        event.preventDefault();
        wasm.bunny_key(code, event.shiftKey ? 1 : 0);
        return;
      }
      if (event.key.length === 1 && !event.metaKey && !event.ctrlKey) {
        event.preventDefault();
        sendText(event.key);
      }
    });
  },
);
