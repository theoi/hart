/// <reference lib="webworker" />
/**
 * The emulator worker: downloads the guest images, instantiates the WebAssembly machine,
 * and runs it in short slices so console input and output flow between them.
 */

import init, { Machine, RunState } from './wasm/hart_web.js';
import wasmUrl from './wasm/hart_web_bg.wasm?url';
import { fetchManifest, loadImages, type LoadedImages } from './images';
import type { FromWorker, MachineState, Manifest, ToWorker } from './protocol';

const scope = self as unknown as DedicatedWorkerGlobalScope;

/** Emulation time per slice before yielding to the worker's event loop. */
const SLICE_MS = 16;
/** Target duration of one `run` call; bounds clock-read and input latency. */
const CALL_MS = 2;
const STATS_MS = 250;
const PROGRESS_MS = 50;
const IDLE_SLEEP_MS = 4;
const MAX_BATCH = 1 << 26;

let manifest: Manifest | null = null;
let images: LoadedImages | null = null;
let machine: Machine | null = null;
let generation = 0;
const pendingInput: Uint8Array[] = [];
let wake: (() => void) | null = null;

function post(msg: FromWorker, transfer: Transferable[] = []): void {
  scope.postMessage(msg, transfer);
}

function fail(err: unknown): void {
  generation++;
  machine = null;
  post({ type: 'error', message: err instanceof Error ? err.message : String(err) });
}

scope.onmessage = (ev: MessageEvent<ToWorker>) => {
  const msg = ev.data;
  switch (msg.type) {
    case 'boot':
      boot(msg.manifestUrl).catch(fail);
      break;
    case 'input':
      // Keystrokes before power-on have nowhere to go.
      if (machine) {
        pendingInput.push(msg.data);
        wake?.();
      }
      break;
    case 'restart':
      if (images) {
        try {
          start();
        } catch (err) {
          fail(err);
        }
      }
      break;
  }
};

async function boot(manifestUrl: string): Promise<void> {
  const t0 = performance.now();
  const wasmReady = init({ module_or_path: wasmUrl }).then(() => performance.now() - t0);
  // Surface a wasm failure even while the download is still running.
  wasmReady.catch(fail);
  manifest = await fetchManifest(manifestUrl);
  post({ type: 'manifest', manifest, backend: '' });
  let lastProgress = 0;
  images = await loadImages(manifest, manifestUrl, (loaded, total) => {
    const now = performance.now();
    if (now - lastProgress >= PROGRESS_MS || loaded === total) {
      lastProgress = now;
      post({ type: 'progress', loaded, total });
    }
  });
  const downloadMs = performance.now() - t0;
  const compileMs = await wasmReady;
  post({ type: 'manifest', manifest, backend: Machine.backend() });
  post({
    type: 'loaded',
    downloadMs,
    compileMs,
    encodedBytes: images.encodedBytes,
    decodedBytes: images.decodedBytes,
  });
  start();
}

function start(): void {
  if (!images || !manifest) return;
  const gen = ++generation;
  if (machine) {
    machine.free();
    machine = null;
  }
  pendingInput.length = 0;
  machine = new Machine(images.firmware, images.kernel, images.dtb ?? null, manifest.ramMiB, manifest.bootargs ?? null);
  post({ type: 'started' });
  runLoop(gen).catch(fail);
}

const channel = new MessageChannel();
let resume: (() => void) | null = null;
channel.port1.onmessage = () => {
  const r = resume;
  resume = null;
  r?.();
};

/** Yields to the event loop without the 4 ms clamp that nested setTimeout gets. */
function yieldToEventLoop(): Promise<void> {
  return new Promise((r) => {
    resume = r;
    channel.port2.postMessage(null);
  });
}

/** Sleeps for `ms`, or until input arrives. */
function sleep(ms: number): Promise<void> {
  return new Promise((r) => {
    const timer = setTimeout(() => {
      wake = null;
      r();
    }, ms);
    wake = () => {
      clearTimeout(timer);
      wake = null;
      r();
    };
  });
}

function stateName(s: RunState): MachineState {
  switch (s) {
    case RunState.Idle:
      return 'idle';
    case RunState.PoweredOff:
      return 'poweroff';
    case RunState.RebootRequested:
      return 'reboot';
    default:
      return 'running';
  }
}

async function runLoop(gen: number): Promise<void> {
  let batch = 20_000;
  let windowStart = performance.now();
  let insns = 0;
  let busyMs = 0;
  while (gen === generation && machine) {
    const m = machine;
    if (pendingInput.length) {
      for (const data of pendingInput) m.push_input(data);
      pendingInput.length = 0;
    }
    const sliceStart = performance.now();
    let rs: RunState;
    for (;;) {
      const t0 = performance.now();
      const n = m.run(batch, t0);
      const t1 = performance.now();
      insns += n;
      busyMs += t1 - t0;
      rs = m.state();
      if (rs !== RunState.Running) break;
      if (n === batch) {
        const dt = t1 - t0;
        if (dt < CALL_MS / 2 && batch < MAX_BATCH) batch *= 2;
        else if (dt > CALL_MS * 2 && batch > 1000) batch = Math.max(1000, Math.floor(batch / 2));
      }
      if (t1 - sliceStart >= SLICE_MS) break;
    }
    const out = m.take_output();
    if (out.byteLength) post({ type: 'output', data: out }, [out.buffer]);
    const stopped = rs === RunState.PoweredOff || rs === RunState.RebootRequested;
    const now = performance.now();
    if (now - windowStart >= STATS_MS || stopped) {
      post({ type: 'stats', instret: m.instret(), insns, busyMs, wallMs: now - windowStart, state: stateName(rs) });
      windowStart = now;
      insns = 0;
      busyMs = 0;
    }
    if (rs === RunState.PoweredOff) {
      post({ type: 'stopped', reason: 'poweroff' });
      return;
    }
    if (rs === RunState.RebootRequested) {
      post({ type: 'stopped', reason: 'reboot' });
      start();
      return;
    }
    if (rs === RunState.Idle) await sleep(IDLE_SLEEP_MS);
    else await yieldToEventLoop();
  }
}
