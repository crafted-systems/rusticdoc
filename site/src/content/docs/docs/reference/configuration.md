---
title: Configuration
description: Every environment variable rusticdoc reads, with its default and purpose.
---

This page lists every environment variable the service reads. All of them are optional. With none set, the service starts and answers anyone, and warns about it.

## How values are read

| Type | Accepted | Not accepted |
|---|---|---|
| Size | `512KB`, `64MB`, `1GB` or a bare number of bytes. Case does not matter, and each unit is a multiple of 1024 | `2g`, `1.5GB`, `2GiB` |
| Duration | `500ms`, `30s`, `2m`, `1h`, or a number of seconds such as `1.5` | `1m30s` |
| `RUSTICDOC_*` boolean | `1`, `true` or `yes` is on. Anything else is off, including `TRUE` | |
| Gotenberg boolean | `1`, `t`, `T`, `TRUE`, `true`, `True`, and `0`, `f`, `F`, `FALSE`, `false`, `False` | Anything else is ignored, except that `API_ENABLE_BASIC_AUTH` and `API_ENABLE_OIDC_AUTH` stop the service |
| List | Comma-separated, with spaces trimmed | |

An empty variable counts as unset.

**A bad value does not always stop the service.** These settings stop it from starting, with a message that names the problem: `RUSTICDOC_STORE`, `RUSTICDOC_STORE_SCOPE`, `RUSTICDOC_ADMIN_KEYS`, `RUSTICDOC_API_KEYS`, `RUSTICDOC_BASIC_AUTH_USERNAME` and `RUSTICDOC_BASIC_AUTH_PASSWORD` when only one is set, `RUSTICDOC_RENDER_MODE`, `RUSTICDOC_MAX_TEMPLATE_BYTES`, `RUSTICDOC_MAX_TEMPLATES`, `RUSTICDOC_MAX_TEMPLATE_STORAGE`, `RUSTICDOC_MAX_STORED_FONTS`, `RUSTICDOC_MAX_FONT_STORAGE`, `RUSTICDOC_STORE_SYNC_INTERVAL`, a `PDFENGINES_*_ENGINES` list that names `pdftk`, `exiftool` or an unknown engine, an invalid regular expression in an allow or deny list, a trace header that is not a valid header name, and half-configured OpenID Connect.

Every other number, size or duration that the service cannot parse is ignored, and the default stays in place with no warning. `RUSTICDOC_RENDER_MEMORY=2g` leaves the limit at 1 GiB.

When a `RUSTICDOC_*` variable and a Gotenberg variable set the same thing, the `RUSTICDOC_*` one wins. `rusticdoc serve --bind` wins over both.

The image sets `RUSTICDOC_LIBREOFFICE_BIN`, `RUSTICDOC_QPDF_BIN`, `RUSTICDOC_PDFCPU_BIN`, `RUSTICDOC_FONTS_DIR` and `RUSTICDOC_LOG_FORMAT` for you, along with `TZ=UTC`, `LANG=C.UTF-8` and `LC_ALL=C.UTF-8`.

`deploy/compose.yaml` has variables of its own, which shape the container. [Docker](/docs/operate/docker/) lists them.

## Service

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_BIND` | `0.0.0.0:9027` | Listen address. `rusticdoc serve --bind` overrides it |
| `RUSTICDOC_ROOT_PATH` | `/` | Path prefix every route sits under. With `/docengine`, the health route is `/docengine/health` |
| `RUSTICDOC_CORS_ORIGINS` | None | Origins allowed cross-origin access, comma-separated, or `*` |
| `RUSTICDOC_ENABLE_DOCS` | `false` | Serve an interactive page at `/docs`. It is off by default because it loads a third-party script from a CDN on the API's origin |
| `RUSTICDOC_METRICS_PUBLIC` | `false` | Keep `/metrics` open when credentials are configured |
| `RUSTICDOC_LOG_FORMAT` | `json` | `json`, or `pretty` for a terminal |
| `RUSTICDOC_LOG_LEVEL` | `info` | A `tracing` filter such as `info` or `rusticdoc=debug` |

## Authentication

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_API_KEYS` | None | Comma-separated keys for `X-Api-Key` or `Authorization: Bearer`. A value with no key in it, such as `,`, stops the service from starting |
| `RUSTICDOC_BASIC_AUTH_USERNAME` | None | Basic auth user. Set it together with `RUSTICDOC_BASIC_AUTH_PASSWORD` |
| `RUSTICDOC_BASIC_AUTH_PASSWORD` | None | Basic auth password. Set it together with `RUSTICDOC_BASIC_AUTH_USERNAME` |
| `RUSTICDOC_ADMIN_KEYS` | None | Comma-separated keys that may upload and delete fonts. With credentials configured and none of these, nobody may |

OpenID Connect uses Gotenberg's variable names, listed [below](#authentication-with-gotenbergs-names). See [authentication](/docs/guides/authentication/).

## Requests and connections

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_API_TIMEOUT` | `30s` | Time limit for one request, including conversion. Past it, the request answers `503`. `deploy/compose.yaml` sets `60s` |
| `RUSTICDOC_BODY_LIMIT` | `64MB` | Largest request body |
| `RUSTICDOC_MAX_INFLIGHT_BYTES` | `2GB` | Request bodies held in memory at once. Each request is charged what it declares, or the whole body limit if it streams. Past the budget it waits, and gets `503` with `Retry-After` at the API timeout. One caller, meaning a credential or with none configured an address, holds at most half |
| `RUSTICDOC_MAX_FORM_PARTS` | `1000` | File parts one form may hold. Text parts are capped at 2,000, which you cannot change |
| `RUSTICDOC_BATCH_LIMIT` | `100` | Most elements one batch render may hold |
| `RUSTICDOC_BATCH_TIMEOUT` | `5m` | Overall limit for `/v1/render/batch` and a multi-file `/v1/convert`, synchronous or not |
| `RUSTICDOC_HEADER_TIMEOUT` | `10s` | How long a connection may wait for a complete set of request headers, including a silent new one or an idle keep-alive one |
| `RUSTICDOC_STALL_TIMEOUT` | `30s` | How long a connection may stay blocked on its client: a response nobody reads, or a body a handler waits for that does not arrive. Time a handler spends working does not count. A slow body that keeps moving is allowed up to the request time limit |
| `RUSTICDOC_RESPONSE_WRITE_TIMEOUT` | `5m` | How long writes may spend blocked in all, so a reader taking a little at a time, below the stall timeout, still loses its connection. It starts over once everything sent has left the send queue |
| `RUSTICDOC_MAX_CONNECTIONS` | `1024` | Connections served at once. More wait in the listen queue |
| `RUSTICDOC_MAX_CONNECTIONS_PER_PEER` | `0`, which is off | Connections at once from one IPv4 address or IPv6 /64. More are closed. Leave it off behind a load balancer, where every connection has the same peer, and rate limit at the proxy instead |

## Templates and rendering

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_RENDER_MODE` | `isolated` under `serve`, `in-process` otherwise | `isolated` evaluates client templates in a worker process. `in-process` does not, so one template can take the service down, and the service warns at start |
| `RUSTICDOC_RENDER_MEMORY` | `1GB` | Address space of one render worker. A template past it gets `413`. The limit relies on Linux |
| `RUSTICDOC_RENDER_WORKERS` | `4` | Render workers at once, so peak memory for rendering is this times `RUSTICDOC_RENDER_MEMORY` |
| `RUSTICDOC_RENDER_WORKER_BIN` | The running binary | The binary with the worker in it |
| `RUSTICDOC_MAX_RENDER_BYTES` | `64MB` | Most a text template, `batchName` or PDF form tag may write. Office templates stop at the package limit of 256 MiB |

[Sizing](/docs/operate/sizing/) shows how these add up.

## Engines and fonts

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_WORKERS` | `2` | LibreOffice conversions at once |
| `RUSTICDOC_LIBREOFFICE_MAX_JOBS` | `10` | Jobs per LibreOffice worker before it restarts |
| `RUSTICDOC_LIBREOFFICE_MAX_DISK` | `512MB` | Most one conversion may write to disk. Past it the worker restarts and the request gets `400`. Allow `RUSTICDOC_WORKERS` times this in `/tmp` |
| `RUSTICDOC_LIBREOFFICE_BIN` | The first of `/usr/lib/libreoffice/program/soffice.bin`, `/usr/bin/soffice`, `/Applications/LibreOffice.app/Contents/MacOS/soffice` | Path to LibreOffice. `none` disables it |
| `RUSTICDOC_PYTHON` | `python3` | The interpreter that runs the LibreOffice bridge |
| `RUSTICDOC_QPDF_BIN` | The first of `/usr/bin/qpdf`, `/opt/homebrew/bin/qpdf`, `/usr/local/bin/qpdf` | Path to qpdf. `none` disables it |
| `RUSTICDOC_PDFCPU_BIN` | The first of `/usr/bin/pdfcpu`, `/opt/homebrew/bin/pdfcpu`, `/usr/local/bin/pdfcpu` | Path to pdfcpu. `none` disables it |
| `RUSTICDOC_FONTS_DIR` | `/fonts` | Mounted fonts, picked up by LibreOffice |

The service does not search your `PATH` for engines.

## Store

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_STORE` | None | Where stored templates and uploaded fonts live: `s3://bucket/prefix` or `file:///path`. With none, those routes answer `501`. See [many containers](/docs/operate/many-containers/) |
| `RUSTICDOC_STORE_SCOPE` | `caller` | `caller`: each credential sees only its own templates, so replacing a key leaves its templates behind. `shared`: every credential reads and deletes the same ones |
| `RUSTICDOC_MAX_TEMPLATE_BYTES` | `50MB` | Largest template that can be stored |
| `RUSTICDOC_MAX_TEMPLATES` | `10000` | Most templates one scope may hold. Replicas storing at the same moment can pass this by what is in flight |
| `RUSTICDOC_MAX_TEMPLATE_STORAGE` | `5GB` | Most bytes of templates one scope may hold, with the same caveat |
| `RUSTICDOC_MAX_STORED_FONTS` | `2000` | Most fonts the store may hold |
| `RUSTICDOC_MAX_FONT_STORAGE` | `1GB` | Most bytes of fonts the store may hold. Every replica copies them all to disk |
| `RUSTICDOC_FONT_CACHE_DIR` | `rusticdoc-stored-fonts` in the temp directory | This replica's copy of the stored fonts. It is local, and rebuilt when the replica starts |
| `RUSTICDOC_STORE_SYNC_INTERVAL` | `30s` | How often a replica looks for fonts added or deleted elsewhere |

## S3 credentials and endpoint

With an `s3://` store, the S3 client reads these from the environment. Credentials never come from the URL. With no keys, it uses the pod's or instance's role.

These five are the ones `make replicas` and the deploy files use:

| Variable | Default | Purpose |
|---|---|---|
| `AWS_ACCESS_KEY_ID` | None | Access key |
| `AWS_SECRET_ACCESS_KEY` | None | Secret key |
| `AWS_REGION` | None | The bucket's region |
| `AWS_ENDPOINT` | AWS | Endpoint for any store other than AWS |
| `AWS_ALLOW_HTTP` | `false` | Allow a plain HTTP endpoint |

The client library also reads the rest of its `AWS_*` settings. They are passed through as they are and have not been exercised here:

| Variables | Purpose |
|---|---|
| `AWS_DEFAULT_REGION` | The region, under its other name |
| `AWS_ENDPOINT_URL`, `AWS_ENDPOINT_URL_S3`, `AWS_ENDPOINT_URL_STS` | Other names for the endpoint, and specific endpoints for S3 and STS |
| `AWS_SESSION_TOKEN`, `AWS_TOKEN` | A session token |
| `AWS_WEB_IDENTITY_TOKEN_FILE`, `AWS_ROLE_ARN`, `AWS_ROLE_SESSION_NAME` | A web identity role, as on EKS |
| `AWS_CONTAINER_CREDENTIALS_RELATIVE_URI`, `AWS_CONTAINER_CREDENTIALS_FULL_URI`, `AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE` | Container credentials |
| `AWS_METADATA_ENDPOINT`, `AWS_IMDSV1_FALLBACK` | The instance metadata service |
| `AWS_VIRTUAL_HOSTED_STYLE_REQUEST`, `AWS_S3_EXPRESS`, `AWS_UNSIGNED_PAYLOAD`, `AWS_CHECKSUM_ALGORITHM`, `AWS_SKIP_SIGNATURE`, `AWS_REQUEST_PAYER` | How requests are made |
| `AWS_COPY_IF_NOT_EXISTS`, `AWS_CONDITIONAL_PUT`, `AWS_DISABLE_TAGGING`, `AWS_DISABLE_BULK_DELETE` | What the store supports |
| `AWS_SERVER_SIDE_ENCRYPTION`, `AWS_SSE_KMS_KEY_ID`, `AWS_SSE_BUCKET_KEY_ENABLED`, `AWS_SSE_CUSTOMER_KEY_BASE64` | Server-side encryption |

The service sets the bucket from `RUSTICDOC_STORE`, so `AWS_BUCKET` has no effect.

## Webhooks and outbound requests

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_WEBHOOK_SECRET` | None | The secret that callers' signing keys derive from. Unset, deliveries are not signed, and the service warns at start |
| `RUSTICDOC_WEBHOOK_ATTEMPTS` | `3` | Attempts per delivery, including the first |
| `RUSTICDOC_WEBHOOK_ALLOW_PRIVATE` | `false` | Allow webhooks to private and loopback addresses |
| `RUSTICDOC_MAX_ASYNC_JOBS` | `32` | Webhook jobs running at once, across the service. Past it, a request gets `429` before its body is read |
| `RUSTICDOC_MAX_ASYNC_JOBS_PER_CALLER` | `8` | Webhook jobs running at once for one caller |
| `RUSTICDOC_DOWNLOAD_FROM_DISABLED` | `false` | Refuse the `downloadFrom` form field entirely |
| `RUSTICDOC_DOWNLOAD_FROM_MAX_ENTRIES` | `10` | Most URLs one request may fetch through `downloadFrom` |
| `RUSTICDOC_DOWNLOAD_FROM_ALLOW_PRIVATE` | `false` | Allow `downloadFrom` to fetch from private and loopback addresses |
| `RUSTICDOC_OUTBOUND_PROXY` | None | Proxy for webhooks and `downloadFrom`. Proxy variables in the environment are otherwise ignored. With this set, DNS pinning no longer applies, because the proxy resolves names itself: it must enforce its own egress policy, including blocking private ranges. The URL and its resolved addresses are still checked first, and the service warns at start |

## Gotenberg's variables

The service accepts Gotenberg's variable names, so a deployment can move over by changing the image. See [moving from Gotenberg](/docs/guides/from-gotenberg/).

### Settings with a rusticdoc equivalent

| Variable | Default | Same as |
|---|---|---|
| `API_PORT` | `9027` | The port in `RUSTICDOC_BIND` |
| `API_BIND_IP` | `0.0.0.0` | The address in `RUSTICDOC_BIND` |
| `API_ROOT_PATH` | `/` | `RUSTICDOC_ROOT_PATH` |
| `API_TIMEOUT` | `30s` | `RUSTICDOC_API_TIMEOUT` |
| `API_BODY_LIMIT` | `64MB` | `RUSTICDOC_BODY_LIMIT` |
| `LIBREOFFICE_BIN_PATH` | As for the rusticdoc variable | `RUSTICDOC_LIBREOFFICE_BIN` |
| `QPDF_BIN_PATH` | As for the rusticdoc variable | `RUSTICDOC_QPDF_BIN` |
| `PDFCPU_BIN_PATH` | As for the rusticdoc variable | `RUSTICDOC_PDFCPU_BIN` |
| `LOG_LEVEL` | `info` | `RUSTICDOC_LOG_LEVEL` |
| `LOG_FORMAT` | `json` | `RUSTICDOC_LOG_FORMAT`. `text` and `pretty` give pretty logs, and anything else JSON |
| `WEBHOOK_ALLOW_PRIVATE` | `false` | `RUSTICDOC_WEBHOOK_ALLOW_PRIVATE`. On for `true` or `1` |
| `API_DISABLE_DOWNLOAD_FROM` | `false` | `RUSTICDOC_DOWNLOAD_FROM_DISABLED`. On for `true` or `1` |
| `API_DOWNLOAD_FROM_MAX_ENTRIES` | `10` | `RUSTICDOC_DOWNLOAD_FROM_MAX_ENTRIES` |

`API_TIMEOUT` and `API_BODY_LIMIT` follow the size and duration rules above, which are narrower than Gotenberg's. `API_TIMEOUT=1m30s` is ignored. Write `90s`.

### Authentication with Gotenberg's names

| Variable | Default | Purpose |
|---|---|---|
| `API_ENABLE_BASIC_AUTH` | Off | Turn on basic auth, with the next two |
| `GOTENBERG_API_BASIC_AUTH_USERNAME` | None | Basic auth user |
| `GOTENBERG_API_BASIC_AUTH_PASSWORD` | None | Basic auth password |
| `API_ENABLE_OIDC_AUTH` | Off | Accept bearer tokens from an OpenID Connect provider on the protected routes |
| `API_OIDC_ISSUER` | None | Required with OpenID Connect. Must be `https`, or `http` on a loopback host |
| `API_OIDC_AUDIENCE` | None | Required with OpenID Connect |
| `API_OIDC_JWKS_URL` | Discovered from the issuer | Where the signing keys live, under the same rule as the issuer |

### Routes

| Variable | Default | Purpose |
|---|---|---|
| `LIBREOFFICE_DISABLE_ROUTES` | `false` | The conversion routes answer `404`, on `/v1/*` as on `/forms/*`. Conversion steps in `/v1/render` answer `503` |
| `PDFENGINES_DISABLE_ROUTES` | `false` | The PDF routes answer `404`, on `/v1/*` as on `/forms/*` |
| `API_ENABLE_DEBUG_ROUTE` | `false` | Serve `/debug` |
| `GOTENBERG_BUILD_DEBUG_DATA` | `true` | Fill `/debug` in |
| `API_TRACE_HEADER` | `Gotenberg-Trace` | The header that carries the correlation id |
| `API_CORRELATION_ID_HEADER` | `Gotenberg-Trace` | The older name for it. `API_TRACE_HEADER` wins |

### PDF engine order

Each of these takes a comma-separated list from `qpdf`, `pdfcpu`, `lopdf` and `qpdf-metadata`. Naming `pdftk` or `exiftool`, which this build lacks, or an unknown engine, stops the service from starting.

With no list set, `/forms/*` tries qpdf, pdfcpu, then `lopdf` for merge, qpdf then `lopdf` for flatten, qpdf then pdfcpu for encrypt, and `lopdf`, qpdf, pdfcpu for the rest. `/v1/*` tries `lopdf`, qpdf, pdfcpu for everything. A list you set applies to both.

| Variable | Operation |
|---|---|
| `PDFENGINES_MERGE_ENGINES` | Merge |
| `PDFENGINES_SPLIT_ENGINES` | Split |
| `PDFENGINES_FLATTEN_ENGINES` | Flatten |
| `PDFENGINES_ROTATE_ENGINES` | Rotate |
| `PDFENGINES_OPTIMIZE_IMAGES_ENGINES` | Optimize images |
| `PDFENGINES_ENCRYPT_ENGINES` | Encrypt. A failed encryption never falls through to the next engine. Naming `pdfcpu` passes passwords as command-line arguments, which this user's other processes can read while it runs, and the service warns at start |
| `PDFENGINES_WATERMARK_ENGINES` | Watermark |
| `PDFENGINES_STAMP_ENGINES` | Stamp |
| `PDFENGINES_EMBED_ENGINES` | Embed files |
| `PDFENGINES_EMBED_METADATA_ENGINES` | Metadata of embedded files |
| `PDFENGINES_READ_BOOKMARKS_ENGINES` | Read bookmarks |
| `PDFENGINES_WRITE_BOOKMARKS_ENGINES` | Write bookmarks |
| `PDFENGINES_READ_METADATA_ENGINES` | Read metadata |
| `PDFENGINES_WRITE_METADATA_ENGINES` | Write metadata |
| `PDFENGINES_FACTUR_X_ENGINES` | Factur-X |
| `PDFENGINES_CONVERT_ENGINES` | Accepted and checked, and shown in `/debug`. It changes no order |
| `PDFENGINES_ENGINES` | Accepted and checked, and shown in `/debug`. It changes no order |

The check also accepts `libreoffice-pdfengine` as a name in these lists.

### Access log

Each of these leaves its route out of the access log, and defaults to `true`. The older `*_LOGGING` name still works, and the `*_TELEMETRY` name wins.

| Variable | Older name | Route |
|---|---|---|
| `API_DISABLE_HEALTH_CHECK_ROUTE_TELEMETRY` | `API_DISABLE_HEALTH_CHECK_LOGGING` | `/health` and `/ready` |
| `API_DISABLE_ROOT_ROUTE_TELEMETRY` | `API_DISABLE_ROOT_ROUTE_LOGGING` | `/` |
| `API_DISABLE_DEBUG_ROUTE_TELEMETRY` | `API_DISABLE_DEBUG_ROUTE_LOGGING` | `/debug` |
| `API_DISABLE_VERSION_ROUTE_TELEMETRY` | `API_DISABLE_VERSION_ROUTE_LOGGING` | `/version` |
| `PROMETHEUS_DISABLE_ROUTE_TELEMETRY` | `PROMETHEUS_DISABLE_ROUTE_LOGGING` | The Gotenberg metrics route |

### Gotenberg's metrics route

| Variable | Default | Purpose |
|---|---|---|
| `PROMETHEUS_DISABLE_COLLECT` | `false` | Remove the route |
| `PROMETHEUS_NAMESPACE` | `gotenberg` | The prefix on its metric names |
| `PROMETHEUS_METRICS_PATH` | `/prometheus/metrics` | Its path. `/metrics` is separate and always has the `rusticdoc_` names |

### Webhooks and downloads

| Variable | Default | Purpose |
|---|---|---|
| `WEBHOOK_ENABLE_SYNC_MODE` | `false` | Answer only after the delivery |
| `WEBHOOK_DISABLE` | `false` | Ignore the webhook headers, on every route |
| `WEBHOOK_ALLOW_LIST` | None | Regular expressions a webhook URL must match one of |
| `WEBHOOK_DENY_LIST` | None | Regular expressions no webhook URL may match |
| `WEBHOOK_ERROR_ALLOW_LIST` | `WEBHOOK_ALLOW_LIST` | The allow list for error URLs |
| `WEBHOOK_ERROR_DENY_LIST` | None | More refusals for error URLs |
| `API_DOWNLOAD_FROM_ALLOW_LIST` | None | The allow list for `downloadFrom` |
| `API_DOWNLOAD_FROM_DENY_LIST` | None | The deny list for `downloadFrom` |

The lists hold comma-separated regular expressions, matched against the URL with its host normalised, on every route. Deny wins. A refusal is `403`. They add to the refusal of private addresses and never relax it. An invalid one stops the service from starting.

## Passed to engines and otherwise unused

The service hands these on to child engines from its own environment, and passes nothing else. `TZ` is also what `/debug` reports. `TMPDIR` also moves the temp directory the service itself uses.

`PATH`, `LANG`, `LC_ALL`, `HOME`, `TMPDIR`, `FONTCONFIG_FILE`, `TZ`, `PYTHONPATH`, `UNO_PATH`, `URE_BOOTSTRAP`

## Tests only

The test suites read `RUSTICDOC_WRITE_FIXTURES`, a directory to write PDF fixtures to, and `RICH_KEEP`, which keeps generated documents. The service never reads them.
