---
title: Rich content
seoTitle: "Images, barcodes, QR codes and charts in Word templates"
description: Put images, barcodes, links, coloured text, HTML, charts and page breaks into Office templates.
sidebar:
  order: 3
---

Rich content is anything a template prints that's more than text: a logo, a QR code, a link, coloured text, formatted HTML, a native chart or a page break. Each one is a filter, and they only work in Office templates. The `shift` filter, which spreads a loop's shapes across a slide, is covered with [PPTX loops](/docs/templates/office/#shapes-that-move-down-or-across-a-slide), and the `drop` function, which removes the element holding its tag, under [remove an element](/docs/templates/office/#remove-an-element).

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

Which elements `drop` removes depends on the format, since a DOCX has no slides and a sheet has no paragraphs. The [table of elements](/docs/templates/office/#remove-an-element) lists them.

These filters don't exist in a text template or a PDF tag, so using one there fails with a `422` and `unknown filter`.

## Rules for all of them

A rich filter's result has to be printed directly. `{{ logo | image }}` works, and so does `{% set x = logo | image %}{{ x }}`, but sending the result through another filter, `~`, a macro or a `{% set %}...{% endset %}` block is refused with `a rich filter's result must be printed directly`.

Unknown arguments are refused rather than ignored, so `barcode("qr", depth=90)` fails with `unknown keyword argument 'depth'`. A missing value is an error, as it is everywhere else: `image: the value is undefined`.

## Trying the examples

The examples below take the sample invoice and replace the words `Billed to` in its XML with a tag. That does the same thing as typing the tag in Word, and leaves the rest of the invoice alone. In your own template, type the tag where you want the content to go.

## image

```text
{{ logo | image }}
{{ logo | image(width=120) }}
{{ logo | image(width=120, height=40) }}
{{ logo | image(width=120, height=120, fit="cover") }}
```

| Argument | Meaning |
|---|---|
| `width`, `height` | Printed size in points, above 0 and at most 14400. If you give one, the aspect ratio is kept. If you give both, the image is stretched to them unless `fit` says otherwise |
| `fit` | How the image sits in its box: `contain`, `cover` (or `fill`), `width` or `stretch`. See [fit](#fit) |
| `alt` | The picture's alt text, read by screen readers. See [alt text](#alt-text) |

Without a size, the picture prints at its natural size at 96 dpi, capped at a width of 450 points.

The value has to carry the image's bytes, in one of three forms, or come from [`asset`](/docs/guides/assets/#use-one-in-an-office-template):

| Form | Example |
|---|---|
| Base64 | `"iVBORw0KGgo..."` |
| A data URL | `"data:image/png;base64,iVBORw0KGgo..."` |
| An object | `{"data": "iVBORw0KGgo...", "mime": "image/png", "width": 30, "height": 30}` |
| An asset the service holds | `{{ asset("brand/logo.png") \| image }}` in the template, with nothing in the data |

In the object, `mime`, `width` and `height` are optional, and the filter's arguments override its sizes.

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

The type is worked out from the bytes, not from a name or the `mime`.

| Type | What goes into the document |
|---|---|
| PNG, JPEG | The bytes as they are |
| GIF | The bytes as they are. A PDF shows the first frame of an animation |
| WebP | Converted to PNG, because not every office suite reads WebP |
| SVG | A cleaned copy of the drawing, plus a PNG of it for readers that don't show SVG. DOCX, XLSX and PPTX carry the PNG as the picture and the SVG as Office's SVG extension. ODF frames list the SVG first and the PNG after it |

An SVG is rebuilt element by element from the parts that draw something. Scripts, event attributes like `onload`, animation, `foreignObject`, `<style>` stylesheets, and links to other files or addresses are all removed. A `style` attribute keeps only presentation properties like `fill`, `stroke` and `font-size`, which become attributes. Every value has to be plain, which means no backslash escapes, comments or `@`, and no functions other than colours, transforms and a `url(#id)` that points inside the file.

References are followed before anything is drawn, so `use` elements and patterns that draw each other can't turn a small file into millions of shapes. Past 1,000,000 elements the image is refused. Elements can nest at most 256 deep, and two elements can't share an `id`. A picture inside an SVG is kept only as embedded PNG, JPEG or GIF data, and a DTD is refused.

The PNG that goes with it is drawn by the service, using the fonts of the machine it runs on, at up to 2000 pixels on the long side.

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

These values are refused:

| Value | Error |
|---|---|
| A URL | `the value must carry the image's bytes: URLs are not fetched`. Fetch it yourself and send the bytes |
| BMP, TIFF or anything else | `only PNG, JPEG, GIF, SVG and WebP images are supported` |
| A `mime` that doesn't match the bytes | `the bytes are image/png but the value says image/jpeg` |
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

In PPTX the position is read from the shape itself. A placeholder that inherits its position from the layout has none of its own, so the picture lands at the slide's top left corner. To avoid that, put the tag in a shape that has its own position, like a text box, or use a placeholder picture.

### Placeholder pictures

To control exactly where a picture goes and how it looks, put a real picture in the template and write the tag in its alt text. The rendered image replaces the picture's image and everything else about the picture stays: its frame, position, wrapping, border, shadow and other effects, and its brightness or recolouring.

| Application | Where the tag goes |
|---|---|
| Word, PowerPoint, Excel | Right-click the picture, **View Alt Text**, and type the tag in the description |
| LibreOffice | Right-click the picture, **Properties** (or **Description...**), and type it in **Title** or **Description** |

```text
{{ photo | image }}
{{ photo | image(fit="cover") }}
{{ sku | barcode("ean13", text=true) }}
```

The alt text holds one tag, which has to end in `image` or `barcode`. Text around it stays as the picture's alt text, and any other tag in the same alt text is filled in as text, the way alt text tags normally are.

A placeholder takes its size from its frame, so `width`, `height` and `size` are refused there with `a placeholder picture takes its size from its frame`. Use `fit` to choose how the image sits in the frame, where it defaults to `contain`.

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

When a loop repeats a table row that has a placeholder in it, each copy gets its own image, as it would with `{{ item.photo | image }}`. XLSX is different, because pictures there live in a drawing beside the sheet. A placeholder in an XLSX sees the data's top level but not loop variables, and a row loop doesn't copy it.

Only pictures can be placeholders, so a tag in the alt text of a shape, chart or text box is refused with `only a picture can be a placeholder`. Errors name the picture, for example `picture "Picture 1"`.

### fit

`fit` decides how an image sits in a box with a different shape from its own. That box is either a placeholder's frame or a `width` and `height` given together.

| Value | Result |
|---|---|
| `contain` | The whole image, as large as fits, centred, with its aspect ratio kept. The rest of the box is left empty. The default in a placeholder |
| `cover`, `fill` | The box is filled, with the aspect ratio kept and the overflow cropped equally from both sides |
| `width` | The box keeps its width, and its height changes to follow the image's aspect ratio |
| `stretch` | The image is stretched to the box. The default when `width` and `height` are given together |

Cropping is done the way the office suites do it, with `a:srcRect` in DOCX, XLSX and PPTX and `fo:clip` in ODF. The full image stays in the document, so the crop can be undone in Word, PowerPoint, Excel or LibreOffice. In XLSX, `fit="width"` turns a two-cell anchor into a one-cell anchor so that the height can change.

## barcode

```text
{{ sku | barcode("code128") }}
{{ url | barcode("qr", size=90) }}
{{ gtin | barcode("ean13", text=true) }}
{{ ticket | barcode("pdf417", width=220, ec=5) }}
{{ url | barcode("qr", style="dots", eye="circle", color="#2a7a6f") }}
```

The first argument names the symbology. It's required, and there's no default.

| Symbology | Accepts |
|---|---|
| `qr` | Any text a QR code can hold, encoded as UTF-8 |
| `gs1-qr` | Application identifiers in parentheses, like `gs1-128`, in a QR code that starts with FNC1 |
| `swissqr` | A Swiss QR-bill payload, which [`qrdata("swiss")`](/docs/templates/filters/#qrdata) builds |
| `datamatrix` | ASCII text, up to what the largest Data Matrix holds |
| `pdf417` | Any text a PDF417 symbol can hold |
| `aztec` | Any text an Aztec code can hold |
| `code128` | Printable ASCII |
| `gs1-128` | Application identifiers in parentheses, like `(01)09501101530003(10)ABC123` |
| `code39` | Upper-case letters, digits, space and `- . $ / + %`. `lower` is refused |
| `code93` | Printable ASCII |
| `ean13` | 12 digits, or 13 with a correct check digit |
| `ean8` | 7 digits, or 8 with a correct check digit |
| `upca` | 11 digits, or 12 with a correct check digit |
| `upce` | 7 digits starting with 0 or 1, or 8 with a correct check digit |
| `itf` | An even number of digits, at most 80. Add a leading 0 to an odd one |
| `itf14` | 13 digits, or 14 with a correct check digit. Bearer bars are drawn above and below |
| `codabar` | Digits and `- $ : / . +`, between optional start and stop letters A to D |

If a check digit is wrong, the error gives the right one: `"5901234123458" has a wrong check digit: EAN-13 expects 7`. A value that's too long for its symbology gives `the value is too long for a QR code`, and so on.

For `gs1-128` and `gs1-qr`, each identifier is 2 to 4 digits in parentheses, followed by its data. Identifiers whose data has a fixed length in the GS1 tables, like `(01)` and `(17)`, are checked for that length, and `(00)`, `(01)` and `(02)` are checked for their check digit. The service puts the FNC1 characters where GS1 wants them, which is at the start and after each variable-length field that's followed by another. In a QR code the start is a mode marker and the separator is the group separator character, `\x1D`, so a scanner returns `010950110153000310ABC123` for `(01)09501101530003(10)ABC123`. The text under a `gs1-128` code shows the parentheses.

These arguments work with every symbology:

| Argument | Meaning |
|---|---|
| `size` | Printed width in points, above 0 and at most 14400. The default is 144 for two-dimensional codes and 200 for the others |
| `width` | The same as `size`. Give one or the other |
| `height` | Printed height in points. A linear code's bars take whatever the text leaves. A `qr`, `datamatrix` or `aztec` code is square, so `height` alone sets both sides, and a `width` and `height` that differ are refused. A `pdf417` symbol stretches its rows to the height |
| `text` | `true` prints the value under a linear code. EAN and UPC codes print their digits between guard bars that reach down, with the leading and trailing digits outside the bars. The default is `false`, and two-dimensional codes refuse it |
| `font_size` | The size of the text or label in points. For text, the default is ten modules or an eighteenth of the width, whichever is smaller, kept between 6 and 12 points. For a label, it's a twelfth of the width, kept between 6 and 14 points |
| `color` | The colour of the bars, as `#rgb` or `#rrggbb`. The default is black |
| `background` | The space around the bars, as a colour or `transparent`. The default is white |
| `invert` | `true` allows bars lighter than the background, and makes the defaults white bars on black. See [colours](#colours-a-scanner-can-read) |
| `quiet_zone` | The empty margin on each side, in modules, from 0 to 100. The default is 10 for linear codes, 4 for QR codes and 2 for the other two-dimensional codes |
| `rotate` | `0`, `90`, `180` or `270` degrees clockwise. The printed width and height swap at 90 and 270 |
| `ec` | Error correction: `L`, `M`, `Q` or `H` for QR codes (default `M`, or `H` with a logo), a percentage from 5 to 95 for `aztec`, a level from 0 to 8 for `pdf417`. Other symbologies refuse it |
| `alt` | The picture's alt text, read by screen readers. See [alt text](#alt-text) |
| `fit` | How the code sits in a placeholder picture. See [fit](#fit) |

These work with `qr` and `gs1-qr`, and other symbologies refuse them:

| Argument | Meaning |
|---|---|
| `style` | How the modules are drawn: `square`, `dots` or `rounded`. `rounded` joins neighbouring modules and rounds the outer corners. The default is `square` |
| `module_scale` | The size of each module's mark, from 0.7 to 1, for gaps between dots or squares. The default is 1 |
| `eye` | The frame of the three corner squares: `square`, `rounded` or `circle` |
| `eye_center` | The middle of the corner squares, with the same choices. It takes the shape of `eye` unless you give it |
| `eye_color`, `eye_center_color` | Colours for the frames and middles of the corner squares. They default to the bars' colour |
| `gradient` | `linear` or `radial`, from `color` to `gradient_color` across the code |
| `gradient_color` | The colour the gradient ends in. A gradient needs it |
| `gradient_angle` | The direction of a linear gradient in degrees, clockwise from left to right. The default is 0 |
| `logo` | A picture for the middle of the code, in any form `image` takes. See [logos](#logos) |
| `logo_size` | The logo's width as a share of the code's, from 0.05 to 0.25. The default is 0.2 |
| `label` | A line of text under the code, such as `Scan to pay`. Printable ASCII only. It adds to the height, so give `width` or `size` rather than `height` |
| `version` | The QR version, from 1 to 40, for codes of one size across a loop. A value that doesn't fit is refused |

The sample invoice already has a QR code in it, so you can see one without changing anything:

```bash test
curl -F template=@invoice.docx -F "data=<invoice.json" \
  http://localhost:9027/v1/render -o out.docx
unzip -l out.docx | grep -o 'word/media/rich1.png'
```

This one is a retail code with its digits under the bars, rotated and in colour:

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

A bad value fails with a `422` that names the problem:

```bash test=fail
mkdir t && cd t && unzip -q ../invoice.docx
perl -pi -e 's/Billed to/{{ "5901234123458" | barcode("ean13") }}/' \
  word/document.xml
zip -qr ../tagged.docx . && cd ..

curl -F template=@tagged.docx -F "data=<invoice.json" \
  http://localhost:9027/v1/render -o out.docx
```

### QR code styles

`style`, `eye` and `eye_center` change how a QR code looks without changing what it holds. These are dots with round corners, then rounded modules in one colour with a label under them:

```bash test
mkdir t && cd t && unzip -q ../invoice.docx
export TAG='{{ url | barcode("qr", style="dots", eye="circle") }}'
TAG+=' {{ url | barcode("qr", style="rounded", eye="rounded",'
TAG+=' color="#c2185b", label="Scan to pay", width=120) }}'
perl -pi -e 's/Billed to/$ENV{TAG}/' word/document.xml
zip -qr ../tagged.docx . && cd ..

jq '. + {url: "https://example.com/pay/2041"}' invoice.json > with-url.json
curl -F template=@tagged.docx -F "data=<with-url.json" \
  http://localhost:9027/v1/render -o out.docx
unzip -l out.docx | grep -c 'word/media/rich[0-9]*\.svg' | grep -x 3
```

The corner squares are what a scanner looks for first, so they keep their place and proportions whatever their shape. Timing and alignment patterns take the module style, as other generators draw them.

Every combination of `style`, `eye` and `eye_center` reads back with ZXing. One pairing is harder for older decoders: a `circle` frame around a `square` or `rounded` middle. ZBar misses some of those at error correction `H`, which is why `eye_center` follows `eye` unless you set it.

### Colours a scanner can read

A scanner tells bars from background by how light they are, not by their hue. Every colour is checked against the background with the WCAG contrast ratio, and anything below 3:1 is refused. That's WCAG's minimum for graphical objects:

```bash test=fail
mkdir t && cd t && unzip -q ../invoice.docx
perl -pi -e 's/Billed to/{{ "x" | barcode("qr", color="#ccc") }}/' \
  word/document.xml
zip -qr ../tagged.docx . && cd ..

curl -F template=@tagged.docx -F "data=<invoice.json" \
  http://localhost:9027/v1/render -o out.docx
```

The error reads `color #cccccc on background #ffffff has a contrast of 1.6:1; scanners need at least 3:1`. The check covers `color`, `eye_color`, `eye_center_color` and the end of a gradient, for every symbology. A `transparent` background is the page's, which the service can't see, so nothing is checked then.

Bars lighter than their background are refused too, unless you pass `invert=true`. Phone cameras read light-on-dark QR codes, but many hand-held scanners and some apps don't. With `invert=true` and no colours, the code is white on black.

### Logos

`logo` puts a picture in the middle of a QR code. The modules under it are removed, not painted over, with a module of margin around the picture, and error correction restores what's missing. With a logo the default `ec` is `H`, and `L` is refused.

```text
{{ url | barcode("qr", logo=brand.mark, style="rounded", color="#2a7a6f") }}
```

How much a logo may hide depends on the error correction: at most 7.5% of the data modules at `M`, 12.5% at `Q` and 15% at `H`, which is half of what each level restores. The hole also stays clear of the corner squares and the timing patterns. If a logo doesn't fit, the service picks a larger version until it does, so a short value makes a denser code. With `version` set, it says what to change instead: `a logo 25% wide hides too much of this version 1 code at error correction H`.

PNG, JPEG, GIF, WebP and SVG logos work, from the data or from an [asset](/docs/guides/assets/#use-one-in-an-office-template), as `logo=asset("brand/mark.svg")`. An SVG is cleaned as for `image` and stays a vector in the code.

With a PNG, JPEG, GIF or WebP logo, the code's curves are written as short straight lines. Excel flattens the curves of any SVG that holds a raster picture so coarsely that rounded modules and round eyes stop scanning. Lines scan, though in Excel a small code's rounded corners look cut off. An SVG logo has no such problem, so use one if your documents open in Excel. A logo holds at most 2 MB, since a copy goes into each code that uses it, and at most 16 megapixels, since it is decoded to draw the PNG.

### GS1 Digital Link and GS1 QR codes

A GS1 Digital Link is a web address that carries a GTIN and its batch, serial or expiry, so one code works for a phone and for a till. [`qrdata("gs1")`](/docs/templates/filters/#qrdata) builds the address from an element string, and `barcode("qr")` draws it:

```text
{{ "(01)09506000134352(10)LOT7(17)261231" | qrdata("gs1") | barcode("qr") }}
```

That gives `https://id.gs1.org/01/09506000134352/10/LOT7?17=261231`. `gs1-qr` is the other way to put GS1 data in a QR code: the element string itself, with FNC1, for scanners in warehouses and hospitals rather than phones.

### Swiss QR-bill

`swissqr` draws the QR code of a Swiss QR-bill payment part from the payload [`qrdata("swiss")`](/docs/templates/filters/#qrdata) builds. The Swiss Implementation Guidelines fix its look, so it takes no colour, size, style or error correction. It's 46 mm wide with a quiet zone of at least 5 mm, at error correction `M`, with the Swiss cross in the middle and the modules under the cross removed.

```text
{{ bill | qrdata("swiss") | barcode("swissqr") }}
```

ZXing and rxing read it, and so do banking apps. ZBar finds nothing, because it takes the cross's black square for part of the symbol.

### How the codes are drawn

A barcode is an SVG, with a PNG of the same drawing for readers that don't show SVG. They're stored the way an SVG image is: DOCX, XLSX and PPTX show the PNG and carry the SVG as Office's SVG extension, and ODF frames list the SVG first and the PNG after it. Office and LibreOffice draw the SVG, so a code stays sharp at any print size, and the PDFs Word, Excel and LibreOffice make have it as vectors. Curves are cubic Béziers rather than SVG arcs, which every renderer draws the same way.

The PNG has a whole number of pixels per module. Square codes in one colour are drawn without anti-aliasing, so their edges stay sharp. Dots, rounded shapes, gradients and logos are anti-aliased.

A QR code whose modules would print smaller than 0.25 mm is refused, with the width it needs: `its modules would print 0.17 mm wide, and phones need at least 0.25 mm: make it at least 30 points wide, or the value shorter`. On packaging, GS1 asks for at least 0.396 mm. A rule of thumb for distance is a code a tenth as wide as the distance it's read from.

A `quiet_zone` below 4 is allowed for QR codes, for a code inside a frame you draw yourself, but the frame then has to leave the margin clear.

### Which decoders read them

Every symbology decodes to its exact value with rxing, a port of ZXing. That holds for the PNG the service makes, with and without text, colours, a wider quiet zone and rotation, and for LibreOffice's PDF of the document at 300 dpi. ZBar reads the same PNGs for every linear code and QR, but it has no Data Matrix or Aztec reader. EAN and UPC codes come back with the check digit the service adds. Codabar comes back without its start and stop letters from rxing, and with them from ZBar.

Every style, a logo of each kind, a label, gradients, the Swiss QR-bill code and the linear codes were checked in Word, PowerPoint and Excel 16.115 for Mac. Each draws them from the SVG, and every code scans from the PDF each one exports. Word shows the alt text.

QR styles, colours, gradients, logos and labels are read back with rxing for every combination of `style`, `eye`, `eye_center`, `module_scale` and error correction, and from LibreOffice's PDF. ZXing reads them from the pictures and from the PDF at 150 and 300 dpi, and ZBar reads them all apart from the exceptions above.

Text that isn't ASCII is written as UTF-8 with an ECI header that says so, which scanners need to show it right.

At 100 dpi, a long Code 128 or Code 39 at the default size is too fine to read, so give it a larger `size` for screens. Try your codes on the scanner you'll use.

## link

```text
{{ url | link }}
{{ url | link("Pay now") }}
```

`link` takes one optional argument, the text to show. Without it, the link shows its address with `mailto:` removed.

Only `http`, `https` and `mailto` links are allowed. `javascript:`, `file:` and relative addresses like `/pay` are refused with an error that names the problem, and so is a link inside a hyperlink that's already in the template.

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

A colour is `#rgb` or `#rrggbb` and nothing else, so `red` is refused. Give a colour, a `background=`, or both. Either can come from data.

The colour applies to the text. `background` fills whatever `scope` names, which is the text itself by default:

| `scope` | The colour | The background |
|---|---|---|
| `text` (default) | The printed value | Shading behind the printed value. In XLSX and ODS, the whole cell |
| `paragraph` | Every run of the paragraph that holds the tag | The paragraph's shading. PPTX and ODP paragraphs have none, so a `background` is refused there |
| `cell` | The text of the table cell that holds the tag | The cell's fill |
| `row` | The text of every cell in the row | The fill of every cell in the row. In XLSX the row's own format too, so empty cells match |
| `shape` | The text of the shape | The shape's fill. PPTX and ODP only |

The printed value always stays where the tag is. A colour from a scope doesn't change runs that another rich value made, like a link.

Sheets have no runs of text here, so in XLSX and ODS `text` and `cell` both colour the whole cell, and the last `color` in a cell wins. They have no paragraphs or shapes either, so asking for those is refused with `XLSX has no paragraphs to colour; use scope="cell" or scope="row"`. A `cell` or `row` scope outside a table is refused where the tag is, with `scope="cell" needs the tag inside a table cell`.

```bash test
mkdir t && cd t && unzip -q ../invoice.docx
perl -pi -e 's/Billed to/{{ number | color("#c00") }}/' word/document.xml
zip -qr ../tagged.docx . && cd ..

curl -F template=@tagged.docx -F "data=<invoice.json" \
  http://localhost:9027/v1/render -o out.docx
unzip -p out.docx word/document.xml | grep -o '<w:color w:val="CC0000"/>'
```

This shades a paragraph:

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

`html` takes no arguments, and reads this subset of HTML:

| Kind | Tags |
|---|---|
| Blocks | `p`, `h1` to `h6`, `ul`, `ol`, `li`, `table`, `tr`, `td`, `th` |
| Inline | `b`, `strong`, `i`, `em`, `u`, `s`, `del`, `strike`, `sub`, `sup`, `br` |
| Links | `a href="..."`, with the same rules as `link` |
| Colour | `span style="color:#hex"` |

Everything else follows these rules:

- `script` and `style` are removed along with their content.
- Other tags are dropped but their text is kept, so `<div>Hi</div>` gives a paragraph holding `Hi` and `<img src="x">` gives nothing.
- No attribute is copied except a checked `href` and colour, so a `span` with any other style loses the style.
- An `a` with a scheme that `link` would refuse is an error. Apart from that, broken HTML is closed as well as it can be and never refused.

What you get depends on the format:

| Format | Result |
|---|---|
| DOCX | Native paragraphs and tables. The tag's paragraph is replaced by generated ones with its properties, and text around the tag stays in paragraphs of its own. Lists are indents with a literal bullet or number. Headings are bold and sized |
| ODT | Native paragraphs, lists and tables, with the same handling of text around the tag |
| PPTX, XLSX, ODS, ODP | Plain text, with a line break per paragraph, item or row. Bullets and numbers stay, and table cells are separated by tabs. XLSX also sets wrap text on the cell |

In DOCX the tag has to sit directly in a paragraph, not inside a hyperlink or content control.

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

Values have to be numbers. A `null` or missing value is a gap, but a key with no value in any row is an error, because that's what a misspelt key looks like: `no row has a value for "nope"`. A string like `"12"` is refused, and so are an empty list and a row with no `x` key.

In DOCX and PPTX the chart carries its values inline and has no embedded workbook, so it renders but you can't edit its data in Word or PowerPoint. In PPTX it goes at the position of the shape that held the tag. A placeholder that PowerPoint hasn't moved has no position of its own, so the chart takes the one its layout or master gives it.

In XLSX the chart is anchored at the tag's cell, and a hidden sheet named `RichChartData` holds the values.

In ODT, ODS and ODP the chart is an embedded chart object (`Object N/` in the package), the way LibreOffice writes one. It carries its values in its own table and doesn't read a sheet, so in ODS it doesn't follow the cells around it. LibreOffice opens it as a chart whose data can be edited. In ODT it sits in the line where the tag is, in ODS it's a frame on the tag's cell, and in ODP it goes at the position of the shape that held the tag. Two charts or pictures for the same cell or shape are stacked, the second below the first.

Every format draws the series, or a pie's slices, in the same colours in turn: blue, orange, grey, gold, light blue and green, which are Office's default accents. DOCX, XLSX and PPTX take them from the document's theme, so a template with a different theme shows its own colours there. ODF charts have the colours written in.

## pagebreak

```text
{{ "" | pagebreak }}
```

`pagebreak` ignores its value and takes no arguments. It works in DOCX and ODT, and other formats refuse it with `page breaks exist only in DOCX and ODT, not PPTX`.

In a DOCX the break goes where the tag is. In an ODT, a tag with nothing after it breaks after its paragraph, and otherwise the text after it starts a new paragraph on the new page.

It's useful between loop iterations (see [paragraph loops](/docs/templates/office/#paragraphs)).

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

## Alt text

`alt` gives an image or a barcode the description a screen reader announces. It's plain text of up to 1000 characters.

```text
{{ logo | image(alt="Mawingu Coffee") }}
{{ url | barcode("qr", alt="Pay this invoice online") }}
```

A barcode without `alt` is described by its kind, such as `QR code` or `EAN-13 barcode`, and never by its value, which can be a Wi-Fi password or a bank account. An image without `alt` has none. In DOCX, XLSX and PPTX it's the picture's `descr`; in ODF it's the frame's `svg:desc`.

In a placeholder picture, `alt` replaces the alt text that held the tag. A barcode's default fills it only if the placeholder kept no text of its own beside the tag.

## Limits

| What | Limit |
|---|---|
| One image | 20 MB |
| One SVG image | 5 MB, 200,000 nodes nested at most 256 deep, 1,000,000 elements once references are expanded, 19200 pixels a side |
| A picture inside an SVG | 16 megapixels |
| One WebP image | 25 megapixels, 16384 pixels a side |
| Images and barcodes in one render | 200 |
| Barcode value | 4 KB, and 4096 modules when linear |
| Barcode PNG | 4 megapixels. The module gets smaller and the printed size doesn't |
| Barcode logo | 2 MB and 16 megapixels |
| Alt text | 1000 characters |
| Charts in one render | 200 |
| Values in one chart | 1,000 |
| One `html` value | 1 MB, nested at most 64 deep |
| One HTML table | 256 columns, 10,000 rows, 50,000 cells counting padded ones |
| All HTML tables in one render | 100,000 cells |
| Rich values in one render | 100,000 |
