---
title: Known limits
description: What rusticdoc doesn't do, and what could surprise you.
sidebar:
  order: 1
---

Read this list before you depend on rusticdoc. These are the things that don't work or could surprise you.

## Requests

- An Office template sent with no data, or with `{}`, comes back unchanged with a `200`. Nothing warns you.
- A file part with an unexpected name isn't rejected.

## Office output

- Word's default font, Aptos, isn't in the image. A document set in it converts to PDF in a substitute font, with different line breaks, until you [upload Aptos](/docs/guides/fonts/) or pick a font the image has.
- Chinese, Japanese and Korean text in a serif face comes out in sans serif. Noto Serif CJK isn't in the image, so text set in a face like SimSun, MS Mincho or Batang converts in Noto Sans CJK until you upload or mount Noto Serif CJK. See [Chinese, Japanese and Korean fonts](/docs/guides/fonts/#chinese-japanese-and-korean-fonts).
- A placeholder picture in an XLSX sees the data's top level but not loop variables, and a row loop doesn't copy it.
- `drop("shape")` in an XLSX only works through alt text, and comments don't move when a spreadsheet row or column is removed. An ODT table that doesn't fill the margins keeps its width after a column goes.
- A repeated sheet or repeated rows aren't followed everywhere. Hyperlinks to a sheet, data validation, conditional formatting, pivot caches, charts that point at repeated rows, and formulas on one sheet that point at repeated rows on another are left as the template had them. A chart on an ODS sheet that a sheet loop repeats isn't copied. See [spreadsheets](/docs/templates/office/).
- Rendering takes out macros, OLE objects, external links and private hyperlinks, and it refuses a template part it can't read with certainty, like one in an encoding other than UTF-8 or UTF-16, or a content-type entry that names no part. A template that relies on one of these needs `keep`. See [what rendering takes out](/docs/templates/office/#what-rendering-takes-out).
- Shapes that move along a loop are PowerPoint only. `shift` refuses other formats.
- A [column loop](/docs/templates/office/#repeating-columns) works in XLSX only, can't sit inside another column loop, and moves only its own row. Rows above and below keep their columns, and merged cells, column widths, conditional formats, tables and print areas don't follow it.
- A date string fills a spreadsheet cell as text. For a cell Excel treats as a date, use [`excel_date`](/docs/templates/filters/#excel_date) and give the template cell a date format. It refuses dates before 1 March 1900.
- A loop that opens in one paragraph and closes in the next repeats both paragraphs whole. A template ported from a tool that repeats only the text between the tags, like docxtemplater, can come out with an extra paragraph.

## PDF output

- HTML is laid out by Fulgur, not a browser. JavaScript never runs and no URL is rendered. Fulgur hasn't reached version 1.0 yet. Text in `@page` margin boxes ignores its alignment, and an image in a flex row with only one of its width and height set isn't drawn. See [HTML to PDF](/docs/guides/html/#limits).
- Text the service draws into form fields and annotations uses the fonts the service has, so a script with no font in the image or the fonts directory draws nothing. Watermark and stamp text is Latin only.
- `/v1/pdf/sign` only places a signature image unless you send `signature=certificate`. The default isn't a cryptographic signature.
- PDFs from LibreOffice aren't byte-identical across architectures or runs. Its layout moves text by up to hundredths of a point between processors, and each PDF gets its own dates and ids.

## PDF signatures

- The service only renews an archive timestamp when you ask it to. A B-LTA signature stays verifiable for as long as its document timestamp's certificate is valid, so [renew the file](/docs/guides/pdf-operations/#renewing-a-b-lta-file) before then. Nothing renews it on a schedule, and renewal isn't a step on `/v1/render`.
- B-LT and B-LTA need certificates that name an OCSP responder or a CRL. Without one, the request fails with `502`. A timestamp authority needs a trust anchor in `RUSTICDOC_SIGNING_TSA_CERT` and an `https` URL.
- Only RSA and ECDSA P-256 keys are supported, always with SHA-256.
- You get one signature per request.
- The signature image ignores page rotation, and its transparency is dropped, as it is for stamps.
- Keys in a cloud KMS and encrypted key files aren't supported. Keys are PEM files or live in a PKCS#11 token.

## Languages

- Numbers, money and dates are formatted for 62 languages and regions. Any other language falls back to its base language, then to the formatting used without one. See [translations and settings](/docs/templates/translations/).
- `words` spells numbers in English, French, Belgian and Swiss French, and Ukrainian only. A template whose `lang` is another language gets English.
- `duration("human")` writes English.

## Fonts

- The missing-font report reads a document's text, styles and theme. It doesn't see fonts used only in an embedded object or a chart. See [find fonts a template needs](/docs/guides/fonts/#find-fonts-a-template-needs).
- Fonts are one set per deployment, not per caller.

## Stored templates

- A stored template can't be changed. Store the new one and point an [alias](/docs/guides/stored-templates/) at it.
- Replacing a key in `RUSTICDOC_API_KEYS` leaves its templates behind under the default `RUSTICDOC_STORE_SCOPE=caller`. A key in `RUSTICDOC_TENANT_KEYS` belongs to its tenant, which keeps them. See [authentication](/docs/guides/authentication/).
- Background jobs, aliases, variables and previews need a store. Without `RUSTICDOC_STORE` they return `501`.
- Replicas storing at the same moment can go past the store's limits by whatever is in flight.

## Limits on rendering

- `data` counts against the render memory limit once it's parsed. At the default limit, a list of 16 million small numbers gets a `413`. Raise `RUSTICDOC_RENDER_MEMORY` for data that size.
- The render memory limit relies on Linux, so run the service there.
- The CLI renders in its own process, with no worker and no memory limit, because there the template is yours.

## Configuration

- Many settings ignore a value they can't parse, without any warning. `RUSTICDOC_RENDER_MEMORY=2g`, `RUSTICDOC_BODY_LIMIT=1.5GB` and `RUSTICDOC_API_TIMEOUT=1m30s` all leave the default in place. The [configuration reference](/docs/reference/configuration/) says which settings stop the service instead.
- A boolean setting is on only for `1`, `true` or `yes`. `TRUE` turns it off.

## Deployment

- The figures in [sizing](/docs/operate/sizing/#measured-under-load) come from one small document on one container. Large documents and several replicas will differ.
- Replicas need memory of their own. Three pods sharing about 6 GB were killed for memory, and their conversions timed out.

## Security

- Escaping in `.html`, `.xml` and `.svg` templates covers text and quoted attributes. It doesn't make a value safe in a `<script>` or `<style>` block, in an unquoted attribute or as a URL. See [escaping](/docs/templates/syntax/#escaping).
- Nothing is escaped in `.csv` and `.md` templates. A value starting with `=` can run as a formula when a spreadsheet opens the file, and Markdown can hold raw HTML.
- The disk limit on a conversion is measured every 100 ms and per conversion. A conversion can go past it by a little before it stops, and several together can use `RUSTICDOC_WORKERS` times the limit, so give `/tmp` a size that allows for that.
- Many clients together can fill the connection and memory budgets for as long as the timeouts allow. The service doesn't rate limit, so do that at a proxy.
- The service speaks plain HTTP. Terminate TLS in front of it.
- A compromised LibreOffice runs as the service's user. It can see other requests' temporary files, and anything else that user can read.
- The image has known CVEs in Debian's packages. See [Security](/docs/operate/security/) for the numbers from the last scan.

## SDKs

- No SDK is published. You generate the TypeScript, Python and Go clients yourself. Their `render` call sends JSON, so it works with a stored template and not with an uploaded file. See [SDKs](/docs/project/sdks/).
