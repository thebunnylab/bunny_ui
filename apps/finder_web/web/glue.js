// The glue: the browser side of the hand-written FFI border.
// It forwards DOM events into the wasm exports and paints the RGBA
// frames the engine hands back. No frameworks, no bindgen output —
// this file IS the web platform layer.

const canvas = document.getElementById("app");
const context = canvas.getContext("2d");

let wasm = null;
let frameArmed = false;
let lastTick = 0;

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
