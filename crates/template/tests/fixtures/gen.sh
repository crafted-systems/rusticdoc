#!/bin/sh
# Regenerates the LibreOffice-made fixtures from the flat ODF sources next
# to this script. Needs Docker and the office image.
set -e
cd "$(dirname "$0")"
IMAGE=${IMAGE:-ghcr.io/crafted-systems/rusticdoc:latest}
mkdir -p profile
soffice() {
  docker run --rm --platform linux/arm64 -v "$PWD:/w" -v "$PWD/profile:/tmp/p" \
    --entrypoint /usr/lib/libreoffice/program/soffice.bin "$IMAGE" \
    -env:UserInstallation=file:///tmp/p --headless "$@"
}
soffice --terminate_after_init || true
soffice --convert-to docx --outdir /w /w/invoice.fodt
soffice --convert-to xlsx --outdir /w /w/invoice.fods
for pair in docx:fodt odt:fodt xlsx:fods ods:fods pptx:fodp odp:fodp; do
  soffice --convert-to "${pair%:*}" --outdir /w "/w/rich.${pair#*:}"
  # Basic macros, links and a linked workbook, for the sanitiser.
  soffice --convert-to "${pair%:*}" --outdir /w "/w/sanitize.${pair#*:}"
  # Pictures whose alt text holds an image or barcode tag.
  soffice --convert-to "${pair%:*}" --outdir /w "/w/placeholder.${pair#*:}"
done
soffice --convert-to xlsx --outdir /w /w/sheets.fods
soffice --convert-to ods --outdir /w /w/sheets.fods
# A drop() tag for each element, on a condition the data sets.
for pair in docx:fodt odt:fodt xlsx:fods ods:fods pptx:fodp odp:fodp; do
  soffice --convert-to "${pair%:*}" --outdir /w "/w/drop.${pair#*:}"
done
python3 drop_fix.py drop.pptx drop.xlsx
cp drop.docx drop.xlsx drop.pptx ../../../../site/samples/fixtures/
rm -rf profile
ls -la invoice.docx invoice.xlsx rich.* sanitize.* sheets.xlsx sheets.ods drop.* placeholder.*
