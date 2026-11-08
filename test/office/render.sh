#!/usr/bin/env bash
# Renders the templates in templates/, which were made in Word, Excel and
# PowerPoint, with data.json: once in their own format and once as PDF. The
# results go to out/<arch>/ for opening in Office and for comparing builds.
#   test/office/render.sh [image]   (default rusticdoc:local)
set -euo pipefail
cd "$(dirname "$0")"
image=${1:-rusticdoc:local}
arch=$(docker image inspect -f '{{.Architecture}}' "$image")
out=out/$arch
mkdir -p "$out"
name=rusticdoc-office-$arch
docker rm -f "$name" >/dev/null 2>&1 || true
docker run -d --name "$name" --platform "linux/$arch" -p 127.0.0.1::9027 "$image" >/dev/null
trap 'docker rm -f "$name" >/dev/null' EXIT
port=$(docker port "$name" 9027 | head -1 | cut -d: -f2)
for _ in $(seq 1 120); do curl -sf "http://127.0.0.1:$port/ready" >/dev/null && break; sleep 1; done
for t in templates/*; do
  base=$(basename "${t%.*}"); ext=${t##*.}
  curl -sf -F "template=@$t" -F "data=<data.json" "http://127.0.0.1:$port/v1/render" -o "$out/$base.$ext"
  curl -sf -F "template=@$t" -F "data=<data.json" -F steps=convert "http://127.0.0.1:$port/v1/render" -o "$out/$base.pdf"
  echo "$base: $(wc -c <"$out/$base.$ext") bytes $ext, $(wc -c <"$out/$base.pdf") bytes pdf"
done
