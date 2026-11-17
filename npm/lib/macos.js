// macOS: the .dmg Tauri builds. The app is copied out of the disk image into
// /Applications (or ~/Applications), where its privacy grants stay put.

import { spawn, spawnSync } from 'node:child_process';
import { accessSync, constants, existsSync, mkdirSync, mkdtempSync, renameSync, rmSync } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { join, posix } from 'node:path';

export const APP = 'Pindot.app';

export function run(cmd, args) {
  const r = spawnSync(cmd, args, { encoding: 'utf8' });
  if (r.error) return { status: -1, stdout: '', stderr: r.error.message };
  return { status: r.status, stdout: r.stdout ?? '', stderr: r.stderr ?? '' };
}

function writable(dir) {
  try {
    accessSync(dir, constants.W_OK);
    return true;
  } catch {
    return false;
  }
}

/** /Applications when this user may write there, else ~/Applications. */
export function applicationsFolder(home = homedir(), canWrite = writable) {
  return canWrite('/Applications') ? '/Applications' : posix.join(home, 'Applications');
}

/** The installed Pindot.app (in `dir`, /Applications or ~/Applications), or null. */
export function installedApp(dir = null) {
  const places = dir ? [dir] : ['/Applications', join(homedir(), 'Applications')];
  for (const p of places) {
    if (existsSync(join(p, APP))) return join(p, APP);
  }
  return null;
}

export function installedVersion(app) {
  const r = run('plutil', ['-extract', 'CFBundleShortVersionString', 'raw', '-o', '-', join(app, 'Contents', 'Info.plist')]);
  return r.status === 0 ? r.stdout.trim() || null : null;
}

export function isRunning() {
  return run('pgrep', ['-x', 'pindot']).status === 0;
}

/**
 * Copies Pindot.app out of `dmg` into `folder`, replacing an older copy. If
 * the copy fails the old app is put back. The image is always detached.
 */
export function installApp(dmg, folder, runCmd = run) {
  mkdirSync(folder, { recursive: true });
  const mnt = mkdtempSync(join(tmpdir(), 'pindot-dmg-'));
  const attach = runCmd('hdiutil', ['attach', '-nobrowse', '-readonly', '-noautoopen', '-mountpoint', mnt, dmg]);
  if (attach.status !== 0) {
    rmSync(mnt, { recursive: true, force: true });
    throw new Error(`The disk image did not open: ${attach.stderr.trim() || `hdiutil exit ${attach.status}`}`);
  }
  const dest = join(folder, APP);
  const backup = join(folder, `.${APP}.old-${process.pid}`);
  try {
    const src = join(mnt, APP);
    if (!existsSync(src)) throw new Error(`The disk image has no ${APP}.`);
    if (existsSync(dest)) renameSync(dest, backup);
    const copy = runCmd('ditto', [src, dest]);
    if (copy.status !== 0) {
      rmSync(dest, { recursive: true, force: true });
      if (existsSync(backup)) renameSync(backup, dest);
      throw new Error(`Copying Pindot failed: ${copy.stderr.trim() || `ditto exit ${copy.status}`}`);
    }
    rmSync(backup, { recursive: true, force: true });
    return dest;
  } finally {
    const d = runCmd('hdiutil', ['detach', mnt, '-quiet']);
    if (d.status !== 0) runCmd('hdiutil', ['detach', mnt, '-force', '-quiet']);
    rmSync(mnt, { recursive: true, force: true });
  }
}

/** Moves the app to the Trash (under a free name); returns where it went. */
export function trashApp(app, home = homedir()) {
  const trash = join(home, '.Trash');
  mkdirSync(trash, { recursive: true });
  let target = join(trash, APP);
  for (let i = 2; existsSync(target); i++) target = join(trash, `Pindot ${i}.app`);
  renameSync(app, target);
  return target;
}

export function open(app) {
  const child = spawn('open', [app], { detached: true, stdio: 'ignore' });
  child.unref();
}

export const dataFolder = () => join(homedir(), 'Library', 'Application Support', 'Pindot');
