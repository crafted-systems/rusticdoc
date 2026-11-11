# rusticdoc

rusticdoc is a document engine written in Rust. It fills Office, HTML and PDF templates from JSON using tags like `{{customer.name}}`, converts the result with LibreOffice or Fulgur, and finishes it with PDF operations. One multipart request does the whole job. If you'd rather not wait for the response, a webhook can deliver the result, or a background job can hold it until you collect it. The SDKs are generated from the OpenAPI description.

It handles documents and doesn't render live web pages. HTML written for print is laid out by [Fulgur](https://github.com/fulgur-rs/fulgur) without a browser, so no URL is fetched and no JavaScript runs. Everything ships in one hardened image with no Node, Java or Chromium in it. LibreOffice does the Office conversions through a small Python bridge, and qpdf and pdfcpu back the PDF stage alongside the built-in engine.

The full documentation is at [rusticdoc.com/docs](https://rusticdoc.com/docs/), and its source is in [site/](site/).

The current version is 0.1.1. The image is `ghcr.io/crafted-systems/rusticdoc:0.1.1`, built for amd64 and arm64. Versions follow [ZeroVer](https://0ver.org/), so they stay below 1.0 and a new minor number may change behaviour, as the [changelog](site/src/content/docs/docs/project/changelog.md) explains. [Known limits](#known-limits) lists what hasn't been checked yet.

## Run it

```bash
docker build -t rusticdoc .
docker run -d --name rusticdoc --read-only --cap-drop ALL --security-opt no-new-privileges \
  --tmpfs /tmp --tmpfs /home/rusticdoc:uid=1001,gid=1001 -p 127.0.0.1:9027:9027 rusticdoc
```

`deploy/compose.yaml` does the same and adds limits on memory, processes and temporary files. With no credentials configured, anyone who can reach the service can use it, and it logs a warning every time it starts, so set `RUSTICDOC_API_KEYS` before you expose it. See [Security](#security) for what else to set.

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

The API description is at `http://localhost:9027/openapi.json`. Set `RUSTICDOC_ENABLE_DOCS=true` and the service also serves an interactive page at `/docs`. It's off by default because the page loads a third-party script from a CDN on the API's origin, under a Content-Security-Policy that limits what the page can do.

## Templates

Tags look like `{{ customer.name }}`, and there are `{% for %}` loops, `{% if %}` conditions and filters. They work in `.docx`, `.odt`, `.xlsx`, `.ods`, `.pptx` and `.odp` files, in PDF form fields and annotations, and in text formats (`.txt`, `.md`, `.html`, `.csv`, `.xml`, `.json`, `.svg`). A tag that Word has split across runs still works, and a loop around a table row or a paragraph repeats that row or paragraph.

Text filters include `currency("KES")`, `date` and `t` for translations. In Office templates, these filters insert richer content:

| Filter | Example | What it inserts |
|---|---|---|
| `image` | `{{ logo \| image(width=120) }}` | A picture from base64 or a data URL. It's never fetched from a URL |
| `barcode` | `{{ sku \| barcode("code128") }}` | A `qr`, `code128`, `code39`, `ean13` or `ean8` barcode |
| `link` | `{{ url \| link("Open") }}` | A hyperlink |
| `color` | `{{ name \| color("#c00") }}` | Coloured text, and a background colour with `background=` |
| `html` | `{{ body \| html }}` | A safe subset of HTML, as native formatting |
| `chart` | `{{ rows \| chart(type="bar", x="month", y=["sales"]) }}` | A native `bar`, `column`, `line` or `pie` chart, in DOCX, PPTX and XLSX |
| `pagebreak` | `{{ "" \| pagebreak }}` | A page break |

`POST /v1/templates/validate` or `rusticdoc validate` checks a template against sample data and lists every problem it finds, with where each one is.

## PDF forms

To fill a PDF's form fields from `data`, send the `.pdf` to `/v1/render` as the template or use `/v1/pdf/forms/fill`. Tags written in field values and annotations are filled too. `flatten=true` makes the result uneditable. `/v1/pdf/forms/fields` lists the fields a PDF has, and `/v1/pdf/overlay` lays one PDF over another.

## Signing PDFs

`/v1/pdf/sign` does two different things. By default (`signature=image`) it places signature images at exact coordinates, which isn't a cryptographic signature. With `signature=certificate` it signs cryptographically with the deployment's key.

The cryptographic signature is an incremental update, so the original bytes stay untouched. It adds a signature field and a `/Sig` dictionary (`/Adobe.PPKLite`, `/ETSI.CAdES.detached`) holding a detached CMS signature over the SHA-256 of the byte ranges. The signed attributes are content-type, message-digest and ESS signing-certificate-v2, and the signer's certificate and chain are included. That's PAdES baseline B-B.

`level` goes further. `b-t` adds an RFC 3161 timestamp from the configured timestamp authority, `b-lt` adds a DSS with the chains' certificates and their OCSP responses or CRLs, and `b-lta` adds a document timestamp over all of it. If the timestamp authority or a revocation source fails, you get a `502` and never a file at a lower level.

`certify=1|2|3` certifies the document (DocMDP) instead of approving it. The signature can be invisible, shown as the one PNG or JPEG you send, or shown as text (signer, date, reason) at `page`, `x`, `y` and `width`. `reason`, `location`, `contactInfo` and `signerName` go into the signature dictionary, and the signing time goes into its `/M`.

```bash
curl -F files=@contract.pdf -F signature=certificate -F reason=Approved \
  http://localhost:9027/v1/pdf/sign -o signed.pdf
```

The key always comes from the deployment and never from a request. Set `RUSTICDOC_SIGNING_KEY` (a PEM private key, RSA or ECDSA P-256, in PKCS#8, PKCS#1 or SEC1) and `RUSTICDOC_SIGNING_CERT` (PEM, with the signer's certificate first, then any chain). Both are checked at start. The key has to match the certificate and the certificate has to be within its validity period, or the service doesn't start, and setting one without the other stops it too. You get a warning for a certificate without digitalSignature or nonRepudiation in its key usage, for a self-signed one, and for a key file other users can read.

Without a key and certificate, `signature=certificate` returns `503` and image signing still works. Named keys (`RUSTICDOC_SIGNING_KEYS`, chosen with `signingKey`) each have an allow list of callers, and any key can live on a PKCS#11 token (`pkcs11:<label>`, with `RUSTICDOC_PKCS11_*`).

`sign` is also a step on `/v1/render`, with the same options. It has to be the last step and can't be combined with `encrypt`. From the command line, `rusticdoc pdf sign --key key.pem --cert cert.pem` does the same.

`signature=renew`, or `rusticdoc pdf sign --renew`, adds a fresh document timestamp to a file signed at B-LTA, so it can still be validated after the old timestamp's certificate expires. It uses the timestamp authority and no key, and nothing runs it on a schedule.

The output validates in poppler's `pdfsig` (signature valid) and in pyHanko (`VALID`). B-LT and B-LTA files also validate offline with revocation checking on, using only what they hold. `make sign-check` repeats both checks against a local timestamp authority, OCSP responder and CRL, and keys on SoftHSM2 have signed at every level.

## Fonts

You can add fonts in three ways, and none of them needs a rebuild:

- Upload them. `POST /v1/fonts` takes a ZIP of TTF, OTF or TTC files, or a single font file, and needs a [store](#storage-and-running-many-containers) and an admin key. Uploaded fonts reach every replica and survive restarts.
- Mount them at `/fonts` and call `POST /v1/fonts/reload`. Conversions that are already running finish on the old set.
- Send them with a request as `fonts` files, and they apply to that request only.

`GET /v1/fonts` lists the fonts available, with the `id` you need to delete an uploaded one. The `x-fonts-used` response header names the fonts a converted document used.

```bash
curl -H "X-Api-Key: $ADMIN_KEY" -F file=@brand-fonts.zip http://localhost:9027/v1/fonts
```

## Storage and running many containers

Stored templates and uploaded fonts have to live somewhere every container can reach, and you have to say where. Until you do, the service stores nothing and these routes return `501`. There's no default store on the container's own disk, because it would work on one replica and return `404` on the next.

```bash
RUSTICDOC_STORE=s3://my-bucket/rusticdoc   # AWS S3 or a store compatible with it
RUSTICDOC_STORE=file:///data/rusticdoc     # a volume every replica mounts: NFS, EFS, Azure Files
```

`make replicas` runs this against VersityGW, which is an S3 gateway, and against a shared Docker volume.

On Kubernetes, use a bucket. A volume has to be `ReadWriteMany`, which many clusters don't offer, and every replica has to mount the same one, which the service can't check. S3 credentials and the endpoint come from the `AWS_*` variables (`AWS_ENDPOINT` and `AWS_ALLOW_HTTP` for a store other than AWS) or from the pod's role, never from the URL.

| | Where it's stored | When other replicas see it |
|---|---|---|
| **Templates** | `templates/<caller>/`. Read from the store, with content cached in memory | Straight away. A delete shows up straight away too, because metadata is read on every request |
| **Fonts** | `fonts/`, named by their hash, and copied to each replica's `RUSTICDOC_FONT_CACHE_DIR` because LibreOffice needs files | Straight away on the replica that took the upload, and within `RUSTICDOC_STORE_SYNC_INTERVAL` on the others |

The store is the only thing replicas share. There are no messages, locks or leader, and because everything written is immutable they don't need to coordinate. `/ready` returns `503` until a new replica has its fonts, so point your readiness probe at `/ready` and a new replica won't be sent documents that would quietly come out in a fallback font. If the store goes away, renders by id return `503`, replicas already in service keep the fonts they have, and requests that send their template as a file keep working.

[deploy/kubernetes.yaml](deploy/kubernetes.yaml) is a starting point, with three hardened replicas, probes, and `emptyDir` for everything local. It has run on minikube with an S3 gateway, but not on a production cluster. Give each pod the memory it asks for. Three pods squeezed into a 6 GB node were killed for memory and timed out on conversions. Back up the bucket and turn on its encryption yourself, because it's the only copy and templates can hold whatever your documents hold.

## API

| Endpoint | What it does |
|---|---|
| `POST /v1/render` | Fills a template, sent as a file or as a stored `templateId`, with `data` JSON, then runs `steps` in order: `convert`, `pdfa`, `pdfua`, `merge`, `split`, `watermark`, `stamp`, `rotate`, `flatten`, `optimize`, `bookmarks`, `metadata`, `embed`, `factur-x`, `encrypt`, `sign` (last, and never with `encrypt`) |
| `POST /v1/render/batch` | Renders one template, sent as a file or a `templateId`, once for each element of an array in the data, and returns a ZIP |
| `POST /v1/templates` | Stores a template and returns its `id`. Refuses bad syntax. Needs a store |
| `GET /v1/templates`, `GET /v1/templates/{id}` | Lists them a page at a time, or shows one |
| `GET /v1/templates/{id}/content`, `DELETE /v1/templates/{id}` | Downloads or deletes one |
| `POST /v1/templates/validate` | Reports template errors against sample data without converting. Takes a file or a `templateId` |
| `POST /v1/convert` | Converts one or more files. Several come back as a ZIP, or as one PDF with `merge=true` |
| `POST /v1/pdf/*` | `merge`, `split`, `rotate`, `flatten`, `optimize`, `encrypt`, `convert`, `watermark`, `stamp`, `overlay`, `sign`, `embed`, `factur-x`, `metadata/read`, `metadata/write`, `bookmarks/read`, `bookmarks/write`, `forms/fields`, `forms/fill` |
| `POST /v1/pdf/sign` with `signature=certificate` | A cryptographic PAdES signature, B-B to B-LTA, approving or certifying, with the deployment's key or a named one. Returns `503` when no key is configured. See [Signing PDFs](#signing-pdfs) |
| `GET /v1/jobs/{id}`, `GET /v1/jobs/{id}/result` | Shows a background job, or downloads its result. Needs a store. See [Background jobs](#background-jobs) |
| `GET /v1/fonts`, `POST /v1/fonts/reload` | Lists the fonts, and picks up new ones |
| `POST /v1/fonts`, `DELETE /v1/fonts/{id}` | Uploads a ZIP of fonts, or deletes one, for every caller and replica. Needs a store and an admin key |
| `GET /health`, `/ready`, `/openapi.json` | Open. With credentials configured, `/health` and `/ready` show only the status to a caller without them. With a store, `/ready` waits for this replica's fonts |
| `GET /metrics` | Open when no credentials are configured. Otherwise it needs them, unless `RUSTICDOC_METRICS_PUBLIC=true` |
| `GET /v1/version` | Needs authentication |
| `GET /docs` | Only with `RUSTICDOC_ENABLE_DOCS=true` |

Every response carries `x-request-id`, and the same id appears in the logs and in webhook deliveries. Errors are JSON, like `{"error": "invalid_request", "message": "..."}`. When the request itself is wrong, the message says what was wrong with it. When an engine fails, the message is only the name of the status, here and in webhook deliveries, and the detail is in the log under the request id.

### Webhooks

Add `X-Rusticdoc-Webhook-Url` to any request and the service returns `204` straight away, then posts the result to that URL, retrying three times with backoff. Failures go to `X-Rusticdoc-Webhook-Error-Url` as JSON. Private addresses are refused unless you set `RUSTICDOC_WEBHOOK_ALLOW_PRIVATE=true`.

At most `RUSTICDOC_MAX_ASYNC_JOBS` webhook and background jobs run at once, and at most `RUSTICDOC_MAX_ASYNC_JOBS_PER_CALLER` for one caller. Past either limit, a request gets `429` before its body is read. A job holds its place through its delivery attempts.

With `RUSTICDOC_WEBHOOK_SECRET` set, deliveries are signed. Each caller's deliveries are signed with that caller's own key, so one tenant can't send another tenant's receiver something that verifies:

- The key is `HMAC-SHA256(secret, principal)`. The principal is `key:<hex SHA-256 of the API key>`, `basic:<hex SHA-256 of the user>` or `oidc:<hex SHA-256 of the token's subject>`. With no credentials configured, the principal is `public` and the key is the secret itself.
- `rusticdoc webhook-secret --api-key <key>` (or `--basic-user`, `--oidc-subject`) prints a caller's key in hex when you run it with `RUSTICDOC_WEBHOOK_SECRET` set. Your receiver decodes the hex and verifies with those bytes. If you don't name a caller, it prints the secret itself, in hex.
- `X-Rusticdoc-Signature` is `v1=` followed by the hex HMAC-SHA256 of `<timestamp>.<event>.<request id>.<body>`, built from the `X-Rusticdoc-Timestamp`, `X-Rusticdoc-Event` and `X-Request-Id` headers of the same delivery. Compare in constant time and reject stale timestamps.

### Background jobs

If you can't receive a webhook, poll for the result instead. Add `Prefer: respond-async` to a request that produces a file (`/v1/render`, `/v1/render/batch`, `/v1/convert` or `/v1/pdf/*`) and the service returns `202` straight away, with the job in the body and its address in `Location`. `GET /v1/jobs/{id}` shows its `status`, which is `queued`, `running`, `done` or `failed`. Once it's `done`, `GET /v1/jobs/{id}/result` returns the file. Before that it returns `409`.

Jobs keep their state and results in the store, so any replica can serve a job that another one ran. That means they need `RUSTICDOC_STORE`. Without it the request gets `501` and nothing runs. A job and its result are kept for `RUSTICDOC_JOB_TTL` after the job ends, which is an hour by default.

A mistake in the request still returns `400` straight away and starts no job. So does asking for a webhook and a job in the same request. A job always belongs to the caller that started it, whatever `RUSTICDOC_STORE_SCOPE` says, and another caller's job id gets `404`. The [background jobs guide](site/src/content/docs/docs/guides/background-jobs.md) has the rest.

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

`rusticdoc pdf` has `merge`, `split`, `rotate`, `flatten`, `optimize`, `encrypt`, `metadata`, `bookmarks`, `embed`, `watermark`, `stamp`, `sign`, `facturx`, `fields`, `fill` and `overlay`, and `--help` shows the options for each. The CLI renders templates in its own process because it assumes they're yours, while `serve` isolates them.

## Configuration

Every setting is an environment variable, and all of them are optional:

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_BIND` | `0.0.0.0:9027` | Listen address |
| `RUSTICDOC_API_TIMEOUT` | `30s` | Time limit for one request, including conversion. Past it, the request returns `503` |
| `RUSTICDOC_BODY_LIMIT` | `64MB` | Largest upload |
| `RUSTICDOC_WORKERS` | `2` | Concurrent LibreOffice conversions |
| `RUSTICDOC_API_KEYS` | none | Comma-separated keys for `X-Api-Key` or `Authorization: Bearer`. With no authentication configured, anyone can use the service and it warns every time it starts. Half-configured authentication stops it from starting |
| `RUSTICDOC_BASIC_AUTH_USERNAME`, `_PASSWORD` | none | Basic auth |
| `RUSTICDOC_ADMIN_KEYS` | none | Comma-separated keys that may upload and delete fonts. With credentials configured but none of these, nobody may |
| `RUSTICDOC_OIDC_ISSUER`, `RUSTICDOC_OIDC_AUDIENCE`, `RUSTICDOC_OIDC_JWKS_URL` | none. The JWKS URL comes from the issuer's discovery document | Accepts bearer tokens from an OpenID Connect provider on the protected routes. It's on when the issuer is set. The issuer and audience go together, and the issuer and JWKS URL must be `https`, or `http` on a loopback host. Otherwise the service doesn't start |
| `RUSTICDOC_STORE` | none | `s3://bucket/prefix` or `file:///path`. See [storage](#storage-and-running-many-containers) |
| `RUSTICDOC_STORE_SCOPE` | `caller` | With `caller`, each credential sees only its own templates, so replacing a key leaves its templates behind. With `shared`, every credential reads and deletes the same ones |
| `RUSTICDOC_MAX_TEMPLATE_BYTES` | `50MB` | Largest template that can be stored |
| `RUSTICDOC_MAX_TEMPLATES`, `RUSTICDOC_MAX_TEMPLATE_STORAGE` | `10000`, `5GB` | How much one scope may hold. Replicas that store at the same moment can go over by whatever is in flight |
| `RUSTICDOC_MAX_STORED_FONTS`, `RUSTICDOC_MAX_FONT_STORAGE` | `2000`, `1GB` | How much the store may hold in fonts. Every replica copies all of them to disk |
| `RUSTICDOC_FONT_CACHE_DIR` | the temp directory | This replica's copy of the stored fonts. It's local to the replica and rebuilt when it starts. It must be a real directory, owned by the service's user, that no one else can write to. Anything else stops the font sync and keeps the replica unready |
| `RUSTICDOC_STORE_SYNC_INTERVAL` | `30s` | How often a replica looks for fonts added or deleted elsewhere |
| `RUSTICDOC_WEBHOOK_SECRET` | none | The secret that callers' signing keys are derived from. If it's unset, deliveries aren't signed and the service warns at start |
| `RUSTICDOC_SIGNING_KEY`, `RUSTICDOC_SIGNING_CERT` | none | A PEM private key (RSA or ECDSA P-256, in PKCS#8, PKCS#1 or SEC1, unencrypted) or `pkcs11:<label>`, and its PEM certificate followed by any chain. They're used by `signature=certificate` and the `sign` step. Set both or neither. One alone, a key that doesn't match, or a certificate outside its validity stops the service from starting. If they're unset, cryptographic signing returns `503`. The [configuration reference](site/src/content/docs/docs/reference/configuration.md#pdf-signing) covers named keys, PKCS#11, the timestamp authority and the rest |
| `RUSTICDOC_WEBHOOK_ALLOW_PRIVATE` | `false` | Allow webhooks to private addresses |
| `RUSTICDOC_DOWNLOAD_FROM_ALLOW_PRIVATE` | `false` | The same for `downloadFrom` |
| `RUSTICDOC_WEBHOOK_DISABLED` | `false` | Ignore the webhook headers, on every route |
| `RUSTICDOC_WEBHOOK_ALLOW_LIST`, `RUSTICDOC_WEBHOOK_DENY_LIST`, `RUSTICDOC_WEBHOOK_ERROR_ALLOW_LIST`, `RUSTICDOC_WEBHOOK_ERROR_DENY_LIST`, `RUSTICDOC_DOWNLOAD_FROM_ALLOW_LIST`, `RUSTICDOC_DOWNLOAD_FROM_DENY_LIST` | none. The error allow list defaults to the webhook allow list | Comma-separated regular expressions, matched against the URL with its host normalised, on every route. Deny wins, and a refused URL gets `403`. The lists add to the refusal of private addresses and never relax it. An invalid expression stops the service from starting |
| `RUSTICDOC_MAX_INFLIGHT_BYTES` | `2GB` | Request bodies held in memory at once. Each request is charged what it declares, or the whole body limit if it streams. Past the budget it waits, and gets `503` with `Retry-After` at the API timeout. One caller (a credential, or an address when none are configured) holds at most half |
| `RUSTICDOC_MAX_ASYNC_JOBS`, `RUSTICDOC_MAX_ASYNC_JOBS_PER_CALLER` | `32`, `8` | Webhook and background jobs running at once, in total and per caller |
| `RUSTICDOC_JOB_TTL` | `1h` | How long a background job and its result are kept after the job ends |
| `RUSTICDOC_MAX_JOB_STORAGE` | `5GB` | How much one scope's background-job results may hold at a time |
| `RUSTICDOC_BATCH_TIMEOUT` | `5m` | Overall limit for `/v1/render/batch` and a multi-file `/v1/convert`, synchronous or not |
| `RUSTICDOC_MAX_FORM_PARTS` | `1000` | File parts one form may hold. Text parts are capped at 2,000 |
| `RUSTICDOC_LIBREOFFICE_MAX_DISK` | `512MB` | The most one conversion may write to disk. Past it the worker restarts and the request gets `400`. Allow `RUSTICDOC_WORKERS` times this in `/tmp` |
| `RUSTICDOC_MAX_RENDER_BYTES` | `64MB` | The most a text template, `batchName` or PDF form tag may write. Office templates stop at the package limit of 256 MiB |
| `RUSTICDOC_RENDER_MODE` | `isolated` under `serve` | `isolated` evaluates client templates in a worker process. `in-process` doesn't, and one template can then take the service down |
| `RUSTICDOC_RENDER_MEMORY` | `1GB` | Address space of one render worker. A template that goes past it gets `413` |
| `RUSTICDOC_RENDER_WORKERS` | `4` | Render workers running at once, so peak memory for rendering is this times `RUSTICDOC_RENDER_MEMORY` |
| `RUSTICDOC_RENDER_WORKER_BIN` | the running binary | The binary with the worker in it |
| `RUSTICDOC_HEADER_TIMEOUT` | `10s` | How long a connection may wait for a complete set of request headers. This covers a new connection that sends nothing and an idle keep-alive one |
| `RUSTICDOC_STALL_TIMEOUT` | `30s` | How long a connection may stay blocked on its client, which means a response nobody reads or a body a handler is waiting for that doesn't arrive. Time a handler spends working doesn't count. A slow body that keeps moving is allowed up to the request time limit |
| `RUSTICDOC_RESPONSE_WRITE_TIMEOUT` | `5m` | How long writes may spend blocked in total, so a reader that takes a little at a time, staying under the stall timeout, still loses its connection. It starts over once everything sent has left the send queue, so pipelining doesn't reset it |
| `RUSTICDOC_MAX_CONNECTIONS` | `1024` | Connections served at once. Any more wait in the listen queue |
| `RUSTICDOC_MAX_CONNECTIONS_PER_PEER` | `0` (off) | Connections at once from one IPv4 address or IPv6 /64, since one client usually holds a whole /64. An IPv4-mapped address counts as its IPv4 address. Any more are closed. Leave it off behind a load balancer, where every connection has the same peer, and rate limit at the proxy instead |
| `RUSTICDOC_OUTBOUND_PROXY` | none | Proxy for webhooks and `downloadFrom`. Proxy variables in the environment are otherwise ignored. With this set, DNS pinning no longer applies because the proxy resolves names itself, so the proxy must enforce its own egress policy, including blocking private ranges. The URL and its resolved addresses are still checked first, and the service warns at start |
| `RUSTICDOC_METRICS_PUBLIC` | `false` | Keep `/metrics` open when credentials are configured |
| `RUSTICDOC_ENABLE_DOCS` | `false` | Serve `/docs`, which loads a third-party script |
| `RUSTICDOC_CORS_ORIGINS` | none | Origins allowed cross-origin access, comma-separated, or `*` |
| `RUSTICDOC_LIBREOFFICE_BIN`, `RUSTICDOC_QPDF_BIN`, `RUSTICDOC_PDFCPU_BIN` | auto-detected | Engine paths. `none` disables one |
| `RUSTICDOC_FONTS_DIR` | `/fonts` | Mounted fonts, picked up by LibreOffice |
| `RUSTICDOC_LOG_FORMAT`, `RUSTICDOC_LOG_LEVEL` | `json`, `info` | Logging |
| `RUSTICDOC_CONVERT_DISABLED` | `false` | `/v1/convert` and `/v1/pdf/convert` return `404`, and conversion steps in `/v1/render` return `503` |
| `RUSTICDOC_PDF_DISABLED` | `false` | The `/v1/pdf/*` routes return `404` |
| `RUSTICDOC_PDF_ENGINES_MERGE`, `_SPLIT`, `_FLATTEN`, `_ROTATE`, `_OPTIMIZE_IMAGES`, `_ENCRYPT`, `_WATERMARK`, `_STAMP`, `_EMBED`, `_EMBED_METADATA`, `_READ_BOOKMARKS`, `_WRITE_BOOKMARKS`, `_READ_METADATA`, `_WRITE_METADATA`, `_FACTUR_X` | `lopdf`, `qpdf`, `pdfcpu` | Engine order per operation, from `lopdf`, `qpdf`, `pdfcpu` and `qpdf-metadata`. An unknown name stops the service from starting |
| `RUSTICDOC_PDF_ENGINES_ENCRYPT` | `lopdf`, `qpdf`, `pdfcpu` | Naming `pdfcpu` passes passwords as command-line arguments, which this user's other processes can read while it runs, and the service warns at start. A failed encryption never falls through to the next engine |

## SDKs

The SDKs are generated from `openapi.json` with `make sdk-typescript`, `make sdk-python` or `make sdk-go`. All three compile and pass a short test against a running service, and none is published yet. See [sdk/README.md](sdk/README.md).

## Develop

```bash
make test      # unit and integration tests, no LibreOffice needed
make lint      # rustfmt and clippy
make run       # the service on 127.0.0.1:9027 with pretty logs
make openapi   # refresh openapi.json; CI checks it is current
make docker    # the image
```

The workspace has four crates: `rusticdoc-template` (the template engine), `rusticdoc-core` (pipeline, engines, the render worker), `rusticdoc-server` (axum API, auth, webhooks, metrics, OpenAPI) and `rusticdoc` (the CLI and `serve`). The tests run without LibreOffice or qpdf, and conversion is exercised by the image smoke test in CI.

Four checks need Docker and the image:

```bash
make outbound    # crafted documents must reach no network and read no local file
make isolation   # secrets stay out of worker processes, and /proc stays closed
make disk        # one conversion cannot fill the disk
make replicas    # several containers on one store: a bucket, then a shared volume
```

`make sign-check` needs openssl, python3, curl and poppler's `pdfsig`, but not Docker. It signs PDFs at every level with the CLI, using throwaway keys and a test CA with a local timestamp authority, OCSP responder and CRL, then has `pdfsig` validate them, and pyHanko too when it's installed. It also renews the B-LTA file and checks both timestamps with `pdfsig` and OpenSSL. `test/sign/softhsm.sh` signs with keys on SoftHSM2 in a Debian container.

## Security

The service does these things by itself:

- Client templates run in a worker process that has no environment and has limits on memory, CPU, files and core dumps. The kernel kills it first when memory runs short. A template that goes over its limit gets `413`, and the service keeps running. The memory limit relies on Linux and isn't known to hold on macOS.
- Input is capped before it expands. That covers request bodies held in memory, form parts, page ranges, split output, stamp text, template output, PDF tags, and webhook and background jobs, and the settings are in the table above.
- Secrets stay out of child processes. LibreOffice, Python, qpdf and pdfcpu start from an empty environment, and on Linux the service closes its own `/proc` entries to them. Don't run the image with `docker run --init`, because that puts another process at PID 1, one that exposes the environment. Anyone who can `docker exec` into the container can still read it.
- Documents can't reach the network. LibreOffice is configured to fetch nothing, and its web and CMIS content providers are removed from the image. `make outbound` checks this across formats.
- Webhooks and `downloadFrom` refuse private addresses, pin the address they checked, and follow no redirects.
- A caller reaches only its own stored templates, and another caller's id returns `404`. Ids are checked for their form before they touch a path, content is checked against its hash when it's read, and names inside a font ZIP are labels, never paths. ZIPs are capped by what actually comes out of them, and uploads and deletes are in the log with `log_type: audit`.
- Uploading and deleting fonts takes an admin key, because a font decides what the digits look like in everyone's documents.
- The signing key never comes from a request, only from `RUSTICDOC_SIGNING_KEY`, and it's never logged. Start-up logs name the certificate's subject, algorithm and expiry. Private-key operations run in aws-lc-rs, because RustCrypto's `rsa` crate has an unpatched timing advisory (RUSTSEC-2023-0071), and that matters for a service whose response times a client can measure.

These are left to you:

- Set `RUSTICDOC_API_KEYS`, basic auth or OpenID Connect, and set `RUSTICDOC_WEBHOOK_SECRET`. Half-configured authentication stops the service from starting.
- If you sign, protect the key file (mode 0600, owned by the service's user) and put signing behind credentials, because anyone who can call `/v1/pdf/sign` signs as that certificate. Anyone who can `docker exec` into the container or read its volume can take the key, and the LibreOffice caveat below applies to the key just as it does to a `file://` store.
- Rate limit at a reverse proxy. The per-caller and connection limits bound what one client holds, but many clients together can still fill them for as long as the timeouts allow.
- Terminate TLS in front of the service, which speaks plain HTTP.
- Rebuild the image regularly. A scan finds CVEs in Debian's packages, and the ones Debian has fixed arrive when you rebuild. `make docker` uses the digests pinned in the `Dockerfile`, which dependabot keeps fresh.

A compromised LibreOffice still runs as the service's user, in the service's container. It can't read the service's secrets or memory, but it can see other requests' temporary files while they exist, along with anything else that user can read, such as a `file://` store or a web identity token mounted for a pod's S3 role. Where tenants don't trust each other, prefer a bucket with keys in the environment, treat the container as the boundary between them, and run one container per tenant if that matters.

## Known limits

- Word's default font, Aptos, isn't in the image, so a document set in it converts in a substitute font until you upload Aptos. Validation reports fonts like this in `missing_fonts`.
- Noto Serif CJK is left out of the image to save 53 MB, so Chinese, Japanese and Korean text in a serif face converts in Noto Sans CJK until you upload Noto Serif CJK or mount it at `/fonts`.
- Marks drawn by `watermark` and `stamp` are Latin only, in Helvetica.
- An archive timestamp is renewed only when you ask for it. Keys in a cloud KMS aren't supported.
- PDFs from LibreOffice aren't byte-identical across architectures or runs, because its layout moves text by hundredths of a point between processors. `make golden` compares 14 outputs from the amd64 and arm64 images. Office and text outputs, and the PDF form the service fills itself, are byte-identical. LibreOffice's PDFs match as page images, or in layout within 0.25 pt.
- `data` counts against the render memory limit once it's parsed. At the default limit, a list of 8 million small numbers rendered and a list of 16 million got `413`, so raise `RUSTICDOC_RENDER_MEMORY` for data that size.
- The load figures come from one small document on one container (`make load`, and `test/load/README.md` has them).
- No SDK is published.

See the [known limits page](site/src/content/docs/docs/project/known-limits.md) for the full list.

## Licence

rusticdoc is licensed under [Apache-2.0](LICENSE). Its Rust dependencies use permissive licences such as MIT, Apache-2.0, BSD, ISC, Zlib and Unicode-3.0. The image also carries LibreOffice under MPL-2.0, and qpdf and pdfcpu under Apache-2.0. All three run as separate processes.
