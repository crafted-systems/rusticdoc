#!/usr/bin/env bash
# Rebuilds the invoice sample and the images the site shows of it, using a
# running service. Needs python-docx and pymupdf, and SERVICE set if the
# service is not on localhost:9027.
set -euo pipefail
cd "$(dirname "$0")/.."
SERVICE="${SERVICE:-http://localhost:9027}"
PY="${PYTHON:-python3}"
OUT=public/samples
"$PY" samples/make_invoice.py "$OUT/invoice-template.docx"
curl -sf -F template=@"$OUT/invoice-template.docx" -F "data=<samples/invoice.json" -F steps=convert \
  "$SERVICE/v1/render" -o "$OUT/invoice.pdf"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
curl -sf -F files=@"$OUT/invoice-template.docx" "$SERVICE/v1/convert" -o "$TMP/template.pdf"
"$PY" -W ignore - "$TMP/template.pdf" "$OUT/invoice.pdf" src/assets <<'P'
import sys, fitz
template, filled, out = sys.argv[1:]
def is_tag(text):
    return any(mark in text for mark in ("{{", "{%", "}}", "%}", "currency("))

for src, name in ((template, "invoice-template"), (filled, "invoice-filled")):
    page = fitz.open(src)[0]
    if name == "invoice-template":
        # Mark the tags the way a proofreader would: this wash is the site's
        # annotation, and is not in the template.
        for block in page.get_text("dict")["blocks"]:
            for line in block.get("lines", []):
                for span in line["spans"]:
                    if is_tag(span["text"]):
                        page.draw_rect(fitz.Rect(span["bbox"]), color=None, fill=(0.56, 0.80, 0.95), fill_opacity=0.45, overlay=True)
    clip = fitz.Rect(0, 0, page.rect.width, page.rect.width * 0.92)
    page.get_pixmap(dpi=200, clip=clip).save(f"{out}/{name}.png")
P
