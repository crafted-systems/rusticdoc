#!/usr/bin/env python3
"""Ports the third-party templates to rusticdoc syntax.

Each port is a list of exact string replacements in named parts of the file.
Nothing else in the file changes, so the result is the upstream template with
its tags rewritten. A replacement that does not match the expected number of
times stops the run, because it means upstream changed.

    port.py <upstream dir> <output dir>
"""
import sys
import zipfile
from pathlib import Path

SST = "xl/sharedStrings.xml"
SHEET1 = "xl/worksheets/sheet1.xml"
SLIDE1 = "ppt/slides/slide1.xml"


def cell(ref, style, text):
    return f'<c r="{ref}" s="{style}" t="inlineStr"><is><t>{text}</t></is></c>'


# name: (source, [(part, old, new, times)], [parts to delete])
PORTS = {
    # ${data[0]} and ${data[1].name}: indexing reads the same way.
    "xlsx-array.xlsx": (
        "xlsx-template/test-array.xlsx",
        [
            (SST, "${data[0]}", "{{ data[0] }}", 1),
            (SST, "${data[1].name}", "{{ data[1].name }}", 1),
        ],
        [],
    ),
    # ${table:sales.payments} gives a row per sale and a column per payment.
    # Here that is a row loop with a cell per payment.
    "xlsx-nested.xlsx": (
        "xlsx-template/test-nested-arrays.xlsx",
        [
            (SST, "${table:sales.payments}", "{% for s in sales %}{{ s.payments[0] }}", 1),
            (
                SHEET1,
                '<c r="A1" s="6" t="s"><v>0</v></c>',
                '<c r="A1" s="6" t="s"><v>0</v></c>' + cell("B1", 6, "{% endfor %}"),
                1,
            ),
        ],
        [],
    ),
    # Upstream fills each of two identical sheets with its own data. Here one
    # sheet is repeated per item, so the second sheet is taken out.
    "xlsx-sheets.xlsx": (
        "xlsx-template/multple-sheets-arrays.xlsx",
        [
            (SST, "${page}", "{% sheet for s in sheets %}{{ s.page }}", 1),
            (SST, "Page ${sheetNumber}", "Page {{ s.sheetNumber }}", 2),
            (
                "xl/workbook.xml",
                '<sheet name="Sheet1" sheetId="1" r:id="rId1"/><sheet name="Sheet2" sheetId="2" r:id="rId2"/>',
                '<sheet name="Sheet{{ s.sheetNumber }}" sheetId="1" r:id="rId1"/>',
                1,
            ),
            ("xl/workbook.xml", ' activeTab="1"', "", 1),
            (
                "xl/_rels/workbook.xml.rels",
                '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/>',
                "",
                1,
            ),
            (
                "[Content_Types].xml",
                '<Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>',
                "",
                1,
            ),
        ],
        ["xl/worksheets/sheet2.xml"],
    ),
    # ${table:planData.name} becomes a row loop. Upstream also spreads a list
    # across columns (${dates}, ${table:planData.days}), which is a column loop
    # here. Upstream writes a date as Excel's serial number, which is the
    # excel_date filter.
    "xlsx-plan.xlsx": (
        "xlsx-template/t1.xlsx",
        [
            (
                SST,
                "Extracted on ${extractDate}",
                "Extracted on {{ extractDate | excel_date }}",
                1,
            ),
            (SST, "${revision}", "{{ revision }}", 1),
            (
                SST,
                "${dates}",
                "{% col for d in dates %}{{ d | excel_date }}{% endcol %}",
                1,
            ),
            (SST, "${table:planData.name}", "{% for p in planData %}{{ p.name }}", 1),
            (SST, "${table:planData.role}", "{{ p.role }}", 1),
            (
                SST,
                "${table:planData.days}",
                "{% col for d in p.days %}{{ d }}{% endcol %}{% endfor %}",
                1,
            ),
        ],
        [],
    ),
    # {#users} ... {/users} across two paragraphs.
    "pptx-loop.pptx": (
        "docxtemplater/loop-example.pptx",
        [
            (SLIDE1, "{#users} {name}", "{% for u in users %} {{ u.name }}", 1),
            (SLIDE1, "{/users}", "{% endfor %}", 1),
        ],
        [],
    ),
    # A table row loop: the tags sit in the row's first and last cells.
    "pptx-table.pptx": (
        "docxtemplater/loop-table.pptx",
        [
            (SLIDE1, "{#products}{name}", "{% for p in products %}{{ p.name }}", 1),
            (SLIDE1, "{price} € {/}", "{{ p.price }} € {% endfor %}", 1),
        ],
        [],
    ),
    # The loop tags sit in paragraphs of their own. Upstream removes those
    # paragraphs; here drop("paragraph") does.
    "pptx-paragraphs.pptx": (
        "docxtemplater/paragraph-loop.pptx",
        [
            (SLIDE1, "<a:t>{#users}</a:t>", '<a:t>{% for u in users %}{{ drop("paragraph") }}</a:t>', 1),
            (SLIDE1, "{age}  {name}", "{{ u.age }}  {{ u.name }}", 1),
            (SLIDE1, "<a:t>{/}</a:t>", '<a:t>{{ drop("paragraph") }}{% endfor %}</a:t>', 1),
        ],
        [],
    ),
}


def port(src, dst, edits, deletes):
    with zipfile.ZipFile(src) as zin:
        parts = {i.filename: zin.read(i.filename) for i in zin.infolist()}
        order = [i.filename for i in zin.infolist()]
    for part, old, new, times in edits:
        text = parts[part].decode("utf-8")
        found = text.count(old)
        if found != times:
            sys.exit(f"{src.name}: {old!r} is in {part} {found} times, expected {times}")
        parts[part] = text.replace(old, new).encode("utf-8")
    with zipfile.ZipFile(dst, "w", zipfile.ZIP_DEFLATED) as zout:
        for name in order:
            if name not in deletes:
                zout.writestr(zipfile.ZipInfo(name), parts[name], zipfile.ZIP_DEFLATED)


def main():
    upstream, out = Path(sys.argv[1]), Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    for name, (source, edits, deletes) in PORTS.items():
        port(upstream / source, out / name, edits, deletes)
        print(f"{name}: from {source}, {len(edits)} replacements")


if __name__ == "__main__":
    main()
