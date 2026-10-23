#!/usr/bin/env node
// Measures the WebAssembly build's speed outside the browser, in Node's V8, by booting
// the images in public/images until the console prints a marker.
//
//   node scripts/bench.mjs [--wasm src/wasm/hart_web_bg.wasm] [--until "Starting crond: OK"]
//
// It drives the machine the way the worker does (host time from performance.now(),
// run calls of about 2 ms), so its MIPS figure is comparable with the page's.

import { readFileSync } from 'node:fs';
import { parseArgs } from 'node:util';
import { gunzipSync } from 'node:zlib';

const { values: opt } = parseArgs({
  options: {
    wasm: { type: 'string', default: new URL('../src/wasm/hart_web_bg.wasm', import.meta.url).pathname },
    glue: { type: 'string' },
    images: { type: 'string', default: new URL('../public/images/', import.meta.url).pathname },
    until: { type: 'string', default: 'Starting crond: OK' },
    timeout: { type: 'string', default: '300' },
  },
});

const gluePath = opt.glue ?? opt.wasm.replace(/_bg\.wasm$/, '.js');
const { initSync, Machine, RunState } = await import(gluePath);
initSync({ module: readFileSync(opt.wasm) });

const dir = opt.images.endsWith('/') ? opt.images : `${opt.images}/`;
const manifest = JSON.parse(readFileSync(`${dir}manifest.json`, 'utf8'));
const files = {};
for (const f of manifest.files) {
  const raw = readFileSync(`${dir}${f.url}`);
  files[f.role] = f.encoding === 'gzip' ? gunzipSync(raw) : raw;
}

const m = new Machine(files.firmware, files.kernel, files.dtb ?? null, manifest.ramMiB, manifest.bootargs ?? null);
const decoder = new TextDecoder();
let tail = '';
let busyMs = 0;
let batch = 100_000;
const start = performance.now();
const deadline = start + Number(opt.timeout) * 1000;
for (;;) {
  const t0 = performance.now();
  const n = m.run(batch, t0);
  const dt = performance.now() - t0;
  busyMs += dt;
  if (n === batch) {
    if (dt < 1) batch *= 2;
    else if (dt > 4) batch = Math.max(1000, batch >> 1);
  }
  tail = (tail + decoder.decode(m.take_output(), { stream: true })).slice(-4096);
  if (tail.includes(opt.until)) break;
  const st = m.state();
  if (st === RunState.PoweredOff || st === RunState.RebootRequested) {
    console.error('the guest stopped before the marker');
    process.exit(1);
  }
  if (st === RunState.Idle) {
    const ms = m.idle_ms();
    await new Promise((r) => setTimeout(r, ms < 0 ? 1 : Math.min(ms, 20)));
  }
  if (performance.now() > deadline) {
    console.error(`timeout; console ended with ${JSON.stringify(tail.slice(-300))}`);
    process.exit(1);
  }
}
const wall = performance.now() - start;
const insns = m.instret();
console.log(
  `${Machine.backend()}: ${(insns / 1e6).toFixed(1)}M instructions, ${(busyMs / 1000).toFixed(2)} s executing ` +
    `(${(wall / 1000).toFixed(2)} s wall): ${(insns / busyMs / 1000).toFixed(1)} MIPS`,
);
