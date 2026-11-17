import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { download } from '../lib/download.js';

const body = Buffer.from('pretend this is an installer '.repeat(1000));
const sha256 = createHash('sha256').update(body).digest('hex');
let server;
let base;
let hits = {};
const dir = mkdtempSync(join(tmpdir(), 'pindot-npm-test-'));

before(async () => {
  server = createServer((req, res) => {
    hits[req.url] = (hits[req.url] ?? 0) + 1;
    if (req.url === '/redirect') {
      res.writeHead(302, { Location: '/file' });
      return res.end();
    }
    if (req.url === '/file') {
      res.writeHead(200, { 'Content-Length': body.length });
      return res.end(body);
    }
    if (req.url === '/flaky' && hits[req.url] === 1) {
      res.writeHead(503);
      return res.end('busy');
    }
    if (req.url === '/flaky') {
      res.writeHead(200);
      return res.end(body);
    }
    if (req.url === '/too-big') {
      res.writeHead(200);
      return res.end(Buffer.concat([body, body]));
    }
    res.writeHead(404);
    res.end('not found');
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  base = `http://127.0.0.1:${server.address().port}`;
});

after(() => {
  server.close();
  rmSync(dir, { recursive: true, force: true });
});

test('a download that matches the size and SHA-256 is kept, through a redirect', async () => {
  const dest = join(dir, 'ok.bin');
  const seen = [];
  await download(`${base}/redirect`, dest, { size: body.length, sha256, onProgress: (d, t) => seen.push([d, t]) });
  assert.deepEqual(readFileSync(dest), body);
  assert.equal(seen.at(-1)[0], body.length);
  assert.equal(seen.at(-1)[1], body.length);
});

test('a download with the wrong SHA-256 is deleted and refused', async () => {
  const dest = join(dir, 'bad.bin');
  await assert.rejects(download(`${base}/file`, dest, { size: body.length, sha256: '0'.repeat(64), retries: 1 }), /checksum/i);
  assert.equal(existsSync(dest), false);
  assert.equal(existsSync(`${dest}.part`), false);
});

test('a download longer than the pinned size is stopped and refused', async () => {
  const dest = join(dir, 'big.bin');
  await assert.rejects(download(`${base}/too-big`, dest, { size: body.length, sha256, retries: 1 }), /size|larger/i);
  assert.equal(existsSync(dest), false);
});

test('a server error is retried', async () => {
  const dest = join(dir, 'flaky.bin');
  await download(`${base}/flaky`, dest, { size: body.length, sha256, retries: 3, backoffMs: 1 });
  assert.deepEqual(readFileSync(dest), body);
  assert.equal(hits['/flaky'], 2);
});

test('a missing file says so with the HTTP status', async () => {
  const dest = join(dir, 'missing.bin');
  await assert.rejects(download(`${base}/nope`, dest, { size: 1, sha256, retries: 1 }), /404/);
});
