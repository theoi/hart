import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';
import { bootText, pageInfo, renderedForm, runCommand, waitForPrompt } from '../lib/hart';
import { applyCheck, loadScript } from '../lib/script';

// Promise acceptance 5 (functional part) and 6: boot, then type the shared command script
// through the terminal and check each output.
const script = loadScript();
const resultsDir = join(dirname(fileURLToPath(import.meta.url)), '..', 'results');

test('boots to a shell and runs the command script', async ({ page, baseURL, browser }, testInfo) => {
  test.setTimeout(script.bootTimeoutMs + script.commands.length * script.commandTimeoutMs + 30_000);
  const start = Date.now();
  await page.goto(baseURL!, { waitUntil: 'commit' });
  await waitForPrompt(page, script, script.bootTimeoutMs);
  const bootMs = Date.now() - start;
  const info = await pageInfo(page);
  console.log(`${testInfo.project.name}: prompt after ${(bootMs / 1000).toFixed(2)} s (${info.backend}, ${info.guest}); script ${script.source}`);

  if (script.banner !== null) {
    expect(await bootText(page), 'banner on the line before the first prompt').toContain(`${script.banner}\n${script.prompt.text}`);
  }

  const results: { id: string; input: string; pass: boolean; output: string; failures: string[] }[] = [];
  try {
    for (const cmd of script.commands) {
      await test.step(`${cmd.id}: ${cmd.input}`, async () => {
        const r = await runCommand(page, script, cmd.input);
        const failures = cmd.checks.map((c) => applyCheck(script, c, r.output)).filter((f): f is string => f !== null);
        const shown = r.rendered.includes(renderedForm(r.output));
        if (!shown) failures.push('the terminal does not show the output');
        results.push({ id: cmd.id, input: cmd.input, pass: failures.length === 0, output: r.output, failures });
        // Soft, so every command runs and is reported even after a failure.
        expect.soft(r.echo, 'the shell echoes the typed command').toContain(cmd.input);
        expect.soft(failures, `checks for ${cmd.id}; output was:\n${r.output}`).toEqual([]);
      });
    }
  } finally {
    mkdirSync(resultsDir, { recursive: true });
    const report = {
      url: baseURL,
      browser: `${testInfo.project.name} ${browser.version()}`,
      date: new Date().toISOString(),
      script: script.source,
      prompt: script.prompt,
      bootMs,
      backend: info.backend,
      guest: info.guest,
      pass: results.length === script.commands.length && results.every((r) => r.pass),
      commands: results,
    };
    writeFileSync(join(resultsDir, `functional-${testInfo.project.name}.json`), JSON.stringify(report, null, 2) + '\n');
  }
});
