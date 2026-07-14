#!/usr/bin/env bash
# build-whisper-cli.sh — build a STATIC, RELOCATABLE whisper-cli for the Tauri
# externalBin (Plan 05-04). Homebrew's whisper-cli is NOT relocatable: it links
# @rpath/libwhisper.dylib + /opt/homebrew/opt/ggml/lib/*.dylib, so it can't be bundled.
# This builds whisper.cpp from source with static libs + an EMBEDDED Metal library, so
# the result depends ONLY on system frameworks (Accelerate / Metal / libSystem / libc++)
# — a ~3 MB arm64 binary that runs from any bundle without dylib paths.
#
# Verified recipe (whisper.cpp @ ggml 0.13.1 / commit 99613cb; produced a 3.2 MB binary
# with `otool -L` showing 0 @rpath / 0 Cellar dylibs):
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="$REPO/app/src-tauri/binaries/whisper-cli-aarch64-apple-darwin"  # Tauri externalBin name (host triple)
WORK="${WHISPER_BUILD_DIR:-$(mktemp -d)/whisper.cpp}"

# PINNED to the exact recipe commit (99613cb — ggml 0.13.1) instead of a moving
# HEAD, so a rebuild is byte-reproducible against the verified recipe above. We
# shallow-fetch that one commit by SHA (GitHub allows want-by-SHA), then check it
# out — keeping the shallow-clone speed while nailing the source revision.
WHISPER_COMMIT="99613cb720b65036237d44b52f753b51f75c2797"
echo "==> fetching whisper.cpp @ $WHISPER_COMMIT (shallow, pinned) -> $WORK"
rm -rf "$WORK"
mkdir -p "$WORK"
git -C "$WORK" init -q
git -C "$WORK" remote add origin https://github.com/ggml-org/whisper.cpp
git -C "$WORK" fetch --depth 1 -q origin "$WHISPER_COMMIT"
git -C "$WORK" checkout -q FETCH_HEAD

echo "==> configuring (static libs + embedded Metal library)"
cmake -S "$WORK" -B "$WORK/build" \
  -DBUILD_SHARED_LIBS=OFF \
  -DGGML_METAL=ON \
  -DGGML_METAL_EMBED_LIBRARY=ON \
  -DWHISPER_BUILD_EXAMPLES=ON \
  -DWHISPER_BUILD_TESTS=OFF \
  -DWHISPER_BUILD_SERVER=OFF \
  -DCMAKE_BUILD_TYPE=Release

echo "==> building whisper-cli"
cmake --build "$WORK/build" --config Release -j --target whisper-cli

mkdir -p "$(dirname "$DEST")"
cp "$WORK/build/bin/whisper-cli" "$DEST"
chmod 755 "$DEST"

echo "==> installed externalBin: $DEST ($(du -h "$DEST" | cut -f1))"
echo "==> dylib check (want ONLY /System + /usr/lib, NO @rpath / NO Cellar):"
otool -L "$DEST" | sed -n '2,$p'
if otool -L "$DEST" | grep -qE 'homebrew|Cellar|@rpath'; then
  echo "ERROR: binary is NOT relocatable (found @rpath/Cellar deps above)" >&2
  exit 1
fi
echo "==> OK: relocatable static binary."
