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

let wasm = null;
let wakeArmed = false;
const decoder = new TextDecoder();

// The wire contract this file decodes (bunny_ui::dom::ABI_VERSION).
// The wasm exports its own number; boot compares the two and refuses
// a stream this mirror was not written for. Deploy the page and the
// wasm together.
const EXPECTED_ABI = 4;

// Which wasm this page boots: the page sets `window.BUNNY_WASM`
// before this script loads; the finder's binary is the default. The
// entry export follows the same door (`window.BUNNY_START`).
const WASM_URL = window.BUNNY_WASM || "finder_web.wasm";
const START_EXPORT = window.BUNNY_START || "start_dom";
// `?stats` on the page URL: the glue accumulates its apply-side wall
// time in `window.__bunnyApply` — the column the wasm cannot see.
const STATS = location.search.includes("stats");

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

// A click resolved by the BROWSER: the nearest [data-path] above the
// event target IS the pressed thing — no coordinates cross the border.
function sendAction(path, clicks) {
  const bytes = new TextEncoder().encode(path);
  const pointer = wasm.bunny_alloc(bytes.length);
  new Uint8Array(wasm.memory.buffer, pointer, bytes.length).set(bytes);
  wasm.bunny_action(pointer, bytes.length, clicks);
}

// Scroll boxes, reported as they resize — the flow frame's window
// math reads them.
const viewportObserver = new ResizeObserver((entries) => {
  if (!wasm) return;
  for (const entry of entries) {
    const id = Number(entry.target.dataset.n);
    const box = entry.contentRect;
    wasm.bunny_dom_viewport(id, box.width, box.height);
  }
});

// Canvas island boxes, reported as they resize — a flexible island
// re-measures against the box the browser really gave it.
const islandObserver = new ResizeObserver((entries) => {
  if (!wasm || !wasm.bunny_dom_box) return;
  for (const entry of entries) {
    const id = Number(entry.target.dataset.n);
    const box = entry.contentRect;
    wasm.bunny_dom_box(id, box.width, box.height);
  }
});

// A canvas island's wiring: its box reported as it resizes, and the
// pointer handed over in the canvas's OWN coordinates — a press
// captures the pointer so the moves keep arriving until the release,
// the way dragging inside an app-painted box needs.
function wireIsland(el, id) {
  islandObserver.observe(el);
  el.addEventListener("pointerdown", (event) => {
    try {
      el.setPointerCapture(event.pointerId);
    } catch {
      // a pointer that already lifted cannot be captured — the press
      // still counts
    }
    event.preventDefault();
    wasm.bunny_island_pointer(id, 0, event.offsetX, event.offsetY);
  });
  el.addEventListener("pointermove", (event) => {
    wasm.bunny_island_pointer(id, 1, event.offsetX, event.offsetY);
  });
  el.addEventListener("pointerup", (event) => {
    wasm.bunny_island_pointer(id, 2, event.offsetX, event.offsetY);
  });
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
// Pseudo-STATE rules only (:hover, :active, :focus, ::placeholder),
// one CSSRule object per declaration, keyed by element id. Everything
// a resting element shows lives inline on the element — the style
// placement law, shared with the server-side serializer. Pseudo rules
// carry !important because an inline declaration outranks the sheet.
const pseudoRules = new Map();

function dropPseudo(id) {
  const live = pseudoRules.get(id);
  if (!live) return;
  const styles = sheet.sheet;
  for (const rule of live) {
    const rules = styles.cssRules;
    // reverse scan: recent elements die young and sit near the end
    for (let i = rules.length - 1; i >= 0; i--) {
      if (rules[i] === rule) {
        styles.deleteRule(i);
        break;
      }
    }
  }
  pseudoRules.delete(id);
}

function setPseudo(id, texts) {
  dropPseudo(id);
  if (!texts.length) return;
  const styles = sheet.sheet;
  const live = [];
  for (const text of texts) {
    const at = styles.cssRules.length;
    styles.insertRule(text, at);
    live.push(styles.cssRules[at]);
  }
  pseudoRules.set(id, live);
}

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

// A family the app NAMED comes first and the house stack stays behind
// it as the fallback — a name this machine does not carry then reads in
// the face it would have had anyway.
// The engine names a key by what it types with NO modifier — so a chord
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

// The family name off the border, or null for the face nobody named —
// a zero length is the common case and costs one comparison.
function familyOf(pointer, length) {
  return length
    ? decoder.decode(new Uint8Array(wasm.memory.buffer, pointer, length))
    : null;
}

function cssFont(size, weight, mono, italic, named) {
  const house = mono
    ? 'ui-monospace, Menlo, Consolas, monospace'
    : 'system-ui, -apple-system, "Segoe UI", sans-serif';
  const family = named ? `"${named.replace(/"/g, "")}", ${house}` : house;
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

// The input's editing dance — shared by creation and hydration.
function wireInput(input) {
  let composing = false;
  const report = () => {
    const path = input.dataset.path ?? "";
    sendField(path, input.value, input.selectionStart ?? input.value.length);
  };
  input.addEventListener("compositionstart", () => {
    composing = true;
  });
  input.addEventListener("compositionend", () => {
    composing = false;
    report();
  });
  input.addEventListener("input", () => {
    // NEVER during a live composition — the browser owns that dance
    if (!composing) report();
  });
}

// A scroll box's reporting — shared by creation and hydration.
function wireScroll(el, id) {
  el.addEventListener("scroll", () => {
    wasm.bunny_dom_scroll(id, el.scrollLeft, el.scrollTop);
    repositionPopovers();
  });
  viewportObserver.observe(el);
}

// The table family lays itself out — a hinted <tr> must BE a table
// row, not a flex box wearing its name. For these tags the browser's
// own display wins and our flex steps aside.
const TABLE_TAGS = new Set(["table", "thead", "tbody", "tfoot", "tr", "td", "th"]);

function createElementOf(kind, tag) {
  const el = createElementRaw(kind, tag);
  if (tag && TABLE_TAGS.has(tag)) {
    el.style.display = "";
    el.style.minWidth = "";
    el.style.minHeight = "";
  }
  return el;
}

function createElementRaw(kind, tag) {
  // 0 group, 1 box, 2 text, 3 field, 4 scroll, 5 content, 6 canvas,
  // 7 image, 8 icon, 9 flex column, 10 flex row, 11 layers, 12 popover,
  // 13 editor — the field of MANY lines, a `<textarea>`
  if (kind === 9 || kind === 10) {
    // a FLOW container: static, the browser lays its children out
    const el = document.createElement(tag || "div");
    el.style.cssText =
      `display:flex;flex-direction:${kind === 9 ? "column" : "row"};` +
      "box-sizing:border-box;min-width:0;min-height:0;";
    return el;
  }
  if (kind === 11) {
    // layered children: one grid cell, everyone in it
    const el = document.createElement(tag || "div");
    el.style.cssText =
      "display:grid;box-sizing:border-box;min-width:0;min-height:0;";
    return el;
  }
  if (kind === 12) {
    // a popover: absolute under the root; the glue positions it from
    // the anchor's real box once the placement round lands
    const el = document.createElement(tag || "div");
    el.style.cssText = "position:absolute;left:0;top:0;box-sizing:border-box;";
    return el;
  }
  if (kind === 6) {
    const canvas = document.createElement("canvas");
    // position rides the ops: absolute geometry sets it, flow leaves
    // the element in the stream
    canvas.style.cssText = "";
    return canvas;
  }
  if (kind === 7) {
    const img = document.createElement("img");
    // the box underneath owns the clicks; our geometry owns the frame
    img.style.cssText = "pointer-events:none;";
    img.draggable = false;
    return img;
  }
  if (kind === 8) {
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    // the viewBox mirrors the engine's ICON_GRID; the default
    // preserveAspectRatio (xMidYMid meet) is the SAME centred square
    // the rasterizers paint
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.style.cssText = "pointer-events:none;";
    return svg;
  }
  if (kind === 3 || kind === 13) {
    // 13 is the field of MANY lines: a textarea, so the browser wraps,
    // breaks and scrolls it at home. A field that changes shape is
    // RECREATED — that is the only way an input becomes a textarea
    const input = document.createElement(kind === 13 ? "textarea" : "input");
    if (kind === 3) input.type = "text";
    // padding mirrors the engine's FIELD_PAD; every color and border
    // arrives through the patches — the theme owns the chrome, and no
    // inline border may outrank the stylesheet rule
    input.style.cssText =
      "box-sizing:border-box;padding:5px 8px;outline:none;" +
      (kind === 13 ? "resize:none;font:inherit;" : "");
    wireInput(input);
    return input;
  }
  const el = document.createElement(tag || "div");
  if (kind === 0 || kind === 1) {
    // a wrapper is a COLUMN, not a block: the engine proposes its box
    // to the child, and only a flex line can hand the offer down
    // (width by the stretch default, height by the fill flag)
    el.style.cssText =
      "display:flex;flex-direction:column;box-sizing:border-box;" +
      "min-width:0;min-height:0;";
    return el;
  }
  el.style.cssText = "box-sizing:border-box;min-width:0;min-height:0;";
  if (kind === 2) {
    // the browser breaks the lines in this mode — pre-wrap keeps the
    // engine's explicit newlines and wraps the rest
    el.style.whiteSpace = "pre-wrap";
    el.style.cursor = "default";
  }
  if (kind === 4) {
    el.style.overflow = "auto";
    el.style.scrollBehavior = "smooth";
  }
  if (kind === 5) {
    // content hosts virtual rows at absolute slots — the element OWNS
    // this position: the layout reset restores it, never erases it
    el.style.position = "relative";
    el.__pos = "relative";
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

  let removedAny = false;
  // fresh siblings gather in fragments and land on the LIVE tree once
  // per parent — a thousand appended rows must not pay a thousand
  // live-tree insertions
  const staged = new Map();
  const stagedFor = (parentId, parentEl) => {
    let fragment = staged.get(parentId);
    if (!fragment) {
      fragment = { holder: document.createDocumentFragment(), parent: parentEl };
      staged.set(parentId, fragment);
    }
    return fragment.holder;
  };
  const count = u32();
  for (let i = 0; i < count; i++) {
    const op = u8();
    const id = u32();
    if (op === 1) {
      const parent = u32();
      const before = u32();
      const tag = text(u8());
      const cls = text(u8());
      const domId = text(u8());
      const kind = u8();
      const el = createElementOf(kind, tag);
      el.dataset.n = id;
      if (cls) el.className = cls;
      if (domId) el.id = domId;
      if (kind === 4) {
        wireScroll(el, id);
      }
      if (kind === 6) {
        wireIsland(el, id);
      }
      const home = elements.get(parent);
      if (home) {
        const anchor = before ? elements.get(before) : null;
        if (anchor) {
          // relative to wherever the anchor LIVES — it may still sit
          // in a staged fragment on its way to the tree
          (anchor.parentNode ?? home).insertBefore(el, anchor);
        } else if (home.isConnected) {
          // stage appends to live parents; detached ones are cheap
          stagedFor(parent, home).appendChild(el);
        } else {
          home.appendChild(el);
        }
      }
      elements.set(id, el);
    } else if (op === 2) {
      const el = elements.get(id);
      if (el) el.remove();
      elements.delete(id);
      dropPseudo(id);
      // the SUBTREE's registrations die in one sweep at the end of
      // the batch — a thousand row removals must not pay a thousand
      // subtree queries
      removedAny = true;
    } else if (op === 3) {
      const el = elements.get(id);
      const x = f32();
      const y = f32();
      if (el) {
        // absolute geometry: the op declares the regime
        el.style.position = "absolute";
        el.style.left = "0";
        el.style.top = "0";
        el.style.transform = `translate(${x}px, ${y}px)`;
      }
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
      // the mask carries twenty-four bits, so it crosses as a u32
      const mask = u32();
      // full replace, the record's semantics: what the mask does not
      // carry, the element does not keep
      if (el) {
        const style = el.style;
        style.backgroundColor = "";
        style.backgroundImage = "";
        style.border = "";
        style.borderRadius = "";
        style.boxShadow = "";
        style.transition = "";
        style.color = "";
        style.overflow = "";
        style.opacity = "";
        style.pointerEvents = "";
        style.backdropFilter = "";
        style.webkitBackdropFilter = "";
      }
      const name = `[data-n="${id}"]`;
      const pseudo = [];
      // the halo and the glass rim share one property: collect both and
      // write a single declaration at the end
      const shadows = [];
      // the states are held back until the whole record is read: the
      // group arrives at bit 19, AFTER the hover and pressed values,
      // and it decides whose pointer the rule listens to
      let hover = null;
      let pressed = null;
      let hoverInk = null;
      let pressedInk = null;
      let hoverFade = null;
      let pressedFade = null;
      let group = null;
      if (mask & 1) {
        const color = rgba(u32());
        if (el) el.style.backgroundColor = color;
      }
      if (mask & 2) hover = rgba(u32());
      if (mask & 4) pressed = rgba(u32());
      if (mask & 8) {
        const borderColor = u32();
        const borderWidth = f32();
        if (el) el.style.border = `${borderWidth}px solid ${rgba(borderColor)}`;
      }
      // bit 4 is the one radius every corner shares; bit 22 below is the
      // four, and a box sends one or the other, never both
      if (mask & 16) {
        const radius = f32();
        if (el) el.style.borderRadius = `${radius}px`;
      }
      if (mask & 32) {
        const radius = f32();
        shadows.push(`0 0 ${radius}px ${rgba(u32())}`);
      }
      if (mask & 64) {
        const response = f32();
        f32(); // damping — the CSS side keeps the duration
        if (el) {
          el.style.transition =
            `background-color ${response}s ease-out,` +
            ` transform ${response}s ease-out`;
        }
      }
      if (mask & 128) {
        const path = text(u16());
        if (el) {
          el.dataset.path = path;
          el.style.cursor = "default";
        }
      }
      if (mask & 256) {
        const focus = rgba(u32());
        pseudo.push(
          `${name}:focus{border-color:${focus} !important;caret-color:${focus}}`,
        );
      }
      if (mask & 512) {
        pseudo.push(`${name}::placeholder{color:${rgba(u32())}}`);
      }
      // the ink the subtree INHERITS: the text below sets no color of
      // its own, so the hover and active rules flip the box at once
      if (mask & 1024) {
        const ink = rgba(u32());
        if (el) el.style.color = ink;
      }
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
        let image;
        if (kind === 0 && aspect !== 1 && d > 0) {
          // the ellipse: X radius on the wire, Y is X times the aspect
          image =
            `radial-gradient(ellipse ${d}px ${d * aspect}px at ` +
            `${a * 100}% ${b * 100}%, ${near} ${((c / d) * 100).toFixed(2)}%, ${far} 100%)`;
        } else if (kind === 0) {
          const reach = d < 0 ? "farthest-corner" : `${d}px`;
          const stop = d < 0 ? "100%" : `${d}px`;
          image =
            `radial-gradient(circle ${reach} at ` +
            `${a * 100}% ${b * 100}%, ${near} ${c}px, ${far} ${stop})`;
        } else {
          // CSS runs its line through the centre: the angle carries the
          // direction (0deg points up, clockwise)
          const degrees = (Math.atan2(c - a, -(d - b)) * 180) / Math.PI;
          image = `linear-gradient(${degrees.toFixed(2)}deg, ${near}, ${far})`;
        }
        if (el) el.style.backgroundImage = image;
      }
      if (mask & 16384) {
        // overflow + the radius already on the box: the browser clips
        // the subtree to the curve, natively, as a layer
        if (el) el.style.overflow = "hidden";
      }
      if (mask & 32768) {
        const tip = text(u16());
        if (el) el.dataset.tip = tip;
      } else if (el && el.dataset.tip !== undefined) {
        delete el.dataset.tip;
      }
      // the fade is a real LAYER here: the browser composites the
      // subtree once, which the per-command multiply of the pixel
      // pipelines only approximates
      if (mask & 65536) {
        const fade = f32();
        if (el) el.style.opacity = `${fade}`;
      }
      if (mask & 131072) hoverFade = f32();
      if (mask & 262144) pressedFade = f32();
      // a box that follows a GROUP takes its states from an ANCESTOR's
      // pointer: the same rules, hung off the group's selector, so the
      // browser still owns the hover and a group frame costs no patch.
      // The group crosses as a NUMBER: the browser needs an anchor to
      // hang a selector on, never the path a person reads
      if (mask & 524288) group = imageKey(u32(), u32());
      if (mask & 1048576) {
        const owner = imageKey(u32(), u32());
        if (el) el.dataset.g = owner;
      } else if (el && el.dataset.g !== undefined) {
        delete el.dataset.g;
      }
      if (mask & 2097152) {
        // a layer that asks for nothing: the click belongs to whatever
        // it covers
        if (el) el.style.pointerEvents = "none";
      }
      if (mask & 4194304) {
        // four corners, clockwise from the top left — the CSS order
        const tl = f32();
        const tr = f32();
        const br = f32();
        const bl = f32();
        if (el) el.style.borderRadius = `${tl}px ${tr}px ${br}px ${bl}px`;
      }
      // liquid glass, the half a browser owns: the blur, the saturation
      // and the brightness are one native filter over what is BEHIND
      // the element, and the rim goes on as two inset shadows along the
      // lit diagonals — the dual lobe the material is known by. The
      // tint already arrived folded into the background. The lens and
      // the touch lights stay with the pixel modes: CSS has no
      // displacement map, and this mode promises the geometry with
      // native text, never the pixels
      if (mask & 8388608) {
        const blur = f32();
        const saturation = f32();
        const brightness = f32();
        const rim = rgba(u32());
        const band = f32();
        const filter =
          `blur(${blur}px) saturate(${saturation}) brightness(${brightness})`;
        if (el) {
          el.style.backdropFilter = filter;
          el.style.webkitBackdropFilter = filter;
        }
        if (band > 0) {
          const spread = Math.max(1, band);
          shadows.push(
            `inset ${spread}px ${spread}px ${spread * 1.5}px ${-spread}px ${rim}`,
          );
          shadows.push(
            `inset ${-spread}px ${-spread}px ${spread * 1.5}px ${-spread}px ${rim}`,
          );
        }
      }
      if (shadows.length && el) el.style.boxShadow = shadows.join(",");
      // a follower hangs its states off the GROUP's pointer; a box
      // without one listens to its own
      const on = (state) =>
        group ? `[data-g="${group}"]:${state} ${name}` : `${name}:${state}`;
      if (hover) pseudo.push(`${on("hover")}{background:${hover} !important}`);
      if (pressed) {
        pseudo.push(`${on("active")}{background:${pressed} !important}`);
      }
      if (hoverInk) pseudo.push(`${on("hover")}{color:${hoverInk} !important}`);
      if (pressedInk) {
        pseudo.push(`${on("active")}{color:${pressedInk} !important}`);
      }
      if (hoverFade !== null) {
        pseudo.push(`${on("hover")}{opacity:${hoverFade}}`);
      }
      if (pressedFade !== null) {
        pseudo.push(`${on("active")}{opacity:${pressedFade}}`);
      }
      setPseudo(id, pseudo);
    } else if (op === 6) {
      const el = elements.get(id);
      const color = rgba(u32());
      const inheritsInk = u8();
      const size = f32();
      const weight = CSS_WEIGHTS[u8()];
      const mono = u8();
      const italic = u8();
      const family = text(u16());
      const truncation = u8();
      const raw = bytes(u32());
      const spanCount = u16();
      const spans = [];
      for (let s = 0; s < spanCount; s++) spans.push([u32(), u32()]);
      const spanColor = rgba(u32());
      if (el) {
        el.style.font = cssFont(size, weight, mono, italic, family);
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
      const family = text(u16());
      const content = text(u32());
      const placeholder = text(u32());
      const path = text(u16());
      if (el) {
        el.style.font = cssFont(size, weight, mono, italic, family);
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
        // a draw wears its OWN colour when it declared one; otherwise
        // it takes the ink the element inherits
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
    } else if (op === 11) {
      // the FULL flow record — reset, then apply what the mask carries
      const el = elements.get(id);
      const mask = u16();
      if (el) {
        const style = el.style;
        style.gap = "";
        style.alignItems = "";
        style.padding = "";
        style.width = "";
        style.height = "";
        style.maxWidth = "";
        style.maxHeight = "";
        style.flex = "";
        style.minWidth = "0";
        style.minHeight = "0";
        style.position = el.__pos || "";
        style.top = "";
        style.left = "";
        style.right = "";
        style.transform = "";
        style.alignSelf = "";
      }
      const apply = el ? el.style : null;
      if (mask & 1) {
        const gap = f32();
        if (apply) apply.gap = `${gap}px`;
      }
      if (mask & 2) {
        const align = u8();
        if (apply) {
          apply.alignItems =
            align === 1 ? "center" : align === 2 ? "flex-end" : align === 3 ? "baseline" : "flex-start";
        }
      }
      if (mask & 4) {
        const [top, right, bottom, left] = [f32(), f32(), f32(), f32()];
        if (apply) apply.padding = `${top}px ${right}px ${bottom}px ${left}px`;
      }
      if (mask & 8) {
        const width = f32();
        if (apply) apply.width = `${width}px`;
      }
      if (mask & 16) {
        const height = f32();
        if (apply) apply.height = `${height}px`;
      }
      if (mask & 32) {
        const max = f32();
        if (apply) apply.maxWidth = `${max}px`;
      }
      if (mask & 64) {
        const max = f32();
        if (apply) apply.maxHeight = `${max}px`;
      }
      if (mask & 128 && apply) {
        // the flexible child — and the classic flex footgun: a zeroed
        // min-size, or content refuses to shrink
        apply.flex = "1 1 0";
        apply.minWidth = "0";
        apply.minHeight = "0";
      }
      if (mask & 256) {
        const slotY = f32();
        if (apply) {
          // a virtual row: absolute inside its relative content box
          apply.position = "absolute";
          apply.top = `${slotY}px`;
          apply.left = "0";
          apply.right = "0";
        }
      }
      if (mask & 512 && apply) {
        // the axis the child left to its container — a flexible
        // island discovers its real box this way
        apply.alignSelf = "stretch";
      }
      if (mask & 1024 && apply) {
        // take the offer, keep the content floor
        apply.flex = "1 1 auto";
        apply.minWidth = "0";
        apply.minHeight = "0";
      }
    } else if (op === 12) {
      // one insertBefore, identity intact (0 = to the end)
      const el = elements.get(id);
      const parent = elements.get(u32());
      const before = u32();
      if (el && parent) parent.insertBefore(el, before ? (elements.get(before) ?? null) : null);
    } else if (op === 13) {
      // the browser computes the offset — dense lists only
      const target = elements.get(u32());
      if (target) target.scrollIntoView({ block: "nearest" });
    } else if (op === 15) {
      // live hints: class and id re-attribute in place
      const cls = text(u8());
      const domId = text(u8());
      const el = elements.get(id);
      if (el) {
        el.className = cls;
        if (domId) {
          el.id = domId;
        } else {
          el.removeAttribute("id");
        }
      }
    } else if (op === 14) {
      // the popover's anchor relation — position now, and again
      // whenever anything scrolls or the window resizes
      const anchor = u32();
      const side = u8();
      const path = text(u16());
      const el = elements.get(id);
      if (el) {
        el.dataset.popover = path;
        el.dataset.anchor = anchor;
        el.dataset.side = side;
        placePopover(el);
      }
    }
  }
  for (const { holder, parent } of staged.values()) {
    parent.appendChild(holder);
  }
  if (removedAny) {
    // one pass over the registry: whatever a removal detached loses
    // its entry and its pseudo rules — ids are never reused, so a
    // survivor here would leak for the page's whole life
    for (const [id, el] of elements) {
      if (id !== 0 && !el.isConnected) {
        elements.delete(id);
        dropPseudo(id);
      }
    }
  }
}

// The popover placement: the browser owns the boxes, so the browser
// positions the card — preferred side, flip when it does not fit,
// then a two-axis clamp into the root. The engine's flip-then-clamp
// policy, in the coordinate system that owns it here.
const POPOVER_GAP = 6;

function placePopover(el) {
  const anchorEl = elements.get(Number(el.dataset.anchor));
  if (!anchorEl || !anchorEl.isConnected) {
    // the anchor left (a filter, a window slide): the popover follows
    sendAction(`${el.dataset.popover}/#dismiss`, 1);
    return;
  }
  const appBox = app.getBoundingClientRect();
  const box = anchorEl.getBoundingClientRect();
  el.style.position = "absolute";
  const width = el.offsetWidth;
  const height = el.offsetHeight;
  const ax = box.left - appBox.left;
  const ay = box.top - appBox.top;
  const side = Number(el.dataset.side);
  const origin = (s) =>
    s === 0
      ? [ax + (box.width - width) / 2, ay - height - POPOVER_GAP]
      : s === 1
        ? [ax + (box.width - width) / 2, ay + box.height + POPOVER_GAP]
        : s === 2
          ? [ax - width - POPOVER_GAP, ay + (box.height - height) / 2]
          : [ax + box.width + POPOVER_GAP, ay + (box.height - height) / 2];
  const fits = ([x, y]) =>
    x >= 0 && y >= 0 && x + width <= appBox.width && y + height <= appBox.height;
  let [x, y] = origin(side);
  if (!fits([x, y])) {
    const flipped = origin({ 0: 1, 1: 0, 2: 3, 3: 2 }[side]);
    if (fits(flipped)) [x, y] = flipped;
  }
  x = Math.min(Math.max(x, 0), Math.max(appBox.width - width, 0));
  y = Math.min(Math.max(y, 0), Math.max(appBox.height - height, 0));
  el.style.left = "0";
  el.style.top = "0";
  el.style.transform = `translate(${x}px, ${y}px)`;
}

function repositionPopovers() {
  for (const el of app.querySelectorAll("[data-popover]")) {
    placePopover(el);
  }
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

// glue_gl.js may be absent (a build without the tier, a file that
// failed to load). Every verb answers zero, `gl_init` included, so the
// tier refuses and the islands take putImageData as they always have.
function bunnyGpuStubsOrNothing() {
  const stubs = {};
  for (const name of GPU_VERBS) stubs[name] = () => 0;
  return stubs;
}

const GPU_VERBS = [
  "gl_init", "gl_log", "gl_island_blit", "gl_teardown", "gl_resize",
  "gl_viewport", "gl_clear_color", "gl_clear", "gl_enable", "gl_disable",
  "gl_blend_func_separate", "gl_pixel_storei", "gl_finish", "gl_flush",
  "gl_compile_shader", "gl_link_program", "gl_bind_attrib_location", "gl_use_program",
  "gl_uniform_location", "gl_uniform_block", "gl_uniform1i", "gl_uniform4f", "gl_last_log",
  "gl_create_buffer", "gl_bind_buffer", "gl_bind_buffer_base", "gl_buffer_data_size",
  "gl_buffer_sub_data", "gl_delete_buffer",
  "gl_create_vertex_array", "gl_bind_vertex_array", "gl_enable_vertex_attrib_array",
  "gl_vertex_attrib_pointer", "gl_vertex_attrib_divisor",
  "gl_create_texture", "gl_bind_texture", "gl_active_texture", "gl_tex_parameteri",
  "gl_tex_image_2d", "gl_tex_sub_image_2d", "gl_delete_texture",
  "gl_create_framebuffer", "gl_bind_framebuffer", "gl_framebuffer_texture_2d",
  "gl_check_framebuffer_status", "gl_delete_framebuffer",
  "gl_draw_arrays", "gl_draw_arrays_instanced", "gl_read_pixels",
];

const imports = {
  bunny_gpu:
    typeof bunnyGpuImports === "object" ? bunnyGpuImports : bunnyGpuStubsOrNothing(),
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
      if (!STATS) {
        applyPatches(view, length);
        return;
      }
      const opened = performance.now();
      applyPatches(view, length);
      const box = (window.__bunnyApply ||= { ms: 0, batches: 0 });
      box.ms += performance.now() - opened;
      box.batches += 1;
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
    js_measure_text(
      pointer,
      length,
      size,
      weight,
      mono,
      italic,
      familyPointer,
      familyLength,
      out,
    ) {
      const text = decoder.decode(
        new Uint8Array(wasm.memory.buffer, pointer, length),
      );
      const ink = inkSurface();
      ink.font = cssFont(
        size,
        weight,
        mono,
        italic,
        familyOf(familyPointer, familyLength),
      );
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
      familyPointer,
      familyLength,
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
      ink.font = cssFont(
        size,
        weight,
        mono,
        italic,
        familyOf(familyPointer, familyLength),
      );
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

const bootOpened = performance.now();
WebAssembly.instantiateStreaming(fetch(WASM_URL), imports).then(
  ({ instance }) => {
    wasm = instance.exports;
    if (typeof gpuAttach === "function") gpuAttach(wasm);
    window.__bunny = wasm;
    window.__bunnyDebug = { elements, pseudoRules };
    // the ABI gate: a missing export counts as version 0
    const abi = wasm.bunny_abi_version ? wasm.bunny_abi_version() >>> 0 : 0;
    if (abi !== EXPECTED_ABI) {
      wasm = null;
      app.textContent =
        `This page decodes ABI ${EXPECTED_ABI}. ` +
        `The wasm encodes ABI ${abi}. ` +
        `Deploy the page and the wasm together, then reload.`;
      return;
    }
    // the window is a one-slot column: its child can take the box
    app.style.display = "flex";
    app.style.flexDirection = "column";
    // a page the BUILD painted: adopt its elements before the wasm
    // takes over — ids are the data-n the serializer stamped
    const hydrated = app.dataset.hydrate === "1";
    if (hydrated) {
      for (const el of app.querySelectorAll("[data-n]")) {
        const id = Number(el.dataset.n);
        elements.set(id, el);
        if (el.tagName === "INPUT") wireInput(el);
        if (el.style.overflow === "auto") wireScroll(el, id);
        if (el.tagName === "CANVAS") wireIsland(el, id);
        if (el.style.position === "relative") el.__pos = "relative";
      }
    }
    // the boot bill: fetch+instantiate, then the first frame inside
    // start_dom — the two numbers a mount argument needs
    window.__bunnyBoot = { instantiate: performance.now() - bootOpened };
    const startOpened = performance.now();
    wasm[START_EXPORT](
      app.clientWidth,
      app.clientHeight,
      window.devicePixelRatio || 1,
      hydrated ? 1 : 0,
    );
    window.__bunnyBoot.start = performance.now() - startOpened;

    // clicks resolve by DELEGATION: the browser already knows what
    // was pressed — the engine never sees a coordinate in this mode
    app.addEventListener("click", (event) => {
      const source = event.target instanceof Element ? event.target : null;
      // a press OUTSIDE the topmost popover dismisses it and is
      // CONSUMED — the engine's own outside-press contract
      const popovers = [...app.querySelectorAll("[data-popover]")];
      if (popovers.length && source) {
        const inside = popovers.some((popover) => popover.contains(source));
        if (!inside) {
          const topmost = popovers[popovers.length - 1];
          sendAction(`${topmost.dataset.popover}/#dismiss`, 1);
          return;
        }
      }
      const target = source ? source.closest("[data-path]") : null;
      if (target && target.dataset.path) {
        // `detail` IS the browser's click tally on a click event —
        // a double never needs a clock on this side
        sendAction(target.dataset.path, event.detail || 1);
      }
    });
    window.addEventListener("resize", repositionPopovers);
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
