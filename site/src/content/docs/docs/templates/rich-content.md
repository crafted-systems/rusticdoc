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
| `image` at the tag | Yes | Yes | Yes | Yes | Yes | Yes |
| `image` in a placeholder picture | Yes | Yes | Yes | Yes | Yes | Yes |
| `barcode` | Yes | Yes | Yes | Yes | Yes | Yes |
| `link` | Yes | Yes | Yes | Yes | Yes | Yes |
| `color` on text | Yes | Yes | Whole cell | Whole cell | Yes | Yes |
| `color` on a paragraph | Yes | Yes | Refused | Refused | Text colour only | Text colour only |
| `color` on a table cell or row | Yes | Yes | Yes | Yes | Yes | Yes |
| `color` on a shape | Refused | Refused | Refused | Refused | Yes | Yes |
| `html` | Native | Native | Plain text | Plain text | Plain text | Plain text |
| `chart` | Yes | Yes | Yes | Yes | Yes | Yes |
| `pagebreak` | Yes | Yes | Refused | Refused | Refused | Refused |
| `shift` | Refused | Refused | Refused | Refused | Yes | Refused |
| [`drop`](/docs/templates/office/#remove-an-element) | Yes | Yes | Yes | Yes | Yes | Yes |

Which elements `drop` removes depends on the format: a DOCX has no slides and a sheet no paragraphs. The [table of elements](/docs/templates/office/#remove-an-element) says which.

In a text template or a PDF tag these filters do not exist, and using one is a `422` with `unknown filter`.

## Rules for all of them

- **Print the result directly.** `{{ logo | image }}` works, and so does `{% set x = logo | image %}{{ x }}`. Sending the result through another filter, `~`, a macro or a `{% set %}...{% endset %}` block is refused: `a rich filter's result must be printed directly`.
- **Unknown arguments are refused**, not ignored. `barcode("qr", depth=90)` fails with `unknown keyword argument 'depth'`.
- **A missing value is an error**, as everywhere: `image: the value is undefined`.

## Trying the examples

The examples below take the sample invoice and replace the words `Billed to` in its XML with a tag. This does what typing the tag in Word does, and leaves the rest of the invoice as it was. In your own template, type the tag where the content should go.

## image

```text
{{ logo | image }}
{{ logo | image(width=120) }}
{{ logo | image(width=120, height=40) }}
{{ logo | image(width=120, height=120, fit="cover") }}
```

| Argument | Meaning |
|---|---|
| `width`, `height` | Printed size in points, above 0 and at most 14400. Give one and the aspect is kept. Give both and the image is stretched to them, unless `fit` says otherwise |
| `fit` | How the image sits in its box: `contain`, `cover` (or `fill`), `width` or `stretch`. See [fit](#fit) |

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

jq --arg logo "$(base64 < logo.png | tr -d '\n')" '. + {logo: $logo}' \
  invoice.json > with-logo.json
curl -F template=@tagged.docx -F "data=<with-logo.json" \
  http://localhost:9027/v1/render -o out.docx
unzip -l out.docx | grep -o 'word/media/rich1.png'
```

### Image types

The bytes decide the type, not a name or the `mime`.

| Type | What goes into the document |
|---|---|
| PNG, JPEG | The bytes as they are |
| GIF | The bytes as they are. A PDF shows the first frame of an animation |
| WebP | Converted to PNG, since office suites do not all read WebP |
| SVG | A cleaned copy of the drawing, with a PNG of it for readers that do not show SVG. DOCX, XLSX and PPTX carry the PNG as the picture and the SVG as Office's SVG extension; ODF frames list the SVG first and the PNG after it |

An SVG is rebuilt from what draws, element by element. Scripts, event attributes such as `onload`, animation, `foreignObject`, `<style>` stylesheets, and links to other files or addresses are all removed. A `style` attribute keeps only presentation properties such as `fill`, `stroke` and `font-size`, which become attributes, and every value must be plain: no backslash escapes, comments or `@`, and no function but colours, transforms and `url(#id)` pointing inside the file. References are followed before anything is drawn, so `use` elements and patterns that draw each other cannot turn a small file into millions of shapes: past 1,000,000 elements the image is refused. Elements may nest at most 256 deep, and two elements may not share an `id`. A picture inside an SVG stays only as embedded PNG, JPEG or GIF data. A DTD is refused. The PNG beside it is drawn by the service, with the fonts of the machine it runs on, at up to 2000 pixels on the long side.

```bash test
mkdir t && cd t && unzip -q ../invoice.docx
perl -pi -e 's/Billed to/{{ mark | image(width=60) }}/' word/document.xml
zip -qr ../tagged.docx . && cd ..

cat > mark.svg <<'EOF'
<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40">
  <circle cx="20" cy="20" r="18" fill="#b8431a"/>
  <script>alert(1)</script>
</svg>
EOF
jq --arg mark "$(base64 < mark.svg | tr -d '\n')" '. + {mark: $mark}' \
  invoice.json > with-mark.json
curl -F template=@tagged.docx -F "data=<with-mark.json" \
  http://localhost:9027/v1/render -o out.docx
unzip -l out.docx | grep -o 'word/media/rich[0-9]*\.svg'
unzip -p out.docx word/document.xml | grep -o 'asvg:svgBlip'
! unzip -p out.docx 'word/media/rich*.svg' | grep script
```

Refused:

| Value | Error |
|---|---|
| A URL | `the value must carry the image's bytes: URLs are not fetched`. Fetch it yourself and send the bytes |
| BMP, TIFF or anything else | `only PNG, JPEG, GIF, SVG and WebP images are supported` |
| A `mime` that does not match the bytes | `the bytes are image/png but the value says image/jpeg` |
| Over 20 MB, or an SVG over 5 MB | `the image is larger than 20971520 bytes` |
| A WebP over 25 megapixels or 16384 pixels a side | `the WebP image has more than 25000000 pixels` |
| An SVG with a DTD, no SVG root, more than 200,000 nodes, or a size over 19200 pixels a side | `the SVG image cannot be read: ...` and similar |
| A picture inside an SVG over 16 megapixels | `an embedded picture in the SVG image has more than 16000000 pixels` |

```bash test=fail
mkdir t && cd t && unzip -q ../invoice.docx
perl -pi -e 's/Billed to/{{ logo | image }}/' word/document.xml
zip -qr ../tagged.docx . && cd ..

jq '. + {logo: "https://example.com/logo.png"}' invoice.json > with-url.json
curl -F template=@tagged.docx -F "data=<with-url.json" \
  http://localhost:9027/v1/render -o out.docx
```

### Where the picture goes

| Format | Place |
|---|---|
| DOCX, ODT | In the line, where the tag is |
| XLSX | Anchored at the tag's cell, after rows are numbered |
| ODS | A frame on the tag's cell |
| PPTX, ODP | A new picture at the position of the shape that held the tag. The shape keeps its other text |

In PPTX the position is read from the shape itself. A placeholder that inherits its position from the layout has none, and the picture lands at the slide's top left corner. Put the tag in a shape with its own position, such as a text box, or use a placeholder picture.

### Placeholder pictures

To control exactly where a picture goes and how it looks, put a real picture in the template and write the tag in its alt text. The rendered image replaces the picture's image, and everything else about the picture stays: its frame, position, wrapping, border, shadow and other effects, and its brightness or recolouring.

| Application | Where the tag goes |
|---|---|
| Word, PowerPoint, Excel | Right-click the picture, **View Alt Text**, and type the tag in the description |
| LibreOffice | Right-click the picture, **Properties** (or **Description...**), and type it in **Title** or **Description** |

```text
{{ photo | image }}
{{ photo | image(fit="cover") }}
{{ sku | barcode("ean13", text=true) }}
```

The alt text holds one tag, and it must end in `image` or `barcode`. Text around it stays as the picture's alt text. Any other tag in the same alt text is filled in as text, as alt text tags are. A placeholder takes its size from its frame, so `width`, `height` and `size` are refused there: `a placeholder picture takes its size from its frame`. Choose how the image sits with `fit`, whose default in a placeholder is `contain`.

```bash test
jq -n --arg photo "$(base64 < logo.png | tr -d '\n')" '{photo: $photo}' \
  > photo.json
curl -F template=@badge.docx -F "data=<photo.json" \
  http://localhost:9027/v1/render -o out.docx
# A 3:1 logo covering a square frame loses a third of its width on each side.
unzip -p out.docx word/document.xml > out.xml
grep -o '<a:srcRect l="33333" t="0" r="33333" b="0"/>' out.xml
grep -o '<wp:extent cx="1371600" cy="1371600"/>' out.xml
```

A placeholder in a table row that a loop repeats gives each copy its own image, as `{{ item.photo | image }}` would. In XLSX, pictures live in a drawing beside the sheet, so a placeholder there sees the data's top level but not loop variables, and is not copied by a row loop.

Only pictures can be placeholders. A tag in the alt text of a shape, chart or text box is refused with `only a picture can be a placeholder`. Errors name the picture, as `picture "Picture 1"`.

### fit

`fit` decides how an image sits in a box of another shape: a placeholder's frame, or `width` and `height` given together.

| Value | Result |
|---|---|
| `contain` | All of the image, as large as fits, centred, with its aspect kept. Empty space fills the rest of the box. The default in a placeholder |
| `cover`, `fill` | The box filled, with the aspect kept and the overflow cropped equally from both sides |
| `width` | The box's width kept, and its height changed to follow the image's aspect |
| `stretch` | The image stretched to the box. The default for `width` and `height` given together |

Cropping is done the way the office suites do it, so the full image stays in the document and the crop can be undone in Word, PowerPoint, Excel or LibreOffice: `a:srcRect` in DOCX, XLSX and PPTX, and `fo:clip` in ODF. In XLSX, `fit="width"` turns a two-cell anchor into a one-cell anchor, so that the height can change.

## barcode

```text
{{ sku | barcode("code128") }}
{{ url | barcode("qr", size=90) }}
{{ gtin | barcode("ean13", text=true) }}
{{ ticket | barcode("pdf417", width=220, ec=5) }}
```

The first argument is required and names the symbology. There is no default.

| Symbology | Takes |
|---|---|
| `qr` | Any text a QR code can hold, encoded as UTF-8 |
| `datamatrix` | ASCII text, up to what the largest Data Matrix holds |
| `pdf417` | Any text a PDF417 symbol can hold |
| `aztec` | Any text an Aztec code can hold |
| `code128` | Printable ASCII |
| `gs1-128` | Application identifiers in parentheses, such as `(01)09501101530003(10)ABC123` |
| `code39` | Upper-case letters, digits, space and `- . $ / + %`. `lower` is refused |
| `code93` | Printable ASCII |
| `ean13` | 12 digits, or 13 with a correct check digit |
| `ean8` | 7 digits, or 8 with a correct check digit |
| `upca` | 11 digits, or 12 with a correct check digit |
| `upce` | 7 digits starting with 0 or 1, or 8 with a correct check digit |
| `itf` | An even number of digits, at most 80. Add a leading 0 to an odd one |
| `itf14` | 13 digits, or 14 with a correct check digit. Bearer bars are drawn above and below |
| `codabar` | Digits and `- $ : / . +`, between optional start and stop letters A to D |

A wrong check digit names the right one: `"5901234123458" has a wrong check digit: EAN-13 expects 7`. A value too long for its symbology gives `the value is too long for a QR code`, and so on.

For `gs1-128`, each identifier is 2 to 4 digits in parentheses, followed by its data. Identifiers whose data has a fixed length in the GS1 tables, such as `(01)` and `(17)`, are checked for that length, and `(00)`, `(01)` and `(02)` for their check digit. The service puts the FNC1 characters where GS1 wants them: at the start, and after each variable-length field that another follows. The text under the bars shows the parentheses.

| Argument | Meaning |
|---|---|
| `size` | Printed width in points, above 0 and at most 14400. The default is 144 for `qr`, `datamatrix`, `pdf417` and `aztec`, and 200 for the others |
| `width` | The same as `size`. Give one or the other |
| `height` | Printed height in points. A linear code's bars take what the text leaves. A `qr`, `datamatrix` or `aztec` code is square, so `height` alone sets both sides, and a `width` and `height` that differ are refused. A `pdf417` symbol stretches its rows to the height |
| `text` | `true` prints the value under a linear code. EAN and UPC codes print their digits between guard bars that reach down, with the leading and trailing digits outside the bars. The default is `false`, and two-dimensional codes refuse it |
| `font_size` | The text's size in points. The default is ten modules or an eighteenth of the width, whichever is smaller, between 6 and 12 points |
| `color` | The bars, `#rgb` or `#rrggbb`. The default is black |
| `background` | The space around them, as a colour or `transparent`. The default is white |
| `quiet_zone` | The empty margin on each side, in modules, from 0 to 100. The default is 10 for linear codes, 4 for `qr` and 2 for the other two-dimensional codes |
| `rotate` | `0`, `90`, `180` or `270` degrees clockwise. The printed width and height swap at 90 and 270 |
| `ec` | Error correction: `L`, `M`, `Q` or `H` for `qr` (default `M`), a percentage from 5 to 95 for `aztec`, a level from 0 to 8 for `pdf417`. Other symbologies refuse it |
| `fit` | How the code sits in a placeholder picture. See [fit](#fit) |

The sample invoice holds a QR code, which you can see with no changes to it:

```bash test
curl -F template=@invoice.docx -F "data=<invoice.json" \
  http://localhost:9027/v1/render -o out.docx
unzip -l out.docx | grep -o 'word/media/rich1.png'
```

A retail code with its digits under the bars, rotated and in colour:

```bash test
mkdir t && cd t && unzip -q ../invoice.docx
export TAG='{{ gtin | barcode("ean13", text=true, width=160,'
TAG+=' color="#123a6b", rotate=90) }}'
perl -pi -e 's/Billed to/$ENV{TAG}/' word/document.xml
zip -qr ../tagged.docx . && cd ..

jq '. + {gtin: "590123412345"}' invoice.json > with-gtin.json
curl -F template=@tagged.docx -F "data=<with-gtin.json" \
  http://localhost:9027/v1/render -o out.docx
unzip -l out.docx | grep -c 'word/media/rich[0-9]*\.png' | grep -x 2
```

A bad value is a `422` naming the problem:

```bash test=fail
mkdir t && cd t && unzip -q ../invoice.docx
perl -pi -e 's/Billed to/{{ "5901234123458" | barcode("ean13") }}/' \
  word/document.xml
zip -qr ../tagged.docx . && cd ..

curl -F template=@tagged.docx -F "data=<invoice.json" \
  http://localhost:9027/v1/render -o out.docx
```

A barcode counts as an image for placement and limits, and fills a placeholder picture like one. The bars are drawn with a whole number of device pixels per module, so their edges stay sharp, and the image is then printed at the size asked for. Every symbology decodes to its exact value with rxing, a port of ZXing, from the PNG the service makes, with and without text, colours, a wider quiet zone and rotation, and from LibreOffice's PDF of the document at 300 dpi. ZBar reads the same PNGs for every linear code and QR; it has no Data Matrix or Aztec reader. EAN and UPC codes come back with the check digit the service adds. rxing reports Codabar without its start and stop letters, and ZBar with them. At 100 dpi a long Code 128 or Code 39 at the default size is too fine to read, so give it a larger `size` for screens. No code has been scanned with a hardware or phone reader, as [known limits](/docs/project/known-limits/) says, so test yours on your own hardware.

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
curl -F template=@tagged.docx -F "data=<with-link.json" \
  http://localhost:9027/v1/render -o out.docx
unzip -p out.docx word/_rels/document.xml.rels |
  grep -o 'Target="https://example.com"'
```

## color

```text
{{ status | color("#c00") }}
{{ status | color("#c00", background="#ffee00") }}
{{ status | color(background="#ffee00") }}
{{ status | color(background="#fdd", scope="row") }}
```

A colour is `#rgb` or `#rrggbb`, and nothing else: `red` is refused. Give a colour, a `background=`, or both. Both can come from data.

The colour is the text's. `background` fills whatever `scope` names, which is the text itself by default:

| `scope` | The colour | The background |
|---|---|---|
| `text` (default) | The printed value | Shading behind the printed value. In XLSX and ODS, the whole cell |
| `paragraph` | Every run of the paragraph that holds the tag | The paragraph's shading. PPTX and ODP paragraphs have none, so a `background` is refused there |
| `cell` | The text of the table cell that holds the tag | The cell's fill |
| `row` | The text of every cell in the row | The fill of every cell in the row. In XLSX the row's own format too, so empty cells match |
| `shape` | The text of the shape | The shape's fill. PPTX and ODP only |

The printed value always stays where the tag is. A colour from a scope does not change runs that another rich value made, such as a link.

Sheets have no runs of text here, so in XLSX and ODS `text` and `cell` both colour the whole cell, and the last `color` in a cell wins. They have no paragraphs or shapes either, and asking for them is refused: `XLSX has no paragraphs to colour; use scope="cell" or scope="row"`. A `cell` or `row` scope outside a table is refused where the tag is: `scope="cell" needs the tag inside a table cell`.

```bash test
mkdir t && cd t && unzip -q ../invoice.docx
perl -pi -e 's/Billed to/{{ number | color("#c00") }}/' word/document.xml
zip -qr ../tagged.docx . && cd ..

curl -F template=@tagged.docx -F "data=<invoice.json" \
  http://localhost:9027/v1/render -o out.docx
unzip -p out.docx word/document.xml | grep -o '<w:color w:val="CC0000"/>'
```

A shaded paragraph:

```bash test
mkdir t && cd t && unzip -q ../invoice.docx
export TAG='{{ number | color(background="#fee", scope="paragraph") }}'
perl -pi -e 's/Billed to/$ENV{TAG}/' word/document.xml
zip -qr ../tagged.docx . && cd ..

curl -F template=@tagged.docx -F "data=<invoice.json" \
  http://localhost:9027/v1/render -o out.docx
unzip -p out.docx word/document.xml |
  grep -o '<w:shd w:val="clear" w:color="auto" w:fill="FFEEEE"/>'
```

```bash test=fail
mkdir t && cd t && unzip -q ../invoice.docx
export TAG='{{ number | color(background="#fee", scope="shape") }}'
perl -pi -e 's/Billed to/$ENV{TAG}/' word/document.xml
zip -qr ../tagged.docx . && cd ..

curl -F template=@tagged.docx -F "data=<invoice.json" \
  http://localhost:9027/v1/render -o out.docx
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

jq '. + {note: "<p>Thanks for your <b>order</b></p><script>bad()</script>"}' \
  invoice.json > with-note.json
curl -F template=@tagged.docx -F "data=<with-note.json" \
  http://localhost:9027/v1/render -o out.docx
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
export TAG='{{ rows | chart(type="column", x="month",'
TAG+=' y=["sales"], title="Sales") }}'
perl -pi -e 's/Billed to/$ENV{TAG}/' word/document.xml
zip -qr ../tagged.docx . && cd ..

jq '. + {rows: [
  {month: "March", sales: 90},
  {month: "April", sales: 120}
]}' invoice.json > with-rows.json
curl -F template=@tagged.docx -F "data=<with-rows.json" \
  http://localhost:9027/v1/render -o out.docx
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

curl -F template=@tagged.docx -F "data=<invoice.json" \
  http://localhost:9027/v1/render -o out.docx
unzip -p out.docx word/document.xml | grep -o '<w:br w:type="page"/>'
```

```bash test=fail
mkdir t && cd t && unzip -q ../deck.pptx
perl -pi -e 's/A slide/{{ "" | pagebreak }}/' ppt/slides/slide1.xml
zip -qr ../tagged.pptx . && cd ..

curl -F template=@tagged.pptx -F 'data={"_":1}' \
  http://localhost:9027/v1/render -o out.pptx
```


## Limits

| What | Limit |
|---|---|
| One image | 20 MB |
| One SVG image | 5 MB, 200,000 nodes nested at most 256 deep, 1,000,000 elements once references are expanded, 19200 pixels a side |
| A picture inside an SVG | 16 megapixels |
| One WebP image | 25 megapixels, 16384 pixels a side |
| Images and barcodes in one render | 200 |
| Barcode value | 4 KB, and 4096 modules when linear |
| Barcode image | 4 megapixels. The module gets smaller, and the printed size does not |
| Charts in one render | 200 |
| Values in one chart | 1,000 |
| One `html` value | 1 MB, nested at most 64 deep |
| One HTML table | 256 columns, 10,000 rows, 50,000 cells counting padded ones |
| All HTML tables in one render | 100,000 cells |
| Rich values in one render | 100,000 |
