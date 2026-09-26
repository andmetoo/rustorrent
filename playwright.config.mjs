import { defineConfig } from '@playwright/test';
// PW_CHROMIUM_PATH lets local runs use a preinstalled Chromium build; CI installs its own.
const executablePath = process.env.PW_CHROMIUM_PATH;
export default defineConfig({
  testDir: './tests/browser',
  timeout: 30000,
  workers: 1,
  use: {
    headless: true, viewport: { width: 1280, height: 850 }, trace: 'retain-on-failure',
    ...(executablePath ? { launchOptions: { executablePath } } : {}),
  },
  reporter: [['list']],
  outputDir: 'output/playwright/results',
});
