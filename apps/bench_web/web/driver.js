// The driver: dispatches real pointer events at the bench table and
// times the full input → state → patches → elements path. The house
// protocol shapes the run: the operations interleave across rounds
// and a cooldown separates them, so no scenario heats the machine
// for the one after it. Results land in `window.__benchResults` and
// on the console as one table.
//
// Run it from the console: `await __bench.all()`.

const SAMPLES_PER_ROUND = 11;
const ROUNDS = 5;
const COOLDOWN_MS = 2000;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function interactive() {
  return [...document.querySelectorAll("#app [data-path]")];
}

function control(name) {
  const el = interactive().find((el) => el.dataset.path.includes(`[${name}]`));
  if (!el) throw new Error(`no control named ${name}`);
  return el;
}

function rowEls() {
  return interactive().filter(
    (el) =>
      !el.dataset.path.includes("[toggle_all]") &&
      !el.dataset.path.includes("[filter]"),
  );
}

// A real click: pointerdown + pointerup at the element's centre,
// bubbling to #app the way a finger would.
function clickOn(el) {
  const rect = el.getBoundingClientRect();
  const options = {
    bubbles: true,
    clientX: rect.left + rect.width / 2,
    clientY: rect.top + rect.height / 2,
  };
  el.dispatchEvent(new PointerEvent("pointerdown", options));
  el.dispatchEvent(new MouseEvent("click", options));
}

// The update path applies patches synchronously, so the bracket
// around the dispatch IS the whole road.
function timed(action) {
  const opened = performance.now();
  action();
  return performance.now() - opened;
}

function summarize(samples) {
  const sorted = [...samples].sort((a, b) => a - b);
  const at = (q) =>
    sorted[Math.min(Math.floor(sorted.length * q), sorted.length - 1)];
  return {
    p50: +at(0.5).toFixed(3),
    p95: +at(0.95).toFixed(3),
    max: +sorted[sorted.length - 1].toFixed(3),
    samples: sorted.length,
  };
}

const OPS = {
  "toggle 1 row": () => clickOn(rowEls()[5]),
  "toggle all 200": () => clickOn(control("toggle_all")),
  "filter 200 → 10": () => {
    if (rowEls().length !== 200) throw new Error("expected the full table");
    clickOn(control("filter"));
  },
  "filter 10 → 200": () => {
    if (rowEls().length !== 10) throw new Error("expected the filtered table");
    clickOn(control("filter"));
  },
};

// Walks the table to the state an operation expects, untimed.
function prepare(label) {
  if (label === "filter 200 → 10" && rowEls().length !== 200) {
    clickOn(control("filter"));
  }
  if (label === "filter 10 → 200" && rowEls().length !== 10) {
    clickOn(control("filter"));
  }
}

async function all() {
  const results = Object.fromEntries(Object.keys(OPS).map((k) => [k, []]));
  const order = Object.keys(OPS);
  for (let round = 0; round < ROUNDS; round++) {
    // rotate the order each round: interleaved, nobody rides a warm tail
    const rotated = order.slice(round % order.length).concat(order.slice(0, round % order.length));
    for (const label of rotated) {
      for (let i = 0; i < SAMPLES_PER_ROUND; i++) {
        prepare(label);
        results[label].push(timed(OPS[label]));
      }
      prepare("filter 10 → 200"); // leave the full table behind
    }
    window.__benchProgress = `round ${round + 1}/${ROUNDS}`;
    await sleep(COOLDOWN_MS);
  }

  // sustained: complete input → element frames in one second
  prepare("filter 10 → 200");
  const row = rowEls()[5];
  const deadline = performance.now() + 1000;
  let sustained = 0;
  while (performance.now() < deadline) {
    clickOn(row);
    sustained += 1;
  }
  // no rAF here: a hidden pane never paints, and the numbers must
  // not depend on the pane being watched

  const table = Object.fromEntries(
    Object.entries(results).map(([label, samples]) => [label, summarize(samples)]),
  );
  table["sustained toggles/sec"] = { p50: sustained };
  table["boot (instantiate / start)"] = {
    p50: +window.__bunnyBoot.instantiate.toFixed(3),
    p95: +window.__bunnyBoot.start.toFixed(3),
  };
  window.__benchResults = table;
  console.table(table);
  return table;
}

window.__bench = { all, clickOn, rowEls, control };
