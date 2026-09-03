#!/usr/bin/env bash
# Cross-compile VulIDE for Linux ARM64 (aarch64) from x86-64, static.
#
# Needs:  zig >= 0.14 on PATH,  cargo-zigbuild >= 0.23  (older versions choke on
#         the aarch64 --fix-cortex-a53-843419 linker arg),
#         rustup target add aarch64-unknown-linux-musl
#
# Ships a raw static binary in a .tar.gz rather than an AppImage: no FUSE
# dependency, which matters on minimal ARM systems (Pi, containers).
# Output: dist/VulIDE-<ver>-aarch64-linux.tar.gz
set -euo pipefail

cd "$(dirname "$0")/.."
TARGET=aarch64-unknown-linux-musl
DIST="$(pwd)/../dist"

rustup target add "$TARGET" >/dev/null 2>&1 || true
cargo zigbuild --release --target "$TARGET"

VER="0.1.0-g$(git rev-parse --short HEAD)"
STAGE="$(mktemp -d)/VulIDE"
mkdir -p "$STAGE"
install -m755 "target/$TARGET/release/vulide" "$STAGE/vulide"
strip "$STAGE/vulide" 2>/dev/null || \
  "$HOME/.local/bin/zig" objcopy --strip-all "$STAGE/vulide" "$STAGE/vulide" 2>/dev/null || true
cp packaging/README-linux-arm64.txt "$STAGE/README.txt"

mkdir -p "$DIST"
TGZ="$DIST/VulIDE-${VER}-aarch64-linux.tar.gz"
rm -f "$TGZ"
tar -C "$(dirname "$STAGE")" -czf "$TGZ" VulIDE
( cd "$DIST" && sha256sum "$(basename "$TGZ")" | tee -a SHA256SUMS )
echo "built $TGZ"
