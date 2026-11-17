#!/usr/bin/env node
// Pins built installers into npm/release.json for the `pindot` npm package,
// and checks a published GitHub release against the pins.
//
//   node scripts/release-npm.mjs add <installer>...   pin files (size + SHA-256)
//   node scripts/release-npm.mjs check                download each pinned file
//                                                     from the release and compare
//
// The version comes from app/src-tauri/tauri.conf.json. Windows is pinned on
// the PC and macOS on the Mac; each `add` keeps the other platform's pin.

import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync, statSync, existsSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const RELEASE_JSON = join(ROOT, 'npm', 'release.json');
const REPO = 'projectservan8n/Pindot';

const KINDS = [
  [/_x64-setup\.exe$/, ['win32-x64']],
  [/_aarch64\.dmg$/, ['darwin-arm64']],
  [/_x64\.dmg$/, ['darwin-x64']],
  [/_universal\.dmg$/, ['darwin-arm64', 'darwin-x64']],
];

export function assetKeys(name, version) {
  const kind = KINDS.find(([re]) => re.test(name));
  if (!kind) throw new Error(`${name} is not an installer this package knows (x64-setup.exe or .dmg).`);
  if (!name.startsWith(`Pindot_${version}_`)) throw new Error(`${name} is not version ${version}.`);
  return kind[1];
}

const sha256 = (file) => createHash('sha256').update(readFileSync(file)).digest('hex');

export function pinFiles(release, version, files) {
  const fresh = !release || release.version !== version;
  const out = {
    version,
    repo: REPO,
    tag: `v${version}`,
    assets: fresh ? {} : { ...release.assets },
  };
  for (const f of files) {
    const name = basename(f);
    const pin = { file: name, size: statSync(f).size, sha256: sha256(f) };
    for (const key of assetKeys(name, version)) out.assets[key] = pin;
  }
  out.assets = Object.fromEntries(Object.entries(out.assets).sort(([a], [b]) => a.localeCompare(b)));
  return out;
}

function appVersion() {
  return JSON.parse(readFileSync(join(ROOT, 'app', 'src-tauri', 'tauri.conf.json'), 'utf8')).version;
}

async function check(release) {
  let bad = 0;
  const seen = new Set();
  for (const [key, a] of Object.entries(release.assets)) {
    if (seen.has(a.file)) continue;
    seen.add(a.file);
    const url = `https://github.com/${release.repo}/releases/download/${release.tag}/${encodeURIComponent(a.file)}`;
    const res = await fetch(url, { redirect: 'follow' });
    if (!res.ok) {
      console.log(`FAIL ${key}: ${a.file}: HTTP ${res.status} (is the release published?)`);
      bad++;
      continue;
    }
    const buf = Buffer.from(await res.arrayBuffer());
    const hash = createHash('sha256').update(buf).digest('hex');
    const ok = buf.length === a.size && hash === a.sha256;
    console.log(`${ok ? 'ok  ' : 'FAIL'} ${key}: ${a.file} ${buf.length} bytes ${hash}`);
    if (!ok) bad++;
  }
  return bad;
}

async function cli(argv) {
  const [cmd, ...files] = argv;
  const current = existsSync(RELEASE_JSON) ? JSON.parse(readFileSync(RELEASE_JSON, 'utf8')) : null;
  if (cmd === 'add' && files.length > 0) {
    const next = pinFiles(current, appVersion(), files);
    writeFileSync(RELEASE_JSON, `${JSON.stringify(next, null, 2)}\n`);
    for (const [k, a] of Object.entries(next.assets)) console.log(`${k}: ${a.file} ${a.size} bytes ${a.sha256}`);
    return 0;
  }
  if (cmd === 'check' && current) return (await check(current)) === 0 ? 0 : 1;
  console.error('usage: release-npm.mjs add <installer>... | check');
  return 2;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = await cli(process.argv.slice(2));
}
