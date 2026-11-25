// Measures a block built for the Trinity shell, in V8 (node), with an
// empty import object — the zero-import sandbox a Pure-tier host gives it.
//
//   cargo build -p hello-block --target wasm32-unknown-unknown --profile web
//   node scripts/measure-hello-block.mjs target/wasm32-unknown-unknown/web/hello_block.wasm [answers-dir]
//
// It prints the module's size, its imports (there must be none), and the
// median of RUNS fresh starts: compile, instantiate, the handshake's turn
// (the offer answered with the first scene) and a press's turn. With an
// answers dir it also writes the bytes of both answers, for
// `HELLO_BLOCK_ANSWERS=<dir> cargo test -p hello-block -- --ignored`,
// which paints them and holds them to the native render.

import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";

const [wasmPath, answersDir] = process.argv.slice(2);
if (!wasmPath) {
  console.error("usage: node scripts/measure-hello-block.mjs <block.wasm> [answers-dir]");
  process.exit(2);
}
const RUNS = 30;
const bytes = fs.readFileSync(wasmPath);

// --- the protocol, as much of it as a measurement needs -------------------
function frame(kind, payload) {
  const out = Buffer.alloc(5 + payload.length);
  out.writeUInt32LE(payload.length + 1, 0);
  out.writeUInt8(kind, 4);
  payload.copy(out, 5);
  return out;
}
function offer({ width, height, scale, dark }) {
  const p = Buffer.alloc(4 + 4 + 4 + 8 + 8 + 8 + 8 + 1);
  p.write("BNYS", 0, "latin1");
  p.writeUInt32LE(1, 4); // oldest
  p.writeUInt32LE(1, 8); // newest
  p.writeBigUInt64LE(0x3fn, 12); // every v1 capability
  p.writeDoubleLE(width, 20);
  p.writeDoubleLE(height, 28);
  p.writeDoubleLE(scale, 36);
  p.writeUInt8(dark ? 1 : 0, 44);
  return frame(0x40, p);
}
function pointer(phase, x, y) {
  const p = Buffer.alloc(1 + 8 + 8 + 1 + 1 + 1);
  p.writeUInt8(phase, 0);
  p.writeDoubleLE(x, 1);
  p.writeDoubleLE(y, 9);
  p.writeUInt8(0, 17); // primary
  p.writeUInt8(1, 18); // one click
  p.writeUInt8(0, 19); // no modifiers
  return frame(0x42, p);
}
function kinds(answer) {
  const seen = [];
  for (let at = 0; at + 5 <= answer.length; ) {
    const len = answer.readUInt32LE(at);
    seen.push(answer.readUInt8(at + 4));
    at += 4 + len;
  }
  return seen;
}
/** The button's centre, from the answer's semantics (kind 0x05). */
function buttonCentre(answer) {
  for (let at = 0; at + 5 <= answer.length; ) {
    const len = answer.readUInt32LE(at);
    const kind = answer.readUInt8(at + 4);
    if (kind === 0x05) {
      let c = at + 5 + 8; // seq
      const count = answer.readUInt32LE(c);
      c += 4;
      for (let i = 0; i < count; i++) {
        const size = answer.readUInt32LE(c);
        let n = c + 4 + 8; // id
        const roleLen = answer.readUInt32LE(n);
        const role = answer.toString("utf8", n + 4, n + 4 + roleLen);
        n += 4 + roleLen;
        n += 4 + answer.readUInt32LE(n); // label
        n += answer.readUInt8(n) === 1 ? 1 + 4 + answer.readUInt32LE(n + 1) : 1; // value
        if (role === "button") {
          const [x, y, w, h] = [0, 8, 16, 24].map((o) => answer.readDoubleLE(n + o));
          return [x + w / 2, y + h / 2];
        }
        c += 4 + size;
      }
    }
    at += 4 + len;
  }
  throw new Error("no button in the semantics");
}

// --- one turn through the canonical ABI ------------------------------------
function turn(instance, input) {
  const { memory, cabi_realloc, call, cabi_post_call } = instance.exports;
  const ptr = cabi_realloc(0, 0, 1, input.length);
  new Uint8Array(memory.buffer, ptr, input.length).set(input);
  const ret = call(ptr, input.length);
  const view = new DataView(memory.buffer);
  const [at, len] = [view.getUint32(ret, true), view.getUint32(ret + 4, true)];
  const answer = Buffer.from(new Uint8Array(memory.buffer, at, len));
  cabi_post_call(ret);
  return answer;
}

const viewport = { width: 320, height: 140, scale: 2, dark: false };
const ms = (from) => Number(process.hrtime.bigint() - from) / 1e6;
const median = (xs) => [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)];
const samples = { compile: [], instantiate: [], handshake: [], press: [] };
let first, second, memoryBytes;

for (let run = 0; run < RUNS; run++) {
  let t = process.hrtime.bigint();
  const module = new WebAssembly.Module(bytes);
  samples.compile.push(ms(t));
  if (WebAssembly.Module.imports(module).length !== 0) throw new Error("the block imports something");
  t = process.hrtime.bigint();
  const instance = new WebAssembly.Instance(module, {});
  samples.instantiate.push(ms(t));
  t = process.hrtime.bigint();
  first = turn(instance, offer(viewport));
  samples.handshake.push(ms(t));
  const [x, y] = buttonCentre(first);
  t = process.hrtime.bigint();
  second = turn(instance, Buffer.concat([pointer(1, x, y), pointer(2, x, y)]));
  samples.press.push(ms(t));
  memoryBytes = instance.exports.memory.buffer.byteLength;
}

const out = {
  module_bytes: bytes.length,
  gzip_bytes: zlib.gzipSync(bytes, { level: 9 }).length,
  brotli_bytes: zlib.brotliCompressSync(bytes, { params: { [zlib.constants.BROTLI_PARAM_QUALITY]: 11 } }).length,
  imports: WebAssembly.Module.imports(new WebAssembly.Module(bytes)).length,
  linear_memory_bytes: memoryBytes,
  runs: RUNS,
  median_ms: Object.fromEntries(Object.entries(samples).map(([k, v]) => [k, Number(median(v).toFixed(3))])),
  first_run_ms: Object.fromEntries(Object.entries(samples).map(([k, v]) => [k, Number(v[0].toFixed(3))])),
  handshake_answer: { bytes: first.length, kinds: kinds(first) },
  press_answer: { bytes: second.length, kinds: kinds(second) },
  press_at: buttonCentre(first),
  node: process.version,
};
console.log(JSON.stringify(out, null, 2));
if (answersDir) {
  fs.mkdirSync(answersDir, { recursive: true });
  fs.writeFileSync(path.join(answersDir, "handshake.bin"), first);
  fs.writeFileSync(path.join(answersDir, "press.bin"), second);
  fs.writeFileSync(path.join(answersDir, "viewport.json"), JSON.stringify(viewport));
}
