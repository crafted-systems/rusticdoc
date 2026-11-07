---
title: Roadmap
description: Where each part of rusticdoc stands, what comes next, and the API gaps worth closing.
sidebar:
  order: 2
---

This page shows you what is built, which of its proofs have been met, what comes next and which API gaps are open. None of what comes next has a date.

## Where it stands

All six phases are built. Each has a gate, and only phase 0's has been met. The others need a corpus or an application the project has not run yet.

| Phase | Scope | State | Gate | Gate met? |
|---|---|---|---|---|
| 0 | Service, CLI, webhooks, OpenAPI, metrics, auth, image, LibreOffice conversion, Gotenberg's routes | Built | The office replay matches the official image on the same 310 of 326 scenarios | Yes |
| 1 | DOCX and ODT templates: tags across split runs, loops on rows and paragraphs, filters, translations, validation | Built | A corpus renders byte-identical to its golden PDFs on amd64 and arm64 | No. The corpus does not exist |
| 2 | XLSX, PPTX, ODS, ODP templates and batch rendering to ZIP | Built, except repeating a whole sheet | Carbone's published XLSX and PPTX examples reproduce with the new syntax | Not run |
| 3 | PDF templates: form fields, tags in annotations, overlay on a base PDF | Built, with Latin text only where the service draws it | Filled forms show their values in Chrome, Acrobat Reader and Preview | Not run |
| 4 | Images, barcodes, native charts, data-bound colours, signature images, an HTML subset | Built, except charts in ODF | Word and LibreOffice open every output without a repair prompt | LibreOffice only. Word not run, and barcodes never scanned with a reader |
| 5 | Fewer external engines | Built in part. `lopdf` is the first engine for most PDF operations. The in-process DOCX path is declined | Per engine, the parity suite | `lopdf` yes. DOCX declined |

## What is next

1. Open generated DOCX, XLSX and PPTX in Word, Excel and PowerPoint, and fix anything that prompts a repair.
2. View filled forms in Chrome, Acrobat Reader and Preview, and scan the barcodes with a reader.
3. Build the golden-PDF corpus, and reproduce Carbone's published examples.
4. Repeat whole sheets in XLSX and ODS, and add charts to ODF.
5. Embed fonts for text drawn into PDFs, so that Arabic and CJK values show.
6. Report fonts a template uses that the service lacks, and strip macros and OLE objects from templates.

## API gaps worth closing

The store made these possible, and none is built:

1. **Template versions or aliases,** so that an application asks for `invoice` rather than an id that changes with every edit.
2. **`GET /v1/jobs/{id}`, with results kept for a time.** Webhooks need a receiver the caller can host, and polling does not.
3. **`GET /v1/templates/{id}/variables`,** listing the tags a template uses, so that an application can build or check its data.
4. **A JSON request body for `POST /v1/render`,** since a render by id has no files to send.
5. **Admin rights for basic auth and OpenID Connect users,** and tenants that survive replacing a key.
6. **`POST /v1/templates/{id}/preview`,** rendering sample data to an image.

## Parity with Gotenberg

`make parity` replays Gotenberg 8.39's own scenarios against this image and the official one. 310 of 326 behave the same. The other 16 are expected:

- 10 select PDFtk, which this build leaves out.
- 6 expect Chromium in the output of `/health`, `/debug` and the metrics route.

## Declined: in-process DOCX to PDF

Converting DOCX without LibreOffice was evaluated with `office2pdf` 0.10.0 and is not worth taking:

| | `office2pdf` 0.10.0 | Warm LibreOffice pool |
|---|---|---|
| Time per document | 280 to 920 ms | About 20 ms |
| Size | 76 MB binary | Already in the image |
| Fidelity | Scrambled the text of a real DOCX | Reference |

It would be slower, larger and less accurate, so LibreOffice stays. Look again if a Rust renderer matures.

## What stays open in security

[Security](/docs/operate/security/) lists the risks that remain, including rate limiting, what a compromised LibreOffice can see, and the CVE scan.
