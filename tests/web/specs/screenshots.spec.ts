import { mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from '@playwright/test';
import { runCommand, waitForPrompt } from '../lib/hart';
import { loadScript } from '../lib/script';

// Screenshots for the page design review: desktop sizes plus a narrow 800 px window.
const sizes = [
  { name: '1920x1080', width: 1920, height: 1080 },
  { name: '1280x800', width: 1280, height: 800 },
  { name: '800x1000', width: 800, height: 1000 },
];
const outDir = join(dirname(fileURLToPath(import.meta.url)), '..', 'results', 'screenshots');
const script = loadScript();

for (const size of sizes) {
  test(`page at ${size.name}`, async ({ browser, baseURL, browserName }) => {
    mkdirSync(outDir, { recursive: true });
    const context = await browser.newContext({ viewport: { width: size.width, height: size.height } });
    const page = await context.newPage();
    await page.goto(baseURL!);
    await waitForPrompt(page, 180_000);
    await runCommand(page, 'uname -a', script.prompt, 60_000);
    // Let the status line show a speed reading and a few seconds of uptime.
    await page.waitForTimeout(1500);
    await page.screenshot({ path: join(outDir, `${browserName}-${size.name}.png`) });
    await page.screenshot({ path: join(outDir, `${browserName}-${size.name}-full.png`), fullPage: true });
    await context.close();
  });
}

test('download progress', async ({ browser, baseURL, browserName }) => {
  test.skip(browserName !== 'chromium', 'network throttling needs the Chrome DevTools Protocol');
  mkdirSync(outDir, { recursive: true });
  const context = await browser.newContext({ viewport: { width: 1280, height: 800 } });
  const page = await context.newPage();
  const cdp = await context.newCDPSession(page);
  await cdp.send('Network.enable');
  await cdp.send('Network.emulateNetworkConditions', {
    offline: false,
    latency: 40,
    downloadThroughput: (20e6 / 8) | 0,
    uploadThroughput: (5e6 / 8) | 0,
  });
  await page.goto(baseURL!);
  await page.waitForFunction(() => {
    const t = document.getElementById('st-speed')?.textContent ?? '';
    const m = t.match(/^([\d.]+) \/ ([\d.]+) MB$/);
    return !!m && Number(m[1]) / Number(m[2]) > 0.4;
  }, null, { polling: 50, timeout: 60_000 });
  await page.screenshot({ path: join(outDir, `${browserName}-loading-1280x800.png`) });
  await context.close();
});
