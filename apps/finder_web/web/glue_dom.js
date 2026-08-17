// The Dom glue: the browser side of the SECOND rendering. The engine
// lowers the semantic scene to a patch stream (fixed little-endian ABI)
// and this file mutates real elements with it. Text selects, scroll
// carries momentum, the input owns the editing — the browser at home.

const app = document.getElementById("app");
const sheet = document.createElement("style");
document.head.appendChild(sheet);

let wasm = null;
const decoder = new TextDecoder();
const elements = new Map([[0, app]]);
const rules = new Map();

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

function cssFont(size, weight, mono) {
  const family = mono
    ? 'ui-monospace, Menlo, Consolas, monospace'
    : 'system-ui, -apple-system, "Segoe UI", sans-serif';
  return `${weight} ${size}px ${family}`;
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
  // 0 group, 1 box, 2 text, 3 field, 4 scroll, 5 content, 6 canvas
  if (kind === 6) {
    const canvas = document.createElement("canvas");
    canvas.style.cssText = "position:absolute;left:0;top:0;";
    return canvas;
  }
  if (kind === 3) {
    const input = document.createElement("input");
    input.type = "text";
    input.style.cssText =
      "position:absolute;left:0;top:0;box-sizing:border-box;" +
      "padding:5px 8px;border:1px solid #c6cad3;border-radius:5px;" +
      "background:transparent;outline:none;";
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
      const name = `[data-n="${id}"]`;
      let rule = `${name}{${base.join(";")}}`;
      if (hover) rule += `\n${name}:hover{background:${hover}}`;
      if (pressed) rule += `\n${name}:active{background:${pressed}}`;
      rules.set(id, rule);
      rulesTouched = true;
    } else if (op === 6) {
      const el = elements.get(id);
      const color = rgba(u32());
      const size = f32();
      const weight = CSS_WEIGHTS[u8()];
      const mono = u8();
      const truncation = u8();
      const raw = bytes(u32());
      const spanCount = u16();
      const spans = [];
      for (let s = 0; s < spanCount; s++) spans.push([u32(), u32()]);
      const spanColor = rgba(u32());
      if (el) {
        el.style.font = cssFont(size, weight, mono);
        el.style.color = color;
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
      const size = f32();
      const weight = CSS_WEIGHTS[u8()];
      const mono = u8();
      const content = text(u32());
      const placeholder = text(u32());
      const path = text(u16());
      if (el) {
        el.style.font = cssFont(size, weight, mono);
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
    js_measure_text(pointer, length, size, weight, mono, out) {
      const text = decoder.decode(
        new Uint8Array(wasm.memory.buffer, pointer, length),
      );
      const ink = inkSurface();
      ink.font = cssFont(size, weight, mono);
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
      ink.fillText(text, 0, height / scale - descent);
      const pixels = ink.getImageData(0, 0, width, height).data;
      new Uint8Array(wasm.memory.buffer, out, width * height * 4).set(pixels);
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
    app.addEventListener("pointerdown", (event) => {
      const [x, y] = point(event);
      wasm.bunny_pointer_down(x, y);
    });
    app.addEventListener("pointerup", (event) => {
      const [x, y] = point(event);
      wasm.bunny_pointer_up(x, y);
    });
  },
);
