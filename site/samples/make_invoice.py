#!/usr/bin/env python3
"""Writes the invoice template used across the site and the docs.

The template is authored here so it can be rebuilt, and everything the site
shows of it comes from running it through the real service. See
scripts/make-samples.sh.
"""
import sys

from docx import Document
from docx.enum.table import WD_TABLE_ALIGNMENT
from docx.enum.text import WD_ALIGN_PARAGRAPH
from docx.oxml import OxmlElement
from docx.oxml.ns import qn
from docx.shared import Cm, Pt, RGBColor

INK = RGBColor(0x14, 0x20, 0x33)
SOFT = RGBColor(0x55, 0x60, 0x70)
FONT = "Liberation Sans"


def run(par, text, size=10, bold=False, color=INK):
    r = par.add_run(text)
    r.font.name = FONT
    r._element.rPr.rFonts.set(qn("w:eastAsia"), FONT)
    r.font.size = Pt(size)
    r.font.bold = bold
    r.font.color.rgb = color
    return r


def para(container, text="", size=10, bold=False, color=INK, align=None, after=0):
    p = container.add_paragraph()
    p.paragraph_format.space_after = Pt(after)
    p.paragraph_format.space_before = Pt(0)
    if align:
        p.alignment = align
    if text:
        run(p, text, size, bold, color)
    return p


def cell_text(cell, text, size=10, bold=False, color=INK, align=None):
    p = cell.paragraphs[0]
    p.paragraph_format.space_after = Pt(0)
    if align:
        p.alignment = align
    run(p, text, size, bold, color)


def border(cell, **edges):
    props = cell._tc.get_or_add_tcPr()
    box = OxmlElement("w:tcBorders")
    for edge, size in edges.items():
        el = OxmlElement(f"w:{edge}")
        el.set(qn("w:val"), "single")
        el.set(qn("w:sz"), str(size))
        el.set(qn("w:color"), "142033")
        box.append(el)
    props.append(box)


def pad(cell, top=90, bottom=90):
    props = cell._tc.get_or_add_tcPr()
    mar = OxmlElement("w:tcMar")
    for side, val in (("top", top), ("bottom", bottom)):
        el = OxmlElement(f"w:{side}")
        el.set(qn("w:w"), str(val))
        el.set(qn("w:type"), "dxa")
        mar.append(el)
    props.append(mar)


def fix_widths(table, widths):
    """Sets the grid as well as the cells, which is what LibreOffice reads."""
    table.autofit = False
    for col, width in zip(table._tbl.tblGrid.findall(qn("w:gridCol")), widths):
        col.set(qn("w:w"), str(int(width.twips)))
    for row in table.rows:
        for i, width in enumerate(widths):
            row.cells[i].width = width


def build(path):
    doc = Document()
    sec = doc.sections[0]
    sec.page_width, sec.page_height = Cm(21), Cm(29.7)
    sec.left_margin = sec.right_margin = Cm(2.2)
    sec.top_margin = Cm(2.2)
    sec.bottom_margin = Cm(2)

    head = doc.add_table(rows=1, cols=2)
    head.alignment = WD_TABLE_ALIGNMENT.CENTER
    left, right = head.rows[0].cells
    cell_text(left, "{{ seller.name }}", 15, True)
    para(left, "{{ seller.address }}", 9, color=SOFT)
    cell_text(right, "Invoice {{ number }}", 15, True, align=WD_ALIGN_PARAGRAPH.RIGHT)
    para(right, "Issued {{ issued | date(\"%d %B %Y\") }}", 9, color=SOFT, align=WD_ALIGN_PARAGRAPH.RIGHT)

    para(doc, after=18)
    for text, size, bold, colour, after in (
        ("Billed to", 9, False, SOFT, 2),
        ("{{ customer.name }}", 11, True, INK, 0),
        ("{{ customer.address }}", 10, False, INK, 20),
    ):
        para(doc, text, size, bold, colour, after=after).paragraph_format.left_indent = Cm(0.19)

    rows = doc.add_table(rows=3, cols=4)
    rows.alignment = WD_TABLE_ALIGNMENT.CENTER
    widths = (Cm(6.2), Cm(1.4), Cm(4.3), Cm(4.7))
    right_edge = WD_ALIGN_PARAGRAPH.RIGHT
    for i, title in enumerate(("Item", "Qty", "Unit price", "Amount")):
        cell = rows.rows[0].cells[i]
        cell_text(cell, title, 9, True, align=right_edge if i else None)
        border(cell, bottom=8)
        pad(cell)
    body = (
        "{% for item in items %}{{ item.name }}",
        "{{ item.qty }}",
        '{{ item.price | currency(currency) }}',
        '{{ (item.qty * item.price) | currency(currency) }}{% endfor %}',
    )
    for i, tag in enumerate(body):
        cell = rows.rows[1].cells[i]
        cell_text(cell, tag, 10, align=right_edge if i else None)
        border(cell, bottom=2)
        pad(cell)
    total = rows.rows[2].cells
    merged = total[0].merge(total[2])
    cell_text(merged, "Total due", 11, True)
    cell_text(total[3], "{{ total | currency(currency) }}", 10, True, align=right_edge)
    for cell in (merged, total[3]):
        pad(cell, 140, 140)
    fix_widths(rows, widths)

    para(doc, after=24)
    pay = doc.add_table(rows=1, cols=2)
    a, b = pay.rows[0].cells
    fix_widths(pay, (Cm(7.6), Cm(9)))
    cell_text(a, "{{ payment_url | barcode(\"qr\", size=68) }}", 9)
    cell_text(b, "Pay by {{ due | date(\"%d %B %Y\") }}", 10, True)
    para(b, "Scan the code, or use reference {{ number }} with your bank.", 9, color=SOFT)
    doc.save(path)


if __name__ == "__main__":
    build(sys.argv[1])
