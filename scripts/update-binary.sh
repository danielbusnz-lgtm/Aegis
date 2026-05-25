#!/bin/bash
# Updates the launcher's bundled aegis binary with a fresh build
set -e

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BINARY_DIR="$REPO_ROOT/launcher/src-tauri/binaries"
ARCH="$(uname -m)"

# Map arch to Rust target triple
case "$ARCH" in
    arm64|aarch64) TARGET="aarch64-apple-darwin" ;;
    x86_64)        TARGET="x86_64-apple-darwin" ;;
    *)             echo "Unknown arch: $ARCH"; exit 1 ;;
esac

BINARY_NAME="aegis-$TARGET"

echo "Building aegis (release) for $TARGET..."
cd "$REPO_ROOT"
cargo build --release -p aegis --no-default-features --features "winit-window"

echo "Copying binary to launcher..."
cp "$REPO_ROOT/target/release/aegis" "$BINARY_DIR/$BINARY_NAME"
chmod +x "$BINARY_DIR/$BINARY_NAME"

echo "Done! Updated: $BINARY_DIR/$BINARY_NAME"
