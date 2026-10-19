// What `bunny run -d web` adds to the page, before the app's glue: the
// page reloads when a new build lands, and its console errors — a wasm
// panic among them — show in the terminal that started it.
(() => {
  const send = (level, parts) => {
    try {
      navigator.sendBeacon("/__bunny/log", level + " " + parts.map(String).join(" "));
    } catch (_) {}
  };
  for (const level of ["error", "warn"]) {
    const original = console[level];
    console[level] = (...parts) => {
      send(level, parts);
      original.apply(console, parts);
    };
  }
  addEventListener("error", (event) => {
    send("error", [`${event.message} (${event.filename}:${event.lineno})`]);
  });
  addEventListener("unhandledrejection", (event) => {
    const reason = event.reason;
    send("error", ["unhandled: " + ((reason && reason.stack) || reason)]);
  });
  let build = null;
  const poll = () =>
    fetch("/__bunny/build" + (build === null ? "" : "?since=" + build), { cache: "no-store" })
      .then((response) => response.text())
      .then((text) => {
        const next = Number(text);
        if (build !== null && next !== build) {
          location.reload();
          return;
        }
        build = next;
        poll();
      })
      // the server is gone (bunny run stopped) or restarting: try again
      .catch(() => setTimeout(poll, 1000));
  poll();
})();
