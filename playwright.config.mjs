import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: './tests/browser',
  timeout: 30000,
  workers: 1,
  use: { headless: true, viewport: { width: 1280, height: 850 }, trace: 'retain-on-failure' },
  reporter: [['list']],
  outputDir: 'output/playwright/results',
});
