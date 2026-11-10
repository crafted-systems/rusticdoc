#!/usr/bin/env python3
"""Checks the rendered ports against what the upstream projects expect.

XLSX: the cell values that xlsx-template's own tests assert (test/crud-test.ts).
PPTX: the paragraphs of docxtemplater's expected-*.pptx files, read from the
upstream directory, and the text its tests assert.

    check.py <upstream dir> <rendered dir>

Exits 1 if a check fails that is not listed under KNOWN.
"""
import re
import sys
import zipfile
import xml.etree.ElementTree as ET
from pathlib import Path

M = "{http://schemas.openxmlformats.org/spreadsheetml/2006/main}"
A = "{http://schemas.openxmlformats.org/drawingml/2006/main}"

# file: {sheet number: {cell: value}}. A str must be a text cell and an int a
# number cell. "dimension" is the sheet's used range.
XLSX = {
    # "Array indexing"
    "xlsx-array.xlsx": {1: {"A2": "First row", "B2": "B"}},
    # "Arrays with single element"
    "xlsx-nested.xlsx": {1: {"A1": 123}},
    # "Each sheet should take each name"
    "xlsx-sheets.xlsx": {
        1: {"A1": "page: 1", "A2": "Page 1"},
        2: {"A1": "page: 2", "A2": "Page 2"},
    },
    # "can substitute values and generate a file"
    "xlsx-plan.xlsx": {
        1: {
            "dimension": "B2:F9",
            "B4": "Extracted on 41276",
            "C4": 10,
            "D6": 41275, "E6": 41276, "F6": 41277,
            "B7": "John Smith", "B8": "James Smith", "B9": "Jim Smith",
            "C7": "Developer", "C8": "Analyst", "C9": "Manager",
            "D7": 8, "D8": 4, "D9": 4,
            "E7": 8, "E8": 4, "E9": 4,
            "F7": 4, "F8": 4, "F9": 4,
        }
    },
}

# file: (expected file upstream, text the upstream test asserts or None)
PPTX = {
    "pptx-loop.pptx": ("docxtemplater/expected-loop-example.pptx", " Doe  John "),
    "pptx-table.pptx": ("docxtemplater/expected-loop-table.pptx", None),
    "pptx-paragraphs.pptx": ("docxtemplater/expected-paragraph-loop.pptx", None),
}

# Differences that are understood and written up in README.md.
KNOWN = {
    ("pptx-loop.pptx", "paragraphs"): "a loop across paragraphs repeats whole paragraphs",
}


def sheet_cells(z, number):
    strings = []
    if "xl/sharedStrings.xml" in z.namelist():
        root = ET.fromstring(z.read("xl/sharedStrings.xml"))
        strings = ["".join(t.text or "" for t in si.iter(M + "t")) for si in root.findall(M + "si")]
    root = ET.fromstring(z.read(f"xl/worksheets/sheet{number}.xml"))
    cells = {"dimension": root.find(M + "dimension").attrib["ref"]}
    for c in root.iter(M + "c"):
        kind, v, inline = c.attrib.get("t", "n"), c.find(M + "v"), c.find(M + "is")
        if kind == "s":
            cells[c.attrib["r"]] = strings[int(v.text)]
        elif kind == "inlineStr":
            cells[c.attrib["r"]] = "".join(t.text or "" for t in inline.iter(M + "t"))
        elif kind == "str":
            cells[c.attrib["r"]] = v.text or ""
        elif kind == "n" and v is not None:
            cells[c.attrib["r"]] = float(v.text) if "." in v.text else int(v.text)
    return cells


def paragraphs(path):
    out = []
    with zipfile.ZipFile(path) as z:
        slides = sorted(
            (n for n in z.namelist() if re.fullmatch(r"ppt/slides/slide\d+\.xml", n)),
            key=lambda n: int(re.search(r"\d+", n.rsplit("/", 1)[1]).group()),
        )
        for name in slides:
            root = ET.fromstring(z.read(name))
            out.append(["".join(t.text or "" for t in p.iter(A + "t")) for p in root.iter(A + "p")])
    return out


def main():
    upstream, rendered = Path(sys.argv[1]), Path(sys.argv[2])
    results = []  # (file, check, ok, detail)

    for name, sheets in XLSX.items():
        with zipfile.ZipFile(rendered / name) as z:
            for number, want in sheets.items():
                got = sheet_cells(z, number)
                for ref, value in want.items():
                    have = got.get(ref)
                    ok = have == value and type(have) is type(value)
                    results.append((name, f"sheet {number} {ref}", ok, f"want {value!r}, got {have!r}"))

    for name, (expected, text) in PPTX.items():
        got, want = paragraphs(rendered / name), paragraphs(upstream / expected)
        results.append((name, "paragraphs", got == want, f"want {want!r}, got {got!r}"))
        if text is not None:
            have = "".join(p for slide in got for p in slide)
            results.append((name, "text", have == text, f"want {text!r}, got {have!r}"))

    failed = known = 0
    for name in list(XLSX) + list(PPTX):
        mine = [r for r in results if r[0] == name]
        bad = [r for r in mine if not r[2]]
        print(f"{name}: {len(mine) - len(bad)} of {len(mine)} checks match")
        for _, check, _, detail in bad:
            reason = KNOWN.get((name, check))
            if reason:
                known += 1
                print(f"  differs, known ({reason}): {check}: {detail}")
            else:
                failed += 1
                print(f"  FAILED {check}: {detail}")
    passed = sum(1 for r in results if r[2])
    print(f"{passed} of {len(results)} checks match, {known} known difference(s), {failed} failure(s)")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
