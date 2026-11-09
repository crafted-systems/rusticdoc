---
title: Configuration
description: Every environment variable rusticdoc reads, with its default and purpose.
---

The service is configured through environment variables, and all of them are optional. With none set it starts, lets anyone use it, and warns you about that.

## How values are read

| Type | Accepted | Not accepted |
|---|---|---|
| Size | `512KB`, `64MB`, `1GB` or a bare number of bytes. Case doesn't matter, and each unit is a multiple of 1024 | `2g`, `1.5GB`, `2GiB` |
| Duration | `500ms`, `30s`, `2m`, `1h`, or a number of seconds like `1.5` | `1m30s` |
| Boolean | `1`, `true` or `yes` turns it on. Anything else is off, including `TRUE` | |
| List | Comma-separated, with spaces trimmed | |

An empty variable counts as unset.

A bad value doesn't always stop the service. These do stop it from starting, with a message that names the problem:

- A bad value in `RUSTICDOC_STORE`, `RUSTICDOC_STORE_SCOPE`, `RUSTICDOC_ADMIN_KEYS`, `RUSTICDOC_API_KEYS`, `RUSTICDOC_TENANT_KEYS`, `RUSTICDOC_JOB_TTL`, `RUSTICDOC_RENDER_MODE`, `RUSTICDOC_MAX_TEMPLATE_BYTES`, `RUSTICDOC_MAX_TEMPLATES`, `RUSTICDOC_MAX_TEMPLATE_STORAGE`, `RUSTICDOC_MAX_STORED_FONTS`, `RUSTICDOC_MAX_FONT_STORAGE` or `RUSTICDOC_STORE_SYNC_INTERVAL`.
- `RUSTICDOC_BASIC_AUTH_USERNAME` and `RUSTICDOC_BASIC_AUTH_PASSWORD` when only one is set, or `RUSTICDOC_BASIC_AUTH_ADMIN` without basic auth.
- `RUSTICDOC_OIDC_ADMIN_SUBJECTS` or `RUSTICDOC_OIDC_ADMIN_CLAIM` without OpenID Connect, or in the wrong form.
- OpenID Connect with only one of the issuer and audience set, or with an issuer or JWKS URL that is neither `https` nor `http` on a loopback host.
- A `RUSTICDOC_PDF_ENGINES_*` list that names an unknown engine.
- An invalid regular expression in an allow or deny list.

Any other number, size or duration that the service can't parse is ignored without a warning, and the default stays in place. `RUSTICDOC_RENDER_MEMORY=2g`, for example, leaves the limit at 1 GiB.

The image sets `RUSTICDOC_LIBREOFFICE_BIN`, `RUSTICDOC_QPDF_BIN`, `RUSTICDOC_PDFCPU_BIN`, `RUSTICDOC_FONTS_DIR` and `RUSTICDOC_LOG_FORMAT` for you, along with `TZ=UTC`, `LANG=C.UTF-8` and `LC_ALL=C.UTF-8`.

`deploy/compose.yaml` has variables of its own that shape the container. They're listed under [Docker](/docs/operate/docker/).

## Service

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_BIND` | `0.0.0.0:9027` | Listen address. `rusticdoc serve --bind` overrides it |
| `RUSTICDOC_ROOT_PATH` | `/` | Path prefix that every route sits under. With `/docengine`, the health route becomes `/docengine/health` |
| `RUSTICDOC_CORS_ORIGINS` | None | Origins allowed cross-origin access, comma-separated, or `*` |
| `RUSTICDOC_ENABLE_DOCS` | `false` | Serves an interactive page at `/docs`. It's off by default because that page loads a third-party script from a CDN on the API's origin |
| `RUSTICDOC_METRICS_PUBLIC` | `false` | Keeps `/metrics` open when credentials are configured |
| `RUSTICDOC_LOG_FORMAT` | `json` | `json`, or `pretty` for a terminal |
| `RUSTICDOC_LOG_LEVEL` | `info` | A `tracing` filter like `info` or `rusticdoc=debug` |

## Routes

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_CONVERT_DISABLED` | `false` | `/v1/convert` and `/v1/pdf/convert` return `404`, and conversion steps in `/v1/render` fail with `503` |
| `RUSTICDOC_PDF_DISABLED` | `false` | The `/v1/pdf/*` routes return `404` |

## Authentication

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_API_KEYS` | None | Comma-separated keys for `X-Api-Key` or `Authorization: Bearer`. A value with no key in it, like `,`, stops the service from starting |
| `RUSTICDOC_TENANT_KEYS` | None | Comma-separated `tenant:key` entries. All of a tenant's keys are the same caller, so replacing a key keeps the tenant's templates, aliases, jobs and webhook signing key. A tenant is 1 to 64 lowercase letters, digits and hyphens |
| `RUSTICDOC_BASIC_AUTH_USERNAME` | None | Basic auth user. Set it together with `RUSTICDOC_BASIC_AUTH_PASSWORD` |
| `RUSTICDOC_BASIC_AUTH_PASSWORD` | None | Basic auth password. Set it together with `RUSTICDOC_BASIC_AUTH_USERNAME` |
| `RUSTICDOC_BASIC_AUTH_ADMIN` | `false` | Makes the basic auth user an admin, who can upload and delete fonts |
| `RUSTICDOC_ADMIN_KEYS` | None | Comma-separated keys that can upload and delete fonts. If credentials are configured and there's no admin of any kind, nobody can |
| `RUSTICDOC_OIDC_ISSUER` | None | Accepts bearer tokens from this OpenID Connect provider on the protected routes. Must be `https`, or `http` on a loopback host |
| `RUSTICDOC_OIDC_AUDIENCE` | None | The audience that tokens must carry. Required with the issuer |
| `RUSTICDOC_OIDC_JWKS_URL` | Discovered from the issuer | Where the signing keys live. It follows the same rule as the issuer |
| `RUSTICDOC_OIDC_ADMIN_SUBJECTS` | None | Comma-separated token subjects that are admins |
| `RUSTICDOC_OIDC_ADMIN_CLAIM` | None | `claim=value`. Tokens whose claim equals the value, or is a list that holds it, are admins. The claim can be a dotted path, like `realm_access.roles` |

Setting `RUSTICDOC_OIDC_ISSUER` turns OpenID Connect on. Set the issuer and the audience together, because one without the other stops the service from starting. See [authentication](/docs/guides/authentication/).

## Requests and connections

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_API_TIMEOUT` | `30s` | Time limit for one request, including conversion. A request that goes past it fails with `503`. `deploy/compose.yaml` sets `60s` |
| `RUSTICDOC_BODY_LIMIT` | `64MB` | Largest request body |
| `RUSTICDOC_MAX_INFLIGHT_BYTES` | `2GB` | Request bodies held in memory at the same time. Each request is charged what it declares, or the whole body limit if it streams. Over the budget a request waits, and gets `503` with `Retry-After` at the API timeout. One caller holds at most half, where a caller is a credential or, with none configured, an address |
| `RUSTICDOC_MAX_FORM_PARTS` | `1000` | File parts one form can hold. Text parts are capped at 2,000, which you can't change |
| `RUSTICDOC_BATCH_LIMIT` | `100` | Maximum number of elements in one batch render |
| `RUSTICDOC_BATCH_TIMEOUT` | `5m` | Overall limit for `/v1/render/batch` and a multi-file `/v1/convert`, synchronous or not |
| `RUSTICDOC_HEADER_TIMEOUT` | `10s` | How long a connection can wait for a complete set of request headers, including a new connection that sends nothing and an idle keep-alive one |
| `RUSTICDOC_STALL_TIMEOUT` | `30s` | How long a connection can stay blocked on its client, because nobody reads the response or because a handler is waiting for a body that doesn't arrive. Time a handler spends working doesn't count, and a slow body that keeps moving is allowed up to the request time limit |
| `RUSTICDOC_RESPONSE_WRITE_TIMEOUT` | `5m` | How long writes can spend blocked in total, so a reader that takes a little at a time and stays below the stall timeout still loses its connection. It starts over once everything sent has left the send queue |
| `RUSTICDOC_MAX_CONNECTIONS` | `1024` | Connections served at the same time. Any more wait in the listen queue |
| `RUSTICDOC_MAX_CONNECTIONS_PER_PEER` | `0`, which is off | Connections open at the same time from one IPv4 address or IPv6 /64. Any more are closed. Leave it off behind a load balancer, where every connection has the same peer, and rate limit at the proxy instead |

## Templates and rendering

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_RENDER_MODE` | `isolated` under `serve`, `in-process` otherwise | `isolated` evaluates client templates in a worker process. `in-process` doesn't, so one template can take the service down, and the service warns at startup |
| `RUSTICDOC_RENDER_MEMORY` | `1GB` | Address space of one render worker. A template that goes past it gets a `413`. The limit relies on Linux |
| `RUSTICDOC_RENDER_WORKERS` | `4` | Render workers running at the same time, so peak memory for rendering is this times `RUSTICDOC_RENDER_MEMORY` |
| `RUSTICDOC_RENDER_WORKER_BIN` | The running binary | The binary that has the worker in it |
| `RUSTICDOC_TRANSLATIONS_DIR` | None | A directory of `<lang>.json` translation dictionaries for `t`, like `fr.json`, each one a flat object of keys and texts. A bad file stops the service from starting. A file that changes while the service runs is read again within a second, and if it turns bad the last good version stays in use. See [translations](/docs/templates/translations/#dictionaries-on-the-service) |
| `RUSTICDOC_MAX_RENDER_BYTES` | `64MB` | The most a text template, `batchName` or PDF form tag can write. Office templates stop at the package limit of 256 MiB |

See [sizing](/docs/operate/sizing/) for how these add up.

## Engines and fonts

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_WORKERS` | `2` | LibreOffice conversions running at the same time |
| `RUSTICDOC_LIBREOFFICE_MAX_JOBS` | `10` | Jobs a LibreOffice worker does before it restarts |
| `RUSTICDOC_LIBREOFFICE_MAX_DISK` | `512MB` | The most one conversion can write to disk. If it writes more, the worker restarts and the request gets a `400`. Allow `RUSTICDOC_WORKERS` times this in `/tmp` |
| `RUSTICDOC_LIBREOFFICE_BIN` | The first of `/usr/lib/libreoffice/program/soffice.bin`, `/usr/bin/soffice`, `/Applications/LibreOffice.app/Contents/MacOS/soffice` | Path to LibreOffice. `none` disables it |
| `RUSTICDOC_PYTHON` | `python3` | The interpreter that runs the LibreOffice bridge |
| `RUSTICDOC_QPDF_BIN` | The first of `/usr/bin/qpdf`, `/opt/homebrew/bin/qpdf`, `/usr/local/bin/qpdf` | Path to qpdf. `none` disables it |
| `RUSTICDOC_PDFCPU_BIN` | The first of `/usr/bin/pdfcpu`, `/opt/homebrew/bin/pdfcpu`, `/usr/local/bin/pdfcpu` | Path to pdfcpu. `none` disables it |
| `RUSTICDOC_PDFTOPPM_BIN` | The first of `/usr/bin/pdftoppm`, `/opt/homebrew/bin/pdftoppm`, `/usr/local/bin/pdftoppm` | Path to poppler's pdftoppm, which draws template previews. `none` disables previews |
| `RUSTICDOC_FONTS_DIR` | `/fonts` | Mounted fonts, which LibreOffice picks up |

The service doesn't search your `PATH` for engines.

## PDF engine order

Each of these takes a comma-separated list from `lopdf`, `qpdf`, `pdfcpu` and `qpdf-metadata`, tried in the order you give them. An unknown name stops the service from starting. If you don't set a list, every operation tries `lopdf`, then qpdf, then pdfcpu.

| Variable | Operation |
|---|---|
| `RUSTICDOC_PDF_ENGINES_MERGE` | Merge |
| `RUSTICDOC_PDF_ENGINES_SPLIT` | Split |
| `RUSTICDOC_PDF_ENGINES_FLATTEN` | Flatten |
| `RUSTICDOC_PDF_ENGINES_ROTATE` | Rotate |
| `RUSTICDOC_PDF_ENGINES_OPTIMIZE_IMAGES` | Optimize images |
| `RUSTICDOC_PDF_ENGINES_ENCRYPT` | Encrypt. A failed encryption never falls through to the next engine. If you name `pdfcpu`, passwords are passed as command-line arguments, which this user's other processes can read while it runs, and the service warns at startup |
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
| `RUSTICDOC_STORE` | None | Where stored templates and uploaded fonts live, as `s3://bucket/prefix` or `file:///path`. Without one, those routes return `501`. See [many containers](/docs/operate/many-containers/) |
| `RUSTICDOC_STORE_SCOPE` | `caller` | With `caller`, each credential sees only its own templates and aliases, so replacing a key leaves them behind unless it's a tenant key. With `shared`, every credential reads and deletes the same ones. Background jobs are always private to their caller |
| `RUSTICDOC_MAX_TEMPLATE_BYTES` | `50MB` | Largest template that can be stored |
| `RUSTICDOC_MAX_TEMPLATES` | `10000` | Maximum number of templates in one scope. Aliases have the same limit, counted separately. Replicas that store at the same moment can go over it by whatever is in flight |
| `RUSTICDOC_MAX_TEMPLATE_STORAGE` | `5GB` | Maximum bytes of templates in one scope, with the same caveat |
| `RUSTICDOC_MAX_STORED_FONTS` | `2000` | Maximum number of fonts in the store |
| `RUSTICDOC_MAX_FONT_STORAGE` | `1GB` | Maximum bytes of fonts in the store. Every replica copies them all to disk |
| `RUSTICDOC_FONT_CACHE_DIR` | `rusticdoc-stored-fonts` in the temp directory | This replica's copy of the stored fonts. It's local, and it's rebuilt when the replica starts |
| `RUSTICDOC_JOB_TTL` | `1h` | How long a background job and its result are kept after it ends. Every replica removes expired jobs from the store |
| `RUSTICDOC_STORE_SYNC_INTERVAL` | `30s` | How often a replica looks for fonts added or deleted elsewhere |

## S3 credentials and endpoint

With an `s3://` store, the S3 client reads these from the environment. Credentials never come from the URL, and with no keys set the client uses the pod's or instance's role.

These five are the ones `make replicas` and the deploy files use:

| Variable | Default | Purpose |
|---|---|---|
| `AWS_ACCESS_KEY_ID` | None | Access key |
| `AWS_SECRET_ACCESS_KEY` | None | Secret key |
| `AWS_REGION` | None | The bucket's region |
| `AWS_ENDPOINT` | AWS | Endpoint for any store other than AWS |
| `AWS_ALLOW_HTTP` | `false` | Allows a plain HTTP endpoint |

The client library also reads the rest of its `AWS_*` settings. They're passed through unchanged and aren't tested here:

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
| `RUSTICDOC_WEBHOOK_SECRET` | None | The secret that callers' signing keys are derived from. If it's unset, deliveries aren't signed and the service warns at startup |
| `RUSTICDOC_WEBHOOK_ATTEMPTS` | `3` | Attempts per delivery, including the first |
| `RUSTICDOC_WEBHOOK_DISABLED` | `false` | Ignores the webhook headers on every route |
| `RUSTICDOC_WEBHOOK_ALLOW_PRIVATE` | `false` | Allows webhooks to private and loopback addresses |
| `RUSTICDOC_WEBHOOK_ALLOW_LIST` | None | Regular expressions, one of which a webhook URL must match |
| `RUSTICDOC_WEBHOOK_DENY_LIST` | None | Regular expressions that no webhook URL can match |
| `RUSTICDOC_WEBHOOK_ERROR_ALLOW_LIST` | `RUSTICDOC_WEBHOOK_ALLOW_LIST` | The allow list for error URLs |
| `RUSTICDOC_WEBHOOK_ERROR_DENY_LIST` | None | A further deny list for error URLs |
| `RUSTICDOC_MAX_ASYNC_JOBS` | `32` | Webhook and background jobs running at the same time, across the service. Beyond that, a request gets a `429` before its body is read |
| `RUSTICDOC_MAX_ASYNC_JOBS_PER_CALLER` | `8` | Webhook and background jobs running at the same time for one caller |
| `RUSTICDOC_DOWNLOAD_FROM_DISABLED` | `false` | Refuses the `downloadFrom` form field entirely |
| `RUSTICDOC_DOWNLOAD_FROM_MAX_ENTRIES` | `10` | Maximum number of URLs one request can fetch through `downloadFrom` |
| `RUSTICDOC_DOWNLOAD_FROM_ALLOW_PRIVATE` | `false` | Allows `downloadFrom` to fetch from private and loopback addresses |
| `RUSTICDOC_DOWNLOAD_FROM_ALLOW_LIST` | None | The allow list for `downloadFrom` |
| `RUSTICDOC_DOWNLOAD_FROM_DENY_LIST` | None | The deny list for `downloadFrom` |
| `RUSTICDOC_OUTBOUND_PROXY` | None | Proxy for webhooks, `downloadFrom` and the requests that [PDF signing](#timestamps-and-validation-data) makes. Proxy variables in the environment are otherwise ignored. With this set, DNS pinning no longer applies because the proxy resolves names itself, so the proxy must enforce its own egress policy, including blocking private ranges. The URL and its resolved addresses are still checked first, and the service warns at startup |

The allow and deny lists hold comma-separated regular expressions, matched against the URL with its host normalised, on every route. Deny wins, and a refused URL gets a `403`. The lists add to the refusal of private addresses and never relax it. An invalid expression stops the service from starting.

## PDF signing

These are the keys and services behind [cryptographic signatures](/docs/guides/pdf-operations/#with-a-certificate), meaning `signature=certificate` on `/v1/pdf/sign` and the `sign` step. A request never carries a key. At most it chooses one of these by name.

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_SIGNING_KEY` | None | The default key, as a path to an unencrypted PEM private key: PKCS#8 (`PRIVATE KEY`), PKCS#1 (`RSA PRIVATE KEY`) or SEC1 (`EC PRIVATE KEY`). It has to be RSA of 2,048 to 8,192 bits, or ECDSA P-256. For a key on a PKCS#11 token, use `pkcs11:<label>`, described below |
| `RUSTICDOC_SIGNING_CERT` | None | Path to a PEM file with the certificate for that key, followed by any chain certificates to include in signatures, in order |
| `RUSTICDOC_SIGNING_ALLOW` | Every caller | Who can sign with the default key, as comma-separated [principals](#who-may-use-a-key) |
| `RUSTICDOC_SIGNING_LEVEL` | `b-b` | The PAdES level a request gets when it doesn't name one: `b-b`, `b-t`, `b-lt` or `b-lta`. From `b-t` up it needs a timestamp authority |

Set both the key and the certificate, or neither. The service reads every key when it starts, and refuses to start if only one of a pair is set, a file can't be read, a key is encrypted or doesn't match its first certificate, or that certificate is outside its validity period. It starts with a warning if a certificate allows neither digitalSignature nor nonRepudiation, is self-signed or has its chain out of order, or if a key file can be read by other users. The log names each certificate's subject, algorithm and expiry, and never anything about a key. With no key, cryptographic signing returns `503` and image signing works as before.

If a certificate expires while the service is running, signing with it returns `503` from then on. Replace the files and restart.

### Named keys

As well as the default key, a deployment can hold named keys, which a request chooses with `signingKey=<name>`.

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_SIGNING_KEYS` | None | Comma-separated key names, made of letters, digits, `-` and `_`, up to 64 characters |
| `RUSTICDOC_SIGNING_KEY_<NAME>` | None | The key for `<name>`, in the same form as `RUSTICDOC_SIGNING_KEY`. `<NAME>` is the name in capitals with `-` turned into `_`, so `acme-eu` becomes `ACME_EU` |
| `RUSTICDOC_SIGNING_CERT_<NAME>` | None | Its certificate chain, in the same form as `RUSTICDOC_SIGNING_CERT` |
| `RUSTICDOC_SIGNING_ALLOW_<NAME>` | None | Who can sign with it. Required, so a named key that every caller can use has to say so with `*` |

#### Who may use a key

An allow list holds comma-separated entries matched against the caller's principal. An entry matches exactly (`key:<sha256 of the API key>`, `basic:<sha256 of the user>`, `oidc:<sha256 of the subject>`, or `public` when no credentials are configured), by prefix with a trailing `*` (`tenant:*`), or is `*` for every caller. You can see a caller's principal with `rusticdoc webhook-secret` and in the logs. A caller who isn't on the list gets a `403`, and a key name that isn't configured gets a `400`.

### Keys in an HSM

Set a key variable to `pkcs11:<label>` and the service signs with the private key that has that label on a PKCS#11 token, which can be an HSM, a smart card or SoftHSM. The key never leaves the token. RFC 7512 attributes work too, as in `pkcs11:object=<label>;token=<token label>;slot-id=<n>`. The certificate still comes from the `CERT` file.

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_PKCS11_MODULE` | None | Path to the token's PKCS#11 module, like `/usr/lib/softhsm/libsofthsm2.so` |
| `RUSTICDOC_PKCS11_TOKEN` | Any | The label of the token that holds the keys |
| `RUSTICDOC_PKCS11_SLOT` | Any | The slot id, instead of or as well as the label |
| `RUSTICDOC_PKCS11_PIN_FILE` | None | A file holding the user PIN, which is the preferred way to give it |
| `RUSTICDOC_PKCS11_PIN` | None | The user PIN itself, when there's no file |

When it starts, the service logs in, finds the key and has it sign a test message that the certificate's public key must verify, so a key that doesn't belong to its certificate stops the service from starting. Each key keeps one logged-in session, which one signature uses at a time.

### Timestamps and validation data

From `b-t` up, the service asks a timestamp authority (RFC 3161) for a timestamp over the signature. `b-lt` also fetches OCSP responses or CRLs for every certificate in the signer's and the authority's chains, from the URLs those certificates name, and `b-lta` asks for a document timestamp at the end. If any of these fails, the request fails with `502`, because the service never returns a file at a lower level than you asked for.

| Variable | Default | Purpose |
|---|---|---|
| `RUSTICDOC_SIGNING_TSA_URL` | None | The timestamp authority's `https` URL. Needs `RUSTICDOC_SIGNING_TSA_CERT` |
| `RUSTICDOC_SIGNING_TSA_ALLOW_HTTP` | `false` | Allows a plain `http` authority URL. Tokens are signed and checked either way, but over http the request and any credentials travel in the clear |
| `RUSTICDOC_SIGNING_TSA_CERT` | None | A PEM file of the certificates that the authority's tokens are trusted through. That's either its own signing certificate, pinned, or the CA certificates it must chain to. It's required with a URL, and the service doesn't start without it |
| `RUSTICDOC_SIGNING_TSA_USERNAME`, `RUSTICDOC_SIGNING_TSA_PASSWORD` | None | Basic credentials for the authority |
| `RUSTICDOC_SIGNING_TSA_HEADERS` | None | Extra headers for it, as a JSON object like `{"X-Api-Key": "..."}` |
| `RUSTICDOC_SIGNING_TSA_TIMEOUT` | `10s` | Time limit for one timestamp request |
| `RUSTICDOC_SIGNING_REVOCATION_TIMEOUT` | `10s` | Time limit for one OCSP, CRL or issuer certificate request |
| `RUSTICDOC_SIGNING_REVOCATION_MAX_BYTES` | `10MiB` | Largest OCSP response, CRL or certificate accepted |
| `RUSTICDOC_SIGNING_OUTBOUND_ALLOW_LIST` | None | Regular expressions, one of which a timestamp, OCSP, CRL or issuer URL must match |
| `RUSTICDOC_SIGNING_OUTBOUND_DENY_LIST` | None | Regular expressions that none of those URLs can match |
| `RUSTICDOC_SIGNING_OUTBOUND_ALLOW_PRIVATE` | `false` | Allows those URLs to reach private and loopback addresses |

A token is used only if its time is within five minutes of the service's clock and its signature verifies with the certificate it names. That certificate has to:

- be one of `RUSTICDOC_SIGNING_TSA_CERT`, or chain to one through CA certificates in the token
- carry a critical `id-kp-timeStamping` extended key usage
- be valid both now and at the token's time, along with every certificate on the way and the anchor

Otherwise the request fails with `502`.

Revocation data is checked the same way before it goes in the DSS. The issuer is the certificate whose key verifies the certificate's signature, never one that only has its name. An OCSP response must be for exactly the certificate that was asked about, and be signed by that issuer or by a responder certificate the issuer signed with `id-kp-OCSPSigning`, which is the only certificate taken from the response. It must also be current, meaning this update is no later than now and next update is after now, with five minutes' clock skew either way. A CRL must be signed with the issuer's key and be current. Data that fails is set aside for the next source, OCSP first and then CRLs, and the request fails with `502` when none is left.

These requests follow the same rules as webhooks. The URL is checked against the lists with its host normalised, private addresses are refused unless you allow them, the connection is pinned to the addresses that were checked, redirects aren't followed, and `RUSTICDOC_OUTBOUND_PROXY` applies. A whole signature, network requests included, keeps to `RUSTICDOC_API_TIMEOUT`, and one signature makes at most 32 requests for validation data.

## Passed to engines and otherwise unused

The service passes these on to child engines from its own environment, and passes nothing else. `TMPDIR` also moves the temp directory that the service itself uses.

`PATH`, `LANG`, `LC_ALL`, `HOME`, `TMPDIR`, `FONTCONFIG_FILE`, `TZ`, `PYTHONPATH`, `UNO_PATH`, `URE_BOOTSTRAP`

## Tests only

The test suites read `RUSTICDOC_WRITE_FIXTURES`, a directory to write PDF fixtures to, and `RICH_KEEP`, which keeps generated documents. The service never reads them.
