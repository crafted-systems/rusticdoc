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
| Boolean | `1`, `true` or `yes` is on. Anything else is off, including `TRUE` | |
| List | Comma-separated, with spaces trimmed | |

An empty variable counts as unset.

**A bad value does not always stop the service.** These settings stop it from starting, with a message that names the problem: `RUSTICDOC_STORE`, `RUSTICDOC_STORE_SCOPE`, `RUSTICDOC_ADMIN_KEYS`, `RUSTICDOC_API_KEYS`, `RUSTICDOC_TENANT_KEYS`, `RUSTICDOC_BASIC_AUTH_USERNAME` and `RUSTICDOC_BASIC_AUTH_PASSWORD` when only one is set, `RUSTICDOC_BASIC_AUTH_ADMIN` without basic auth, `RUSTICDOC_OIDC_ADMIN_SUBJECTS` or `RUSTICDOC_OIDC_ADMIN_CLAIM` without OpenID Connect or in the wrong form, `RUSTICDOC_JOB_TTL`, `RUSTICDOC_RENDER_MODE`, `RUSTICDOC_MAX_TEMPLATE_BYTES`, `RUSTICDOC_MAX_TEMPLATES`, `RUSTICDOC_MAX_TEMPLATE_STORAGE`, `RUSTICDOC_MAX_STORED_FONTS`, `RUSTICDOC_MAX_FONT_STORAGE`, `RUSTICDOC_STORE_SYNC_INTERVAL`, a `RUSTICDOC_PDF_ENGINES_*` list that names an unknown engine, an invalid regular expression in an allow or deny list, and OpenID Connect with only one of the issuer and audience set, or with an issuer or JWKS URL that is neither `https` nor `http` on a loopback host.

Every other number, size or duration that the service cannot parse is ignored, and the default stays in place with no warning. `RUSTICDOC_RENDER_MEMORY=2g` leaves the limit at 1 GiB.

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

## Routes

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_CONVERT_DISABLED` | `false` | `/v1/convert` and `/v1/pdf/convert` answer `404`. Conversion steps in `/v1/render` answer `503` |
| `RUSTICDOC_PDF_DISABLED` | `false` | The `/v1/pdf/*` routes answer `404` |

## Authentication

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_API_KEYS` | None | Comma-separated keys for `X-Api-Key` or `Authorization: Bearer`. A value with no key in it, such as `,`, stops the service from starting |
| `RUSTICDOC_TENANT_KEYS` | None | Comma-separated `tenant:key` entries. Every key of a tenant is the same caller, so replacing a key keeps its templates, aliases, jobs and webhook signing key. A tenant is 1 to 64 lowercase letters, digits and hyphens |
| `RUSTICDOC_BASIC_AUTH_USERNAME` | None | Basic auth user. Set it together with `RUSTICDOC_BASIC_AUTH_PASSWORD` |
| `RUSTICDOC_BASIC_AUTH_PASSWORD` | None | Basic auth password. Set it together with `RUSTICDOC_BASIC_AUTH_USERNAME` |
| `RUSTICDOC_BASIC_AUTH_ADMIN` | `false` | The basic-auth user is an admin and may upload and delete fonts |
| `RUSTICDOC_ADMIN_KEYS` | None | Comma-separated keys that may upload and delete fonts. With credentials configured and no admin of any kind, nobody may |
| `RUSTICDOC_OIDC_ISSUER` | None | Accept bearer tokens from this OpenID Connect provider on the protected routes. Must be `https`, or `http` on a loopback host |
| `RUSTICDOC_OIDC_AUDIENCE` | None | The audience tokens must carry. Required with the issuer |
| `RUSTICDOC_OIDC_JWKS_URL` | Discovered from the issuer | Where the signing keys live, under the same rule as the issuer |
| `RUSTICDOC_OIDC_ADMIN_SUBJECTS` | None | Comma-separated token subjects that are admins |
| `RUSTICDOC_OIDC_ADMIN_CLAIM` | None | `claim=value`: tokens whose claim equals the value, or is a list holding it, are admins. The claim may be a dotted path, such as `realm_access.roles` |

OpenID Connect is on when `RUSTICDOC_OIDC_ISSUER` is set. Set the issuer and the audience together: one without the other stops the service from starting. See [authentication](/docs/guides/authentication/).

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
| `RUSTICDOC_TRANSLATIONS_DIR` | None | A directory of `<lang>.json` translation dictionaries for `t`, such as `fr.json`, each a flat object of keys and texts. A bad file stops the service from starting; a file changed while it runs is read again within a second, and one that turns bad keeps its last good version. See [translations](/docs/templates/translations/#dictionaries-on-the-service) |
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
| `RUSTICDOC_PDFTOPPM_BIN` | The first of `/usr/bin/pdftoppm`, `/opt/homebrew/bin/pdftoppm`, `/usr/local/bin/pdftoppm` | Path to poppler's pdftoppm, which draws template previews. `none` disables previews |
| `RUSTICDOC_FONTS_DIR` | `/fonts` | Mounted fonts, picked up by LibreOffice |

The service does not search your `PATH` for engines.

## PDF engine order

Each of these takes a comma-separated list from `lopdf`, `qpdf`, `pdfcpu` and `qpdf-metadata`, tried in the order given. An unknown name stops the service from starting. With no list set, every operation tries `lopdf`, qpdf, then pdfcpu.

| Variable | Operation |
|---|---|
| `RUSTICDOC_PDF_ENGINES_MERGE` | Merge |
| `RUSTICDOC_PDF_ENGINES_SPLIT` | Split |
| `RUSTICDOC_PDF_ENGINES_FLATTEN` | Flatten |
| `RUSTICDOC_PDF_ENGINES_ROTATE` | Rotate |
| `RUSTICDOC_PDF_ENGINES_OPTIMIZE_IMAGES` | Optimize images |
| `RUSTICDOC_PDF_ENGINES_ENCRYPT` | Encrypt. A failed encryption never falls through to the next engine. Naming `pdfcpu` passes passwords as command-line arguments, which this user's other processes can read while it runs, and the service warns at start |
| `RUSTICDOC_PDF_ENGINES_WATERMARK` | Watermark |
| `RUSTICDOC_PDF_ENGINES_STAMP` | Stamp |
| `RUSTICDOC_PDF_ENGINES_EMBED` | Embed files |
| `RUSTICDOC_PDF_ENGINES_EMBED_METADATA` | Metadata of embedded files |
| `RUSTICDOC_PDF_ENGINES_READ_BOOKMARKS` | Read bookmarks |
| `RUSTICDOC_PDF_ENGINES_WRITE_BOOKMARKS` | Write bookmarks |
| `RUSTICDOC_PDF_ENGINES_READ_METADATA` | Read metadata |
| `RUSTICDOC_PDF_ENGINES_WRITE_METADATA` | Write metadata |
| `RUSTICDOC_PDF_ENGINES_FACTUR_X` | Factur-X |

## Store

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_STORE` | None | Where stored templates and uploaded fonts live: `s3://bucket/prefix` or `file:///path`. With none, those routes answer `501`. See [many containers](/docs/operate/many-containers/) |
| `RUSTICDOC_STORE_SCOPE` | `caller` | `caller`: each credential sees only its own templates and aliases, so replacing a key leaves them behind unless it is a tenant key. `shared`: every credential reads and deletes the same ones. Background jobs are always each caller's own |
| `RUSTICDOC_MAX_TEMPLATE_BYTES` | `50MB` | Largest template that can be stored |
| `RUSTICDOC_MAX_TEMPLATES` | `10000` | Most templates one scope may hold, and apart from them, most aliases. Replicas storing at the same moment can pass this by what is in flight |
| `RUSTICDOC_MAX_TEMPLATE_STORAGE` | `5GB` | Most bytes of templates one scope may hold, with the same caveat |
| `RUSTICDOC_MAX_STORED_FONTS` | `2000` | Most fonts the store may hold |
| `RUSTICDOC_MAX_FONT_STORAGE` | `1GB` | Most bytes of fonts the store may hold. Every replica copies them all to disk |
| `RUSTICDOC_FONT_CACHE_DIR` | `rusticdoc-stored-fonts` in the temp directory | This replica's copy of the stored fonts. It is local, and rebuilt when the replica starts |
| `RUSTICDOC_JOB_TTL` | `1h` | How long a background job and its result are kept after it ends. Every replica removes expired jobs from the store |
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
| `RUSTICDOC_WEBHOOK_DISABLED` | `false` | Ignore the webhook headers, on every route |
| `RUSTICDOC_WEBHOOK_ALLOW_PRIVATE` | `false` | Allow webhooks to private and loopback addresses |
| `RUSTICDOC_WEBHOOK_ALLOW_LIST` | None | Regular expressions a webhook URL must match one of |
| `RUSTICDOC_WEBHOOK_DENY_LIST` | None | Regular expressions no webhook URL may match |
| `RUSTICDOC_WEBHOOK_ERROR_ALLOW_LIST` | `RUSTICDOC_WEBHOOK_ALLOW_LIST` | The allow list for error URLs |
| `RUSTICDOC_WEBHOOK_ERROR_DENY_LIST` | None | More refusals for error URLs |
| `RUSTICDOC_MAX_ASYNC_JOBS` | `32` | Webhook and background jobs running at once, across the service. Past it, a request gets `429` before its body is read |
| `RUSTICDOC_MAX_ASYNC_JOBS_PER_CALLER` | `8` | Webhook and background jobs running at once for one caller |
| `RUSTICDOC_DOWNLOAD_FROM_DISABLED` | `false` | Refuse the `downloadFrom` form field entirely |
| `RUSTICDOC_DOWNLOAD_FROM_MAX_ENTRIES` | `10` | Most URLs one request may fetch through `downloadFrom` |
| `RUSTICDOC_DOWNLOAD_FROM_ALLOW_PRIVATE` | `false` | Allow `downloadFrom` to fetch from private and loopback addresses |
| `RUSTICDOC_DOWNLOAD_FROM_ALLOW_LIST` | None | The allow list for `downloadFrom` |
| `RUSTICDOC_DOWNLOAD_FROM_DENY_LIST` | None | The deny list for `downloadFrom` |
| `RUSTICDOC_OUTBOUND_PROXY` | None | Proxy for webhooks, `downloadFrom` and the requests of [PDF signing](#timestamps-and-validation-data). Proxy variables in the environment are otherwise ignored. With this set, DNS pinning no longer applies, because the proxy resolves names itself: it must enforce its own egress policy, including blocking private ranges. The URL and its resolved addresses are still checked first, and the service warns at start |

The allow and deny lists hold comma-separated regular expressions, matched against the URL with its host normalised, on every route. Deny wins. A refusal is `403`. They add to the refusal of private addresses and never relax it. An invalid one stops the service from starting.

## PDF signing

The keys and services for [cryptographic signatures](/docs/guides/pdf-operations/#with-a-certificate): `signature=certificate` on `/v1/pdf/sign` and the `sign` step. Requests never carry a key; they choose one of these by name at most.

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_SIGNING_KEY` | None | The default key: a path to the private key, PEM: PKCS#8 (`PRIVATE KEY`), PKCS#1 (`RSA PRIVATE KEY`) or SEC1 (`EC PRIVATE KEY`), unencrypted. RSA of 2,048 to 8,192 bits, or ECDSA P-256. Or `pkcs11:<label>` for a key on a PKCS#11 token, below |
| `RUSTICDOC_SIGNING_CERT` | None | Path to the certificate for that key, PEM, followed by any chain certificates to include in signatures, in order |
| `RUSTICDOC_SIGNING_ALLOW` | Every caller | Who may sign with the default key: comma-separated [principals](#who-may-use-a-key) |
| `RUSTICDOC_SIGNING_LEVEL` | `b-b` | The PAdES level a request gets when it names none: `b-b`, `b-t`, `b-lt` or `b-lta`. From `b-t` on it needs a timestamp authority |

Set the key and certificate both or neither. At start the service reads every key and refuses to start when only one of a pair is set, a file cannot be read, a key is encrypted or does not match its first certificate, or that certificate is outside its validity period. It warns, and starts, when a certificate allows neither digitalSignature nor nonRepudiation, when it is self-signed, when the chain is out of order, and when a key file can be read by other users. The log names each certificate's subject, algorithm and expiry, never anything about a key. With no key, cryptographic signing answers `503`, and image signing works as before.

A certificate that expires while the service runs makes signing with it answer `503` from then on. Replace the files and restart.

### Named keys

Besides the default key a deployment can hold named keys, which a request chooses with `signingKey=<name>`.

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_SIGNING_KEYS` | None | Comma-separated key names: letters, digits, `-` and `_`, up to 64 characters |
| `RUSTICDOC_SIGNING_KEY_<NAME>` | None | The key of `<name>`, as `RUSTICDOC_SIGNING_KEY`. `<NAME>` is the name in capitals with `-` as `_`, so `acme-eu` becomes `ACME_EU` |
| `RUSTICDOC_SIGNING_CERT_<NAME>` | None | Its certificate chain, as `RUSTICDOC_SIGNING_CERT` |
| `RUSTICDOC_SIGNING_ALLOW_<NAME>` | None | Who may sign with it. Required: a named key every caller may use says so with `*` |

#### Who may use a key

An allow list holds comma-separated entries matched against the caller's principal: exactly (`key:<sha256 of the API key>`, `basic:<sha256 of the user>`, `oidc:<sha256 of the subject>`, `public` when no credentials are configured), by prefix with a trailing `*` (`tenant:*`), or `*` for every caller. `rusticdoc webhook-secret` and the logs show a caller's principal. A caller not on the list gets `403`; a name that is not configured gets `400`.

### Keys in an HSM

A key variable set to `pkcs11:<label>` signs with the private key of that label on a PKCS#11 token: an HSM, a smart card or SoftHSM. The key never leaves the token. RFC 7512 attributes work too: `pkcs11:object=<label>;token=<token label>;slot-id=<n>`. The certificate still comes from the `CERT` file.

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_PKCS11_MODULE` | None | Path to the token's PKCS#11 module, such as `/usr/lib/softhsm/libsofthsm2.so` |
| `RUSTICDOC_PKCS11_TOKEN` | Any | The label of the token that holds the keys |
| `RUSTICDOC_PKCS11_SLOT` | Any | The slot id, instead of or as well as the label |
| `RUSTICDOC_PKCS11_PIN_FILE` | None | A file holding the user PIN. Preferred |
| `RUSTICDOC_PKCS11_PIN` | None | The user PIN itself, when there is no file |

At start the service logs in, finds the key, and has it sign a test message that the certificate's public key must verify, so a key that does not belong to its certificate stops the service from starting. Each key keeps one logged-in session, used by one signature at a time.

### Timestamps and validation data

Levels from `b-t` on ask a timestamp authority (RFC 3161) for a timestamp over the signature; `b-lt` also fetches OCSP responses or CRLs for every certificate in the signer's and the authority's chains, from the URLs those certificates name, and `b-lta` asks for a document timestamp at the end. A failure at any of these makes the request fail with `502`: the service never returns a file at a lower level than asked for.

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_SIGNING_TSA_URL` | None | The timestamp authority, `http` or `https`. Needs `RUSTICDOC_SIGNING_TSA_CERT` |
| `RUSTICDOC_SIGNING_TSA_CERT` | None | A PEM file of the certificates the authority's tokens are trusted through: its own signing certificate, pinned, or the CA certificates it must chain to. Required with a URL: without it the service does not start |
| `RUSTICDOC_SIGNING_TSA_USERNAME`, `RUSTICDOC_SIGNING_TSA_PASSWORD` | None | Basic credentials for it |
| `RUSTICDOC_SIGNING_TSA_HEADERS` | None | Further headers for it, as a JSON object such as `{"X-Api-Key": "..."}` |
| `RUSTICDOC_SIGNING_TSA_TIMEOUT` | `10s` | Time limit for one timestamp request |
| `RUSTICDOC_SIGNING_REVOCATION_TIMEOUT` | `10s` | Time limit for one OCSP, CRL or issuer certificate request |
| `RUSTICDOC_SIGNING_REVOCATION_MAX_BYTES` | `10MiB` | Largest OCSP response, CRL or certificate accepted |
| `RUSTICDOC_SIGNING_OUTBOUND_ALLOW_LIST` | None | Regular expressions a timestamp, OCSP, CRL or issuer URL must match one of |
| `RUSTICDOC_SIGNING_OUTBOUND_DENY_LIST` | None | Regular expressions none of those URLs may match |
| `RUSTICDOC_SIGNING_OUTBOUND_ALLOW_PRIVATE` | `false` | Allow those URLs to reach private and loopback addresses |

A token is used only when its signature verifies with the certificate it names, and that certificate is one of `RUSTICDOC_SIGNING_TSA_CERT` or chains to one through CA certificates in the token, carries a critical `id-kp-timeStamping` extended key usage, and was valid at the token's time; otherwise the request fails with `502`. Revocation data is checked the same way before it goes in the DSS: an OCSP response must answer for exactly the certificate asked about, be signed by the certificate's issuer or by a responder certificate the issuer signed with `id-kp-OCSPSigning`, and be current (this update no later than now, next update after now, five minutes' clock skew either way); a CRL must be signed with the issuer's key and be current. One that fails is set aside for the next source, OCSP then CRLs, and the request fails with `502` when none is left.

These requests follow the same rules as webhooks: the URL is checked against the lists with its host normalised, private addresses are refused unless allowed, the connection is pinned to the addresses that were checked, redirects are not followed, and `RUSTICDOC_OUTBOUND_PROXY` applies. The whole of a signature, network included, keeps to `RUSTICDOC_API_TIMEOUT`, and one signature makes at most 32 requests for validation data.

## Passed to engines and otherwise unused

The service hands these on to child engines from its own environment, and passes nothing else. `TMPDIR` also moves the temp directory the service itself uses.

`PATH`, `LANG`, `LC_ALL`, `HOME`, `TMPDIR`, `FONTCONFIG_FILE`, `TZ`, `PYTHONPATH`, `UNO_PATH`, `URE_BOOTSTRAP`

## Tests only

The test suites read `RUSTICDOC_WRITE_FIXTURES`, a directory to write PDF fixtures to, and `RICH_KEEP`, which keeps generated documents. The service never reads them.
