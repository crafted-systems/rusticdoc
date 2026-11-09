---
title: Rich content
description: Put images, barcodes, links, coloured text, HTML, charts and page breaks into Office templates.
sidebar:
  order: 3
---

This page shows how to place content that is more than text: a logo, a QR code, a link, coloured text, formatted HTML, a native chart or a page break. Each one is a filter, and each works in Office templates only. The `shift` filter, which spreads a loop's shapes across a slide, is described with [PPTX loops](/docs/templates/office/#shapes-that-move-down-or-across-a-slide), and the `drop` function, which removes the element holding its tag, under [remove an element](/docs/templates/office/#remove-an-element).

## What works where

| Filter | DOCX | ODT | XLSX | ODS | PPTX | ODP |
|---|---|---|---|---|---|---|
| `image` | Yes | Yes | Yes | Yes | Yes | Yes |
| `barcode` | Yes | Yes | Yes | Yes | Yes | Yes |
| `link` | Yes | Yes | Yes | Yes | Yes | Yes |
| `color` | Yes | Yes | Whole cell | Whole cell | Yes | Yes |
| `html` | Native | Native | Plain text | Plain text | Plain text | Plain text |
| `chart` | Yes | Yes | Yes | Yes | Yes | Yes |
| `pagebreak` | Yes | Yes | Refused | Refused | Refused | Refused |
| `shift` | Refused | Refused | Refused | Refused | Yes | Refused |
| [`drop`](/docs/templates/office/#remove-an-element) | Yes | Yes | Yes | Yes | Yes | Yes |

Which elements `drop` removes depends on the format: a DOCX has no slides and a sheet no paragraphs. The [table of elements](/docs/templates/office/#remove-an-element) says which.

In a text template or a PDF tag these filters do not exist, and using one is a `422` with `unknown filter`.

## Rules for all of them

- **Print the result directly.** `{{ logo | image }}` works, and so does `{% set x = logo | image %}{{ x }}`. Sending the result through another filter, `~`, a macro or a `{% set %}...{% endset %}` block is refused: `a rich filter's result must be printed directly`.
- **Unknown arguments are refused**, not ignored. `barcode("qr", width=90)` fails with `unknown keyword argument 'width'`.
- **A missing value is an error**, as everywhere: `image: the value is undefined`.

## Trying the examples

The examples below take the sample invoice and replace the words `Billed to` in its XML with a tag. This does what typing the tag in Word does, and leaves the rest of the invoice as it was. In your own template, type the tag where the content should go.

## image

```text
{{ logo | image }}
{{ logo | image(width=120) }}
{{ logo | image(width=120, height=40) }}
```

| Argument | Meaning |
|---|---|
| `width`, `height` | Printed size in points, above 0 and at most 14400. Give one and the aspect is kept. Give both and it is not |

With no size, the picture prints at its natural size at 96 dpi, capped at a width of 450 points.

The value carries the image's bytes, in one of three forms:

| Form | Example |
|---|---|
| Base64 | `"iVBORw0KGgo..."` |
| A data URL | `"data:image/png;base64,iVBORw0KGgo..."` |
| An object | `{"data": "iVBORw0KGgo...", "mime": "image/png", "width": 30, "height": 30}` |

`mime`, `width` and `height` are optional in the object, and the filter's arguments win over its sizes.

```bash test
mkdir t && cd t && unzip -q ../invoice.docx
perl -pi -e 's/Billed to/{{ logo | image(width=120) }}/' word/document.xml
zip -qr ../tagged.docx . && cd ..

jq --arg logo "$(base64 < logo.png | tr -d '\n')" '. + {logo: $logo}' invoice.json > with-logo.json
curl -F template=@tagged.docx -F "data=<with-logo.json" http://localhost:9027/v1/render -o out.docx
unzip -l out.docx | grep -o 'word/media/rich1.png'
```

Refused:

| Value | Error |
|---|---|
| A URL | `the value must carry the image's bytes: URLs are not fetched`. Fetch it yourself and send the bytes |
| GIF, SVG, WebP or anything else | `only PNG and JPEG images are supported`. The bytes decide, not the name |
| A `mime` that does not match the bytes | `the bytes are image/png but the value says image/jpeg` |
| Over 20 MB | `the image is larger than 20971520 bytes` |

```bash test=fail
mkdir t && cd t && unzip -q ../invoice.docx
perl -pi -e 's/Billed to/{{ logo | image }}/' word/document.xml
zip -qr ../tagged.docx . && cd ..

jq '. + {logo: "https://example.com/logo.png"}' invoice.json > with-url.json
curl -F template=@tagged.docx -F "data=<with-url.json" http://localhost:9027/v1/render -o out.docx
```

### Where the picture goes

| Format | Place |
|---|---|
| DOCX, ODT | In the line, where the tag is |
| XLSX | Anchored at the tag's cell, after rows are numbered |
| ODS | A frame on the tag's cell |
| PPTX, ODP | A new picture at the position of the shape that held the tag. The shape keeps its other text |

In PPTX the position is read from the shape itself. A placeholder that inherits its position from the layout has none, and the picture lands at the slide's top left corner. Put the tag in a shape with its own position, such as a text box.

## barcode

```text
{{ sku | barcode("code128") }}
{{ url | barcode("qr", size=90) }}
```

The first argument is required and names the symbology. There is no default.

| Symbology | Takes |
|---|---|
| `qr` | Any text a QR code can hold. Too much gives `data too long` |
| `code128` | Printable ASCII |
| `code39` | Upper-case letters, digits and the symbols Code 39 defines. `lower` is refused |
| `ean13` | 12 digits, or 13 with a correct check digit |
| `ean8` | 7 digits, or 8 with a correct check digit |

| Argument | Meaning |
|---|---|
| `size` | Printed width in points, above 0 and at most 14400. The default is 144 for `qr` and 200 for the others |

The keyword is `size`. There is no `width` or `height`. No human-readable text is drawn under the bars, so print the value next to the code if a person needs it.

The sample invoice holds a QR code, which you can see with no changes to it:

```bash test
curl -F template=@invoice.docx -F "data=<invoice.json" http://localhost:9027/v1/render -o out.docx
unzip -l out.docx | grep -o 'word/media/rich1.png'
```

A barcode counts as an image for placement and limits. Every symbology decodes to its exact value with ZXing and ZBar, from the document and from its PDF at 150 and 300 dpi, and EAN codes come back with the check digit the service adds. At 100 dpi a long Code 128 or Code 39 at the default size is too fine to read, so give it a larger `size` for screens. No code has been scanned with a hardware or phone reader, as [known limits](/docs/project/known-limits/) says, so test yours on your own hardware.

## link

```text
{{ url | link }}
{{ url | link("Pay now") }}
```

The one optional argument is the text to show. Without it the link shows its address, with `mailto:` removed.

Only `http`, `https` and `mailto` links are allowed. `javascript:`, `file:` and relative addresses such as `/pay` are refused with an error naming the problem. A link inside a hyperlink that is already in the template is refused too.

```bash test
mkdir t && cd t && unzip -q ../invoice.docx
perl -pi -e 's/Billed to/{{ site | link("Open") }}/' word/document.xml
zip -qr ../tagged.docx . && cd ..

jq '. + {site: "https://example.com"}' invoice.json > with-link.json
curl -F template=@tagged.docx -F "data=<with-link.json" http://localhost:9027/v1/render -o out.docx
unzip -p out.docx word/_rels/document.xml.rels | grep -o 'Target="https://example.com"'
```

## color

```text
{{ status | color("#c00") }}
{{ status | color("#c00", background="#ffee00") }}
{{ status | color(background="#ffee00") }}
```

A colour is `#rgb` or `#rrggbb`, and nothing else: `red` is refused. Give a colour, a `background=`, or both. Both can come from data.

Sheets have no runs of text here, so in XLSX and ODS the filter colours the whole cell, and the last `color` in a cell wins.

```bash test
mkdir t && cd t && unzip -q ../invoice.docx
perl -pi -e 's/Billed to/{{ number | color("#c00") }}/' word/document.xml
zip -qr ../tagged.docx . && cd ..

curl -F template=@tagged.docx -F "data=<invoice.json" http://localhost:9027/v1/render -o out.docx
unzip -p out.docx word/document.xml | grep -o '<w:color w:val="CC0000"/>'
```

## html

```text
{{ body | html }}
```

It takes no arguments. It reads this subset:

| Kind | Tags |
|---|---|
| Blocks | `p`, `h1` to `h6`, `ul`, `ol`, `li`, `table`, `tr`, `td`, `th` |
| Inline | `b`, `strong`, `i`, `em`, `u`, `s`, `sub`, `sup`, `br` |
| Links | `a href="..."`, with the same rules as `link` |
| Colour | `span style="color:#hex"` |

Everything else follows these rules:

- `script` and `style` are removed with their content.
- Other tags are dropped and their text kept, so `<div>Hi</div>` gives a paragraph holding `Hi` and `<img src="x">` gives nothing.
- No attribute is copied except a checked `href` and colour. A `span` with any other style loses the style.
- An `a` with a scheme `link` would refuse is an error. Broken HTML is otherwise closed as well as can be, and never refused.

What you get depends on the format:

| Format | Result |
|---|---|
| DOCX | Native paragraphs and tables. The tag's paragraph is replaced by generated ones with its properties, and text around the tag stays in paragraphs of its own. Lists are indents with a literal bullet or number. Headings are bold and sized |
| ODT | Native paragraphs, lists and tables, with the same handling of text around the tag |
| PPTX, XLSX, ODS, ODP | Plain text, with a line break per paragraph, item or row. Bullets and numbers stay, and table cells are separated by tabs. XLSX also sets wrap text on the cell |

In DOCX the tag must sit directly in a paragraph, not in a hyperlink or content control.

```bash test
mkdir t && cd t && unzip -q ../invoice.docx
perl -pi -e 's/Billed to/{{ note | html }}/' word/document.xml
zip -qr ../tagged.docx . && cd ..

jq '. + {note: "<p>Thanks for your <b>order</b></p><script>bad()</script>"}' invoice.json > with-note.json
curl -F template=@tagged.docx -F "data=<with-note.json" http://localhost:9027/v1/render -o out.docx
unzip -p out.docx word/document.xml > out.xml
grep -o 'Thanks for your ' out.xml
! grep 'bad()' out.xml
```

## chart

```text
{{ rows | chart(type="bar", x="month", y=["sales", "costs"], title="Revenue") }}
```

The value is a list of objects, and each object is one category.

| Argument | Required | Meaning |
|---|---|---|
| `type` | Yes | `bar`, `column`, `line` or `pie` |
| `x` | Yes | The key holding each category's label |
| `y` | Yes | A key, or a list of keys for several series. A `pie` takes one |
| `title` | No | The chart's title |
| `width`, `height` | No | Size in points. The default is 450 by 270 |

```bash test
mkdir t && cd t && unzip -q ../invoice.docx
perl -pi -e 's/Billed to/{{ rows | chart(type="column", x="month", y=["sales"], title="Sales") }}/' word/document.xml
zip -qr ../tagged.docx . && cd ..

jq '. + {rows: [{month: "March", sales: 90}, {month: "April", sales: 120}]}' invoice.json > with-rows.json
curl -F template=@tagged.docx -F "data=<with-rows.json" http://localhost:9027/v1/render -o out.docx
unzip -l out.docx | grep -o 'word/charts/chart1.xml'
```

Values must be numbers. A `null` or missing value is a gap, while a key with no value in any row is an error, since that is what a misspelt key looks like: `no row has a value for "nope"`. A string such as `"12"` is refused. So is an empty list, and a row with no `x` key.

- **DOCX and PPTX.** The chart carries its values inline and has no embedded workbook. It renders, but its data cannot be edited in Word or PowerPoint.
- **XLSX.** The chart is anchored at the tag's cell, and a hidden sheet named `RichChartData` holds the values.
- **PPTX.** The chart goes at the position of the shape that held the tag. A placeholder PowerPoint has not moved has no position of its own, so the chart takes the one its layout or master gives it.
- **ODT, ODS and ODP.** The chart is an embedded chart object, `Object N/` in the package, as LibreOffice writes one. It carries its values in its own table and does not read a sheet, so in ODS it does not follow the cells around it. LibreOffice opens it as a chart whose data can be edited. In ODT it sits in the line where the tag is, in ODS it is a frame on the tag's cell, and in ODP it goes at the position of the shape that held the tag. Two charts or pictures for the same cell or shape are stacked, the second below the first.

Every format draws the series, or a pie's slices, in the same colours in turn: blue, orange, grey, gold, light blue and green, Office's default accents. DOCX, XLSX and PPTX take them from the document's theme, so a template with another theme shows its own colours there. ODF charts have the colours written in.

## pagebreak

```text
{{ "" | pagebreak }}
```

The value is ignored, and it takes no arguments. It works in DOCX and ODT. Other formats refuse it: `page breaks exist only in DOCX and ODT, not PPTX`.

In a DOCX the break goes where the tag is. In an ODT, a tag with nothing after it breaks after its paragraph, and otherwise the text after it starts a new paragraph on the new page.

It is useful between loop iterations. See [paragraph loops](/docs/templates/office/#paragraphs).

```bash test
mkdir t && cd t && unzip -q ../invoice.docx
perl -pi -e 's/Billed to/{{ "" | pagebreak }}/' word/document.xml
zip -qr ../tagged.docx . && cd ..

curl -F template=@tagged.docx -F "data=<invoice.json" http://localhost:9027/v1/render -o out.docx
unzip -p out.docx word/document.xml | grep -o '<w:br w:type="page"/>'
```

```bash test=fail
mkdir t && cd t && unzip -q ../deck.pptx
perl -pi -e 's/A slide/{{ "" | pagebreak }}/' ppt/slides/slide1.xml
zip -qr ../tagged.pptx . && cd ..

curl -F template=@tagged.pptx -F 'data={"_":1}' http://localhost:9027/v1/render -o out.pptx
```

## Limits

| What | Limit |
|---|---|
| One image | 20 MB |
| Images and barcodes in one render | 200 |
| Barcode value | 4 KB, and 4096 modules when linear |
| Barcode image | 4 megapixels. The module gets smaller, and the printed size does not |
| Charts in one render | 200 |
| Values in one chart | 1,000 |
| One `html` value | 1 MB, nested at most 64 deep |
| One HTML table | 256 columns, 10,000 rows, 50,000 cells counting padded ones |
| All HTML tables in one render | 100,000 cells |
| Rich values in one render | 100,000 |
