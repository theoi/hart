import type { Page } from '@playwright/test';

/** The page API the site exposes on window.hart (see web/src/main.ts). */
interface HartPageApi {
  state: string;
  backend: string;
  guest: string | null;
  bootMs: number | null;
  promptAt: number | null;
  instret: number;
  mips: number | null;
  promptVisible(): boolean;
  terminalText(): string;
  transcript(from?: number): string;
  transcriptLength(): number;
}

declare global {
  interface Window {
    hart: HartPageApi;
  }
}

/** Strips ANSI escape sequences (CSI and OSC) so terminal output can be compared as text. */
export function stripAnsi(s: string): string {
  // eslint-disable-next-line no-control-regex
  return s.replace(/\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[()][0-9A-Za-z]|\x1b[=>]/g, '');
}

/** Waits until the terminal's cursor line ends with the shell prompt. */
export async function waitForPrompt(page: Page, timeout: number): Promise<void> {
  await page.waitForFunction(() => window.hart?.promptVisible() === true, null, { polling: 20, timeout });
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
  }));
}

export interface CommandResult {
  /** The echoed command line, as the shell printed it. */
  echo: string;
  /** Raw console output between the echoed command and the next prompt, with CRLF -> LF. */
  output: string;
  /** Rendered terminal contents after the command finished. */
  rendered: string;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * Types `command` into the terminal through the keyboard, presses Enter, and waits for the
 * shell to print the next prompt.
 */
export async function runCommand(page: Page, command: string, prompt: string, timeout: number): Promise<CommandResult> {
  const from = await page.evaluate(() => window.hart.transcriptLength());
  await page.locator('#terminal').click();
  await page.keyboard.type(command, { delay: 15 });
  await page.keyboard.press('Enter');

  const deadline = Date.now() + timeout;
  let last = '';
  let stableSince = 0;
  for (;;) {
    const seg = stripAnsi(await page.evaluate((f) => window.hart.transcript(f), from));
    const echoEnd = seg.indexOf('\r\n');
    const rest = echoEnd >= 0 ? seg.slice(echoEnd + 2) : '';
    const done = echoEnd >= 0 && (rest === prompt || rest.endsWith(`\r\n${prompt}`));
    if (done && seg === last) {
      // Require a quiet period so a prompt-like fragment mid-output is not mistaken for the end.
      if (Date.now() - stableSince >= 300) {
        return {
          echo: seg.slice(0, echoEnd),
          output: rest.slice(0, rest.length - prompt.length).replace(/\r\n/g, '\n'),
          rendered: await page.evaluate(() => window.hart.terminalText()),
        };
      }
    } else {
      last = seg;
      stableSince = Date.now();
    }
    if (Date.now() > deadline) {
      throw new Error(`timed out after ${timeout} ms waiting for the prompt after ${JSON.stringify(command)}; got ${JSON.stringify(seg.slice(-400))}`);
    }
    await sleep(50);
  }
}
