#!/usr/bin/env python3
"""Decodes the barcodes rusticdoc drew, with two independent decoders.

ZXing (zxing-cpp, the decoder behind most phone scanner apps) and ZBar read
each picture embedded in the DOCX, then every page of the PDF rasterised at
300, 150 and 100 dpi. A code passes when a decoder returns exactly the value
in expected.json, which holds EAN codes with the check digit the service adds.
100 dpi is reported but does not fail the check: at that resolution a long
Code 128 or Code 39 at the default 200 pt has bars under 1.5 pixels wide. Needs zxing-cpp, pyzbar (with the zbar library), Pillow and
pdftoppm.
  test/barcodes/decode.py out/barcodes.docx out/barcodes.pdf
"""
import io
import json
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

import zxingcpp
from PIL import Image
from pyzbar import pyzbar

HERE = Path(__file__).parent
CASES = json.loads((HERE / "expected.json").read_text())
EXPECTED = {v for _, _, v in CASES}
# ZBar takes the black square of the QR-bill's Swiss cross for part of the
# symbol and finds nothing; ZXing reads it. The cross is the one the Swiss
# Implementation Guidelines draw, so ZBar's miss is reported, not failed.
KNOWN = {"zbar": {v for kind, _, v in CASES if kind == "swissqr"}}


def zxing(img):
    # Each format reads the way phone apps do, light-on-dark included.
    return {r.text for r in zxingcpp.read_barcodes(img, try_invert=True)}


def zbar(img):
    return {r.data.decode("utf-8") for r in pyzbar.decode(img)}


def report(label, found_by, informational=False):
    ok = True
    for name, found in found_by.items():
        known = KNOWN.get(name, set())
        missing = EXPECTED - found - known
        extra = found - EXPECTED
        status = "ok" if not missing and not extra else "FAIL"
        if known - found and status == "ok":
            print(f"info {label:28} {name:5} known miss: {len(known - found)} Swiss QR-bill code")
        if informational and status == "FAIL":
            status = "info"
        ok &= status != "FAIL"
        detail = "" if status == "ok" else f" missing={sorted(missing)} unexpected={sorted(extra)}"
        print(f"{status:4} {label:28} {name:5} {len(found & EXPECTED)}/{len(EXPECTED)}{detail}")
    return ok


def main():
    docx, pdf = Path(sys.argv[1]), Path(sys.argv[2])
    ok = True
    # Barcodes are SVG with a PNG beside it; the PNG is the picture to read.
    pictures = [
        n
        for n in zipfile.ZipFile(docx).namelist()
        if n.startswith("word/media/") and n.endswith(".png")
    ]
    found = {"zxing": set(), "zbar": set()}
    for name in pictures:
        img = Image.open(io.BytesIO(zipfile.ZipFile(docx).read(name))).convert("RGB")
        # A quiet zone as on paper: the pictures are cropped to the symbol.
        framed = Image.new("RGB", (img.width + 80, img.height + 80), "white")
        framed.paste(img, (40, 40))
        found["zxing"] |= zxing(framed)
        found["zbar"] |= zbar(framed)
    ok &= report(f"DOCX pictures ({len(pictures)})", found)
    for dpi in (300, 150, 100):
        with tempfile.TemporaryDirectory() as tmp:
            subprocess.run(["pdftoppm", "-r", str(dpi), "-png", str(pdf), f"{tmp}/p"], check=True)
            found = {"zxing": set(), "zbar": set()}
            for page in sorted(Path(tmp).glob("p-*.png")):
                img = Image.open(page).convert("RGB")
                found["zxing"] |= zxing(img)
                found["zbar"] |= zbar(img)
            ok &= report(f"PDF at {dpi} dpi", found, informational=dpi < 150)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
