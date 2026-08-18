// The keyed operations, timed the house way: real clicks on the ids
// the official harness uses, interleaved rounds, a cooldown between
// them. Run `await __keyed.all()` from the console.

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

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

function rowCount() {
  return document.querySelectorAll("#app tr").length;
}

function timed(action) {
  const opened = performance.now();
  action();
  return performance.now() - opened;
}

async function all() {
  const results = {};
  const measure = (label, action, samples = 10) => {
    const bucket = (results[label] ||= []);
    for (let i = 0; i < samples; i++) {
      bucket.push(timed(action));
    }
  };

  for (let round = 0; round < 3; round++) {
    press("clear");
    measure("create 1k", () => press("run"), 5);
    measure("update 10th", () => press("update"), 10);
    measure("select", () => {
      const label = document.querySelectorAll("#app tr a")[6];
      if (label) {
        const rect = label.getBoundingClientRect();
        label.dispatchEvent(
          new PointerEvent("pointerup", {
            bubbles: true,
            clientX: rect.left + 3,
            clientY: rect.top + 3,
          }),
        );
      }
    }, 10);
    measure("swap", () => press("swaprows"), 10);
    measure("append 1k", () => press("add"), 3);
    measure("clear", () => press("clear"), 3);
    press("runlots");
    measure("create 10k (once)", () => {}, 1);
    results["create 10k rows"] ||= [];
    press("clear");
    measure("create 10k rows", () => press("runlots"), 2);
    press("clear");
    await sleep(2000);
  }

  const table = {};
  for (const [label, samples] of Object.entries(results)) {
    const sorted = [...samples].sort((a, b) => a - b);
    table[label] = {
      p50: +sorted[Math.floor(sorted.length / 2)].toFixed(2),
      max: +sorted[sorted.length - 1].toFixed(2),
      samples: sorted.length,
    };
  }
  table.rows = { p50: rowCount() };
  window.__keyedResults = table;
  console.table(table);
  return table;
}

window.__keyed = { all, press, rowCount };
