---
title: Roadmap
description: Where each part of rusticdoc stands, what comes next, and the API gaps worth closing.
sidebar:
  order: 2
---

This page shows you what is built, which of its proofs have been met, what comes next and which API gaps are open. None of what comes next has a date.

## Where it stands

All six phases are built. Each has a gate. Phase 0's is met, phase 4's is met on a Mac, phases 1 and 3 are met in part, and phase 2's has not been run.

| Phase | Scope | State | Gate | Gate met? |
|---|---|---|---|---|
| 0 | Service, CLI, webhooks, OpenAPI, metrics, auth, image, LibreOffice conversion | Built | The unit and integration tests pass on every commit, with the linter and a dependency audit | Yes |
| 1 | DOCX and ODT templates: tags across split runs, loops on rows and paragraphs, filters, translations, validation | Built | A corpus renders byte-identical to its golden PDFs on amd64 and arm64 | Partly. `make golden` renders 14 outputs on both: the Office and text outputs and the PDF form the service fills itself are byte-identical. LibreOffice's PDFs are not, since its layout moves text by hundredths of a point between processors: seven match as page images, one in layout within 0.25 pt |
| 2 | XLSX, PPTX, ODS, ODP templates and batch rendering to ZIP | Built, except repeating a whole sheet | Third-party XLSX and PPTX examples reproduce with this syntax | Not run |
| 3 | PDF templates: form fields, tags in annotations, overlay on a base PDF | Built, with Latin text only where the service draws it | Filled forms show their values in Chrome, Acrobat Reader and Preview | Chrome and Preview yes. Acrobat Reader not run |
| 4 | Images, barcodes, native charts, data-bound colours, signature images, an HTML subset | Built, except charts in ODF | Word and LibreOffice open every output without a repair prompt | Yes on a Mac, for the Office-made corpus: Word, Excel and PowerPoint 16.114 and LibreOffice open it with no repair prompt. Barcodes decode with ZXing and ZBar, and have not been scanned with a reader |
| 5 | Fewer external engines | Built in part. `lopdf` is the first engine for most PDF operations. The in-process DOCX path is declined | Per engine, the PDF tests pass with it first | `lopdf` yes. DOCX declined |

## What is next

1. Open the output in Office for Windows, filled forms and signed files in Adobe Acrobat, and scan the barcodes with a hardware or phone reader.
2. Reproduce third-party XLSX and PPTX examples.
3. Repeat whole sheets in XLSX and ODS, and add charts to ODF.
4. Embed fonts for text drawn into PDFs, so that Arabic and CJK values show.
5. Report fonts a template uses that the service lacks, such as Word's default Aptos, and strip macros and OLE objects from templates.
6. Add an RFC 3161 timestamp to signatures so they reach PAdES B-T.

## Cryptographic signatures

`signature=certificate` on `/v1/pdf/sign`, the `sign` step and `rusticdoc pdf sign --key --cert` sign with one key per deployment, PAdES B-B style. Signed files validate in poppler's `pdfsig` 26.09 (valid, total document signed) and pyHanko 0.38.1 (`VALID`, also in its AdES report), and the unit tests verify each signature without the signer's code. They have not been opened in Adobe Acrobat. Timestamps (B-T), revocation data (B-LT, B-LTA), keys per caller or in an HSM, and certification signatures are not built.

## API gaps worth closing

The store made these possible, and none is built:

1. **Template versions or aliases,** so that an application asks for `invoice` rather than an id that changes with every edit.
2. **`GET /v1/jobs/{id}`, with results kept for a time.** Webhooks need a receiver the caller can host, and polling does not.
3. **`GET /v1/templates/{id}/variables`,** listing the tags a template uses, so that an application can build or check its data.
4. **A JSON request body for `POST /v1/render`,** since a render by id has no files to send.
5. **Admin rights for basic auth and OpenID Connect users,** and tenants that survive replacing a key.
6. **`POST /v1/templates/{id}/preview`,** rendering sample data to an image.

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
