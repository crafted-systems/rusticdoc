#!/usr/bin/env python3
"""Builds a rusticdoc invoice template as a .docx with python-docx.

    pip install python-docx
    python3 make_invoice_docx.py invoice-template.docx

The result renders with invoice.json, the same data as invoice.docx and
invoice.html. What it shows:

- Tags are ordinary text. Formatting a run formats what the tag prints.
- The line-item loop opens in the first cell of a table row and closes in its
  last cell, so that row repeats once per item.
- Block tags never sit alone in a paragraph, which would leave an empty
  paragraph behind in the output.
"""
import sys

from docx import Document
from docx.enum.table import WD_TABLE_ALIGNMENT
from docx.enum.text import WD_ALIGN_PARAGRAPH
from docx.shared import Pt, RGBColor

BLUE = RGBColor(0x2B, 0x59, 0xC3)


def right(cell, text, bold=False):
    paragraph = cell.paragraphs[0]
    paragraph.alignment = WD_ALIGN_PARAGRAPH.RIGHT
    paragraph.add_run(text).bold = bold


def main(out):
    doc = Document()
    doc.styles["Normal"].font.name = "Liberation Sans"
    doc.styles["Normal"].font.size = Pt(10.5)

    title = doc.add_paragraph()
    run = title.add_run("Invoice {{ number }}")
    run.bold = True
    run.font.size = Pt(20)
    run.font.color.rgb = BLUE

    doc.add_paragraph("{{ seller.name }}, {{ seller.address }}")
    doc.add_paragraph('Issued {{ issued | date("%d %B %Y") }}, due {{ due | date("%d %B %Y") }}')

    billed = doc.add_paragraph()
    billed.add_run("Billed to ").bold = True
    billed.add_run("{{ customer.name }}, {{ customer.address }}")

    table = doc.add_table(rows=3, cols=4)
    table.style = "Table Grid"
    table.alignment = WD_TABLE_ALIGNMENT.CENTER
    header, line, total = table.rows

    for cell, text in zip(header.cells, ["Item", "Qty", "Unit price", "Amount"]):
        cell.paragraphs[0].add_run(text).bold = True

    # The row loop: `{% for %}` in the first cell, `{% endfor %}` in the last.
    line.cells[0].paragraphs[0].add_run("{% for item in items %}{{ item.name }}")
    right(line.cells[1], "{{ item.qty }}")
    right(line.cells[2], "{{ item.price | currency(currency) }}")
    right(line.cells[3], "{{ (item.qty * item.price) | currency(currency) }}{% endfor %}")

    merged = total.cells[0].merge(total.cells[2])
    right(merged, "Total", bold=True)
    right(total.cells[3], "{{ total | currency(currency) }}", bold=True)

    # A condition and its content share one paragraph, so nothing is left
    # behind when it's false.
    doc.add_paragraph("{% if payment_url is defined %}Pay online at {{ payment_url }}{% endif %}")

    doc.save(out)
    print(f"wrote {out}")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "invoice-template.docx")
