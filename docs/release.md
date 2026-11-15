# Release builds

Status (14 Nov 2026): the macOS arm64 release build is made and checked
unsigned (see `docs/phase3/README.md`). Signing, notarization and the
Windows build are documented here only; none of them has been set up or run.

## macOS: unsigned build (done)

```sh
cd app
npm ci
npx tauri build            # runs `npm run build` first (beforeBuildCommand)
```

Output (Apple silicon):

- `target/release/bundle/macos/Deskhand.app`
- `target/release/bundle/dmg/Deskhand_<version>_aarch64.dmg`

`npx tauri build` never enables the `test-switches` feature, so a release
build has no test-only switches (they print a warning and are ignored).
`cargo tauri` is not needed; the npm package `@tauri-apps/cli` is the CLI.

The app is ad-hoc signed by the linker only. Gatekeeper blocks the first
open of a downloaded copy ("Open Anyway" in System Settings > Privacy &
Security). macOS also ties privacy grants (Accessibility, Input Monitoring,
Screen Recording, Microphone) to the code signature: with an unsigned or
ad-hoc build, every rebuild counts as a new app and the grants have to be
given again (remove the old Deskhand entry first).

For an Intel or universal build: `rustup target add x86_64-apple-darwin`,
then `npx tauri build --target universal-apple-darwin`. Not tried.

## macOS: signing (documented, not set up)

1. Join the Apple Developer Program and create a **Developer ID
   Application** certificate (Xcode > Settings > Accounts > Manage
   Certificates, or developer.apple.com). Install it in the login keychain.
   `security find-identity -v -p codesigning` shows its name, e.g.
   `Developer ID Application: Tony Herrera (TEAMID)`.
2. Add an entitlements file, `app/src-tauri/Entitlements.plist`, for the
   hardened runtime. Deskhand needs only the microphone:

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
   work inside it. No Apple Events entitlement is needed (Deskhand sends
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
   codesign --verify --deep --strict --verbose=2 target/release/bundle/macos/Deskhand.app
   codesign -d --entitlements - target/release/bundle/macos/Deskhand.app
   ```

## macOS: notarization (documented, not set up)

Tauri notarizes and staples during `tauri build` when these are set:

- With an App Store Connect API key (preferred for CI): `APPLE_API_ISSUER`,
  `APPLE_API_KEY` (the key id) and `APPLE_API_KEY_PATH` (the .p8 file).
- Or with an Apple ID: `APPLE_ID`, `APPLE_PASSWORD` (an app-specific
  password from account.apple.com) and `APPLE_TEAM_ID`.

Then check:

```sh
xcrun stapler validate target/release/bundle/macos/Deskhand.app
spctl -a -vvv -t exec target/release/bundle/macos/Deskhand.app
spctl -a -vvv -t open --context context:primary-signature target/release/bundle/dmg/Deskhand_*_aarch64.dmg
```

If notarization fails, `xcrun notarytool log <submission-id> --keychain-profile <profile>`
(or the same with the API key options) shows why. Typical causes: a
binary without the hardened runtime, or an unsigned nested binary.

After the first signed release, users must grant the four permissions once
more (the signature changed), and from then on the grants survive updates
signed with the same certificate.

## Windows (documented, not run: no Windows machine here)

Nothing in this section has been tried. The Windows platform code is still
stubs (see `docs/windows-test-checklist.md`), so a Windows build today
starts, serves MCP and shows its windows, but cannot capture, click, type
or hear the hotkey.

1. On Windows 11: Rust (MSVC toolchain, `x86_64-pc-windows-msvc`), the
   Visual Studio 2022 Build Tools with "Desktop development with C++",
   Node.js 22, and WebView2 (built into Windows 11).
2. `bundle.targets` in `tauri.conf.json` is `["app", "dmg"]` (macOS only),
   so name the Windows bundles on the command line:

   ```powershell
   cd app
   npm ci
   npx tauri build --bundles nsis,msi
   ```

   Output: `target\release\bundle\nsis\Deskhand_<version>_x64-setup.exe`
   and `target\release\bundle\msi\Deskhand_<version>_x64_en-US.msi`.
   The MSI needs WiX, which the Tauri CLI downloads on first use.
3. WebView2: the default `bundle.windows.webviewInstallMode` downloads the
   bootstrapper during install if WebView2 is missing (needs the internet);
   `offlineInstaller` or `fixedRuntime` avoid that at the cost of size.
4. Signing (not set up): with a code-signing certificate in the Windows
   certificate store, set under `bundle.windows`:

   ```json
   "certificateThumbprint": "<SHA1 thumbprint>",
   "digestAlgorithm": "sha256",
   "timestampUrl": "http://timestamp.digicert.com"
   ```

   For Azure Trusted Signing or a hardware token, use `signCommand`
   instead (a command with `%1` for the file to sign). Unsigned installers
   show a SmartScreen warning ("More info" > "Run anyway").
5. Check: `Get-AuthenticodeSignature .\Deskhand_*_x64-setup.exe`, then
   install on a clean Windows 11 machine and run the checklist.

## Release checklist

1. `cargo test --workspace`, and in `app/`: `npm run check`, `npm test`.
2. Bump the version in `app/src-tauri/tauri.conf.json`, the root
   `Cargo.toml` (`[workspace.package]`) and `app/package.json`.
3. `npx tauri build` (signed and notarized once set up).
4. Launch check on a clean user account: the app starts with a menu-bar
   icon and the Setup window, `curl` of `http://127.0.0.1:47823/health`
   with the token answers, and Quit leaves no `deskhand` or child `claude`
   process.
5. Publish the .dmg with its SHA-256 (`shasum -a 256`).
