// Refreshes docs/screenshots from a realistic demo library. Opt-in because it needs a
// release build and Python 3: cargo build --release && RUSTORRENT_SCREENSHOTS=1 npx playwright test screenshots
import { test, expect } from '@playwright/test';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import path from 'node:path';
import { demoLibrary, makeTorrent } from './demo-library.mjs';

test.skip(!process.env.RUSTORRENT_SCREENSHOTS, 'set RUSTORRENT_SCREENSHOTS=1 to refresh docs/screenshots');

const OUT = 'docs/screenshots';
const BINARY = path.resolve(process.env.RUSTORRENT_BINARY || 'target/release/rustorrent');
const MiB = 1024 * 1024;

// A search plugin with fixed, realistic results so the search view has content.
const PLUGIN = `#VERSION: 1.0
from novaprinter import prettyPrinter
ROWS = [
    ('ubuntu-25.10-desktop-amd64.iso', '5.7 GB', 4812, 210),
    ('ubuntu-25.10-live-server-amd64.iso', '2.9 GB', 1934, 88),
    ('ubuntu-24.04.3-desktop-amd64.iso', '6.1 GB', 3120, 97),
    ('Ubuntu Studio 25.10 (amd64)', '5.2 GB', 402, 31),
    ('ubuntu-25.10-desktop-arm64.iso', '5.4 GB', 377, 25),
    ('Kubuntu 25.10 Desktop', '4.4 GB', 356, 19),
]
class archive(object):
    url = 'https://archive.example.org'
    name = 'Open Archive'
    supported_categories = {'all': '0', 'software': '1'}
    def search(self, what, cat='all'):
        for i, (name, size, seeds, leech) in enumerate(ROWS):
            prettyPrinter({'link': 'magnet:?xt=urn:btih:%040x' % (i + 1), 'name': name, 'size': size,
                'seeds': seeds, 'leech': leech, 'engine_url': self.url,
                'desc_link': self.url + '/item/%d' % i, 'pub_date': 1788000000 - i * 86400})
`;

const page_css = (dark) => `
  *{box-sizing:border-box;margin:0}
  body{font-family:"DejaVu Sans",system-ui,sans-serif}
  .stage{display:flex;justify-content:center;padding:56px;
    background:${dark ? 'radial-gradient(120% 120% at 0% 0%,#1d2b4a 0%,#0d1220 60%)' : 'radial-gradient(120% 120% at 0% 0%,#dbe7ff 0%,#f4f6fb 55%,#eef1f7 100%)'}}
  .win{border-radius:14px;overflow:hidden;box-shadow:0 30px 80px rgba(15,23,42,${dark ? '.6' : '.25'}),0 0 0 1px rgba(${dark ? '255,255,255,.08' : '15,23,42,.08'});background:${dark ? '#1a1d24' : '#fff'}}
  .bar{height:38px;display:flex;align-items:center;gap:8px;padding:0 14px;background:${dark ? '#262a33' : '#eceef2'};border-bottom:1px solid ${dark ? '#000' : '#d9dce2'}}
  .dot{width:12px;height:12px;border-radius:50%}
  .t{flex:1;text-align:center;font-size:13px;color:${dark ? '#9aa3b2' : '#6b7280'};margin-right:60px}
  img{display:block}`;
const dots = '<span class="dot" style="background:#ff5f57"></span><span class="dot" style="background:#febc2e"></span><span class="dot" style="background:#28c840"></span>';

// Places a raw capture inside a desktop-style window on a soft background.
async function framed(browser, file, png, { dark = false, title = 'Rustorrent', width }) {
  const context = await browser.newContext({ viewport: { width: width + 112, height: 400 }, deviceScaleFactor: 2, colorScheme: dark ? 'dark' : 'light' });
  const page = await context.newPage();
  await page.setContent(`<style>${page_css(dark)}</style><div class="stage"><div class="win"><div class="bar">${dots}<span class="t">${title}</span></div><img style="width:${width}px" src="data:image/png;base64,${png.toString('base64')}"></div></div>`);
  await page.locator('img').evaluate(img => img.decode());
  await page.locator('.stage').screenshot({ path: path.join(OUT, file) });
  await context.close();
}

// Renders terminal output (HTML from terminal-capture.py) as a terminal window.
async function terminal(browser, file, htmlText, title, cols) {
  const context = await browser.newContext({ viewport: { width: 1200, height: 400 }, deviceScaleFactor: 2 });
  const page = await context.newPage();
  const colors = { 30: '#3b4252', 31: '#ff6b6b', 32: '#5fd38d', 33: '#f5c451', 34: '#6aa8ff', 35: '#c792ea', 36: '#56d4dd', 37: '#e5e9f0', 90: '#6b7280' };
  const css = Object.entries(colors).map(([k, v]) => `.f${k}{color:${v}}`).join('');
  await page.setContent(`<style>${page_css(true)}
    pre{font:13px/1.45 "DejaVu Sans Mono",Menlo,monospace;color:#d8dee9;background:#11141b;padding:14px 18px 18px;width:${cols * 7.83 + 36}px}
    .b{font-weight:bold}.d{opacity:.55}.u{text-decoration:underline}.r{background:#d8dee9;color:#11141b}${css}</style>
    <div class="stage"><div class="win"><div class="bar" style="background:#1f232b">${dots}<span class="t">${title}</span></div><pre>${htmlText}</pre></div></div>`);
  await page.locator('.stage').screenshot({ path: path.join(OUT, file) });
  await context.close();
}

// Asynchronous so the demo peers, which run in this process, keep transferring.
async function capture(rows, cols, keys, args) {
  const { stdout } = await promisify(execFile)('python3', [path.resolve('tests/browser/terminal-capture.py'), String(rows), String(cols), keys, '--', ...args], { encoding: 'utf8', timeout: 120000 });
  return stdout;
}

async function appShot(browser, url, { dark = false, viewport = { width: 1280, height: 800 }, prepare }) {
  const context = await browser.newContext({ viewport, colorScheme: dark ? 'dark' : 'light', deviceScaleFactor: 2 });
  const page = await context.newPage();
  await page.goto(url);
  await expect(page.locator('.row')).toHaveCount(6);
  if (prepare) await prepare(page);
  await page.mouse.move(0, 0);
  await page.waitForTimeout(1200);
  const png = await page.screenshot();
  await context.close();
  return png;
}

const openDetails = async page => {
  await page.locator('.row', { hasText: 'Field Recordings' }).locator('.rn').click();
  await expect(page.locator('.fl .li')).toHaveCount(6);
};

test('documentation screenshots', async ({ browser }) => {
  test.setTimeout(600000);
  const demo = await demoLibrary(BINARY);
  try {
    await demo.post('/search/install-plugin?filename=archive.py', PLUGIN, 'application/octet-stream');
    // Let the rate history fill in so the sidebar chart and ETAs are representative.
    await new Promise(resolve => setTimeout(resolve, 12000));

    const light = await appShot(browser, demo.url, { prepare: openDetails });
    const dark = await appShot(browser, demo.url, { dark: true, prepare: openDetails });
    const mobile = await appShot(browser, demo.url, { viewport: { width: 390, height: 844 }, prepare: openDetails });

    await framed(browser, 'hero-light.png', light, { width: 1280, title: 'Rustorrent — All transfers' });
    await framed(browser, 'hero-dark.png', dark, { dark: true, width: 1280, title: 'Rustorrent — All transfers' });
    await framed(browser, 'mobile.png', mobile, { width: 390, title: 'Rustorrent' });

    const add = await appShot(browser, demo.url, {
      prepare: async page => {
        await page.click('#addBtn');
        const album = makeTorrent('Field Recordings — Autumn Sessions', [
          ['01 Harbour at Dawn.flac', 188 * MiB], ['02 Rain on Tin.flac', 241 * MiB], ['03 Market Voices.flac', 305 * MiB],
          ['04 Night Train.flac', 276 * MiB], ['cover.jpg', 3 * MiB], ['liner-notes.pdf', 2 * MiB]].map(([file, length]) => ({ path: [file], length })), MiB);
        await page.setInputFiles('#tFile', { name: 'autumn-sessions.torrent', mimeType: 'application/x-bittorrent', buffer: album.data });
        await expect(page.locator('#addGo')).toBeEnabled();
        await page.waitForTimeout(600);
      },
    });
    await framed(browser, 'add-dialog.png', add, { width: 1280, title: 'Rustorrent — Add torrent' });

    const search = await appShot(browser, demo.url, {
      dark: true,
      prepare: async page => {
        await page.click('[data-v="search"]');
        await page.fill('#sq', 'ubuntu');
        await page.click('#sGo');
        await expect(page.locator('#sResults tbody tr')).toHaveCount(6, { timeout: 30000 });
      },
    });
    await framed(browser, 'search.png', search, { dark: true, width: 1280, title: 'Rustorrent — Search' });

    const settings = await appShot(browser, demo.url, {
      prepare: async page => {
        await page.click('[data-v="settings"]');
        await expect(page.locator('#v-settings')).toBeVisible();
      },
    });
    await framed(browser, 'settings.png', settings, { width: 1280, title: 'Rustorrent — Settings' });

    const addr = new URL(demo.url).host;
    const tui = await capture(23, 118, 'j|\\r|l', [BINARY, 'remote', '--ui-addr', addr, 'tui']);
    await terminal(browser, 'tui.png', tui, 'rustorrent remote tui', 118);

    const list = (await capture(12, 118, '', [BINARY, 'remote', '--ui-addr', addr, 'list'])).trimEnd();
    await terminal(browser, 'cli.png', `<span class="f32">~</span> <span class="b">rustorrent remote list</span>\n${list}`, 'Terminal', 118);
  } finally {
    await demo.close();
  }
});
