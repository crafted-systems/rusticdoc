---
title: rusticdoc and Carbone
seoTitle: "rusticdoc vs Carbone: Office templates from JSON, and moving Carbone templates"
description: How rusticdoc compares with Carbone for generating reports from Office templates and JSON, with a syntax mapping for moving Carbone templates.
sidebar:
  order: 2
---

Carbone and rusticdoc both fill documents made in Word, LibreOffice or other editors with JSON data, and convert the result to PDF. They differ in template syntax, licensing, how they're offered, and what happens when data is missing.

- **Carbone** is a report generator. You write tags like `{d.companyName}` into a template, and Carbone injects JSON data, then converts the result with LibreOffice or another converter. It's offered as a hosted service, Carbone Cloud, as a self-hosted Docker image or binary, Carbone On-Premise, and as a Node.js library.
- **rusticdoc** is a self-hosted document service. It fills Office, text, HTML and PDF form templates from JSON with a Jinja-style syntax, converts the result with LibreOffice or Fulgur, and finishes it with PDF operations.

## At a glance

| | rusticdoc | Carbone |
|---|---|---|
| What it is | A document service that fills templates from JSON, converts them and runs PDF operations | A report generator that injects JSON into templates and converts the result |
| Licence | Apache 2.0, one edition | The Node.js library is source-available under the Carbone Code License. On-Premise runs under the Carbone On-Premise License, and its Enterprise features need a paid licence. Carbone Cloud has paid plans and a free sandbox |
| How you run it | A container image or a CLI, on your own infrastructure | Carbone Cloud, a hosted API. Carbone On-Premise, a Docker image or binary, which runs Community features without a licence. The Community Edition as a Node.js library, one major version behind |
| Templates it fills | DOCX, ODT, XLSX, ODS, PPTX, ODP, PDF forms, and text formats such as `.html`, `.md`, `.csv`, `.xml`, `.json` and `.svg` | DOCX, XLSX, PPTX, ODT, ODS, ODP, ODG, PDF forms, HTML, Markdown, CSV, TXT, XML, SVG and IDML |
| Template syntax | Jinja-style, evaluated by minijinja: `{{ customer.name }}`, `{% for %}`, `{% if %}`, filters with `\|` | Single tags with a data path and formatters chained with `:`, such as `{d.name:upperCase}`. Loops are marked with `[i]` and `[i+1]` |
| Missing values | Stop the render with an error, unless the template gives a default | Resolve to empty |
| Conversion engines | LibreOffice for Office files, Fulgur for HTML | LibreOffice by default. Carbone ICE for DOCX and ODT to PDF, Chromium for HTML and SVG, and OnlyOffice for Office files |
| Images, charts, barcodes, HTML | In Office templates, in every deployment. Images come as base64 or data URLs only, and are never fetched | Enterprise features. Images come from a public URL or a base64 data URI |
| PDF operations | Merge, split, overlay, rotate, watermark, stamp, flatten, optimize, encrypt, metadata, bookmarks, embedded files, Factur-X, PDF/A, PDF/UA, image and certificate signatures | PDF export options for PDF/A, encryption, permissions and watermark. Enterprise features append PDFs, attach files and place signature fields for an e-signature provider |
| Hosting | Self-hosted only | Hosted or self-hosted |

## Where they differ

### Syntax

Carbone puts everything in one tag. A loop is two consecutive items marked `[i]` and `[i+1]`, and Carbone works out what to repeat. Conditions and formatting are formatters chained with `:`.

rusticdoc uses statements that open and close, like `{% for %}` and `{% endfor %}`. Where you put the two tags decides what repeats: text, a table row, paragraphs, sheet rows or slides. See [Office formats](/docs/templates/office/#what-a-block-repeats). Filters follow a value with `|`, as in `{{ total | currency("EUR") }}`.

### Missing values

Carbone resolves a tag with no data to empty. In rusticdoc, a missing value stops the render with a `422` that names the tag, so a typo can't reach a reader as a blank. If a field is optional, give it a default:

```text
{{ nickname | default("friend") }}
{% if vip is defined %}VIP{% endif %}
```

For data that's sparse by design, send `undefinedAsEmpty=true` with the request. See [missing values are errors](/docs/templates/syntax/#missing-values-are-errors). [Validate a template](/docs/templates/validate/) lists every problem in a template against sample data, which helps when you port one.

### Editions and features

Carbone splits its features between editions. Substitutions, loops, formatters, conditions, translations and simple maths are Community features. Pictures, charts, barcodes, colours, HTML, hyperlinks, aggregations and file operations are Enterprise features, available through Carbone Cloud or a licensed On-Premise install, and not in the embedded Node.js library.

rusticdoc has one edition, so images, barcodes, charts, colours, links and HTML work in any deployment. They work in Office templates, not in text templates. See [rich content](/docs/templates/rich-content/).

### Images and the network

Carbone can download a picture from a URL in your data. rusticdoc never fetches an image. The `image` filter takes base64, a data URL or an object holding the bytes, so your code fetches or reads the image first. HTML is laid out by Fulgur with no network access, and LibreOffice fetches nothing either. See the [security overview](/docs/project/security-overview/).

### Converters and hosting

Carbone offers several converters, including Chromium for HTML and Carbone ICE, which writes PDFs from DOCX and ODT without an office suite. rusticdoc has LibreOffice and Fulgur, and no browser.

Carbone Cloud is a hosted API. rusticdoc has no hosted service: every call goes to your own deployment.

### Spreadsheets and presentations

Carbone's documentation says XLSX templates don't yet support creating sheets and PPTX templates don't yet support creating slides, and suggests ODS and ODP templates instead. rusticdoc repeats sheets in XLSX and ODS with `{% sheet for %}`, and slides in PPTX and ODP. Its own limits are in [known limits](/docs/project/known-limits/#office-output), such as a placeholder picture in an XLSX not seeing loop variables, and column loops working in XLSX only.

## Moving templates from Carbone

Templates can't be moved as they are, because the tags are different. The layout, styles, headers, footers and placeholder pictures carry over, since both read ordinary Office files. In practice you replace each tag.

The data usually stays the same, with one change: rusticdoc's `data` must be a JSON object. If your Carbone data is an array at the top, `d[i]`, wrap it, as in `{"items": [...]}`.

| What you want | Carbone | rusticdoc |
|---|---|---|
| A field | `{d.firstname}` | `{{ firstname }}` |
| A nested field | `{d.type.name}` | `{{ type.name }}` |
| An item of a list | `{d.movies[1].title}` | `{{ movies[1].title }}` |
| A parent object's field | `{d.movie.sub..name}` | The full path, `{{ movie.name }}`, or inside a loop the outer loop's variable |
| Data from outside the main object | `{c.field}`, the complement object | `{{ ctx.field }}`, from the `ctx` field |
| Repeat table rows | `{d.cars[i].brand}` in one row, `{d.cars[i+1].brand}` in the next | `{% for car in cars %}{{ car.brand }}` in the row's first cell, `{% endfor %}` in its last |
| Repeat paragraphs | `{d.brands[i].name}` ... `{d.brands[i+1]}` | `{% for b in brands %}{{ b.name }}` in the first paragraph, `{% endfor %}` in the last |
| Filter a loop | `{d.people[i+1, age > 19].name}` | `{% for p in people if p.age > 19 %}` |
| Show text on a condition | `{d.paid:ifEQ(true):show('Paid'):elseShow('Due')}` | `{{ "Paid" if paid else "Due" }}` |
| Choose between several values | `{d.v:ifEQ(1):show(A):ifEQ(2):show(B):elseShow(C)}` | `{% if v == 1 %}A{% elif v == 2 %}B{% else %}C{% endif %}` |
| Show a block | `{d.toBuy:ifEQ(true):showBegin}` ... `{d.toBuy:showEnd}` | `{% if toBuy %}` ... `{% endif %}` |
| Hide a block | `{d.x:ifEQ(true):hideBegin}` ... `{d.x:hideEnd}` | `{% if not x %}` ... `{% endif %}` |
| Test for empty | `{d.notes:ifEM:show('None')}` | `{{ "None" if notes is empty }}` |
| Test membership | `:ifIN(value)` | `{% if value in list %}` |
| Remove a paragraph or rows | `{d.text:ifEM:drop(p, 3)}`, `drop(row)` | `{{ drop("paragraph", 3) if text is empty }}`, `drop("row")` |
| Remove a column, table, picture, chart or shape | `drop(col)`, `drop(table)`, `drop(img)`, `drop(chart)`, `drop(shape)` | `drop("column")`, `drop("table")`, `drop("picture")`, `drop("chart")`, `drop("shape")`. See [remove an element](/docs/templates/office/#remove-an-element) |
| Upper and lower case | `:upperCase`, `:lowerCase`, `:ucWords` | `upper`, `lower`, `title` |
| First letter upper case | `:ucFirst` | `capitalize`, which also lowers the rest |
| Shorten or pad text | `:ellipsis(20)`, `:padl(6, '0')`, `:padr(6)` | `truncate(20)`, which counts the `…` in the length, `pad_left(6, "0")`, `pad_right(6)` |
| Replace, prepend, append | `:replace('a', 'b')`, `:prepend('x')`, `:append('x')` | `replace("a", "b")`, `"x" ~ value`, `value ~ "x"` |
| Remove accents | `:unaccent` | `unaccent` |
| Format a number | `:formatN(2)` | `number(2)` |
| Format money | `:formatC()` with a currency from the options | `currency("EUR")`, or `currency` with the request's `currency` |
| Spell a number | `:toWord()` | `words` |
| Format a date | `:formatD('LL')` | `date("%d %B %Y")`. rusticdoc uses `%` codes, not Carbone's patterns. See [date](/docs/templates/filters/#date) |
| Date arithmetic | `:addD`, `:subD`, `:startOfD`, `:endOfD`, `:diffD` | `date_add`, `date_sub`, `start_of`, `end_of`, `date_diff` |
| Arithmetic | `{d.qty:mul(.price)}`, `:add`, `:sub`, `:div`, `:mod` | `{{ qty * price }}`, `+`, `-`, `/`, `%` |
| Round, absolute value | `:round(2)`, `:abs`, `:ceil`, `:floor` | `round(2)`, `abs`, `ceil`, `floor` |
| Sum a list | `{d.cars[].qty:aggSum}` | `{{ cars \| sum(attribute="qty") }}` |
| Translate | `{t(meeting)}`, `:t` | `{{ "meeting" \| t }}`, `{{ label \| t }}`. See [translations](/docs/templates/translations/) |
| Language and time zone | The `lang` and `timezone` options | The `lang` and `timezone` request fields, or `{% settings %}` in the template |
| Picture | `{d.photo}` in a placeholder picture's alt text | `{{ photo \| image }}` in the alt text, or `{{ photo \| image(fit="width") }}` to match Carbone's default `fillWidth` |
| Picture fit | `:imageFit(contain)`, `:imageFit(fill)` | `image(fit="contain")`, `image(fit="stretch")` |
| Barcode | `{d.number:barcode(qrcode)}` in a picture's alt text | `{{ number \| barcode("qr") }}`, at the tag or in a placeholder picture's alt text |
| HTML | `{d.description:html}` | `{{ description \| html }}`, native in DOCX and ODT, plain text elsewhere |

Some things need more than a tag change:

- **Image URLs.** Replace URLs in your data with base64 or data URLs.
- **Charts.** Carbone's `:chart` takes chart options for an image. rusticdoc's `chart` filter builds a native chart from a list of objects, so rebuild the data. See [chart](/docs/templates/rich-content/#chart).
- **Paragraph breaks around a block.** A paragraph that holds only `{% for %}` or `{% endfor %}` stays in each copy as an empty paragraph. Put block tags in paragraphs that also hold text, as [Office formats](/docs/templates/office/#paragraphs) shows.
- **Aggregations.** Carbone's `:aggSum` and the other aggregators are Enterprise features. In rusticdoc, `sum`, `avg`, `min`, `max` and the [calculation helpers](/docs/templates/calculations/) work in every template.

After porting, check each template against sample data:

```bash
curl -F template=@invoice.docx -F "data=<invoice.json" \
  http://localhost:9027/v1/templates/validate
```

## When to pick which

Carbone is the better fit when:

- You want a hosted API, or a support contract from the vendor.
- Your templates are in formats rusticdoc doesn't fill, such as ODG or IDML.
- You need Chromium to convert HTML, or a converter that doesn't need an office suite.
- Your data holds image URLs that the service should download.
- You have many Carbone templates working today, and porting them isn't worth it.

rusticdoc is the better fit when:

- You want an Apache 2.0 licence with every feature in one edition.
- You want a missing value to fail the render instead of printing a blank.
- You want documents that never reach the network while they're filled or converted.
- You want to fill, convert, sign, archive as PDF/A or embed Factur-X in one request.
- You prefer a Jinja-style syntax with loops and conditions that open and close.

## Related pages

- [Template syntax](/docs/templates/syntax/)
- [Filters](/docs/templates/filters/)
- [Conditions](/docs/templates/conditions/)
- [Office formats](/docs/templates/office/)
- [Rich content](/docs/templates/rich-content/)
- [Known limits](/docs/project/known-limits/)

## Sources

Checked in November 2026. Carbone is a trademark of its owner.

- [Carbone repository and README](https://github.com/carboneio/carbone): editions, Community Edition one major version behind, converters.
- [Carbone Code License](https://github.com/carboneio/carbone/blob/master/LICENSE.md): source-available terms of the Community Edition.
- [Getting started](https://carbone.io/documentation/design/overview/getting-started.html): tag types `{d.}`, `{c.}`, `{t( )}`, formatters.
- [Template feature](https://carbone.io/documentation/design/overview/template-feature.html): template formats, Community and Enterprise features, XLSX and PPTX limitations.
- [The basics](https://carbone.io/documentation/design/substitutions/the-basics.html): fields, nested objects, arrays, parent properties.
- [Loop over array](https://carbone.io/documentation/design/repetitions/with-arrays.html) and [filtering](https://carbone.io/documentation/design/repetitions/filtering.html): `[i]`, `[i+1]` and filters.
- [Conditions](https://carbone.io/documentation/design/conditions/overview.html), [inline conditions](https://carbone.io/documentation/design/conditions/inline-conditions.html), [conditional blocks](https://carbone.io/documentation/design/conditions/conditional-blocks.html) and [smart conditions](https://carbone.io/documentation/design/conditions/smart-conditions.html).
- Formatters: [text](https://carbone.io/documentation/design/formatters/text.html), [number](https://carbone.io/documentation/design/formatters/number.html), [currency](https://carbone.io/documentation/design/formatters/currency.html), [date](https://carbone.io/documentation/design/formatters/date.html), [aggregation](https://carbone.io/documentation/design/computation/aggregation.html), [translations](https://carbone.io/documentation/design/advanced-features/translations-i18n.html).
- [Pictures](https://carbone.io/documentation/design/advanced-features/pictures.html), [barcodes](https://carbone.io/documentation/design/advanced-features/barcode.html), [HTML](https://carbone.io/documentation/design/advanced-features/html.html), [charts](https://carbone.io/documentation/design/advanced-features/charts.html), [file operations](https://carbone.io/documentation/design/advanced-features/file-operations.html), [signatures](https://carbone.io/documentation/design/advanced-features/signatures.html).
- [Generate reports](https://carbone.io/documentation/developer/http-api/generate-reports.html): converters, PDF options, empty data resolving to empty.
- [How Carbone works](https://carbone.io/documentation/developer/under-the-hood/overview.html) and [On-Premise](https://carbone.io/documentation/developer/on-premise-installation/introduction.html): converters, Docker image and binary, Community features without a licence.
- [Licensing](https://carbone.io/documentation/developer/on-premise-installation/licensing.html): Community and Enterprise editions of On-Premise, the On-Premise License.
- [HTTP API introduction](https://carbone.io/documentation/developer/http-api/introduction.html): Carbone Cloud, test and production tokens, the free Sandbox plan.
- [Embedding in Node.js](https://carbone.io/documentation/developer/embedding/embedding-in-node.html): the library is the Community Edition only.
