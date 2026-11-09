---
title: Roadmap
description: Where each part of rusticdoc stands, which of its proofs are still open, and what comes next.
sidebar:
  order: 2
---

## Where it stands

All six phases are built. Each one has a gate, which is a test that proves it. Phases 0 and 4 have met theirs, phases 1 and 3 have met theirs in part, and phase 2's hasn't been run.

| Phase | Scope | State | Gate | Gate met? |
|---|---|---|---|---|
| 0 | Service, CLI, webhooks, background jobs, OpenAPI, metrics, auth, image, LibreOffice conversion | Built | The unit and integration tests pass on every commit, along with the linter and a dependency audit | Yes |
| 1 | DOCX and ODT templates: tags across split runs, loops on rows and paragraphs, filters, translations, validation | Built | A corpus renders byte-identical to its golden PDFs on amd64 and arm64 | Partly. `make golden` renders 14 outputs on both. The Office and text outputs and the PDF form the service fills itself are byte-identical. LibreOffice's PDFs aren't, because its layout moves text by hundredths of a point between processors. Seven of them match as page images, and one in layout within 0.25 pt |
| 2 | XLSX, PPTX, ODS, ODP templates, whole-sheet loops, nested row loops and batch rendering to ZIP | Built | Third-party XLSX and PPTX examples reproduce with this syntax | Not run |
| 3 | PDF templates: form fields, tags in annotations, text in any script, overlay and underlay on a base PDF | Built | Filled forms show their values in Chrome, Acrobat Reader and Preview | Preview yes, for Latin and other scripts. Chrome yes for Latin text, not checked for other scripts. Acrobat Reader not run |
| 4 | Images, barcodes, native charts in every format, colours, signature images, an HTML subset, removing elements on a condition | Built | Word and LibreOffice open every output without a repair prompt | Yes on a Mac for the Office-made corpus, and for removed elements in Word, Excel and PowerPoint. Barcodes decode with rxing and ZBar, but haven't been scanned with a reader |
| 5 | Fewer external engines | Built in part. `lopdf` is the first engine for most PDF operations, and Fulgur lays out HTML without a browser. The in-process DOCX path is declined | Per engine, the PDF tests pass with it first | `lopdf` yes. DOCX declined |

## What is next

None of this has a date.

1. Open the output in Office for Windows, filled forms and signed files in Adobe Acrobat, filled forms in other scripts in Chrome, and scan the barcodes with a hardware or phone reader.
2. Reproduce third-party XLSX and PPTX examples, which is phase 2's gate.
3. Run a load test, and give sizing figures measured under load rather than worked out.
4. Publish the image and make the generated SDKs compile.
5. Renew archive timestamps on B-LTA signatures, and support keys in a cloud KMS.

## Cryptographic signatures

`signature=certificate` on `/v1/pdf/sign`, the `sign` step and `rusticdoc pdf sign` all sign at any level from PAdES B-B to B-LTA, with an RFC 3161 timestamp from an authority you trust, revocation data in a DSS, and a document timestamp. They can also certify with DocMDP, place a visible signature, sign with named keys that are each limited to their own callers, and use keys held in a PKCS#11 token. Signed files validate in poppler's `pdfsig` and in pyHanko at every level, including with keys on a SoftHSM token. They haven't been opened in Adobe Acrobat.

## Declined: in-process DOCX to PDF

Converting DOCX without LibreOffice was evaluated with `office2pdf` 0.10.0, and it isn't worth taking on:

| | `office2pdf` 0.10.0 | Warm LibreOffice pool |
|---|---|---|
| Time per document | 280 to 920 ms | About 20 ms |
| Size | 76 MB binary | Already in the image |
| Fidelity | Scrambled the text of a real DOCX | Reference |

It would be slower, larger and less accurate, so LibreOffice stays. It's worth another look if a Rust renderer matures.

## What stays open in security

[Security](/docs/operate/security/) lists the risks that remain, including rate limiting, what a compromised LibreOffice can see, and the CVE scan.
