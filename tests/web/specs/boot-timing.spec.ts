import { execSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { cpus, platform, release, totalmem } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';
import { pageInfo, waitForPrompt } from '../lib/hart';

// D5: navigation start to the shell prompt appearing in the terminal, headless Chromium,
// median of at least five runs, each in a fresh browser context (cold cache).
const RUNS = Math.max(1, Number(process.env.HART_TIMING_RUNS ?? 5));
const LIMIT_MS = 30_000;
const STRETCH_MS = 10_000;
const BOOT_TIMEOUT = Number(process.env.HART_BOOT_TIMEOUT_MS ?? 180_000);

const resultsDir = join(dirname(fileURLToPath(import.meta.url)), '..', 'results');

function median(xs: number[]): number {
  const s = [...xs].sort((a, b) => a - b);
  const mid = s.length >> 1;
  return s.length % 2 ? s[mid] : (s[mid - 1] + s[mid]) / 2;
}

function gitCommit(): string | null {
  if (process.env.HART_COMMIT) return process.env.HART_COMMIT;
  try {
    return execSync('git rev-parse HEAD', { stdio: ['ignore', 'pipe', 'ignore'] }).toString().trim();
  } catch {
    return null;
  }
}

test('D5: boots to the shell prompt in headless Chromium', async ({ browser, baseURL }, testInfo) => {
  test.setTimeout(RUNS * (BOOT_TIMEOUT + 15_000));
  const runs = [];
  for (let i = 1; i <= RUNS; i++) {
    const context = await browser.newContext({ viewport: { width: 1280, height: 800 } });
    const page = await context.newPage();
    const start = Date.now();
    await page.goto(baseURL!, { waitUntil: 'commit' });
    await waitForPrompt(page, BOOT_TIMEOUT);
    const ms = Date.now() - start;
    const info = await pageInfo(page);
    runs.push({ run: i, ms, inPageMs: info.promptAt, backend: info.backend, guest: info.guest, instret: info.instret, mips: info.mips });
    console.log(`run ${i}: ${(ms / 1000).toFixed(2)} s to prompt (in-page ${((info.promptAt ?? 0) / 1000).toFixed(2)} s, ${info.backend}, ${Math.round(info.mips ?? 0)} MIPS)`);
    await context.close();
  }
  const times = runs.map((r) => r.ms);
  const med = median(times);
  const report = {
    measured: 'navigation start (page.goto) to shell prompt at the terminal cursor, wall clock from the test process',
    url: baseURL,
    commit: gitCommit(),
    date: new Date().toISOString(),
    browser: `${testInfo.project.name} ${browser.version()}`,
    machine: { cpus: cpus().length, cpuModel: cpus()[0]?.model, memoryGiB: +(totalmem() / 2 ** 30).toFixed(1), os: `${platform()} ${release()}` },
    runs,
    medianMs: med,
    minMs: Math.min(...times),
    maxMs: Math.max(...times),
    limitMs: LIMIT_MS,
    pass: med < LIMIT_MS,
    stretchMs: STRETCH_MS,
    stretchMet: med < STRETCH_MS,
  };
  mkdirSync(resultsDir, { recursive: true });
  writeFileSync(join(resultsDir, 'd5-timing.json'), JSON.stringify(report, null, 2) + '\n');
  await testInfo.attach('d5-timing.json', { body: JSON.stringify(report, null, 2), contentType: 'application/json' });
  console.log(
    `D5 median ${(med / 1000).toFixed(2)} s over ${RUNS} runs [${times.map((t) => (t / 1000).toFixed(2)).join(', ')}]; ` +
      `limit 30 s: ${report.pass ? 'PASS' : 'FAIL'}; stretch 10 s: ${report.stretchMet ? 'met' : 'not met'}`,
  );
  expect(RUNS, 'D5 requires at least five runs').toBeGreaterThanOrEqual(process.env.HART_TIMING_RUNS ? 1 : 5);
  expect(med).toBeLessThan(LIMIT_MS);
});
