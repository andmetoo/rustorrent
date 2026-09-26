// Builds a realistic local library for screenshots and manual UI work. Payloads are
// sparse zero-filled files, so multi-gigabyte torrents cost almost no disk space, and
// small wire-protocol peers make transfers genuinely download and upload.
// Standalone: node tests/browser/demo-library.mjs [path/to/rustorrent]
import { createHash, randomBytes } from 'node:crypto';
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, open, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import net from 'node:net';

const MiB = 1024 * 1024, GiB = 1024 * MiB;
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
function encode(value) {
  if (Buffer.isBuffer(value)) return Buffer.concat([Buffer.from(value.length + ':'), value]);
  if (typeof value === 'string') return encode(Buffer.from(value));
  if (typeof value === 'number') return Buffer.from('i' + value + 'e');
  if (Array.isArray(value)) return Buffer.concat([Buffer.from('l'), ...value.map(encode), Buffer.from('e')]);
  return Buffer.concat([Buffer.from('d'), ...Object.keys(value).sort().flatMap(key => [encode(key), encode(value[key])]), Buffer.from('e')]);
}
function makeTorrent(name, files, pieceLength = 4 * MiB) {
  const total = files.reduce((sum, file) => sum + file.length, 0);
  const full = createHash('sha1').update(Buffer.alloc(pieceLength)).digest();
  const count = Math.ceil(total / pieceLength), rest = total % pieceLength;
  const pieces = Buffer.concat(Array.from({ length: count }, (_, i) =>
    i === count - 1 && rest ? createHash('sha1').update(Buffer.alloc(rest)).digest() : full));
  const info = { name, 'piece length': pieceLength, pieces, private: 1 };
  if (files.length === 1 && !files[0].path) info.length = total;
  else info.files = files.map(file => ({ length: file.length, path: file.path }));
  return { name, files, total, pieceLength, count, data: encode({ info }), hash: createHash('sha1').update(encode(info)).digest() };
}
async function sparse(file, size) {
  await mkdir(path.dirname(file), { recursive: true });
  const handle = await open(file, 'w');
  await handle.truncate(size);
  await handle.close();
}
// Writes `fraction` of the torrent's bytes (in file order) into `root`.
async function materialize(root, torrent, fraction) {
  let budget = Math.floor(torrent.total * fraction);
  for (const file of torrent.files) {
    const target = file.path ? path.join(root, torrent.name, ...file.path) : path.join(root, torrent.name);
    const size = Math.min(file.length, budget);
    budget -= size;
    if (size > 0) await sparse(target, size);
  }
}
function freePort() {
  return new Promise(resolve => {
    const server = net.createServer();
    server.listen(0, '127.0.0.1', () => { const { port } = server.address(); server.close(() => resolve(port)); });
  });
}
// A minimal peer that connects to the app. As a seeder it serves zero-filled blocks
// at `rate` bytes/s; as a leecher it requests blocks at `rate` bytes/s.
function peer(port, torrent, role, rate) {
  const socket = net.connect(port, '127.0.0.1');
  let buffer = Buffer.alloc(0), handshaken = false, unchoked = false, budget = 0, next = 0;
  const queue = [];
  const send = (id, payload = Buffer.alloc(0)) => {
    const head = Buffer.alloc(5); head.writeUInt32BE(payload.length + 1); head[4] = id;
    socket.write(Buffer.concat([head, payload]));
  };
  const reserved = Buffer.from([0, 0, 0, 0, 0, 0x10, 0, 0]);
  socket.on('connect', () => socket.write(Buffer.concat([Buffer.from('\x13BitTorrent protocol'), reserved, torrent.hash, Buffer.from('-DM0001-'), randomBytes(12)])));
  const timer = setInterval(() => {
    budget = Math.min(budget + rate / 20, rate);
    if (role === 'seed') {
      while (queue.length && budget >= queue[0].length) {
        const { index, begin, length } = queue.shift(); budget -= length;
        const payload = Buffer.alloc(8 + length); payload.writeUInt32BE(index); payload.writeUInt32BE(begin, 4);
        send(7, payload);
      }
    } else if (unchoked) {
      while (budget >= 16384) {
        budget -= 16384;
        const index = Math.floor(next / torrent.pieceLength) % torrent.count, begin = next % torrent.pieceLength;
        const length = Math.min(16384, torrent.pieceLength - begin, torrent.total - index * torrent.pieceLength - begin);
        next = index * torrent.pieceLength + begin + length;
        const request = Buffer.alloc(12); request.writeUInt32BE(index); request.writeUInt32BE(begin, 4); request.writeUInt32BE(length, 8);
        send(6, request);
      }
    }
  }, 50);
  socket.on('data', chunk => {
    buffer = Buffer.concat([buffer, chunk]);
    if (!handshaken) {
      if (buffer.length < 68) return;
      buffer = buffer.subarray(68); handshaken = true;
      if (role === 'seed') {
        const bits = Buffer.alloc(Math.ceil(torrent.count / 8), 0xff);
        const spare = bits.length * 8 - torrent.count;
        bits[bits.length - 1] = (0xff << spare) & 0xff;
        send(5, bits); send(1);
      } else send(2);
    }
    while (buffer.length >= 4) {
      const size = buffer.readUInt32BE(0);
      if (buffer.length < 4 + size) break;
      const message = buffer.subarray(4, 4 + size); buffer = buffer.subarray(4 + size);
      if (!size) continue;
      if (message[0] === 1) unchoked = true;
      if (message[0] === 0) unchoked = false;
      if (message[0] === 6 && role === 'seed') queue.push({ index: message.readUInt32BE(1), begin: message.readUInt32BE(5), length: message.readUInt32BE(9) });
    }
  });
  const stop = () => { clearInterval(timer); socket.destroy(); };
  socket.on('error', stop); socket.on('close', () => clearInterval(timer));
  return stop;
}

export async function demoLibrary(binary = path.resolve('target/release/rustorrent')) {
  const root = await mkdtemp(path.join(tmpdir(), 'rustorrent-demo-'));
  const [ui, peerPort] = [await freePort(), await freePort()];
  const url = `http://127.0.0.1:${ui}`;
  const child = spawn(binary, ['--ui', '--ui-addr', `127.0.0.1:${ui}`, '--port', String(peerPort), '--no-port-mapping', '--no-utp', '--max-active', '8', '--download-dir', root], { stdio: 'ignore' });
  const status = async () => (await fetch(url + '/status')).json();
  for (let i = 0; i < 100; i++) { try { await status(); break; } catch { await sleep(100); } }
  const post = async (endpoint, body, type = 'application/x-www-form-urlencoded') => {
    const { token } = await (await fetch(url + '/api-token')).json();
    const response = await fetch(url + endpoint, { method: 'POST', headers: { Origin: url, 'X-Rustorrent-Token': token, 'Content-Type': type }, body });
    const result = await response.json();
    if (!response.ok) throw new Error(endpoint + ': ' + result.error);
    return result;
  };
  const until = async (id, test) => {
    for (let i = 0; i < 600; i++) { const t = (await status()).torrents.find(x => x.id === id); if (t && test(t)) return t; await sleep(100); }
    throw new Error('demo torrent did not settle: ' + JSON.stringify((await status()).torrents.find(x => x.id === id)).slice(0, 400));
  };
  const track = (name, sizes) => sizes.map(([file, length]) => ({ path: [file], length }));
  const library = [
    { t: makeTorrent('ubuntu-25.10-desktop-amd64.iso', [{ length: Math.round(5.7 * GiB) }]), have: 0.38, label: 'Linux', seed: 2.6 * MiB },
    { t: makeTorrent('Field Recordings — Spring Sessions', track('', [['01 Dawn Chorus.flac', 212 * MiB], ['02 River Delta.flac', 348 * MiB], ['03 Night Market.flac', 296 * MiB], ['04 Thunderstorm.flac', 404 * MiB], ['cover.jpg', 3 * MiB], ['notes.txt', 12000]]), 1 * MiB), have: 0.72, label: 'Audio', seed: 0.9 * MiB },
    { t: makeTorrent('Blender 4.5 Splash Demo Files', track('', [['splash.blend', 612 * MiB], ['textures.zip', 248 * MiB], ['README.md', 4200]])), have: 1, leech: 1.3 * MiB },
    { t: makeTorrent('Night of the Living Dead (1968) — Public Domain', [{ length: Math.round(1.9 * GiB) }]), have: 0.12, pause: true },
    { t: makeTorrent('wikipedia_en_all_maxi_2026-09.zim', [{ length: Math.round(3.2 * GiB) }]), have: 1, label: 'Reference' },
  ];
  const stops = [];
  for (const item of library) {
    await materialize(root, item.t, item.have);
    const { torrent_id: id } = await post('/add-torrent', item.t.data, 'application/x-bittorrent');
    await until(id, t => !/^(queued|loading|checking|pending)$/.test(t.status) && (item.have < 1 || t.completed_bytes >= t.total_bytes));
    if (item.label) await post('/torrent/set-label', new URLSearchParams({ id, label: item.label }).toString());
    if (item.pause) await post(`/torrent/pause?id=${id}`);
    if (item.seed) stops.push(peer(peerPort, item.t, 'seed', item.seed));
    if (item.leech) stops.push(peer(peerPort, item.t, 'leech', item.leech));
  }
  // An existing, larger file blocks preallocation: a real, recoverable error state.
  const blocked = makeTorrent('debian-13.1.0-amd64-netinst.iso', [{ length: 754 * MiB }]);
  await sparse(path.join(root, blocked.name), 800 * MiB);
  const { torrent_id: errorId } = await post('/add-torrent?prealloc=1', blocked.data, 'application/x-bittorrent');
  await until(errorId, t => t.status === 'error');
  const close = async () => {
    stops.forEach(stop => stop());
    if (child.exitCode === null) { const exited = new Promise(resolve => child.once('exit', resolve)); child.kill('SIGTERM'); await exited; }
    await rm(root, { recursive: true, force: true });
  };
  return { url, root, status, post, close };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const demo = await demoLibrary(process.argv[2] && path.resolve(process.argv[2]));
  console.log(`Demo library running at ${demo.url} (Ctrl+C to stop)`);
  process.on('SIGINT', () => demo.close().then(() => process.exit(0)));
}
