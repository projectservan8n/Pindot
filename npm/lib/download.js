// Downloads one file to `dest`, checking its size as it arrives and its
// SHA-256 at the end. A file that fails either check is deleted, never kept.

import { createHash } from 'node:crypto';
import { createWriteStream, renameSync, rmSync } from 'node:fs';
import { once } from 'node:events';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function attempt(url, dest, size, sha256, onProgress) {
  const part = `${dest}.part`;
  const res = await fetch(url, { redirect: 'follow', headers: { 'User-Agent': 'pindot-installer' } });
  if (!res.ok || !res.body) {
    const err = new Error(`The download failed: HTTP ${res.status} from ${new URL(url).host}.`);
    err.retry = res.status >= 500 || res.status === 429;
    throw err;
  }
  const hash = createHash('sha256');
  const out = createWriteStream(part);
  let done = 0;
  try {
    for await (const chunk of res.body) {
      done += chunk.length;
      if (done > size) {
        throw new Error(`The download is larger than the ${size} bytes this release expects; it was not kept.`);
      }
      hash.update(chunk);
      if (!out.write(chunk)) await once(out, 'drain');
      onProgress?.(done, size);
    }
    out.end();
    await once(out, 'close');
  } catch (e) {
    out.destroy();
    rmSync(part, { force: true });
    throw e;
  }
  if (done !== size) {
    rmSync(part, { force: true });
    const err = new Error(`The download stopped at ${done} of ${size} bytes.`);
    err.retry = true;
    throw err;
  }
  if (hash.digest('hex') !== sha256) {
    rmSync(part, { force: true });
    throw new Error('The downloaded file does not match its checksum (SHA-256); it was deleted, not installed.');
  }
  renameSync(part, dest);
}

export async function download(url, dest, { size, sha256, onProgress, retries = 3, backoffMs = 1500 } = {}) {
  let last;
  for (let i = 1; i <= retries; i++) {
    try {
      return await attempt(url, dest, size, sha256, onProgress);
    } catch (e) {
      last = e;
      // A network error has no status; a checksum or size failure is final.
      const retry = e.retry ?? (e.name === 'TypeError' || e.code === 'ECONNRESET');
      if (!retry || i === retries) break;
      await sleep(backoffMs * i);
    }
  }
  throw last;
}
