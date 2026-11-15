# Build Pindot for Windows: the release app and its installer.
#
#   scripts\build-windows.ps1              pindot.exe and an installer (NSIS, per user, no admin)
#   scripts\build-windows.ps1 -NoBundle    pindot.exe only (target\release\pindot.exe)
#
# ONNX Runtime is linked as a DLL and shipped next to pindot.exe. The static
# library the `ort` crate would download only links with a recent Visual
# Studio 2022 toolset; the DLL links with any. If ORT_LIB_LOCATION is not
# set, Microsoft's release of the version `ort-sys` pins is downloaded once
# into target\ort-dist and checked against its SHA-256.
#
# Not signed: Windows SmartScreen warns about an unsigned installer. See
# docs\release.md for signing.
param([switch]$NoBundle)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot

# The version ort-sys 2.0.0-rc.13 pins (crate file build\download\dist.tsv).
$ortVersion = '1.28.0'
$ortSha256 = 'abef733dacbe2f571547a7150b479b5cb9cc0df22f96c24983a42cadb1b4f8bc'
$ortUrl = "https://github.com/microsoft/onnxruntime/releases/download/v$ortVersion/onnxruntime-win-x64-$ortVersion.zip"

if (-not $env:ORT_LIB_LOCATION) {
    $dist = Join-Path $repo 'target\ort-dist'
    $lib = Join-Path $dist "x\onnxruntime-win-x64-$ortVersion\lib"
    if (-not (Test-Path (Join-Path $lib 'onnxruntime.lib'))) {
        $zip = Join-Path $dist 'dl\ort.zip'
        New-Item -ItemType Directory -Force (Split-Path $zip), (Join-Path $dist 'x') | Out-Null
        Write-Host "Downloading ONNX Runtime $ortVersion ..."
        Invoke-WebRequest -Uri $ortUrl -OutFile $zip -UseBasicParsing
        $got = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLower()
        if ($got -ne $ortSha256) {
            Remove-Item -LiteralPath $zip
            throw "ONNX Runtime download has SHA-256 $got, expected $ortSha256. Nothing was unpacked."
        }
        Expand-Archive -LiteralPath $zip -DestinationPath (Join-Path $dist 'x') -Force
    }
    $env:ORT_LIB_LOCATION = $lib
}
$env:ORT_PREFER_DYNAMIC_LINK = '1'
$dll = Join-Path $env:ORT_LIB_LOCATION 'onnxruntime.dll'
if (-not (Test-Path $dll)) { throw "No onnxruntime.dll in ORT_LIB_LOCATION ($env:ORT_LIB_LOCATION)." }
Write-Host "ONNX Runtime: $env:ORT_LIB_LOCATION (linked as a DLL)"

Push-Location (Join-Path $repo 'app')
try {
    if (-not (Test-Path 'node_modules')) { npm ci; if ($LASTEXITCODE) { throw 'npm ci failed' } }
    if ($NoBundle) {
        npm run build; if ($LASTEXITCODE) { throw 'the frontend build failed' }
        cargo build --release -p pindot; if ($LASTEXITCODE) { throw 'cargo build failed' }
    }
    else {
        # Ship the DLL next to pindot.exe. Given on the command line, not in
        # tauri.conf.json, so a plain `cargo build` needs no DLL to exist.
        $override = @{
            bundle = @{
                targets   = @('nsis')
                resources = @{ ($dll -replace '\\', '/') = 'onnxruntime.dll' }
            }
        } | ConvertTo-Json -Depth 5 -Compress
        $overrideFile = Join-Path ([IO.Path]::GetTempPath()) "pindot-bundle-$PID.json"
        # No byte-order mark: the Tauri CLI reads this as plain JSON.
        [IO.File]::WriteAllText($overrideFile, $override, (New-Object Text.UTF8Encoding($false)))
        try {
            npx tauri build --bundles nsis --config $overrideFile
            if ($LASTEXITCODE) { throw 'tauri build failed' }
        }
        finally { Remove-Item -LiteralPath $overrideFile -ErrorAction SilentlyContinue }
    }
}
finally { Pop-Location }

$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
$exe = Join-Path $target 'release\pindot.exe'
Write-Host "Built: $exe"
if (-not $NoBundle) {
    Get-ChildItem (Join-Path $target 'release\bundle\nsis') -Filter '*-setup.exe' |
        ForEach-Object { Write-Host ("Installer: {0} ({1:N1} MB)" -f $_.FullName, ($_.Length / 1MB)) }
}
