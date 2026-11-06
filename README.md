# rusticdoc

A document engine in Rust: fills Office and PDF templates from JSON with `{{customer.name}}` tags, converts the result with LibreOffice, and finishes it with PDF operations. One multipart request does the whole job; webhooks deliver results asynchronously; the OpenAPI description feeds the SDKs.

It takes templating from Carbone and conversion and PDF handling from Gotenberg, covers documents only (no HTML or URL rendering), and ships as one hardened image with no Node, Java, Python or Chromium. The design is in [rusticdoc: a document engine in Rust](https://claude.ai/artifact/4ZBnc3L7jTH1ov5TKH8qs3); the plan in [docs/ROADMAP.md](docs/ROADMAP.md).

**Status: phase 0.** The service, CLI, webhooks, metrics and image work. Text templates (`.txt`, `.md`, `.html`, `.csv`, `.xml`, `.json`, `.svg`) render today; Office files are converted as they are; DOCX templating is phase 1. PDF operations so far: merge and encrypt.

## Run it

```bash
docker build -t rusticdoc .
docker run -d --name rusticdoc --read-only --cap-drop ALL --security-opt no-new-privileges \
  --tmpfs /tmp --tmpfs /home/rusticdoc:uid=1001,gid=1001 -p 3000:3000 rusticdoc
```

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

The interactive API description is at `http://localhost:3000/docs`.

## API

| Endpoint | Does |
|---|---|
| `POST /v1/render` | Template plus `data` JSON, then `steps` in order: `convert`, `pdfa`, `metadata`, `encrypt`, `merge` |
| `POST /v1/templates/validate` | Reports template errors against sample data without converting |
| `POST /v1/convert` | Convert one or more files; several come back as a ZIP, or one PDF with `merge=true` |
| `POST /v1/pdf/*` | PDF operations |
| `POST /forms/libreoffice/convert`, `POST /forms/pdfengines/*` | Gotenberg's API, see below |
| `GET /health`, `/ready`, `/v1/version`, `/metrics`, `/openapi.json`, `/docs` | Service, without authentication |
| `GET /`, `/favicon.ico`, `/version`, `/debug`, `/prometheus/metrics` | Gotenberg's service routes, behind authentication; `/debug` needs `API_ENABLE_DEBUG_ROUTE` |

Every response carries `x-request-id`; the same id is in the logs and in webhook deliveries. Errors on `/v1/*` are JSON: `{"error": "invalid_request", "message": "..."}`.

### Gotenberg's paths

`/forms/*` answers as Gotenberg 8.39 does, so its clients move over by changing the image: every route takes many PDFs and answers with one file or a ZIP, with its file naming (`<input>.pdf` from LibreOffice, zero-based `<stem>_<n>.pdf` split parts, `Gotenberg-Output-Filename`), and errors are `text/plain` with its exact strings instead of JSON. `Gotenberg-Trace` is the correlation id in the logs and on webhook deliveries, and `Gotenberg-Version` is on every authenticated response. Gotenberg's webhook headers work on these routes, including `Gotenberg-Webhook-Events-Url`; a delivery cannot set `Content-Disposition` or the other headers the service owns, which is the one known difference in behaviour. `test/parity.sh` replays Gotenberg's own scenarios against both.

### Webhooks

Add `X-Rusticdoc-Webhook-Url` to any request. The service answers `204` at once and posts the result to that URL, retrying three times with backoff. Failures go to `X-Rusticdoc-Webhook-Error-Url` as JSON. With `RUSTICDOC_WEBHOOK_SECRET` set, deliveries carry `X-Rusticdoc-Timestamp` and `X-Rusticdoc-Signature: v1=<hex HMAC-SHA256 of "<timestamp>.<body>">`; verify the signature and reject stale timestamps. Gotenberg's `Gotenberg-Webhook-*` headers are accepted too. Private addresses are refused unless `RUSTICDOC_WEBHOOK_ALLOW_PRIVATE=true`.

## CLI

The same binary works without the service:

```bash
rusticdoc render letter.txt --data data.json --steps convert -o letter.pdf
rusticdoc convert report.docx deck.pptx --merge -o bundle.pdf
rusticdoc engines          # which engines this machine has
rusticdoc openapi          # the API description
rusticdoc serve            # the service
```

## Configuration

Environment variables, all optional:

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_BIND` | `0.0.0.0:3000` | Listen address |
| `RUSTICDOC_API_TIMEOUT` | `30s` | Per request, including conversion; over it returns 503 |
| `RUSTICDOC_BODY_LIMIT` | `64MB` | Largest upload |
| `RUSTICDOC_WORKERS` | `2` | Concurrent LibreOffice conversions |
| `RUSTICDOC_API_KEYS` | none | Comma-separated keys for `X-Api-Key` or `Authorization: Bearer` |
| `RUSTICDOC_BASIC_AUTH_USERNAME`, `_PASSWORD` | none | Basic auth |
| `RUSTICDOC_WEBHOOK_SECRET` | none | HMAC key for deliveries |
| `RUSTICDOC_WEBHOOK_ALLOW_PRIVATE` | `false` | Allow webhooks to private addresses |
| `RUSTICDOC_CORS_ORIGINS` | none | Origins allowed cross-origin access, comma-separated, or `*` |
| `RUSTICDOC_LIBREOFFICE_BIN`, `RUSTICDOC_QPDF_BIN` | auto-detected | Engine paths; `none` disables one |
| `RUSTICDOC_FONTS_DIR` | `/fonts` | Mounted fonts, picked up by LibreOffice |
| `RUSTICDOC_LOG_FORMAT`, `RUSTICDOC_LOG_LEVEL` | `json`, `info` | Logging |

Gotenberg's variable names are accepted as well. These decide how its routes behave:

| Variable | Default | Purpose |
|---|---|---|
| `LIBREOFFICE_DISABLE_ROUTES`, `PDFENGINES_DISABLE_ROUTES` | `false` | The routes answer 404 |
| `PDFENGINES_MERGE_ENGINES`, `_SPLIT_`, `_FLATTEN_`, `_ENCRYPT_`, `_ROTATE_`, `_STAMP_`, `_WATERMARK_`, `_EMBED_`, `_EMBED_METADATA_`, `_OPTIMIZE_IMAGES_`, `_READ_BOOKMARKS_`, `_WRITE_BOOKMARKS_`, `_READ_METADATA_`, `_WRITE_METADATA_`, `_FACTUR_X_` | qpdf first for merge, flatten and encrypt; lopdf first otherwise | Engine order per operation, from `qpdf`, `pdfcpu`, `lopdf` and `qpdf-metadata`. Naming `pdftk` or `exiftool`, which this build lacks, stops the service from starting |
| `API_ENABLE_DEBUG_ROUTE`, `GOTENBERG_BUILD_DEBUG_DATA` | `false`, `true` | `/debug` and what it lists |
| `API_TRACE_HEADER`, `API_CORRELATION_ID_HEADER` | `Gotenberg-Trace` | The correlation id header |
| `API_DISABLE_HEALTH_CHECK_ROUTE_TELEMETRY`, `API_DISABLE_ROOT_ROUTE_TELEMETRY`, `API_DISABLE_DEBUG_ROUTE_TELEMETRY`, `API_DISABLE_VERSION_ROUTE_TELEMETRY`, `PROMETHEUS_DISABLE_ROUTE_TELEMETRY` (or `*_LOGGING`) | `true` | Leave the route out of the access log |
| `PROMETHEUS_NAMESPACE`, `PROMETHEUS_METRICS_PATH`, `PROMETHEUS_DISABLE_COLLECT` | `gotenberg`, `/prometheus/metrics`, `false` | Gotenberg's metrics route; `/metrics` is separate and always on |
| `WEBHOOK_ENABLE_SYNC_MODE`, `WEBHOOK_DISABLE` | `false` | Answer only after the delivery; ignore the webhook headers |
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

The workspace has three crates: `rusticdoc-core` (pipeline, template stage, engines), `rusticdoc-server` (axum API, auth, webhooks, metrics, OpenAPI) and `rusticdoc` (the CLI and `serve`). Tests run without LibreOffice or qpdf; conversion is exercised by the CI image smoke test.

## Licence

Not chosen yet. Dependencies are MIT, Apache-2.0 or BSD; LibreOffice is MPL and runs as a separate process.
