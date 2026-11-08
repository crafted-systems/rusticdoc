---
title: Changelog
description: What each rusticdoc release contains.
sidebar:
  order: 3
---

This page lists what each release contains, so that you can see what you get and what changed.

## 0.1.0

Not yet released. This is the first release. The image will be `ghcr.io/vmosoti/rusticdoc:0.1.0`.

### Templates

- `{{ }}` tags, `{% for %}`, `{% if %}` and filters in DOCX, ODT, XLSX, ODS, PPTX and ODP, in PDF form fields and annotations, and in text formats (`.txt`, `.md`, `.html`, `.csv`, `.xml`, `.json`, `.svg`).
- Tags split across runs by Word still work, and a loop around a table row or a paragraph repeats it.
- Templates typed in Word, Excel and PowerPoint for Mac open there without a repair prompt once filled. Filled workbooks drop Excel's calculation chain, a loop across two shapes on one slide repeats the slide, and charts and pictures in an unmoved placeholder take the position its layout gives it.
- The same template and data give the same bytes on every run and on amd64 and arm64, except for PDFs from LibreOffice, which it dates and lays out to within hundredths of a point.
- Filters for currency, dates and translations, and for images, barcodes, hyperlinks, colours, a safe subset of HTML, native charts and page breaks.
- `POST /v1/templates/validate` and `rusticdoc validate`, which list every problem in a template against sample data.
- Values escaped by file type in `.html`, `.htm`, `.xml`, `.svg` and `.json` templates.
- `data` and `ctx` sent as text or as a file, with any file name and in any position.

### Conversion and PDF

- LibreOffice conversion through a warm worker pool.
- HTML to PDF with Fulgur, without a browser: CSS layout with `@page` rules, page counters and margin boxes, images, stylesheets and fonts sent as `assets`, and tagged PDF/UA. It runs in the render workers and fetches nothing. `htmlEngine=libreoffice` keeps LibreOffice, and `rusticdoc convert --asset` does the same on the command line.
- An image of about 300 MB to download. Noto Serif CJK and LibreOffice's gallery, wizards and spare icon themes are left out, and Windows Chinese, Japanese and Korean font names map to the matching Noto Sans CJK variant.
- `POST /v1/render`, one multipart request for template, data and steps in order.
- `POST /v1/render/batch`, one template per element of an array, returned as a ZIP.
- `POST /v1/convert`, for one or more files.
- PDF operations under `/v1/pdf/*`: merge, split, rotate, flatten, optimize, encrypt, convert, watermark, stamp, overlay, sign, embed, Factur-X, metadata, bookmarks, and form fields and filling.
- `lopdf`, qpdf and pdfcpu as PDF engines.
- Cryptographic PDF signatures, PAdES B-B style: `signature=certificate` on `/v1/pdf/sign`, a `sign` step on `/v1/render`, and `rusticdoc pdf sign --key --cert`. The key comes from `RUSTICDOC_SIGNING_KEY` and `RUSTICDOC_SIGNING_CERT`, RSA or ECDSA P-256. Without them, signing answers `503` and the image signature works as before.

### Service

- Authentication with API keys, basic auth or OpenID Connect.
- Webhooks with per-caller HMAC signatures.
- Stored templates and uploaded fonts on S3-compatible storage or a shared volume, with several replicas on one store.
- An OpenAPI description at `/openapi.json`.
- JSON logs with access and audit lines, Prometheus metrics, and `/health` and `/ready`.

### Security

- Client templates evaluated in a separate worker process with its own limits.
- Limits on input before it expands, on connections, and on what one caller can hold.
- Engines started with no secrets in their environment, and `/proc` entries closed to them.
- Documents that cannot reach the network, checked by `make outbound`.
- A limit on what one conversion may write to disk, checked by `make disk`.
- A hardened image, `deploy/compose.yaml` and `deploy/kubernetes.yaml`.

### CLI

- The same binary as the service: `render`, `convert`, `validate`, `pdf`, `engines`, `openapi`, `serve`, `health` and `webhook-secret`.

### Not in this release

See [known limits](/docs/project/known-limits/) and the [roadmap](/docs/project/roadmap/). No SDK is published.
