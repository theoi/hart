# Browser tests

Playwright tests for the web demo (promise acceptance 5 and 6).

| Project | What it checks |
| --- | --- |
| `chromium-timing` | D5: navigation start to the shell prompt in headless Chromium, in fresh browser contexts. Five runs by default; reports every run and the median, requires a median under 30 s, and records whether it is under 10 s. Writes `results/d5-timing.json`. |
| `chromium`, `firefox` | Boots, then types the command script through the terminal with the keyboard and checks both the raw console output and the rendered terminal. Writes `results/functional-<project>.json`. |
| `screenshots-chromium`, `screenshots-firefox` | Screenshots at 1920x1080, 1280x800, and 800x1000 for the design review, plus a throttled download-progress shot. Writes `results/screenshots/`. |

The command script is `tests/script/commands.json` (shared with the native boot test). Until
it exists, the tests use `script.fallback.json`, which matches the stub machine.

## Run

```sh
cd tests/web
npm ci
npx playwright install --with-deps chromium firefox
npm run test:timing            # D5 timing, Chromium
npm run test:functional        # command script, Chromium and Firefox
npm run screenshots
```

By default the suite serves `web/dist` (build it first; see `web/README.md`) with
`serve.mjs` at `http://127.0.0.1:4173/hart/`, so boot timing excludes internet bandwidth.

Environment variables:

- `HART_BASE_URL`: test another deployment instead, such as the live site
  (`HART_BASE_URL=https://theoi.github.io/hart/ npm run test:functional`).
- `HART_TIMING_RUNS`: number of timing runs (default 5).
- `HART_BOOT_TIMEOUT_MS`: per-boot timeout (default 180000).
- `HART_SCRIPT`: path to a different command script.
