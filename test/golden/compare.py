#!/usr/bin/env python3
"""Compares the golden corpus rendered on two architectures.

Each pair of files is reported as one of:
  identical   the same bytes
  same-normalized  the same once creation dates, document ids and XMP
                   timestamps are blanked, which differ on every run anyway
  same-pixels the pages rasterise to the same pixels at 100 dpi (needs
              pdftoppm and Pillow)
  same-layout the same pages, words, images and drawings, with no word box
              more than 0.25 pt from its place (needs PyMuPDF). LibreOffice's
              floating-point layout can move text by hundredths of a point
              between processors, which shows only in anti-aliasing
  DIFFERENT   none of the above
Exits 1 when any pair is DIFFERENT.
  test/golden/compare.py out/arm64 out/amd64
"""
import re
import subprocess
import sys
import tempfile
from pathlib import Path

VOLATILE = [
    rb"/(CreationDate|ModDate)\s*\([^)]*\)",
    rb"/ID\s*\[\s*<[0-9A-Fa-f]+>\s*<[0-9A-Fa-f]+>\s*\]",
    rb"<(xmp:CreateDate|xmp:ModifyDate|xmp:MetadataDate)>[^<]*</\1>",
    rb"(xmpMM:DocumentID|xmpMM:InstanceID)[=>\"][^<\"]*",
    rb"uuid:[0-9a-fA-F-]{36}",
]


def normalize(data: bytes) -> bytes:
    for pattern in VOLATILE:
        data = re.sub(pattern, b"", data)
    return data


def pixels(a: Path, b: Path) -> bool:
    from PIL import Image, ImageChops

    with tempfile.TemporaryDirectory() as tmp:
        pages = {}
        for side, path in (("a", a), ("b", b)):
            subprocess.run(["pdftoppm", "-r", "100", "-png", str(path), f"{tmp}/{side}"], check=True)
            pages[side] = sorted(Path(tmp).glob(f"{side}-*.png"))
        if len(pages["a"]) != len(pages["b"]) or not pages["a"]:
            return False
        for pa, pb in zip(pages["a"], pages["b"]):
            ia, ib = Image.open(pa).convert("RGB"), Image.open(pb).convert("RGB")
            if ia.size != ib.size or ImageChops.difference(ia, ib).getbbox():
                return False
    return True


TOLERANCE_PT = 0.25


def layout(a: Path, b: Path) -> bool:
    import pymupdf

    da, db = pymupdf.open(a), pymupdf.open(b)
    if len(da) != len(db):
        return False
    for pa, pb in zip(da, db):
        wa, wb = pa.get_text("words"), pb.get_text("words")
        if [w[4] for w in wa] != [w[4] for w in wb]:
            return False
        for x, y in zip(wa, wb):
            if max(abs(p - q) for p, q in zip(x[:4], y[:4])) > TOLERANCE_PT:
                return False
        if len(pa.get_images()) != len(pb.get_images()):
            return False
        if len(pa.get_drawings()) != len(pb.get_drawings()):
            return False
    return True


def main() -> int:
    left, right = Path(sys.argv[1]), Path(sys.argv[2])
    names = sorted({p.name for p in left.iterdir()} | {p.name for p in right.iterdir()})
    failed = 0
    for name in names:
        a, b = left / name, right / name
        if not a.exists() or not b.exists():
            verdict = "DIFFERENT (missing on one side)"
        else:
            da, db = a.read_bytes(), b.read_bytes()
            if da == db:
                verdict = "identical"
            elif normalize(da) == normalize(db):
                verdict = "same-normalized"
            elif name.endswith(".pdf") and pixels(a, b):
                verdict = "same-pixels"
            elif name.endswith(".pdf") and layout(a, b):
                verdict = "same-layout"
            else:
                verdict = "DIFFERENT"
        failed += verdict.startswith("DIFFERENT")
        print(f"{verdict:18} {name}")
    print(f"{len(names) - failed} of {len(names)} match")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
