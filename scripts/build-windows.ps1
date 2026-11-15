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

# onnxruntime.dll needs Microsoft's C++ runtime (msvcp140.dll and three
# more), which a fresh Windows PC may not have: Pindot would not start
# there. So the installer puts them next to pindot.exe as well; Microsoft
# lets these DLLs be shipped with an app. The newest set on this PC is
# taken (Visual Studio's Redist folder, or the installed redistributable in
# System32). A set older than 14.40 would crash a library built with a
# recent Visual Studio, so it is not shipped and a warning says so.
$crtNames = 'msvcp140.dll', 'msvcp140_1.dll', 'vcruntime140.dll', 'vcruntime140_1.dll'
$crtDirs = @(Join-Path $env:WINDIR 'System32')
$crtDirs += @(Get-ChildItem "${env:ProgramFiles(x86)}\Microsoft Visual Studio\*\*\VC\Redist\MSVC\*\x64\Microsoft.VC*.CRT", `
        "$env:ProgramFiles\Microsoft Visual Studio\*\*\VC\Redist\MSVC\*\x64\Microsoft.VC*.CRT" -Directory -ErrorAction SilentlyContinue |
    ForEach-Object { $_.FullName })
$crtDir = $crtDirs |
    Where-Object { $d = $_; -not ($crtNames | Where-Object { -not (Test-Path (Join-Path $d $_)) }) } |
    Sort-Object { (Get-Item (Join-Path $_ 'msvcp140.dll')).VersionInfo.FileVersionRaw } -Descending |
    Select-Object -First 1
$crtVersion = if ($crtDir) { (Get-Item (Join-Path $crtDir 'msvcp140.dll')).VersionInfo.FileVersionRaw } else { $null }
if ($crtVersion -and $crtVersion -ge [version]'14.40') {
    Write-Host "C++ runtime: $crtVersion from $crtDir (shipped next to pindot.exe)"
    # A copy under the repo: the bundler loses the drive letter of a
    # resource that is on another drive than the project.
    $crtCopy = Join-Path $repo "target\crt-dist\$crtVersion"
    New-Item -ItemType Directory -Force $crtCopy | Out-Null
    foreach ($n in $crtNames) { Copy-Item -LiteralPath (Join-Path $crtDir $n) -Destination $crtCopy -Force }
    $crtDir = $crtCopy
}
else {
    Write-Warning "No C++ runtime of version 14.40 or later was found on this PC (best: $crtVersion). The installer will not carry it: Pindot then needs the Visual C++ Redistributable on the PC it is installed on."
    $crtDir = $null
}

Push-Location (Join-Path $repo 'app')
try {
    if (-not (Test-Path 'node_modules')) { npm ci; if ($LASTEXITCODE) { throw 'npm ci failed' } }
    if ($NoBundle) {
        npm run build; if ($LASTEXITCODE) { throw 'the frontend build failed' }
        cargo build --release -p pindot; if ($LASTEXITCODE) { throw 'cargo build failed' }
    }
    else {
        # Ship the DLLs next to pindot.exe. Given on the command line, not in
        # tauri.conf.json, so a plain `cargo build` needs no DLL to exist.
        $resources = @{ ($dll -replace '\\', '/') = 'onnxruntime.dll' }
        if ($crtDir) { foreach ($n in $crtNames) { $resources[((Join-Path $crtDir $n) -replace '\\', '/')] = $n } }
        $override = @{
            bundle = @{
                targets   = @('nsis')
                resources = $resources
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
