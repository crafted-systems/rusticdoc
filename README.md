# rusticdoc

A document engine in Rust: fills Office, HTML and PDF templates from JSON with `{{customer.name}}` tags, converts the result with LibreOffice or Fulgur, and finishes it with PDF operations. One multipart request does the whole job; webhooks deliver results asynchronously; the OpenAPI description feeds the SDKs.

It covers documents, not live web pages: HTML written for print is laid out by [Fulgur](https://github.com/fulgur-rs/fulgur) with no browser, and no URL is fetched or JavaScript run. It ships as one hardened image with no Node, Java or Chromium. LibreOffice does the Office conversions, driven through a small Python bridge, and qpdf and pdfcpu back the PDF stage alongside the built-in engine. The design is in [rusticdoc: a document engine in Rust](https://claude.ai/artifact/4ZBnc3L7jTH1ov5TKH8qs3), and what is done against it is in [docs/ROADMAP.md](docs/ROADMAP.md).

Full documentation is at [rusticdoc.com/docs](https://rusticdoc.com/docs/), and its source is in [site/](site/).

**Status: pre-release.** The templating, PDF, service and CLI work described here is built and tested. Generated Office files have only been opened in LibreOffice so far, not in Microsoft Office. [Known limits](#known-limits) lists this and the rest of what has not been checked.

## Run it

```bash
docker build -t rusticdoc .
docker run -d --name rusticdoc --read-only --cap-drop ALL --security-opt no-new-privileges \
  --tmpfs /tmp --tmpfs /home/rusticdoc:uid=1001,gid=1001 -p 127.0.0.1:9027:9027 rusticdoc
```

`deploy/compose.yaml` does the same with memory, process and temporary file limits. With no credentials configured the service answers anyone and warns at every start, so set `RUSTICDOC_API_KEYS` before exposing it. See [Security](#security).

Fill a template and get a PDF:

```bash
printf 'Dear {{ customer.name }},\nTotal: {{ total | currency("KES") }}\n' > letter.txt
curl -F template=@letter.txt -F 'data={"customer":{"name":"Ada"},"total":1500}' -F steps=convert \
  http://localhost:9027/v1/render -o letter.pdf
```

Convert an upload:

```bash
curl -F files=@report.docx http://localhost:9027/v1/convert -o report.pdf
```

The API description is at `http://localhost:9027/openapi.json`. `RUSTICDOC_ENABLE_DOCS=true` also serves an interactive page at `/docs`; it is off by default because it loads a third-party script from a CDN on the API's origin, under a Content-Security-Policy that limits what the page can do.

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

Send a `.pdf` to `/v1/render` as the template, or use `/v1/pdf/forms/fill`, to fill its form fields from `data`, including tags written in field values and annotations. `flatten=true` makes the result uneditable. `/v1/pdf/forms/fields` lists what a PDF has, and `/v1/pdf/overlay` lays one PDF over another.

## Signing PDFs

`/v1/pdf/sign` does two different things. By default (`signature=image`) it places signature images at exact coordinates, which is not a cryptographic signature. With `signature=certificate` it signs cryptographically with the deployment's key: an incremental update that leaves the original bytes untouched and adds a signature field and a `/Sig` dictionary (`/Adobe.PPKLite`, `/ETSI.CAdES.detached`) holding a detached CMS signature over SHA-256 of the byte ranges, with content-type, message-digest and ESS signing-certificate-v2 as signed attributes and the signer's certificate and chain included. That is PAdES baseline B-B style. The signature is invisible, or shown with the one PNG or JPEG sent (`page`, `x`, `y`, `width`); `reason`, `location`, `contactInfo` and `signerName` go into the signature dictionary, and the signing time into its `/M`.

```bash
curl -F files=@contract.pdf -F signature=certificate -F reason=Approved \
  http://localhost:9027/v1/pdf/sign -o signed.pdf
```

The key comes from the deployment, never from a request: set `RUSTICDOC_SIGNING_KEY` (a PEM private key, RSA or ECDSA P-256, in PKCS#8, PKCS#1 or SEC1) and `RUSTICDOC_SIGNING_CERT` (PEM, the signer's certificate first, then any chain). They are checked at start: the key must match the certificate and the certificate must be within its validity period, or the service does not start, and one without the other stops it too. A certificate without digitalSignature or nonRepudiation in its key usage, a self-signed one, or a key file other users can read gets a warning. Without them, `signature=certificate` answers 503 and image signing works as before. `sign` is also a step on `/v1/render`, which must be last and cannot be combined with `encrypt`, and `rusticdoc pdf sign --key key.pem --cert cert.pem` does the same from the command line.

Output has been checked with poppler's `pdfsig` (signature valid, total document signed) and pyHanko (`VALID`, also in its AdES report); `make sign-check` repeats the `pdfsig` check. It has not been opened in Adobe Acrobat. There is no RFC 3161 timestamp, so it is B-B, not B-T or LTV: a validator has only the signer's claimed time.

## Fonts

There are three ways to add fonts, and none needs a rebuild:

- **Upload them.** `POST /v1/fonts` takes a ZIP of TTF, OTF or TTC files, or one such file, and needs a [store](#storage-and-running-many-containers) and an admin key. They reach every replica, and survive restarts.
- **Mount them** at `/fonts` and call `POST /v1/fonts/reload`. Running conversions finish on the old set.
- **Send them with a request** as `fonts` files, for that request alone.

`GET /v1/fonts` lists what is available, with the `id` to delete an uploaded one by. The `x-fonts-used` response header names the fonts a converted document used.

```bash
curl -H "X-Api-Key: $ADMIN_KEY" -F file=@brand-fonts.zip http://localhost:9027/v1/fonts
```

## Storage and running many containers

Stored templates and uploaded fonts need somewhere every container can reach, so name it. Until you do, these routes answer `501` and the service stores nothing, because a default on a container's own disk would work on one replica and return `404` on the next.

```bash
RUSTICDOC_STORE=s3://my-bucket/rusticdoc   # AWS S3 or a store compatible with it
RUSTICDOC_STORE=file:///data/rusticdoc     # a volume every replica mounts: NFS, EFS, Azure Files
```

`make replicas` runs this against VersityGW, an S3 gateway, and a shared Docker volume; it has not been run against AWS itself.

Use a bucket on Kubernetes. A volume has to be `ReadWriteMany`, which many clusters do not offer, and every replica must mount the same one: the service cannot tell if they do not. S3 credentials and the endpoint come from the `AWS_*` variables (`AWS_ENDPOINT` and `AWS_ALLOW_HTTP` for a store other than AWS), or from the pod's role, and never from the URL.

| | Stored as | Reaches other replicas |
|---|---|---|
| **Templates** | `templates/<caller>/`, read from the store, content cached in memory | At once. A delete is also seen at once, because metadata is read on every request |
| **Fonts** | `fonts/`, named by their hash, copied to each replica's `RUSTICDOC_FONT_CACHE_DIR` because LibreOffice needs files | The uploading replica at once, the others within `RUSTICDOC_STORE_SYNC_INTERVAL` |

Replicas share nothing but the store: no messages, locks or leader. Everything written is immutable, so no coordination is needed. `/ready` answers `503` until a new replica has its fonts, so point the readiness probe at `/ready` and it gets no documents that would quietly come out in a fallback font. If the store goes away, renders by id answer `503`, replicas in service keep the fonts they have, and requests that send their template as a file keep working.

[deploy/kubernetes.yaml](deploy/kubernetes.yaml) is a starting point: three hardened replicas, probes, and `emptyDir` for everything local. It has been run on minikube with an S3 gateway, not on a production cluster. Give each pod the memory it asks for: three squeezed into a 6 GB node were killed for memory and timed out on conversions. Back up the bucket and turn on its encryption yourself: this is the only copy, and templates can hold what your documents hold.

## API

| Endpoint | Does |
|---|---|
| `POST /v1/render` | Template, as a file or a stored `templateId`, plus `data` JSON, then `steps` in order: `convert`, `pdfa`, `pdfua`, `merge`, `split`, `watermark`, `stamp`, `rotate`, `flatten`, `optimize`, `bookmarks`, `metadata`, `embed`, `factur-x`, `encrypt`, `sign` (last, never with `encrypt`) |
| `POST /v1/render/batch` | One template, as a file or a `templateId`, once per element of an array in the data, returned as a ZIP |
| `POST /v1/templates` | Stores a template and returns its `id`, refusing bad syntax. Needs a store |
| `GET /v1/templates`, `GET /v1/templates/{id}` | List them, a page at a time, and show one |
| `GET /v1/templates/{id}/content`, `DELETE /v1/templates/{id}` | Download or delete one |
| `POST /v1/templates/validate` | Reports template errors against sample data without converting. Takes a file or a `templateId` |
| `POST /v1/convert` | Convert one or more files; several come back as a ZIP, or one PDF with `merge=true` |
| `POST /v1/pdf/*` | `merge`, `split`, `rotate`, `flatten`, `optimize`, `encrypt`, `convert`, `watermark`, `stamp`, `overlay`, `sign`, `embed`, `factur-x`, `metadata/read`, `metadata/write`, `bookmarks/read`, `bookmarks/write`, `forms/fields`, `forms/fill` |
| `POST /v1/pdf/sign` with `signature=certificate` | A cryptographic PAdES B-B style signature with the deployment's key; 503 when none is configured. See [Signing PDFs](#signing-pdfs) |
| `GET /v1/fonts`, `POST /v1/fonts/reload` | List the fonts and pick up new ones |
| `POST /v1/fonts`, `DELETE /v1/fonts/{id}` | Upload a ZIP of fonts, or delete one, for every caller and replica. Needs a store and an admin key |
| `GET /health`, `/ready`, `/openapi.json` | Open. With credentials configured, `/health` and `/ready` show only the status to a caller without them. With a store, `/ready` waits for this replica's fonts |
| `GET /metrics` | Open with no credentials configured; otherwise behind them unless `RUSTICDOC_METRICS_PUBLIC=true` |
| `GET /v1/version` | Behind authentication |
| `GET /docs` | Only with `RUSTICDOC_ENABLE_DOCS=true` |

Every response carries `x-request-id`; the same id is in the logs and in webhook deliveries. Errors are JSON: `{"error": "invalid_request", "message": "..."}`. A request's own mistakes say what they were; an engine's failure reads as the status's name, here and in webhook deliveries, with the detail in the log under the request id.

### Webhooks

Add `X-Rusticdoc-Webhook-Url` to any request. The service answers `204` at once and posts the result to that URL, retrying three times with backoff. Failures go to `X-Rusticdoc-Webhook-Error-Url` as JSON. Private addresses are refused unless `RUSTICDOC_WEBHOOK_ALLOW_PRIVATE=true`.

At most `RUSTICDOC_MAX_ASYNC_JOBS` jobs run at once, and `RUSTICDOC_MAX_ASYNC_JOBS_PER_CALLER` for one caller; past that a request gets `429` before its body is read. A job holds its place through its delivery attempts.

With `RUSTICDOC_WEBHOOK_SECRET` set, deliveries are signed, and each caller signs with its own key, so one tenant cannot send another's receiver something that verifies:

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
| `RUSTICDOC_BIND` | `0.0.0.0:9027` | Listen address |
| `RUSTICDOC_API_TIMEOUT` | `30s` | Per request, including conversion; over it returns 503 |
| `RUSTICDOC_BODY_LIMIT` | `64MB` | Largest upload |
| `RUSTICDOC_WORKERS` | `2` | Concurrent LibreOffice conversions |
| `RUSTICDOC_API_KEYS` | none | Comma-separated keys for `X-Api-Key` or `Authorization: Bearer`. With no authentication configured the service answers anyone and warns at every start. Half-configured authentication stops it from starting |
| `RUSTICDOC_BASIC_AUTH_USERNAME`, `_PASSWORD` | none | Basic auth |
| `RUSTICDOC_ADMIN_KEYS` | none | Comma-separated keys that may upload and delete fonts. With credentials configured and none of these, nobody may |
| `RUSTICDOC_OIDC_ISSUER`, `RUSTICDOC_OIDC_AUDIENCE`, `RUSTICDOC_OIDC_JWKS_URL` | none; the JWKS URL comes from the issuer's discovery document | Accept bearer tokens from an OpenID Connect provider on the protected routes. On when the issuer is set. The issuer and audience go together, and the issuer and JWKS URL must be `https`, or `http` on a loopback host; otherwise the service does not start |
| `RUSTICDOC_STORE` | none | `s3://bucket/prefix` or `file:///path`. See [storage](#storage-and-running-many-containers) |
| `RUSTICDOC_STORE_SCOPE` | `caller` | `caller`: each credential sees only its own templates, so replacing a key leaves its templates behind. `shared`: every credential reads and deletes the same ones |
| `RUSTICDOC_MAX_TEMPLATE_BYTES` | `50MB` | Largest template that can be stored |
| `RUSTICDOC_MAX_TEMPLATES`, `RUSTICDOC_MAX_TEMPLATE_STORAGE` | `10000`, `5GB` | What one scope may hold. Replicas storing at the same moment can pass these by what is in flight |
| `RUSTICDOC_MAX_STORED_FONTS`, `RUSTICDOC_MAX_FONT_STORAGE` | `2000`, `1GB` | What the store may hold in fonts, all of which every replica copies to disk |
| `RUSTICDOC_FONT_CACHE_DIR` | the temp directory | This replica's copy of the stored fonts. Local to it, and rebuilt when it starts. It must be a real directory owned by the service's user that no one else can write to; anything else stops the font sync and keeps the replica unready |
| `RUSTICDOC_STORE_SYNC_INTERVAL` | `30s` | How often a replica looks for fonts added or deleted elsewhere |
| `RUSTICDOC_WEBHOOK_SECRET` | none | Secret that callers' signing keys derive from. Unset, deliveries are not signed, and the service warns at start |
| `RUSTICDOC_SIGNING_KEY`, `RUSTICDOC_SIGNING_CERT` | none | PEM private key (RSA or ECDSA P-256; PKCS#8, PKCS#1 or SEC1, unencrypted) and its PEM certificate, then any chain, for `signature=certificate` and the `sign` step. Both or neither: one alone, a key that does not match, or a certificate outside its validity stops the service from starting. Unset, cryptographic signing answers 503 |
| `RUSTICDOC_WEBHOOK_ALLOW_PRIVATE` | `false` | Allow webhooks to private addresses |
| `RUSTICDOC_DOWNLOAD_FROM_ALLOW_PRIVATE` | `false` | The same for `downloadFrom` |
| `RUSTICDOC_WEBHOOK_DISABLED` | `false` | Ignore the webhook headers, on every route |
| `RUSTICDOC_WEBHOOK_ALLOW_LIST`, `RUSTICDOC_WEBHOOK_DENY_LIST`, `RUSTICDOC_WEBHOOK_ERROR_ALLOW_LIST`, `RUSTICDOC_WEBHOOK_ERROR_DENY_LIST`, `RUSTICDOC_DOWNLOAD_FROM_ALLOW_LIST`, `RUSTICDOC_DOWNLOAD_FROM_DENY_LIST` | none; the error allow list defaults to the webhook allow list | Comma-separated regular expressions, matched against the URL with its host normalised, on every route. Deny wins. A refusal is `403`. They add to the refusal of private addresses and never relax it, and an invalid one stops the service from starting |
| `RUSTICDOC_MAX_INFLIGHT_BYTES` | `2GB` | Request bodies held in memory at once. Each request is charged what it declares, or the whole body limit if it streams; past the budget it waits, and gets `503` with `Retry-After` at the API timeout. One caller, meaning a credential or with none configured an address, holds at most half |
| `RUSTICDOC_MAX_ASYNC_JOBS`, `RUSTICDOC_MAX_ASYNC_JOBS_PER_CALLER` | `32`, `8` | Webhook jobs running at once, in all and per caller |
| `RUSTICDOC_BATCH_TIMEOUT` | `5m` | Overall limit for `/v1/render/batch` and a multi-file `/v1/convert`, synchronous or not |
| `RUSTICDOC_MAX_FORM_PARTS` | `1000` | File parts one form may hold; text parts are capped at 2,000 |
| `RUSTICDOC_LIBREOFFICE_MAX_DISK` | `512MB` | Most one conversion may write to disk. Past it the worker restarts and the request gets `400`. Allow `RUSTICDOC_WORKERS` times this in `/tmp` |
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
| `RUSTICDOC_CONVERT_DISABLED` | `false` | `/v1/convert` and `/v1/pdf/convert` answer 404, and conversion steps in `/v1/render` answer 503 |
| `RUSTICDOC_PDF_DISABLED` | `false` | The `/v1/pdf/*` routes answer 404 |
| `RUSTICDOC_PDF_ENGINES_MERGE`, `_SPLIT`, `_FLATTEN`, `_ROTATE`, `_OPTIMIZE_IMAGES`, `_ENCRYPT`, `_WATERMARK`, `_STAMP`, `_EMBED`, `_EMBED_METADATA`, `_READ_BOOKMARKS`, `_WRITE_BOOKMARKS`, `_READ_METADATA`, `_WRITE_METADATA`, `_FACTUR_X` | `lopdf`, `qpdf`, `pdfcpu` | Engine order per operation, from `lopdf`, `qpdf`, `pdfcpu` and `qpdf-metadata`. An unknown name stops the service from starting |
| `RUSTICDOC_PDF_ENGINES_ENCRYPT` | `lopdf`, `qpdf`, `pdfcpu` | Naming `pdfcpu` passes passwords as command-line arguments, which this user's other processes can read while it runs, and the service warns at start. A failed encryption never falls through to the next engine |

## SDKs

Generated from `openapi.json` with `make sdk-typescript`, `make sdk-python` or `make sdk-go`. See [sdk/README.md](sdk/README.md).

## Develop

```bash
make test      # unit and integration tests, no LibreOffice needed
make lint      # rustfmt and clippy
make run       # the service on 127.0.0.1:9027 with pretty logs
make openapi   # refresh openapi.json; CI checks it is current
make docker    # the image
```

The workspace has four crates: `rusticdoc-template` (the template engine), `rusticdoc-core` (pipeline, engines, the render worker), `rusticdoc-server` (axum API, auth, webhooks, metrics, OpenAPI) and `rusticdoc` (the CLI and `serve`). Tests run without LibreOffice or qpdf; conversion is exercised by the CI image smoke test.

Four checks need Docker and the image:

```bash
make outbound    # crafted documents must reach no network and read no local file
make isolation   # secrets stay out of worker processes, and /proc stays closed
make disk        # one conversion cannot fill the disk
make replicas    # several containers on one store: a bucket, then a shared volume
```

`make sign-check` needs no Docker but openssl and poppler's `pdfsig`: it signs PDFs with the CLI using throwaway keys and has `pdfsig`, and pyHanko when installed, validate them.

## Security

What the service does by itself:

- **Client templates run in a worker process.** It has no environment, limits on memory, CPU, files and core dumps, and is the kernel's first choice under memory pressure. A template that exhausts its limit gets `413`, and the service keeps running. The memory limit relies on Linux; on macOS it has not been shown to hold.
- **Input is capped before it expands.** That covers request bodies held in memory, form parts, page ranges, split output, stamp text, template output, PDF tags and webhook jobs. The settings are in the table above.
- **Secrets stay out of children.** LibreOffice, Python, qpdf and pdfcpu start from an empty environment, and on Linux the service closes its own `/proc` entries to them. Do not run the image with `docker run --init`, which puts a process back at PID 1 that exposes the environment. Anyone who can `docker exec` into the container can still read it.
- **Documents cannot reach out.** LibreOffice is configured to fetch nothing, and its web and CMIS content providers are removed from the image. `make outbound` checks this across formats.
- **Webhooks and `downloadFrom` refuse private addresses**, pin the address they checked, and follow no redirects.
- **Stored objects are guarded by who asks and what comes back.** A caller reaches only its own templates, and another's id answers `404`. Ids are checked in form before they touch a path, content is checked against its hash when read, and names inside a font ZIP are labels, never paths. ZIPs are capped by what actually comes out, and uploads and deletes are in the log with `log_type: audit`.
- **Fonts take an admin key,** because a font decides what the digits look like in everyone's documents.
- **The signing key never comes from a request,** only from `RUSTICDOC_SIGNING_KEY`, and is never logged: start-up logs name the certificate's subject, algorithm and expiry. Its private-key operations run in aws-lc-rs, because RustCrypto's `rsa` crate has an unpatched timing advisory (RUSTSEC-2023-0071) that matters for a service whose response times a client can measure.

What it leaves to you:

- Set `RUSTICDOC_API_KEYS`, basic auth or OpenID Connect, and `RUSTICDOC_WEBHOOK_SECRET`. Half-configured authentication stops the service from starting.
- If you sign, protect the key file (mode 0600, owned by the service's user) and remember that anyone who can call `/v1/pdf/sign` signs as that certificate: put signing behind credentials. Anyone who can `docker exec` into the container or read its volume can take the key, and the same LibreOffice caveat below applies to it as to a `file://` store.
- Rate limit at a reverse proxy. The per-caller and connection limits bound what one client holds, but many clients together can still fill them for as long as the timeouts allow.
- Terminate TLS in front of it. The service speaks plain HTTP.
- Rebuild the image regularly. A scan finds CVEs in Debian's packages, and those with a Debian fix arrive by rebuilding. `make docker` uses the digests pinned in the `Dockerfile`, which dependabot keeps fresh.

A compromised LibreOffice still runs as the service's user, in the service's container. It cannot read the service's secrets or memory, but it can see other requests' temporary files while they exist, and anything else that user can read: a `file://` store, or a web identity token mounted for a pod's S3 role. Prefer a bucket with keys in the environment where tenants do not trust each other. Treat the container as the boundary between tenants that do not trust each other, and run one per tenant if that matters.

## Known limits

- Office has been tried on a Mac only: templates typed in Word, Excel and PowerPoint 16.114 for Mac were filled, and the results open there without a repair prompt (`test/office`). Office for Windows has not been tried.
- Word's default font, Aptos, is not in the image, so a document set in it converts in a substitute font until Aptos is uploaded.
- Noto Serif CJK is left out of the image to save 53 MB, so Chinese, Japanese and Korean text in a serif face converts in Noto Sans CJK until Noto Serif CJK is uploaded or mounted at `/fonts`.
- Barcodes have been read by ZXing and ZBar (`test/barcodes`), not by a hardware or phone scanner.
- Filled PDF forms show their values in Chrome and Preview, and have not been opened in Acrobat.
- Text the service draws into a PDF form outside Windows Latin, such as Arabic or Chinese, is shaped and embedded as subset fonts. It has been checked with Poppler (`pdftoppm`, `pdftotext`) and macOS Quick Look, not in Chrome or Acrobat. Poppler extracts an Arabic lam-alef ligature with its two letters swapped; the file maps them in order. Comb fields are drawn as one line. Marks drawn by `watermark` and `stamp` are still Latin only, in Helvetica.
- A template that uses a font the service lacks is not reported. Check `GET /v1/fonts` and the `x-fonts-used` header.
- Macros and OLE objects in a template are not stripped. LibreOffice will not run or fetch them when converting, but a request with no conversion step returns them in its output.
- A stored template cannot be changed: store the new one and use its id. There are no versions or aliases yet.
- Fonts are one set per deployment, not per caller, and only admin keys can change them, not basic auth or OpenID Connect users.
- PDFs from LibreOffice are not byte-identical across architectures or runs, since its layout moves text by hundredths of a point between processors. `make golden` compares 14 outputs from the amd64 and arm64 images: Office and text outputs and the PDF form the service fills itself are byte-identical, and LibreOffice's PDFs match as page images or in layout within 0.25 pt.
- Repeating a whole sheet in XLSX or ODS is not supported, only rows within one, and neither is a row loop nested in another row block of the same sheet. ODS formulas are not re-based. Charts are not supported in ODF templates.
- Cryptographic signatures are PAdES B-B style only: no RFC 3161 timestamp (B-T), no revocation data or document timestamp (B-LT, B-LTA). They validate in `pdfsig` and pyHanko but have not been checked in Adobe Acrobat. There is one signing key per deployment, RSA or ECDSA P-256 only, and one signature per request; the `sign` step on `/v1/render` is invisible. The appearance ignores page rotation, and a document certified with DocMDP is not checked before another signature is added.
- `data` counts against the render memory limit once parsed. At the default limit, a list of 8 million small numbers rendered and one of 16 million got `413`, so raise `RUSTICDOC_RENDER_MEMORY` for data that size.

## Licence

[Apache-2.0](LICENSE). Rust dependencies use permissive licences such as MIT, Apache-2.0, BSD, ISC, Zlib and Unicode-3.0. The image also carries LibreOffice under MPL-2.0, and qpdf and pdfcpu under Apache-2.0, which run as separate processes.
