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
