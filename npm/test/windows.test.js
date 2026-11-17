import { test } from 'node:test';
import assert from 'node:assert/strict';
import { commandLine, installLocation, parseRegValue, parseTasklist } from '../lib/windows.js';

test('the installer runs silently, with /D= last and unquoted (NSIS rule)', () => {
  assert.equal(commandLine('C:\\Temp\\Pindot_0.0.1_x64-setup.exe', null), '"C:\\Temp\\Pindot_0.0.1_x64-setup.exe" /S');
  assert.equal(
    commandLine('C:\\Users\\A B\\Temp\\setup.exe', 'D:\\My Apps\\Pindot'),
    '"C:\\Users\\A B\\Temp\\setup.exe" /S /D=D:\\My Apps\\Pindot',
  );
});

test('a folder with a quote in it is refused rather than mangled', () => {
  assert.throws(() => commandLine('C:\\setup.exe', 'D:\\a"b'), /quote/);
});

test('reg query output gives the value, spaces in it included', () => {
  const out = [
    '',
    'HKEY_CURRENT_USER\\Software\\pindot',
    '    (Default)    REG_SZ    C:\\Users\\A B\\AppData\\Local\\Pindot',
    '',
  ].join('\r\n');
  assert.equal(parseRegValue(out, '(Default)'), 'C:\\Users\\A B\\AppData\\Local\\Pindot');
  const ver = '\r\nHKEY_CURRENT_USER\\...\\Pindot\r\n    DisplayVersion    REG_SZ    0.0.1\r\n';
  assert.equal(parseRegValue(ver, 'DisplayVersion'), '0.0.1');
  assert.equal(parseRegValue(ver, 'Publisher'), null);
});

test('the install folder comes from the Uninstall entry, without its quotes', () => {
  // As the Tauri installer writes it (seen on Windows 11).
  const out = [
    '',
    'HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Pindot',
    '    InstallLocation    REG_SZ    "C:\\Users\\A B\\AppData\\Local\\Pindot"',
    '',
  ].join('\r\n');
  assert.equal(installLocation(out), 'C:\\Users\\A B\\AppData\\Local\\Pindot');
  assert.equal(installLocation('\r\n    InstallLocation    REG_SZ    D:\\Pindot\r\n'), 'D:\\Pindot');
  assert.equal(installLocation(''), null);
});

test('tasklist output tells whether pindot.exe runs', () => {
  assert.equal(parseTasklist('"pindot.exe","1234","Console","1","120,000 K"\r\n'), true);
  assert.equal(parseTasklist('INFO: No tasks are running which match the specified criteria.\r\n'), false);
  assert.equal(parseTasklist('"pindot-test-target.exe","1","Console","1","1 K"\r\n'), false);
});
