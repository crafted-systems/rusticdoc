# Roadmap

The design lives in [rusticdoc: a document engine in Rust](https://claude.ai/artifact/4ZBnc3L7jTH1ov5TKH8qs3). This is its delivery plan, with what exists today.

| Phase | Scope | Gate | State |
|---|---|---|---|
| 0 | Service skeleton: HTTP API, CLI, webhooks, OpenAPI, metrics, auth, Docker image; LibreOffice conversion and qpdf merge and encrypt; text templates | The office replay from gotenberg-slim matches the official image on the same 310 of 326 scenarios | Replay at 309 of the 310. The one short is `webhook.feature: Extra HTTP Headers`, which needs a client to set `Content-Disposition` on a delivery, a header the service reserves. The other 16 differ as gotenberg-slim's do: 10 select PDFtk, 6 list Chromium |
| 1 | DOCX and ODT templates: `{{ }}` tags across split runs, loops on rows and paragraphs, filters, translations, front matter, validation | A template corpus renders byte-identical to its golden PDFs on amd64 and arm64 | Not started |
| 2 | XLSX, PPTX, ODS, ODP templates and batch rendering to ZIP | Carbone's published XLSX and PPTX examples reproduce with the new syntax | Not started |
| 3 | PDF templates: form fields, tags in annotations, overlay on a base PDF | Filled forms show their values in Chrome, Acrobat Reader and Preview | Not started |
| 4 | Rich content: images, barcodes, native charts, data-bound colours, signatures, an HTML subset | Word and LibreOffice open every output without a repair prompt | Not started |
| 5 | Fewer external engines: `lopdf` where it passes parity, an in-process DOCX to PDF path | Per engine, the parity suite | Not started |

## Conventions that hold from phase 0

- Every request is one multipart POST; `steps` lists the pipeline in order.
- Gotenberg's form paths behave as Gotenberg does, with their own handlers beside `/v1/*`, so its clients and the parity harness keep working.
- Engines sit behind the core's `Engines` and are found at start; an absent engine fails its step with 503, never the service.
- Nothing is stored. Templates are cached by content hash in memory only.
- No attribution trailers in commits.
