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
EXPECTED = {v for _, _, v in json.loads((HERE / "expected.json").read_text())}


def zxing(img):
    return {r.text for r in zxingcpp.read_barcodes(img)}


def zbar(img):
    return {r.data.decode("utf-8") for r in pyzbar.decode(img)}


def report(label, found_by, informational=False):
    ok = True
    for name, found in found_by.items():
        missing = EXPECTED - found
        extra = found - EXPECTED
        status = "ok" if not missing and not extra else "FAIL"
        if informational and status == "FAIL":
            status = "info"
        ok &= status != "FAIL"
        detail = "" if status == "ok" else f" missing={sorted(missing)} unexpected={sorted(extra)}"
        print(f"{status:4} {label:28} {name:5} {len(found & EXPECTED)}/{len(EXPECTED)}{detail}")
    return ok


def main():
    docx, pdf = Path(sys.argv[1]), Path(sys.argv[2])
    ok = True
    pictures = [n for n in zipfile.ZipFile(docx).namelist() if n.startswith("word/media/")]
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
