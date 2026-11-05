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
| `POST /v1/convert`, `POST /forms/libreoffice/convert` | Convert one or more files; several come back as a ZIP, or one PDF with `merge=true` |
| `POST /v1/pdf/merge`, `/v1/pdf/encrypt` and `/forms/pdfengines/*` | PDF operations |
| `GET /health`, `/ready`, `/version`, `/metrics`, `/openapi.json`, `/docs` | Service |

Every response carries `x-request-id`; the same id is in the logs and in webhook deliveries. Errors are JSON: `{"error": "invalid_request", "message": "..."}`.

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
