#!/usr/bin/env bash
# Replays Gotenberg's own integration scenarios against a rusticdoc image and
# the official Gotenberg image, side by side. Chromium scenarios are skipped:
# rusticdoc has no browser. Usage: test/parity.sh [image] [out-dir] [extra args]
set -u
IMAGE=${1:-rusticdoc:local}
OUT=${2:-parity-results}
HERE=$(cd "$(dirname "$0")" && pwd)
UPSTREAM=${UPSTREAM_DIR:-$HERE/../.upstream}
PLATFORM=${PLATFORM:-linux/arm64}
ENV_ARGS=(--slim-env RUSTICDOC_WEBHOOK_ALLOW_PRIVATE=true --slim-env RUSTICDOC_DOWNLOAD_FROM_ALLOW_PRIVATE=true)

if [ ! -d "$UPSTREAM/test/integration/features" ]; then
    echo "cloning Gotenberg v8.39.0 for its scenario files"
    git clone -q --depth 1 --branch v8.39.0 https://github.com/gotenberg/gotenberg.git "$UPSTREAM"
fi

COMMON=(--features "$UPSTREAM/test/integration/features" --testdata "$UPSTREAM/test/integration/testdata"
    --out "$OUT" --platform "$PLATFORM" --office-only --slim-office "$IMAGE" --only '^(?!chromium_)' "${ENV_ARGS[@]}")
python3 "$HERE/parity/replay.py" "${COMMON[@]}" "${@:3}"
python3 "$HERE/parity/replay.py" "${COMMON[@]}" --workers 1 --retry-failed
python3 "$HERE/parity/summarize.py" "$OUT/results.jsonl"
