# The render pipeline

<!-- Generated from site/src/content/docs/docs/guides/pipeline.md by site/scripts/generate-skills.mjs. Do not edit. -->

Fill a template from JSON and run conversion and PDF steps in one request to /v1/render.

`POST /v1/render` fills a template from JSON and runs a list of steps on the result. This request fills an invoice, converts it, sets its metadata and encrypts it:

```bash
curl -F template=@invoice.docx -F "data=<invoice.json" \
  -F steps=convert,metadata,encrypt \
  -F 'metadata={"Title":"Invoice INV-2041"}' \
  -F userPassword=open-sesame \
  http://localhost:9027/v1/render -o invoice.pdf
head -c4 invoice.pdf | grep -q %PDF
```

The body is a `multipart/form-data` form. For the tags themselves, see [template syntax](https://rusticdoc.com/docs/templates/syntax/).

## Send data as text or as a file

These two requests do the same thing:

```bash
# As a text field. The quotes stop your shell reading the <.
curl -F template=@invoice.docx -F "data=<invoice.json" \
  -F steps=convert -F to=html \
  http://localhost:9027/v1/render -o filled.html
grep -q "Amani" filled.html

# As a file part, with any file name, before or after the template.
curl -F data=@invoice.json -F template=@invoice.docx \
  -F steps=convert -F to=html \
  http://localhost:9027/v1/render -o filled.html
grep -q "Amani" filled.html
```

`ctx` works the same way. A `data` or `ctx` part that isn't valid JSON gets a `400` that names the file. Sending the same field twice, for example as text and as a file, is also a `400`.

A file named exactly `data.json` or `ctx.json` is read as that field, whichever field it arrives in. Once those are set aside, the first file part in the form is the template, whatever its field name.

## With no data and no steps, a document comes back unchanged

A form with an Office file or PDF, no `data` and no `steps` is a no-op. You get a `200` with the same bytes you sent.

```bash
curl -F template=@report.docx \
  http://localhost:9027/v1/render -D headers.txt -o same.docx
head -c2 same.docx | grep -q PK
grep -qi "report.docx" headers.txt
```

Tags in an Office file are only filled when `data` is a non-empty object, so `steps=convert` with no `data` converts the file with its tags as written. Text templates (`.txt`, `.md`, `.html`, `.htm`, `.csv`, `.xml`, `.json`, `.svg`) always render, so one with tags and no data gets a `422`.

A file in any other format gets `501 not_supported` if you send `data` with it, and passes through if you don't.

## Form fields

| Field | Kind | What it does |
|---|---|---|
| `template` | File | The template. The first file part in the form takes this role, whatever its field name |
| `templateId` | Text | The id of a [stored template](stored-templates.md). Sending it along with a `template` file part gets a `400` |
| `templateHash` | Text | The hex SHA-256 of a template this caller sent recently. See [sending a hash](#sending-a-hash) |
| `data` | Text, JSON object | What the template sees as its root. Anything other than an object gets a `400` |
| `ctx` | Text, JSON | Available in the template as `ctx`. If `data` has its own `ctx` key, `data` wins |
| `steps` | Text | Comma-separated step names, in order |
| `outputFilename` | Text | The result's file name, without an extension |
| `files` | Files | Extra PDFs for the `merge` step. Every file part after the template lands here, except the ones under the fields below |
| `fonts` | Files | Fonts for this request only. See [fonts](fonts.md) |
| `watermark`, `stamp` | Files | Image or PDF marks for those steps |
| `underlay` | File | The PDF that the `underlay` step lays the document over |
| `embeds` | Files | Attachments for the `embed` step |
| `facturxXml` | File | The invoice XML for the `factur-x` step |
| `flatten` | Text | With a PDF template and data, fixes the filled values into the page |
| `stripAnnotations` | Text | With a PDF template and data, `false` keeps the annotations that held tags. They're removed by default |
| `downloadFrom` | Text, JSON array | Fetches files from URLs instead of uploading them, as `[{"url": "..."}]`. At most 10 entries. Private addresses are refused unless `RUSTICDOC_DOWNLOAD_FROM_ALLOW_PRIVATE=true` |

The conversion options in [Convert documents](convert.md#conversion-options), like `landscape` and `nativePageRanges`, apply to every LibreOffice step.

A PDF sent with data is a form to fill (see [PDF templates](https://rusticdoc.com/docs/templates/pdf-templates/)).

## Steps

Steps run in the order `steps` lists them, each on the document or documents in flight. Step names aren't case-sensitive.

| Step | Fields | What it does |
|---|---|---|
| `convert` | `to`, default `pdf`: `pdf`, `docx`, `xlsx`, `pptx`, `odt`, `ods`, `odp`, `html`, `txt` | Converts with LibreOffice |
| `pdfa` | `pdfa`, default `PDF/A-2b`: `PDF/A-1b`, `PDF/A-2b`, `PDF/A-3b`, `PDF/A-4` | Exports as PDF/A with LibreOffice |
| `pdfua` | None | Exports as PDF/UA with LibreOffice |
| `merge` | `files` | Appends the extra PDFs, in the order sent. After a `split`, it joins the parts back together |
| `split` | `splitMode` (`pages` or `intervals`) and `splitSpan`, both required; `splitUnify` | Splits into several documents |
| `watermark` | `watermarkSource`, `watermarkExpression`, `watermarkOptions`, `watermarkPages`, `watermark` | Draws text, an image or a PDF under the page content |
| `stamp` | `stampSource`, `stampExpression`, `stampOptions`, `stampPages`, `stamp` | Draws text, an image or a PDF over the page content |
| `underlay` | `underlay`, one PDF | Lays each page over the matching page of the `underlay` PDF, like letterhead or a pre-printed form. See [below](#underlay) |
| `rotate` | `rotateAngle`, default `90`: `90`, `180`, `270`, `-90`; `rotatePages`, default all | Rotates pages |
| `flatten` | None | Fixes forms and annotations into the page content |
| `optimize` | None | Recompresses streams and drops unused objects |
| `bookmarks` | `bookmarks`, a JSON array of `{"title", "page", "children"}` | Replaces the outline |
| `metadata` | `metadata`, a JSON object like `{"Title": "Invoice"}` | Writes the metadata keys you send and keeps the others |
| `embed` | `embeds`; `embedsMetadata`, a JSON object mapping a file name to `{"mimeType": "..."}` | Attaches files |
| `factur-x` | `facturxXml`; `facturxConformanceLevel`, default `EN16931`: `MINIMUM`, `BASIC_WL`, `BASIC`, `EN16931`, `EXTENDED` | Embeds Factur-X invoice XML |
| `encrypt` | `userPassword`, `ownerPassword`, at least one of them. A missing owner password takes the user password | Encrypts the PDF |
| `sign` | `reason`, `location`, `contactInfo`, `signerName`, `level`, `certify`, `signingKey`, and for a visible signature `page`, `x`, `y`, `width` and an image under `signatureImage`, all optional | Signs the PDF cryptographically, the way `/v1/pdf/sign` does with `signature=certificate`. The signature is invisible, or shown as the image or a box of text at `x` and `y`. See [signing with a certificate](pdf-operations.md#with-a-certificate) |

Every step from `merge` down needs a PDF, so if the template isn't one, put `convert`, `pdfa` or `pdfua` first. Otherwise you get a `400` that says `'letter.txt' is not a PDF`.

`pdfa` and `pdfua` run LibreOffice on whatever is in flight. On an Office file, `steps=pdfa` converts it directly, while `steps=convert,pdfa` converts to PDF, then opens that PDF in LibreOffice and exports it again. Prefer `steps=pdfa`.

### Watermark and stamp fields

`watermarkSource` is `text`, `image` or `pdf`. With `text`, the text comes from `watermarkExpression`. With `image` or `pdf`, the mark is a file uploaded under the `watermark` field. A stamp has the same fields with the `stamp` prefix.

`watermarkPages` is a page selection like `1-3,5,7-`. `watermarkOptions` is a JSON object whose values are all strings, so `{"op": "0.5"}` works and `{"op": 0.5}` gets a `400`.

| Option | Default | Meaning |
|---|---|---|
| `pos` | `c` | `tl`, `tc`, `tr`, `c`, `bl`, `bc`, `br`. `t` means `tc` and `b` means `bc` |
| `op` | `1` | Opacity, 0 to 1 |
| `rot` | `0` | Rotation in degrees |
| `sc` | `0.5` | Scale relative to the page, 0.01 to 1 |
| `fontsize` | `24` | Font size of a text mark |
| `fillc` | `#808080` | Colour of a text mark, as `#rrggbb` |

```bash
curl -F template=@report.docx -F steps=convert,stamp \
  -F stampSource=text -F stampExpression=PAID \
  -F 'stampOptions={"pos":"tr","op":"0.5","fontsize":"12"}' -F stampPages=1 \
  http://localhost:9027/v1/render -o paid.pdf
head -c4 paid.pdf | grep -q %PDF
```

For several marks, repeat the `Source`, `Expression`, `Options` and `Pages` fields, up to 5 of each kind. Name the step only once. Each name in `steps` applies every mark of that kind, so `steps=watermark,watermark` with two sources draws four.

### Underlay

`underlay` puts a base PDF under the document in flight, page by page, the way [`/v1/pdf/overlay`](https://rusticdoc.com/docs/templates/pdf-templates/#overlay) does when the base is sent first. That lets one request fill an Office template, convert it and lay it on your letterhead:

```bash
curl -F template=@letter.txt -F "data=<letter.json" -F underlay=@a.pdf \
  -F steps=convert,underlay http://localhost:9027/v1/render -o on-letterhead.pdf
head -c4 on-letterhead.pdf | grep -q %PDF
```

Page 1 of the document is drawn over page 1 of the base, page 2 over page 2, and so on, and the result has the base's page sizes. Base pages past the end of the document stay as they are. Document pages past the end of the base are added at the end with nothing under them, so for letterhead on every page, send a base with at least as many pages.

The `underlay` file part is never taken as the template or as a file to merge, wherever it comes in the form. The step needs exactly one `underlay` PDF and a PDF in flight, so `convert` comes first for an Office or text template. Otherwise you get a `400`.

### How the fields are read

The order of `steps` decides the order of work, and the fields only give each step its options. Fields for a step you didn't name have no effect, but they're still checked, so `-F rotateAngle=45` gets a `400` even without `rotate` in `steps`. A request runs at most 50 steps.

`encrypt` goes last, because any later PDF step gets a `400` that says the file is encrypted.

`sign` must be the last step, because any later step would change the signed bytes, and it can't be combined with `encrypt` in either order. Either mistake is refused with a `400` before any work starts. If no signing key is configured, a request with `sign` gets a `503`, and so does a `level` that needs a timestamp authority the service doesn't have. A caller who isn't allowed the key gets a `403`. After a `split`, each part is signed.

```bash
curl -F template=@invoice.docx -F data=@data.json -F steps=convert,sign \
  -F reason=Issued http://localhost:9027/v1/render -o invoice.pdf
```

## Several documents in flight

`split` turns one document into several, and every later step runs on each of them until a `merge` joins them again. If several remain at the end, the response is a ZIP.

```bash
curl -F template=@a.pdf -F files=@b.pdf \
  -F steps=merge,split,rotate \
  -F splitMode=intervals -F splitSpan=1 -F rotateAngle=180 \
  -F outputFilename=parts \
  http://localhost:9027/v1/render -D headers.txt -o parts.zip
head -c2 parts.zip | grep -q PK
grep -qi 'filename="parts.zip"' headers.txt
```

Here the ZIP holds `parts_1.pdf` and `parts_2.pdf`, numbered from 1. A request can have at most 1,000 documents in flight.

`merge` only takes PDFs and doesn't convert the extra files, so convert them first with [`/v1/convert`](convert.md) or send them as PDFs to begin with.

To render one template once per element of an array, use [batch rendering](https://rusticdoc.com/docs/templates/batch/).

## What comes back

| Status | Body |
|---|---|
| `200` | The file, with a `content-type` that follows its extension. Several documents come back as `application/zip` |
| `204` | Nothing, because the request named a [webhook](webhooks.md) and the result goes there |
| `4xx`, `5xx` | JSON, as described in [errors](errors.md) |

The file is named after `outputFilename`, or after the template if you didn't send one, with the extension of what came out. `invoice.docx` put through `convert` comes back as `invoice.pdf`.

## Response headers

| Header | Sent on | Meaning |
|---|---|---|
| `x-request-id` | Every response | The id of this request, which also appears in the logs and on webhook deliveries. Send your own `X-Request-Id` to set it |
| `content-disposition` | Files | `attachment; filename="..."` |
| `x-fonts-used` | PDF results of up to 32 MB | The fonts named in the PDF, like `LiberationSans, LiberationSans-Bold`. Check it to catch a font that fell back. See [fonts](fonts.md) |
| `server-timing` | Files | Milliseconds per stage, like `template;dur=18, convert;dur=67` |
| `x-rusticdoc-version` | Authenticated routes, and every route when no credentials are configured | The service version |
| `x-content-type-options` | Every response | `nosniff` |

```bash
curl -F template=@invoice.docx -F "data=<invoice.json" -F steps=convert \
  -H "X-Request-Id: order-2041" \
  http://localhost:9027/v1/render -D headers.txt -o invoice.pdf
grep -qi "^x-request-id: order-2041" headers.txt
grep -qi "^x-fonts-used:" headers.txt
```

## Sending a hash

Each template you send as a file is kept in that replica's memory for a while, under the caller's identity, so a later request from the same caller can send `templateHash` in place of the file. The hash is the hex SHA-256 of the file, which [`/v1/templates/validate`](https://rusticdoc.com/docs/templates/validate/) returns as `template_hash`.

On a miss you get `404 template_not_cached` and send the file again. A hit is never guaranteed, because the cache is per replica, only holds templates up to 16 MB and empties on restart. To keep a template, [store it](stored-templates.md).
