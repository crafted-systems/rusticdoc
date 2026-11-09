---
title: Known limits
description: What rusticdoc does not do, and what has not been proven, stated plainly.
sidebar:
  order: 1
---

This page lists what does not work, what has not been checked, and what could surprise you. Read it before you depend on rusticdoc. 0.1.0 is its first release.

## Requests

- **An Office template with no data, or with `{}`, is returned as it came,** with `200`. Nothing warns you.
- **A file part with an unexpected name is not rejected.**

## Office output

- **Office has been tried on a Mac only.** Templates typed in Word, Excel and PowerPoint 16.114 for Mac were filled, and the results open in those apps without a repair prompt. Office for Windows and Office on the web have not been tried.
- **Word's default font, Aptos, is not in the image.** A document set in it converts to PDF in a substitute font, with different line breaks, until you [upload Aptos](/docs/guides/fonts/) or choose a font the image has.
- **Chinese, Japanese and Korean text in a serif face comes out in sans serif.** Noto Serif CJK is left out of the image, which saves 53 MB, so text set in a face such as SimSun, MS Mincho or Batang converts in Noto Sans CJK. Upload or mount Noto Serif CJK and those faces use it with no other change. See [Chinese, Japanese and Korean fonts](/docs/guides/fonts/#chinese-japanese-and-korean-fonts).
- **Barcodes have been read by software decoders, not by a scanner.** rxing decodes all fifteen symbologies to their exact values, from the image and from LibreOffice's PDF at 300 dpi, and ZBar reads the linear codes and QR. No hardware or phone reader has been tried.
- **Placeholder pictures and colour scopes have been checked in LibreOffice, not in Microsoft Office.** In an XLSX, a placeholder sees the data's top level but not a sheet loop's variable.
- **`drop("shape")` in an XLSX works through alt text only,** and comments do not move when a spreadsheet row or column is removed. An ODT table that does not fill the margins keeps its width after a column goes.
- **A repeated sheet or rows are not followed everywhere.** Formulas, print areas, charts and tables on the repeated sheet follow its copies, and formulas follow repeated rows. Hyperlinks to a sheet, data validation, conditional formatting, pivot caches, charts that point at repeated rows, and formulas on one sheet that point at repeated rows on another are left as the template had them. A chart on an ODS sheet that a sheet loop repeats is not copied. See [spreadsheets](/docs/templates/office/).
- **Rendering takes out macros, OLE objects, external links and private hyperlinks,** and refuses a template part it cannot read with certainty, such as one in an encoding other than UTF-8 or UTF-16, or a content-type entry naming no part. A template that relies on one of these needs `keep`. See [what rendering takes out](/docs/templates/office/#what-rendering-takes-out). The stand-ins for macro projects and OLE objects in its tests are not working macros, since none could be made without Office.
- **Shapes that move along a loop are PowerPoint only.** `shift` refuses other formats.

## PDF output

- **HTML is laid out by Fulgur, not a browser.** JavaScript never runs and no URL is rendered. Fulgur is before version 1.0: text in `@page` margin boxes ignores its alignment, and an image in a flex row with only one of its width and height set is not drawn. See [HTML to PDF](/docs/guides/html/#limits).
- **Filled PDF forms have not been opened in Acrobat.** Editable and flattened forms show their values in Chrome and Preview.
- **Text the service draws into form fields and annotations uses the fonts the service has.** Latin text is drawn in Helvetica as before. Other scripts are shaped and drawn in a font found for them, so a script with no font in the image or the fonts directory draws nothing. Watermark and stamp text is still Latin only. Chrome has not been checked for these fields.
- **`/v1/pdf/sign` places only a signature image unless you send `signature=certificate`.** The default is not a cryptographic signature.

## PDF signatures

- **The service does not renew archive timestamps.** A B-LTA signature stays verifiable for as long as its document timestamp's certificate is valid. Extend it later with a tool such as pyHanko's `ltaupdate`.
- **Signed files have been validated with poppler's `pdfsig` and with pyHanko at every level, not in Adobe Acrobat.**
- **B-LT and B-LTA need certificates that name an OCSP responder or a CRL.** Without one, the request answers `502`. A timestamp authority needs a trust anchor in `RUSTICDOC_SIGNING_TSA_CERT` and an `https` URL.
- **RSA and ECDSA P-256 keys only,** always with SHA-256.
- **One signature per request.**
- **The signature image ignores page rotation,** and the image's transparency is dropped, as for stamps.
- **Keys in a cloud KMS and encrypted key files are not supported.** Keys are PEM files or live in a PKCS#11 token, which was tested with SoftHSM, not a hardware module.
- **PDFs from LibreOffice are not byte-identical across architectures or runs.** Its layout moves text by up to hundredths of a point between processors, and each PDF gets its own dates and ids. `make golden` compares 14 outputs from the amd64 and arm64 images: the Office and text outputs and the PDF form the service fills itself are byte-identical, and LibreOffice's PDFs match as page images or in layout within 0.25 pt.

## Languages

- **Numbers, money and dates follow 62 languages and regions,** from CLDR 48. Another language falls back to its base language, then to the formatting used without one. See [translations and settings](/docs/templates/translations/).
- **`words` spells numbers in English, French, Belgian and Swiss French, and Ukrainian only.** A template whose `lang` is another language gets English.
- **`duration("human")` writes English.**

## Fonts

- **The missing-font report reads a document's text, styles and theme,** not fonts used only in an embedded object or a chart. See [find fonts a template needs](/docs/guides/fonts/#find-fonts-a-template-needs).
- **Fonts are one set per deployment, not per caller.**

## Stored templates

- **A stored template cannot be changed.** Store the new one and point an [alias](/docs/guides/stored-templates/) at it.
- **Replacing a key in `RUSTICDOC_API_KEYS` leaves its templates behind** under the default `RUSTICDOC_STORE_SCOPE=caller`. A key in `RUSTICDOC_TENANT_KEYS` belongs to its tenant, which keeps them. See [authentication](/docs/guides/authentication/).
- **Background jobs, aliases, variables and previews need a store.** Without `RUSTICDOC_STORE` they answer `501`.
- **OpenID Connect admin rules have been tested with local tokens,** not against a real identity provider.
- **Replicas storing at the same moment can pass the store's limits** by what is in flight.

## Limits on rendering

- **`data` counts against the render memory limit once parsed.** At the default limit, a list of 8 million small numbers rendered and one of 16 million got `413`. Raise `RUSTICDOC_RENDER_MEMORY` for data that size.
- **The render memory limit relies on Linux.** On macOS it has not been shown to hold.
- **The CLI renders in its own process,** with no worker and no memory limit, because there the template is yours.

## Configuration

- **Many settings ignore a value they cannot parse, with no warning.** `RUSTICDOC_RENDER_MEMORY=2g`, `RUSTICDOC_BODY_LIMIT=1.5GB` and `RUSTICDOC_API_TIMEOUT=1m30s` all leave the default in place. The [configuration reference](/docs/reference/configuration/) says which settings stop the service instead.
- **A boolean setting is on only for `1`, `true` or `yes`.** `TRUE` turns it off.

## Deployment

- **The image is private while the repository is.** Pulling `ghcr.io/vmosoti/rusticdoc:0.1.0` needs `docker login ghcr.io` with a token that can read packages, or build it with `docker build -t rusticdoc .`.
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

## SDKs

- **No SDK is published.** The generated TypeScript and Go SDKs do not compile, and the generated Python one has only been byte-compiled. See [SDKs](/docs/project/sdks/).
