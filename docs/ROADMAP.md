# Roadmap

The design lives in [rusticdoc: a document engine in Rust](https://claude.ai/artifact/4ZBnc3L7jTH1ov5TKH8qs3). This is its delivery plan and where each part stands.

All six phases are built. Each phase has a gate. Phases 0 and 4 have met theirs, and phases 1, 2 and 3 have met theirs in part.

This file is the delivery record and stays in the repository. The site has no roadmap or status page: [STATUS.md](STATUS.md) says what was run and what was not, and `/docs/project/roadmap` redirects to known limits.

| Phase | Scope | State | Gate | Gate met? |
|---|---|---|---|---|
| 0 | Service, CLI, webhooks, background jobs, OpenAPI, metrics, auth, image, LibreOffice conversion | Built | The unit and integration tests pass on every commit, with the linter and a dependency audit | Yes |
| 1 | DOCX and ODT templates: tags across split runs, loops on rows and paragraphs, filters, translations, validation | Built | A corpus renders byte-identical to its golden PDFs on amd64 and arm64 | Partly. `make golden` renders 14 outputs on both: the Office and text outputs and the PDF form the service fills itself are byte-identical. LibreOffice's PDFs are not, since its layout moves text by hundredths of a point between processors: seven match as page images, one in layout within 0.25 pt |
| 2 | XLSX, PPTX, ODS, ODP templates, whole-sheet loops, nested row loops and batch rendering to ZIP | Built | Third-party XLSX and PPTX examples reproduce with this syntax | Partly. `make thirdparty` ports seven templates from xlsx-template and docxtemplater: six give what upstream expects, and one PowerPoint loop across two paragraphs gives the same text in four paragraphs where docxtemplater gives three. A list spread across columns and dates as Excel serial numbers had no equivalent when the gate first ran. `{% col for %}` and `excel_date` were added for them, and the port uses both |
| 3 | PDF templates: form fields, tags in annotations, text in any script, overlay and underlay on a base PDF | Built | Filled forms show their values in Chrome, Acrobat Reader and Preview | Preview and Chrome yes, for Latin and other scripts. Acrobat Reader not run |
| 4 | Images, barcodes, native charts in every format, colours, signature images, an HTML subset, removing elements on a condition | Built | Word and LibreOffice open every output without a repair prompt | Yes on a Mac for the Office-made corpus, and for removed elements in Word, Excel and PowerPoint. Barcodes decode with ZXing and ZBar, and have not been scanned with a reader |
| 5 | Fewer external engines | Built in part. `lopdf` is the first engine for most PDF operations, and Fulgur lays out HTML without a browser. The in-process DOCX path is declined | Per engine, the PDF tests pass with it first | `lopdf` yes. DOCX declined |

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

`make outbound` and `make isolation` keep the last two true, and `make disk` the row above them.

Since then, rendering takes macros, OLE objects, external links and private hyperlinks out of every Office output, failing closed on anything it cannot read with certainty, and automated review of each new feature (the sanitiser, sheet copies, previews, underlays and signing) found issues that are fixed with a test for each. Still open:

- Many clients together can fill the connection and memory budgets for as long as the timeouts allow. Rate limiting at a proxy is the answer, and the README says so.
- A compromised LibreOffice runs as the service's user. It cannot read the service's secrets or memory, but it can see other requests' temporary files.
- The render memory limit has only been shown to hold on Linux.
- Trivy reports 691 findings in Debian packages, 1 critical and 108 high, none with a Debian fix at the time of the scan. One further advisory, with no severity, is in a module `pdfcpu` builds against; whether `pdfcpu` reaches it has not been checked.

## Cryptographic signatures

`/v1/pdf/sign` with `signature=certificate`, the `sign` step on `/v1/render` and `rusticdoc pdf sign` sign from PAdES B-B to B-LTA (`level`): an incremental update with a `/ETSI.CAdES.detached` CMS signature, RSA (PKCS#1 v1.5, SHA-256) or ECDSA P-256, then for B-T an RFC 3161 timestamp from an authority pinned in `RUSTICDOC_SIGNING_TSA_CERT`, for B-LT a DSS with the chains and OCSP responses or CRLs, each verified, and for B-LTA a document timestamp. `certify` writes a DocMDP certification, the `sign` step can be visible, `RUSTICDOC_SIGNING_KEYS` names keys each with its allowed callers, and a key can live in a PKCS#11 token.

`signature=renew` on `/v1/pdf/sign` and `rusticdoc pdf sign --renew` renew the archive timestamp of a file signed at B-LTA: validation data for the last document timestamp goes into the DSS, then a new document timestamp covers the whole file. It uses no key, only the timestamp authority. A file that does not end in a trusted, unchanged document timestamp is refused. It is not a step on `/v1/render`, and nothing renews a file on a schedule.

| Check | Result |
|---|---|
| poppler `pdfsig` 26.09 | Valid at every level; the document timestamp covers the whole file. `make sign-check` repeats it |
| pyHanko 0.38.1, offline, revocation hard-fail | B-T, B-LT, B-LTA and a certified file at B-LTA, read from the file's own DSS |
| SoftHSM2 token | RSA at every level and ECDSA P-256 at B-LTA; a wrong certificate or PIN is refused |
| Unit tests | Signatures verified without the signer's code; forged OCSP answers, impostor issuers, untrusted or stale tokens and expired revocation data refused |
| Renewed file | `pdfsig` reports the new document timestamp over the whole file and the old one over its revision, and `openssl ts -verify` verifies both over their byte ranges. The signed file's bytes are a prefix of the renewed one, and a B-T file is refused. `make sign-check` repeats it. pyHanko was not run on a renewed file |
| Adobe Acrobat | Not run |

Not built: keys in a cloud KMS, encrypted key files.

## Office, viewers, barcodes, load and architectures

Checked on 7 November 2026 on a Mac with macOS 27. The third-party templates, other scripts in Chrome, the load test and the SDKs were checked on 10 November.

| Check | How | Result |
|---|---|---|
| Microsoft Office | `test/office`: templates typed in Word, Excel and PowerPoint 16.114.4 for Mac, filled, then opened in the same apps | No repair prompt, after three fixes: a stale calculation chain made Excel repair workbooks; a loop across two shapes on one slide printed the copies over each other; charts and pictures in unmoved placeholders went to the corner. `crates/template/tests/office.rs` pins all three |
| PDF viewers | A filled form, editable and flattened, in Chrome 154 and Preview. Then forms filled in Arabic, Hebrew, Chinese, Japanese, Korean and Cyrillic, editable and flattened, in Chrome 157 | Values, the check mark and the fields as expected in both, and those scripts draw correctly in both. Acrobat Reader not run |
| Barcodes | `make barcodes`: QR (a URL, and text with non-ASCII characters), Code 128, Code 39, EAN-13 and EAN-8, decoded by ZXing and ZBar | All six exact from the DOCX and from the PDF at 300 and 150 dpi. At 100 dpi the long Code 128 and Code 39 are too fine. No scanner tried |
| Third-party templates | `make thirdparty`: four workbooks from xlsx-template and three decks from docxtemplater, both MIT, ported by exact tag replacement, filled with upstream's data and compared with the cell values its tests assert or the paragraphs of its expected files | 31 of 32 checks. The difference is the paragraph loop in phase 2's row. The ports use a column loop where upstream loops across columns, `excel_date` where upstream converts dates, and `drop("paragraph")` after tag-only paragraphs. All seven outputs convert in LibreOffice and open in Excel and PowerPoint 16.114.4 for Mac with no repair prompt. `test/thirdparty/README.md` has the detail |
| Column loops in Excel | Two workbooks filled with `{% col for %}`, one with formulas that follow the copies, and the ported plan table, opened in Excel 16.114.4 for Mac | No repair prompt. The values, the widened `SUM` ranges and the dates are as expected |
| Load | `make load`: the sample invoice posted with ApacheBench at 1 to 16 connections to the arm64 image, on an M4 Pro with 8 CPUs and 5.78 GiB given to Docker | 136,330 requests, all `200`. DOCX to PDF runs at 16 a second on one connection and about 28 from four connections up, and to PDF/A at 8 to 14. Memory peaked at 249 MiB. One small document on one machine. `test/load/README.md` and the site's sizing page have the figures |
| SDKs | `make sdk`, then each client compiled and run through five calls against a local service with a store: version, template upload, render from the stored template, metadata write and read | TypeScript, Python and Go all compile and pass. The generated `render` sends JSON, so it needs a stored template. Not published |
| Architectures | `make golden`: 14 outputs from the arm64 image and the amd64 one under emulation | See phase 1. A render also differed between runs: new relationship files were written in a hash map's order, now fixed |

## Still open

None of these could be done from the machine the rest was checked on. Each needs software, hardware or an account it does not have, or is a decision to publish.

1. Open the output in Office for Windows.
2. Open filled forms, signed files and a renewed file in Adobe Acrobat, and validate a renewed file with pyHanko.
3. Scan the barcodes with a hardware or phone reader.
4. Make the image public and publish the SDK packages.
5. Support keys in a cloud KMS and encrypted key files.
6. Run the store against AWS itself and the manifest on a production cluster, and load test with larger documents on more than one machine.

## API gaps, closed

The store made these possible, and all are built: template aliases with versions, `GET /v1/jobs/{id}` with results kept for `RUSTICDOC_JOB_TTL`, `GET /v1/templates/{id}/variables`, a JSON body for `POST /v1/render`, admin rights for basic-auth and OpenID Connect users, tenants whose keys can be replaced, and `POST /v1/templates/{id}/preview`.

## Conventions

- Every request is one POST, multipart or, for a stored template, JSON; `steps` lists the pipeline in order. `Prefer: respond-async` runs it as a background job.
- Engines sit behind the core's `Engines` and are found at start; an absent engine fails its step with 503, never the service.
- Nothing is stored until `RUSTICDOC_STORE` names a place. Everything written there is immutable, and replicas share nothing else.
- No attribution trailers in commits.
