#!/usr/bin/env bash
# Phase 2's gate: ports third-party XLSX and PPTX templates to this syntax,
# fills them with the upstream data and checks the results against what the
# upstream projects expect. Rendering needs no engine, so the CLI is enough.
#   test/thirdparty/run.sh [upstream dir]   (default: fetched into upstream/)
#   RUSTICDOC=path/to/rusticdoc picks the binary (default target/debug/rusticdoc)
set -euo pipefail
cd "$(dirname "$0")"
python=${PYTHON:-python3}
bin=${RUSTICDOC:-../../target/debug/rusticdoc}
upstream=${1:-upstream}
[ -d "$upstream" ] || ./fetch.sh "$upstream"
rm -rf out && mkdir -p out/templates out/rendered
"$python" -I port.py "$upstream" out/templates
for t in out/templates/*; do
  name=$(basename "$t")
  "$bin" render "$t" --data "data/${name%.*}.json" -o "out/rendered/$name" >/dev/null
done
"$python" -I check.py "$upstream" out/rendered
