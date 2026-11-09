#!/usr/bin/env python3
"""Writes the small files the documentation examples run against.

Needs python-docx, python-pptx, openpyxl, pillow and pymupdf.
"""
import sys
from pathlib import Path

import fitz
from docx import Document
from docx.shared import Inches

from openpyxl import Workbook
from PIL import Image, ImageDraw
from pptx import Presentation

out = Path(sys.argv[1])
(out / "letter.txt").write_text(
    'Dear {{ customer.name }},\nTotal: {{ total | currency("KES") }}\n'
)
(out / "letter.json").write_text('{"customer":{"name":"Ada"},"total":1500}\n')

doc = Document()
doc.add_heading("Quarterly report", 1)
doc.add_paragraph("A document with no tags in it.")
doc.save(out / "report.docx")

deck = Presentation()
deck.slides.add_slide(deck.slide_layouts[0]).shapes.title.text = "A slide"
deck.save(out / "deck.pptx")

book = Workbook()
book.active.append(["Month", "Sales"])
book.active.append(["January", 120])
book.save(out / "sheet.xlsx")

for name, text in (("a.pdf", "Page A"), ("b.pdf", "Page B")):
    pdf = fitz.open()
    pdf.new_page().insert_text((72, 72), text, fontsize=24)
    pdf.save(out / name)

pdf = fitz.open()
page = pdf.new_page()
page.insert_text((72, 60), "Application form", fontsize=18)
for i, (field, kind) in enumerate(
    (
        ("name", fitz.PDF_WIDGET_TYPE_TEXT),
        ("date", fitz.PDF_WIDGET_TYPE_TEXT),
        ("agree", fitz.PDF_WIDGET_TYPE_CHECKBOX),
    )
):
    y = 90 + i * 40
    page.insert_text((72, y + 14), field, fontsize=11)
    widget = fitz.Widget()
    widget.field_name = field
    widget.field_type = kind
    widget.rect = fitz.Rect(130, y, 330 if kind == fitz.PDF_WIDGET_TYPE_TEXT else 146, y + (20 if kind == fitz.PDF_WIDGET_TYPE_TEXT else 16))
    page.add_widget(widget)
pdf.save(out / "form.pdf")

logo = Image.new("RGB", (120, 40), (18, 58, 107))
ImageDraw.Draw(logo).rectangle((8, 8, 32, 32), fill=(184, 67, 26))
logo.save(out / "logo.png")

# A placeholder picture: a grey square whose alt text holds an image tag,
# as Word's Alt Text pane writes it.
grey = Image.new("RGB", (60, 60), (200, 200, 200))
grey.save(out / "grey.png")
badge = Document()
badge.add_paragraph("Badge")
run = badge.add_paragraph().add_run()
shape = run.add_picture(str(out / "grey.png"), width=Inches(1.5), height=Inches(1.5))
shape._inline.docPr.set("descr", '{{ photo | image(fit="cover") }}')
badge.save(out / "badge.docx")
(out / "grey.png").unlink()
