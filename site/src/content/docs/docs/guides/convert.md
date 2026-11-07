---
title: Convert documents
description: Convert one or more uploaded documents with LibreOffice, and merge or finish the result with PDF options.
sidebar: { order: 2 }
---

`POST /v1/convert` converts files with LibreOffice and no templating. After this page you can convert one file or several, merge them into one PDF, and choose the export options.

```bash test
curl -F files=@report.docx http://localhost:9027/v1/convert -o report.pdf
head -c4 report.pdf | grep -q %PDF
```

## Formats

`to` sets the output format. The default is `pdf`.

| `to` | Output |
|---|---|
| `pdf` | PDF |
| `docx`, `odt` | Word processing |
| `xlsx`, `ods` | Spreadsheets |
| `pptx`, `odp` | Presentations |
| `html`, `txt` | Web page, plain text. `txt` is for word processing files: a spreadsheet or presentation gets `500 engine_failed` |

```bash test
curl -F files=@sheet.xlsx -F to=ods http://localhost:9027/v1/convert -o sheet.ods
head -c2 sheet.ods | grep -q PK
```

Any other value of `to`, such as `rtf` or `png`, is a `400`.

A conversion that writes more than `RUSTICDOC_LIBREOFFICE_MAX_DISK` to disk, 512 MB by default, is stopped and answers `400`, so one document cannot fill the temporary directory. The other conversions carry on.

```bash test
curl -F files=@invoice.docx -F to=txt http://localhost:9027/v1/convert -o invoice.txt
grep -q 'Total due' invoice.txt
```

LibreOffice decides what can become what. A pair it cannot do, such as `report.docx` to `xlsx` or a PDF to `docx`, answers `500 engine_failed`, and the reason is in the log under the request id. See [errors](/docs/reference/errors/).

Inputs are chosen by extension. The list has about 130 entries, including the Office and OpenDocument families, `rtf`, `csv`, `txt`, `html`, `xml`, `epub`, `pdf` and common image formats. Anything else, such as `.json` or `.md`, is a `400` that says `is not a file type this service converts`.

HTML goes through LibreOffice, not a browser. There is no route that renders a URL.

## Several files

Send `files` more than once. Several files come back as a ZIP, one output per input.

```bash test
curl -F files=@report.docx -F files=@deck.pptx \
  http://localhost:9027/v1/convert -D headers.txt -o converted.zip
head -c2 converted.zip | grep -q PK
grep -qi 'filename="converted.zip"' headers.txt
```

The ZIP holds `report.pdf` and `deck.pdf`. `outputFilename` names the ZIP, and the entries keep their own names.

A request with one file has the usual time limit, `RUSTICDOC_API_TIMEOUT`, 30 seconds by default. A request with several has `RUSTICDOC_BATCH_TIMEOUT` over all of them, 5 minutes by default, and each file still has the usual limit.

## Merge

`merge=true` converts each file to PDF, then joins them in the order sent.

```bash test
curl -F files=@report.docx -F files=@deck.pptx -F merge=true -F outputFilename=bundle \
  http://localhost:9027/v1/convert -D headers.txt -o bundle.pdf
head -c4 bundle.pdf | grep -q %PDF
grep -qi 'filename="bundle.pdf"' headers.txt
```

Without `outputFilename` the result takes the first file's name. Merging needs `to=pdf`: `merge=true` with `to=docx` is a `400`.

## Conversion options

These apply when the output is a PDF, and LibreOffice ignores them otherwise. Except for `pdfa`, `quality` and `maxImageResolution`, they are booleans: `true` or `1`, `false` or `0`. Anything else is a `400`.

| Field | Default | What it does |
|---|---|---|
| `pdfa` | None | Exports as `PDF/A-1b`, `PDF/A-2b`, `PDF/A-3b` or `PDF/A-4` |
| `pdfua` | `false` | Exports as PDF/UA |
| `landscape` | `false` | Turns every page to landscape first |
| `nativePageRanges` | All | Pages to export, such as `1-3,5` |
| `password` | None | Opens a protected input. A wrong password, or one sent with an unprotected file, is a `500 engine_failed` |
| `exportFormFields` | `true` | Exports form fields as PDF form fields |
| `allowDuplicateFieldNames` | `false` | Allows two form fields with one name |
| `exportBookmarks` | `true` | Exports headings as an outline |
| `exportBookmarksToPdfDestination` | `false` | Exports bookmarks as named destinations |
| `exportPlaceholders` | `false` | Exports placeholder fields |
| `exportNotes` | `false` | Exports comments as PDF annotations |
| `exportNotesPages` | `false` | Exports speaker notes pages (presentations) |
| `exportOnlyNotesPages` | `false` | Exports only the notes pages (presentations) |
| `exportNotesInMargin` | `false` | Exports comments in the margin (Writer) |
| `exportHiddenSlides` | `false` | Includes hidden slides |
| `convertOooTargetToPdfTarget` | `false` | Rewrites links to OpenDocument targets as PDF targets |
| `exportLinksRelativeFsys` | `false` | Exports links to files as relative |
| `skipEmptyPages` | `false` | Leaves out automatically inserted blank pages |
| `addOriginalDocumentAsStream` | `false` | Embeds the source document in the PDF |
| `singlePageSheets` | `false` | One page per sheet (spreadsheets) |
| `losslessImageCompression` | `false` | Compresses images without loss |
| `quality` | `90` | JPEG quality, 1 to 100 |
| `reduceImageResolution` | `false` | Lowers image resolution to `maxImageResolution` |
| `maxImageResolution` | `300` | `75`, `150`, `300`, `600` or `1200` DPI |

```bash test
curl -F files=@report.docx -F pdfa=PDF/A-2b -F nativePageRanges=1 \
  http://localhost:9027/v1/convert -o report.pdf
grep -aq pdfaid report.pdf
```

`/forms/libreoffice/convert` reads more fields than this, such as `initialView`, `updateIndexes` and `nativeWatermarkText`. `/v1/convert` ignores them, and it does not refresh indexes and tables of contents before exporting, while `/forms/libreoffice/convert` does by default. See [moving from Gotenberg 8](/docs/guides/from-gotenberg/).

## Finish the result in one request

`/v1/convert` also runs PDF steps from fields you send, in this fixed order after conversion: merge, split, watermark, stamp, rotate, flatten, optimize, bookmarks, metadata, embed, Factur-X, encrypt.

| To run | Send |
|---|---|
| Split | `splitMode` and `splitSpan`, optionally `splitUnify` |
| Watermark, stamp | `watermarkSource` or `stampSource`, with the fields in [the pipeline guide](/docs/guides/pipeline/#watermark-and-stamp-fields) |
| Rotate | `rotateAngle`, optionally `rotatePages` |
| Flatten | `flatten=true` |
| Optimize | `optimizeImages=true`, or any `imageQuality`. The value is not used on this route |
| Bookmarks, metadata | `bookmarks`, `metadata` as JSON |
| Embed | `embeds` files, optionally `embedsMetadata` |
| Factur-X | `facturxXml` file, optionally `facturxConformanceLevel` |
| Encrypt | `userPassword` or `ownerPassword` |

```bash test
curl -F files=@report.docx \
  -F watermarkSource=text -F watermarkExpression=DRAFT \
  -F 'metadata={"Title":"Quarterly report"}' -F userPassword=open-sesame \
  http://localhost:9027/v1/convert -o report.pdf
head -c4 report.pdf | grep -q %PDF
```

These steps need a PDF, so with `to=odt` they get a `400`. To choose your own order, use [`/v1/render`](/docs/guides/pipeline/).

## Fonts and other parts of the form

`fonts` files apply to this request alone. See [fonts](/docs/guides/fonts/). `downloadFrom` works here as in [the pipeline guide](/docs/guides/pipeline/#form-fields). A [webhook](/docs/guides/webhooks/) header works here too.

The response headers are the same as on [`/v1/render`](/docs/guides/pipeline/#response-headers).
