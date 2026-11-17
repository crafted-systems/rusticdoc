---
title: Known limits
description: What rusticdoc doesn't do, and what could surprise you.
sidebar:
  order: 1
---

Read this list before you depend on rusticdoc. These are the things that don't work or could surprise you. How to set up fonts, signing, stores and sizing is in the guides.

## Requests

- An Office template sent with no data, or with `{}`, comes back unchanged with a `200`. Nothing warns you.
- A file part with an unexpected name isn't rejected.

## Office output

- A placeholder picture in an XLSX sees the data's top level but not loop variables, and a row loop doesn't copy it.
- `drop("shape")` in an XLSX only works through alt text, and comments don't move when a spreadsheet row or column is removed. An ODT table that doesn't fill the margins keeps its width after a column goes.
- A repeated sheet or repeated rows aren't followed everywhere. Hyperlinks to a sheet, data validation, conditional formatting, pivot caches, charts that point at repeated rows, and formulas on one sheet that point at repeated rows on another are left as the template had them. A chart on an ODS sheet that a sheet loop repeats isn't copied. See [spreadsheets](/docs/templates/office/).
- Shapes that move along a loop are PowerPoint only. `shift` refuses other formats.
- A [column loop](/docs/templates/office/#repeating-columns) works in XLSX only, can't sit inside another column loop, and moves only its own row. Rows above and below keep their columns, and merged cells, column widths, conditional formats, tables and print areas don't follow it.

## PDF output

- HTML is laid out by Fulgur, not a browser. JavaScript never runs and no URL is rendered. Fulgur hasn't reached version 1.0 yet. Text in `@page` margin boxes ignores its alignment, and an image in a flex row with only one of its width and height set isn't drawn. See [HTML to PDF](/docs/guides/html/#limits).
- Watermark and stamp text is Latin only.
- PDFs from LibreOffice aren't byte-identical across architectures or runs. Its layout moves text by up to hundredths of a point between processors, and each PDF gets its own dates and ids.

## PDF signatures

- Only RSA and ECDSA P-256 keys are supported, always with SHA-256.
- You get one signature per request.
- The signature image ignores page rotation, and its transparency is dropped, as it is for stamps.
- Keys in a cloud KMS and encrypted key files aren't supported. Keys are PEM files or live in a PKCS#11 token.
- Nothing renews a B-LTA timestamp on a schedule, and renewal isn't a step on `/v1/render`. See [renewing a B-LTA file](/docs/guides/pdf-operations/#renewing-a-b-lta-file).

## Languages

- Numbers, money and dates are formatted for 62 languages and regions. Any other language falls back to its base language, then to the formatting used without one. See [translations and settings](/docs/templates/translations/).
- `words` spells numbers in English, French, Belgian and Swiss French, and Ukrainian only. A template whose `lang` is another language gets English.
- `duration("human")` writes English.

## Stored templates

- A service has one API key, and templates, aliases, fonts and background jobs are one set per store. Callers that must stay apart need a store each. See [authentication](/docs/guides/authentication/).
- Replicas storing at the same moment can go past the store's limits by whatever is in flight.

## Rendering

- The render memory limit relies on Linux, so run the service there.
- The CLI renders in its own process, with no worker and no memory limit.

## Configuration

- Many settings ignore a value they can't parse, without any warning. `RUSTICDOC_RENDER_MEMORY=2g`, `RUSTICDOC_BODY_LIMIT=1.5GB` and `RUSTICDOC_API_TIMEOUT=1m30s` all leave the default in place. The [configuration reference](/docs/reference/configuration/) says which settings stop the service instead.
- A boolean setting is on only for `1`, `true` or `yes`. `TRUE` turns it off.

## Security

- Escaping in `.html`, `.xml` and `.svg` templates covers text and quoted attributes. It doesn't make a value safe in a `<script>` or `<style>` block, in an unquoted attribute or as a URL. See [escaping](/docs/templates/syntax/#escaping).
- Nothing is escaped in `.csv` and `.md` templates. A value starting with `=` can run as a formula when a spreadsheet opens the file, and Markdown can hold raw HTML.
- The disk limit on a conversion is measured every 100 ms and per conversion. A conversion can go past it by a little before it stops, and several together can use `RUSTICDOC_WORKERS` times the limit.
- The service doesn't rate limit. Many clients together can fill the connection and memory budgets for as long as the timeouts allow.
- The service speaks plain HTTP, with no TLS.
- A compromised LibreOffice runs as the service's user. It can see other requests' temporary files, and anything else that user can read.
- The image has known CVEs in Debian's packages. See [Security](/docs/operate/security/) for the numbers from the last scan.

## SDKs

- No SDK is published. You generate the TypeScript, Python and Go clients yourself. Their `render` call sends JSON, so it works with a stored template and not with an uploaded file. See [SDKs](/docs/project/sdks/).
