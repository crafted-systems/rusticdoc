---
title: Changelog
description: What each rusticdoc release contains.
sidebar:
  order: 3
---

This page lists what each release contains, so that you can see what you get and what changed.

## Version numbers

rusticdoc follows [ZeroVer](https://0ver.org/): its versions stay below 1.0. A new minor number, such as 0.2.0, may change behaviour you rely on, and this page says what changed. A new patch number, such as 0.1.1, fixes faults without changing what works. Pin the exact version, and read this page before you move to a new minor one.

## 0.1.0

Released on 9 November 2026. This is the first release. The image is `ghcr.io/vmosoti/rusticdoc:0.1.0`, for amd64 and arm64.

### Templates

- `{{ }}` tags, `{% for %}`, `{% if %}` and filters in DOCX, ODT, XLSX, ODS, PPTX and ODP, in PDF form fields and annotations, and in text formats (`.txt`, `.md`, `.html`, `.csv`, `.xml`, `.json`, `.svg`).
- Tags split across runs by Word still work, and a loop around a table row or a paragraph repeats it.
- Templates typed in Word, Excel and PowerPoint for Mac open there without a repair prompt once filled. Filled workbooks drop Excel's calculation chain, a loop across two shapes on one slide repeats the slide, and charts and pictures in an unmoved placeholder take the position its layout gives it.
- The same template and data give the same bytes on every run and on amd64 and arm64, except for PDFs from LibreOffice, which it dates and lays out to within hundredths of a point.
- Filters for currency, dates and translations, and for images, barcodes, hyperlinks, colours, a safe subset of HTML, native charts and page breaks.
- `POST /v1/templates/validate` and `rusticdoc validate`, which list every problem in a template against sample data.
- Values escaped by file type in `.html`, `.htm`, `.xml`, `.svg` and `.json` templates.
- `data` and `ctx` sent as text or as a file, with any file name and in any position.
- A whole sheet repeated per element of a list in XLSX and ODS with `{% sheet for … %}`, row loops nested in other row loops, and formulas, print areas, tables and charts that follow the copies, in ODS as well as XLSX.
- Settings read from custom document properties as well as `{% settings %}`.
- Native charts in ODT, ODS and ODP, and shapes that move along a loop in PowerPoint with `shift`.
- Rendering takes macros, OLE objects and ActiveX controls, external links and links to private addresses out of every output, unless the request keeps them with `keep`.
- `undefinedAsEmpty`, to print values missing from the data as nothing for one request.
- Calculation helpers: `avg`, `ceil`, `floor`, `words`, `truncate`, `unaccent`, `pad_left`, `pad_right`, date arithmetic with `date_add`, `date_sub`, `start_of`, `end_of` and `date_diff`, `duration`, `now()`, and the `empty` test. New pages on [calculations](/docs/templates/calculations/) and [conditions](/docs/templates/conditions/).
- Translations and formatting by language: `lang`, `translations`, `timezone` and `currency` request fields, dictionaries in `RUSTICDOC_TRANSLATIONS_DIR`, fallback from a region to its language, numbers, money and dates written by CLDR for 62 languages, IANA time zones, a `number` filter, and `t`, `date`, `currency` and `{% settings %}` in text templates and PDF tags. A template that sets `lang` now formats numbers, money and dates for that language. `rusticdoc translate` lists a template's keys in a dictionary file.
- Validation reports fonts a template names that the service lacks, and keys without a translation in the language asked for.
- `drop()` removes the paragraph, row, column, table, picture, chart, shape, slide or sheet that holds it, on a condition, in every Office format where that element exists. See [remove an element](/docs/templates/office/#remove-an-element).
- Placeholder pictures: a picture in the template whose alt text holds an `image` or `barcode` tag keeps its frame, position and effects, with `fit` set to `contain`, `cover`, `width` or `stretch`. Images may be PNG, JPEG, GIF, WebP or SVG, which is cleaned and given a PNG fallback.
- Fifteen barcode symbologies, adding Data Matrix, PDF417, Aztec, Code 93, UPC-A, UPC-E, ITF, ITF-14, Codabar and GS1-128, with text under linear codes, colours, quiet zone, rotation, error correction and exact sizes.
- `color` reaches a paragraph, table cell, table row or shape, as a text or background colour.

### Conversion and PDF

- LibreOffice conversion through a warm worker pool.
- HTML to PDF with Fulgur, without a browser: CSS layout with `@page` rules, page counters and margin boxes, images, stylesheets and fonts sent as `assets`, and tagged PDF/UA. It runs in the render workers and fetches nothing. `htmlEngine=libreoffice` keeps LibreOffice, and `rusticdoc convert --asset` does the same on the command line.
- An image of about 300 MB to download. Noto Serif CJK and LibreOffice's gallery, wizards and spare icon themes are left out, and Windows Chinese, Japanese and Korean font names map to the matching Noto Sans CJK variant.
- `POST /v1/render`, one multipart request for template, data and steps in order.
- `POST /v1/render/batch`, one template per element of an array, returned as a ZIP.
- `POST /v1/convert`, for one or more files.
- PDF operations under `/v1/pdf/*`: merge, split, rotate, flatten, optimize, encrypt, convert, watermark, stamp, overlay, sign, embed, Factur-X, metadata, bookmarks, and form fields and filling.
- `lopdf`, qpdf and pdfcpu as PDF engines.
- Cryptographic PDF signatures from PAdES B-B to B-LTA: `signature=certificate` on `/v1/pdf/sign`, a `sign` step on `/v1/render` that can be visible, and `rusticdoc pdf sign`. Timestamps from a timestamp authority you trust, revocation data in a DSS and a document timestamp, certification with DocMDP, named keys each with its own callers, and keys in a PKCS#11 token. RSA or ECDSA P-256. Without a key, signing answers `503` and the image signature works as before.
- Text in any script drawn into PDF form fields and annotations, shaped and in subset fonts.
- An `underlay` step that lays a rendered document over a base PDF, such as letterhead.

### Service

- Authentication with API keys, basic auth or OpenID Connect.
- Webhooks with per-caller HMAC signatures.
- Stored templates and uploaded fonts on S3-compatible storage or a shared volume, with several replicas on one store.
- An OpenAPI description at `/openapi.json`.
- JSON request bodies for renders by stored template, template aliases with versions, a template's variables, page previews as PNG, and background jobs polled at `/v1/jobs/{id}` with `Prefer: respond-async`.
- Tenants whose keys can be replaced without losing their templates, and admin rights for basic-auth and OpenID Connect users.
- JSON logs with access and audit lines, Prometheus metrics including the LibreOffice queue, worker restarts and template cache hits, and `/health` and `/ready`.
- A client that hangs up stops its conversion and frees its place in the queue.

### Security

- Client templates evaluated in a separate worker process with its own limits.
- Limits on input before it expands, on connections, and on what one caller can hold.
- Engines started with no secrets in their environment, and `/proc` entries closed to them.
- Documents that cannot reach the network, checked by `make outbound`.
- A limit on what one conversion may write to disk, checked by `make disk`.
- A hardened image, `deploy/compose.yaml` and `deploy/kubernetes.yaml`.

### CLI

- The same binary as the service: `render`, `convert`, `validate`, `translate`, `pdf`, `engines`, `openapi`, `serve`, `health` and `webhook-secret`.

### Not in this release

See [known limits](/docs/project/known-limits/) and the [roadmap](/docs/project/roadmap/). No SDK is published.
