// The Dom glue: the browser side of the SECOND rendering. The engine
// lowers the semantic scene to a patch stream (fixed little-endian ABI)
// and this file mutates real elements with it. Text selects, scroll
// carries momentum, the input owns the editing — the browser at home.

const app = document.getElementById("app");
// the root element IS scene node 0 — its backdrop arrives through the
// patches (the theme's canvas), like every other color here
app.dataset.n = "0";
app.__n = 0;
const sheet = document.createElement("style");
document.head.appendChild(sheet);
// every target shows the same cursor: one rule on the path attribute,
// where a declaration per element would copy the inline style of each
// clone it lands on
sheet.sheet.insertRule("[data-path]{cursor:default}", 0);

let wasm = null;
let wakeArmed = false;
let frameArmed = false;
let lastFrame = 0;
const decoder = new TextDecoder();

// The wire contract this file decodes (bunny_ui::dom::ABI_VERSION).
// The wasm exports its own number; boot compares the two and refuses
// a stream this mirror was not written for. Deploy the page and the
// wasm together.
const EXPECTED_ABI = 16;

// Which wasm this page boots: the page sets `window.BUNNY_WASM`
// before this script loads; the finder's binary is the default. The
// entry export follows the same door (`window.BUNNY_START`).
const WASM_URL = window.BUNNY_WASM || "finder_web.wasm";
const START_EXPORT = window.BUNNY_START || "start_dom";
// `?stats` on the page URL: the glue accumulates its apply-side wall
// time in `window.__bunnyApply` — the column the wasm cannot see.
const STATS = location.search.includes("stats");

// After a batch, the engine is told once when the page is idle: it
// frees then what the frames removed, never between a click and its
// paint. One callback at a time; the browser's idle door when it has
// one, a timeout where it does not.
let idleArmed = false;
function armIdle() {
  if (idleArmed || !wasm || !wasm.bunny_idle) return;
  idleArmed = true;
  const run = () => {
    idleArmed = false;
    if (wasm && wasm.bunny_idle) wasm.bunny_idle();
  };
  if (typeof requestIdleCallback === "function") {
    requestIdleCallback(run, { timeout: 1000 });
  } else {
    setTimeout(run, 50);
  }
}

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

// The function row: `F1` to `F24`, sent as 101 to 124.
const FUNCTION_KEY = /^F([1-9]|1[0-9]|2[0-4])$/;

// The keys whose going down or coming up is itself the news.
const MODIFIER_KEYS = new Set(["Shift", "Meta", "Control", "Alt"]);

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
    const id = entry.target.__n ?? Number(entry.target.dataset.n);
    const box = entry.contentRect;
    wasm.bunny_dom_viewport(id, box.width, box.height);
  }
});

// Canvas island boxes, reported as they resize — a flexible island
// re-measures against the box the browser really gave it.
const islandObserver = new ResizeObserver((entries) => {
  if (!wasm || !wasm.bunny_dom_box) return;
  for (const entry of entries) {
    const id = entry.target.__n ?? Number(entry.target.dataset.n);
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
    wasm.bunny_island_pointer(id, 0, event.offsetX, event.offsetY, modifiers(event));
  });
  el.addEventListener("pointermove", (event) => {
    wasm.bunny_island_pointer(id, 1, event.offsetX, event.offsetY, modifiers(event));
  });
  el.addEventListener("pointerup", (event) => {
    wasm.bunny_island_pointer(id, 2, event.offsetX, event.offsetY, modifiers(event));
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
// The element registry: what the engine created, by id — and the
// roots of the clones. A clone's members are not registered at all:
// their ids follow the root's in pre-order, so a member is found by its
// offset from the root along the shape's trail of child indexes, a hop
// or three each time an op names it. A thousand rows register a
// thousand roots, not eight thousand elements — and forget as many on
// a clear.
const elements = new Map([[0, app]]);
// the clone roots in the order they were made (ids only grow), each
// [root id, element count, template id]
const cloneRoots = [];
// a template's shape: for every member in pre-order, the child indexes
// that lead to it from the root
const memberTrails = new Map();

function trailsOf(templateId, templateEl) {
  let trails = memberTrails.get(templateId);
  if (trails) return trails;
  trails = [];
  const walk = (el, trail) => {
    trails.push(trail);
    const kids = el.children;
    for (let i = 0; i < kids.length; i++) walk(kids[i], trail.concat(i));
  };
  walk(templateEl, []);
  memberTrails.set(templateId, trails);
  return trails;
}

// the clone whose range holds the id: the last root at or before it
function cloneAt(id) {
  let lo = 0;
  let hi = cloneRoots.length - 1;
  let found = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (cloneRoots[mid][0] <= id) {
      found = mid;
      lo = mid + 1;
    } else {
      hi = mid - 1;
    }
  }
  return found;
}

function lookup(id) {
  const known = elements.get(id);
  if (known !== undefined) return known;
  const at = cloneAt(id);
  if (at < 0) return undefined;
  const [root, size, template] = cloneRoots[at];
  const k = id - root;
  if (k >= size) return undefined;
  const rootEl = elements.get(root);
  if (!rootEl) return undefined;
  const trail = trailsOf(template, rootEl)[k];
  let el = rootEl;
  for (let i = 0; i < trail.length; i++) {
    el = el.children[trail[i]];
    if (!el) return undefined;
  }
  return el;
}

// a clone root leaves: its range leaves with it
function forgetClone(root) {
  const at = cloneAt(root);
  if (at >= 0 && cloneRoots[at][0] === root) cloneRoots.splice(at, 1);
}

// A member is found by its offset only while the clone keeps its
// template's shape. An op about to change that shape — an element
// born, moved or removed inside the clone — registers every member
// first, by the trail that still holds, and the clone stops being one.
function settleClone(at) {
  const [root, size, template] = cloneRoots[at];
  const rootEl = elements.get(root);
  if (rootEl) {
    const trails = trailsOf(template, rootEl);
    for (let k = 1; k < size; k++) {
      let el = rootEl;
      const trail = trails[k];
      for (let i = 0; i < trail.length && el; i++) el = el.children[trail[i]];
      if (!el) continue;
      el.__n = root + k;
      elements.set(root + k, el);
    }
    delete rootEl.__members;
  }
  cloneRoots.splice(at, 1);
}

// the clone `id` belongs to, member or root, as an index into the roots
function cloneHolding(id) {
  if (cloneRoots.length === 0) return -1;
  const at = cloneAt(id);
  if (at < 0) return -1;
  return id - cloneRoots[at][0] < cloneRoots[at][1] ? at : -1;
}

// the children of `id` are about to change
function settleInside(id) {
  const known = elements.get(id);
  if (known !== undefined && known.__members === undefined) return;
  const at = cloneHolding(id);
  if (at >= 0) settleClone(at);
}

// the element `id` itself is about to move or leave: only a member's
// leaving changes a clone (a root leaves with its range)
function settleMember(id) {
  if (elements.get(id) !== undefined) return;
  const at = cloneHolding(id);
  if (at >= 0) settleClone(at);
}
// The looks the page wears: one rule per distinct look, inserted once
// and never removed. An element wears a look by CLASS, so a thousand
// rows that look alike share one rule — and the browser shares their
// computed style, where an inline declaration per element gave each
// its own. The class doubles in the selector so the rule outranks a
// page's own class rules, as the inline declaration did; a state rule
// (:hover, :active, :focus, ::placeholder) outranks the base by its
// pseudo-class alone, so none carries !important any more.
const looks = new Map(); // the look's high word -> (low word -> class)
const defined = new Set(); // the classes whose rules this page inserted

// The class a look is worn by: base36 of its two words, the
// serializer's spelling — a served page and a mounted one agree.
function lookName(hi, lo) {
  let inner = looks.get(hi);
  if (!inner) {
    inner = new Map();
    looks.set(hi, inner);
  }
  let name = inner.get(lo);
  if (name === undefined) {
    name = "b_" + ((BigInt(hi >>> 0) << 32n) | BigInt(lo >>> 0)).toString(36);
    inner.set(lo, name);
  }
  return name;
}

function defineLook(hi, lo, kind, flags, style, layout, face) {
  const name = lookName(hi, lo);
  if (defined.has(name)) return;
  defined.add(name);
  const styles = sheet.sheet;
  for (const rule of lookRules(`.${name}.${name}`, kind, flags, style, layout, face)) {
    styles.insertRule(rule, styles.cssRules.length);
  }
}

// A group crosses as a NUMBER in two words; its anchor on the page is
// the decimal of the whole, the serializer's spelling.
function u64text(hi, lo) {
  return ((BigInt(hi >>> 0) << 32n) | BigInt(lo >>> 0)).toString();
}

// The declarations a kind brings along — what createElement wrote
// inline before looks were shared (ssr::kind_shape, mirrored).
const KIND_BASE = [
  // 0 group, 1 box: a wrapper is a COLUMN, not a block — the engine
  // proposes its box to the child, and only a flex line can hand the
  // offer down (width by the stretch default, height by the fill flag)
  "display:flex;flex-direction:column;box-sizing:border-box;min-width:0;min-height:0",
  "display:flex;flex-direction:column;box-sizing:border-box;min-width:0;min-height:0",
  // 2 text: the browser breaks the lines in this mode — pre-wrap keeps
  // the engine's explicit newlines and wraps the rest
  "box-sizing:border-box;min-width:0;min-height:0;white-space:pre-wrap;cursor:default",
  // 3 field: the padding mirrors the engine's FIELD_PAD; every color
  // and border arrives through the look — the theme owns the chrome
  "box-sizing:border-box;padding:5px 8px;outline:none",
  // 4 scroll
  "box-sizing:border-box;min-width:0;min-height:0;overflow:auto;scroll-behavior:smooth",
  // 5 content: hosts virtual rows at absolute slots
  "box-sizing:border-box;min-width:0;min-height:0;position:relative",
  // 6 canvas: position rides the ops
  "",
  // 7 image, 8 icon: the box underneath owns the clicks
  "pointer-events:none",
  "pointer-events:none",
  // 9 flex column, 10 flex row: FLOW containers, the browser lays
  // their children out
  "display:flex;flex-direction:column;box-sizing:border-box;min-width:0;min-height:0",
  "display:flex;flex-direction:row;box-sizing:border-box;min-width:0;min-height:0",
  // 11 layers: one grid cell, everyone in it
  "display:grid;box-sizing:border-box;min-width:0;min-height:0",
  // 12 popover: absolute under the root; the glue positions it from
  // the anchor's real box once the placement round lands
  "position:absolute;left:0;top:0;box-sizing:border-box",
  // 13 editor: the field of MANY lines
  "box-sizing:border-box;padding:5px 8px;outline:none;resize:none;font:inherit",
  // 14 iframe: the native host's page, pointer events ON
  "border:0;box-sizing:border-box;min-width:0;min-height:0",
  // 15 video: the video host's stream, the box underneath owns the clicks
  "display:block;pointer-events:none;box-sizing:border-box;min-width:0;min-height:0",
];

// The rules of one look: the base, then one per state. The twin of the
// serializer's `rule_text` — a served page must agree with a mounted one.
function lookRules(selector, kind, flags, style, layout, face) {
  const decl = {};
  for (const pair of (KIND_BASE[kind] || "").split(";")) {
    if (!pair) continue;
    const at = pair.indexOf(":");
    decl[pair.slice(0, at)] = pair.slice(at + 1);
  }
  if (flags & 1) {
    // the table family lays itself out — a hinted <tr> must BE a table
    // row, not a flex box wearing its name: the browser's own display
    // wins and our flex steps aside
    delete decl.display;
    delete decl["flex-direction"];
    delete decl["min-width"];
    delete decl["min-height"];
  } else if (layout.plain) {
    // no flex box: an inline tag around one child keeps the browser's
    // own display for the tag
    delete decl.display;
    delete decl["flex-direction"];
  }
  if (layout.gap !== null) decl.gap = `${layout.gap}px`;
  if (layout.align !== null) {
    decl["align-items"] =
      layout.align === 1 ? "center" : layout.align === 2 ? "flex-end" : layout.align === 3 ? "baseline" : "flex-start";
  }
  if (layout.padding) decl.padding = layout.padding.map((side) => `${side}px`).join(" ");
  if (layout.grow) {
    // the flexible child — and the classic flex footgun: a zeroed
    // min-size, or content refuses to shrink
    decl.flex = "1 1 0";
    decl["min-width"] = "0";
    decl["min-height"] = "0";
  }
  if (layout.stretch) {
    // the axis the child left to its container — a flexible island
    // discovers its real box this way
    decl["align-self"] = "stretch";
  }
  if (layout.fill) {
    // take the offer, keep the content floor
    decl.flex = "1 1 auto";
    decl["min-width"] = "0";
    decl["min-height"] = "0";
  }
  if (layout.wrap !== null) {
    // a row that wraps: its lines break where its items' widths say
    decl["flex-wrap"] = "wrap";
    decl["row-gap"] = `${layout.wrap}px`;
  }
  if (style.background) decl["background-color"] = style.background;
  // background-image sits OVER the flat background
  if (style.image) decl["background-image"] = style.image;
  if (style.border) decl.border = style.border;
  if (style.radius) decl["border-radius"] = style.radius;
  // the halo and the glass rim share one property
  if (style.shadows.length) decl["box-shadow"] = style.shadows.join(",");
  if (style.transition) decl.transition = style.transition;
  // the ink the subtree INHERITS: the text below sets no color of its
  // own, so the hover and active rules flip the box at once
  if (style.ink) decl.color = style.ink;
  // overflow + the radius already on the box: the browser clips the
  // subtree to the curve, natively, as a layer
  if (style.clip) decl.overflow = "hidden";
  // the fade is a real LAYER here: the browser composites the subtree
  // once, which the per-command multiply of the pixel pipelines only
  // approximates
  if (style.fade !== null) decl.opacity = `${style.fade}`;
  // a layer that asks for nothing: the click belongs to whatever it covers
  if (style.passThrough) decl["pointer-events"] = "none";
  if (style.filter) {
    decl["backdrop-filter"] = style.filter;
    decl["-webkit-backdrop-filter"] = style.filter;
  }
  if (face) {
    if (!face.inheritsFace) decl.font = face.font;
    // AFTER the font shorthand, which resets line-height: 0 means the
    // face's own box
    if (face.lineHeight > 0) decl["line-height"] = `${face.lineHeight}px`;
    // 0 leading — the browser's own default for this direction
    if (face.align === 1) decl["text-align"] = "center";
    else if (face.align === 2) decl["text-align"] = "right";
    // an inherited ink takes NO color: the box above owns both states
    if (face.color) decl.color = face.color;
    else delete decl.color;
    if (face.truncation !== 0) {
      decl.overflow = "hidden";
      decl["text-overflow"] = "ellipsis";
      decl["white-space"] = "nowrap";
    }
  }
  const body = Object.entries(decl)
    .map(([name, value]) => `${name}:${value}`)
    .join(";");
  const rules = [`${selector}{${body}}`];
  // a follower hangs its states off the GROUP's pointer: the same
  // rules, hung off the group's selector, so the browser still owns
  // the hover and a group frame costs no patch; a box without one
  // listens to its own
  const on = (state) =>
    style.group ? `[data-g="${style.group}"]:${state} ${selector}` : `${selector}:${state}`;
  if (style.hover) rules.push(`${on("hover")}{background-color:${style.hover}}`);
  if (style.pressed) rules.push(`${on("active")}{background-color:${style.pressed}}`);
  if (style.hoverInk) rules.push(`${on("hover")}{color:${style.hoverInk}}`);
  if (style.pressedInk) rules.push(`${on("active")}{color:${style.pressedInk}}`);
  if (style.hoverFade !== null) rules.push(`${on("hover")}{opacity:${style.hoverFade}}`);
  if (style.pressedFade !== null) rules.push(`${on("active")}{opacity:${style.pressedFade}}`);
  if (style.focus) {
    rules.push(`${selector}:focus{border-color:${style.focus};caret-color:${style.focus}}`);
  }
  if (style.placeholder) rules.push(`${selector}::placeholder{color:${style.placeholder}}`);
  return rules;
}

// The class attribute: the page's own classes first, the look's last
// (the serializer's order). An element the glue did not create — a
// clone, a hydrated one — tells the two apart by the look's spelling.
const LOOK = /^b_[0-9a-z]+$/;

function learn(el) {
  if (el.__cls !== undefined) return;
  let own = "";
  let look = "";
  for (const token of (el.getAttribute("class") || "").split(" ")) {
    if (!token) continue;
    if (LOOK.test(token)) look = token;
    else own = own ? `${own} ${token}` : token;
  }
  el.__cls = own;
  el.__look = look;
}

function dress(el) {
  const own = el.__cls;
  const look = el.__look;
  if (own && look) el.setAttribute("class", `${own} ${look}`);
  else if (own || look) el.setAttribute("class", own || look);
  else el.removeAttribute("class");
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

const CSS_WEIGHTS = [400, 500, 600, 700, 800, 900];

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

function createElementOf(kind, tag) {
  // 0 group, 1 box, 2 text, 3 field, 4 scroll, 5 content, 6 canvas,
  // 7 image, 8 icon, 9 flex column, 10 flex row, 11 layers, 12 popover,
  // 13 editor — the field of MANY lines, a `<textarea>` —
  // 14 iframe — the native host's page, 15 video — the video host's
  // stream. The element is born BARE:
  // every declaration its kind brings along is the look's (op 21),
  // worn by class — a thousand rows that look alike share one rule
  if (kind === 6) return document.createElement("canvas");
  if (kind === 14) return document.createElement("iframe");
  if (kind === 15) {
    // the video host's web lowering: the browser's own `<video>`,
    // playing a stream the page registered (media.js). Muted, inline
    // and autoplaying — the audio is the app's business; the box
    // underneath owns the clicks, by the look
    const video = document.createElement("video");
    video.autoplay = true;
    video.muted = true;
    video.playsInline = true;
    video.setAttribute("playsinline", "");
    return video;
  }
  if (kind === 7) {
    const img = document.createElement("img");
    img.draggable = false;
    return img;
  }
  if (kind === 8) {
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    // the viewBox mirrors the engine's ICON_GRID; the default
    // preserveAspectRatio (xMidYMid meet) is the SAME centred square
    // the rasterizers paint
    svg.setAttribute("viewBox", "0 0 24 24");
    return svg;
  }
  if (kind === 3 || kind === 13) {
    // 13 is the field of MANY lines: a textarea, so the browser wraps,
    // breaks and scrolls it at home. A field that changes shape is
    // RECREATED — that is the only way an input becomes a textarea
    const input = document.createElement(kind === 13 ? "textarea" : "input");
    if (kind === 3) input.type = "text";
    wireInput(input);
    return input;
  }
  return document.createElement(tag || "div");
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

  // the three records a look is made of, decoded with no element in
  // hand: a look is defined once (op 21), then worn by class (op 22)
  const readStyle = () => {
    // the mask carries twenty-four bits, so it crosses as a u32
    const mask = u32();
    const style = {
      background: null,
      hover: null,
      pressed: null,
      border: null,
      radius: null,
      shadows: [],
      transition: null,
      focus: null,
      placeholder: null,
      ink: null,
      hoverInk: null,
      pressedInk: null,
      image: null,
      clip: false,
      fade: null,
      hoverFade: null,
      pressedFade: null,
      group: null,
      passThrough: false,
      filter: null,
    };
    if (mask & 1) style.background = rgba(u32());
    if (mask & 2) style.hover = rgba(u32());
    if (mask & 4) style.pressed = rgba(u32());
    if (mask & 8) {
      const borderColor = u32();
      const borderWidth = f32();
      style.border = `${borderWidth}px solid ${rgba(borderColor)}`;
    }
    // bit 4 is the one radius every corner shares; bit 22 below is the
    // four, and a box sends one or the other, never both
    if (mask & 16) style.radius = `${f32()}px`;
    if (mask & 32) {
      const radius = f32();
      style.shadows.push(`0 0 ${radius}px ${rgba(u32())}`);
    }
    if (mask & 64) {
      const response = f32();
      f32(); // damping — the CSS side keeps the duration
      style.transition = `background-color ${response}s ease-out, transform ${response}s ease-out`;
    }
    // the action path, the tooltip and the group owned are the
    // element's own (ops 19 and 24): a look carries none, but the
    // record keeps their bits
    if (mask & 128) text(u16());
    if (mask & 256) style.focus = rgba(u32());
    if (mask & 512) style.placeholder = rgba(u32());
    if (mask & 1024) style.ink = rgba(u32());
    if (mask & 2048) style.hoverInk = rgba(u32());
    if (mask & 4096) style.pressedInk = rgba(u32());
    // a two-stop ramp: the geometry is the engine's, the pixels are
    // the browser's
    if (mask & 8192) {
      const kind = u8();
      const [a, b, c, d] = [f32(), f32(), f32(), f32()];
      const aspect = kind === 0 ? f32() : 1;
      const near = rgba(u32());
      const far = rgba(u32());
      if (kind === 0 && aspect !== 1 && d > 0) {
        // the ellipse: X radius on the wire, Y is X times the aspect
        style.image =
          `radial-gradient(ellipse ${d}px ${d * aspect}px at ` +
          `${a * 100}% ${b * 100}%, ${near} ${((c / d) * 100).toFixed(2)}%, ${far} 100%)`;
      } else if (kind === 0) {
        const reach = d < 0 ? "farthest-corner" : `${d}px`;
        const stop = d < 0 ? "100%" : `${d}px`;
        style.image =
          `radial-gradient(circle ${reach} at ` +
          `${a * 100}% ${b * 100}%, ${near} ${c}px, ${far} ${stop})`;
      } else {
        // CSS runs its line through the centre: the angle carries the
        // direction (0deg points up, clockwise)
        const degrees = (Math.atan2(c - a, -(d - b)) * 180) / Math.PI;
        style.image = `linear-gradient(${degrees.toFixed(2)}deg, ${near}, ${far})`;
      }
    }
    if (mask & 16384) style.clip = true;
    if (mask & 32768) text(u16());
    if (mask & 65536) style.fade = f32();
    if (mask & 131072) style.hoverFade = f32();
    if (mask & 262144) style.pressedFade = f32();
    // a box that follows a GROUP takes its states from an ANCESTOR's
    // pointer. The group crosses as a NUMBER: the browser needs an
    // anchor to hang a selector on, never the path a person reads
    if (mask & 524288) style.group = u64text(u32(), u32());
    if (mask & 1048576) {
      u32();
      u32();
    }
    if (mask & 2097152) style.passThrough = true;
    if (mask & 4194304) {
      // four corners, clockwise from the top left — the CSS order
      const tl = f32();
      const tr = f32();
      const br = f32();
      const bl = f32();
      style.radius = `${tl}px ${tr}px ${br}px ${bl}px`;
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
      style.filter = `blur(${blur}px) saturate(${saturation}) brightness(${brightness})`;
      if (band > 0) {
        const spread = Math.max(1, band);
        style.shadows.push(`inset ${spread}px ${spread}px ${spread * 1.5}px ${-spread}px ${rim}`);
        style.shadows.push(
          `inset ${-spread}px ${-spread}px ${spread * 1.5}px ${-spread}px ${rim}`,
        );
      }
    }
    return style;
  };
  const readLayout = () => {
    const mask = u16();
    const layout = {
      gap: null,
      align: null,
      padding: null,
      grow: (mask & 128) !== 0,
      stretch: (mask & 512) !== 0,
      fill: (mask & 1024) !== 0,
      wrap: null,
      plain: (mask & 4096) !== 0,
    };
    if (mask & 1) layout.gap = f32();
    if (mask & 2) layout.align = u8();
    if (mask & 4) layout.padding = [f32(), f32(), f32(), f32()];
    // a look carries no box of its own (op 23 does), but the record
    // keeps the bits
    if (mask & 8) f32();
    if (mask & 16) f32();
    if (mask & 32) f32();
    if (mask & 64) f32();
    if (mask & 256) f32();
    if (mask & 2048) layout.wrap = f32();
    return layout;
  };
  const readFace = () => {
    const color = rgba(u32());
    const inheritsInk = u8();
    const size = f32();
    const weight = CSS_WEIGHTS[u8()];
    const mono = u8();
    const italic = u8();
    const family = text(u16());
    const lineHeight = f32();
    const align = u8();
    const truncation = u8();
    // 1 = the face declared above is this text's: its look names no font
    const inheritsFace = u8();
    return {
      font: cssFont(size, weight, mono, italic, family),
      inheritsFace,
      lineHeight,
      align,
      color: inheritsInk ? null : color,
      truncation,
    };
  };

  // the words of an element: into the one text node it already holds
  // when it holds exactly that — a clone's cell, an updated label —
  // else by replacing the children
  const setWords = (el, words) => {
    const first = el.firstChild;
    if (first !== null && first.nodeType === 3 && first.nextSibling === null) {
      first.data = words;
    } else {
      el.textContent = words;
    }
  };
  // a removed subtree takes its registrations along: ids are never
  // reused, so a survivor here would leak for the page's whole life
  const unregister = (el) => {
    for (const inner of el.getElementsByTagName("*")) {
      const n = inner.__n;
      if (n === undefined) continue;
      elements.delete(n);
    }
  };
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
  // where a new element lands: before its anchor wherever that lives
  // (it may still sit in a staged fragment), or appended — staged when
  // the parent is live, so a thousand rows reach the tree once
  const place = (el, parent, before) => {
    settleInside(parent);
    const home = lookup(parent);
    if (!home) return;
    const anchor = before ? lookup(before) : null;
    if (anchor) {
      (anchor.parentNode ?? home).insertBefore(el, anchor);
    } else if (home.isConnected) {
      stagedFor(parent, home).appendChild(el);
    } else {
      home.appendChild(el);
    }
  };
  const count = u32();
  for (let i = 0; i < count; i++) {
    const op = u8();
    if (op === 21) {
      // a look, defined once: its two words, the kind it dresses, the
      // flags (bit 0: the element lays itself out — the table family),
      // its style, its flow record and, for a text, its face
      const hi = u32();
      const lo = u32();
      const kind = u8();
      const flags = u8();
      const style = readStyle();
      const layout = readLayout();
      const face = u8() ? readFace() : null;
      defineLook(hi, lo, kind, flags, style, layout, face);
      continue;
    }
    const id = u32();
    if (op === 1) {
      const parent = u32();
      const before = u32();
      const tag = text(u8());
      const cls = text(u8());
      const domId = text(u8());
      const kind = u8();
      const el = createElementOf(kind, tag);
      el.__n = id;
      el.__cls = cls;
      el.__look = "";
      if (cls) el.setAttribute("class", cls);
      if (domId) el.id = domId;
      if (kind === 4) {
        wireScroll(el, id);
      }
      if (kind === 6) {
        wireIsland(el, id);
      }
      place(el, parent, before);
      elements.set(id, el);
    } else if (op === 17) {
      // a shape already on the page: one deep clone of the live
      // instance, numbered in pre-order from the copy's own id — the
      // words and the action paths follow as their own ops
      const parent = u32();
      const before = u32();
      const source = lookup(u32());
      if (source) {
        const el = source.cloneNode(true);
        // a template the serializer stamped carries its number as an
        // attribute, and so does every element under it; the copy must
        // not wear them — such a copy is numbered node by node, the old
        // way. Any other copy registers its ROOT alone: the members are
        // found by their offset when an op asks for one
        if (source.hasAttribute("data-n")) {
          let n = id;
          const number = (node) => {
            node.__n = n;
            node.removeAttribute("data-n");
            elements.set(n, node);
            n++;
            for (const child of node.children) number(child);
          };
          number(el);
        } else {
          const templateId = source.__n;
          const trails = trailsOf(templateId, source);
          el.__n = id;
          el.__members = trails.length;
          elements.set(id, el);
          cloneRoots.push([id, trails.length, templateId]);
        }
        place(el, parent, before);
      }
    } else if (op === 19) {
      // the action path alone
      const el = lookup(id);
      const path = text(u16());
      if (el) {
        // the attribute written as one: the dataset door converts the
        // name and costs three times the write, two thousand times a page
        if (path) {
          el.setAttribute("data-path", path);
        } else {
          el.removeAttribute("data-path");
        }
      }
    } else if (op === 20) {
      // the words alone: the font and the ink already stand
      const el = lookup(id);
      const raw = bytes(u32());
      if (el) setWords(el, decoder.decode(raw));
    } else if (op === 2) {
      settleMember(id);
      const el = lookup(id);
      if (el) {
        unregister(el);
        el.remove();
      }
      elements.delete(id);
      forgetClone(id);
    } else if (op === 18) {
      // the element empties: every child leaves in one call, and the
      // ids that leave come as ranges — a thousand rows mounted
      // together are one — so the registry forgets them by counting,
      // never by walking the subtree
      settleInside(id);
      const el = lookup(id);
      const ranges = u16();
      const spans = [];
      let leaving = 0;
      for (let r = 0; r < ranges; r++) {
        const start = u32();
        const end = u32();
        spans.push(start, end);
        leaving += end - start;
        // the clone roots in the range leave the table: from the first
        // at or after the start, while before the end
        let at = cloneAt(start);
        if (at < 0 || cloneRoots[at][0] < start) at++;
        let gone = at;
        while (gone < cloneRoots.length && cloneRoots[gone][0] < end) gone++;
        if (gone > at) cloneRoots.splice(at, gone - at);
      }
      if (leaving * 2 > elements.size) {
        // most of the registry leaves: keep the survivors in one pass
        // over it, instead of a delete per id that leaves
        const kept = [];
        for (const entry of elements) {
          const n = entry[0];
          let gone = false;
          for (let i = 0; i < spans.length; i += 2) {
            if (n >= spans[i] && n < spans[i + 1]) {
              gone = true;
              break;
            }
          }
          if (!gone) kept.push(entry);
        }
        elements.clear();
        for (const [n, kept_el] of kept) elements.set(n, kept_el);
      } else {
        for (let i = 0; i < spans.length; i += 2) {
          for (let n = spans[i]; n < spans[i + 1]; n++) elements.delete(n);
        }
      }
      if (el) el.replaceChildren();
    } else if (op === 3) {
      const el = lookup(id);
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
      const el = lookup(id);
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
    } else if (op === 6) {
      // the words and their spans: the face is the look's
      const el = lookup(id);
      const raw = bytes(u32());
      const spanCount = u16();
      const spans = [];
      for (let s = 0; s < spanCount; s++) spans.push([u32(), u32()]);
      const spanColor = rgba(u32());
      if (el) {
        if (spanCount === 0) {
          // one write, into the text node that stands when one does
          setWords(el, decoder.decode(raw));
          continue;
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
      const el = lookup(id);
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
      const el = lookup(id);
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
      const el = lookup(id);
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
      const el = lookup(id);
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
    } else if (op === 22) {
      // the element wears a look: its class, after the page's own
      const hi = u32();
      const lo = u32();
      const el = lookup(id);
      if (el) {
        learn(el);
        el.__look = lookName(hi, lo);
        dress(el);
      }
    } else if (op === 23) {
      // the box the element owns — pinned sizes, ceilings, a virtual
      // row's slot — the record's semantics: what the mask does not
      // carry, the element does not keep; a bare element keeps nothing
      const el = lookup(id);
      const mask = u8();
      const width = mask & 1 ? f32() : null;
      const height = mask & 2 ? f32() : null;
      const maxWidth = mask & 4 ? f32() : null;
      const maxHeight = mask & 8 ? f32() : null;
      const slotY = mask & 16 ? f32() : null;
      if (el) {
        const style = el.style;
        const dressed = style.length !== 0;
        if (width !== null) style.width = `${width}px`;
        else if (dressed) style.width = "";
        if (height !== null) style.height = `${height}px`;
        else if (dressed) style.height = "";
        if (maxWidth !== null) style.maxWidth = `${maxWidth}px`;
        else if (dressed) style.maxWidth = "";
        if (maxHeight !== null) style.maxHeight = `${maxHeight}px`;
        else if (dressed) style.maxHeight = "";
        if (slotY !== null) {
          // a virtual row: absolute inside its relative content box
          style.position = "absolute";
          style.top = `${slotY}px`;
          style.left = "0";
          style.right = "0";
        } else if (dressed) {
          style.position = "";
          style.top = "";
          style.left = "";
          style.right = "";
        }
      }
    } else if (op === 24) {
      // the element's marks: the tooltip it shows, the group it owns
      const el = lookup(id);
      const mask = u8();
      const tip = mask & 1 ? text(u16()) : null;
      const owner = mask & 2 ? u64text(u32(), u32()) : null;
      if (el) {
        if (tip !== null) el.dataset.tip = tip;
        else if (el.dataset.tip !== undefined) delete el.dataset.tip;
        if (owner !== null) el.dataset.g = owner;
        else if (el.dataset.g !== undefined) delete el.dataset.g;
      }
    } else if (op === 12) {
      // one insertBefore, identity intact (0 = to the end)
      settleMember(id);
      const el = lookup(id);
      const parentId = u32();
      settleInside(parentId);
      const parent = lookup(parentId);
      const before = u32();
      if (el && parent) parent.insertBefore(el, before ? (lookup(before) ?? null) : null);
    } else if (op === 13) {
      // the browser computes the offset — dense lists only
      const target = lookup(u32());
      if (target) target.scrollIntoView({ block: "nearest" });
    } else if (op === 15) {
      // live hints: class and id re-attribute in place
      const cls = text(u8());
      const domId = text(u8());
      const el = lookup(id);
      if (el) {
        learn(el);
        el.__cls = cls;
        dress(el);
        if (domId) {
          el.id = domId;
        } else {
          el.removeAttribute("id");
        }
      }
    } else if (op === 16) {
      // the iframe navigates: the diff only ships a CHANGED src, so
      // the write is the navigation (the same one would reload).
      // Sealed, the frame holds a DOCUMENT instead of a url: the
      // browser's sandbox with no powers, the page's own policy at
      // its head, and the page itself as `srcdoc` — the sandbox is
      // set before the document lands, because it is read at the load
      const sealed = u8() === 1;
      const src = text(u32());
      const el = lookup(id);
      if (el) {
        if (sealed) {
          el.setAttribute("sandbox", "");
          el.removeAttribute("src");
          el.srcdoc = src;
        } else {
          el.removeAttribute("sandbox");
          el.removeAttribute("srcdoc");
          el.src = src;
        }
      }
    } else if (op === 25) {
      // the video's whole record. The fit, the mirror and the radius
      // are writes; the stream is a REWIRE, performed only when the
      // handle changed — setting srcObject again restarts the playback.
      // The mirror rides the `scale` property and not `transform`,
      // which the layout record resets on every change of the box
      const stream = u32();
      const mirrored = u8() === 1;
      const cover = u8() === 1;
      const radius = f32();
      const el = lookup(id);
      if (el) {
        el.style.objectFit = cover ? "cover" : "contain";
        el.style.scale = mirrored ? "-1 1" : "";
        el.style.borderRadius = radius > 0 ? `${radius}px` : "";
        if (el.__stream !== stream) {
          el.__stream = stream;
          el.srcObject = typeof bunnyMedia === "object" && stream ? bunnyMedia.get(stream) : null;
          el.play().catch(() => {});
        }
      }
    } else if (op === 14) {
      // the popover's anchor relation — position now, and again
      // whenever anything scrolls or the window resizes
      const anchor = u32();
      const side = u8();
      const path = text(u16());
      const el = lookup(id);
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
}

// The popover placement: the browser owns the boxes, so the browser
// positions the card — preferred side, flip when it does not fit,
// then a two-axis clamp into the root. The engine's flip-then-clamp
// policy, in the coordinate system that owns it here.
const POPOVER_GAP = 6;

function placePopover(el) {
  const anchorEl = lookup(Number(el.dataset.anchor));
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
function bunnyGlStubsOrNothing() {
  const stubs = {};
  for (const name of GPU_VERBS) stubs[name] = () => 0;
  return stubs;
}

const GPU_VERBS = [
  "gl_init", "gl_log", "gl_island_blit", "gl_now", "gl_teardown", "gl_resize",
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

// Keyed by the modules' RELATIVE names — see glue.js on why.
const imports = {
  "./bunny_gpu.js":
    typeof bunnyGlImports === "object" ? bunnyGlImports : bunnyGlStubsOrNothing(),
  "./bunny.js": {
    js_blit() {},
    // the page's clock, for the engine's stage timers
    js_now() {
      return performance.now();
    },
    // the canvas shell's host overlay: this page lowers a host to an
    // element of its own (kind 15), so the three verbs answer nothing
    js_host_begin() {},
    js_host_video() {},
    js_host_end() {},
    // a focused island copied: the same road the canvas shell takes
    js_clipboard_write(pointer, length) {
      const text = decoder.decode(new Uint8Array(wasm.memory.buffer, pointer, length));
      if (navigator.clipboard) navigator.clipboard.writeText(text).catch(() => {});
    },
    // the browser owns hover in this mode; the shell never asks
    js_set_cursor() {},
    // The loop clocks' driver. Springs are the browser's here (a spec
    // lowers to a CSS transition), so this only ever runs while a
    // `.looping(…)` box is alive — and only when the reader allows
    // motion at all.
    js_request_frame() {
      if (frameArmed) return;
      frameArmed = true;
      requestAnimationFrame((now) => {
        frameArmed = false;
        const dt = lastFrame ? (now - lastFrame) / 1000 : 0;
        lastFrame = now;
        wasm.bunny_frame(dt);
      });
    },
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
        armIdle();
        return;
      }
      const opened = performance.now();
      applyPatches(view, length);
      const box = (window.__bunnyApply ||= { ms: 0, batches: 0 });
      box.ms += performance.now() - opened;
      box.batches += 1;
      armIdle();
    },
    // the pixels of the rect that changed inside one canvas island,
    // straight onto its element at the rect's place — the first frame
    // and a resize bring the whole box
    js_island_rect(id, pointer, width, height, x, y, dirtyWidth, dirtyHeight) {
      const el = lookup(id);
      if (!el || el.tagName !== "CANVAS") return;
      if (el.width !== width) el.width = width;
      if (el.height !== height) el.height = height;
      const pixels = new Uint8ClampedArray(
        wasm.memory.buffer,
        pointer,
        dirtyWidth * dirtyHeight * 4,
      );
      el.getContext("2d").putImageData(new ImageData(pixels, dirtyWidth, dirtyHeight), x, y);
    },
    // A panic on its way out of wasm: decode the message and log it, so
    // an abort is a sentence instead of `unreachable` and a stack of
    // numbers.
    js_panic(pointer, length) {
      const bytes = new Uint8Array(wasm.memory.buffer, pointer, length);
      console.error("bunny panic: " + decoder.decode(bytes));
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
    window.__bunnyDebug = { elements, looks, cloneRoots };
    // probe builds: the hit table of the last layout, for a runner
    // that clicks what a page without elements cannot select
    if (wasm.bunny_hits_json && wasm.bunny_probe_ptr) {
      window.__bunnyHits = () => {
        const len = wasm.bunny_hits_json() >>> 0;
        const ptr = wasm.bunny_probe_ptr() >>> 0;
        return JSON.parse(new TextDecoder().decode(new Uint8Array(wasm.memory.buffer, ptr, len)));
      };
    }
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
        el.__n = id;
        elements.set(id, el);
        if (el.tagName === "INPUT") wireInput(el);
        // a scroll box wears its kind as a mark: its look is a class,
        // which says nothing to a hydration that must wire the wheel
        if (el.dataset.k === "4") wireScroll(el, id);
        if (el.tagName === "CANVAS") wireIsland(el, id);
      }
    }
    // the boot bill: fetch+instantiate, then the first frame inside
    // start_dom — the two numbers a mount argument needs
    // `?present=gpu`: the GPU tier takes the page whatever its own probe of
    // the device says (and `?present=cpu` keeps it off the page)
    if (new URLSearchParams(location.search).get("present") === "gpu" && wasm.bunny_gpu_forced) {
      wasm.bunny_gpu_forced();
    }
    if (STATS && wasm.bunny_stats_enable) {
      // the engine's own stage table beside the glue's apply time: one
      // read drains both, so a reader times one operation at a time
      wasm.bunny_stats_enable();
      window.__bunnyStats = () => {
        wasm.bunny_stats_take();
        const stage = (i) => +wasm.bunny_stats_stage(i).toFixed(3);
        const count = (i) => wasm.bunny_stats_counter(i) >>> 0;
        const apply = window.__bunnyApply || { ms: 0, batches: 0 };
        window.__bunnyApply = { ms: 0, batches: 0 };
        return {
          settle: stage(0), capture: stage(2), diff: stage(3), encode: stage(4),
          pass: stage(5), assemble: stage(6),
          apply: +apply.ms.toFixed(3), batches: apply.batches,
          passes: count(0), built: count(3), visited: count(4), reused: count(5),
          patches: count(6), bytes: count(7), indexed: count(11), bound: count(12),
        };
      };
    }
    window.__bunnyBoot = { instantiate: performance.now() - bootOpened };
    const startOpened = performance.now();
    wasm[START_EXPORT](
      app.clientWidth,
      app.clientHeight,
      window.devicePixelRatio || 1,
      hydrated ? 1 : 0,
    );
    window.__bunnyBoot.start = performance.now() - startOpened;

    // Is motion welcome? The PLATFORM answers, not this file: a reader
    // who asked their system for less motion gets the resting frame, and
    // one who did not gets the clocks. The engine drives only the loops
    // either way — every spring here is a CSS transition already.
    if (wasm.bunny_set_motion) {
      const query = matchMedia("(prefers-reduced-motion: reduce)");
      wasm.bunny_set_motion(query.matches ? 0 : 1);
      query.addEventListener("change", (event) => {
        if (wasm) wasm.bunny_set_motion(event.matches ? 0 : 1);
      });
    }

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
    // a modifier's release types nothing and makes no stroke: the
    // state it leaves is the whole event
    window.addEventListener("keyup", (event) => {
      if (MODIFIER_KEYS.has(event.key) && wasm && wasm.bunny_modifiers) {
        wasm.bunny_modifiers(modifiers(event));
      }
    });
    // the browser owns the <input>s in this mode. What still belongs
    // to the engine: Escape (the keymap dismisses the popover) and
    // every stroke a focused canvas island wants — a box the app
    // paints has no element to type into.
    window.addEventListener("keydown", (event) => {
      const typing = event.target && event.target.tagName === "INPUT";
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
