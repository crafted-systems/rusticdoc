---
title: rusticdoc and Gotenberg
seoTitle: "rusticdoc vs Gotenberg: templates, Office conversion and PDF operations"
description: How rusticdoc compares with Gotenberg for converting Office documents and HTML to PDF, filling templates and processing PDFs, and when to use each.
sidebar:
  order: 1
---

Gotenberg and rusticdoc are both self-hosted HTTP services that take files in a multipart `POST` and send back PDFs. They overlap on Office conversion and PDF processing, and differ on where the document comes from and how HTML becomes a PDF.

- **Gotenberg** is a Docker-based API for PDF. It prints web pages, HTML and Markdown with headless Chromium, converts Office documents with LibreOffice, and runs PDF operations with its own engine, litho.
- **rusticdoc** fills Office, text, HTML and PDF form templates from JSON, converts the result with LibreOffice or, for HTML, with Fulgur, and finishes it with PDF operations. It runs as a container or as a CLI.

## At a glance

| | rusticdoc | Gotenberg |
|---|---|---|
| What it is | A document service that fills templates from JSON, converts them and runs PDF operations | A Docker-based API that converts web pages and Office documents to PDF and processes PDFs |
| Licence | Apache 2.0 | MIT |
| How you run it | A container image for amd64 and arm64, or the same binary as a CLI | A Docker image, with Chromium-only, LibreOffice-only, AWS Lambda and Cloud Run variants, for amd64, arm64, arm/v7, 386 and ppc64le |
| Templates it fills | DOCX, ODT, XLSX, ODS, PPTX, ODP, PDF forms, and text formats such as `.html`, `.txt`, `.csv`, `.xml` and `.json` | None. It converts and processes the files you send |
| Template syntax | Jinja-style, evaluated by minijinja: `{{ customer.name }}`, `{% for %}`, `{% if %}` | Not applicable |
| Conversion engines | LibreOffice for Office files, Fulgur for HTML | Chromium for URLs, HTML and Markdown, LibreOffice for Office files |
| HTML to PDF | Fulgur lays out HTML and CSS written for print. No browser, no JavaScript, no network, and no route that renders a URL | Chromium prints a URL, an HTML file with its assets, or Markdown. Pages run JavaScript, and a request can wait for a selector or an expression |
| Images of pages | One page of a stored template as a PNG preview, with a store configured | Screenshots of URLs and HTML, and PDF pages rendered as images |
| PDF operations | Merge, split, overlay, rotate, watermark, stamp, flatten, optimize, encrypt, metadata, bookmarks, embedded files, Factur-X, form filling, PDF/A and PDF/UA, image signatures and certificate signatures up to PAdES B-LTA | Merge, split, select, rotate, stamp, form filling and flattening, metadata, bookmarks, attachments, text extraction, redaction, optimize, encrypt, sign, timestamp, and PDF/A, PDF/UA and Factur-X remediation and validation |
| Authentication and TLS | One API key. Plain HTTP, so you terminate TLS in front of it | Basic authentication or OIDC bearer tokens, and HTTPS built in |
| Hosting | Self-hosted only | Self-hosted. A public demo instance is there to try the API |

## Where they differ

### Where the document comes from

Gotenberg converts what you send. It has no route that fills a template with data, so your code builds the HTML or the Word file first, then posts it.

rusticdoc does the filling itself. You send a template and a JSON `data` field to `/v1/render`, and `steps` says what to do with the result:

```bash
curl -F template=@invoice.docx -F "data=<invoice.json" -F steps=convert \
  http://localhost:9027/v1/render -o invoice.pdf
```

A template is an ordinary Word, Excel, PowerPoint or LibreOffice file with tags typed into it. See [template syntax](/docs/templates/syntax/) and [Office formats](/docs/templates/office/). If you only want conversion, `/v1/convert` works without a template, much like Gotenberg's LibreOffice route.

### HTML to PDF

This is the biggest difference. Gotenberg uses Chromium, so it renders what a browser renders: live pages, pages that load data with JavaScript, and URLs. It can wait for the page to be ready, set cookies, and fail the request when a resource doesn't load.

rusticdoc doesn't run a browser. HTML goes to [Fulgur](/docs/guides/html/), a layout engine that writes the PDF itself. It runs no JavaScript and fetches nothing, so the images, stylesheets and fonts a page uses travel with the request as `assets`:

```bash
curl -F files=@invoice.html -F assets=@invoice.css -F assets=@logo.png \
  http://localhost:9027/v1/convert -o invoice.pdf
```

That suits HTML you write to be printed, such as invoices and statements, and keeps documents from reaching the network. It doesn't suit pages built for the web. Fulgur hasn't reached version 1.0, text in `@page` margin boxes ignores its alignment, and an image in a flex row needs both its width and height. The [known limits](/docs/project/known-limits/#pdf-output) list these.

### PDF operations

Both services merge, split, rotate, stamp, lay one PDF over another, encrypt, fill forms, write metadata and bookmarks, and sign. The differences:

- Gotenberg redacts regions, extracts text and attachments, renders pages as images, infers tags for untagged PDFs, and checks PDF/A, PDF/UA and Factur-X conformance with `/forms/pdf/validate`. rusticdoc has none of these for the PDFs you send.
- Gotenberg signs with a certificate you upload with the request. rusticdoc signs with a key the deployment holds, and a request never carries one. It signs at PAdES levels B-B to B-LTA and can renew a B-LTA file's archive timestamp. See [PDF operations](/docs/guides/pdf-operations/).

Gotenberg runs operations as `stages`, a JSON list on the conversion routes and on `/forms/pdf/pipe`. rusticdoc runs them as `steps`, a comma-separated list on `/v1/render`, in the order you give. See [the pipeline](/docs/guides/pipeline/).

### Running it

Gotenberg's image comes in more variants, including images set up for AWS Lambda and Cloud Run, and for more architectures. It has basic authentication, OIDC and HTTPS of its own.

rusticdoc's image is built for amd64 and arm64 and runs hardened by default, as a non-root user with a read-only root filesystem. It speaks plain HTTP and checks one API key, so TLS and rate limiting belong at a proxy. See [security](/docs/operate/security/) and the [security overview](/docs/project/security-overview/).

## Moving requests from Gotenberg

If you use Gotenberg for Office conversion and PDF operations, most requests have a direct counterpart:

| Gotenberg | rusticdoc |
|---|---|
| Port 3000 | Port 9027 |
| `POST /forms/libreoffice/convert` with `files` | `POST /v1/convert` with `files` |
| `POST /forms/chromium/convert/html` with `index.html` and its assets | `POST /v1/convert` with the page in `files` and the rest in `assets`. Fulgur lays it out, without JavaScript |
| `POST /forms/chromium/convert/url` | No equivalent. rusticdoc never renders a URL |
| `POST /forms/chromium/screenshot/*` | No equivalent |
| `POST /forms/pdf/merge` | `POST /v1/pdf/merge` |
| `stages` on a conversion route | `steps` on `/v1/render`, or the step fields on `/v1/convert`, which run in a fixed order |

Check fonts when you move Office conversions. Both services convert with LibreOffice, so the output depends on the fonts each image carries. rusticdoc's image doesn't include Word's default font, Aptos. See [fonts](/docs/guides/fonts/).

## When to pick which

Gotenberg is the better fit when:

- You need to print live web pages or URLs, or pages that draw content with JavaScript.
- You need screenshots, page images, redaction, text extraction or conformance validation.
- You want ready-made images for AWS Lambda or Cloud Run, or architectures beyond amd64 and arm64.
- Your code already builds the finished document, and you only need conversion and processing.

rusticdoc is the better fit when:

- You want templates people edit in Word, Excel, PowerPoint or LibreOffice, filled from JSON by the same service that converts them.
- You want HTML laid out with no browser and no network access, from files you send.
- You want certificate signatures at PAdES B-LTA with a key the deployment holds.
- You want one request to fill, convert and finish a document.

You can also run both: Gotenberg for web pages, and rusticdoc for documents built from templates and data.

## Related pages

- [Concepts](/docs/start/concepts/)
- [Convert documents](/docs/guides/convert/)
- [HTML to PDF](/docs/guides/html/)
- [PDF operations](/docs/guides/pdf-operations/)
- [Known limits](/docs/project/known-limits/)

## Sources

Checked in November 2026. Gotenberg is a trademark of its owner.

- [Gotenberg repository and README](https://github.com/gotenberg/gotenberg): licence (MIT), features, version 9.
- [Gotenberg documentation home](https://gotenberg.dev/docs): what Gotenberg is, Docker image.
- [How Gotenberg works](https://gotenberg.dev/docs/about): Chromium, LibreOffice and litho, stages.
- [Routes](https://gotenberg.dev/docs/getting-started/routes): every route and which takes which feature.
- [Install](https://gotenberg.dev/docs/getting-started/installation): image variants, architectures, live demo.
- [Pipe](https://gotenberg.dev/docs/pdf/pipe): the list of PDF stages.
- [Convert Markdown](https://gotenberg.dev/docs/chromium/convert-markdown): Chromium wait and failure options.
- [Secure your deployment](https://gotenberg.dev/docs/operate/secure): basic authentication, OIDC, HTTPS, outbound requests.
