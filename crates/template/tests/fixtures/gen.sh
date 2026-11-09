#!/bin/sh
# Regenerates the LibreOffice-made fixtures from the flat ODF sources next
# to this script. Needs Docker and the office image.
set -e
cd "$(dirname "$0")"
IMAGE=ghcr.io/crafted-systems/gotenberg-slim-office:latest
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
done
rm -rf profile
ls -la invoice.docx invoice.xlsx rich.* sanitize.*
