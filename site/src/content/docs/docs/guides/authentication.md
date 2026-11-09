---
title: Authentication
description: Protect the service with API keys, tenant keys, basic auth or OpenID Connect, choose who is an admin, and know which routes stay open.
sidebar: { order: 7 }
---

With no credentials configured, the service answers anyone and warns at every start. After this page you can turn on API keys, tenant keys, basic auth or OpenID Connect, choose who may change fonts, and you will know what stays open.

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

Each key in `RUSTICDOC_API_KEYS` is its own caller. Stored templates, aliases, background jobs and webhook signing keys belong to the key, so replacing it leaves them behind and changes its signing key. To replace keys without that, give them a tenant's name, as the next section shows.

## Tenant keys

```bash
RUSTICDOC_TENANT_KEYS=acme:old-secret,acme:new-secret,globex:another-secret
```

Each entry is `tenant:key`. Every key of one tenant is the same caller, `tenant:acme`, whichever of them a request sends. Its templates, aliases, jobs and webhook signing key belong to the tenant, so you can replace a key without losing anything:

1. Add the new key beside the old one: `acme:old-secret,acme:new-secret`.
2. Move clients to the new key.
3. Remove the old key. It stops working at once; the tenant keeps everything.

Callers send a tenant key as they send an API key, in `X-Api-Key` or `Authorization: Bearer`.

A tenant name is 1 to 64 lowercase letters, digits and hyphens. A key may belong to one caller only, so the service refuses to start when a key is listed twice or is also in `RUSTICDOC_API_KEYS`. Keys in `RUSTICDOC_API_KEYS` keep the caller they always had, so turning tenant keys on changes nothing for them. To move an existing key's templates to a tenant, store them again under a tenant key.

`rusticdoc webhook-secret --tenant acme` prints the tenant's webhook signing key. See [webhooks](/docs/guides/webhooks/).

## Basic auth

```bash
RUSTICDOC_BASIC_AUTH_USERNAME=ada
RUSTICDOC_BASIC_AUTH_PASSWORD=a-long-password
```

```bash
curl -u ada:a-long-password http://localhost:9027/v1/version
```

There is one user. A refused request gets a `WWW-Authenticate: Basic realm="rusticdoc"` header. The user may change fonts only when `RUSTICDOC_BASIC_AUTH_ADMIN=true`. See [admins](#admins).

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

The caller is the token's `sub`, so each subject has its own templates and webhook signing key. Tokens without a `sub` are treated as a caller of their own. Which users are admins is up to you; see [admins](#admins).

For this guide it was run with a local key server and RS256 tokens: a good token passed, and expired tokens, a wrong audience and a wrong issuer got `401`. It has not been run against a production identity provider.

## Using several together

You can configure them all. The service tries an API key first, then a tenant key, then an admin key, then basic auth, then OpenID Connect. The first that matches decides who the caller is.

## Configuration mistakes stop the service

The service refuses to start rather than run half-protected, and says why:

| Setting | Message |
|---|---|
| `RUSTICDOC_API_KEYS=" , "` | `RUSTICDOC_API_KEYS is set but holds no key` |
| A basic-auth username without a password | `RUSTICDOC_BASIC_AUTH_USERNAME and RUSTICDOC_BASIC_AUTH_PASSWORD go together` |
| An OpenID Connect issuer without an audience, or the reverse | `RUSTICDOC_OIDC_ISSUER and RUSTICDOC_OIDC_AUDIENCE go together` |
| `RUSTICDOC_OIDC_ISSUER=http://id.example.com` | `RUSTICDOC_OIDC_ISSUER must be https, or http on a loopback host, not 'http://id.example.com'` |
| `RUSTICDOC_TENANT_KEYS=Acme:secret` | `RUSTICDOC_TENANT_KEYS: each entry is tenant:key, and a tenant is 1 to 64 lowercase letters, digits and hyphens` |
| A key in two tenants, or in a tenant and `RUSTICDOC_API_KEYS` | `RUSTICDOC_TENANT_KEYS: a key is listed twice, or is also in RUSTICDOC_API_KEYS; a key belongs to one caller` |
| `RUSTICDOC_BASIC_AUTH_ADMIN=true` without basic auth | `RUSTICDOC_BASIC_AUTH_ADMIN is on but basic auth is not configured` |
| `RUSTICDOC_OIDC_ADMIN_SUBJECTS` or `RUSTICDOC_OIDC_ADMIN_CLAIM` without OpenID Connect | `RUSTICDOC_OIDC_ADMIN_SUBJECTS and RUSTICDOC_OIDC_ADMIN_CLAIM need OpenID Connect configured` |
| `RUSTICDOC_OIDC_ADMIN_CLAIM=groups` | `RUSTICDOC_OIDC_ADMIN_CLAIM is 'groups'; use claim=value, such as groups=rusticdoc-admins` |

Error messages about tenant keys never repeat an entry, since it may hold a key.

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

## Admins

Only an admin may upload and delete [fonts](/docs/guides/fonts/#admin-keys), because a font changes everyone's documents. With credentials configured nobody is an admin until you say who:

| Setting | Who is an admin |
|---|---|
| `RUSTICDOC_ADMIN_KEYS=ops-key` | A request that sends one of these keys |
| `RUSTICDOC_BASIC_AUTH_ADMIN=true` | The basic-auth user |
| `RUSTICDOC_OIDC_ADMIN_SUBJECTS=alice,bob` | Tokens whose `sub` is listed |
| `RUSTICDOC_OIDC_ADMIN_CLAIM=groups=rusticdoc-admins` | Tokens whose claim equals the value, or is a list holding it |

The claim can be a path into nested claims, such as Keycloak's `realm_access.roles=rusticdoc-admin`, or a whole claim name with dots in it, such as `https://example.com/roles=admin`. The value must match a whole entry: `groups=admin` does not match `admins`. Any of these settings can be combined.

Being an admin changes nothing else. An admin is still the caller its credential makes it, with its own templates.

An admin key works anywhere a credential is asked for, as a caller of its own: it has its own stored templates and does not see other callers'. Setting only `RUSTICDOC_ADMIN_KEYS`, with no other credentials, leaves the service open to anyone except for font changes.

```bash
RUSTICDOC_API_KEYS=first-key
RUSTICDOC_ADMIN_KEYS=ops-key
```

## With no credentials

Every caller is the one principal `public`, which shares one scope of templates, and webhooks sign with the secret itself. The service warns at every start, and with a store it warns that anyone can store, read and delete templates and fonts. Run this way only on a network you trust.
