---
title: rusticdoc and Docmosis
seoTitle: "rusticdoc vs Docmosis: document generation from templates, and moving templates"
description: How rusticdoc compares with Docmosis for generating PDF and Word documents from templates and JSON, with a syntax mapping for moving Docmosis templates.
sidebar:
  order: 4
---

Docmosis and rusticdoc both generate documents from templates made in Word or LibreOffice, filled with data your application sends over a REST API, and both convert with LibreOffice. They differ in licensing, hosting, template syntax and the formats they fill.

- **Docmosis** is commercial document generation software. Templates are Microsoft Word or LibreOffice Writer documents with fields like `<<name>>`. It comes as Docmosis Cloud, a hosted service, Tornado, a self-hosted engine, and Docmosis-Java, a Java library.
- **rusticdoc** is a self-hosted document service. It fills Office, text, HTML and PDF form templates from JSON with a Jinja-style syntax, converts the result with LibreOffice or Fulgur, and finishes it with PDF operations.

## At a glance

| | rusticdoc | Docmosis |
|---|---|---|
| What it is | A document service that fills templates from JSON, converts them and runs PDF operations | Commercial document generation from Word and Writer templates, through a REST API or a Java library |
| Licence | Apache 2.0 | Commercial. Cloud has usage-based monthly plans. Tornado and Docmosis-Java are sold as perpetual licences with 12 months of support and updates |
| How you run it | A container image or a CLI, on your own infrastructure | Docmosis Cloud, hosted on AWS. Tornado, on Linux, Windows or Docker, which needs Java 11 or later, LibreOffice 7 or later and a licence key. Docmosis-Java, inside a Java application |
| Templates it fills | DOCX, ODT, XLSX, ODS, PPTX, ODP, PDF forms, and text formats such as `.html`, `.txt`, `.csv`, `.xml` and `.json` | Microsoft Word and LibreOffice Writer documents |
| Data | JSON | JSON or XML |
| Template syntax | Jinja-style, evaluated by minijinja: `{{ customer.name }}`, `{% for %}`, `{% if %}` | Fields in double angle brackets: `<<name>>`, `<<rs_items>>...<<es_items>>`, `<<cs_paid>>...<<es_paid>>`, expressions in `<<{...}>>` |
| Missing values | Stop the render with an error, unless the template gives a default | A field with no matching data is removed |
| Conversion engine | LibreOffice for Office files, Fulgur for HTML | LibreOffice |
| Output formats | The template's own format, or with a `convert` step PDF, DOCX, XLSX, PPTX, ODT, ODS, ODP, HTML or TXT | PDF, DOCX, ODT and TXT, among others |
| PDF features | Merge, split, overlay, rotate, watermark, stamp, flatten, optimize, encrypt, metadata, bookmarks, embedded files, Factur-X, PDF/A, PDF/UA, form filling, image and certificate signatures | Set when rendering: PDF/A, PDF/UA, tagged PDF, watermark, permissions password, and ZUGFeRD and Factur-X e-invoices. PDF forms from Writer templates |
| Hosting | Self-hosted only | Hosted or self-hosted |

## Where they differ

### Licence and hosting

Docmosis is licensed software. Cloud is billed by usage, and Tornado needs a licence key to start, with a 30-day trial. Docmosis offers a hosted service, so you can generate documents without running anything.

rusticdoc is open source under Apache 2.0, with nothing to license. There's no hosted service, so you run the container yourself. See [install](/docs/start/install/).

### Formats

Docmosis templates are word processing documents, from Word or Writer. rusticdoc also fills spreadsheets (XLSX, ODS), presentations (PPTX, ODP), PDF forms and text formats. See [Office formats](/docs/templates/office/) and [PDF templates](/docs/templates/pdf-templates/).

Docmosis can merge templates into each other with `<<ref:sub.docx>>`, and combine several templates into one PDF. A rusticdoc template is a single file, but you can merge the PDFs of several renders with a `merge` step or `/v1/pdf/merge`.

### Missing values and errors

Docmosis removes a field that has no matching data. When a template has an error, its dev mode writes the error into the document and its production mode returns the error.

rusticdoc stops the render with a `422` when a value is missing, and names the tag. For optional fields, use `default` or `is defined`, or send `undefinedAsEmpty=true`. See [missing values are errors](/docs/templates/syntax/#missing-values-are-errors). [Validate a template](/docs/templates/validate/) lists every problem in a template against sample data.

### Images

Tornado takes an image as base64 in the data, from the templates location as a stock image, or from a URL. rusticdoc only takes images in the data, as base64 or a data URL, and never fetches one. See [image](/docs/templates/rich-content/#image).

### PDF processing

Docmosis sets PDF options as it renders, such as PDF/A, watermarks and e-invoice attachments. rusticdoc has those as steps you order after the render, and as `/v1/pdf/*` routes for PDFs you already have, including certificate signatures up to PAdES B-LTA. See [the pipeline](/docs/guides/pipeline/) and [PDF operations](/docs/guides/pdf-operations/).

## Moving templates from Docmosis

The templates' layout carries over, since both read ordinary Word and Writer files. The fields need rewriting.

| What you want | Docmosis | rusticdoc |
|---|---|---|
| A field | `<<name>>` | `{{ name }}` |
| A nested field | `<<hotel.floor>>` | `{{ hotel.floor }}` |
| An item of a list | `<<hotel[0]>>`, `<<hotel[L]>>` | `{{ hotel[0] }}`, `{{ hotel \| last }}` |
| An expression | `<<{amount * qty}>>` | `{{ amount * qty }}` |
| Join text | `<<{'Ms. ' + lastName}>>` | `{{ "Ms. " ~ lastName }}` |
| A default for a blank value | `<<{ifBlank(name, 'Not Specified')}>>` | `{{ name \| default("Not Specified", true) }}` |
| An optional paragraph | `<<op:name>>` | `{{ drop("paragraph") if name is empty }}{{ name \| default("") }}` |
| Repeat content | `<<rs_items>>` ... `<<es_items>>` | `{% for item in items %}` ... `{% endfor %}` |
| Repeat table rows | `<<rr_items>>` ... `<<er_items>>` | `{% for item in items %}` in the row's first cell, `{% endfor %}` in its last |
| Fields inside a loop | `<<name>>`, looked up in the current item | `{{ item.name }}` |
| The position in a loop | `<<$num>>`, `<<$idx>>`, `<<$size>>` | `{{ loop.index }}`, `{{ loop.index0 }}`, `{{ loop.length }}` |
| Filter a loop | `<<rs_persons:filter(ID<10)>>` | `{% for p in persons if p.ID < 10 %}` |
| Sort a loop | `<<rs_persons:sort(name)>>` | `{% for p in persons \| sort(attribute="name") %}` |
| Group a loop | `<<rs_animals:group(species)>>` with `<<$groupKey>>` and `<<rs_$groupItems>>` | `{% for g in animals \| groupby("species") %}{{ g.grouper }}`, with `g.list` for the items |
| A condition | `<<cs_paid>>` ... `<<else>>` ... `<<es_paid>>` | `{% if paid %}` ... `{% else %}` ... `{% endif %}` |
| A condition on an expression | `<<cs_{total > 1000}>>` | `{% if total > 1000 %}` |
| A conditional table row | `<<cr_name>>` ... `<<er_name>>` | `{% if name %}` and `{% endif %}` in the row's cells |
| A conditional column | `<<cc_name>>` | `{{ drop("column") if not name }}` in a cell of the column |
| A variable | `<<$total={amount * qty}>>` | `{% set total = amount * qty %}` |
| Format a number | `<<{numFormat(total, '#,##0.00')}>>` | `{{ total \| number(2) }}`, or `currency` for money |
| Format a date | `<<{dateFormat(d, 'dd/MM/yyyy')}>>` | `{{ d \| date("%d/%m/%Y") }}`. rusticdoc uses `%` codes |
| Change case | `<<{toUpperCase(name)}>>`, `<<{titleCase(name)}>>` | `{{ name \| upper }}`, `{{ name \| title }}` |
| HTML | `<<html:body>>` | `{{ body \| html }}`, native in DOCX and ODT |
| Hyperlink | `<<link:url>>` | `{{ url \| link }}`, or `{{ url \| link("Pay now") }}` for display text |
| Page break | `<<pageBreak>>` | `{{ "" \| pagebreak }}` |
| Image | A placeholder image bookmarked or named `img_logo` | A placeholder picture with `{{ logo \| image }}` in its alt text |
| Image scaled to fit or stretched | `imgfit_logo`, `imgstretch_logo` | `image(fit="contain")`, `image(fit="stretch")` |
| Image data | `"image:base64:..."` or a data URL | Base64 or a data URL, without the `image:base64:` prefix |
| Barcode | `<<barcode:label:code128>>` with a placeholder image | `{{ label \| barcode("code128") }}`, at the tag or in a placeholder picture's alt text |
| Another template | `<<ref:sub1.docx>>` | No equivalent. A template is a single file |
| A comment | `<<## ... ##>>` | `{# ... #}` |

After porting, check each template against sample data, since a field that used to vanish now stops the render:

```bash
curl -F template=@invoice.docx -F "data=<invoice.json" \
  http://localhost:9027/v1/templates/validate
```

## When to pick which

Docmosis is the better fit when:

- You want a hosted service, or vendor support under a commercial licence.
- Your data is XML, or your application embeds a Java library.
- Your templates rely on merging other templates with `<<ref:>>`.
- You need images fetched from URLs, or stock images kept with the templates.

rusticdoc is the better fit when:

- You want an open source licence and no licence key.
- You fill spreadsheets, presentations, PDF forms or text formats as well as Word documents.
- You want a missing value to fail the render instead of disappearing.
- You want PDF operations, such as merging, stamping and certificate signatures, in the same service.

## Related pages

- [Template syntax](/docs/templates/syntax/)
- [Filters](/docs/templates/filters/)
- [Conditions](/docs/templates/conditions/)
- [Office formats](/docs/templates/office/)
- [Rich content](/docs/templates/rich-content/)
- [Known limits](/docs/project/known-limits/)

## Sources

Checked in November 2026. Docmosis is a trademark of Docmosis Pty Ltd.

- [Docmosis llms.txt](https://www.docmosis.com/llms.txt): Cloud and Tornado, Word or Writer templates, JSON or XML data, output formats.
- [Product comparison](https://www.docmosis.com/document-generation-software/): Cloud, Tornado and Docmosis-Java, pricing models.
- [Cloud (DWS4) Template Guide](https://resources.docmosis.com/documentation/cloud-dws4-template-guide), October 2026: fields, repeating and conditional sections, expressions, functions, images, barcodes, missing fields, error handling, PDF forms.
- [Tornado 2.11.4 Installation Guide](https://resources.docmosis.com/documentation/tornado-v2-11-4-installation-guide): Java 11, LibreOffice 7, licence key, Docker.
- [Tornado 2.11.4 Web Services Guide](https://resources.docmosis.com/documentation/tornado-v2-11-4-web-services-guide): output formats, PDF options, e-invoices, image sources.
