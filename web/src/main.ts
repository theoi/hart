import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import './style.css';
import type { FromWorker, Manifest, ToWorker } from './protocol';

type UiState = 'loading' | 'booting' | 'running' | 'stopped' | 'error';

/** Shell prompt at the cursor; the guest's BusyBox ash prints "~ # " for root. */
const PROMPT_RE = /[#$] $/;
const TRANSCRIPT_LIMIT = 4_000_000;

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const el = {
  terminal: $('terminal'),
  frame: $('term-frame'),
  progress: $('term-progress'),
  progressBar: $('term-progress-bar'),
  dot: $('st-dot'),
  state: $('st-state'),
  speed: $('st-speed'),
  boot: $('st-boot'),
  up: $('st-up'),
  restart: $<HTMLButtonElement>('restart'),
  chips: Array.from(document.querySelectorAll<HTMLButtonElement>('.chip')),
};

const theme = {
  background: '#0b0e14',
  foreground: '#d7dde5',
  cursor: '#f5b53f',
  cursorAccent: '#0b0e14',
  selectionBackground: '#2a3a55',
  black: '#1b1f27',
  red: '#f47067',
  green: '#7ccf8a',
  yellow: '#f5c561',
  blue: '#6cb6ff',
  magenta: '#d2a8ff',
  cyan: '#56d4dd',
  white: '#c9d1d9',
  brightBlack: '#6e7781',
  brightRed: '#ff938a',
  brightGreen: '#a2e5ad',
  brightYellow: '#ffd98a',
  brightBlue: '#9fd0ff',
  brightMagenta: '#e2c5ff',
  brightCyan: '#8ce8ee',
  brightWhite: '#f0f3f6',
};

function fontSizeFor(width: number): number {
  if (width >= 1600) return 15;
  if (width >= 900) return 14;
  if (width >= 600) return 13;
  return 12;
}

const term = new Terminal({
  fontFamily: getComputedStyle(document.documentElement).getPropertyValue('--mono').trim() || 'monospace',
  fontSize: fontSizeFor(window.innerWidth),
  lineHeight: 1.2,
  cursorBlink: true,
  cursorInactiveStyle: 'outline',
  scrollback: 10_000,
  theme,
});
const fit = new FitAddon();
term.loadAddon(fit);
term.open(el.terminal);

/** Guest programs assume an 80-column console, so shrink the font rather than wrap. */
const MIN_COLS = 80;
const MIN_FONT_SIZE = 10;

function fitTerminal(): void {
  let size = fontSizeFor(window.innerWidth);
  if (term.options.fontSize !== size) term.options.fontSize = size;
  let dims = fit.proposeDimensions();
  while (dims && dims.cols < MIN_COLS && size > MIN_FONT_SIZE) {
    term.options.fontSize = --size;
    dims = fit.proposeDimensions();
  }
  fit.fit();
}

fitTerminal();
term.focus();

let fitQueued = false;
new ResizeObserver(() => {
  if (fitQueued) return;
  fitQueued = true;
  requestAnimationFrame(() => {
    fitQueued = false;
    fitTerminal();
  });
}).observe(el.terminal);

el.frame.addEventListener('mousedown', (e) => {
  if (e.target === el.frame) {
    e.preventDefault();
    term.focus();
  }
});

// UI state, also exposed on window.hart for tests and curious visitors.

let state: UiState = 'loading';
let backend = '';
let manifest: Manifest | null = null;
let bootStart = 0; // performance.now() origin is navigation start
let bootMs: number | null = null;
let promptAt: number | null = null;
let startedAt: number | null = null;
let stoppedAt: number | null = null;
let instret = 0;
let mips: number | null = null;
let busyMs = 0;
let errorMessage = '';
let progress = { loaded: 0, total: 0 };

const decoder = new TextDecoder();
let transcriptChunks: string[] = [];
let transcriptBase = 0;
let transcriptLen = 0;

function appendTranscript(text: string): void {
  transcriptChunks.push(text);
  transcriptLen += text.length;
  if (transcriptLen - transcriptBase > TRANSCRIPT_LIMIT) {
    const joined = transcriptChunks.join('');
    const drop = joined.length - TRANSCRIPT_LIMIT / 2;
    transcriptChunks = [joined.slice(drop)];
    transcriptBase += drop;
  }
}

function transcript(from = 0): string {
  const joined = transcriptChunks.join('');
  transcriptChunks = [joined];
  return joined.slice(Math.max(0, from - transcriptBase));
}

/** The cursor line up to the cursor, trailing blanks included. */
function textBeforeCursor(): string {
  const buf = term.buffer.active;
  const line = buf.getLine(buf.baseY + buf.cursorY);
  return line ? line.translateToString(false, 0, buf.cursorX) : '';
}

function promptVisible(): boolean {
  return PROMPT_RE.test(textBeforeCursor());
}

function terminalText(): string {
  const buf = term.buffer.active;
  const lines: string[] = [];
  for (let i = 0; i < buf.length; i++) {
    const line = buf.getLine(i);
    if (!line) continue;
    const text = line.translateToString(true);
    if (line.isWrapped && lines.length) lines[lines.length - 1] += text;
    else lines.push(text);
  }
  while (lines.length && lines[lines.length - 1] === '') lines.pop();
  return lines.join('\n');
}

declare global {
  interface Window {
    hart: {
      readonly state: UiState;
      readonly backend: string;
      readonly guest: string | null;
      /** Milliseconds from boot start (navigation, or Restart) to the first shell prompt. */
      readonly bootMs: number | null;
      /** performance.now() when the first shell prompt of the page reached the terminal. */
      readonly promptAt: number | null;
      readonly instret: number;
      readonly mips: number | null;
      /** Milliseconds the current machine has spent executing instructions. */
      readonly busyMs: number;
      promptVisible(): boolean;
      textBeforeCursor(): string;
      terminalText(): string;
      transcript(from?: number): string;
      transcriptLength(): number;
      restart(): void;
    };
  }
}

window.hart = {
  get state() {
    return state;
  },
  get backend() {
    return backend;
  },
  get guest() {
    return manifest?.guest ?? null;
  },
  get bootMs() {
    return bootMs;
  },
  get promptAt() {
    return promptAt;
  },
  get instret() {
    return instret;
  },
  get mips() {
    return mips;
  },
  get busyMs() {
    return busyMs;
  },
  promptVisible,
  textBeforeCursor,
  terminalText,
  transcript,
  transcriptLength: () => transcriptLen,
  restart,
};

// Loader messages, written into the terminal in a dim color before the guest starts.

const DIM = '\x1b[38;5;244m';
const ACCENT = '\x1b[38;2;245;181;63m';
const TRACK = '\x1b[38;5;238m';
const RESET = '\x1b[0m';

function mb(bytes: number): string {
  return (bytes / 1e6).toFixed(1);
}

function progressLine(loaded: number, total: number): string {
  const cells = 24;
  const frac = total ? loaded / total : 0;
  const filled = Math.round(frac * cells);
  const bar = `${ACCENT}${'━'.repeat(filled)}${TRACK}${'━'.repeat(cells - filled)}`;
  const pct = `${Math.floor(frac * 100)}%`.padStart(4);
  return `\r\x1b[2K${DIM}hart: downloading ${manifest?.guest ?? 'guest'} ${bar}${DIM} ${pct}  ${mb(loaded)} / ${mb(total)} MB${RESET}`;
}

// Status line.

function formatDuration(ms: number): string {
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, '0');
  return h ? `${h}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`;
}

function formatMips(v: number | null): string {
  if (v === null) return '';
  return `${v >= 10 ? Math.round(v) : v.toFixed(1)} MIPS`;
}

function render(): void {
  const now = performance.now();
  el.dot.dataset.state = state;
  let label: string = state;
  let speed = '';
  let boot = '';
  let up = '';
  switch (state) {
    case 'loading':
      speed = progress.total ? `${mb(progress.loaded)} / ${mb(progress.total)} MB` : '';
      break;
    case 'booting':
      speed = formatMips(mips);
      boot = `${((now - bootStart) / 1000).toFixed(1)} s`;
      break;
    case 'running':
    case 'stopped':
      label = state === 'stopped' ? 'powered off' : 'running';
      speed = state === 'running' ? formatMips(mips) : '';
      boot = bootMs !== null ? `booted in ${(bootMs / 1000).toFixed(1)} s` : '';
      up = startedAt !== null ? `up ${formatDuration((stoppedAt ?? now) - startedAt)}` : '';
      break;
    case 'error':
      label = 'error';
      speed = errorMessage;
      break;
  }
  if (el.state.textContent !== label) el.state.textContent = label;
  if (el.speed.textContent !== speed) el.speed.textContent = speed;
  if (el.boot.textContent !== boot) el.boot.textContent = boot;
  if (el.up.textContent !== up) el.up.textContent = up;
  el.restart.disabled = state === 'loading';
  for (const chip of el.chips) chip.disabled = state !== 'running';
}

setInterval(render, 250);

function setProgress(loaded: number, total: number): void {
  progress = { loaded, total };
  const pct = total ? (100 * loaded) / total : 0;
  el.progressBar.style.width = `${pct}%`;
  el.progress.setAttribute('aria-valuenow', String(Math.round(pct)));
  if (total && loaded >= total) el.progress.dataset.done = '';
}

// The worker.

const encoder = new TextEncoder();
let worker: Worker;

function send(msg: ToWorker, transfer: Transferable[] = []): void {
  worker.postMessage(msg, transfer);
}

function spawnWorker(): void {
  worker = new Worker(new URL('./worker.ts', import.meta.url), { type: 'module', name: 'hart' });
  worker.onmessage = (ev: MessageEvent<FromWorker>) => onWorkerMessage(ev.data);
  worker.onerror = (ev) => {
    ev.preventDefault();
    showError(ev.message || 'the emulator worker failed to start');
  };
  send({ type: 'boot', manifestUrl: new URL('images/manifest.json', document.baseURI).href });
}

function showError(message: string): void {
  state = 'error';
  errorMessage = message;
  term.write(`\r\n\x1b[38;2;248;81;73mhart: ${message}${RESET}\r\n`);
  render();
}

function onWorkerMessage(msg: FromWorker): void {
  switch (msg.type) {
    case 'manifest':
      manifest = msg.manifest;
      if (msg.backend) backend = msg.backend;
      break;
    case 'progress':
      setProgress(msg.loaded, msg.total);
      term.write(progressLine(msg.loaded, msg.total));
      render();
      break;
    case 'loaded': {
      setProgress(msg.encodedBytes, msg.encodedBytes);
      term.write(progressLine(msg.encodedBytes, msg.encodedBytes) + '\r\n');
      const ram = manifest ? `${manifest.ramMiB} MiB RAM` : '';
      const stub = backend === 'stub' ? ' (stub machine)' : '';
      term.write(
        `${DIM}hart: ${mb(msg.encodedBytes)} MB in ${(msg.downloadMs / 1000).toFixed(1)} s, powering on${stub}: RV64GC, ${ram}${RESET}\r\n`,
      );
      break;
    }
    case 'started':
      startedAt = performance.now();
      stoppedAt = null;
      state = 'booting';
      render();
      break;
    case 'output': {
      const text = decoder.decode(msg.data, { stream: true });
      appendTranscript(text);
      term.write(msg.data, onOutputParsed);
      break;
    }
    case 'stats':
      instret = msg.instret;
      busyMs += msg.busyMs;
      if (msg.busyMs >= 20) {
        const rate = msg.insns / msg.busyMs / 1000;
        mips = mips === null ? rate : mips * 0.5 + rate * 0.5;
      }
      break;
    case 'stopped':
      if (msg.reason === 'reboot') {
        term.write(`\r\n${DIM}hart: the guest requested a reboot${RESET}\r\n`);
        beginBoot();
      } else {
        stoppedAt = performance.now();
        state = 'stopped';
        term.write(`\r\n${DIM}hart: powered off. Press Restart to boot again.${RESET}\r\n`);
        render();
      }
      break;
    case 'error':
      showError(msg.message);
      break;
  }
}

function onOutputParsed(): void {
  if (state === 'booting' && promptVisible()) {
    const now = performance.now();
    bootMs = now - bootStart;
    promptAt ??= now;
    state = 'running';
    render();
  }
}

function beginBoot(): void {
  bootStart = performance.now();
  bootMs = null;
  mips = null;
  busyMs = 0;
  state = 'booting';
}

function restart(): void {
  if (state === 'loading') return;
  // After an error the wasm instance may be unusable, so start over with a fresh worker.
  const respawn = state === 'error' || !manifest;
  term.reset();
  term.write(`${DIM}hart: restarting${RESET}\r\n`);
  beginBoot();
  if (respawn) {
    worker.terminate();
    state = 'loading';
    delete el.progress.dataset.done;
    setProgress(0, 0);
    spawnWorker();
  } else {
    send({ type: 'restart' });
  }
  render();
  term.focus();
}

term.onData((data) => {
  if (state === 'booting' || state === 'running') {
    const bytes = encoder.encode(data);
    send({ type: 'input', data: bytes }, [bytes.buffer]);
  }
});

el.restart.addEventListener('click', restart);
for (const chip of el.chips) {
  chip.addEventListener('click', () => {
    const cmd = chip.dataset.cmd;
    if (!cmd || state !== 'running') return;
    const bytes = encoder.encode(`${cmd}\r`);
    send({ type: 'input', data: bytes }, [bytes.buffer]);
    term.focus();
  });
}

render();
spawnWorker();
