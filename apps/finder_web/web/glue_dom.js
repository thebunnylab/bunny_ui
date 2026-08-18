// The Dom glue: the browser side of the SECOND rendering. The engine
// lowers the semantic scene to a patch stream (fixed little-endian ABI)
// and this file mutates real elements with it. Text selects, scroll
// carries momentum, the input owns the editing — the browser at home.

const app = document.getElementById("app");
// the root element IS scene node 0 — its backdrop arrives through the
// patches (the theme's canvas), like every other color here
app.dataset.n = "0";
const sheet = document.createElement("style");
document.head.appendChild(sheet);

// The engine names a key by what it types with NO modifier, so a chord
// on shifted punctuation is spellable at all. `event.key` has the shift
// APPLIED (shift and backslash arrives as a pipe), so the chord road
// asks the keyboard layout — the USER'S own, never a table of US pairs,
// which would be wrong on a Brazilian keyboard. Where the browser has
// no such map the base falls back to the typed character: today's
// behaviour, and never a crash.
let layoutMap = null;
if (navigator.keyboard && navigator.keyboard.getLayoutMap) {
  navigator.keyboard.getLayoutMap().then((map) => {
    layoutMap = map;
  });
}

function baseChar(event) {
  const base = layoutMap && event.code ? layoutMap.get(event.code) : undefined;
  return base && base.length === 1 ? base : event.key;
}

// the tooltip is the browser's in this mode, like the hover and the
// inputs: a data attribute, a static rule, a transition-delay for the
// wait — zero patches by construction. The bubble mirrors the engine's
// (ink ground, canvas text, radius 5) without a wire round trip.
const tooltipSheet = document.createElement("style");
tooltipSheet.textContent = [
  '[data-tip]::after{content:attr(data-tip);position:absolute;',
  'left:50%;top:100%;transform:translate(-50%,6px);z-index:9;',
  'background:rgba(32,37,49,0.95);color:#F5F6FA;padding:3px 7px;',
  'border-radius:5px;font:11px -apple-system,system-ui,sans-serif;',
  'white-space:nowrap;pointer-events:none;opacity:0;',
  'box-shadow:0 2px 10px rgba(0,0,0,0.35)}',
  '[data-tip]:hover::after{opacity:1;transition:opacity .1s linear .7s}',
].join("");
document.head.appendChild(tooltipSheet);

let wasm = null;
let wakeArmed = false;
const decoder = new TextDecoder();

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

// 1 shift, 2 command, 4 option, 8 control — the engine's bits.
function modifiers(event) {
  return (
    (event.shiftKey ? 1 : 0) |
    (event.metaKey ? 2 : 0) |
    (event.altKey ? 4 : 0) |
    (event.ctrlKey ? 8 : 0)
  );
}

// Text into the engine: one allocation, owned by the engine after the
// call (the same door the canvas mode types through).
function sendText(text) {
  const bytes = new TextEncoder().encode(text);
  const pointer = wasm.bunny_alloc(bytes.length);
  new Uint8Array(wasm.memory.buffer, pointer, bytes.length).set(bytes);
  wasm.bunny_text(pointer, bytes.length);
}
const elements = new Map([[0, app]]);
const rules = new Map();

// Registered images by split key ("hi:lo"): a blob URL the <img>
// elements load from, plus the decoded size once the probe lands
// (width 0 = still decoding; a broken blob never reports and its
// element simply stays empty).
const images = new Map();

function imageKey(hi, lo) {
  // wasm hands u32 arguments through the SIGNED i32 border while the
  // patch decoder reads unsigned — normalize or the same key differs
  return `${hi >>> 0}:${lo >>> 0}`;
}

// The engine measures text through the SAME canvas engine in this mode
// (layout is always ours) — only the raster never runs: no bitmap here.
let inkCanvas = null;
let inkContext = null;

function inkSurface(width, height) {
  if (!inkCanvas) {
    inkCanvas = document.createElement("canvas");
    inkCanvas.width = 256;
    inkCanvas.height = 64;
    inkContext = inkCanvas.getContext("2d", { willReadFrequently: true });
  }
  if (width && inkCanvas.width < width) inkCanvas.width = width;
  if (height && inkCanvas.height < height) inkCanvas.height = height;
  return inkContext;
}

function cssFont(size, weight, mono, italic) {
  const family = mono
    ? 'ui-monospace, Menlo, Consolas, monospace'
    : 'system-ui, -apple-system, "Segoe UI", sans-serif';
  // CSS order is style, then weight, then size — a leaning label is a
  // real face, never a skew we paint ourselves
  return `${italic ? "italic " : ""}${weight} ${size}px ${family}`;
}

const CSS_WEIGHTS = [400, 500, 600, 700];

function rgba(packed) {
  const r = (packed >>> 24) & 0xff;
  const g = (packed >>> 16) & 0xff;
  const b = (packed >>> 8) & 0xff;
  const a = packed & 0xff;
  return `rgba(${r}, ${g}, ${b}, ${a / 255})`;
}

function flushRules() {
  sheet.textContent = [...rules.values()].join("\n");
}

function createElementOf(kind) {
  // 0 group, 1 box, 2 text, 3 field, 4 scroll, 5 content, 6 canvas,
  // 7 image
  if (kind === 6) {
    const canvas = document.createElement("canvas");
    canvas.style.cssText = "position:absolute;left:0;top:0;";
    return canvas;
  }
  if (kind === 7) {
    const img = document.createElement("img");
    // the box underneath owns the clicks; our geometry owns the frame
    img.style.cssText = "position:absolute;left:0;top:0;pointer-events:none;";
    img.draggable = false;
    return img;
  }
  if (kind === 8) {
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    // the viewBox mirrors the engine's ICON_GRID; the default
    // preserveAspectRatio (xMidYMid meet) is the SAME centred square
    // the rasterizers paint
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.style.cssText = "position:absolute;left:0;top:0;pointer-events:none;";
    return svg;
  }
  if (kind === 3) {
    const input = document.createElement("input");
    input.type = "text";
    // padding mirrors the engine's FIELD_PAD; every color and border
    // arrives through the patches — the theme owns the chrome, and no
    // inline border may outrank the stylesheet rule
    input.style.cssText =
      "position:absolute;left:0;top:0;box-sizing:border-box;" +
      "padding:5px 8px;outline:none;";
    let composing = false;
    input.addEventListener("compositionstart", () => {
      composing = true;
    });
    input.addEventListener("compositionend", () => {
      composing = false;
      report(input);
    });
    input.addEventListener("input", () => {
      // NEVER during a live composition — the browser owns that dance
      if (!composing) report(input);
    });
    function report(input) {
      const path = input.dataset.path ?? "";
      sendField(path, input.value, input.selectionStart ?? input.value.length);
    }
    return input;
  }
  const el = document.createElement("div");
  el.style.cssText = "position:absolute;left:0;top:0;box-sizing:border-box;";
  if (kind === 2) {
    el.style.whiteSpace = "pre";
    el.style.cursor = "default";
  }
  if (kind === 4) {
    el.style.overflow = "auto";
    el.style.scrollBehavior = "smooth";
  }
  return el;
}

function applyPatches(view, length) {
  let at = 0;
  const u8 = () => view.getUint8(at++);
  const u16 = () => {
    const value = view.getUint16(at, true);
    at += 2;
    return value;
  };
  const u32 = () => {
    const value = view.getUint32(at, true);
    at += 4;
    return value;
  };
  const f32 = () => {
    const value = view.getFloat32(at, true);
    at += 4;
    return value;
  };
  const bytes = (count) => {
    const slice = new Uint8Array(view.buffer, view.byteOffset + at, count);
    at += count;
    return slice;
  };
  const text = (count) => decoder.decode(bytes(count));

  let rulesTouched = false;
  const count = u32();
  for (let i = 0; i < count; i++) {
    const op = u8();
    const id = u32();
    if (op === 1) {
      const parent = u32();
      const kind = u8();
      const el = createElementOf(kind);
      el.dataset.n = id;
      if (kind === 4) {
        el.addEventListener("scroll", () => {
          wasm.bunny_dom_scroll(id, el.scrollLeft, el.scrollTop);
        });
      }
      elements.get(parent)?.appendChild(el);
      elements.set(id, el);
    } else if (op === 2) {
      const el = elements.get(id);
      el?.remove();
      elements.delete(id);
      rules.delete(id);
      rulesTouched = true;
    } else if (op === 3) {
      const el = elements.get(id);
      const x = f32();
      const y = f32();
      if (el) el.style.transform = `translate(${x}px, ${y}px)`;
    } else if (op === 4) {
      const el = elements.get(id);
      const width = f32();
      const height = f32();
      if (el) {
        el.style.width = `${width}px`;
        el.style.height = `${height}px`;
        if (el.tagName === "DIV" && el.style.whiteSpace === "pre") {
          el.style.lineHeight = `${height}px`;
        }
        if (el.tagName === "CANVAS") {
          // backing store in physical px; the island blit matches
          const dpr = Math.max(1, Math.round(window.devicePixelRatio || 1));
          el.width = Math.max(1, Math.round(width * dpr));
          el.height = Math.max(1, Math.round(height * dpr));
        }
      }
    } else if (op === 5) {
      const el = elements.get(id);
      const mask = u16();
      const base = [];
      let hover = null;
      let pressed = null;
      if (mask & 1) base.push(`background:${rgba(u32())}`);
      if (mask & 2) hover = rgba(u32());
      if (mask & 4) pressed = rgba(u32());
      if (mask & 8) {
        const borderColor = u32();
        const borderWidth = f32();
        base.push(`border:${borderWidth}px solid ${rgba(borderColor)}`);
      }
      if (mask & 16) base.push(`border-radius:${f32()}px`);
      if (mask & 32) {
        const radius = f32();
        base.push(`box-shadow:0 0 ${radius}px ${rgba(u32())}`);
      }
      if (mask & 64) {
        const response = f32();
        f32(); // damping — the CSS side keeps the duration
        base.push(
          `transition:background-color ${response}s ease-out,` +
            ` transform ${response}s ease-out`,
        );
      }
      if (mask & 128) {
        const path = text(u16());
        if (el) {
          el.dataset.path = path;
          el.style.cursor = "default";
        }
      }
      let focus = null;
      let placeholder = null;
      if (mask & 256) focus = rgba(u32());
      if (mask & 512) placeholder = rgba(u32());
      // the ink the subtree INHERITS: the text below sets no color of
      // its own, so these two rules flip the whole box at once
      let hoverInk = null;
      let pressedInk = null;
      if (mask & 1024) base.push(`color:${rgba(u32())}`);
      if (mask & 2048) hoverInk = rgba(u32());
      if (mask & 4096) pressedInk = rgba(u32());
      // a two-stop ramp: the geometry is the engine's, the pixels are
      // the browser's (background-image sits OVER the flat background)
      if (mask & 8192) {
        const kind = u8();
        const [a, b, c, d] = [f32(), f32(), f32(), f32()];
        const aspect = kind === 0 ? f32() : 1;
        const near = rgba(u32());
        const far = rgba(u32());
        if (kind === 0 && aspect !== 1 && d > 0) {
          // the ellipse: X radius on the wire, Y is X times the aspect
          base.push(
            `background-image:radial-gradient(ellipse ${d}px ${d * aspect}px at ` +
              `${a * 100}% ${b * 100}%, ${near} ${((c / d) * 100).toFixed(2)}%, ${far} 100%)`,
          );
        } else if (kind === 0) {
          const reach = d < 0 ? "farthest-corner" : `${d}px`;
          const stop = d < 0 ? "100%" : `${d}px`;
          base.push(
            `background-image:radial-gradient(circle ${reach} at ` +
              `${a * 100}% ${b * 100}%, ${near} ${c}px, ${far} ${stop})`,
          );
        } else {
          // CSS runs its line through the centre: the angle carries the
          // direction (0deg points up, clockwise)
          const degrees = (Math.atan2(c - a, -(d - b)) * 180) / Math.PI;
          base.push(
            `background-image:linear-gradient(${degrees.toFixed(2)}deg, ${near}, ${far})`,
          );
        }
      }
      if (mask & 16384) {
        // overflow + the radius already on the box: the browser clips
        // the subtree to the curve, natively, as a layer
        base.push("overflow:hidden");
      }
      if (mask & 32768) {
        const tip = text(u16());
        if (el) el.dataset.tip = tip;
      } else if (el && el.dataset.tip !== undefined) {
        delete el.dataset.tip;
      }
      const name = `[data-n="${id}"]`;
      let rule = `${name}{${base.join(";")}}`;
      if (hover) rule += `\n${name}:hover{background:${hover}}`;
      if (pressed) rule += `\n${name}:active{background:${pressed}}`;
      if (hoverInk) rule += `\n${name}:hover{color:${hoverInk}}`;
      if (pressedInk) rule += `\n${name}:active{color:${pressedInk}}`;
      if (focus) {
        rule += `\n${name}:focus{border-color:${focus};caret-color:${focus}}`;
      }
      if (placeholder) rule += `\n${name}::placeholder{color:${placeholder}}`;
      rules.set(id, rule);
      rulesTouched = true;
    } else if (op === 6) {
      const el = elements.get(id);
      const color = rgba(u32());
      const inheritsInk = u8();
      const size = f32();
      const weight = CSS_WEIGHTS[u8()];
      const mono = u8();
      const italic = u8();
      const truncation = u8();
      const raw = bytes(u32());
      const spanCount = u16();
      const spans = [];
      for (let s = 0; s < spanCount; s++) spans.push([u32(), u32()]);
      const spanColor = rgba(u32());
      if (el) {
        el.style.font = cssFont(size, weight, mono, italic);
        // an inherited ink takes NO inline color: an inline one would
        // outrank the :hover rule of the box that owns both states
        el.style.color = inheritsInk ? "" : color;
        if (truncation !== 0) {
          el.style.overflow = "hidden";
          el.style.textOverflow = "ellipsis";
        }
        el.textContent = "";
        // spans are BYTE ranges into the UTF-8 — slice before decoding
        let cursor = 0;
        const emit = (from, to, highlighted) => {
          if (to <= from) return;
          const piece = decoder.decode(raw.subarray(from, to));
          if (highlighted) {
            const mark = document.createElement("span");
            mark.style.color = spanColor;
            mark.textContent = piece;
            el.appendChild(mark);
          } else {
            el.appendChild(document.createTextNode(piece));
          }
        };
        for (const [from, to] of spans) {
          emit(cursor, from, false);
          emit(from, to, true);
          cursor = to;
        }
        emit(cursor, raw.length, false);
      }
    } else if (op === 7) {
      const el = elements.get(id);
      const color = rgba(u32());
      const size = f32();
      const weight = CSS_WEIGHTS[u8()];
      const mono = u8();
      const italic = u8();
      const content = text(u32());
      const placeholder = text(u32());
      const path = text(u16());
      if (el) {
        el.style.font = cssFont(size, weight, mono, italic);
        el.style.color = color;
        el.placeholder = placeholder;
        el.dataset.path = path;
        // write only when the value differs — echoing the browser's own
        // edit back into it would fight the caret
        if (el.value !== content) el.value = content;
      }
    } else if (op === 8) {
      const el = elements.get(id);
      const x = f32();
      const y = f32();
      if (el) {
        if (Math.abs(el.scrollLeft - x) >= 1) el.scrollLeft = x;
        if (Math.abs(el.scrollTop - y) >= 1) el.scrollTop = y;
      }
    } else if (op === 9) {
      const hi = u32();
      const lo = u32();
      const cover = u8();
      const el = elements.get(id);
      const entry = images.get(imageKey(hi, lo));
      if (el && entry) {
        el.src = entry.url;
        // false: our frame IS the rect (contain and stretch resolve in
        // the engine's geometry) — the element just fills it
        el.style.objectFit = cover ? "cover" : "fill";
      }
    } else if (op === 10) {
      u32(); // the symbol identity rides for the debugger's eyes
      u32();
      const color = rgba(u32());
      const inheritsInk = u8();
      const count = u8();
      const el = elements.get(id);
      if (el) {
        // an inherited ink takes NO inline color — the box above owns
        // both states, the same law the text keeps
        el.style.color = inheritsInk ? "" : color;
        el.textContent = "";
      }
      for (let d = 0; d < count; d++) {
        const paint = u8();
        const width = f32();
        // the draw's own palette, or currentColor to ride the ink
        const ink = u8() === 1 ? rgba(u32()) : "currentColor";
        const data = text(u32());
        if (!el) continue;
        const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
        path.setAttribute("d", data);
        if (paint === 2) {
          path.setAttribute("fill", "none");
          path.setAttribute("stroke", ink);
          path.setAttribute("stroke-width", width);
          path.setAttribute("stroke-linecap", "round");
          path.setAttribute("stroke-linejoin", "round");
        } else {
          path.setAttribute("fill", ink);
          if (paint === 1) path.setAttribute("fill-rule", "evenodd");
        }
        el.appendChild(path);
      }
    }
  }
  // ONE stylesheet rebuild per batch — a window churn touches dozens
  // of rules and must not pay the whole sheet for each
  if (rulesTouched) flushRules();
}

function sendField(path, value, caret) {
  const pathBytes = new TextEncoder().encode(path);
  const valueBytes = new TextEncoder().encode(value);
  const pathPointer = wasm.bunny_alloc(pathBytes.length);
  new Uint8Array(wasm.memory.buffer, pathPointer, pathBytes.length).set(pathBytes);
  const valuePointer = wasm.bunny_alloc(valueBytes.length);
  new Uint8Array(wasm.memory.buffer, valuePointer, valueBytes.length).set(valueBytes);
  wasm.bunny_field(
    pathPointer,
    pathBytes.length,
    valuePointer,
    valueBytes.length,
    caret,
  );
}

const imports = {
  bunny: {
    js_blit() {},
    js_request_frame() {},
    // A task woke: ONE turn, out of the current job. Here the turn is
    // a patch pass, not a repaint — the browser owns the pixels.
    js_request_wake() {
      if (wakeArmed) return;
      wakeArmed = true;
      queueMicrotask(() => {
        wakeArmed = false;
        wasm.bunny_wake();
      });
    },
    // The image edge, at home: the bytes become a blob URL the <img>
    // elements load straight from — the browser decodes, caches and
    // paints; no pixel ever crosses back for elements. The probe
    // reports the intrinsic size so the engine's geometry reflows.
    js_image_register(hi, lo, pointer, length) {
      const key = imageKey(hi, lo);
      const bytes = new Uint8Array(wasm.memory.buffer, pointer, length).slice();
      const url = URL.createObjectURL(new Blob([bytes]));
      const probe = new Image();
      const entry = { url, probe, width: 0, height: 0 };
      images.set(key, entry);
      probe.onload = () => {
        entry.width = probe.naturalWidth;
        entry.height = probe.naturalHeight;
        wasm.bunny_image_ready(hi, lo);
      };
      probe.src = url;
    },
    js_image_size(hi, lo, out) {
      const entry = images.get(imageKey(hi, lo));
      const view = new Uint32Array(wasm.memory.buffer, out, 2);
      view[0] = entry ? entry.width : 0;
      view[1] = entry ? entry.height : 0;
    },
    // islands still composite in the engine — a `.rendering(Gpu)`
    // subtree needs the pixels on our side of the border
    js_image_raster(hi, lo, width, height, out) {
      const entry = images.get(imageKey(hi, lo));
      if (!entry || !entry.width) return;
      const ink = inkSurface(width, height);
      ink.setTransform(1, 0, 0, 1, 0, 0);
      ink.clearRect(0, 0, width, height);
      ink.imageSmoothingEnabled = true;
      ink.imageSmoothingQuality = "high";
      ink.drawImage(entry.probe, 0, 0, width, height);
      const pixels = ink.getImageData(0, 0, width, height).data;
      new Uint8Array(wasm.memory.buffer, out, width * height * 4).set(pixels);
    },
    js_apply_patches(pointer, length) {
      const view = new DataView(wasm.memory.buffer, pointer, length);
      applyPatches(view, length);
    },
    // fresh pixels for one canvas island, straight onto its element
    js_island(id, pointer, width, height) {
      const el = elements.get(id);
      if (!el || el.tagName !== "CANVAS") return;
      if (el.width !== width) el.width = width;
      if (el.height !== height) el.height = height;
      const pixels = new Uint8ClampedArray(
        wasm.memory.buffer,
        pointer,
        width * height * 4,
      );
      el.getContext("2d").putImageData(new ImageData(pixels, width, height), 0, 0);
    },
    js_measure_text(pointer, length, size, weight, mono, italic, out) {
      const text = decoder.decode(
        new Uint8Array(wasm.memory.buffer, pointer, length),
      );
      const ink = inkSurface();
      ink.font = cssFont(size, weight, mono, italic);
      const probe = ink.measureText(text || "Mg");
      const metrics = new Float64Array(wasm.memory.buffer, out, 3);
      metrics[0] = text ? probe.width : 0;
      metrics[1] = probe.fontBoundingBoxAscent ?? size * 0.8;
      metrics[2] = probe.fontBoundingBoxDescent ?? size * 0.25;
    },
    // canvas islands raster their text through the engine — the same
    // contract as the full-canvas mode
    js_raster_text(
      pointer,
      length,
      size,
      weight,
      mono,
      italic,
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
      ink.font = cssFont(size, weight, mono, italic);
      ink.textBaseline = "alphabetic";
      const r = (color >>> 24) & 0xff;
      const g = (color >>> 16) & 0xff;
      const b = (color >>> 8) & 0xff;
      const a = color & 0xff;
      ink.fillStyle = `rgba(${r}, ${g}, ${b}, ${a / 255})`;
      ink.fillText(text, 0, height / scale - descent);
      const pixels = ink.getImageData(0, 0, width, height).data;
      new Uint8Array(wasm.memory.buffer, out, width * height * 4).set(pixels);
    },
  },
  // The APP's own door to the network, in its own module: the engine
  // opens no socket, and the answer goes back through an export the
  // app declared. A failed fetch answers with an empty body — the task
  // decides what that means.
  app: {
    js_fetch(pointer, length) {
      const url = decoder.decode(
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

WebAssembly.instantiateStreaming(fetch("finder_web.wasm"), imports).then(
  ({ instance }) => {
    wasm = instance.exports;
    window.__bunny = wasm;
    wasm.start_dom(
      app.clientWidth,
      app.clientHeight,
      window.devicePixelRatio || 1,
    );

    const point = (event) => {
      const rect = app.getBoundingClientRect();
      return [event.clientX - rect.left, event.clientY - rect.top];
    };
    // The move door: this mode hears NO pointer moves — the browser
    // owns the hover, and that is why a hover here costs zero patches.
    // A drag is the one gesture that needs them, so the door opens only
    // when a press landed on a drag source and shuts on the release: a
    // hover with no button down can never reach the engine.
    const onDragMove = (event) => {
      const [x, y] = point(event);
      wasm.bunny_pointer_move(x, y);
    };
    const closeDragDoor = () => {
      app.removeEventListener("pointermove", onDragMove);
      app.style.userSelect = "";
    };
    app.addEventListener("pointerdown", (event) => {
      const [x, y] = point(event);
      wasm.bunny_pointer_down(x, y, event.detail || 1);
      if (wasm.bunny_drag_armed()) {
        // the browser's own text selection would fight the drag
        app.style.userSelect = "none";
        app.addEventListener("pointermove", onDragMove);
      }
    });
    // a pointer that leaves the window mid-drag ends the gesture too
    app.addEventListener("pointercancel", closeDragDoor);
    app.addEventListener("contextmenu", (event) => {
      // the scene offers its own menu — the browser's stays home
      event.preventDefault();
      const [x, y] = point(event);
      wasm.bunny_context_click(x, y);
    });
    app.addEventListener("pointerup", (event) => {
      closeDragDoor();
      const [x, y] = point(event);
      wasm.bunny_pointer_up(x, y);
    });
    // the browser owns the <input>s in this mode. What still belongs
    // to the engine: Escape (the keymap dismisses the popover) and
    // every stroke a focused canvas island wants — a box the app
    // paints has no element to type into.
    window.addEventListener("keydown", (event) => {
      const typing = event.target && event.target.tagName === "INPUT";
      const mods = modifiers(event);
      const code = KEYS[event.key];
      if (code !== undefined) {
        if (typing && code !== 7) return;
        if (code !== 7) event.preventDefault();
        wasm.bunny_key(code, mods);
        return;
      }
      if (event.key.length !== 1) return;
      if (event.metaKey || event.ctrlKey) {
        wasm.bunny_key_char(baseChar(event).codePointAt(0), mods);
        return;
      }
      if (typing) return;
      event.preventDefault();
      sendText(event.key);
    });
  },
);
