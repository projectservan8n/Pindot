// Windows: the NSIS installer Tauri builds (per user, no administrator).

import { spawn, spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join } from 'node:path';

const UNINSTALL_KEY = 'HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Pindot';

export function run(cmd, args, opts = {}) {
  const r = spawnSync(cmd, args, { encoding: 'utf8', windowsHide: true, ...opts });
  if (r.error) return { status: -1, stdout: '', stderr: r.error.message };
  return { status: r.status, stdout: r.stdout ?? '', stderr: r.stderr ?? '' };
}

/**
 * The installer's command line. NSIS wants `/D=` last and unquoted, spaces
 * and all, so the line is written out here and passed verbatim.
 */
export function commandLine(file, dir) {
  let line = `"${file}" /S`;
  if (dir) {
    if (dir.includes('"')) throw new Error('The folder name has a quote in it; pick another folder.');
    line += ` /D=${dir}`;
  }
  return line;
}

/** The value named `name` in `reg query` output, or null. */
export function parseRegValue(stdout, name) {
  for (const line of stdout.split(/\r?\n/)) {
    const m = /^\s+(.+?)\s{4}(REG_\w+)\s{4}(.*)$/.exec(line);
    if (m && m[1] === name) return m[3].trim();
  }
  return null;
}

export function parseTasklist(stdout) {
  return stdout.split(/\r?\n/).some((l) => l.toLowerCase().startsWith('"pindot.exe"'));
}

/** The folder in the Uninstall entry's `InstallLocation` (written quoted), or null. */
export function installLocation(stdout) {
  const v = parseRegValue(stdout, 'InstallLocation');
  return v ? v.replace(/^"(.*)"$/, '$1') : null;
}

function regQuery(key, name) {
  const r = run('reg', ['query', key, '/v', name]);
  return r.status === 0 ? r.stdout : '';
}

export function isRunning() {
  return parseTasklist(run('tasklist', ['/FI', 'IMAGENAME eq pindot.exe', '/FO', 'CSV', '/NH']).stdout);
}

/** Where Pindot is installed (its Installed-apps entry), or null. */
export function installedDir() {
  const dir = installLocation(regQuery(UNINSTALL_KEY, 'InstallLocation'));
  return dir && existsSync(join(dir, 'pindot.exe')) ? dir : null;
}

export function installedVersion() {
  return installedDir() ? parseRegValue(regQuery(UNINSTALL_KEY, 'DisplayVersion'), 'DisplayVersion') : null;
}

export function installFile(file, dir) {
  commandLine(file, dir); // refuses a folder NSIS could not be given
  // With verbatim arguments Node joins argv0 and the arguments with spaces,
  // unquoted: exactly commandLine(file, dir).
  const args = dir ? ['/S', `/D=${dir}`] : ['/S'];
  const r = spawnSync(file, args, { windowsVerbatimArguments: true, argv0: `"${file}"`, windowsHide: true });
  if (r.error) throw new Error(`The installer did not start: ${r.error.message}`);
  if (r.status !== 0) throw new Error(`The installer stopped with code ${r.status}.`);
  const where = installedDir();
  if (!where) throw new Error('The installer finished, but Pindot is not where it said it would be.');
  return join(where, 'pindot.exe');
}

/** Runs the uninstaller and waits (up to a minute) until the app is gone. */
export async function uninstall() {
  const dir = installedDir();
  if (!dir) return null;
  const exe = join(dir, 'uninstall.exe');
  if (!existsSync(exe)) throw new Error(`There is no uninstaller in ${dir}.`);
  // The NSIS uninstaller copies itself to a temporary folder and returns at
  // once, so wait for the app's files to go.
  const r = spawnSync(exe, ['/S'], { windowsHide: true });
  if (r.error) throw new Error(`The uninstaller did not start: ${r.error.message}`);
  for (let i = 0; i < 120 && existsSync(join(dir, 'pindot.exe')); i++) {
    await new Promise((res) => setTimeout(res, 500));
  }
  if (existsSync(join(dir, 'pindot.exe'))) throw new Error(`The uninstaller did not remove ${dir}.`);
  return dir;
}

export function open(exe) {
  const child = spawn(exe, [], { detached: true, stdio: 'ignore' });
  child.unref();
}

export const dataFolder = () => join(process.env.APPDATA ?? '', 'Pindot');
