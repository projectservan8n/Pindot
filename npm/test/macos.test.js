import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, existsSync, cpSync, rmSync, readdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { installApp, applicationsFolder, trashApp } from '../lib/macos.js';

// A stand-in for hdiutil and ditto: attach "mounts" a folder holding
// Pindot.app with the given version, ditto copies, detach removes.
function fakeRun(version, { dittoFails = false } = {}) {
  const calls = [];
  const run = (cmd, args) => {
    calls.push([cmd, ...args]);
    if (cmd === 'hdiutil' && args[0] === 'attach') {
      const mnt = args[args.indexOf('-mountpoint') + 1];
      mkdirSync(join(mnt, 'Pindot.app', 'Contents'), { recursive: true });
      writeFileSync(join(mnt, 'Pindot.app', 'Contents', 'version'), version);
      return { status: 0, stdout: '', stderr: '' };
    }
    if (cmd === 'hdiutil' && args[0] === 'detach') {
      rmSync(args[1], { recursive: true, force: true });
      return { status: 0, stdout: '', stderr: '' };
    }
    if (cmd === 'ditto') {
      if (dittoFails) return { status: 1, stdout: '', stderr: 'ditto: No space left on device' };
      cpSync(args[0], args[1], { recursive: true });
      return { status: 0, stdout: '', stderr: '' };
    }
    return { status: 1, stdout: '', stderr: `unexpected ${cmd}` };
  };
  return { run, calls };
}

test('the app is copied out of the disk image into the folder, and the image is detached', () => {
  const apps = mkdtempSync(join(tmpdir(), 'pindot-apps-'));
  const { run, calls } = fakeRun('0.0.2');
  const app = installApp('/tmp/Pindot_0.0.2_aarch64.dmg', apps, run);
  assert.equal(app, join(apps, 'Pindot.app'));
  assert.equal(readFileSync(join(app, 'Contents', 'version'), 'utf8'), '0.0.2');
  const attach = calls.find((c) => c[1] === 'attach');
  assert.ok(attach.includes('-nobrowse') && attach.includes('-readonly') && attach.includes('-noautoopen'));
  assert.equal(attach.at(-1), '/tmp/Pindot_0.0.2_aarch64.dmg');
  assert.ok(calls.some((c) => c[1] === 'detach'));
  rmSync(apps, { recursive: true, force: true });
});

test('an update replaces the old app and leaves no backup behind', () => {
  const apps = mkdtempSync(join(tmpdir(), 'pindot-apps-'));
  mkdirSync(join(apps, 'Pindot.app', 'Contents'), { recursive: true });
  writeFileSync(join(apps, 'Pindot.app', 'Contents', 'version'), '0.0.1');
  installApp('/tmp/new.dmg', apps, fakeRun('0.0.2').run);
  assert.equal(readFileSync(join(apps, 'Pindot.app', 'Contents', 'version'), 'utf8'), '0.0.2');
  assert.deepEqual(readdirSync(apps), ['Pindot.app']);
  rmSync(apps, { recursive: true, force: true });
});

test('a failed copy puts the old app back and still detaches the image', () => {
  const apps = mkdtempSync(join(tmpdir(), 'pindot-apps-'));
  mkdirSync(join(apps, 'Pindot.app', 'Contents'), { recursive: true });
  writeFileSync(join(apps, 'Pindot.app', 'Contents', 'version'), '0.0.1');
  const { run, calls } = fakeRun('0.0.2', { dittoFails: true });
  assert.throws(() => installApp('/tmp/new.dmg', apps, run), /No space left/);
  assert.equal(readFileSync(join(apps, 'Pindot.app', 'Contents', 'version'), 'utf8'), '0.0.1');
  assert.deepEqual(readdirSync(apps), ['Pindot.app']);
  assert.ok(calls.some((c) => c[1] === 'detach'));
  rmSync(apps, { recursive: true, force: true });
});

test('/Applications when it can be written, else ~/Applications', () => {
  assert.equal(applicationsFolder('/Users/me', () => true), '/Applications');
  assert.equal(applicationsFolder('/Users/me', () => false), '/Users/me/Applications');
});

test('uninstall moves the app to the Trash under a free name', () => {
  const home = mkdtempSync(join(tmpdir(), 'pindot-home-'));
  const apps = join(home, 'Applications');
  mkdirSync(join(apps, 'Pindot.app'), { recursive: true });
  mkdirSync(join(home, '.Trash', 'Pindot.app'), { recursive: true });
  const moved = trashApp(join(apps, 'Pindot.app'), home);
  assert.equal(existsSync(join(apps, 'Pindot.app')), false);
  assert.ok(existsSync(moved));
  assert.notEqual(moved, join(home, '.Trash', 'Pindot.app'));
  rmSync(home, { recursive: true, force: true });
});
