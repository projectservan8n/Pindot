// `npx pindot-app`: download the ready-made Pindot for this computer from the
// project's GitHub release, check it, install it and open it.

import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { assetUrl, loadRelease, pickAsset, platformLabel } from './release.js';
import { download } from './download.js';

const HELP = `Pindot installer

Usage:
  npx pindot-app                 install or update Pindot, then open it
  npx pindot-app install [options]
  npx pindot-app uninstall [--dir <folder>]
  npx pindot-app --version | --help

Options:
  --dir <folder>   install into this folder (Windows: the program folder;
                   macOS: the folder that gets Pindot.app, /Applications
                   by default)
  --no-open        do not open Pindot afterwards
  --reinstall      install even when this version is already there

Your settings, voice models and Vault are kept on update and uninstall.
More: https://pindot.click`;

export function parseArgs(argv) {
  const out = { command: 'install', dir: null, open: true, reinstall: false };
  let commandSeen = false;
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--help' || a === '-h') out.command = 'help';
    else if (a === '--version' || a === '-v') out.command = 'version';
    else if (a === '--no-open') out.open = false;
    else if (a === '--reinstall') out.reinstall = true;
    else if (a === '--dir') {
      const v = argv[++i];
      if (!v || v.startsWith('--')) throw new Error('--dir needs a folder.');
      out.dir = v;
    } else if (a.startsWith('--dir=')) {
      out.dir = a.slice('--dir='.length);
      if (!out.dir) throw new Error('--dir needs a folder.');
    } else if (a.startsWith('-')) throw new Error(`unknown option "${a}" (try --help)`);
    else if (!commandSeen && (a === 'install' || a === 'uninstall')) {
      if (out.command === 'install') out.command = a;
      commandSeen = true;
    } else throw new Error(`unknown command "${a}" (try --help)`);
  }
  return out;
}

export function compareVersions(a, b) {
  const pa = String(a).split('.').map(Number);
  const pb = String(b).split('.').map(Number);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d !== 0) return d < 0 ? -1 : 1;
  }
  return 0;
}

const mb = (n) => `${(n / 1e6).toFixed(1)} MB`;

function progress(label) {
  const tty = process.stderr.isTTY;
  let lastPct = -1;
  return (done, total) => {
    const pct = Math.floor((done / total) * 100);
    if (pct === lastPct) return;
    lastPct = pct;
    if (tty) process.stderr.write(`\r${label}: ${mb(done)} of ${mb(total)} (${pct}%)   `);
    if (tty && done === total) process.stderr.write('\n');
  };
}

async function platformModule() {
  if (process.platform === 'win32') return import('./windows.js');
  if (process.platform === 'darwin') return import('./macos.js');
  throw new Error('Pindot runs on macOS and Windows only.');
}

async function install(opts, release) {
  const asset = pickAsset(release, process.platform, process.arch);
  const os = await platformModule();
  const mac = process.platform === 'darwin';

  const existing = mac ? os.installedApp(opts.dir) : os.installedDir();
  const have = existing ? (mac ? os.installedVersion(existing) : os.installedVersion()) : null;
  const target = mac ? existing : existing && `${existing}\\pindot.exe`;
  if (have && !opts.reinstall && compareVersions(have, release.version) >= 0) {
    console.log(`Pindot ${have} is already installed${compareVersions(have, release.version) > 0 ? ' (newer than this installer)' : ''}.`);
    if (opts.open) {
      os.open(target);
      console.log('Opened it. Look for its icon in the ' + (mac ? 'menu bar.' : 'tray, by the clock.'));
    }
    return;
  }
  if (os.isRunning()) {
    throw new Error(
      'Pindot is running. Quit it first (its ' +
        (mac ? 'menu-bar icon' : 'tray icon') +
        ' > Quit Pindot), then run this again.',
    );
  }

  console.log(`${have ? `Updating Pindot ${have} to` : 'Installing Pindot'} ${release.version} for ${platformLabel(asset.key)}.`);
  const work = mkdtempSync(join(tmpdir(), 'pindot-install-'));
  try {
    const file = join(work, asset.file);
    const url = assetUrl(release, asset, process.env.PINDOT_RELEASE_BASE || null);
    if (!process.stderr.isTTY) console.log(`Downloading ${asset.file} (${mb(asset.size)})...`);
    await download(url, file, { size: asset.size, sha256: asset.sha256, onProgress: progress(`Downloading ${asset.file}`) });
    console.log('Checked: the file matches its SHA-256.');

    let app;
    if (mac) {
      app = os.installApp(file, opts.dir ?? (existing ? dirname(existing) : os.applicationsFolder()));
    } else {
      app = os.installFile(file, opts.dir);
    }
    console.log(`Installed: ${app}`);
    if (opts.open) {
      os.open(app);
      console.log(
        mac
          ? 'Opened Pindot. Its Setup window walks you through the brain, the permissions and the voice models.'
          : 'Opened Pindot. Its Setup window walks you through the brain and the voice models.',
      );
    }
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

async function uninstall(opts) {
  const os = await platformModule();
  const mac = process.platform === 'darwin';
  if (os.isRunning()) {
    throw new Error('Pindot is running. Quit it first (' + (mac ? 'menu-bar' : 'tray') + ' icon > Quit Pindot).');
  }
  if (mac) {
    const app = os.installedApp(opts.dir);
    if (!app) return console.log('Pindot is not installed.');
    const moved = os.trashApp(app);
    console.log(`Moved ${app} to the Trash (${moved}).`);
  } else {
    const dir = await os.uninstall();
    if (!dir) return console.log('Pindot is not installed.');
    console.log(`Removed Pindot from ${dir}.`);
  }
  console.log(`Your settings, models and Vault are still in ${os.dataFolder()}. Delete that folder to remove them too.`);
}

function ownVersion() {
  const pkg = JSON.parse(readFileSync(fileURLToPath(new URL('../package.json', import.meta.url)), 'utf8'));
  return pkg.version;
}

export async function main(argv) {
  let opts;
  try {
    opts = parseArgs(argv);
  } catch (e) {
    console.error(`pindot: ${e.message}`);
    return 2;
  }
  try {
    if (opts.command === 'help') {
      console.log(HELP);
    } else if (opts.command === 'version') {
      const release = loadRelease();
      console.log(`pindot installer ${ownVersion()}, installs Pindot ${release.version}`);
    } else if (opts.command === 'uninstall') {
      await uninstall(opts);
    } else {
      await install(opts, loadRelease());
    }
    return 0;
  } catch (e) {
    console.error(`pindot: ${e.message}`);
    return 1;
  }
}
