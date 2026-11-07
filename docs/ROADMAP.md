# Roadmap

The design lives in [rusticdoc: a document engine in Rust](https://claude.ai/artifact/4ZBnc3L7jTH1ov5TKH8qs3). This is its delivery plan and where each part stands.

All six phases are built. Each phase has a gate, and only phase 0's has been met; the others need a corpus or an application this project has not run yet.

| Phase | Scope | State | Gate | Gate met? |
|---|---|---|---|---|
| 0 | Service, CLI, webhooks, OpenAPI, metrics, auth, image, LibreOffice conversion, Gotenberg's routes | Built | The office replay matches the official image on the same 310 of 326 scenarios | **Yes**, see below |
| 1 | DOCX and ODT templates: tags across split runs, loops on rows and paragraphs, filters, translations, validation | Built | A corpus renders byte-identical to its golden PDFs on amd64 and arm64 | No: the corpus does not exist |
| 2 | XLSX, PPTX, ODS, ODP templates and batch rendering to ZIP | Built, except repeating a whole sheet | Carbone's published XLSX and PPTX examples reproduce with the new syntax | Not run |
| 3 | PDF templates: form fields, tags in annotations, overlay on a base PDF | Built, with Latin text only where the service draws it | Filled forms show their values in Chrome, Acrobat Reader and Preview | Not run |
| 4 | Images, barcodes, native charts, data-bound colours, signature images, an HTML subset | Built, except charts in ODF | Word and LibreOffice open every output without a repair prompt | LibreOffice only; Word not run, and barcodes never scanned |
| 5 | Fewer external engines | Built in part: `lopdf` is the first engine for most PDF operations. The in-process DOCX path is declined | Per engine, the parity suite | `lopdf` yes; see below for DOCX |

## Parity with Gotenberg

`make parity` replays Gotenberg 8.39's own scenarios against this image and the official one. On the image from the security work, 310 of 326 behave the same. The other 16 are expected:

- 10 select PDFtk, which this build leaves out.
- 6 expect Chromium in the output of `/health`, `/debug` and the metrics route.

## In-process DOCX to PDF: declined

Phase 5 proposed converting DOCX without LibreOffice. It was evaluated with `office2pdf` 0.10.0 and is not worth taking:

| | `office2pdf` 0.10.0 | Warm LibreOffice pool |
|---|---|---|
| Time per document | 280 to 920 ms | About 20 ms |
| Size | 76 MB binary | Already in the image |
| Fidelity | Scrambled the text of a real DOCX | Reference |

It would be slower, larger and less accurate, so LibreOffice stays. Look again if a Rust renderer matures.

## Security

An OWASP review found 20 issues, 6 of them High, and all are fixed. Automated review of those fixes then found more gaps, which are fixed too. The full-size attacks were run in a container limited to 3 GiB:

| Attack | Before | After |
|---|---|---|
| 40 doublings of a template variable | Service killed, out of memory | `413` in under a second, service up |
| 40 concurrent 60 MB uploads | Service killed, out of memory | Refused cleanly at a 1.31 GiB peak, service up |
| 100 MB value printed in a loop | Service killed, out of memory | `400`, service up |
| 840,000 same-named form parts | No answer in 150 s | `400` in 6 ms |
| Document that makes LibreOffice write without stopping | 512 MB of `/tmp` filled in under 15 s | `400` at the disk limit, space freed, other conversions unaffected |
| Stylesheet link in a converted HTML file | Fetched by LibreOffice | No request leaves the container |
| Service secrets in `/proc/1/environ` | Readable by LibreOffice | Permission denied |

`make outbound` and `make isolation` keep the last two true, and `make disk` the row above them. Still open:

- Many clients together can fill the connection and memory budgets for as long as the timeouts allow. Rate limiting at a proxy is the answer, and the README says so.
- A compromised LibreOffice runs as the service's user. It cannot read the service's secrets or memory, but it can see other requests' temporary files.
- The render memory limit has only been shown to hold on Linux.
- Trivy reports 691 findings in Debian packages, 1 critical and 108 high, none with a Debian fix at the time of the scan. One further advisory, with no severity, is in a module `pdfcpu` builds against; whether `pdfcpu` reaches it has not been checked.

## Next

1. Open generated DOCX, XLSX and PPTX in Word, Excel and PowerPoint, and fix anything that prompts a repair.
2. View filled forms in Chrome, Acrobat Reader and Preview, and scan the barcodes with a reader.
3. Build the golden-PDF corpus, and reproduce Carbone's published examples.
4. Repeat whole sheets in XLSX and ODS, and add charts to ODF.
5. Embed fonts for text drawn into PDFs, so Arabic and CJK values show.
6. Report fonts a template uses that the service lacks, and strip macros and OLE objects from templates.

## API gaps worth closing

The store made these possible, and none is built:

1. **Template versions or aliases**, so an application asks for `invoice` rather than an id that changes with every edit.
2. **`GET /v1/jobs/{id}`, with results kept for a time.** Webhooks need a receiver the caller can host; polling does not.
3. **`GET /v1/templates/{id}/variables`**, listing the tags a template uses, so an application can build or check its data.
4. **A JSON request body for `POST /v1/render`**, since a render by id has no files to send.
5. **Admin rights for basic auth and OpenID Connect users**, and tenants that survive replacing a key.
6. **`POST /v1/templates/{id}/preview`**, rendering sample data to an image.

## Conventions

- Every request is one multipart POST; `steps` lists the pipeline in order.
- Gotenberg's form paths behave as Gotenberg does, with their own handlers beside `/v1/*`, so its clients and the parity harness keep working.
- Engines sit behind the core's `Engines` and are found at start; an absent engine fails its step with 503, never the service.
- Nothing is stored until `RUSTICDOC_STORE` names a place. Everything written there is immutable, and replicas share nothing else.
- No attribution trailers in commits.
