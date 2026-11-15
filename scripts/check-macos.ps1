# Type-check pindot-core and the app for macOS from Windows.
#
# The mirror of scripts/check-windows.sh. It proves only that the Rust code
# compiles for aarch64-apple-darwin, the macOS-only files included. The C
# code (ring) needs Apple's tools, so stand-ins that write empty files take
# their place: nothing built here can be linked or run. Build and test on a
# Mac. The app's check needs the frontend built once (app\dist).
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$target = 'aarch64-apple-darwin'
rustup target add $target | Out-Null

$tools = Join-Path $repo 'target\macos-check-tools'
New-Item -ItemType Directory -Force $tools | Out-Null
$ascii = New-Object Text.ASCIIEncoding
# Stand-in C compiler: writes an empty object file (the name after -o).
[IO.File]::WriteAllText((Join-Path $tools 'cc.cmd'), @'
@echo off
setlocal
set "out="
:loop
if "%~1"=="" goto done
if "%~1"=="-o" set "out=%~2"
shift
goto loop
:done
if defined out type nul > "%out%"
exit /b 0
'@, $ascii)
# Stand-in archiver: writes an empty archive (the first name ending in .a).
[IO.File]::WriteAllText((Join-Path $tools 'ar.cmd'), @'
@echo off
setlocal
set "out="
:loop
if "%~1"=="" goto done
if not defined out if /i "%~x1"==".a" set "out=%~1"
shift
goto loop
:done
if defined out type nul > "%out%"
exit /b 0
'@, $ascii)

$env:CC_aarch64_apple_darwin = Join-Path $tools 'cc.cmd'
$env:CXX_aarch64_apple_darwin = Join-Path $tools 'cc.cmd'
$env:AR_aarch64_apple_darwin = Join-Path $tools 'ar.cmd'
# The Windows build links ONNX Runtime as a DLL; that says nothing for a Mac.
Remove-Item Env:ORT_LIB_LOCATION, Env:ORT_PREFER_DYNAMIC_LINK -ErrorAction SilentlyContinue

Push-Location $repo
try {
    cargo check -p pindot-core --all-targets --target $target
    if ($LASTEXITCODE) { throw 'pindot-core does not compile for macOS' }
    cargo check -p pindot --all-targets --target $target
    if ($LASTEXITCODE) { throw 'the app does not compile for macOS' }
    cargo check -p pindot --all-targets --features test-switches --target $target
    if ($LASTEXITCODE) { throw 'the app with the test switches does not compile for macOS' }
}
finally { Pop-Location }
Write-Host 'macOS type-check passed (Rust only; not linked, not run).'
