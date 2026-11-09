---
title: Known limits
description: What rusticdoc doesn't do, what hasn't been tested, and what could surprise you.
sidebar:
  order: 1
---

Read this list before you depend on rusticdoc. 0.1.0 is its first release, and these are the things that don't work, haven't been checked or could surprise you.

## Requests

- An Office template sent with no data, or with `{}`, comes back unchanged with a `200`. Nothing warns you.
- A file part with an unexpected name isn't rejected.

## Office output

- Office has only been tested on a Mac. Templates typed in Word, Excel and PowerPoint 16.114 for Mac were filled, and the results open in those apps without a repair prompt. Office for Windows and Office on the web aren't tested yet.
- Word's default font, Aptos, isn't in the image. A document set in it converts to PDF in a substitute font, with different line breaks, until you [upload Aptos](/docs/guides/fonts/) or pick a font the image has.
- Chinese, Japanese and Korean text in a serif face comes out in sans serif. Noto Serif CJK is left out of the image to save 53 MB, so text set in a face like SimSun, MS Mincho or Batang converts in Noto Sans CJK. Upload or mount Noto Serif CJK and those faces use it with no other change. See [Chinese, Japanese and Korean fonts](/docs/guides/fonts/#chinese-japanese-and-korean-fonts).
- Barcodes have been read by software decoders, but not by a hardware scanner or a phone. rxing decodes all fifteen symbologies to their exact values, from the image and from LibreOffice's PDF at 300 dpi, and ZBar reads the linear codes and QR.
- Placeholder pictures and colour scopes have been checked in LibreOffice, but not in Microsoft Office. In an XLSX, a placeholder sees the data's top level but not a sheet loop's variable.
- `drop("shape")` in an XLSX only works through alt text, and comments don't move when a spreadsheet row or column is removed. An ODT table that doesn't fill the margins keeps its width after a column goes.
- A repeated sheet or repeated rows aren't followed everywhere. Formulas, print areas, charts and tables on the repeated sheet follow its copies, and formulas follow repeated rows. Hyperlinks to a sheet, data validation, conditional formatting, pivot caches, charts that point at repeated rows, and formulas on one sheet that point at repeated rows on another are left as the template had them. A chart on an ODS sheet that a sheet loop repeats isn't copied. See [spreadsheets](/docs/templates/office/).
- Rendering takes out macros, OLE objects, external links and private hyperlinks. It also refuses a template part it can't read with certainty, like one in an encoding other than UTF-8 or UTF-16, or a content-type entry that names no part. A template that relies on one of these needs `keep`. See [what rendering takes out](/docs/templates/office/#what-rendering-takes-out). In the tests, the stand-ins for macro projects and OLE objects aren't working macros, because none could be made without Office.
- Shapes that move along a loop are PowerPoint only. `shift` refuses other formats.

## PDF output

- HTML is laid out by Fulgur, not a browser. JavaScript never runs and no URL is rendered. Fulgur hasn't reached version 1.0 yet. Text in `@page` margin boxes ignores its alignment, and an image in a flex row with only one of its width and height set isn't drawn. See [HTML to PDF](/docs/guides/html/#limits).
- Filled PDF forms haven't been opened in Acrobat. Editable and flattened forms show their values in Chrome and Preview.
- Text the service draws into form fields and annotations uses the fonts the service has. Latin text is drawn in Helvetica as before. Other scripts are shaped and drawn in a font found for them, so a script with no font in the image or the fonts directory draws nothing. Watermark and stamp text is still Latin only. Chrome hasn't been checked for these fields.
- `/v1/pdf/sign` only places a signature image unless you send `signature=certificate`. The default isn't a cryptographic signature.

## PDF signatures

- The service doesn't renew archive timestamps. A B-LTA signature stays verifiable for as long as its document timestamp's certificate is valid, and you can extend it later with a tool like pyHanko's `ltaupdate`.
- Signed files have been validated with poppler's `pdfsig` and with pyHanko at every level, but not in Adobe Acrobat.
- B-LT and B-LTA need certificates that name an OCSP responder or a CRL. Without one, the request fails with `502`. A timestamp authority needs a trust anchor in `RUSTICDOC_SIGNING_TSA_CERT` and an `https` URL.
- Only RSA and ECDSA P-256 keys are supported, always with SHA-256.
- You get one signature per request.
- The signature image ignores page rotation, and its transparency is dropped, as it is for stamps.
- Keys in a cloud KMS and encrypted key files aren't supported. Keys are PEM files or live in a PKCS#11 token, which was tested with SoftHSM, not a hardware module.
- PDFs from LibreOffice aren't byte-identical across architectures or runs. Its layout moves text by up to hundredths of a point between processors, and each PDF gets its own dates and ids. `make golden` compares 14 outputs from the amd64 and arm64 images. The Office and text outputs and the PDF form the service fills itself are byte-identical, and LibreOffice's PDFs match as page images or in layout within 0.25 pt.

## Languages

- Numbers, money and dates follow 62 languages and regions, from CLDR 48. Any other language falls back to its base language, then to the formatting used without one. See [translations and settings](/docs/templates/translations/).
- `words` spells numbers in English, French, Belgian and Swiss French, and Ukrainian only. A template whose `lang` is another language gets English.
- `duration("human")` writes English.

## Fonts

- The missing-font report reads a document's text, styles and theme. It doesn't see fonts used only in an embedded object or a chart. See [find fonts a template needs](/docs/guides/fonts/#find-fonts-a-template-needs).
- Fonts are one set per deployment, not per caller.

## Stored templates

- A stored template can't be changed. Store the new one and point an [alias](/docs/guides/stored-templates/) at it.
- Replacing a key in `RUSTICDOC_API_KEYS` leaves its templates behind under the default `RUSTICDOC_STORE_SCOPE=caller`. A key in `RUSTICDOC_TENANT_KEYS` belongs to its tenant, which keeps them. See [authentication](/docs/guides/authentication/).
- Background jobs, aliases, variables and previews need a store. Without `RUSTICDOC_STORE` they return `501`.
- OpenID Connect admin rules have been tested with local tokens, not against a real identity provider.
- Replicas storing at the same moment can go past the store's limits by whatever is in flight.

## Limits on rendering

- `data` counts against the render memory limit once it's parsed. At the default limit, a list of 8 million small numbers rendered and one of 16 million got a `413`. Raise `RUSTICDOC_RENDER_MEMORY` for data that size.
- The render memory limit relies on Linux. It hasn't been shown to hold on macOS.
- The CLI renders in its own process, with no worker and no memory limit, because there the template is yours.

## Configuration

- Many settings ignore a value they can't parse, without any warning. `RUSTICDOC_RENDER_MEMORY=2g`, `RUSTICDOC_BODY_LIMIT=1.5GB` and `RUSTICDOC_API_TIMEOUT=1m30s` all leave the default in place. The [configuration reference](/docs/reference/configuration/) says which settings stop the service instead.
- A boolean setting is on only for `1`, `true` or `yes`. `TRUE` turns it off.

## Deployment

- The image is private for as long as the repository is. To pull `ghcr.io/vmosoti/rusticdoc:0.1.0` you need `docker login ghcr.io` with a token that can read packages, or you can build it with `docker build -t rusticdoc .`.
- The Kubernetes manifest has run on minikube with an S3 gateway, not on a production cluster.
- The store has run against VersityGW, an S3 gateway, and a shared Docker volume, not against AWS itself.
- No load test has been run. These docs give no throughput figures, and the memory settings come from arithmetic and one failure, not from measurement under load. See [sizing](/docs/operate/sizing/).
- Three pods sharing about 6 GB were killed for memory and their conversions timed out.

## Security

- Escaping in `.html`, `.xml` and `.svg` templates covers text and quoted attributes. It doesn't make a value safe in a `<script>` or `<style>` block, in an unquoted attribute or as a URL. See [escaping](/docs/templates/syntax/#escaping).
- Nothing is escaped in `.csv` and `.md` templates. A value starting with `=` can run as a formula when a spreadsheet opens the file, and Markdown can hold raw HTML.
- The disk limit on a conversion is measured every 100 ms and per conversion. A conversion can go past it by a little before it stops, and several together can use `RUSTICDOC_WORKERS` times the limit, so give `/tmp` a size that allows for that.
- Many clients together can fill the connection and memory budgets for as long as the timeouts allow. The service doesn't rate limit, so do that at a proxy.
- The service speaks plain HTTP. Terminate TLS in front of it.
- A compromised LibreOffice runs as the service's user. It can see other requests' temporary files, and anything else that user can read.
- The image has known CVEs in Debian's packages. See [Security](/docs/operate/security/) for the numbers from the last scan.

## SDKs

- No SDK is published. The generated TypeScript and Go SDKs don't compile, and the generated Python one has only been byte-compiled. See [SDKs](/docs/project/sdks/).
