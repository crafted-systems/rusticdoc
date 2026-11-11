#!/usr/bin/env bash
# Drives the real bridge (crates/core/bridge/unobridge.py) inside a container
# that has LibreOffice and python3-uno, and checks the outputs with verapdf
# and pdfinfo from gotenberg/integration-tools. Needs docker. The DOCX is the
# site's one-page report unless you pass another; the other samples (ODT,
# XLSX, PPTX, a password-protected ODT) are made by the bridge itself.
#
#   crates/core/tests/libreoffice-container.sh [path/to/sample.docx]
#
# Environment: OFFICE_IMAGE, TOOLS_IMAGE, PLATFORM, SOFFICE, WORK override the
# defaults below. Exit status is non-zero when any check fails.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
BRIDGE="$HERE/../bridge/unobridge.py"
DOCX=${1:-$HERE/../../../site/samples/fixtures/report.docx}
OFFICE_IMAGE=${OFFICE_IMAGE:-ghcr.io/crafted-systems/rusticdoc:latest}
TOOLS_IMAGE=${TOOLS_IMAGE:-gotenberg/integration-tools:latest}
PLATFORM=${PLATFORM:-linux/arm64}
SOFFICE=${SOFFICE:-/usr/lib/libreoffice/program/soffice.bin}
WORK=${WORK:-$(mktemp -d "${TMPDIR:-/tmp}/rusticdoc-lo.XXXXXX")}
mkdir -p "$WORK/out"
cp "$BRIDGE" "$WORK/unobridge.py"
cp "$DOCX" "$WORK/sample.docx"
echo "work dir: $WORK"

bridge() {  # stdin: requests, stdout: responses
  docker run --rm -i --platform "$PLATFORM" -v "$WORK:/w" --entrypoint python3 \
    "$OFFICE_IMAGE" /w/unobridge.py --soffice "$SOFFICE" --profile /tmp/profile --socket-path rd-test
}
tool() {  # tool args...: a command from the integration-tools image
  docker run --rm --platform "$PLATFORM" -v "$WORK/out:/o" --entrypoint "$1" "$TOOLS_IMAGE" "${@:2}"
}
req() {  # id input output filter filter_options password landscape
  printf '{"id": %s, "input": "/w/%s", "output": "/w/out/%s", "filter": "%s", "filter_options": %s, "password": %s, "landscape": %s}\n' "$@"
}
FAILED=0
expect() {  # id ok|fail responses-file
  local want=false
  [ "$2" = ok ] && want=true
  if grep -q "\"id\": $1, \"ok\": $want" "$3"; then
    echo "  ok   request $1 ($2)"
  else
    echo "  FAIL request $1 expected $2: $(grep "\"id\": $1," "$3" || echo 'no answer')"; FAILED=1
  fi
}
check() {  # description command...
  if "${@:2}" >/dev/null 2>&1; then echo "  ok   $1"; else echo "  FAIL $1"; FAILED=1; fi
}
pdfinfo_field() {  # file field: one pdfinfo line, e.g. "Pages: 2"
  tool pdfinfo "/o/$1" | grep "^$2:" | tr -s ' '
}

printf 'name,qty,price\nwidget,2,3.5\ngadget,10,1.25\n' > "$WORK/sheet.csv"
cat > "$WORK/deck.fodp" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0" office:version="1.2" office:mimetype="application/vnd.oasis.opendocument.presentation">
<office:body><office:presentation>
<draw:page draw:name="page1"><draw:frame svg:width="20cm" svg:height="3cm" svg:x="2cm" svg:y="2cm"><draw:text-box><text:p>Slide one</text:p></draw:text-box></draw:frame></draw:page>
<draw:page draw:name="page2"><draw:frame svg:width="20cm" svg:height="3cm" svg:x="2cm" svg:y="2cm"><draw:text-box><text:p>Slide two</text:p></draw:text-box></draw:frame></draw:page>
</office:presentation></office:body></office:document>
EOF
# A protected ODT (password "secret"), stored through the bridge's Office class.
cat > "$WORK/mkprotected.py" <<EOF
import sys; sys.path.insert(0, "/w")
import unobridge
office = unobridge.Office("$SOFFICE", "/tmp/profile-mk", "rd-mk")
doc = office.desktop.loadComponentFromURL("file:///w/sample.docx", "_blank", 0, unobridge.props({"Hidden": True}))
doc.storeToURL("file:///w/protected.odt", unobridge.props({"FilterName": "writer8", "Password": "secret"}))
doc.close(True); office.shutdown(); print("protected.odt written")
EOF
docker run --rm --platform "$PLATFORM" -v "$WORK:/w" --entrypoint python3 "$OFFICE_IMAGE" /w/mkprotected.py

echo "== batch 1: formats (DOCX->PDF/ODT/DOCX, CSV->XLSX, FODP->PPTX, bad input, impossible target)"
{
  req 1 sample.docx sample.pdf pdf '{}' null false
  req 2 sample.docx sample.odt odt '{}' null false
  req 3 sheet.csv sheet.xlsx xlsx '{}' null false
  req 4 deck.fodp deck.pptx pptx '{}' null false
  req 5 sample.docx sample.copy.docx docx '{}' null false
  req 6 missing.docx x.pdf pdf '{}' null false
  req 7 sample.docx x.xlsx xlsx '{}' null false
} | bridge | tee "$WORK/batch1.jsonl"
for i in 1 2 3 4 5; do expect $i ok "$WORK/batch1.jsonl"; done
expect 6 fail "$WORK/batch1.jsonl"
expect 7 fail "$WORK/batch1.jsonl"
check "ODT produced" test -s "$WORK/out/sample.odt"
check "XLSX produced" test -s "$WORK/out/sheet.xlsx"
check "PPTX produced" test -s "$WORK/out/deck.pptx"

echo "== batch 2: PDF options on one warm soffice, then 10 timed DOCX->PDF conversions"
PDFA='{"SelectPdfVersion": 2, "ExportFormFields": true, "ExportBookmarks": true, "Quality": 90, "MaxImageResolution": 300, "ReduceImageResolution": false, "UseLosslessCompression": false, "IsSkipEmptyPages": false, "IsAddStream": false, "ExportNotes": false, "PDFUACompliance": false}'
FLAGS='{"ExportNotes": true, "ExportNotesInMargin": true, "ExportPlaceholders": true, "AllowDuplicateFieldNames": true, "ExportBookmarksToPDFDestination": true, "ConvertOOoTargetToPDFTarget": true, "ExportLinksRelativeFsys": true, "ExportHiddenSlides": true, "ExportNotesPages": false, "ExportOnlyNotesPages": false, "IsSkipEmptyPages": true, "IsAddStream": true, "UseLosslessCompression": true, "ReduceImageResolution": true, "MaxImageResolution": 75, "Quality": 50}'
{
  req 1 sample.docx warm.pdf pdf '{}' null false
  req 2 sample.docx pdfa2b.pdf pdf "$PDFA" null false
  req 3 sample.docx pdfua.pdf pdf '{"PDFUACompliance": true}' null false
  req 4 deck.fodp range.pdf pdf '{"PageRange": "2"}' null false
  req 5 sample.docx landscape.pdf pdf '{}' null true
  req 6 out/sheet.xlsx sheet.pdf pdf '{"SinglePageSheets": true}' null false
  req 7 protected.odt wrongpw.pdf pdf '{}' '"nope"' false
  req 8 protected.odt nopw.pdf pdf '{}' null false
  req 9 protected.odt rightpw.pdf pdf '{}' '"secret"' false
  req 10 sample.docx flags.pdf pdf "$FLAGS" null false
  req 11 deck.fodp deck-landscape.pdf pdf '{}' null true
  req 12 deck.fodp deck.pdf pdf '{}' null false
  for i in $(seq 101 110); do req "$i" sample.docx timed.pdf pdf '{}' null false; done
} | bridge | tee "$WORK/batch2.jsonl"
for i in 1 2 3 4 5 6 9 10 11 12; do expect $i ok "$WORK/batch2.jsonl"; done
expect 7 fail "$WORK/batch2.jsonl"
expect 8 fail "$WORK/batch2.jsonl"

echo "== checks"
check "PDF/A-2b passes verapdf"      grep -q '^PASS' <(tool verapdf -f 2b --format text /o/pdfa2b.pdf)
check "plain PDF is not PDF/A"       grep -q '^FAIL' <(tool verapdf -f 2b --format text /o/warm.pdf)
check "PDF/UA output is tagged"      grep -q 'yes' <(pdfinfo_field pdfua.pdf Tagged)
check "page range kept 1 of 2 pages" grep -q 'Pages: 1' <(pdfinfo_field range.pdf Pages)
check "deck has 2 pages"             grep -q 'Pages: 2' <(pdfinfo_field deck.pdf Pages)
check "single page sheet"            grep -q 'Pages: 1' <(pdfinfo_field sheet.pdf Pages)
check "right password opened"        test -s "$WORK/out/rightpw.pdf"
check "embedded source stream"       grep -q 'EmbeddedFile' "$WORK/out/flags.pdf"
echo "  page sizes: portrait $(pdfinfo_field warm.pdf 'Page size') / landscape $(pdfinfo_field landscape.pdf 'Page size')"
check "landscape page is wider than tall" \
  awk '{exit !($3 > $5)}' <(pdfinfo_field landscape.pdf 'Page size')
check "portrait page is taller than wide" \
  awk '{exit !($3 < $5)}' <(pdfinfo_field warm.pdf 'Page size')
echo "verapdf ua1 on the PDF/UA output (informational; LibreOffice does not pass every rule):"
tool verapdf -f ua1 --format text /o/pdfua.pdf 2>/dev/null | grep -v '^WARNING' | head -5 || true

echo "== batch 3: soffice killed between two requests"
cat > "$WORK/killtest.py" <<EOF
import json, os, signal, stat, subprocess, sys, time
p = subprocess.Popen([sys.executable, "/w/unobridge.py", "--soffice", "$SOFFICE", "--profile", "/tmp/profile-kill",
                      "--socket-path", "rd-kill"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
ready = json.loads(p.stdout.readline())
pipe = "/tmp/OSL_PIPE_%d_rd-kill" % os.getuid()
mode = stat.S_IMODE(os.stat(pipe).st_mode)
print("pipe", pipe, oct(mode))
assert mode == 0o700, "the UNO pipe must be private to this user"
def ask(i):
    p.stdin.write(json.dumps({"id": i, "input": "/w/sample.docx", "output": "/w/out/kill%d.pdf" % i, "filter": "pdf",
                              "filter_options": {}, "password": None, "landscape": False}) + "\n"); p.stdin.flush()
    return json.loads(p.stdout.readline())
first = ask(1)
os.kill(ready["soffice_pid"], signal.SIGKILL); time.sleep(0.5)
second = ask(2)
code = p.wait(timeout=10)
print("first", first, "second", second, "exit", code)
sys.exit(0 if first["ok"] and second["error"] == "libreoffice gone" and code != 0 else 1)
EOF
check "dead soffice is reported and the bridge exits non-zero" \
  docker run --rm --platform "$PLATFORM" -v "$WORK:/w" --entrypoint python3 "$OFFICE_IMAGE" /w/killtest.py

echo "== timing: warm DOCX->PDF, bridge-side ms per conversion (requests 101-110)"
grep -E '"id": 1(0[1-9]|10),' "$WORK/batch2.jsonl" | sed -E 's/.*"ms": ([0-9]+).*/\1/' \
  | awk '{s+=$1; if (NR==1||$1<min) min=$1; if ($1>max) max=$1} END {printf "  n=%d avg=%.0f ms min=%d ms max=%d ms\n", NR, s/NR, min, max}'
echo "  first (cold) DOCX->PDF: $(grep '"id": 1,' "$WORK/batch2.jsonl" | sed -E 's/.*"ms": ([0-9]+).*/\1/') ms"

if [ "$FAILED" = 0 ]; then echo "ALL CHECKS PASSED"; else echo "SOME CHECKS FAILED"; exit 1; fi
