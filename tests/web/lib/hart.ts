import type { Page } from '@playwright/test';
import type { CommandScript } from './script';

/** The page API the site exposes on window.hart (see web/src/main.ts). */
interface HartPageApi {
  state: string;
  backend: string;
  guest: string | null;
  bootMs: number | null;
  promptAt: number | null;
  instret: number;
  mips: number | null;
  busyMs: number;
  promptVisible(): boolean;
  textBeforeCursor(): string;
  terminalText(): string;
  transcript(from?: number): string;
  transcriptLength(): number;
}

declare global {
  interface Window {
    hart: HartPageApi;
  }
}

/**
 * Normalizes console text as tests/script/README.md specifies: drop CSI and two-byte ESC
 * sequences, then CRLF -> LF, then drop any remaining CR.
 */
export function normalize(s: string): string {
  // eslint-disable-next-line no-control-regex
  return s.replace(/\x1b\[[0-?]*[ -/]*[@-~]|\x1b[@-Z\\-_]/g, '').replace(/\r\n/g, '\n').replace(/\r/g, '');
}

/**
 * Waits until the shell prompt is at the end of the console output (the spec's prompt
 * regex on the normalized text) and at the terminal cursor, as the visitor sees it.
 */
export async function waitForPrompt(page: Page, script: CommandScript, timeout: number, from = 0): Promise<void> {
  await page.waitForFunction(
    ({ regex, text, from }) => {
      const h = window.hart;
      if (!h || !h.textBeforeCursor().endsWith(text)) return false;
      const norm = h
        .transcript(from)
        // eslint-disable-next-line no-control-regex
        .replace(/\x1b\[[0-?]*[ -/]*[@-~]|\x1b[@-Z\\-_]/g, '')
        .replace(/\r\n/g, '\n')
        .replace(/\r/g, '');
      return new RegExp(regex).test(norm);
    },
    { regex: script.prompt.regex, text: script.prompt.text, from },
    { polling: 20, timeout },
  );
}

export async function pageInfo(page: Page) {
  return page.evaluate(() => ({
    state: window.hart.state,
    backend: window.hart.backend,
    guest: window.hart.guest,
    bootMs: window.hart.bootMs,
    promptAt: window.hart.promptAt,
    instret: window.hart.instret,
    mips: window.hart.mips,
    busyMs: window.hart.busyMs,
  }));
}

/** Normalized console text before the first prompt, for the banner check. */
export async function bootText(page: Page): Promise<string> {
  return normalize(await page.evaluate(() => window.hart.transcript()));
}

export interface CommandResult {
  /** The echoed command line. */
  echo: string;
  /** The command's output, extracted and normalized as tests/script/README.md specifies. */
  output: string;
  /** Rendered terminal contents after the command finished. */
  rendered: string;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * Types `input` into the terminal with the keyboard, presses Enter, waits for the next
 * prompt, and extracts the output following the protocol in tests/script/README.md.
 */
export async function runCommand(page: Page, script: CommandScript, input: string): Promise<CommandResult> {
  if (script.enter !== '\r') throw new Error(`unsupported enter sequence ${JSON.stringify(script.enter)}`);
  const from = await page.evaluate(() => window.hart.transcriptLength());
  await page.locator('#terminal').click();
  await page.keyboard.type(input, { delay: 15 });
  await page.keyboard.press('Enter'); // xterm.js sends "\r" for Enter

  const prompt = new RegExp(script.prompt.regex);
  const deadline = Date.now() + script.commandTimeoutMs;
  for (;;) {
    const seg = normalize(await page.evaluate((f) => window.hart.transcript(f), from));
    const m = prompt.exec(seg);
    // `seg` starts mid-stream, where "^" in the spec's regex must not match.
    if (m && !(m.index === 0 && !m[0].startsWith('\n'))) {
      await waitForPrompt(page, script, Math.max(1000, deadline - Date.now()), from);
      const before = seg.slice(0, m.index);
      const nl = before.indexOf('\n');
      let output = nl >= 0 ? before.slice(nl + 1) : '';
      if (output.endsWith('\n')) output = output.slice(0, -1);
      return {
        echo: nl >= 0 ? before.slice(0, nl) : before,
        output,
        rendered: await page.evaluate(() => window.hart.terminalText()),
      };
    }
    if (Date.now() > deadline) {
      throw new Error(`no prompt within ${script.commandTimeoutMs} ms after ${JSON.stringify(input)}; output ended with ${JSON.stringify(seg.slice(-400))}`);
    }
    await sleep(50);
  }
}

/** Expands tabs to 8-column stops, as the terminal renders them. */
export function expandTabs(line: string): string {
  let out = '';
  for (const ch of line) {
    if (ch === '\t') out += ' '.repeat(8 - (out.length % 8));
    else out += ch;
  }
  return out;
}

/** The output as the terminal should render it: tabs expanded, trailing blanks dropped. */
export function renderedForm(output: string): string {
  return output
    .split('\n')
    .map((l) => expandTabs(l).trimEnd())
    .join('\n')
    .trimEnd();
}
