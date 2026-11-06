# rusticdoc

A document engine in Rust: fills Office and PDF templates from JSON with `{{customer.name}}` tags, converts the result with LibreOffice, and finishes it with PDF operations. One multipart request does the whole job; webhooks deliver results asynchronously; the OpenAPI description feeds the SDKs.

It takes templating from Carbone and conversion and PDF handling from Gotenberg, and covers documents only: no HTML or URL rendering through a browser. It ships as one hardened image with no Node, Java or Chromium. LibreOffice does the conversions, driven through a small Python bridge, and qpdf and pdfcpu back the PDF stage alongside the built-in engine. The design is in [rusticdoc: a document engine in Rust](https://claude.ai/artifact/4ZBnc3L7jTH1ov5TKH8qs3), and what is done against it is in [docs/ROADMAP.md](docs/ROADMAP.md).

**Status: pre-release.** The templating, PDF, service and CLI work described here is built and tested. Generated Office files have only been opened in LibreOffice so far, not in Microsoft Office. [Known limits](#known-limits) lists this and the rest of what has not been checked.

## Run it

```bash
docker build -t rusticdoc .
docker run -d --name rusticdoc --read-only --cap-drop ALL --security-opt no-new-privileges \
  --tmpfs /tmp --tmpfs /home/rusticdoc:uid=1001,gid=1001 -p 127.0.0.1:3000:3000 rusticdoc
```

`deploy/compose.yaml` does the same with memory, process and temporary file limits. With no credentials configured the service answers anyone and warns at every start, so set `RUSTICDOC_API_KEYS` before exposing it. See [Security](#security).

Fill a template and get a PDF:

```bash
printf 'Dear {{ customer.name }},\nTotal: {{ total | currency("KES") }}\n' > letter.txt
curl -F template=@letter.txt -F 'data={"customer":{"name":"Ada"},"total":1500}' -F steps=convert \
  http://localhost:3000/v1/render -o letter.pdf
```

Convert an upload, with Gotenberg's path:

```bash
curl -F files=@report.docx http://localhost:3000/forms/libreoffice/convert -o report.pdf
```

The API description is at `http://localhost:3000/openapi.json`. `RUSTICDOC_ENABLE_DOCS=true` also serves an interactive page at `/docs`; it is off by default because it loads a third-party script from a CDN on the API's origin, under a Content-Security-Policy that limits what the page can do.

## Templates

Tags are `{{ customer.name }}`, with `{% for %}`, `{% if %}` and filters. They work in `.docx`, `.odt`, `.xlsx`, `.ods`, `.pptx`, `.odp`, in PDF form fields and annotations, and in text formats (`.txt`, `.md`, `.html`, `.csv`, `.xml`, `.json`, `.svg`). A tag split across runs by Word still works, and a loop around a table row or a paragraph repeats that row or paragraph.

Text filters include `currency("KES")`, `date` and `t` for translations. These filters put richer content in Office templates:

| Filter | Example | Puts in |
|---|---|---|
| `image` | `{{ logo \| image(width=120) }}` | A picture from base64 or a data URL. Never fetched from a URL |
| `barcode` | `{{ sku \| barcode("code128") }}` | `qr`, `code128`, `code39`, `ean13` or `ean8` |
| `link` | `{{ url \| link("Open") }}` | A hyperlink |
| `color` | `{{ name \| color("#c00") }}` | Coloured text, with `background=` too |
| `html` | `{{ body \| html }}` | A safe subset of HTML as native formatting |
| `chart` | `{{ rows \| chart(type="bar", x="month", y=["sales"]) }}` | A native `bar`, `column`, `line` or `pie` chart, in DOCX, PPTX and XLSX |
| `pagebreak` | `{{ "" \| pagebreak }}` | A page break |

`POST /v1/templates/validate` or `rusticdoc validate` lists every problem in a template against sample data, with where it sits.

## PDF forms

Send a `.pdf` to `/v1/render` as the template, or use `/v1/pdf/forms/fill`, to fill its form fields from `data`, including tags written in field values and annotations. `flatten=true` makes the result uneditable. `/v1/pdf/forms/fields` lists what a PDF has, and `/v1/pdf/overlay` lays one PDF over another. `/v1/pdf/sign` places a signature image; it is not a cryptographic signature.

## Fonts

Mount fonts at `/fonts` and call `POST /v1/fonts/reload`: no rebuild, no restart, and running conversions finish on the old set. `GET /v1/fonts` lists what is available. A request can also carry its own `fonts` files, which apply to that request only. The `x-fonts-used` response header names the fonts a converted document used.

## API

| Endpoint | Does |
|---|---|
| `POST /v1/render` | Template plus `data` JSON, then `steps` in order: `convert`, `pdfa`, `pdfua`, `merge`, `split`, `watermark`, `stamp`, `rotate`, `flatten`, `optimize`, `bookmarks`, `metadata`, `embed`, `factur-x`, `encrypt` |
| `POST /v1/render/batch` | One template, once per element of an array in the data, returned as a ZIP |
| `POST /v1/templates/validate` | Reports template errors against sample data without converting |
| `POST /v1/convert` | Convert one or more files; several come back as a ZIP, or one PDF with `merge=true` |
| `POST /v1/pdf/*` | `merge`, `split`, `rotate`, `flatten`, `optimize`, `encrypt`, `convert`, `watermark`, `stamp`, `overlay`, `sign`, `embed`, `factur-x`, `metadata/read`, `metadata/write`, `bookmarks/read`, `bookmarks/write`, `forms/fields`, `forms/fill` |
| `GET /v1/fonts`, `POST /v1/fonts/reload` | List the fonts and pick up new ones |
| `POST /forms/libreoffice/convert`, `POST /forms/pdfengines/*` | Gotenberg's API, see below |
| `GET /health`, `/ready`, `/openapi.json` | Open. With credentials configured, `/health` and `/ready` show only the status to a caller without them |
| `GET /metrics` | Open with no credentials configured; otherwise behind them unless `RUSTICDOC_METRICS_PUBLIC=true` |
| `GET /v1/version` | Behind authentication |
| `GET /docs` | Only with `RUSTICDOC_ENABLE_DOCS=true` |
| `GET /`, `/favicon.ico`, `/version`, `/debug`, `/prometheus/metrics` | Gotenberg's service routes, behind authentication; `/debug` needs `API_ENABLE_DEBUG_ROUTE` |

Every response carries `x-request-id`; the same id is in the logs and in webhook deliveries. Errors on `/v1/*` are JSON: `{"error": "invalid_request", "message": "..."}`. A request's own mistakes say what they were; an engine's failure reads as the status's name, here and in webhook deliveries, with the detail in the log under the request id.

### Gotenberg's paths

`/forms/*` answers as Gotenberg 8.39 does, so its clients move over by changing the image: every route takes many PDFs and answers with one file or a ZIP, with its file naming (`<input>.pdf` from LibreOffice, zero-based `<stem>_<n>.pdf` split parts, `Gotenberg-Output-Filename`), and errors are `text/plain` with its exact strings instead of JSON. `Gotenberg-Trace` is the correlation id in the logs and on webhook deliveries, and `Gotenberg-Version` is on every authenticated response. Gotenberg's webhook headers work on these routes, including `Gotenberg-Webhook-Events-Url`. `test/parity.sh` replays Gotenberg's own scenarios against both. Scenarios that select PDFtk or expect Chromium in the output differ, because this build has neither; naming `pdftk` stops the service from starting rather than failing a request later.

### Webhooks

Add `X-Rusticdoc-Webhook-Url` to any request. The service answers `204` at once and posts the result to that URL, retrying three times with backoff. Failures go to `X-Rusticdoc-Webhook-Error-Url` as JSON. Gotenberg's `Gotenberg-Webhook-*` headers are accepted too. Private addresses are refused unless `RUSTICDOC_WEBHOOK_ALLOW_PRIVATE=true`.

At most `RUSTICDOC_MAX_ASYNC_JOBS` jobs run at once, and `RUSTICDOC_MAX_ASYNC_JOBS_PER_CALLER` for one caller; past that a request gets `429` before its body is read. A job holds its place through its delivery attempts.

With `RUSTICDOC_WEBHOOK_SECRET` set, deliveries and events are signed, and each caller signs with its own key, so one tenant cannot send another's receiver something that verifies:

- The key is `HMAC-SHA256(secret, principal)`. The principal is `key:<hex SHA-256 of the API key>`, `basic:<hex SHA-256 of the user>` or `oidc:<hex SHA-256 of the token's subject>`. With no credentials configured the principal is `public`, and the key is the secret itself.
- `rusticdoc webhook-secret --api-key <key>` (or `--basic-user`, `--oidc-subject`), run with `RUSTICDOC_WEBHOOK_SECRET` set, prints a caller's key in hex. A receiver decodes the hex and verifies with those bytes. With no caller named it prints the secret itself, in hex.
- `X-Rusticdoc-Signature` is `v1=` and the hex HMAC-SHA256 of `<timestamp>.<event>.<request id>.<body>`, using `X-Rusticdoc-Timestamp`, `X-Rusticdoc-Event` and `X-Request-Id` from the same delivery. Compare in constant time and reject stale timestamps.

## CLI

The same binary works without the service:

```bash
rusticdoc render letter.txt --data data.json --steps convert -o letter.pdf
rusticdoc convert report.docx deck.pptx --merge -o bundle.pdf
rusticdoc validate invoice.docx --data sample.json
rusticdoc pdf merge a.pdf b.pdf -o both.pdf
rusticdoc engines          # which engines this machine has
rusticdoc openapi          # the API description
rusticdoc serve            # the service
```

`rusticdoc pdf` has `merge`, `split`, `rotate`, `flatten`, `optimize`, `encrypt`, `metadata`, `bookmarks`, `embed`, `watermark`, `stamp`, `sign`, `facturx`, `fields`, `fill` and `overlay`; `--help` shows each one's options. The CLI renders templates in its own process, because there the template is yours. `serve` isolates them.

## Configuration

Environment variables, all optional:

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_BIND` | `0.0.0.0:3000` | Listen address |
| `RUSTICDOC_API_TIMEOUT` | `30s` | Per request, including conversion; over it returns 503 |
| `RUSTICDOC_BODY_LIMIT` | `64MB` | Largest upload |
| `RUSTICDOC_WORKERS` | `2` | Concurrent LibreOffice conversions |
| `RUSTICDOC_API_KEYS` | none | Comma-separated keys for `X-Api-Key` or `Authorization: Bearer`. With no authentication configured the service answers anyone, as Gotenberg's clients expect, and warns at every start. Half-configured authentication stops it from starting |
| `RUSTICDOC_BASIC_AUTH_USERNAME`, `_PASSWORD` | none | Basic auth |
| `RUSTICDOC_WEBHOOK_SECRET` | none | Secret that callers' signing keys derive from. Unset, deliveries are not signed, and the service warns at start |
| `RUSTICDOC_WEBHOOK_ALLOW_PRIVATE` | `false` | Allow webhooks to private addresses |
| `RUSTICDOC_DOWNLOAD_FROM_ALLOW_PRIVATE` | `false` | The same for `downloadFrom` |
| `RUSTICDOC_MAX_INFLIGHT_BYTES` | `2GB` | Request bodies held in memory at once. Each request is charged what it declares, or the whole body limit if it streams; past the budget it waits, and gets `503` with `Retry-After` at the API timeout. One caller, meaning a credential or with none configured an address, holds at most half |
| `RUSTICDOC_MAX_ASYNC_JOBS`, `RUSTICDOC_MAX_ASYNC_JOBS_PER_CALLER` | `32`, `8` | Webhook jobs running at once, in all and per caller |
| `RUSTICDOC_BATCH_TIMEOUT` | `5m` | Overall limit for `/v1/render/batch` and a multi-file `/v1/convert`, synchronous or not |
| `RUSTICDOC_MAX_FORM_PARTS` | `1000` | File parts one form may hold; text parts are capped at 2,000 |
| `RUSTICDOC_MAX_RENDER_BYTES` | `64MB` | Most a text template, `batchName` or PDF form tag may write. Office templates stop at the package limit of 256 MiB |
| `RUSTICDOC_RENDER_MODE` | `isolated` under `serve` | `isolated` evaluates client templates in a worker process; `in-process` does not, and one template can then take the service down |
| `RUSTICDOC_RENDER_MEMORY` | `1GB` | Address space of one render worker. A template past it gets `413` |
| `RUSTICDOC_RENDER_WORKERS` | `4` | Render workers at once, so peak memory for rendering is this times the above |
| `RUSTICDOC_RENDER_WORKER_BIN` | the running binary | The binary with the worker in it |
| `RUSTICDOC_HEADER_TIMEOUT` | `10s` | How long a connection may wait for a complete set of request headers, including a silent new one or an idle keep-alive one |
| `RUSTICDOC_STALL_TIMEOUT` | `30s` | How long a connection may stay blocked on its client: a response nobody reads, or a body a handler waits for that does not arrive. Time a handler spends working does not count. A slow body that keeps moving is allowed up to the request time limit |
| `RUSTICDOC_RESPONSE_WRITE_TIMEOUT` | `5m` | How long writes may spend blocked in all, so a reader taking a little at a time, below the stall timeout, still loses its connection. It starts over once everything sent has left the send queue, so pipelining does not reset it |
| `RUSTICDOC_MAX_CONNECTIONS` | `1024` | Connections served at once; more wait in the listen queue |
| `RUSTICDOC_MAX_CONNECTIONS_PER_PEER` | `0` (off) | Connections at once from one IPv4 address or IPv6 /64, since one client usually holds a whole /64; an IPv4-mapped address counts as its IPv4 address. More are closed. Leave it off behind a load balancer, where every connection has the same peer, and rate limit at the proxy instead |
| `RUSTICDOC_OUTBOUND_PROXY` | none | Proxy for webhooks and `downloadFrom`. Proxy variables in the environment are otherwise ignored. With this set, DNS pinning no longer applies, because the proxy resolves names itself: it must enforce its own egress policy, including blocking private ranges. The URL and its resolved addresses are still checked first, and the service warns at start |
| `RUSTICDOC_METRICS_PUBLIC` | `false` | Keep `/metrics` open when credentials are configured |
| `RUSTICDOC_ENABLE_DOCS` | `false` | Serve `/docs`, which loads a third-party script |
| `RUSTICDOC_CORS_ORIGINS` | none | Origins allowed cross-origin access, comma-separated, or `*` |
| `RUSTICDOC_LIBREOFFICE_BIN`, `RUSTICDOC_QPDF_BIN` | auto-detected | Engine paths; `none` disables one |
| `RUSTICDOC_FONTS_DIR` | `/fonts` | Mounted fonts, picked up by LibreOffice |
| `RUSTICDOC_LOG_FORMAT`, `RUSTICDOC_LOG_LEVEL` | `json`, `info` | Logging |

Gotenberg's variable names are accepted as well. These decide how its routes behave:

| Variable | Default | Purpose |
|---|---|---|
| `LIBREOFFICE_DISABLE_ROUTES`, `PDFENGINES_DISABLE_ROUTES` | `false` | The routes answer 404, on `/v1/*` as on `/forms/*`; conversion steps in `/v1/render` refuse too |
| `PDFENGINES_ENCRYPT_ENGINES` | `qpdf`, then `pdfcpu` when there is no qpdf | Naming `pdfcpu` passes passwords as command-line arguments, which this user's other processes can read while it runs, and the service warns at start. A failed encryption never falls through to the next engine |
| `PDFENGINES_MERGE_ENGINES`, `_SPLIT_`, `_FLATTEN_`, `_ENCRYPT_`, `_ROTATE_`, `_STAMP_`, `_WATERMARK_`, `_EMBED_`, `_EMBED_METADATA_`, `_OPTIMIZE_IMAGES_`, `_READ_BOOKMARKS_`, `_WRITE_BOOKMARKS_`, `_READ_METADATA_`, `_WRITE_METADATA_`, `_FACTUR_X_` | qpdf first for merge, flatten and encrypt; lopdf first otherwise | Engine order per operation, from `qpdf`, `pdfcpu`, `lopdf` and `qpdf-metadata`. Naming `pdftk` or `exiftool`, which this build lacks, stops the service from starting |
| `API_ENABLE_DEBUG_ROUTE`, `GOTENBERG_BUILD_DEBUG_DATA` | `false`, `true` | `/debug` and what it lists |
| `API_TRACE_HEADER`, `API_CORRELATION_ID_HEADER` | `Gotenberg-Trace` | The correlation id header |
| `API_DISABLE_HEALTH_CHECK_ROUTE_TELEMETRY`, `API_DISABLE_ROOT_ROUTE_TELEMETRY`, `API_DISABLE_DEBUG_ROUTE_TELEMETRY`, `API_DISABLE_VERSION_ROUTE_TELEMETRY`, `PROMETHEUS_DISABLE_ROUTE_TELEMETRY` (or `*_LOGGING`) | `true` | Leave the route out of the access log |
| `PROMETHEUS_NAMESPACE`, `PROMETHEUS_METRICS_PATH`, `PROMETHEUS_DISABLE_COLLECT` | `gotenberg`, `/prometheus/metrics`, `false` | Gotenberg's metrics route; `/metrics` is separate |
| `WEBHOOK_ENABLE_SYNC_MODE`, `WEBHOOK_DISABLE` | `false` | Answer only after the delivery; ignore the webhook headers, on every route |
| `WEBHOOK_ALLOW_LIST`, `WEBHOOK_DENY_LIST`, `WEBHOOK_ERROR_ALLOW_LIST`, `WEBHOOK_ERROR_DENY_LIST`, `API_DOWNLOAD_FROM_ALLOW_LIST`, `API_DOWNLOAD_FROM_DENY_LIST` | none | Comma-separated regular expressions, matched against the URL with its host normalised, on every route. Deny wins. A refusal is `403`. They add to the refusal of private addresses and never relax it, and an invalid one stops the service from starting |
| `API_ENABLE_OIDC_AUTH`, `API_OIDC_ISSUER`, `API_OIDC_AUDIENCE`, `API_OIDC_JWKS_URL` | off | Accept bearer tokens from an OpenID Connect provider on the protected routes |

## SDKs

Generated from `openapi.json` with `make sdk-typescript`, `make sdk-python` or `make sdk-go`. See [sdk/README.md](sdk/README.md).

## Develop

```bash
make test      # unit and integration tests, no LibreOffice needed
make lint      # rustfmt and clippy
make run       # the service on 127.0.0.1:3000 with pretty logs
make openapi   # refresh openapi.json; CI checks it is current
make docker    # the image
```

The workspace has four crates: `rusticdoc-template` (the template engine), `rusticdoc-core` (pipeline, engines, the render worker), `rusticdoc-server` (axum API, auth, webhooks, metrics, OpenAPI) and `rusticdoc` (the CLI and `serve`). Tests run without LibreOffice or qpdf; conversion is exercised by the CI image smoke test.

Three checks need Docker and the image:

```bash
make parity      # Gotenberg's own scenarios against this image and the official one
make outbound    # crafted documents must reach no network and read no local file
make isolation   # secrets stay out of worker processes, and /proc stays closed
```

## Security

What the service does by itself:

- **Client templates run in a worker process.** It has no environment, limits on memory, CPU, files and core dumps, and is the kernel's first choice under memory pressure. A template that exhausts its limit gets `413`, and the service keeps running. The memory limit relies on Linux; on macOS it has not been shown to hold.
- **Input is capped before it expands.** That covers request bodies held in memory, form parts, page ranges, split output, stamp text, template output, PDF tags and webhook jobs. The settings are in the table above.
- **Secrets stay out of children.** LibreOffice, Python, qpdf and pdfcpu start from an empty environment, and on Linux the service closes its own `/proc` entries to them. Do not run the image with `docker run --init`, which puts a process back at PID 1 that exposes the environment. Anyone who can `docker exec` into the container can still read it.
- **Documents cannot reach out.** LibreOffice is configured to fetch nothing, and its web and CMIS content providers are removed from the image. `make outbound` checks this across formats.
- **Webhooks and `downloadFrom` refuse private addresses**, pin the address they checked, and follow no redirects.

What it leaves to you:

- Set `RUSTICDOC_API_KEYS`, basic auth or OpenID Connect, and `RUSTICDOC_WEBHOOK_SECRET`. Half-configured authentication stops the service from starting.
- Rate limit at a reverse proxy. The per-caller and connection limits bound what one client holds, but many clients together can still fill them for as long as the timeouts allow.
- Terminate TLS in front of it. The service speaks plain HTTP.
- Rebuild the image regularly. A scan finds CVEs in Debian's packages, and those with a Debian fix arrive by rebuilding. `make docker` uses the digests pinned in the `Dockerfile`, which dependabot keeps fresh.

A compromised LibreOffice still runs as the service's user, in the service's container. It cannot read the service's secrets or memory, but it can see other requests' temporary files while they exist. Treat the container as the boundary between tenants that do not trust each other, and run one per tenant if that matters.

## Known limits

- Generated DOCX, XLSX and PPTX files have been opened in LibreOffice, not in Word, Excel or PowerPoint, so a repair prompt there has not been ruled out.
- Barcodes have not been scanned with a reader.
- Filled PDF forms have not been viewed in Chrome, Acrobat or Preview.
- There is no golden-PDF corpus: output is checked by content, not compared byte for byte across architectures.
- Repeating a whole sheet in XLSX or ODS is not supported, only rows within one. Charts are not supported in ODF templates.
- `data` counts against the render memory limit once parsed. At the default limit, a list of 8 million small numbers rendered and one of 16 million got `413`, so raise `RUSTICDOC_RENDER_MEMORY` for data that size.

## Licence

Not chosen yet. Dependencies are MIT, Apache-2.0 or BSD; LibreOffice is MPL and runs as a separate process.
