#!/usr/bin/env node
// The keyed operations on any page, timed the official way: a fresh
// page per sample, the page's own warm-ups, a CPU throttle per
// operation, a trace from the click to the last paint, and the
// duration computed by the harness's own timeline code when a harness
// checkout is given (a port of its rule otherwise).
//
//   node three_modes.mjs --chrome <binary> [--harness <js-framework-benchmark dir>]
//        --page dom=http://host/dom.html --page hybrid=http://host/hybrid.html:hybrid
//        --page gpu=http://host/gpu.html:gpu [--count 5] [--ops 1,4,9] [--out traces]
//
// A page is `name=url[:kind]`; kind is `dom` (controls and rows are
// elements), `hybrid` (elements around canvas islands) or `gpu` (one
// canvas). Rows on a canvas are found through the page's hit table
// (`window.__bunnyHits`, probe builds).
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";

const args = process.argv.slice(2);
const flag = (name, fallback) => {
  const at = args.indexOf(`--${name}`);
  return at >= 0 ? args[at + 1] : fallback;
};
const chrome = flag("chrome");
const harness = flag("harness", process.env.BUNNY_HARNESS);
const count = +flag("count", 5);
const out = flag("out", "traces");
const only = (flag("ops", "") || "").split(",").filter(Boolean).map(Number);
const pages = [];
for (let i = 0; i < args.length; i++) {
  if (args[i] === "--page") {
    const spec = args[++i];
    const eq = spec.indexOf("=");
    const name = spec.slice(0, eq);
    let url = spec.slice(eq + 1);
    let kind = "dom";
    const m = url.match(/^(.*):(dom|hybrid|gpu)$/);
    if (m) {
      url = m[1];
      kind = m[2];
    }
    pages.push({ name, url, kind });
  }
}
if (!chrome || pages.length === 0) {
  console.error("usage: three_modes.mjs --chrome <binary> --page name=url[:kind] ... [--harness dir] [--count n] [--ops 1,4] [--out dir]");
  process.exit(2);
}

// puppeteer-core and the timeline code come from the harness checkout
// when one is given; the harness pins the Chrome build its puppeteer
// drives, so the two travel together
const requireFrom = createRequire(harness ? path.join(harness, "webdriver-ts", "package.json") : import.meta.url);
const puppeteer = requireFrom("puppeteer-core");
let computeResultsCPU = null;
if (harness) {
  const timeline = await import(pathToFileURL(path.join(harness, "webdriver-ts", "dist", "timeline.js")).href);
  computeResultsCPU = timeline.computeResultsCPU;
}

const RUN = "#run";
const CLEAR = "#clear";
const cycles = (n) => Array(n).fill(["#run", "#clear"]).flat();
// each operation: its id, the warm-ups before the measured click (the
// harness's own), the target, and the throttle the harness applies
const OPS = [
  { id: 1, label: "create 1k", prep: cycles(5), target: RUN, throttle: 1, done: { rows: 1000 } },
  { id: 2, label: "replace 1k", prep: ["#run", "#run", "#run", "#run", "#run"], target: RUN, throttle: 1, done: { rows: 1000 } },
  { id: 3, label: "update 10th", prep: ["#run", "#update", "#update", "#update"], target: "#update", throttle: 4, done: {} },
  { id: 4, label: "select", prep: ["#run", { row: 5, cell: "label" }, { row: 6, cell: "label" }, { row: 7, cell: "label" }, { row: 8, cell: "label" }, { row: 9, cell: "label" }], target: { row: 2, cell: "label" }, throttle: 4, done: {} },
  { id: 5, label: "swap", prep: ["#run", "#swaprows", "#swaprows", "#swaprows", "#swaprows", "#swaprows"], target: "#swaprows", throttle: 4, done: {} },
  { id: 6, label: "remove", prep: ["#run", { row: 4, cell: "remove" }, { row: 4, cell: "remove" }, { row: 4, cell: "remove" }, { row: 4, cell: "remove" }, { row: 4, cell: "remove" }], target: { row: 4, cell: "remove" }, throttle: 2, done: {} },
  { id: 7, label: "create 10k", prep: cycles(5), target: "#runlots", throttle: 1, done: { rows: 10000 } },
  { id: 8, label: "append 1k", prep: [...cycles(5), "#run"], target: "#add", throttle: 1, done: { rows: 2000 } },
  { id: 9, label: "clear", prep: [...cycles(5), "#run"], target: CLEAR, throttle: 4, done: { rows: 0 } },
];
const CATEGORIES = ["disabled-by-default-v8.cpu_profiler", "blink.user_timing", "devtools.timeline", "disabled-by-default-devtools.timeline"];
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const median = (xs) => { const s = [...xs].sort((a, b) => a - b); return s[s.length >> 1]; };

// the duration rule, ported: from the anchoring event's dispatch to
// the end of the last paint on its thread, and the script within it
function portedDuration(file, anchor) {
  const raw = JSON.parse(fs.readFileSync(file, "utf8"));
  const events = raw.traceEvents || raw;
  const start = events.find((e) => e.name === "TracingStartedInBrowser");
  const click = events
    .filter((e) => e.name === "EventDispatch" && e.args?.data?.type === anchor && e.ph === "X" && (!start || e.ts >= start.ts))
    .sort((a, b) => a.ts - b.ts)[0];
  if (!click) return null;
  const main = events.filter((e) => e.pid === click.pid && e.tid === click.tid && e.ph === "X" && e.ts >= click.ts).sort((a, b) => a.ts - b.ts || b.dur - a.dur);
  const paints = main.filter((e) => e.name === "Paint" || e.name === "Commit" || e.name === "CompositeLayers");
  const last = paints.length ? Math.max(...paints.map((e) => e.ts + e.dur)) : click.ts + click.dur;
  let script = 0;
  let openEnd = -Infinity;
  for (const e of main) {
    if (e.ts >= last) break;
    if (e.ts + e.dur <= openEnd) continue;
    openEnd = e.ts + e.dur;
    if (["EventDispatch", "FunctionCall", "EvaluateScript", "TimerFire", "FireAnimationFrame", "RunMicrotasks"].includes(e.name)) script += e.dur / 1000;
  }
  return { total: (last - click.ts) / 1000, script };
}

// where a target is on the page: an element's center for a selector
// in the modes that have the element, a point from the hit table for
// a row on a canvas
async function locate(page, kind, target) {
  if (typeof target === "string") {
    const box = await page.evaluate((sel) => {
      const el = document.querySelector(sel);
      if (!el) return null;
      const r = el.getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
    }, target);
    if (box) return box;
    if (kind !== "gpu") throw new Error(`no element ${target}`);
    // a chip on a canvas: its identity carries the same name
    return hitPoint(page, { name: target.slice(1) }, 0);
  }
  const { row, cell } = target;
  if (kind === "dom") {
    const sel = cell === "label" ? `tbody>tr:nth-of-type(${row})>td:nth-of-type(2)>a` : `tbody>tr:nth-of-type(${row})>td:nth-of-type(3)>a`;
    return locate(page, kind, sel);
  }
  // a row on a canvas: its cells carry the names `label` and `remove`
  return hitPoint(page, { name: cell }, row - 1);
}

// the nth hit (top to bottom) whose path carries the needle: a chip by
// its name, a row's cell by the suffix of its path
async function hitPoint(page, needle, nth) {
  return page.evaluate(({ nth, needle }) => {
    const accept = (path) =>
      needle.suffix ? path.endsWith(needle.suffix) : path.includes(`[${needle.name}]`) || path.includes(`@${needle.name}`) || path.includes(`/${needle.name}/`);
    const probe = window.__bunnyHits();
    // the window's own targets, and the targets on each island (in the
    // island's canvas coordinates) — one list, top to bottom
    const all = [
      ...probe.hits.map((h) => ({ ...h, island: null })),
      ...(probe.islandHits || []),
    ].filter((h) => accept(h.path));
    const place = (h) => {
      const cx = h.x + h.w / 2;
      const cy = h.y + h.h / 2;
      if (h.island !== null && h.island !== undefined && window.__bunnyDebug) {
        const canvas = window.__bunnyDebug.elements.get(h.island);
        const r = canvas.getBoundingClientRect();
        return { x: r.left + cx, y: r.top + cy };
      }
      const host = document.getElementById("app").getBoundingClientRect();
      return { x: host.left + cx, y: host.top + cy };
    };
    const placed = all.map(place).sort((a, b) => a.y - b.y || a.x - b.x);
    return placed[nth] || null;
  }, { nth, needle });
}

async function click(page, kind, target) {
  const at = await locate(page, kind, target);
  if (!at) throw new Error(`no target ${JSON.stringify(target)}`);
  await page.mouse.click(at.x, at.y);
}

// where the page's work starts for a target: an element answers the
// click, a canvas answers the pointer's release — the trace window
// opens at that event
function anchorOf(kind, target) {
  if (kind === "gpu") return "pointerup";
  if (kind === "hybrid" && typeof target !== "string") return "pointerup";
  return "click";
}

async function ready(page) {
  await page.waitForFunction(() => {
    if (!document.querySelector("#run") && !window.__bunny) return false;
    if (window.__bunnyBoot && window.__bunnyBoot.start === undefined) return false;
    return true;
  }, { timeout: 20000 });
  await wait(150);
}

// the page settles without the runner driving frames: a frame the
// runner asked for would be a commit the trace counts as the end
async function settled(page, kind, done) {
  if (kind === "dom" && done.rows !== undefined) {
    await page.waitForFunction((rows) => document.querySelectorAll("tbody>tr").length === rows, { timeout: 20000, polling: "mutation" }, done.rows);
  }
  await wait(kind === "dom" ? 100 : 150);
}

fs.mkdirSync(out, { recursive: true });
const browser = await puppeteer.launch({ executablePath: chrome, headless: "new", args: ["--window-size=1280,800", "--js-flags=--expose-gc", "--no-first-run", "--disable-extensions"] });
const results = {};
for (const op of OPS) {
  if (only.length && !only.includes(op.id)) continue;
  for (const spec of pages) {
    const samples = [];
    for (let i = 0; i < count; i++) {
      const page = await browser.newPage();
      await page.setViewport({ width: 1280, height: 800 });
      try {
        // `load` and the page's own readiness: a page that keeps a
        // connection open would never be idle for the network's rule
        await page.goto(spec.url, { waitUntil: "load" });
        await ready(page);
        for (const step of op.prep) {
          await click(page, spec.kind, step);
          await settled(page, spec.kind, {});
        }
        if (op.throttle > 1) await page.emulateCPUThrottling(op.throttle);
        const file = path.join(out, `${spec.name}_${String(op.id).padStart(2, "0")}_${i}.json`);
        await page.tracing.start({ path: file, screenshots: false, categories: CATEGORIES });
        await wait(50);
        await page.evaluate(() => window.gc && window.gc());
        await click(page, spec.kind, op.target);
        await settled(page, spec.kind, op.done);
        await page.tracing.stop();
        if (op.throttle > 1) await page.emulateCPUThrottling(1);
        let total = null;
        const anchor = anchorOf(spec.kind, op.target);
        if (computeResultsCPU) {
          try {
            const r = await computeResultsCPU(file, anchor);
            total = r.duration;
          } catch (error) {
            console.error(`  ${spec.name} ${op.label} #${i}: ${String(error).slice(0, 120)}`);
          }
        }
        const ported = portedDuration(file, anchor);
        samples.push({ total: total ?? ported?.total ?? NaN, script: ported?.script ?? NaN });
      } catch (error) {
        // one sample lost is one sample: the run goes on and says so
        console.error(`  ${spec.name} ${op.label} #${i}: ${String(error).slice(0, 160)}`);
        samples.push({ total: NaN, script: NaN });
      } finally {
        await page.close().catch(() => {});
      }
    }
    const good = samples.filter((s) => Number.isFinite(s.total));
    const t = good.length ? median(good.map((s) => s.total)) : NaN;
    const sc = good.length ? median(good.map((s) => s.script)) : NaN;
    (results[op.label] ||= {})[spec.name] = { total: t, script: sc, samples: samples.map((s) => +s.total.toFixed(1)) };
    console.log(`${op.label.padEnd(12)} ${spec.name.padEnd(8)} total ${t.toFixed(1).padStart(7)} ms  script ${sc.toFixed(1).padStart(6)}   [${samples.map((s) => s.total.toFixed(1)).join(" ")}]`);
  }
}
await browser.close();
fs.writeFileSync(path.join(out, "three_modes.json"), JSON.stringify(results, null, 2));
