---
title: rusticdoc and docxtemplater
seoTitle: "rusticdoc vs docxtemplater: Word templates from JSON, and moving templates"
description: How rusticdoc compares with docxtemplater for filling Word, PowerPoint and Excel templates from JSON, with a syntax mapping for moving templates.
sidebar:
  order: 3
---

docxtemplater and rusticdoc both fill Word, PowerPoint and Excel templates with data, using tags typed into the document. One is a library inside your application, the other a service beside it.

- **docxtemplater** is a JavaScript library that generates DOCX and PPTX files from templates, in Node.js or in the browser. Tags like `{name}` and `{#items}...{/items}` hold values, loops and conditions. Paid modules add images, HTML, charts, XLSX templates and more.
- **rusticdoc** is a self-hosted document service. It fills Office, text, HTML and PDF form templates from JSON with a Jinja-style syntax, converts the result with LibreOffice or Fulgur, and finishes it with PDF operations.

## At a glance

| | rusticdoc | docxtemplater |
|---|---|---|
| What it is | A document service that fills templates from JSON, converts them and runs PDF operations | A JavaScript library that fills DOCX and PPTX templates |
| Licence | Apache 2.0, one edition | The core is dual-licensed MIT or GPLv3. Modules are sold under a commercial licence |
| How you run it | A container image or a CLI. Your application calls it over HTTP | A package in your Node.js or browser application. The top commercial plans include a Docker image with an HTTP API |
| Templates it fills | DOCX, ODT, XLSX, ODS, PPTX, ODP, PDF forms, and text formats such as `.html`, `.txt`, `.csv`, `.xml` and `.json` | DOCX and PPTX. XLSX with the paid XLSX module |
| Template syntax | Jinja-style, evaluated by minijinja: `{{ customer.name }}`, `{% for %}`, `{% if %}`, built-in filters | Mustache-inspired: `{name}`, `{#items}...{/items}`, `{^items}`. Expressions and filters with the Angular parser, where you write the filters in JavaScript |
| Missing values | Stop the render with an error, unless the template gives a default | Print `undefined` by default. A `nullGetter` function changes that |
| Conversion to PDF | LibreOffice for Office files, Fulgur for HTML, in the same request | None. Its FAQ suggests converting with LibreOffice or another tool |
| Images, HTML, charts | Built-in filters in Office templates. Images come as base64 or data URLs | Paid modules. The image module calls a function you write to get each image's bytes |
| PDF operations | Merge, split, overlay, rotate, watermark, stamp, flatten, optimize, encrypt, metadata, bookmarks, embedded files, Factur-X, PDF/A, PDF/UA, image and certificate signatures | None |
| Hosting | Self-hosted only | Runs wherever your code runs |

## Where they differ

### Library or service

docxtemplater runs inside your application, so there's nothing to deploy and no network hop, and it works in a browser. It produces DOCX, PPTX or XLSX, and anything after that, like a PDF, is up to you.

rusticdoc runs beside your application as a container. Any language that can send a multipart `POST` can use it, and one request can fill a template, convert it and finish the PDF:

```bash
curl -F template=@invoice.docx -F "data=<invoice.json" -F steps=convert \
  http://localhost:9027/v1/render -o invoice.pdf
```

See [the pipeline](/docs/guides/pipeline/) and [PDF operations](/docs/guides/pdf-operations/).

### Expressions and filters

docxtemplater's default parser only looks up plain names, so `{user.name}` needs the Angular parser. With it, you get operators, and you write each filter as a JavaScript function.

rusticdoc evaluates expressions, nested lookups and filters out of the box, and has filters for numbers, money, dates, translations, text and lists. See [filters](/docs/templates/filters/) and [calculations](/docs/templates/calculations/). There's no way to add a filter of your own, so logic that the built-in filters and `{% macro %}` don't cover belongs in the data you send.

### Scope inside a loop

In docxtemplater, a section changes the scope. Inside `{#users}`, `{name}` means the current user's name, and a name not found there is looked up in the parent scope.

In rusticdoc, a loop names its variable, and you write it: `{% for user in users %}{{ user.name }}{% endfor %}`. Names outside the loop stay as they are, so `{{ company }}` still works inside it.

### Missing values

docxtemplater prints `undefined` for a missing value unless you set a `nullGetter`. rusticdoc stops the render with a `422` that names the tag, so a typo can't reach a reader. For optional fields, use `default` or `is defined`, or send `undefinedAsEmpty=true` for data that's sparse by design. See [missing values are errors](/docs/templates/syntax/#missing-values-are-errors).

### Paragraphs in loops

This catches templates ported from docxtemplater. With `paragraphLoop` on, docxtemplater drops the paragraphs that hold only `{#items}` and `{/items}`. rusticdoc keeps them, so each copy gets an empty paragraph. A loop that opens in one paragraph and closes in another repeats both paragraphs whole, which can leave an extra paragraph in a template written for docxtemplater.

To avoid it, put the tags in paragraphs that also hold text:

```text
{% for i in items %}Item {{ i.name }}
Some text about it.
Last line{% endfor %}
```

See [Office formats](/docs/templates/office/#paragraphs) and [known limits](/docs/project/known-limits/#office-output).

## Moving templates from docxtemplater

The layout of your templates carries over, since both read ordinary Office files. The tags need rewriting, and the data usually stays as it is.

| What you want | docxtemplater | rusticdoc |
|---|---|---|
| A field | `{name}` | `{{ name }}` |
| A nested field | `{user.name}`, with the Angular parser | `{{ user.name }}` |
| Repeat a block | `{#products}{name}, {price}{/products}` | `{% for p in products %}{{ p.name }}, {{ p.price }}{% endfor %}` |
| A list of plain values | `{#products}{.} {/products}` | `{% for p in products %}{{ p }} {% endfor %}` |
| Repeat table rows | `{#users}` in the first cell, `{/}` in the last | `{% for user in users %}` in the first cell, `{% endfor %}` in the last |
| Close a block | `{/products}` or `{/}` | `{% endfor %}` or `{% endif %}` |
| Show a block | `{#hasKitty}...{/hasKitty}` | `{% if hasKitty %}...{% endif %}` |
| Show a block when a value is false or empty | `{^repo}No repos{/repo}` | `{% if not repo %}No repos{% endif %}`, or `{% else %}` inside the loop |
| A condition on an expression | `{#users.length > 1}...{/}` | `{% if users \| length > 1 %}...{% endif %}` |
| Inline choice | `{paid ? "Paid" : "Due"}` | `{{ "Paid" if paid else "Due" }}` |
| And, or, not | `&&`, `\|\|`, `!` | `and`, `or`, `not` |
| Arithmetic | `{price * 1.2}` | `{{ price * 1.2 }}` |
| Set a value | `{total = price * qty}` | `{% set total = price * qty %}` |
| Filter a list | `{#users \| where:'age > 15'}`, with a filter you write | `{% for u in users if u.age > 15 %}` |
| Upper case | `{name \| toUpperCase}`, with a filter you write | `{{ name \| upper }}` |
| Money and dates | Filters you write | `{{ total \| currency("EUR") }}`, `{{ issued \| date("%d %B %Y") }}` |
| A missing value | `nullGetter` | `{{ x \| default("") }}`, or `undefinedAsEmpty=true` on the request |
| Newlines in a value | The `linebreaks` option | Nothing to set. A newline becomes a line break in DOCX and ODT |
| Image | `{%image}`, image module | `{{ logo \| image }}`, or a placeholder picture with the tag in its alt text. See [image](/docs/templates/rich-content/#image) |
| HTML | `{~html}` or `{~~html}`, HTML module | `{{ body \| html }}`, native in DOCX and ODT, plain text elsewhere |
| Chart | `{$chart}`, chart module | `{{ rows \| chart(type="bar", x="month", y=["sales"]) }}`. See [chart](/docs/templates/rich-content/#chart) |
| One slide per item | `{:users}`, slides module | `{% for u in users %}` and `{% endfor %}` in different shapes of the slide. See [slide loops](/docs/templates/office/#slide-loops) |
| Excel templates | XLSX module | Built in. See [XLSX](/docs/templates/office/#xlsx) |
| Raw XML | `{@rawXml}` | No equivalent. Use `html` for formatted text |
| Another document | `{:include doc}`, subtemplate module | No equivalent. A template is a single file |
| Different delimiters | `{=<% %>=}` | Not available. Print literal braces with `{{ "{{" }}` |

Some things need more than a tag change:

- **Images.** docxtemplater's image module gets bytes from a `getImage` function you write, which can read a file or fetch a URL. rusticdoc never fetches, so put the image in the data as base64 or a data URL.
- **Custom filters.** Move their logic into the data you send, or into the built-in [filters](/docs/templates/filters/) and [calculations](/docs/templates/calculations/).
- **Errors.** A template that worked because missing values printed as blanks will now fail. [Validate](/docs/templates/validate/) it against realistic data first:

```bash
curl -F template=@invoice.docx -F "data=<invoice.json" \
  http://localhost:9027/v1/templates/validate
```

## When to pick which

docxtemplater is the better fit when:

- You want to generate documents inside a JavaScript application, or in the browser, with nothing else to run.
- You only need DOCX, PPTX or XLSX output, not PDF.
- You need features from its modules that rusticdoc doesn't have, such as including other documents or footnotes.
- You want to write your own filters in JavaScript.

rusticdoc is the better fit when:

- Your application isn't written in JavaScript, or several services need documents.
- You need PDFs, and want conversion and PDF operations in the same service.
- You want images, HTML, charts and spreadsheet templates without paid modules.
- You also fill ODT, ODS, ODP or PDF form templates.
- You want a missing value to fail the render instead of printing `undefined`.

## Related pages

- [Template syntax](/docs/templates/syntax/)
- [Filters](/docs/templates/filters/)
- [Conditions](/docs/templates/conditions/)
- [Office formats](/docs/templates/office/)
- [Rich content](/docs/templates/rich-content/)
- [Known limits](/docs/project/known-limits/)

## Sources

Checked in November 2026. docxtemplater is a trademark of its owner.

- [docxtemplater repository and README](https://github.com/open-xml-templating/docxtemplater): what it is, the list of paid modules and their tags.
- [Licence file](https://github.com/open-xml-templating/docxtemplater/blob/master/LICENSE.md): dual MIT or GPLv3.
- [npm package](https://www.npmjs.com/package/docxtemplater): Node.js, the browser and the command line.
- [Types of tags](https://docxtemplater.com/docs/tag-types/): placeholders, loops, sections, inverted sections, raw XML, delimiters, `paragraphLoop`.
- [Configuration](https://docxtemplater.com/docs/configuration/): the default parser, `nullGetter` printing `undefined`, `linebreaks`, scope lookup.
- [Angular parser](https://docxtemplater.com/docs/angular-parse/): expressions, operators, filters written in JavaScript.
- [Image module](https://docxtemplater.com/modules/image/): `getImage` and image sources.
- [FAQ](https://docxtemplater.com/docs/faq/): converting DOCX to PDF.
- [Pricing](https://docxtemplater.com/pricing/): MIT core, commercial modules, a Docker image with an HTTP API in the higher plans.
