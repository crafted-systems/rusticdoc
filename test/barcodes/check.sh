#!/usr/bin/env bash
# Renders template.docx with every symbology, then decodes the codes with
# ZXing and ZBar. PYTHON names an interpreter that has zxing-cpp, pyzbar and
# Pillow; ZBar itself comes from a package such as Homebrew's zbar.
#   test/barcodes/check.sh [image]   (default rusticdoc:local)
set -euo pipefail
cd "$(dirname "$0")"
image=${1:-rusticdoc:local}
mkdir -p out
name=rusticdoc-barcodes
docker rm -f "$name" >/dev/null 2>&1 || true
docker run -d --name "$name" -p 127.0.0.1::9027 "$image" >/dev/null
trap 'docker rm -f "$name" >/dev/null' EXIT
url=http://127.0.0.1:$(docker port "$name" 9027 | head -1 | cut -d: -f2)
for _ in $(seq 1 120); do curl -sf "$url/ready" >/dev/null && break; sleep 1; done
curl -sf -F template=@template.docx -F "data=<data.json" "$url/v1/render" -o out/barcodes.docx
curl -sf -F template=@template.docx -F "data=<data.json" -F steps=convert "$url/v1/render" -o out/barcodes.pdf
DYLD_LIBRARY_PATH=${DYLD_LIBRARY_PATH:-/opt/homebrew/lib} "${PYTHON:-python3}" decode.py out/barcodes.docx out/barcodes.pdf
