// The border as ES modules, for a page whose LOADER is someone else's.
//
// `glue.js` instantiates the wasm itself and IS the platform layer of a
// page we own whole. This file is the same border for a page whose
// module is instantiated by another loader — wasm-bindgen's, say —
// which resolves an import module it does not own as
// `import * as m from "./bunny.js"`. So nothing here touches
// WebAssembly: the page hands the instance over ONCE, through `attach`,
// and everything after travels through events, exactly as in glue.js.
//
// Three rules the classic glue never needed:
//
// 1. This module is evaluated wherever the wasm is instantiated — and a
//    threaded build instantiates it again in every Web Worker it
//    spawns. Nothing touches `document` or `window` at module top level;
//    every DOM read waits for `attach`, which only the page calls. A
//    verb that fires in a worker before any attach answers quietly.
// 2. Under shared memory (`--shared-memory`) `memory.buffer` is a
//    SharedArrayBuffer, and `ImageData` and `TextDecoder.decode` refuse
//    a view over one. `bytes()` copies then; writes INTO the memory stay
//    on views, which is legal either way.
// 3. A task that wakes on a WORKER wakes the executor of the worker's
//    own instance — an engine that is not there. The wake crosses to the
//    page on a BroadcastChannel and lands in the page's single microtask,
//    so a query that came home is a frame and not a beachball.

import { attach as attachGpu } from "./bunny_gpu.js";

// The wire contract this file mirrors (bunny_ui::dom::ABI_VERSION):
// the key table, the modifier bits, the import/export surface. The
// wasm exports its own number; `attach` compares the two and refuses a
// pairing this mirror was not written for.
export const EXPECTED_ABI = 12;

const IN_WORKER = typeof window === "undefined";
const WAKE_CHANNEL = "bunny-wake";

let memory = null;
let wasm = null;
let host = null;
let shared = false;
let frameArmed = false;
let wakeArmed = false;
let lastTick = 0;
// asked before the page attached: answered the moment it does
let pendingFrame = false;
let pendingWake = false;
let wakeChannel = null;
let primaryIsMeta = true;

const decoder = new TextDecoder();

// MARK: - Memory

// A view over wasm memory — or a copy of it when the memory is shared,
// for the two consumers that refuse a shared view.
function bytes(pointer, length) {
  const view = new Uint8Array(memory.buffer, pointer >>> 0, length >>> 0);
  return shared ? view.slice() : view;
}

function text(pointer, length) {
  return length ? decoder.decode(bytes(pointer, length)) : "";
}

// MARK: - The presentation surface

// A canvas element's context kind is fixed for its LIFE. Claim "2d" and
// webgl2 is gone from that element forever; claim webgl2 and the CPU
// road can never blit into it again. So the page owns a WRAPPER (the
// host) and the surface is its CHILD, minted the first time a tier asks
// for it and swapped whole when a tier falls.
const surfaces = new Map();

function surface(kind) {
  let canvas = surfaces.get(kind);
  if (!canvas) {
    canvas = document.createElement("canvas");
    canvas.style.cssText = "display:block;width:100%;height:100%;outline:none";
    surfaces.set(kind, canvas);
  }
  if (canvas.parentNode !== host) {
    for (const [held, spent] of surfaces) {
      if (held === kind) continue;
      // a zero-sized backing store is the only way to hand the memory
      // back before the element is collected
      spent.width = 0;
      spent.height = 0;
      surfaces.delete(held);
    }
    host.replaceChildren(canvas);
  }
  return canvas;
}

// The 2d context is claimed on the FIRST blit, never at load.
let paintCanvas = null;
let paintContext = null;
function painter() {
  if (!paintContext) {
    paintCanvas = surface("2d");
    paintContext = paintCanvas.getContext("2d");
  }
  return paintContext;
}

// The hidden ink canvas: the engine's TextEngine measures and rasters
// through it. Grow-only, read often (getImageData every raster).
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

// A family the app NAMED comes first and the house stack stays behind
// it as the fallback — a name this page does not carry then reads in
// the face it would have had anyway. A page that ships faces of its own
// declares them as @font-face under the names the app uses.
function cssFont(size, weight, mono, italic, named) {
  const house = mono
    ? 'ui-monospace, Menlo, Consolas, monospace'
    : 'system-ui, -apple-system, "Segoe UI", sans-serif';
  const family = named ? `"${named.replace(/"/g, "")}", ${house}` : house;
  // CSS order is style, then weight, then size — a leaning label is a
  // real face, never a skew we paint ourselves
  return `${italic ? "italic " : ""}${weight} ${size}px ${family}`;
}

// MARK: - The frame and the wake

function requestFrame() {
  if (!wasm) {
    pendingFrame = true;
    return;
  }
  if (frameArmed) return;
  frameArmed = true;
  requestAnimationFrame((timestamp) => {
    frameArmed = false;
    const dt = lastTick ? (timestamp - lastTick) / 1000 : 1 / 60;
    lastTick = timestamp;
    wasm.bunny_frame(dt);
  });
}

// ONE turn, out of the current job. The flag folds a burst of sends
// into a single wake, and the microtask keeps the engine off the stack
// of whatever called back.
function requestWake() {
  if (!wasm) {
    pendingWake = true;
    return;
  }
  if (wakeArmed) return;
  wakeArmed = true;
  queueMicrotask(() => {
    wakeArmed = false;
    wasm.bunny_wake();
    followText();
  });
}

function wakeChannelOnce() {
  if (!wakeChannel) wakeChannel = new BroadcastChannel(WAKE_CHANNEL);
  return wakeChannel;
}

// MARK: - The imports (the wasm's `./bunny.js` module)

// The glue paints this RGBA buffer onto the canvas, whole. `ImageData`
// refuses a shared view, so under shared memory the frame is copied
// once — the CPU road's price, and only its.
export function js_blit(pointer, width, height) {
  if (!host) return;
  const context = painter();
  if (paintCanvas.width !== width || paintCanvas.height !== height) {
    paintCanvas.width = width;
    paintCanvas.height = height;
  }
  const pixels = bytes(pointer, width * height * 4);
  const clamped = new Uint8ClampedArray(pixels.buffer, pixels.byteOffset, pixels.length);
  context.putImageData(new ImageData(clamped, width, height), 0, 0);
}

// The focused box copied: the text goes to the platform's clipboard.
// Called inside the stroke's own keydown, which is the user gesture the
// browser wants; a refusal (no permission, no focus) is silent.
export function js_clipboard_write(pointer, length) {
  if (!memory || typeof navigator === "undefined" || !navigator.clipboard) return;
  navigator.clipboard.writeText(text(pointer, length)).catch(() => {});
}

// The cursor the scene wants under the pointer — the shell's table:
// 0 arrow, 1 text, 2 pointing, 3 cell, 4 a seam left-right, 5 up-down,
// 6 a frame's side edge, 7 its top or bottom, 8 and 9 its corners.
const CURSORS = [
  "default", "text", "pointer", "cell", "col-resize", "row-resize",
  "ew-resize", "ns-resize", "nwse-resize", "nesw-resize",
];
export function js_set_cursor(kind) {
  if (!host) return;
  host.style.cursor = CURSORS[kind >>> 0] || "default";
}

export function js_request_frame() {
  // a worker never presents: the frame belongs to the page
  if (IN_WORKER) return;
  requestFrame();
}

// A task woke and asks for one turn. On the page: the microtask above.
// In a worker: the engine that must turn is the PAGE's, so the wake
// crosses on the channel the page listens to. (Every tab of the same
// origin hears it too; a spare wake is one settled frame, never a
// fault.)
// The page's clock, for the engine's stage timers.
export function js_now() {
  return performance.now();
}

export function js_request_wake() {
  if (IN_WORKER) {
    wakeChannelOnce().postMessage(0);
    return;
  }
  requestWake();
}

// Dom-mode imports — the single binary carries both shells, and this
// module only ever drives the canvas one.
export function js_apply_patches() {}
export function js_island() {}

// A panic on its way out of wasm: decode the message and log it, so an
// abort is a sentence instead of `unreachable` and a stack of numbers.
// A worker holds no memory handle here — its message stays in the
// memory the page holds, and the page's own hook says the rest.
export function js_panic(pointer, length) {
  if (!memory) {
    console.error("bunny panic (in a worker; the page's hook carries the message)");
    return;
  }
  console.error("bunny panic: " + text(pointer, length));
}

// The image edge: the engine hands the encoded bytes ONCE; the browser
// decodes off-thread and calls bunny_image_ready when the bitmap lands.
// Broken bytes park a null and never call back.
const images = new Map();

function imageKey(hi, lo) {
  // wasm hands u32 arguments through the SIGNED i32 border while the
  // patch decoder reads unsigned — normalize or the same key differs
  return `${hi >>> 0}:${lo >>> 0}`;
}

export function js_image_register(hi, lo, pointer, length) {
  if (!memory) return;
  const key = imageKey(hi, lo);
  // `slice` copies — the bytes outlive any growth, shared or not
  const copy = new Uint8Array(memory.buffer, pointer >>> 0, length >>> 0).slice();
  createImageBitmap(new Blob([copy]))
    .then((bitmap) => {
      images.set(key, bitmap);
      if (wasm) wasm.bunny_image_ready(hi, lo);
    })
    .catch(() => {
      images.set(key, null);
    });
}

// Writes [width, height] as two u32 at `out`; [0, 0] = not decoded.
export function js_image_size(hi, lo, out) {
  if (!memory) return;
  const bitmap = images.get(imageKey(hi, lo));
  const view = new Uint32Array(memory.buffer, out >>> 0, 2);
  view[0] = bitmap ? bitmap.width : 0;
  view[1] = bitmap ? bitmap.height : 0;
}

// Draws the bitmap at exactly width×height physical px and writes the
// straight-alpha RGBA back (getImageData is straight by spec).
export function js_image_raster(hi, lo, width, height, out) {
  if (!host) return;
  const bitmap = images.get(imageKey(hi, lo));
  if (!bitmap) return;
  const ink = inkSurface(width, height);
  ink.setTransform(1, 0, 0, 1, 0, 0);
  ink.clearRect(0, 0, width, height);
  ink.imageSmoothingEnabled = true;
  ink.imageSmoothingQuality = "high";
  ink.drawImage(bitmap, 0, 0, width, height);
  const pixels = ink.getImageData(0, 0, width, height).data;
  new Uint8Array(memory.buffer, out >>> 0, width * height * 4).set(pixels);
}

// Writes [width, ascent, descent] as three f64 at `out` — logical px.
// Ascent/descent come from the FONT's bounding box (stable per font);
// an empty string keeps the metrics and reports width 0.
export function js_measure_text(
  pointer, length, size, weight, mono, italic, familyPointer, familyLength, out,
) {
  if (!host) return;
  const line = text(pointer, length);
  const ink = inkSurface(1, 1);
  ink.font = cssFont(size, weight, mono, italic, text(familyPointer, familyLength) || null);
  const probe = ink.measureText(line || "Mg");
  const metrics = new Float64Array(memory.buffer, out >>> 0, 3);
  metrics[0] = line ? probe.width : 0;
  metrics[1] = probe.fontBoundingBoxAscent ?? size * 0.8;
  metrics[2] = probe.fontBoundingBoxDescent ?? size * 0.25;
}

// Draws one line into a width×height physical rectangle and copies the
// RGBA into wasm memory at `out`. getImageData hands back straight
// alpha — the compositor's contract, no conversion here.
export function js_raster_text(
  pointer, length, size, weight, mono, italic, familyPointer, familyLength,
  scale, width, height, descent, color, out,
) {
  if (!host) return;
  const line = text(pointer, length);
  const ink = inkSurface(width, height);
  ink.setTransform(1, 0, 0, 1, 0, 0);
  ink.clearRect(0, 0, width, height);
  ink.setTransform(scale, 0, 0, scale, 0, 0);
  ink.font = cssFont(size, weight, mono, italic, text(familyPointer, familyLength) || null);
  ink.textBaseline = "alphabetic";
  const r = (color >>> 24) & 0xff;
  const g = (color >>> 16) & 0xff;
  const b = (color >>> 8) & 0xff;
  const a = color & 0xff;
  ink.fillStyle = `rgba(${r}, ${g}, ${b}, ${a / 255})`;
  // baseline sits `descent` above the box bottom — the ceil slack stays
  // on top, mirroring the desktop engine
  ink.fillText(line, 0, height / scale - descent);
  const pixels = ink.getImageData(0, 0, width, height).data;
  new Uint8Array(memory.buffer, out >>> 0, width * height * 4).set(pixels);
}

// MARK: - The page's side: events into the exports

// The engine's key table, mirrored (bunny_ui_web::named_key).
const KEYS = {
  Backspace: 1,
  Delete: 2,
  ArrowLeft: 3,
  ArrowRight: 4,
  Home: 5,
  End: 6,
  Escape: 7,
  ArrowUp: 8,
  ArrowDown: 9,
  Enter: 10,
  Tab: 11,
  PageUp: 12,
  PageDown: 13,
};

// The keys a field asks a software keyboard for, as the editable's
// `inputmode` — `bunny_caret_keyboard` answers an index into this table
// (bunny_ui_web::keyboard_code).
const INPUT_MODES = ["text", "email", "numeric", "decimal", "tel", "url", "search"];

// A trackpad's pinch, said by the browser as a turn of the wheel with
// control held: Chromium makes the turn −100·ln(ratio) pixels, so the
// ratio of the span comes back out of it. A mouse's notch is a line or a
// page, made pixels first, and no single turn is taken for more than
// doubling or halving.
const pinchRatio = (event) => {
  const unit = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? 400 : 1;
  return Math.min(2, Math.max(0.5, Math.exp((-event.deltaY * unit) / 100)));
};

// The function row: `F1` to `F24`, sent as 101 to 124.
const FUNCTION_KEY = /^F([1-9]|1[0-9]|2[0-4])$/;

// The keys whose going down or coming up is itself the news.
const MODIFIER_KEYS = new Set(["Shift", "Meta", "Control", "Alt"]);

// Set when the page attaches: moves the composition editable to wherever
// the engine's keyboard is taking text (see the composition road).
let followText = () => {};

// 1 shift, 2 command, 4 option, 8 control — the engine's bits. The
// COMMAND bit is the platform's primary modifier, the way the Windows
// shell hands Ctrl to the engine as `command`: ⌘ on a Mac, Ctrl
// elsewhere. The other one is `control`.
function modifiers(event) {
  const primary = primaryIsMeta ? event.metaKey : event.ctrlKey;
  const secondary = primaryIsMeta ? event.ctrlKey : event.metaKey;
  return (
    (event.shiftKey ? 1 : 0) |
    (primary ? 2 : 0) |
    (event.altKey ? 4 : 0) |
    (secondary ? 8 : 0)
  );
}

// The engine names a key by what it types with NO modifier — so a
// chord on shifted punctuation is spellable at all. `event.key` has the
// shift APPLIED, so the chord road asks the keyboard layout instead.
// getLayoutMap reads the USER'S OWN layout, which is the only correct
// answer; where the browser has no such map the base falls back to the
// typed character.
let layoutMap = null;

function baseChar(event) {
  const base = layoutMap && event.code ? layoutMap.get(event.code) : undefined;
  return base && base.length === 1 ? base : event.key;
}

function sendText(value) {
  const encoded = new TextEncoder().encode(value);
  const pointer = wasm.bunny_alloc(encoded.length);
  new Uint8Array(memory.buffer, pointer >>> 0, encoded.length).set(encoded);
  wasm.bunny_text(pointer, encoded.length);
}

// The host's box in CSS px and the device ratio — what `bunny_resize`
// takes; the shell multiplies the two itself.
let lastBox = [0, 0, 0];
function resize() {
  const rect = host.getBoundingClientRect();
  const scale = window.devicePixelRatio || 1;
  if (rect.width === lastBox[0] && rect.height === lastBox[1] && scale === lastBox[2]) return;
  lastBox = [rect.width, rect.height, scale];
  wasm.bunny_resize(rect.width, rect.height, scale);
}

// The device ratio changes when a window crosses screens: a media
// query pinned to the CURRENT ratio fires once when it stops matching,
// and is re-armed against the new one.
function watchScale() {
  const query = matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`);
  query.addEventListener(
    "change",
    () => {
      resize();
      watchScale();
    },
    { once: true },
  );
}

// Boots the shell into `hostElement`.
//
// `memoryHandle` is the instance's memory (imported or exported — the
// page knows which); `exports` its exports; `start(width, height,
// scale)` the app's own entry, which the page exported through its
// loader and which reaches `bunny_ui_web::start_with`. Resolves to
// `true` once the shell runs, `false` on an ABI the mirror was not
// written for — in which case the host says so and nothing else runs.
export async function attach(memoryHandle, exports, hostElement, start) {
  memory = memoryHandle || exports.memory;
  wasm = exports;
  host = hostElement;
  shared = typeof SharedArrayBuffer !== "undefined" && memory.buffer instanceof SharedArrayBuffer;
  primaryIsMeta = /Mac|iPhone|iPad|iPod/.test(navigator.platform || "");
  attachGpu(exports, memory, surface);

  // the ABI gate: a missing export counts as version 0
  const abi = wasm.bunny_abi_version ? wasm.bunny_abi_version() >>> 0 : 0;
  if (abi !== EXPECTED_ABI) {
    const notice = document.createElement("pre");
    notice.textContent =
      `This page speaks ABI ${EXPECTED_ABI}. ` +
      `The wasm speaks ABI ${abi}. ` +
      `Deploy the page and the wasm together, then reload.`;
    host.replaceChildren(notice);
    wasm = null;
    return false;
  }

  if (navigator.keyboard && navigator.keyboard.getLayoutMap) {
    navigator.keyboard.getLayoutMap().then((map) => {
      layoutMap = map;
    });
  }
  // a wake from a worker crosses here (rule 3)
  wakeChannelOnce().onmessage = () => requestWake();

  // the faces the page declared: the first measure must see them, or
  // the shell caches a fallback's widths under the real name
  if (document.fonts && document.fonts.ready) {
    await document.fonts.ready;
  }

  // `?present=gpu`: the GPU tier takes the page whatever its own probe of
  // the device says (and `?present=cpu` keeps it off the page)
  if (new URLSearchParams(location.search).get("present") === "gpu" && wasm.bunny_gpu_forced) {
    wasm.bunny_gpu_forced();
  }
  const scale = window.devicePixelRatio || 1;
  const rect = host.getBoundingClientRect();
  lastBox = [rect.width, rect.height, scale];
  start(rect.width, rect.height, scale);
  // whatever the engine asked for while the page was still attaching
  if (pendingWake) {
    pendingWake = false;
    requestWake();
  }
  if (pendingFrame) {
    pendingFrame = false;
    requestFrame();
  }

  // Is motion welcome? The PLATFORM answers. This shell drives every
  // animation itself, so the preference silences springs and clocks
  // alike — and a reader who changes their mind is heard at once.
  if (wasm.bunny_set_motion) {
    const query = matchMedia("(prefers-reduced-motion: reduce)");
    wasm.bunny_set_motion(query.matches ? 0 : 1);
    query.addEventListener("change", (event) => {
      if (wasm) wasm.bunny_set_motion(event.matches ? 0 : 1);
    });
  }

  // the box follows the page: the host's own size, and the screen's ratio
  new ResizeObserver(() => resize()).observe(host);
  watchScale();

  // A finger on the host is the scene's whole: no pan or zoom of the
  // browser's, no callout, no selection, no tap flash — a drag reaches
  // the recognizer instead of moving the page.
  host.style.touchAction = "none";
  host.style.userSelect = "none";
  host.style.setProperty("-webkit-user-select", "none");
  host.style.setProperty("-webkit-touch-callout", "none");
  host.style.setProperty("-webkit-tap-highlight-color", "transparent");

  const point = (event) => {
    const box = host.getBoundingClientRect();
    return [event.clientX - box.left, event.clientY - box.top];
  };
  // the tooltip's slow clock: two beats after the pointer settles —
  // the first ages the wait, the second shows. The engine no-ops the
  // strays, so the glue never has to know whether one is pending.
  let tooltipBeats = [];
  const armTooltip = () => {
    tooltipBeats.forEach(clearTimeout);
    tooltipBeats = [
      setTimeout(() => wasm.bunny_tooltip_tick(), 360),
      setTimeout(() => wasm.bunny_tooltip_tick(), 720),
    ];
  };
  // The finger's road. Down the pointer's, a finger was a mouse that
  // drags — a drag over a list swept a selection and the list never
  // moved. The engine's recognizer, the one the phone shells feed,
  // decides what a touch meant instead: a tap, a pan that slides the
  // content and flings on after the lift, a hold that opens a menu. A
  // wasm without the door keeps the pointer's road for every pointer.
  const fingers = new Set();
  // where the mouse last stood — a trackpad's pinch zooms around it
  let lastPoint = [0, 0];
  // Safari's pinch in flight: the `scale` its last gesture event said, 0
  // when none is
  let pinchScale = 0;
  let fingerLifted = -Infinity;
  // did the last press come from a finger? its keyboard is a phone's
  let byFinger = false;
  const touch = (event, phase) => {
    if (event.pointerType !== "touch" || !wasm.bunny_touch) return false;
    if (phase === 0) fingers.add(event.pointerId);
    if (phase > 1) {
      fingers.delete(event.pointerId);
      fingerLifted = event.timeStamp;
    }
    const [x, y] = point(event);
    wasm.bunny_touch(phase, event.pointerId >>> 0, x, y, event.timeStamp, event.isPrimary ? 1 : 0);
    return true;
  };
  host.addEventListener("pointermove", (event) => {
    if (touch(event, 1)) return;
    const [x, y] = point(event);
    lastPoint = [x, y];
    wasm.bunny_pointer_move(x, y, modifiers(event));
    armTooltip();
  });
  host.addEventListener("pointerdown", (event) => {
    byFinger = event.pointerType === "touch";
    // a finger's keyboard comes with its tap's click (see below)
    if (touch(event, 0)) return;
    const [x, y] = point(event);
    // the middle press is the scene's: no autoscroll over a canvas
    if (event.button === 1) event.preventDefault();
    // `pointerdown` reports detail 0 — the browser only counts on
    // `mousedown`, and this door stays on pointer events so touch and
    // pen keep working. The shell counts, from the event's own
    // timestamp and the button it came from.
    wasm.bunny_pointer_down(x, y, event.timeStamp, event.button, modifiers(event));
    followText();
  });
  host.addEventListener("contextmenu", (event) => {
    // the scene offers its own menu — the browser's stays home
    event.preventDefault();
    // and a finger's hold IS the recognizer's menu: the browser's word
    // for the same hold would open it a second time
    const fromFinger =
      fingers.size > 0 || event.pointerType === "touch" || event.timeStamp - fingerLifted < 800;
    if (wasm.bunny_touch && fromFinger) return;
    const [x, y] = point(event);
    wasm.bunny_context_click(x, y, modifiers(event));
  });
  host.addEventListener("pointerup", (event) => {
    if (touch(event, 2)) return;
    const [x, y] = point(event);
    wasm.bunny_pointer_up(x, y);
    followText();
  });
  // the browser took the finger (a gesture of the system's): a press in
  // flight fires nothing
  host.addEventListener("pointercancel", (event) => touch(event, 3));
  // The keyboard a finger raises comes with its tap's `click`. A phone
  // opens the keys only for a focus taken inside a gesture it trusts, and
  // the click it makes of a tap is the one every phone trusts: an iPhone
  // grants nothing to a `touchend`, which it may dispatch outside the tap.
  // A press the recognizer decided at the lift (a field in a scroll
  // region) has put the keyboard on its field by then too.
  host.addEventListener("click", () => {
    if (byFinger || fingerFirst()) summon();
  });
  // Before its click the tap hands the browser a compatibility `mousedown`,
  // whose default focused the host and took the focus from the editable —
  // the keys rose and fell at once. While the keyboard is taking text, the
  // default goes; a press of the mouse keeps the editable's focus the same
  // way.
  host.addEventListener("mousedown", (event) => {
    if (document.activeElement === ime || takesText()) event.preventDefault();
  });
  host.addEventListener(
    "wheel",
    (event) => {
      event.preventDefault();
      const [x, y] = point(event);
      // A trackpad's pinch comes as a turn with control held: a box that
      // zooms takes it as the pinch it is, and a turn nobody zoomed goes on
      // as the wheel it always was. Inside Safari's own pinch (below) the
      // turn is that gesture's, never a second zoom.
      if (event.ctrlKey && wasm.bunny_magnify) {
        if (pinchScale) return;
        if (wasm.bunny_magnify(x, y, pinchRatio(event))) return;
      }
      wasm.bunny_wheel(x, y, event.deltaX, event.deltaY, modifiers(event));
      // the same two beats end the scroll gesture: the region that
      // took the wheel keeps it until they land
      armTooltip();
    },
    { passive: false },
  );
  // Safari says a trackpad's pinch with gesture events of its own, the
  // `scale` counted from the gesture's start, and zooms the page unless the
  // default goes. A phone's two fingers make them too: those are the
  // fingers' (`bunny_touch`), so a gesture that begins with a finger down is
  // left alone.
  host.addEventListener("gesturestart", (event) => {
    if (fingers.size > 0 || !wasm.bunny_magnify) return;
    event.preventDefault();
    pinchScale = 1;
  });
  host.addEventListener("gesturechange", (event) => {
    if (!pinchScale) return;
    event.preventDefault();
    const scale = event.scale > 0 ? event.scale : pinchScale;
    const ratio = scale / pinchScale;
    pinchScale = scale;
    const [x, y] = Number.isFinite(event.clientX) ? point(event) : lastPoint;
    if (ratio !== 1) wasm.bunny_magnify(x, y, ratio);
  });
  host.addEventListener("gestureend", (event) => {
    if (!pinchScale) return;
    event.preventDefault();
    pinchScale = 0;
  });
  // The composition road. Canvas mode owns no editable element, so an input
  // method had nothing to compose into: Latin typing, dead keys and AltGr
  // arrive by `keydown`, and a composition — every CJK reader's — produced
  // nothing at all. A hidden editable is the input method's target while the
  // engine's keyboard takes text: it stands under the caret, so the
  // candidates open there, and hands the composition over as marked text and
  // its commit as text. It lives on the BODY, never in the host, whose
  // children are the drawing surface and are replaced whole.
  const ime = document.createElement("textarea");
  ime.setAttribute("aria-hidden", "true");
  ime.setAttribute("autocomplete", "off");
  ime.setAttribute("autocorrect", "off");
  ime.setAttribute("autocapitalize", "off");
  ime.spellcheck = false;
  ime.tabIndex = -1;
  // Sixteen pixels: iOS zooms the page into an editable whose text is
  // smaller, and a page zoomed by a focus stays zoomed. The ink is
  // transparent too — iOS draws its caret over an invisible element.
  ime.style.cssText =
    "position:fixed;left:0;top:0;width:1px;height:16px;padding:0;margin:0;border:0;" +
    "font-size:16px;color:transparent;caret-color:transparent;background:transparent;" +
    "opacity:0;resize:none;overflow:hidden;pointer-events:none;white-space:pre;z-index:-1";
  document.body.appendChild(ime);
  let composing = false;
  // Did a tap focus the editable, so a keyboard rose with it? A focus
  // taken any other way raised none.
  let granted = false;
  // the blur in flight is ours, never the reader's
  let releasing = false;
  const release = () => {
    releasing = true;
    ime.blur();
    releasing = false;
  };
  // Is the engine's keyboard taking text? (The caret it would type at is
  // then in `bunny_caret_x`, `_y` and `_height`.)
  const takesText = () => !!(wasm && wasm.bunny_text_caret && wasm.bunny_text_caret());
  // Does a finger drive this device? Its keys rise only for a tap.
  const fingerFirst = () => matchMedia("(pointer: coarse)").matches;
  // The editable stands under the caret, so an input method's candidates
  // open there. A finger's keyboard is a phone's, whose candidates ride the
  // keys, and the browser scrolls the whole page to reveal an editable the
  // keys would cover — the engine already lifts the field above them, so
  // the page moved twice. There the editable stands at the top.
  const place = () => {
    // the keys the field asks for: a phone lays out an `@` for an address
    // and a pad for a code, and lays them out again when the keyboard
    // moves to a field of another kind while it is up
    const mode = INPUT_MODES[wasm.bunny_caret_keyboard ? wasm.bunny_caret_keyboard() : 0] || "text";
    if (ime.inputMode !== mode) ime.inputMode = mode;
    const box = host.getBoundingClientRect();
    ime.style.left = `${box.left + wasm.bunny_caret_x()}px`;
    const top = byFinger ? Math.max(0, box.top) : box.top + wasm.bunny_caret_y();
    ime.style.top = `${top}px`;
    ime.style.height = `${Math.max(1, wasm.bunny_caret_height())}px`;
  };
  // The editable follows the keyboard: focused, under the caret, exactly
  // while the engine takes text — asked after anything that could move it.
  // Not on a device a finger drives: a focus no tap asked for raises no
  // keys there, and it strands the next tap's — an iPhone raises nothing
  // for a focus that lands where the focus already is. There the editable
  // waits for the finger's tap (`summon`).
  followText = () => {
    if (!takesText()) {
      if (document.activeElement === ime && !composing) release();
      return;
    }
    place();
    if (document.activeElement !== ime && !fingerFirst()) {
      ime.focus({ preventScroll: true });
      granted = false;
    }
  };
  // The tap's own focus — the one place a phone raises the keys.
  const summon = () => {
    if (!takesText()) {
      if (document.activeElement === ime && !composing) release();
      return;
    }
    place();
    if (document.activeElement !== ime) ime.focus({ preventScroll: true });
    granted = true;
  };
  // The reader put the keys away while a field held them: iOS's Done
  // blurs the editable, and Android's back key hides the keys and leaves
  // it focused. Either way the field lets the keyboard go, as on the
  // phones — kept, it would raise the keys again at the next tap
  // anywhere. Only a keyboard a finger raised is put away: a page that
  // lost the focus whole (another tab, another window) and a desktop's
  // editable keep the field as it was.
  const dismiss = () => {
    if (wasm.bunny_keyboard_dismissed && wasm.bunny_text_caret()) wasm.bunny_keyboard_dismissed();
  };
  ime.addEventListener("blur", () => {
    const raised = granted;
    granted = false;
    measureKeys();
    if (raised && !releasing && !composing && document.hasFocus()) dismiss();
  });
  ime.addEventListener("focus", () => {
    restHeight = host.getBoundingClientRect().height;
    measureKeys();
  });
  // The keys' overlap. The browser shrinks its VISUAL viewport under the
  // keys and leaves the page its size, so the overlap is what the visual
  // viewport lost above the host's bottom (or the page's, where the host
  // runs past it) — told to the engine, whose root lays itself out above
  // the keys. Only while the editable holds the focus, and unzoomed: a
  // pinch shrinks the visual viewport too, and is no keyboard.
  const keys = window.visualViewport;
  // less than this is a toolbar settling, not a keyboard
  const KEYS_MIN = 80;
  let overlap = 0;
  // A browser that resizes the PAGE under the keys instead (Firefox, an
  // older Chrome) shrinks the host itself, and the scene follows it with
  // no overlap to tell; the keys are there when the host lost their height
  // since the editable took the focus.
  let restHeight = 0;
  let keysUp = false;
  function measureKeys() {
    if (!keys || !wasm.bunny_keyboard) return;
    const typing = document.activeElement === ime && Math.abs(keys.scale - 1) < 0.01;
    const box = host.getBoundingClientRect();
    // the layout viewport's own bottom: some browsers shrink `innerHeight`
    // with the keys, never the initial containing block
    const page = Math.max(window.innerHeight, document.documentElement.clientHeight);
    const bottom = Math.min(box.bottom, page);
    const now = typing ? Math.max(0, Math.round(bottom - (keys.offsetTop + keys.height))) : 0;
    const up = typing && (now >= KEYS_MIN || restHeight - box.height >= KEYS_MIN);
    // the keys went down under a focused editable: Android's back key
    const fell = keysUp && !up && typing && granted;
    keysUp = up;
    if (now !== overlap) {
      overlap = now;
      wasm.bunny_keyboard(now);
    }
    if (fell) {
      release();
      dismiss();
    }
  }
  if (keys) {
    keys.addEventListener("resize", measureKeys);
    keys.addEventListener("scroll", measureKeys);
  }
  const sendMarked = (value) => {
    if (!wasm.bunny_marked) return;
    const encoded = new TextEncoder().encode(value);
    const pointer = wasm.bunny_alloc(encoded.length);
    new Uint8Array(memory.buffer, pointer >>> 0, encoded.length).set(encoded);
    wasm.bunny_marked(pointer, encoded.length);
  };
  ime.addEventListener("compositionstart", () => {
    composing = true;
  });
  ime.addEventListener("compositionupdate", (event) => sendMarked(event.data || ""));
  ime.addEventListener("compositionend", (event) => {
    composing = false;
    // the commit replaces what is marked; an empty one cancels the mark
    if (event.data) sendText(event.data);
    else sendMarked("");
    ime.value = "";
    followText();
  });
  // what arrives by `input` and never by `keydown`: a driver's insertText, a
  // keyboard that types without keys. A composition's own input events are
  // the composition's, and it already spoke for them
  ime.addEventListener("input", (event) => {
    if (composing || event.isComposing) return;
    if (event.inputType === "insertText" && event.data) sendText(event.data);
    ime.value = "";
  });

  // a modifier's release types nothing and makes no stroke: the
  // state it leaves is the whole event
  window.addEventListener("keyup", (event) => {
    if (MODIFIER_KEYS.has(event.key) && wasm && wasm.bunny_modifiers) {
      wasm.bunny_modifiers(modifiers(event));
    }
  });
  window.addEventListener("keydown", (event) => {
    // a key the input method is composing with is the input method's:
    // it commits, walks the candidates, cancels — none of it is ours
    if (event.isComposing || event.keyCode === 229) return;
    const mods = modifiers(event);
    if (MODIFIER_KEYS.has(event.key)) {
      if (wasm.bunny_modifiers) wasm.bunny_modifiers(mods);
      return;
    }
    // the function row is the browser's as much as the page's — F5
    // reloads, F12 opens the tools — so its default goes only when
    // the app took the key
    const row = FUNCTION_KEY.exec(event.key);
    if (row) {
      if (wasm.bunny_key(100 + Number(row[1]), mods)) event.preventDefault();
      return;
    }
    const code = KEYS[event.key];
    if (code !== undefined) {
      event.preventDefault();
      wasm.bunny_key(code, mods);
      return;
    }
    if (event.key.length !== 1) return;
    // the paste chord keeps its default: the `paste` event below IS the
    // clipboard read, and a prevented keydown never fires it
    const paste = (mods & 2) !== 0 && event.key.toLowerCase() === "v";
    if (!paste) event.preventDefault();
    // a command stroke is a stroke; a bare character is TEXT, so typing
    // takes the same road a paste and a composition take
    if (event.metaKey || event.ctrlKey) {
      wasm.bunny_key_char(baseChar(event).codePointAt(0), mods);
    } else {
      sendText(event.key);
    }
  });
  // after every key — a Tab, an Escape, a binding may have moved the keyboard
  window.addEventListener("keydown", () => followText());
  // The paste road: a page cannot read the clipboard on a keystroke,
  // so the browser's own `paste` event carries the text — through the
  // same door typing takes.
  window.addEventListener("paste", (event) => {
    const value = event.clipboardData ? event.clipboardData.getData("text/plain") : "";
    event.preventDefault();
    if (value) sendText(value);
  });
  return true;
}
