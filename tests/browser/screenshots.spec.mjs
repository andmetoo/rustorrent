// Refreshes docs/screenshots from a realistic demo library. Opt-in because it needs a
// release build: cargo build --release && RUSTORRENT_SCREENSHOTS=1 npx playwright test screenshots
import { test, expect } from '@playwright/test';
import path from 'node:path';
import { demoLibrary } from './demo-library.mjs';

test.skip(!process.env.RUSTORRENT_SCREENSHOTS, 'set RUSTORRENT_SCREENSHOTS=1 to refresh docs/screenshots');

test('documentation screenshots', async ({browser}) => {
  test.setTimeout(300000);
  const demo = await demoLibrary(path.resolve(process.env.RUSTORRENT_BINARY || 'target/release/rustorrent'));
  try {
    // Let the rate history fill in so the sidebar chart and ETAs are representative.
    await new Promise(resolve => setTimeout(resolve, 12000));
    const shots = [
      ['beta-library.png', 'light', {width: 1280, height: 800}],
      ['beta-dark.png', 'dark', {width: 1280, height: 800}],
      ['beta-mobile.png', 'light', {width: 390, height: 844}],
    ];
    for (const [file, colorScheme, viewport] of shots) {
      const context = await browser.newContext({viewport, colorScheme, deviceScaleFactor: 2});
      const page = await context.newPage();
      await page.goto(demo.url);
      await expect(page.locator('.row')).toHaveCount(6);
      await page.locator('.row', {hasText: 'Field Recordings'}).locator('.rn').click();
      await expect(page.locator('.fl .li')).toHaveCount(6);
      await page.mouse.move(0, 0);
      await page.waitForTimeout(1500);
      await page.screenshot({path: path.join('docs/screenshots', file)});
      await context.close();
    }
  } finally {
    await demo.close();
  }
});
