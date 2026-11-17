// Which ready-made build fits this computer, and where it is downloaded from.
// release.json (written by scripts/release-npm.mjs) pins each file's size and
// SHA-256, so the npm package vouches for the bytes, not only GitHub.

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const LABELS = {
  'win32-x64': 'Windows',
  'darwin-arm64': 'a Mac with Apple silicon',
  'darwin-x64': 'a Mac with an Intel processor',
};

export function loadRelease() {
  const path = fileURLToPath(new URL('../release.json', import.meta.url));
  return JSON.parse(readFileSync(path, 'utf8'));
}

/** The release.json key for this OS and processor. Windows on Arm runs the x64 build. */
export function platformKey(platform, arch) {
  if (platform === 'win32') return 'win32-x64';
  if (platform === 'darwin') return arch === 'arm64' ? 'darwin-arm64' : 'darwin-x64';
  return null;
}

export function platformLabel(key) {
  return LABELS[key] ?? key;
}

export function pickAsset(release, platform, arch) {
  const key = platformKey(platform, arch);
  if (!key) throw new Error('Pindot runs on macOS and Windows only.');
  const asset = release.assets?.[key];
  if (!asset) {
    throw new Error(
      `There is no Pindot ${release.version} for ${platformLabel(key)} yet. ` +
        `See https://github.com/${release.repo}/releases for what there is.`,
    );
  }
  if (typeof asset.file !== 'string' || !/^[0-9a-f]{64}$/.test(asset.sha256 ?? '') || !(asset.size > 0)) {
    throw new Error('This copy of the pindot package is damaged (release.json). Try again with npx pindot@latest.');
  }
  return { ...asset, key };
}

export function assetUrl(release, asset, base = null) {
  const name = encodeURIComponent(asset.file);
  if (base) return `${base.replace(/\/+$/, '')}/${name}`;
  return `https://github.com/${release.repo}/releases/download/${encodeURIComponent(release.tag)}/${name}`;
}
