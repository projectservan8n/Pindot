// Tests for scripts/release-npm.mjs, the maintainers' tool that pins the
// built installers into release.json. It is not part of the published package.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { assetKeys, pinFiles } from '../../scripts/release-npm.mjs';

test('each installer name maps to the computers it is for', () => {
  assert.deepEqual(assetKeys('Pindot_0.0.1_x64-setup.exe', '0.0.1'), ['win32-x64']);
  assert.deepEqual(assetKeys('Pindot_0.0.1_aarch64.dmg', '0.0.1'), ['darwin-arm64']);
  assert.deepEqual(assetKeys('Pindot_0.0.1_x64.dmg', '0.0.1'), ['darwin-x64']);
  assert.deepEqual(assetKeys('Pindot_0.0.1_universal.dmg', '0.0.1'), ['darwin-arm64', 'darwin-x64']);
});

test('a file of another version or kind is refused', () => {
  assert.throws(() => assetKeys('Pindot_0.0.2_x64-setup.exe', '0.0.1'), /version 0\.0\.1/);
  assert.throws(() => assetKeys('Pindot_0.0.1_x64_en-US.msi', '0.0.1'), /not an installer/);
});

test('pinning adds files to the release and keeps the other platform', () => {
  const dir = mkdtempSync(join(tmpdir(), 'pindot-pin-'));
  const exe = join(dir, 'Pindot_0.0.1_x64-setup.exe');
  writeFileSync(exe, 'abc');
  const before = {
    version: '0.0.1',
    repo: 'projectservan8n/Pindot',
    tag: 'v0.0.1',
    assets: { 'darwin-arm64': { file: 'Pindot_0.0.1_aarch64.dmg', size: 5, sha256: 'f'.repeat(64) } },
  };
  const after = pinFiles(before, '0.0.1', [exe]);
  assert.deepEqual(after.assets['win32-x64'], {
    file: 'Pindot_0.0.1_x64-setup.exe',
    size: 3,
    sha256: 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
  });
  assert.deepEqual(after.assets['darwin-arm64'], before.assets['darwin-arm64']);
  rmSync(dir, { recursive: true, force: true });
});

test('a new version starts with no files from the old one', () => {
  const dir = mkdtempSync(join(tmpdir(), 'pindot-pin-'));
  const exe = join(dir, 'Pindot_0.0.2_x64-setup.exe');
  writeFileSync(exe, 'abc');
  const old = { version: '0.0.1', repo: 'projectservan8n/Pindot', tag: 'v0.0.1', assets: { 'darwin-arm64': {} } };
  const after = pinFiles(old, '0.0.2', [exe]);
  assert.equal(after.version, '0.0.2');
  assert.equal(after.tag, 'v0.0.2');
  assert.deepEqual(Object.keys(after.assets), ['win32-x64']);
  rmSync(dir, { recursive: true, force: true });
});
