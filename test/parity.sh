#!/usr/bin/env bash
# Replays Gotenberg's own integration scenarios against a rusticdoc image and
# the official Gotenberg image, using the harness from the sibling
# gotenberg-slim project. Chromium scenarios are skipped: rusticdoc has no
# browser. Usage: test/parity.sh [image] [out-dir]
set -u
IMAGE=${1:-rusticdoc:local}
OUT=${2:-parity-results}
SLIM=${SLIM_DIR:-../gotenberg-slim}
UPSTREAM=${UPSTREAM_DIR:-$SLIM/.upstream}
PLATFORM=${PLATFORM:-linux/arm64}

if [ ! -d "$SLIM/test/parity" ]; then
    echo "gotenberg-slim's harness not found at $SLIM; set SLIM_DIR" >&2
    exit 2
fi
if [ ! -d "$UPSTREAM/test/integration/features" ]; then
    echo "cloning Gotenberg v8.39.0 for its scenario files"
    git clone -q --depth 1 --branch v8.39.0 https://github.com/gotenberg/gotenberg.git "$UPSTREAM"
fi

python3 "$SLIM/test/parity/replay.py" --features "$UPSTREAM/test/integration/features" \
    --testdata "$UPSTREAM/test/integration/testdata" --out "$OUT" --platform "$PLATFORM" \
    --office-only --slim-office "$IMAGE" --only '^(?!chromium_)' "${@:3}"
python3 "$SLIM/test/parity/replay.py" --features "$UPSTREAM/test/integration/features" \
    --testdata "$UPSTREAM/test/integration/testdata" --out "$OUT" --platform "$PLATFORM" \
    --office-only --slim-office "$IMAGE" --only '^(?!chromium_)' --workers 1 --retry-failed
python3 "$SLIM/test/parity/summarize.py" "$OUT/results.jsonl"
