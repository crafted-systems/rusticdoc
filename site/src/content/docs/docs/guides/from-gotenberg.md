---
title: Moving from Gotenberg 8
description: Point a Gotenberg 8 client at rusticdoc, and see which routes answer the same, what differs, and what a Gotenberg 9 client should expect.
sidebar: { order: 8 }
---

rusticdoc's compatibility targets **Gotenberg 8.39**. Nothing has been tested against Gotenberg 9. After this page you can point a Gotenberg 8 client at rusticdoc and know what changes, what is not there, and what would happen with a Gotenberg 9 client.

[Gotenberg](https://gotenberg.dev/) is a separate project. The checks here are against the official `gotenberg/gotenberg:8.39.0` image and rusticdoc's code. Gotenberg's own [documentation](https://gotenberg.dev/docs/getting-started/introduction) is the reference for its fields.

## Change the image and the port

The image listens on `9027`, where Gotenberg listens on `3000`. Either map the port or set Gotenberg's variable:

```bash
docker run -d --name rusticdoc -p 3000:9027 ghcr.io/vmosoti/rusticdoc:0.1.0
# or
docker run -d --name rusticdoc -e API_PORT=3000 -p 3000:3000 ghcr.io/vmosoti/rusticdoc:0.1.0
```

Gotenberg's environment variables work, such as `API_PORT`, `API_BIND_IP`, `API_TIMEOUT`, `API_BODY_LIMIT` and `API_ROOT_PATH`. `RUSTICDOC_*` wins when both are set. The full list is in [configuration](/docs/reference/configuration/).

Gotenberg's command-line flags, such as `gotenberg --api-port=3000`, do not work, because the image's command is `rusticdoc serve`. Use the variables.

## Routes that answer the same

Requests and responses keep Gotenberg's shape on these paths:

| Group | Routes |
|---|---|
| LibreOffice | `POST /forms/libreoffice/convert` |
| PDF engines | `POST /forms/pdfengines/` + `merge`, `split`, `flatten`, `optimize`, `convert`, `metadata/read`, `metadata/write`, `bookmarks/read`, `bookmarks/write`, `encrypt`, `embed`, `watermark`, `stamp`, `rotate`, `factur-x` |
| Service | `GET /health`, `/`, `/favicon.ico`, `/version`, `/prometheus/metrics`, and `/debug` with `API_ENABLE_DEBUG_ROUTE` |

That includes:

- **File naming.** LibreOffice results are `<input>.pdf`, such as `report.docx.pdf`. A merge is `merged.pdf`. Split parts are zero-based `<stem>_<n>.pdf`. `Gotenberg-Output-Filename` renames a result.
- **Errors.** `text/plain` with Gotenberg's strings, not JSON. See [errors](/docs/reference/errors/#forms-routes).
- **Correlation.** `Gotenberg-Trace` is the correlation id in the logs and on deliveries. `Gotenberg-Version` is on authenticated responses.
- **Webhooks.** `Gotenberg-Webhook-Url`, `-Error-Url`, `-Method`, `-Error-Method`, `-Extra-Http-Headers` and `-Events-Url`. As on Gotenberg, a result URL needs an error URL or an events URL beside it, or the answer is `400`.
- **`downloadFrom`**, with private addresses refused unless `RUSTICDOC_DOWNLOAD_FROM_ALLOW_PRIVATE=true`.

```bash test
curl -F files=@report.docx http://localhost:9027/forms/libreoffice/convert \
  -D headers.txt -o report.pdf
head -c4 report.pdf | grep -q %PDF
grep -qi 'filename="report.docx.pdf"' headers.txt
grep -qi "^gotenberg-trace:" headers.txt
```

## What differs

| Topic | Gotenberg 8 | rusticdoc |
|---|---|---|
| Default port | `3000` | `9027` |
| Chromium routes (`/forms/chromium/*`) | HTML, URL and Markdown to PDF, screenshots | `404` |
| PDFtk | An engine you can select | Not in the build. Naming `pdftk` in an engine list, such as `PDFENGINES_MERGE_ENGINES=pdftk`, stops the service from starting |
| ExifTool | Reads and writes metadata | Not in the build. Naming `exiftool` stops the service from starting. Use `qpdf-metadata` or `lopdf`. Metadata routes answer from the service's own code |
| Request id | `Gotenberg-Trace` | Also `x-request-id` |

If your engine lists name `pdftk` or `exiftool`, change them before you switch. The service refuses to start, with a message that names the variable:

```text
Error: cannot start: PDFENGINES_MERGE_ENGINES names 'pdftk', which this build does not include; use qpdf, pdfcpu, lopdf or qpdf-metadata
```

Health, debug and metrics output has no Chromium in it, so a monitor that looks for it will not find it.

Webhook and `downloadFrom` URLs that point at private addresses are refused with a `400`, unless you set `RUSTICDOC_WEBHOOK_ALLOW_PRIVATE=true` or `RUSTICDOC_DOWNLOAD_FROM_ALLOW_PRIVATE=true`. If your receiver is on the same network as the service, you will need the first of these. See [webhooks](/docs/guides/webhooks/#which-urls-are-allowed).

## What rusticdoc does not do

These are in Gotenberg's feature list. Each was checked against rusticdoc's routes and code.

| Feature | In rusticdoc | Do this instead |
|---|---|---|
| HTML to PDF through Chromium | No. LibreOffice can open `.html` files through `/v1/convert`, but that is not a browser and will not match a browser's rendering | Keep a Chromium service for this, and send its PDFs to rusticdoc |
| URL to PDF | No. Nothing renders a URL | The same. `downloadFrom` fetches a file, and does not render a page |
| Markdown to PDF through Chromium | No. `/v1/convert` and `/forms/libreoffice/convert` refuse `.md`. A `.md` template on `/v1/render` with `steps=convert` does convert, through LibreOffice's own Markdown import, so its look will differ | Check the result, or convert Markdown to DOCX first |
| Screenshots of URLs and HTML | No | Keep a Chromium service for this |
| Cryptographic signing | No. [`/v1/pdf/sign`](/docs/guides/pdf-operations/#post-v1pdfsign) only places an image | Sign in your application or with a signing service, after rusticdoc |
| Timestamping | No | The same |
| Redaction | No | Remove the content before it reaches the template, or use a dedicated tool |
| Rendering pages as images | No. `to=png` is a `400` | Use a PDF rasteriser such as `pdftoppm` in your application |
| Validating PDF/A or PDF/UA | No. It exports through LibreOffice and does not validate the result | Check with a validator such as veraPDF |

## Parity numbers

`make parity` replays Gotenberg 8.39's own integration scenarios against this image and the official one, side by side, and compares the outcomes. Scenarios for the Chromium routes are not replayed.

At the last recorded run, **310 of 326 scenarios behave the same**. The other 16 are expected:

- 10 select PDFtk.
- 6 expect Chromium in the output of `/health`, `/debug` and the metrics route.

Run it yourself with `make parity`, which needs Docker and clones the Gotenberg 8.39.0 repository for its scenario files. The script is `test/parity.sh`.

Passing these scenarios does not prove a byte-for-byte match. The output of a conversion depends on the fonts and LibreOffice version in each image.

## What you gain on `/v1/*`

The same service also has its own routes, with JSON errors: [template rendering](/docs/guides/pipeline/), [stored templates](/docs/guides/stored-templates/), [uploaded fonts](/docs/guides/fonts/) and [signed webhooks](/docs/guides/webhooks/#signatures). You can move to them one route at a time. Field names mostly match, with some differences. For example, `/forms/libreoffice/convert` reads options that `/v1/convert` does not, as [Convert documents](/docs/guides/convert/#conversion-options) describes.

## A Gotenberg 9 client

Gotenberg 9 changed its API, as its [upgrade guide](https://gotenberg.dev/docs/getting-started/upgrade) describes. rusticdoc does not implement those changes, and `gotenberg/gotenberg:9` has never been compared with it. **Keep a client on the Gotenberg 8 API to use it with rusticdoc.**

A client written for version 9 will see this. Each row was run against rusticdoc:

| Gotenberg 9 client sends | rusticdoc answers |
|---|---|
| `/forms/pdf/*`, such as `/forms/pdf/merge` or `/forms/pdf/pipe` | `404 Not Found`. rusticdoc has `/forms/pdfengines/*` only |
| The `stages` field on `/forms/libreoffice/convert` | `200`, and `stages` is ignored. Two files with `[{"op":"merge"}]` come back as a ZIP of two PDFs, not merged |
| `nativePdfa` in place of `pdfa` | `200`, and the field is ignored. The result is an ordinary PDF, not PDF/A |
| `singlePage` or `paperSize` | `200`, and the field is ignored |
| Named strings for `initialView`, `magnification` or `pageLayout` | `400`, because rusticdoc expects the 8.x numbers |
| `Gotenberg-Webhook-Url` with only `Gotenberg-Webhook-Events-Url` | Works: `204`, and failures go to the events URL |

Watch the `200` rows. Unknown fields do not produce an error here, while the upgrade guide says Gotenberg 9 answers `400` for them. So a version 9 client's request can succeed and produce the wrong document.

Other points in that guide that do not apply to rusticdoc: `/prometheus/metrics` is still served, `QPDF_BIN_PATH` and `PDFCPU_BIN_PATH` are still read, and the `--pdf-*` flags are not accepted.
