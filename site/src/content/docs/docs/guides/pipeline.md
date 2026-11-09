---
title: The render pipeline
description: Fill a template from JSON and run conversion and PDF steps in one request to /v1/render.
sidebar: { order: 1 }
---

`POST /v1/render` fills a template from JSON and runs a list of steps on the result. After this page you can build one request that fills an invoice, converts it, sets its metadata and encrypts it, and you will know what comes back.

```bash test
curl -F template=@invoice.docx -F "data=<invoice.json" \
  -F steps=convert,metadata,encrypt \
  -F 'metadata={"Title":"Invoice INV-2041"}' \
  -F userPassword=open-sesame \
  http://localhost:9027/v1/render -o invoice.pdf
head -c4 invoice.pdf | grep -q %PDF
```

The body is a `multipart/form-data` form. Tag syntax is in [template syntax](/docs/templates/syntax/).

## Send data as text or as a file

These two do the same thing:

```bash test
# As a text field. The quotes stop your shell reading the <.
curl -F template=@invoice.docx -F "data=<invoice.json" -F steps=convert -F to=html \
  http://localhost:9027/v1/render -o filled.html
grep -q "Amani" filled.html

# As a file part, with any file name, before or after the template.
curl -F data=@invoice.json -F template=@invoice.docx -F steps=convert -F to=html \
  http://localhost:9027/v1/render -o filled.html
grep -q "Amani" filled.html
```

`ctx` works the same way. What the service does with these parts:

- A `data` or `ctx` part that is not valid JSON is a `400` that names the file.
- Sending the same field twice, such as text and a file, is a `400`.
- A file named exactly `data.json` or `ctx.json` is read as that field, whichever field it arrives in.
- Once those are set aside, the first file part in the form is the template, whatever its field name.

## With no data and no steps, a document returns as it came in

A form with an Office file or PDF, no `data` and no `steps` is a no-op. The response is the same bytes you sent, with a `200`.

```bash test
curl -F template=@report.docx http://localhost:9027/v1/render -D headers.txt -o same.docx
head -c2 same.docx | grep -q PK
grep -qi "report.docx" headers.txt
```

Tags in an Office file are filled only when `data` is a non-empty object. So `steps=convert` with no `data` converts the file with its tags as written. Text templates (`.txt`, `.md`, `.html`, `.htm`, `.csv`, `.xml`, `.json`, `.svg`) always render, so one with tags and no data gets `422`.

A file in any other format with `data` gets `501 not_supported`. With no `data` it passes through.

## Form fields

| Field | Kind | What it does |
|---|---|---|
| `template` | File | The template. The first file part in the form takes this role, whatever its field name |
| `templateId` | Text | The id of a [stored template](/docs/guides/stored-templates/). Sending it with a `template` file part is a `400` |
| `templateHash` | Text | The hex SHA-256 of a template this caller sent recently. See [sending a hash](#sending-a-hash) |
| `data` | Text, JSON object | What the template sees as its root. Anything other than an object is a `400` |
| `ctx` | Text, JSON | Visible in the template as `ctx`. `data` wins if it has its own `ctx` key |
| `steps` | Text | Comma-separated step names, in order |
| `outputFilename` | Text | The result's file name, without an extension |
| `files` | Files | Further PDFs for the `merge` step. Every file part after the template lands here, except those under the fields below |
| `fonts` | Files | Fonts for this request alone. See [fonts](/docs/guides/fonts/) |
| `watermark`, `stamp` | Files | Image or PDF marks for those steps |
| `embeds` | Files | Attachments for the `embed` step |
| `facturxXml` | File | The invoice XML for the `factur-x` step |
| `flatten` | Text | With a PDF template and data: fix the filled values into the page |
| `stripAnnotations` | Text | With a PDF template and data: `false` keeps the annotations that held tags. Default is to remove them |
| `downloadFrom` | Text, JSON array | Fetches files from URLs instead of uploading them, as `[{"url": "..."}]`. At most 10 entries. Private addresses are refused unless `RUSTICDOC_DOWNLOAD_FROM_ALLOW_PRIVATE=true` |

The conversion options in [Convert documents](/docs/guides/convert/#conversion-options), such as `landscape` and `nativePageRanges`, apply to every LibreOffice step.

A PDF sent with data is a form to fill. See [PDF templates](/docs/templates/pdf-templates/).

## Steps

Each step works on the document or documents in flight, in the order `steps` lists them. Names are not case-sensitive.

| Step | Fields | What it does |
|---|---|---|
| `convert` | `to`, default `pdf`: `pdf`, `docx`, `xlsx`, `pptx`, `odt`, `ods`, `odp`, `html`, `txt` | Converts with LibreOffice |
| `pdfa` | `pdfa`, default `PDF/A-2b`: `PDF/A-1b`, `PDF/A-2b`, `PDF/A-3b`, `PDF/A-4` | Exports as PDF/A with LibreOffice |
| `pdfua` | None | Exports as PDF/UA with LibreOffice |
| `merge` | `files` | Appends the further PDFs, in the order sent. After a `split`, merges the parts again |
| `split` | `splitMode` (`pages` or `intervals`) and `splitSpan`, both required; `splitUnify` | Splits into several documents |
| `watermark` | `watermarkSource`, `watermarkExpression`, `watermarkOptions`, `watermarkPages`, `watermark` | Draws text, an image or a PDF under the page content |
| `stamp` | `stampSource`, `stampExpression`, `stampOptions`, `stampPages`, `stamp` | Draws text, an image or a PDF over the page content |
| `rotate` | `rotateAngle`, default `90`: `90`, `180`, `270`, `-90`; `rotatePages`, default all | Rotates pages |
| `flatten` | None | Fixes forms and annotations into the page content |
| `optimize` | None | Recompresses streams and drops unused objects |
| `bookmarks` | `bookmarks`, a JSON array of `{"title", "page", "children"}` | Replaces the outline |
| `metadata` | `metadata`, a JSON object such as `{"Title": "Invoice"}` | Writes metadata keys and keeps the others |
| `embed` | `embeds`; `embedsMetadata`, a JSON object mapping a file name to `{"mimeType": "..."}` | Attaches files |
| `factur-x` | `facturxXml`; `facturxConformanceLevel`, default `EN16931`: `MINIMUM`, `BASIC_WL`, `BASIC`, `EN16931`, `EXTENDED` | Embeds Factur-X invoice XML |
| `encrypt` | `userPassword`, `ownerPassword`; at least one. A missing owner password takes the user password | Encrypts the PDF |
| `sign` | `reason`, `location`, `contactInfo`, `signerName`, `level`, `certify`, `signingKey`, and for a visible signature `page`, `x`, `y`, `width` and an image under `signatureImage`, all optional | Signs the PDF cryptographically, as `/v1/pdf/sign` with `signature=certificate` does: invisibly, or with the image or a box of text at `x` and `y`. See [signing with a certificate](/docs/guides/pdf-operations/#with-a-certificate) |

Every step from `merge` down needs a PDF, so one of `convert`, `pdfa` or `pdfua` comes first when the template is not a PDF. Otherwise the response is a `400` that says `'letter.txt' is not a PDF`.

`pdfa` and `pdfua` run LibreOffice on whatever is in flight. `steps=pdfa` on an Office file converts it directly. `steps=convert,pdfa` converts to PDF, then opens that PDF in LibreOffice and exports it again, so prefer `steps=pdfa`.

### Watermark and stamp fields

`watermarkSource` is `text`, `image` or `pdf`. `text` takes its text from `watermarkExpression`. `image` and `pdf` take a file uploaded under the `watermark` field. A stamp has the same fields with the `stamp` prefix.

`watermarkPages` is a page selection such as `1-3,5,7-`. `watermarkOptions` is a JSON object whose values are all strings. `{"op": 0.5}` is a `400`, and `{"op": "0.5"}` works.

| Option | Default | Meaning |
|---|---|---|
| `pos` | `c` | `tl`, `tc`, `tr`, `c`, `bl`, `bc`, `br`. `t` is `tc` and `b` is `bc` |
| `op` | `1` | Opacity, 0 to 1 |
| `rot` | `0` | Rotation in degrees |
| `sc` | `0.5` | Scale relative to the page, 0.01 to 1 |
| `fontsize` | `24` | Font size of a text mark |
| `fillc` | `#808080` | Colour of a text mark, as `#rrggbb` |

```bash test
curl -F template=@report.docx -F steps=convert,stamp \
  -F stampSource=text -F stampExpression=PAID \
  -F 'stampOptions={"pos":"tr","op":"0.5","fontsize":"12"}' -F stampPages=1 \
  http://localhost:9027/v1/render -o paid.pdf
head -c4 paid.pdf | grep -q %PDF
```

Repeat the `Source`, `Expression`, `Options` and `Pages` fields for several marks, up to 5 of each kind. Name the step once: each name in `steps` applies every mark of that kind, so `steps=watermark,watermark` with two sources draws four.

### How the fields are read

- Fields of a step you did not name have no effect, but they are still checked. `-F rotateAngle=45` is a `400` even without `rotate` in `steps`.
- The order of `steps` decides the order of work. The fields only give each step its options.
- `encrypt` goes last. A later PDF step gets a `400` that says the file is encrypted.
- `sign` must be the last step, because any later step would change the signed bytes, and it cannot be combined with `encrypt` in either order. Both are refused with a `400` before any work starts. Without a signing key configured, a request with `sign` gets a `503`, as does a `level` that needs a timestamp authority the service does not have; a caller not allowed the key gets `403`. After a `split`, each part is signed.

```bash
curl -F template=@invoice.docx -F data=@data.json -F steps=convert,sign \
  -F reason=Issued http://localhost:9027/v1/render -o invoice.pdf
```
- A request runs at most 50 steps.

## Several documents in flight

`split` turns one document into several, and every later step runs on each. `merge` joins them again. If several remain at the end, the response is a ZIP.

```bash test
curl -F template=@a.pdf -F files=@b.pdf \
  -F steps=merge,split,rotate \
  -F splitMode=intervals -F splitSpan=1 -F rotateAngle=180 \
  -F outputFilename=parts \
  http://localhost:9027/v1/render -D headers.txt -o parts.zip
head -c2 parts.zip | grep -q PK
grep -qi 'filename="parts.zip"' headers.txt
```

Here the ZIP holds `parts_1.pdf` and `parts_2.pdf`, numbered from 1. One request may have at most 1,000 documents in flight.

`merge` takes PDFs only. It does not convert the further files, so convert them first with [`/v1/convert`](/docs/guides/convert/) or list them already as PDFs.

To render one template once per element of an array, use [batch rendering](/docs/templates/batch/).

## What comes back

| Status | Body |
|---|---|
| `200` | The file. `content-type` follows its extension. Several documents come as `application/zip` |
| `204` | Nothing, because the request named a [webhook](/docs/guides/webhooks/) and the result goes there |
| `4xx`, `5xx` | JSON, as in [errors](/docs/reference/errors/) |

The file is named after `outputFilename`, or else after the template, with the extension of what came out. `invoice.docx` through `convert` is `invoice.pdf`.

## Response headers

| Header | On | Meaning |
|---|---|---|
| `x-request-id` | Every response | The id of this request, also in the logs and on webhook deliveries. Send your own `X-Request-Id` to set it |
| `content-disposition` | Files | `attachment; filename="..."` |
| `x-fonts-used` | PDF results of up to 32 MB | The fonts named in the PDF, such as `LiberationSans, LiberationSans-Bold`. Check it to catch a font that fell back. See [fonts](/docs/guides/fonts/) |
| `server-timing` | Files | Milliseconds per stage, such as `template;dur=18, convert;dur=67` |
| `x-rusticdoc-version` | Authenticated routes, and every route when no credentials are configured | The service version |
| `x-content-type-options` | Every response | `nosniff` |

```bash test
curl -F template=@invoice.docx -F "data=<invoice.json" -F steps=convert \
  -H "X-Request-Id: order-2041" \
  http://localhost:9027/v1/render -D headers.txt -o invoice.pdf
grep -qi "^x-request-id: order-2041" headers.txt
grep -qi "^x-fonts-used:" headers.txt
```

## Sending a hash

Each template sent as a file is kept in this replica's memory for a while, under the caller's identity. A later request from the same caller can send `templateHash` in place of the file. The hash is the hex SHA-256 of the file, which [`/v1/templates/validate`](/docs/templates/validate/) returns as `template_hash`.

A miss answers `404 template_not_cached`, and the client sends the file again. Nothing guarantees a hit: the cache is per replica, holds templates up to 16 MB and empties on restart. To keep a template, [store it](/docs/guides/stored-templates/).
