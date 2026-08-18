// The keyed operations, measured the OFFICIAL way: every sample runs
// on a freshly loaded page — the harness this mirrors reloads between
// benchmarks, and so does this driver, carrying its progress in
// sessionStorage. Kick it with `__keyed.suite()`; read the table in
// `__keyedResults` when the page stops reloading.

const ITERATIONS = 5;

function press(id) {
  const el = document.getElementById(id);
  const rect = el.getBoundingClientRect();
  el.dispatchEvent(
    new PointerEvent("pointerup", {
      bubbles: true,
      clientX: rect.left + 4,
      clientY: rect.top + 4,
    }),
  );
}

function timed(action) {
  const opened = performance.now();
  action();
  return performance.now() - opened;
}

// Each scenario: `prep` runs untimed on the fresh page, `run` is the
// one measured action — the official harness's own shape.
const SCENARIOS = {
  "create 1k": { prep: () => {}, run: () => press("run") },
  "replace 1k": { prep: () => press("run"), run: () => press("run") },
  "update every 10th": { prep: () => press("run"), run: () => press("update") },
  "select row": {
    prep: () => press("run"),
    run: () => {
      const label = document.querySelectorAll("#app tr a")[10];
      const rect = label.getBoundingClientRect();
      label.dispatchEvent(
        new PointerEvent("pointerup", {
          bubbles: true,
          clientX: rect.left + 3,
          clientY: rect.top + 3,
        }),
      );
    },
  },
  "swap rows": { prep: () => press("run"), run: () => press("swaprows") },
  "append 1k": { prep: () => press("run"), run: () => press("add") },
  "clear 1k": { prep: () => press("run"), run: () => press("clear") },
  "create 10k": { prep: () => {}, run: () => press("runlots") },
};

function schedule() {
  const labels = Object.keys(SCENARIOS);
  const plan = [];
  for (const label of labels) {
    for (let i = 0; i < ITERATIONS; i++) {
      plan.push(label);
    }
  }
  return plan;
}

async function ready() {
  while (!window.__bunny || !document.getElementById("run")) {
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

async function step() {
  const state = JSON.parse(sessionStorage.getItem("keyedSuite") || "null");
  if (!state) return;
  if (state.at >= state.plan.length) {
    sessionStorage.removeItem("keyedSuite");
    const table = {};
    for (const [label, samples] of Object.entries(state.results)) {
      const sorted = [...samples].sort((a, b) => a - b);
      table[label] = {
        p50: +sorted[Math.floor(sorted.length / 2)].toFixed(1),
        max: +sorted[sorted.length - 1].toFixed(1),
        samples: sorted.length,
      };
    }
    window.__keyedResults = table;
    document.title = "keyed DONE";
    console.table(table);
    return;
  }
  await ready();
  const label = state.plan[state.at];
  const scenario = SCENARIOS[label];
  scenario.prep();
  // one macrotask so the prep's layout settles before the sample
  await new Promise((resolve) => setTimeout(resolve, 50));
  const sample = timed(scenario.run);
  (state.results[label] ||= []).push(sample);
  state.at += 1;
  sessionStorage.setItem("keyedSuite", JSON.stringify(state));
  location.reload();
}

function suite() {
  sessionStorage.setItem(
    "keyedSuite",
    JSON.stringify({ at: 0, plan: schedule(), results: {} }),
  );
  location.reload();
}

window.__keyed = { suite, press };
step();
