#!/usr/bin/env bash
# Cross-checks the PDF engines against the real binaries, which this machine
# may not have: qpdf and pdfcpu from the rusticdoc image, and
# pdfinfo, pdftotext and veraPDF from gotenberg/integration-tools.
#
# The host builds the fixtures and runs the lopdf operations (through the
# `pdfops_fixtures` example); the containers only run the binaries. Every
# qpdf flag and pdfcpu description string the engines emit is exercised
# here in the exact form the engines use.
#
#   crates/core/tests/pdfops-container.sh [work-dir]
set -euo pipefail

WORK=${1:-$(mktemp -d)}
mkdir -p "$WORK"
WORK=$(cd "$WORK" && pwd)
ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
ENGINES=${ENGINES_IMAGE:-ghcr.io/crafted-systems/rusticdoc:latest}
TOOLS=${TOOLS_IMAGE:-gotenberg/integration-tools:latest}
PLATFORM=${DOCKER_PLATFORM:-linux/arm64}

run() { # image entrypoint args...
  local image=$1 bin=$2
  shift 2
  docker run --rm --platform "$PLATFORM" -v "$WORK:/work" -w /work -e HOME=/tmp \
    --entrypoint "$bin" "$image" "$@"
}
qpdf() { run "$ENGINES" /usr/bin/qpdf --warning-exit-0 "$@"; }
pdfcpu() { run "$ENGINES" /usr/bin/pdfcpu -c disable "$@"; }
pdfinfo() { run "$TOOLS" /usr/bin/pdfinfo "$@"; }
pdftotext() { run "$TOOLS" /usr/bin/pdftotext "$@"; }
verapdf() { run "$TOOLS" /usr/bin/verapdf "$@"; }
lopdf() { (cd "$ROOT" && cargo run -q -p rusticdoc-core --example pdfops_fixtures -- "$@"); }
pages() { pdfinfo "$1" | awk '/^Pages:/ {print $2}'; }
expect() { # description actual expected
  if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got '$2', want '$3'"; exit 1; fi
}

echo "work dir: $WORK"
lopdf write "$WORK"
printf 'Hello rusticdoc\nSecond line\n' >"$WORK/hello.txt"

echo "== qpdf"
qpdf --empty --pages three-pages.pdf outlined.pdf -- q-merge.pdf
expect "qpdf merge page count" "$(pages q-merge.pdf)" 6
qpdf --empty --pages three-pages.pdf 1-2 -- q-pages.pdf
expect "qpdf --pages range" "$(pages q-pages.pdf)" 2
qpdf --empty --pages three-pages.pdf 1-2,3 -- q-unify.pdf
expect "qpdf --pages unified" "$(pages q-unify.pdf)" 3
rm -f "$WORK"/q-part-*.pdf
qpdf --split-pages=2 three-pages.pdf q-part-%d.pdf
expect "qpdf --split-pages parts" "$(ls "$WORK" | grep -c '^q-part-')" 2
qpdf --rotate=+90:1-2 three-pages.pdf q-rotate.pdf
expect "qpdf --rotate" "$(pdfinfo -f 1 -l 1 q-rotate.pdf | awk '/^Page.*rot:/ {print $NF}')" 90
qpdf --generate-appearances --flatten-annotations=all form.pdf q-flatten.pdf
expect "qpdf flatten keeps text" "$(pdftotext q-flatten.pdf - | grep -c Ada)" 1
qpdf --object-streams=generate --recompress-flate --compression-level=9 --linearize three-pages.pdf q-optimize.pdf
expect "qpdf optimize page count" "$(pages q-optimize.pdf)" 3
printf -- '--encrypt\n--user-password=user\n--owner-password=owner\n--bits=256\n--\n' >"$WORK/args"
chmod 600 "$WORK/args"
qpdf @args three-pages.pdf q-encrypt.pdf
expect "qpdf encrypt AES-256" "$(qpdf --password=user --show-encryption q-encrypt.pdf | awk '/^R = / {print $3}')" 6

echo "== pdfcpu"
pdfcpu stamp add --mode text -p 1-2 -- "DRAFT (v1)" \
  "pos:c, rot:45.0, scale:0.500 rel, op:0.30, font:Helvetica, points:24, fillcolor:#808080" three-pages.pdf pc-stamp.pdf
expect "pdfcpu text stamp keeps page text" "$(pdftotext pc-stamp.pdf - | grep -c 'Page 1$')" 1
pdfcpu watermark add --mode text -- "DRAFT" "pos:bc, rot:0.0, scale:1 abs, op:0.50, font:Helvetica, points:24, fillcolor:#ff0000" three-pages.pdf pc-wm.pdf
expect "pdfcpu text watermark" "$(pdftotext pc-wm.pdf - | grep -c DRAFT)" 3
pdfcpu stamp add --mode image -- dot.png "pos:tl, rot:0.0, scale:0.250 rel, op:1.00" three-pages.pdf pc-image.pdf
expect "pdfcpu image stamp" "$(pages pc-image.pdf)" 3
pdfcpu watermark add --mode pdf -- outlined.pdf "pos:c, rot:0.0, scale:0.500 rel, op:1.00" three-pages.pdf pc-pdf.pdf
expect "pdfcpu pdf watermark" "$(pdftotext pc-pdf.pdf - | grep -c 'Chapter 1')" 1
pdfcpu rotate -p 1 three-pages.pdf 90 pc-rotate.pdf
expect "pdfcpu rotate" "$(pdfinfo -f 1 -l 1 pc-rotate.pdf | awk '/^Page.*rot:/ {print $NF}')" 90
pdfcpu optimize three-pages.pdf pc-optimize.pdf
expect "pdfcpu optimize" "$(pages pc-optimize.pdf)" 3
rm -rf "$WORK/parts" && mkdir "$WORK/parts"
pdfcpu split -m span three-pages.pdf parts 2
expect "pdfcpu split span names" "$(ls "$WORK/parts" | tr '\n' ' ')" "three-pages_1-2.pdf three-pages_3.pdf "
pdfcpu trim -p 1-2 three-pages.pdf pc-trim.pdf
expect "pdfcpu trim" "$(pages pc-trim.pdf)" 2
pdfcpu merge --bookmark-mode preserve pc-merge.pdf three-pages.pdf outlined.pdf
expect "pdfcpu merge page count" "$(pages pc-merge.pdf)" 6
expect "pdfcpu merge keeps bookmarks" "$(pdfcpu bookmarks list pc-merge.pdf | grep -c Chapter)" 2
pdfcpu bookmarks export outlined.pdf bm.json
expect "pdfcpu bookmarks export JSON" "$(python3 -c "import json,sys; b=json.load(open(sys.argv[1]))['bookmarks']; print(b[0]['title'], b[0]['page'], b[0]['kids'][0]['page'])" "$WORK/bm.json")" "Chapter 1 1 2"
pdfcpu bookmarks import --replace three-pages.pdf bm.json pc-bm.pdf
expect "pdfcpu bookmarks import" "$(pdfcpu bookmarks list pc-bm.pdf | grep -c .)" 3

echo "== lopdf outputs checked with poppler and veraPDF"
lopdf merge "$WORK/l-merge.pdf" "$WORK/three-pages.pdf" "$WORK/outlined.pdf"
expect "lopdf merge page count" "$(pages l-merge.pdf)" 6
lopdf stamp "$WORK/three-pages.pdf" "$WORK/l-stamp.pdf" "DRAFT"
expect "lopdf stamp keeps text" "$(pdftotext l-stamp.pdf - | grep -c 'Page 2$')" 1
expect "lopdf stamp adds text" "$(pdftotext l-stamp.pdf - | grep -c DRAFT)" 3
expect "lopdf stamp resources resolve" "$(pdftotext l-stamp.pdf - 2>&1 >/dev/null | grep -c 'Syntax Error')" 0
run "$ENGINES" /usr/bin/soffice --headless \
  --convert-to 'pdf:writer_pdf_Export:{"SelectPdfVersion":{"type":"long","value":"2"}}' \
  --outdir /work/pdfa /work/hello.txt >/dev/null
expect "LibreOffice PDF/A-2b baseline" "$(verapdf -f 2b --format text pdfa/hello.pdf | awk '{print $1}')" PASS
lopdf metadata "$WORK/pdfa/hello.pdf" "$WORK/l-pdfa.pdf" \
  '{"Title":"Invoice 42","Author":"Zoë Example","Subject":"Validation","Keywords":"pdfa, test","Creator":"rusticdoc","ModDate":"2025-06-07T08:09:10Z"}'
expect "metadata-written PDF/A-2b still validates" "$(verapdf -f 2b --format text l-pdfa.pdf | awk '{print $1}')" PASS
expect "metadata-written title" "$(pdfinfo l-pdfa.pdf | awk -F': *' '/^Title:/ {print $2}')" "Invoice 42"
echo "all checks passed"
