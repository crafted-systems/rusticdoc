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
rm -rf profile
ls -la invoice.docx invoice.xlsx
