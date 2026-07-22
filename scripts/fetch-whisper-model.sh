#!/usr/bin/env bash
# fetch-whisper-model.sh — download the ggml-tiny.en whisper model into the Tauri
# resource dir at BUILD time (Plan 05-04, AC-5). The model is ~74 MB and is NEVER
# committed to git (it's gitignored). Run this once before `bun tauri build` (or wire
# it into the build step) so the bundle picks up models/ggml-tiny.en.bin as a resource.
#
# Idempotent: skips the download if the model already exists and is non-trivial.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODEL_DIR="$REPO/app/src-tauri/models"
MODEL="$MODEL_DIR/ggml-tiny.en.bin"
# PINNED to an IMMUTABLE Hugging Face commit revision (not the mutable `main` ref),
# so a rebuild always fetches the exact bytes this recipe was verified against.
# Revision = repo HEAD commit at pin time (2026-07-02).
HF_COMMIT="5359861c739e955e79d9a303bcbc70fb988958b1"
URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/$HF_COMMIT/ggml-tiny.en.bin"
# sha256 of the model content (= the HF LFS oid for ggml-tiny.en.bin @ that commit;
# size 77704715 bytes). The download is verified against this — a mismatch is a HARD
# failure (corrupt / tampered / wrong-file), never a silent accept.
MODEL_SHA256="921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f"

mkdir -p "$MODEL_DIR"

# Compute sha256 portably (macOS shasum / Linux sha256sum). Empty on neither present.
model_sha256() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" 2>/dev/null | awk '{print $1}'
  elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" 2>/dev/null | awk '{print $1}'
  fi
}

# Idempotent: skip only if present AND the sha256 matches (a truncated / stale file
# is re-fetched). If no sha tool is available, fall back to the size>1MB heuristic.
if [[ -f "$MODEL" ]]; then
  HAVE="$(model_sha256 "$MODEL")"
  if [[ -n "$HAVE" ]]; then
    if [[ "$HAVE" == "$MODEL_SHA256" ]]; then
      echo "==> ggml-tiny.en.bin already present + sha256 verified — skipping."
      exit 0
    fi
    echo "==> ggml-tiny.en.bin present but sha256 MISMATCH — re-downloading."
  elif [[ "$(stat -f%z "$MODEL" 2>/dev/null || stat -c%s "$MODEL")" -gt 1000000 ]]; then
    echo "==> ggml-tiny.en.bin present (no sha256 tool to verify) — skipping on size."
    exit 0
  fi
fi

# LOUD preflight (plan: never fail silently on a missing model). The model is absent here
# (the idempotent skip above would have exited otherwise). If we cannot reach the pinned
# Hugging Face host, `curl --fail` below would still exit non-zero, but its raw HTTP error is
# NOT actionable — the operator sees a cryptic "beforeBuildCommand failed" and loses hours.
# So probe connectivity FIRST and fail LOUD with the exact remediation: fetch online, or place
# the file / set AGENT_TEAMS_WHISPER_MODEL for an offline build. (The build itself no longer
# hard-requires the model — see tauri.conf.json AC-5 — so this loud failure is the operator's
# signal, not a build-correctness gate.)
if ! curl -L --fail --silent --show-error --max-time 20 -o /dev/null \
      -I "https://huggingface.co/ggerganov/whisper.cpp/resolve/$HF_COMMIT/ggml-tiny.en.bin" 2>/tmp/hf-preflight.err; then
  echo "ERROR: ggml-tiny.en.bin is MISSING and the pinned Hugging Face host is unreachable." >&2
  echo "       model path: $MODEL" >&2
  echo "       url:        $URL" >&2
  echo "       probe:      $(cat /tmp/hf-preflight.err 2>/dev/null)" >&2
  echo "       Fix (any one): (a) connect to the network and re-run; the build auto-fetches it;" >&2
  echo "                      (b) copy ggml-tiny.en.bin (sha256 $MODEL_SHA256) to $MODEL;" >&2
  echo "                      (c) set AGENT_TEAMS_WHISPER_MODEL=/abs/path/ggml-tiny.en.bin." >&2
  echo "       NOTE: the app now BUILDS without the model (AC-5) — only dictation needs it;" >&2
  echo "       typed replies still work. This failure is loud by design so the absence is never silent." >&2
  rm -f /tmp/hf-preflight.err
  exit 1
fi
rm -f /tmp/hf-preflight.err

echo "==> downloading ggml-tiny.en.bin (~74 MB, pinned rev $HF_COMMIT) -> $MODEL"
curl -L --fail --progress-bar -o "$MODEL" "$URL"

# Verify integrity. If no sha256 tool exists, warn (do not hard-fail — the URL is
# already pinned to an immutable commit, which is the primary guarantee).
GOT="$(model_sha256 "$MODEL")"
if [[ -z "$GOT" ]]; then
  echo "==> WARNING: no shasum/sha256sum available — could not verify sha256 (URL is pinned, proceeding)." >&2
elif [[ "$GOT" != "$MODEL_SHA256" ]]; then
  echo "ERROR: sha256 mismatch for $MODEL" >&2
  echo "       expected $MODEL_SHA256" >&2
  echo "       got      $GOT" >&2
  rm -f "$MODEL"
  exit 1
else
  echo "==> sha256 verified ($MODEL_SHA256)"
fi
echo "==> done: $(du -h "$MODEL" | cut -f1)"
