#!/bin/sh
# Type-check deskhand-core and the app for Windows from macOS.
#
# This proves only that the Rust code compiles for x86_64-pc-windows-msvc.
# The C code (ring) and the icon resource need Windows headers and tools, so
# stand-ins that write empty files take their place: nothing built here can be
# linked or run. Build and test on Windows (docs/windows-test-checklist.md).
set -eu
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
TARGET=x86_64-pc-windows-msvc
rustup target add "$TARGET" >/dev/null

# A fixed folder, so build scripts that watch CC/AR/RC don't rerun each time.
tools="$PWD/target/windows-check-tools"
mkdir -p "$tools"

cat > "$tools/cc" <<'EOF'
#!/bin/sh
# Stand-in C compiler: writes an empty object file.
out=""; prev=""
for a in "$@"; do
  case "$a" in -Fo?*|/Fo?*) out="${a#???}";; esac
  [ "$prev" = "-o" ] && out="$a"
  prev="$a"
done
[ -n "$out" ] && : > "$out"
exit 0
EOF

cat > "$tools/ar" <<'EOF'
#!/bin/sh
# Stand-in archiver: writes an empty archive.
out=""
for a in "$@"; do
  case "$a" in
    -out:*|-OUT:*|/out:*|/OUT:*) out="${a#*:}";;
    *.lib|*.a) [ -z "$out" ] && out="$a";;
  esac
done
[ -n "$out" ] && : > "$out"
exit 0
EOF

cat > "$tools/llvm-rc" <<'EOF'
#!/bin/sh
# Stand-in resource compiler: answers embed-resource's probe, writes an empty .res.
case "$*" in *"/?"*) echo "OVERVIEW: LLVM Resource Converter"; exit 0;; esac
out=""; prev=""
for a in "$@"; do
  case "$a" in /fo?*|/FO?*) out="${a#???}";; esac
  case "$prev" in /fo|/FO) out="$a";; esac
  prev="$a"
done
[ -n "$out" ] && : > "$out"
exit 0
EOF
chmod +x "$tools/cc" "$tools/ar" "$tools/llvm-rc"

# ort-sys links its DLLs next to the build output; off Windows it expects
# these folders to exist already (old and new cargo layouts).
for d in debug debug/build; do
  mkdir -p "target/$TARGET/$d/examples" "target/$TARGET/$d/deps"
done

export CC_x86_64_pc_windows_msvc="$tools/cc"
export CXX_x86_64_pc_windows_msvc="$tools/cc"
export AR_x86_64_pc_windows_msvc="$tools/ar"
export RC_x86_64_pc_windows_msvc="$tools/llvm-rc"

cargo check -p deskhand-core --all-targets --target "$TARGET"
cargo check -p deskhand --all-targets --target "$TARGET"
cargo check -p deskhand --all-targets --features test-switches --target "$TARGET"
echo "Windows type-check passed (Rust only; not linked, not run)."
