import { expect, test } from '@playwright/test';
import { waitForPrompt } from '../lib/hart';
import { loadScript } from '../lib/script';

// The page's own controls: powering off from the shell, then Restart booting again.
const script = loadScript();

test('poweroff stops the machine and Restart boots it again', async ({ page, baseURL }) => {
  test.setTimeout(2 * script.bootTimeoutMs + 120_000);
  await page.goto(baseURL!);
  await waitForPrompt(page, script, script.bootTimeoutMs);
  await expect(page.locator('#st-state')).toHaveText('running');
  await expect(page.locator('#st-boot')).toHaveText(/^booted in \d+\.\d s$/);

  await page.locator('#terminal').click();
  await page.keyboard.type('poweroff', { delay: 15 });
  await page.keyboard.press('Enter');
  await expect(page.locator('#st-state')).toHaveText('powered off', { timeout: 120_000 });
  await expect(page.locator('#st-speed')).toHaveText('');

  await page.locator('#restart').click();
  await expect(page.locator('#st-state')).toHaveText('booting');
  const restartedAt = await page.evaluate(() => performance.now());
  await waitForPrompt(page, script, script.bootTimeoutMs);
  await expect(page.locator('#st-state')).toHaveText('running');
  const bootMs = await page.evaluate(() => window.hart.bootMs);
  expect(bootMs, 'the second boot is timed from the Restart click').not.toBeNull();
  expect(bootMs!).toBeLessThan((await page.evaluate(() => performance.now())) - restartedAt + 1000);
});

test('keys typed during boot reach the shell intact', async ({ page, baseURL }) => {
  test.setTimeout(script.bootTimeoutMs + script.commandTimeoutMs + 30_000);
  await page.goto(baseURL!);
  // Type once the kernel is running, well before the shell exists.
  await page.waitForFunction(() => window.hart?.transcript().includes('Linux version'), null, {
    polling: 20,
    timeout: script.bootTimeoutMs,
  });
  await page.locator('#terminal').click();
  // The arithmetic makes the expected output differ from the echoed input.
  await page.keyboard.type('echo early-$((6*7))', { delay: 15 });
  await page.keyboard.press('Enter');
  expect(await page.evaluate(() => window.hart.state), 'typing finished before the first prompt').toBe('booting');

  await page.waitForFunction(
    ({ regex }) => {
      const text = window.hart
        .transcript()
        // eslint-disable-next-line no-control-regex
        .replace(/\x1b\[[0-?]*[ -/]*[@-~]|\x1b[@-Z\\-_]/g, '')
        .replace(/\r\n/g, '\n')
        .replace(/\r/g, '');
      return /\nearly-42\n/.test(text) && new RegExp(regex).test(text);
    },
    { regex: script.prompt.regex },
    { polling: 50, timeout: script.bootTimeoutMs + script.commandTimeoutMs },
  );
});
