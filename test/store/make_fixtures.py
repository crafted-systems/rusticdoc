#!/usr/bin/env python3
"""Fixtures for replicas.sh: a font with a family name nothing else has, packed in a
ZIP with a hostile path and a licence, and a DOCX template that asks for that font."""
import sys
import zipfile

from fontTools.ttLib import TTFont

FAMILY = "Rusticdoc Probe"
source, out = sys.argv[1], sys.argv[2]

font = TTFont(source)
for record in font["name"].names:
    if record.nameID in (1, 4, 16):
        record.string = FAMILY
    elif record.nameID == 6:
        record.string = FAMILY.replace(" ", "")
font.save(f"{out}/probe.ttf")

with zipfile.ZipFile(f"{out}/fonts.zip", "w", zipfile.ZIP_DEFLATED) as pack:
    pack.write(f"{out}/probe.ttf", "../../../../tmp/escaped/probe.ttf")
    pack.writestr("LICENSE.txt", "terms")
    pack.writestr("__MACOSX/._probe.ttf", "junk")

WORD = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
PKG = "http://schemas.openxmlformats.org/package/2006"
with zipfile.ZipFile(f"{out}/letter.docx", "w", zipfile.ZIP_DEFLATED) as docx:
    docx.writestr(
        "[Content_Types].xml",
        f'<?xml version="1.0"?><Types xmlns="{PKG}/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
        "</Types>",
    )
    docx.writestr(
        "_rels/.rels",
        f'<?xml version="1.0"?><Relationships xmlns="{PKG}/relationships">'
        '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>'
        "</Relationships>",
    )
    docx.writestr(
        "word/document.xml",
        f'<?xml version="1.0"?><w:document xmlns:w="{WORD}"><w:body><w:p><w:r>'
        f'<w:rPr><w:rFonts w:ascii="{FAMILY}" w:hAnsi="{FAMILY}" w:cs="{FAMILY}"/></w:rPr>'
        "<w:t>Dear {{ name }}</w:t></w:r></w:p></w:body></w:document>",
    )
