import { test } from 'node:test';
import assert from 'node:assert/strict';
import { pickAsset, assetUrl, platformKey, platformLabel } from '../lib/release.js';

const release = {
  version: '0.0.1',
  repo: 'projectservan8n/Pindot',
  tag: 'v0.0.1',
  assets: {
    'win32-x64': { file: 'Pindot_0.0.1_x64-setup.exe', size: 10, sha256: 'a'.repeat(64) },
    'darwin-arm64': { file: 'Pindot_0.0.1_aarch64.dmg', size: 20, sha256: 'b'.repeat(64) },
  },
};

test('a Windows PC gets the x64 installer, on Arm too (it runs under emulation)', () => {
  assert.equal(platformKey('win32', 'x64'), 'win32-x64');
  assert.equal(platformKey('win32', 'arm64'), 'win32-x64');
  assert.equal(pickAsset(release, 'win32', 'x64').file, 'Pindot_0.0.1_x64-setup.exe');
});

test('an Apple silicon Mac gets the arm64 disk image', () => {
  assert.equal(pickAsset(release, 'darwin', 'arm64').file, 'Pindot_0.0.1_aarch64.dmg');
});

test('an Intel Mac is told there is no build for it, not given the arm64 one', () => {
  assert.throws(() => pickAsset(release, 'darwin', 'x64'), /no Pindot 0\.0\.1 for a Mac with an Intel processor/i);
});

test('Linux is refused with a plain sentence', () => {
  assert.throws(() => pickAsset(release, 'linux', 'x64'), /runs on macOS and Windows only/);
});

test('a release that lacks this platform says so', () => {
  const winOnly = { ...release, assets: { 'win32-x64': release.assets['win32-x64'] } };
  assert.throws(() => pickAsset(winOnly, 'darwin', 'arm64'), /no Pindot 0\.0\.1 for a Mac with Apple silicon/i);
});

test('an asset with a bad hash or size in the release file is refused', () => {
  const bad = { ...release, assets: { 'win32-x64': { file: 'x.exe', size: 10, sha256: 'nothex' } } };
  assert.throws(() => pickAsset(bad, 'win32', 'x64'), /damaged/);
  const noSize = { ...release, assets: { 'win32-x64': { file: 'x.exe', size: 0, sha256: 'a'.repeat(64) } } };
  assert.throws(() => pickAsset(noSize, 'win32', 'x64'), /damaged/);
});

test('the download URL is the GitHub release asset, or a mirror base when one is given', () => {
  const a = release.assets['win32-x64'];
  assert.equal(
    assetUrl(release, a),
    'https://github.com/projectservan8n/Pindot/releases/download/v0.0.1/Pindot_0.0.1_x64-setup.exe',
  );
  assert.equal(assetUrl(release, a, 'http://127.0.0.1:9/files/'), 'http://127.0.0.1:9/files/Pindot_0.0.1_x64-setup.exe');
  assert.equal(assetUrl(release, a, 'http://127.0.0.1:9/files'), 'http://127.0.0.1:9/files/Pindot_0.0.1_x64-setup.exe');
});

test('platform labels read as words', () => {
  assert.equal(platformLabel('win32-x64'), 'Windows');
  assert.equal(platformLabel('darwin-arm64'), 'a Mac with Apple silicon');
});
