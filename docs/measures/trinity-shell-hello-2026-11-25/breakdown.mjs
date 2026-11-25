import fs from "node:fs";
const b = fs.readFileSync(process.argv[2]);
let at = 8;
function leb() { let r = 0, s = 0, x; do { x = b[at++]; r += (x & 0x7f) * 2 ** s; s += 7; } while (x & 0x80); return r; }
const sections = {}; let code = null, names = new Map(), imports = 0;
while (at < b.length) {
  const id = b[at++]; const size = leb(); const start = at;
  let name = ["custom","type","import","function","table","memory","global","export","start","element","code","data","datacount"][id];
  if (id === 0) { const n = leb(); name = "custom:" + b.toString("utf8", at, at + n); at += n;
    if (name === "custom:name") { while (at < start + size) { const sub = b[at++]; const ss = leb(); const se = at + ss;
      if (sub === 1) { const c = leb(); for (let i = 0; i < c; i++) { const idx = leb(); const l = leb(); names.set(idx, b.toString("utf8", at, at + l)); at += l; } }
      at = se; } } }
  if (id === 2) { imports = leb(); }
  if (id === 10) { const c = leb(); code = []; for (let i = 0; i < c; i++) { const s = leb(); code.push(s); at += s; } }
  sections[name] = (sections[name] || 0) + size; at = start + size;
}
console.log("sections", sections);
const byCrate = {};
code.forEach((s, i) => { const n = names.get(i + imports) || "?"; const m = n.match(/^_ZN(\d+)(\w+?)/) ; let crate = "?";
  if (m) { const len = +m[1]; crate = n.substr(3 + m[1].length, len); } else if (n.startsWith("_R")) { const v = n.match(/C(?:s[0-9a-zA-Z]*_)?(\d+)([a-z_][a-z0-9_]*)/); crate = v ? v[2].slice(0, +v[1]) : "v0"; } else crate = n.split(/[:<]/)[0].slice(0,40);
  crate = crate.replace(/17h[0-9a-f]{16}E$/, "");
  byCrate[crate] = (byCrate[crate] || 0) + s; });
console.log(Object.entries(byCrate).sort((a, b) => b[1] - a[1]).slice(0, 15));
const top = code.map((s, i) => [s, names.get(i + imports)]).sort((a, b) => b[0] - a[0]).slice(0, 12);
console.log(top.map(([s, n]) => s + " " + (n || "").slice(0, 110)).join("\n"));
