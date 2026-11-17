# Release builds

Status (15 Nov 2026): the macOS arm64 release build is made and checked
unsigned (see `docs/phase3/README.md`). The Windows build and its installer
are made by `scripts\build-windows.ps1`; the installer was installed
silently into a test folder, the installed copy run, and uninstalled; its
pages and the uninstaller's were walked by sending their buttons' commands,
not by hand. Signing and notarization are documented
here only; neither has been set up.

## macOS: unsigned build (done)

```sh
cd app
npm ci
npx tauri build            # runs `npm run build` first (beforeBuildCommand)
```

Output (Apple silicon):

- `target/release/bundle/macos/Pindot.app`
- `target/release/bundle/dmg/Pindot_<version>_aarch64.dmg`

`npx tauri build` never enables the `test-switches` feature, so a release
build has no test-only switches (they print a warning and are ignored).
`cargo tauri` is not needed; the npm package `@tauri-apps/cli` is the CLI.

The app is ad-hoc signed by the linker only. Gatekeeper blocks the first
open of a downloaded copy ("Open Anyway" in System Settings > Privacy &
Security). macOS also ties privacy grants (Accessibility, Input Monitoring,
Screen Recording, Microphone) to the code signature: with an unsigned or
ad-hoc build, every rebuild counts as a new app and the grants have to be
given again (remove the old Pindot entry first).

For an Intel or universal build: `rustup target add x86_64-apple-darwin`,
then `npx tauri build --target universal-apple-darwin`. Not tried.

## macOS: signing (documented, not set up)

1. Join the Apple Developer Program and create a **Developer ID
   Application** certificate (Xcode > Settings > Accounts > Manage
   Certificates, or developer.apple.com). Install it in the login keychain.
   `security find-identity -v -p codesigning` shows its name, e.g.
   `Developer ID Application: Tony Herrera (TEAMID)`.
2. Add an entitlements file, `app/src-tauri/Entitlements.plist`, for the
   hardened runtime. Pindot needs only the microphone:

   ```xml
   <?xml version="1.0" encoding="UTF-8"?>
   <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
   <plist version="1.0">
   <dict>
     <key>com.apple.security.device.audio-input</key>
     <true/>
   </dict>
   </plist>
   ```

   Do not turn on the App Sandbox: event taps (the hold-to-talk key and
   Esc), the accessibility API on other apps and synthetic input do not
   work inside it. No Apple Events entitlement is needed (Pindot sends
   none).
3. In `app/src-tauri/tauri.conf.json`, under `bundle.macOS`:

   ```json
   "signingIdentity": "Developer ID Application: Tony Herrera (TEAMID)",
   "hardenedRuntime": true,
   "entitlements": "Entitlements.plist"
   ```

   Or leave the file alone and set `APPLE_SIGNING_IDENTITY` in the
   environment. On a CI machine without the keychain, also set
   `APPLE_CERTIFICATE` (the .p12 exported and base64-encoded) and
   `APPLE_CERTIFICATE_PASSWORD`.
4. `scripts/build-mac.sh` (it finds the identity in the keychain and sets
   `APPLE_SIGNING_IDENTITY`; steps 2 and 3 are done in the repo, without
   the identity), or `npx tauri build` with the variable set, then check:

   ```sh
   codesign --verify --deep --strict --verbose=2 target/release/bundle/macos/Pindot.app
   codesign -d --entitlements - target/release/bundle/macos/Pindot.app
   ```

## macOS: notarization (documented, not set up)

Tauri notarizes and staples during `tauri build` when these are set:

- With an App Store Connect API key (preferred for CI): `APPLE_API_ISSUER`,
  `APPLE_API_KEY` (the key id) and `APPLE_API_KEY_PATH` (the .p8 file).
- Or with an Apple ID: `APPLE_ID`, `APPLE_PASSWORD` (an app-specific
  password from account.apple.com) and `APPLE_TEAM_ID`.

Then check:

```sh
xcrun stapler validate target/release/bundle/macos/Pindot.app
spctl -a -vvv -t exec target/release/bundle/macos/Pindot.app
spctl -a -vvv -t open --context context:primary-signature target/release/bundle/dmg/Pindot_*_aarch64.dmg
```

If notarization fails, `xcrun notarytool log <submission-id> --keychain-profile <profile>`
(or the same with the API key options) shows why. Typical causes: a
binary without the hardened runtime, or an unsigned nested binary.

After the first signed release, users must grant the four permissions once
more (the signature changed), and from then on the grants survive updates
signed with the same certificate.

## Windows (built on 15 Nov 2026; the installer was run silently only)

1. On Windows 11: Rust (MSVC toolchain, `x86_64-pc-windows-msvc`), the
   Visual Studio Build Tools with "Desktop development with C++" (2019 is
   enough, see below), Node.js 22, and WebView2 (built into Windows 11).
2. Build:

   ```powershell
   scripts\build-windows.ps1              # pindot.exe and the installer
   scripts\build-windows.ps1 -NoBundle    # pindot.exe only
   ```

   Output: `target\release\pindot.exe` and
   `target\release\bundle\nsis\Pindot_<version>_x64-setup.exe` (10 MB; it
   installs for the current user, no administrator rights).
3. **ONNX Runtime ships as a DLL** next to `pindot.exe`. The static library
   the `ort` crate downloads only links with a recent Visual Studio 2022
   toolset; the DLL links with any. The script sets `ORT_LIB_LOCATION` and
   `ORT_PREFER_DYNAMIC_LINK=1`, downloading Microsoft's release of the
   version `ort-sys` pins into `target\ort-dist` once and checking its
   SHA-256 (set `ORT_LIB_LOCATION` yourself to use another copy). The DLL
   is given to the bundler with `--config` on the command line, not in
   `tauri.conf.json`: a resource listed there has to exist for every
   `cargo build`, on every OS. When `ort-sys` is upgraded, change the
   version and the hash in the script.
4. What was checked: the installer holds `pindot.exe` and `onnxruntime.dll`
   side by side (7-Zip) and the DLL's SHA-256 is the official one. Its
   `pindot.exe` is the built one except for three bytes, the bundle type
   Tauri writes into it (`NSS` for `UNK`). A silent install
   (`Pindot_<version>_x64-setup.exe /S /D=<folder>`) took 2 s and left the
   app's files and `uninstall.exe`, a Start-menu and a desktop shortcut to
   the installed `pindot.exe`, and "Pindot 0.0.1" under Installed apps.
   The installed copy started, its server answered (401 without the
   token), a second start of it ended at once, and it quit cleanly. It
   loaded `onnxruntime.dll` and Microsoft's C++ runtime (`msvcp140.dll`,
   `msvcp140_1.dll`, `vcruntime140.dll`, `vcruntime140_1.dll`, 14.51 here)
   from its own folder. The installer carries those four because
   `onnxruntime.dll` needs them and a fresh PC may not have them; the
   script takes the newest set on the build PC and wants 14.40 or later. A
   silent uninstall (`uninstall.exe /S`) removed the files, both shortcuts
   and the Installed-apps entry. It keeps the install folder's path under
   `HKCU\Software\pindot` for the next install. By its script, the
   uninstaller's "delete the application data" box removes only the
   WebView2 data under the bundle id (`%LOCALAPPDATA%\dev.pindot.app`),
   not `%APPDATA%\Pindot` with the settings, the token and the models.
   The pages were walked too, into a test folder, by sending each
   button's command to the installer's own window: Welcome, Choose
   Install Location, Installing, Installation Complete, and the last page
   with "Run Pindot" unticked; then the uninstaller's: the confirmation
   with "Delete the application data" left unticked, Uninstalling, and
   Uninstallation Complete. Both ended with nothing left behind. Not
   checked: a person doing it with the mouse, that box ticked, the
   "Create desktop shortcut" box of the last page (a silent install makes
   the shortcut), a clean PC without WebView2. An MSI (`--bundles msi`, needs WiX) was
   not tried.
5. WebView2: the default `bundle.windows.webviewInstallMode` downloads the
   bootstrapper during install if WebView2 is missing (needs the internet);
   `offlineInstaller` or `fixedRuntime` avoid that at the cost of size.
6. Signing (not set up): with a code-signing certificate in the Windows
   certificate store, set under `bundle.windows`:

   ```json
   "certificateThumbprint": "<SHA1 thumbprint>",
   "digestAlgorithm": "sha256",
   "timestampUrl": "http://timestamp.digicert.com"
   ```

   For Azure Trusted Signing or a hardware token, use `signCommand`
   instead (a command with `%1` for the file to sign). Unsigned installers
   show a SmartScreen warning ("More info" > "Run anyway").
7. Check: `Get-AuthenticodeSignature .\Pindot_*_x64-setup.exe`, then
   install on a clean Windows 11 machine and run the checklist.

## The npm installer (`npx pindot`)

`npm/` is the `pindot` package on npm: a script with no dependencies and no
copy of the app. `npx pindot` takes the installer for this computer from the
GitHub release `v<version>`, checks its size while it downloads and its
SHA-256 at the end against `npm/release.json`, and then installs it:
- **Windows:** `setup.exe /S`, or `/D=<folder>` with `--dir`. The folder is
  read back from `HKCU\Software\pindot`.
- **macOS:** `hdiutil attach` the .dmg read-only, `ditto` `Pindot.app` into
  `/Applications` (or `~/Applications`), and detach. A failed copy puts the
  old app back.

The pins live in the package, so a file changed on GitHub after publishing
is refused. Running it again updates. If the installed version is the same
or newer, it only opens Pindot (`--reinstall` installs anyway).

It refuses to update while Pindot is running. `npx pindot uninstall` runs
`uninstall.exe /S` on Windows or moves the app to the Trash on macOS; the
data folder stays.

There is no `postinstall` script: `npm install -g pindot` only adds the
`pindot` command, and the user runs it.

Releasing (no CI; each platform is built on its own machine):

1. Build: `scripts\build-windows.ps1` on the PC, `scripts/build-mac.sh` on
   the Mac (signed and notarized, see above, or Gatekeeper and lost
   permission grants get in the way).
2. Pin each installer:
   `node scripts/release-npm.mjs add <path to Pindot_<v>_x64-setup.exe>` on
   the PC and the same with the `.dmg` on the Mac. Each `add` keeps the
   other platform's pin; a new version starts over. Commit
   `npm/release.json`.
3. Upload the same files to a GitHub release tagged `v<version>`
   (`gh release create v<version> --draft ...`, then
   `gh release upload v<version> <file>`). Publish it when both are there:
   a draft's files cannot be downloaded without signing in.
4. `node scripts/release-npm.mjs check` downloads each pinned file from the
   published release and compares.
5. In `npm/`: `npm test`, then `npm publish` (the maintainer's own
   `npm login`; no token in any file). The package version is the
   installer's own; `release.json` names the app version, so a fix to the
   script alone needs only a new package version.

Tests: `cd npm && npm test` (Node's own runner). The macOS steps are
tested with a stand-in for `hdiutil` and `ditto`, not on a Mac.

## Release checklist

1. `cargo test --workspace`, and in `app/`: `npm run check`, `npm test`.
2. Bump the version in `app/src-tauri/tauri.conf.json`, the root
   `Cargo.toml` (`[workspace.package]`) and `app/package.json`.
3. `npx tauri build` (signed and notarized once set up).
4. Launch check on a clean user account: the app starts with a menu-bar
   icon and the Setup window, `curl` of `http://127.0.0.1:47823/health`
   with the token answers, and Quit leaves no `pindot` or child `claude`
   process.
5. Publish the .dmg with its SHA-256 (`shasum -a 256`).
