import { test } from 'node:test';
import assert from 'node:assert/strict';
import { parseArgs, compareVersions } from '../lib/cli.js';

test('no arguments means install (or update) and open', () => {
  assert.deepEqual(parseArgs([]), { command: 'install', dir: null, open: true, reinstall: false });
});

test('install takes a folder, --no-open and --reinstall', () => {
  assert.deepEqual(parseArgs(['install', '--dir', 'D:\\Apps\\Pindot', '--no-open', '--reinstall']), {
    command: 'install',
    dir: 'D:\\Apps\\Pindot',
    open: false,
    reinstall: true,
  });
  assert.equal(parseArgs(['--dir=/Users/me/Applications']).dir, '/Users/me/Applications');
});

test('uninstall, version and help are commands', () => {
  assert.equal(parseArgs(['uninstall']).command, 'uninstall');
  assert.equal(parseArgs(['--version']).command, 'version');
  assert.equal(parseArgs(['-v']).command, 'version');
  assert.equal(parseArgs(['--help']).command, 'help');
  assert.equal(parseArgs(['-h']).command, 'help');
});

test('an unknown word or flag is an error, not a guess', () => {
  assert.throws(() => parseArgs(['instal']), /unknown command "instal"/);
  assert.throws(() => parseArgs(['--force']), /unknown option "--force"/);
  assert.throws(() => parseArgs(['--dir']), /--dir needs a folder/);
});

test('versions compare by number, not as text', () => {
  assert.equal(compareVersions('0.0.1', '0.0.1'), 0);
  assert.equal(compareVersions('0.0.2', '0.0.10'), -1);
  assert.equal(compareVersions('1.0.0', '0.9.9'), 1);
  assert.equal(compareVersions('0.1', '0.1.0'), 0);
});
