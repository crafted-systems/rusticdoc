#!/usr/bin/env bash
# Renders the golden corpus with one image into out/<arch>/: Office templates
# made in Word, Excel and PowerPoint, the site's sample invoice, conversions of
# plain Office files, a filled PDF form and PDF/A. compare.py then checks two
# architectures against each other.
#   test/golden/run.sh [image]   (default rusticdoc:local)
set -euo pipefail
cd "$(dirname "$0")"
image=${1:-rusticdoc:local}
arch=$(docker image inspect -f '{{.Architecture}}' "$image")
out=out/$arch; rm -rf "$out"; mkdir -p "$out"
office=../office fx=../../site/samples/fixtures site=../../site/public/samples
name=rusticdoc-golden-$arch
docker rm -f "$name" >/dev/null 2>&1 || true
docker run -d --name "$name" --platform "linux/$arch" -e TZ=UTC -p 127.0.0.1::9027 "$image" >/dev/null
trap 'docker rm -f "$name" >/dev/null' EXIT
url=http://127.0.0.1:$(docker port "$name" 9027 | head -1 | cut -d: -f2)
for _ in $(seq 1 180); do curl -sf "$url/ready" >/dev/null && break; sleep 1; done
render() { local o=$1; shift; curl -sf "$@" "$url/v1/render" -o "$out/$o"; }
convert() { local o=$1; shift; curl -sf "$@" "$url/v1/convert" -o "$out/$o"; }
for t in "$office"/templates/*; do
  b=$(basename "${t%.*}"); e=${t##*.}
  render "$b.$e" -F "template=@$t" -F "data=<$office/data.json"
  render "$b.pdf" -F "template=@$t" -F "data=<$office/data.json" -F steps=convert
done
render site-invoice.docx -F "template=@$site/invoice-template.docx" -F "data=<$site/invoice.json"
render site-invoice.pdf -F "template=@$site/invoice-template.docx" -F "data=<$site/invoice.json" -F steps=convert
render site-invoice-pdfa.pdf -F "template=@$site/invoice-template.docx" -F "data=<$site/invoice.json" -F steps=pdfa
render letter.txt -F "template=@$fx/letter.txt" -F "data=<$fx/letter.json"
render form-filled.pdf -F "template=@$fx/form.pdf" -F 'data={"name":"Amani Otieno","date":"06/11/2026","agree":true}'
convert report.pdf -F "files=@$fx/report.docx"
convert deck.pdf -F "files=@$fx/deck.pptx"
convert sheet.pdf -F "files=@$fx/sheet.xlsx"
ls "$out" | wc -l | xargs echo "files in $out:"
