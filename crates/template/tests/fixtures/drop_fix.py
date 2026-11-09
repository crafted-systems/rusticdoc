"""Finishes drop.pptx and drop.xlsx after LibreOffice wrote them.

LibreOffice leaves out a chart's alt text when it saves PPTX or XLSX, and
sizes table rows by the tags' text, which wraps in narrow cells. This puts
the chart's tag back in its alt text and shrinks the rows, as an author
would in PowerPoint and Excel. Usage: drop_fix.py drop.pptx drop.xlsx
"""
import re, sys, zipfile

ALT = 'descr="{% if not chart %}{{ drop(&quot;chart&quot;) }}{% endif %}"'
for path, part in [(sys.argv[1], "ppt/slides/slide1.xml"), (sys.argv[2], "xl/drawings/drawing1.xml")]:
    with zipfile.ZipFile(path) as z:
        items = [(i, z.read(i.filename)) for i in z.infolist()]
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for info, data in items:
            if info.filename == part:
                text = data.decode()
                for prefix in ("p", "xdr"):
                    text = text.replace(f'<{prefix}:cNvPr id="14" name="Sales chart"', f'<{prefix}:cNvPr id="14" name="Sales chart" {ALT}')
                    text = text.replace(f'<{prefix}:cNvPr id="2" name="Sales chart"', f'<{prefix}:cNvPr id="2" name="Sales chart" {ALT}')
                assert ALT in text, path
                # LibreOffice measured the rows with the tags' text wrapped;
                # shrink them, as an author would in PowerPoint.
                text = re.sub(r'<a:tr h="\d+">', '<a:tr h="370840">', text)
                data = text.encode()
            z.writestr(info, data)
