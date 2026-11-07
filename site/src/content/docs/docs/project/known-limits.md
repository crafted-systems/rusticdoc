---
title: Known limits
description: What rusticdoc does not do, and what has not been proven, stated plainly.
sidebar:
  order: 1
---

This page lists what does not work, what has not been checked, and what could surprise you. Read it before you depend on rusticdoc. 0.1.0 is its first release, and it is not published yet.

## Requests

- **An Office template with no data, or with `{}`, is returned as it came,** with `200`. Nothing warns you.
- **A file part with an unexpected name is not rejected.**

## Office output

- **Generated DOCX, XLSX and PPTX files have been opened in LibreOffice, not in Word, Excel or PowerPoint.** A repair prompt there has not been ruled out.
- **Barcodes have never been scanned with a reader.** OpenCV has decoded a QR code in the sample invoice once. No other symbology has been decoded, and no hardware or phone reader has been tried.
- **Repeating a whole sheet in XLSX or ODS is not supported,** only rows within one, and neither is a row loop nested in another row block of the same sheet.
- **ODS formulas are not re-based.**
- **Charts are not supported in ODF templates.**
- **Macros and OLE objects in a template are not stripped.** LibreOffice will not run or fetch them when converting, but a request with no conversion step returns them in its output.

## PDF output

- **Filled PDF forms have not been viewed in Chrome, Acrobat or Preview.**
- **Text the service draws into a PDF is Latin only, in Helvetica.** That means field appearances and annotation values. Other characters draw as `?`. Field values are stored in full, so a viewer that redraws the field still shows them.
- **`/v1/pdf/sign` places a signature image.** It is not a cryptographic signature.
- **There is no golden-PDF corpus.** Output is checked by content, not compared byte for byte across architectures.

## Fonts

- **A template that uses a font the service lacks is not reported.** Check `GET /v1/fonts` and the `x-fonts-used` header.
- **Fonts are one set per deployment, not per caller.** Only admin keys can change them, not basic auth or OpenID Connect users.

## Stored templates

- **A stored template cannot be changed.** Store the new one and use its id. There are no versions or aliases yet.
- **Replacing an API key leaves its templates behind** under the default `RUSTICDOC_STORE_SCOPE=caller`.
- **Replicas storing at the same moment can pass the store's limits** by what is in flight.

## Limits on rendering

- **`data` counts against the render memory limit once parsed.** At the default limit, a list of 8 million small numbers rendered and one of 16 million got `413`. Raise `RUSTICDOC_RENDER_MEMORY` for data that size.
- **The render memory limit relies on Linux.** On macOS it has not been shown to hold.
- **The CLI renders in its own process,** with no worker and no memory limit, because there the template is yours.

## Configuration

- **Many settings ignore a value they cannot parse, with no warning.** `RUSTICDOC_RENDER_MEMORY=2g`, `RUSTICDOC_BODY_LIMIT=1.5GB` and `RUSTICDOC_API_TIMEOUT=1m30s` all leave the default in place. Gotenberg's `API_TIMEOUT` and `API_BODY_LIMIT` behave the same way. The [configuration reference](/docs/reference/configuration/) says which settings stop the service instead.
- **A `RUSTICDOC_*` boolean is on only for `1`, `true` or `yes`.** `TRUE` turns it off.

## Deployment

- **The image is not published yet.** Build it with `docker build -t rusticdoc .` until 0.1.0 is out.
- **The Kubernetes manifest has run on minikube with an S3 gateway,** not on a production cluster.
- **The store has run against VersityGW, an S3 gateway, and a shared Docker volume,** not against AWS itself.
- **No load test has been run.** These docs give no throughput figures, and the memory settings come from arithmetic and one failure, not from measurement under load. See [sizing](/docs/operate/sizing/).
- **Three pods sharing about 6 GB were killed for memory** and their conversions timed out.

## Security

- **Escaping in `.html`, `.xml` and `.svg` templates covers text and quoted attributes.** It does not make a value safe in a `<script>` or `<style>` block, in an unquoted attribute or as a URL. See [escaping](/docs/templates/syntax/#escaping).
- **Nothing is escaped in `.csv` and `.md` templates.** A value starting with `=` can run as a formula when a spreadsheet opens the file, and Markdown can hold raw HTML.
- **The disk limit on a conversion is measured every 100 ms and per conversion.** A conversion can pass it by a little before it stops, and several together can use `RUSTICDOC_WORKERS` times the limit. Give `/tmp` a size that allows for that.

- **Many clients together can fill the connection and memory budgets** for as long as the timeouts allow. The service does not rate limit. Do that at a proxy.
- **The service speaks plain HTTP.** Terminate TLS in front of it.
- **A compromised LibreOffice runs as the service's user.** It can see other requests' temporary files, and anything else that user can read.
- **The image has known CVEs in Debian's packages.** [Security](/docs/operate/security/) has the numbers from the last scan.

## Compatibility with other tools

- **`/forms/*` follows Gotenberg 8.39, with exceptions.** In the replay of its own scenarios, 310 of 326 behaved the same as the official image. The other 16 are expected: 10 select PDFtk, which this build leaves out, and 6 expect Chromium in the output of `/health`, `/debug` and the metrics route.
- **There is no HTML or URL rendering through a browser.** This image has no Chromium.
- **Carbone's published XLSX and PPTX examples have not been reproduced** with the new syntax.

## SDKs

- **No SDK is published.** The generated TypeScript and Go SDKs do not compile, and the generated Python one has only been byte-compiled. See [SDKs](/docs/project/sdks/).
