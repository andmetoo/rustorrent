import { test, expect } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import { createHash } from 'node:crypto';
import { spawn } from 'node:child_process';
import { mkdtemp, writeFile, readFile, rm, access } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import net from 'node:net';

let root, child, url, port, peerPort;
const binary = path.resolve('target/debug/rustorrent');
function encode(value) {
  if (Buffer.isBuffer(value)) return Buffer.concat([Buffer.from(value.length + ':'), value]);
  if (typeof value === 'string') return encode(Buffer.from(value));
  if (typeof value === 'number') return Buffer.from('i' + value + 'e');
  if (Array.isArray(value)) return Buffer.concat([Buffer.from('l'), ...value.map(encode), Buffer.from('e')]);
  return Buffer.concat([Buffer.from('d'), ...Object.keys(value).sort().flatMap(key => [encode(key), encode(value[key])]), Buffer.from('e')]);
}
function torrent(name, payload=Buffer.from('independent browser fixture\n')) {
  return encode({info: {name, length: payload.length, 'piece length': 16384, pieces: createHash('sha1').update(payload).digest(), private: 1}});
}
async function freePort() {
  const server=net.createServer();
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  const result=server.address().port;
  await new Promise(resolve=>server.close(resolve));
  return result;
}
async function start() {
  child=spawn(binary, ['--ui','--ui-addr',`127.0.0.1:${port}`,'--port',String(peerPort),'--download-dir',root,'--proxy','socks5://127.0.0.1:9'], {stdio:'ignore'});
  await expect.poll(async()=>{try{return (await fetch(url+'/status')).status;}catch{return 0;}}).toBe(200);
}
async function stop() {
  if(!child||child.exitCode!==null) return;
  const exited=new Promise(resolve=>child.once('exit',resolve));
  child.kill('SIGTERM');
  await exited;
}
async function post(endpoint, body=undefined, type='application/x-www-form-urlencoded') {
  const {token}=await (await fetch(url+'/api-token')).json();
  const response=await fetch(url+endpoint,{method:'POST',headers:{Origin:url,'X-Rustorrent-Token':token,'Content-Type':type},body});
  const result=await response.json();
  expect(response.ok,JSON.stringify(result)).toBeTruthy();
  return result;
}
async function state(){return (await fetch(url+'/status')).json();}
async function add(name, paused=false, seed=true) {
  if(seed) await writeFile(path.join(root,name),'independent browser fixture\n');
  await post('/add-torrent?paused='+(paused?'1':'0'),torrent(name),'application/x-bittorrent');
  await expect.poll(async()=> (await state()).torrents.find(t=>t.name===name)?.files.length||0).toBe(1);
}

test.beforeAll(async()=>{
  root=await mkdtemp(path.join(tmpdir(),'rustorrent-browser-'));
  port=await freePort();peerPort=await freePort();url=`http://127.0.0.1:${port}`;
  await start();
});
test.afterAll(async()=>{await stop();if(root)await rm(root,{recursive:true,force:true});});
test.beforeEach(async()=>{
  for(const item of (await state()).torrents) await post(`/torrent/delete?id=${item.id}&data=0`);
  await expect.poll(async()=>(await state()).torrents.length).toBe(0);
});

test('empty library, keyboard dialog, inline validation, focus restoration',async({page})=>{
  const errors=[];page.on('pageerror',error=>errors.push(error.message));
  await page.goto(url);
  await expect(page.getByRole('heading',{name:'Transfers',exact:true})).toBeVisible();
  await page.getByRole('button',{name:'Add your first torrent'}).click();
  const dialog=page.getByRole('dialog',{name:'Add Torrent'});
  await expect(dialog.getByLabel('Magnet link')).toBeFocused();
  await expect(dialog.getByRole('button',{name:'Add Torrent',exact:true})).toBeDisabled();
  await dialog.getByLabel('Magnet link').fill('not-a-magnet');
  await expect(dialog.locator('#addSummary')).toContainText('Enter a valid magnet');
  await expect(dialog.getByRole('button',{name:'Add Torrent',exact:true})).toBeDisabled();
  await dialog.getByLabel('Magnet link').fill('magnet:?xt=urn:btih:'+'a'.repeat(40)+'&xt=urn:btih:'+'b'.repeat(40));
  await dialog.getByRole('button',{name:'Add Torrent',exact:true}).click();
  await expect(dialog.getByRole('alert')).toContainText('Could not add torrent');
  await page.keyboard.press('Escape');
  await expect(dialog).not.toBeVisible();
  await expect(page.getByRole('button',{name:'Add your first torrent'})).toBeFocused();
  expect(errors).toEqual([]);
});

test('browser upload starts paused and survives restart',async({page})=>{
  await page.goto(url);await page.getByRole('button',{name:'Add Torrent',exact:true}).click();
  const dialog=page.getByRole('dialog',{name:'Add Torrent'});
  await dialog.getByLabel('Torrent file',{exact:true}).setInputFiles({name:'readme.torrent',mimeType:'application/x-bittorrent',buffer:torrent('Read me.txt')});
  await expect(dialog.locator('#addSummary')).not.toContainText('Reading');
  await dialog.getByLabel('Start immediately').uncheck();
  await dialog.getByRole('button',{name:'Add Torrent',exact:true}).click();
  const card=page.locator('.torrent-card').filter({hasText:'Read me.txt'});
  await expect(card.getByRole('button',{name:'Resume',exact:true})).toBeEnabled();
  expect((await state()).torrents[0].paused).toBe(true);
  await stop();await start();
  await expect.poll(async()=>(await state()).torrents[0]?.paused).toBe(true);
  await expect(page.locator('.connection-banner')).toBeHidden({timeout:10000});
  await card.getByRole('button',{name:'Resume',exact:true}).click();
  await expect(card.getByRole('button',{name:'Pause',exact:true})).toBeEnabled();
});

test('filter states and expanded inputs survive live updates',async({page})=>{
  await add('Field recordings.txt');await add('Release notes.txt',true);
  await page.goto(url);
  await page.getByLabel('Filter transfers').fill('no matching name');
  await expect(page.locator('#filterEmpty')).toBeVisible();
  await page.getByRole('button',{name:'Show all transfers'}).click();
  await expect(page.locator('.torrent-card:visible')).toHaveCount(2);
  const card=page.locator('.torrent-card').filter({hasText:'Field recordings.txt'});
  await card.getByRole('button',{name:'Expand',exact:true}).click();
  const input=card.getByLabel('Transfer label');await input.fill('Work in progress');
  const id=(await state()).torrents.find(t=>t.name==='Release notes.txt').id;
  await post(`/torrent/pause?id=${id}`);
  await page.waitForTimeout(1100);
  await expect(input).toHaveValue('Work in progress');await expect(input).toBeFocused();
});

test('remove dialog keeps files by default and deletes only when selected',async({page})=>{
  await add('Keep this.txt');await page.goto(url);
  await page.locator('.torrent-card').getByRole('button',{name:'Remove',exact:true}).click();
  const dialog=page.getByRole('dialog',{name:'Remove transfer?'});
  await expect(dialog.getByLabel('Also delete downloaded files')).not.toBeChecked();
  await dialog.getByRole('button',{name:'Remove transfer',exact:true}).click();
  await expect(page.locator('.torrent-card')).toHaveCount(0);
  expect(await readFile(path.join(root,'Keep this.txt'),'utf8')).toBe('independent browser fixture\n');
  await add('Delete this.txt');
  await expect(page.locator('.torrent-card')).toHaveCount(1);
  await page.locator('.torrent-card').getByRole('button',{name:'Remove',exact:true}).click();
  await dialog.getByLabel('Also delete downloaded files').check();
  await dialog.getByRole('button',{name:'Remove transfer',exact:true}).click();
  await expect(page.locator('.torrent-card')).toHaveCount(0);
  await expect(access(path.join(root,'Delete this.txt'))).rejects.toThrow();
});

test('desktop, dark, narrow layout, and accessibility',async({page})=>{
  await add('Ubuntu installation notes.txt');await add('Archive of photographs.txt',true);await add('A very long filename that should remain readable and never push the transfer controls off the screen.txt',false,false);
  await page.goto(url);
  await expect(page.locator('.torrent-card')).toHaveCount(3);
  await page.screenshot({path:'output/playwright/beta-desktop.png',fullPage:true});
  let scan=await new AxeBuilder({page}).withTags(['wcag2a','wcag2aa','wcag21aa']).analyze();
  expect(scan.violations.map(v=>({id:v.id,nodes:v.nodes.map(n=>n.target)}))).toEqual([]);
  await page.getByRole('button',{name:'Toggle theme'}).click();
  await page.screenshot({path:'output/playwright/beta-dark.png',fullPage:true});
  scan=await new AxeBuilder({page}).withTags(['wcag2a','wcag2aa','wcag21aa']).analyze();
  expect(scan.violations.map(v=>v.id)).toEqual([]);
  await page.setViewportSize({width:390,height:844});
  await page.screenshot({path:'output/playwright/beta-mobile.png',fullPage:true});
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth)).toBe(true);
  await page.getByRole('button',{name:'Add Torrent',exact:true}).click();
  scan=await new AxeBuilder({page}).withTags(['wcag2a','wcag2aa','wcag21aa']).analyze();
  expect(scan.violations.map(v=>({id:v.id,nodes:v.nodes.map(n=>n.target)}))).toEqual([]);
});
