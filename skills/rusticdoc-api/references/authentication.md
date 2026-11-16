# Authentication

<!-- Generated from site/src/content/docs/docs/guides/authentication.md by site/scripts/generate-skills.mjs. Do not edit. -->

Turn on the API key and see which routes stay open.

With no key configured, anyone who can reach the service can use it, and it warns you at every start. Set the API key to require it.

The service speaks plain HTTP, so terminate TLS in front of it (see [security](https://rusticdoc.com/docs/operate/security/)).

## The API key

```bash
RUSTICDOC_API_KEY=a-long-random-key
```

The service has one key. Make it long and random, for example with `openssl rand -hex 32`. The service warns at every start about a key shorter than 16 characters and checks nothing else of its strength. It names the caller in the logs and its stored files after a digest of the key, so anyone who can read either could guess a short one.

Callers send the key in either header:

```bash
curl -H "X-Api-Key: $KEY" -F files=@report.docx \
  http://localhost:9027/v1/convert -o report.pdf
curl -H "Authorization: Bearer $KEY" -F files=@report.docx \
  http://localhost:9027/v1/convert -o report.pdf
```

`Bearer` needs its capital letter, and `bearer` gets a `401`. If a request has both headers, the service tries the `Authorization` one. The key is the only credential, so basic auth and tokens from an identity provider get a `401`.

The key can use every route, including uploading and deleting [fonts](fonts.md#who-can-change-fonts).

Stored templates and aliases don't depend on the key, so they stay when you replace it. Background jobs and the webhook signing key belong to the key: replace it and you leave running jobs behind and get a new signing key.

## Configuration mistakes stop the service

Rather than run half-protected, the service refuses to start and tells you why:

| Setting | Message |
|---|---|
| `RUSTICDOC_API_KEY=" "` | `RUSTICDOC_API_KEY is set but holds no key` |
| `RUSTICDOC_API_KEY=first-key,second-key` | `RUSTICDOC_API_KEY holds one key, with no comma in it` |
| A [removed setting](https://rusticdoc.com/docs/reference/configuration/#removed-settings), such as `RUSTICDOC_API_KEYS` | `RUSTICDOC_API_KEYS is no longer supported; callers authenticate with the key in RUSTICDOC_API_KEY` |

An empty `RUSTICDOC_API_KEY=` counts as unset, so the service stays open.

## What stays open

Once the key is configured, each route returns:

| Route | Without the key | With the key |
|---|---|---|
| `GET /health` | `200`, `{"status":"up"}` and nothing else | `200`, with engines and pool numbers |
| `GET /ready` | `200` with the status, or `503` while fonts sync | The same, with details when ready |
| `GET /openapi.json` | `200` | `200` |
| `GET /metrics` | `401`, unless `RUSTICDOC_METRICS_PUBLIC=true` | `200` |
| `GET /docs` | `401`, or `200` with `RUSTICDOC_ENABLE_DOCS=true` | `404`, or `200` with `RUSTICDOC_ENABLE_DOCS=true` |
| Every `/v1/*` route | `401` | Served |

Responses from the open routes also carry no version header.

A `401` has a JSON body, `{"error":"unauthorized","message":"Unauthorized"}`.

## With no key

Webhooks sign with the secret itself. If the service has a store, the warning at start also says that anyone can store, read and delete templates and fonts. Only run it this way on a network you trust.
