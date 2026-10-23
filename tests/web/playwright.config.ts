import { defineConfig, devices } from '@playwright/test';

// HART_BASE_URL points the suite at any deployment, such as the live Pages site.
// Without it, the built site in web/dist is served locally.
const external = process.env.HART_BASE_URL;
const port = Number(process.env.HART_PORT ?? 4173);
const baseURL = external ?? `http://127.0.0.1:${port}/hart/`;
const viewport = { width: 1280, height: 800 };

export default defineConfig({
  testDir: './specs',
  // One test at a time: parallel boots would compete for CPU and skew timings.
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 240_000,
  reporter: [['list'], ['json', { outputFile: 'results/report.json' }]],
  outputDir: 'results/artifacts',
  use: {
    baseURL,
    viewport,
    trace: 'retain-on-failure',
  },
  projects: [
    {
      name: 'chromium-timing',
      testMatch: /boot-timing\.spec\.ts/,
      use: { ...devices['Desktop Chrome'], viewport },
    },
    {
      name: 'chromium',
      testMatch: /commands\.spec\.ts/,
      use: { ...devices['Desktop Chrome'], viewport },
    },
    {
      name: 'firefox',
      testMatch: /commands\.spec\.ts/,
      use: { ...devices['Desktop Firefox'], viewport },
    },
    {
      name: 'chromium-ui',
      testMatch: /ui\.spec\.ts/,
      use: { ...devices['Desktop Chrome'], viewport },
    },
    {
      name: 'screenshots-chromium',
      testMatch: /screenshots\.spec\.ts/,
      use: { ...devices['Desktop Chrome'], viewport },
    },
    {
      name: 'screenshots-firefox',
      testMatch: /screenshots\.spec\.ts/,
      use: { ...devices['Desktop Firefox'], viewport },
    },
  ],
  webServer: external
    ? undefined
    : {
        command: `node serve.mjs --port ${port}`,
        url: baseURL,
        reuseExistingServer: true,
        stdout: 'ignore',
      },
});
