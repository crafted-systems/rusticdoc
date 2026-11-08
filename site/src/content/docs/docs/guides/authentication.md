---
title: Authentication
description: Protect the service with API keys, basic auth or OpenID Connect, and know which routes stay open and what admin keys add.
sidebar: { order: 7 }
---

With no credentials configured, the service answers anyone and warns at every start. After this page you can turn on API keys, basic auth or OpenID Connect, and you will know what stays open.

The service speaks plain HTTP, so terminate TLS in front of it. See [security](/docs/operate/security/).

## API keys

```bash
RUSTICDOC_API_KEYS=first-key,second-key
```

Callers send a key in either header:

```bash
curl -H "X-Api-Key: $KEY" -F files=@report.docx http://localhost:9027/v1/convert -o report.pdf
curl -H "Authorization: Bearer $KEY" -F files=@report.docx http://localhost:9027/v1/convert -o report.pdf
```

Write `Bearer` with that capital for an API key: `bearer` gets `401`. If both headers are present, the `Authorization` one is tried. An OpenID Connect token takes the scheme in any case.

Each key is its own caller. Stored templates and webhook signing keys belong to the key, so replacing a key leaves its templates behind and changes its signing key. To rotate a key, list the old and the new together, move clients over, and store their templates again under the new key. `RUSTICDOC_STORE_SCOPE=shared` avoids the template half of this. See [stored templates](/docs/guides/stored-templates/#scope).

## Basic auth

```bash
RUSTICDOC_BASIC_AUTH_USERNAME=ada
RUSTICDOC_BASIC_AUTH_PASSWORD=a-long-password
```

```bash
curl -u ada:a-long-password http://localhost:9027/v1/version
```

There is one user. A refused request gets a `WWW-Authenticate: Basic realm="rusticdoc"` header. Basic auth cannot change fonts, because that takes an admin key.

## OpenID Connect

The service accepts bearer tokens from one provider. Setting the issuer turns it on, and the audience is required with it:

```bash
RUSTICDOC_OIDC_ISSUER=https://id.example.com
RUSTICDOC_OIDC_AUDIENCE=rusticdoc
# Optional. Otherwise the keys come from the issuer's discovery document.
RUSTICDOC_OIDC_JWKS_URL=https://id.example.com/keys
```

```bash
curl -H "Authorization: Bearer $TOKEN" http://localhost:9027/v1/version
```

A token passes when its signature checks out and its issuer, audience and expiry are right. The rules are:

- `exp`, `iss` and `aud` are required.
- Only `RS256` and `ES256` are accepted.
- `RUSTICDOC_OIDC_ISSUER` and `RUSTICDOC_OIDC_JWKS_URL` must be `https`, or `http` on a loopback host.
- Keys are fetched when the first token arrives, kept for 5 minutes, and fetched at most once every 30 seconds.

The caller is the token's `sub`, so each subject has its own templates and webhook signing key. Tokens without a `sub` are treated as a caller of their own.

For this guide it was run with a local key server and RS256 tokens: a good token passed, and expired tokens, a wrong audience and a wrong issuer got `401`. It has not been run against a production identity provider.

## Using several together

You can configure all three. The service tries an API key or admin key first, then basic auth, then OpenID Connect. The first that matches decides who the caller is.

## Configuration mistakes stop the service

The service refuses to start rather than run half-protected, and says why:

| Setting | Message |
|---|---|
| `RUSTICDOC_API_KEYS=" , "` | `RUSTICDOC_API_KEYS is set but holds no key` |
| A basic-auth username without a password | `RUSTICDOC_BASIC_AUTH_USERNAME and RUSTICDOC_BASIC_AUTH_PASSWORD go together` |
| An OpenID Connect issuer without an audience, or the reverse | `RUSTICDOC_OIDC_ISSUER and RUSTICDOC_OIDC_AUDIENCE go together` |
| `RUSTICDOC_OIDC_ISSUER=http://id.example.com` | `RUSTICDOC_OIDC_ISSUER must be https, or http on a loopback host, not 'http://id.example.com'` |

An empty `RUSTICDOC_API_KEYS=` counts as unset, which leaves the service open.

## What stays open

With credentials configured:

| Route | Without credentials | With credentials |
|---|---|---|
| `GET /health` | `200`, `{"status":"up"}` and nothing else | `200`, with engines and pool numbers |
| `GET /ready` | `200` with the status, or `503` while fonts sync | The same, with details when ready |
| `GET /openapi.json` | `200` | `200` |
| `GET /metrics` | `401`, unless `RUSTICDOC_METRICS_PUBLIC=true` | `200` |
| `GET /docs` | `401`, or `200` with `RUSTICDOC_ENABLE_DOCS=true` | `404`, or `200` with `RUSTICDOC_ENABLE_DOCS=true` |
| Every `/v1/*` route | `401` | Served |

With credentials configured, responses from the open routes carry no version header.

A `401` is JSON: `{"error":"unauthorized","message":"Unauthorized"}`.

## Admin keys

```bash
RUSTICDOC_API_KEYS=first-key
RUSTICDOC_ADMIN_KEYS=ops-key
```

Admin keys may upload and delete [fonts](/docs/guides/fonts/#admin-keys), which no other credential may do. Sending one in `X-Api-Key` or `Authorization: Bearer` also works anywhere a credential is asked for, as a caller of its own: it has its own stored templates and does not see other callers'.

Setting only `RUSTICDOC_ADMIN_KEYS`, with no other credentials, leaves the service open to anyone except for font changes.

## With no credentials

Every caller is the one principal `public`, which shares one scope of templates, and webhooks sign with the secret itself. The service warns at every start, and with a store it warns that anyone can store, read and delete templates and fonts. Run this way only on a network you trust.
