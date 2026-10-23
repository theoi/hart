import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';
import { pageInfo, runCommand, waitForPrompt } from '../lib/hart';
import { expectedText, loadScript } from '../lib/script';

const script = loadScript();
const BOOT_TIMEOUT = Number(process.env.HART_BOOT_TIMEOUT_MS ?? 180_000);
const COMMAND_TIMEOUT = 120_000;
const resultsDir = join(dirname(fileURLToPath(import.meta.url)), '..', 'results');

test('boots to a shell and runs the command script', async ({ page, baseURL, browser }, testInfo) => {
  test.setTimeout(BOOT_TIMEOUT + script.commands.length * COMMAND_TIMEOUT);
  const start = Date.now();
  await page.goto(baseURL!, { waitUntil: 'commit' });
  await waitForPrompt(page, BOOT_TIMEOUT);
  const bootMs = Date.now() - start;
  const info = await pageInfo(page);
  console.log(`${testInfo.project.name}: prompt after ${(bootMs / 1000).toFixed(2)} s (${info.backend}, ${info.guest}); script ${script.source}`);

  const commands: { input: string; output: string }[] = [];
  for (const cmd of script.commands) {
    await test.step(cmd.input, async () => {
      const r = await runCommand(page, cmd.input, script.prompt, COMMAND_TIMEOUT);
      expect(r.echo, 'the shell echoes the typed command').toContain(cmd.input);
      for (const s of cmd.expect.contains ?? []) {
        expect(r.output, `console output of ${cmd.input}`).toContain(s);
        expect(r.rendered, `terminal after ${cmd.input}`).toContain(s);
      }
      const want = expectedText(script, cmd.expect);
      if (want !== undefined) {
        expect(r.output, `console output of ${cmd.input}`).toBe(want);
        // The rendered terminal drops trailing blanks, so compare right-trimmed lines.
        const block = want.split('\n').map((l) => l.trimEnd()).join('\n').trimEnd();
        expect(r.rendered, `terminal after ${cmd.input}`).toContain(block);
      }
      commands.push({ input: cmd.input, output: r.output });
    });
  }

  mkdirSync(resultsDir, { recursive: true });
  const report = {
    url: baseURL,
    browser: `${testInfo.project.name} ${browser.version()}`,
    date: new Date().toISOString(),
    script: script.source,
    bootMs,
    backend: info.backend,
    guest: info.guest,
    commands,
  };
  writeFileSync(join(resultsDir, `functional-${testInfo.project.name}.json`), JSON.stringify(report, null, 2) + '\n');
});
