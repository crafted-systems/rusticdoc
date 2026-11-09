---
title: Authentication
description: Turn on API keys, tenant keys, basic auth or OpenID Connect, choose who is an admin, and see which routes stay open.
sidebar: { order: 7 }
---

With no credentials configured, anyone who can reach the service can use it, and it warns you at every start. You can turn on API keys, tenant keys, basic auth or OpenID Connect, and choose who can change fonts.

The service speaks plain HTTP, so terminate TLS in front of it (see [security](/docs/operate/security/)).

## API keys

```bash
RUSTICDOC_API_KEYS=first-key,second-key
```

Callers send a key in either header:

```bash
curl -H "X-Api-Key: $KEY" -F files=@report.docx \
  http://localhost:9027/v1/convert -o report.pdf
curl -H "Authorization: Bearer $KEY" -F files=@report.docx \
  http://localhost:9027/v1/convert -o report.pdf
```

For an API key, `Bearer` needs its capital letter, and `bearer` gets a `401`. An OpenID Connect token takes the scheme in any case. If a request has both headers, the service tries the `Authorization` one.

Each key in `RUSTICDOC_API_KEYS` is its own caller, and its stored templates, aliases, background jobs and webhook signing key belong to that key. Replace the key and you leave them all behind and get a new signing key. To replace keys without losing anything, use tenant keys.

## Tenant keys

```bash
RUSTICDOC_TENANT_KEYS=acme:old-secret,acme:new-secret,globex:another-secret
```

Each entry is `tenant:key`. All of a tenant's keys are the same caller (`tenant:acme` here), whichever one a request sends. The templates, aliases, jobs and webhook signing key belong to the tenant, so you can replace a key without losing anything:

1. Add the new key beside the old one: `acme:old-secret,acme:new-secret`.
2. Move clients to the new key.
3. Remove the old key. It stops working immediately.

Callers send a tenant key the same way as an API key, in `X-Api-Key` or `Authorization: Bearer`.

A tenant name is 1 to 64 lowercase letters, digits and hyphens. A key can belong to only one caller, so the service refuses to start if a key is listed twice or is also in `RUSTICDOC_API_KEYS`.

Turning tenant keys on changes nothing for the keys in `RUSTICDOC_API_KEYS`, which keep the caller they always had. To move an existing key's templates to a tenant, store them again under a tenant key.

`rusticdoc webhook-secret --tenant acme` prints the tenant's signing key for [webhooks](/docs/guides/webhooks/).

## Basic auth

```bash
RUSTICDOC_BASIC_AUTH_USERNAME=ada
RUSTICDOC_BASIC_AUTH_PASSWORD=a-long-password
```

```bash
curl -u ada:a-long-password http://localhost:9027/v1/version
```

Basic auth has a single user, who can change fonts only if you set `RUSTICDOC_BASIC_AUTH_ADMIN=true` (see [admins](#admins)). A refused request gets a `WWW-Authenticate: Basic realm="rusticdoc"` header.

## OpenID Connect

The service accepts bearer tokens from one provider. Setting the issuer turns this on, and you have to set the audience with it:

```bash
RUSTICDOC_OIDC_ISSUER=https://id.example.com
RUSTICDOC_OIDC_AUDIENCE=rusticdoc
# Optional. Otherwise the keys come from the issuer's discovery document.
RUSTICDOC_OIDC_JWKS_URL=https://id.example.com/keys
```

```bash
curl -H "Authorization: Bearer $TOKEN" http://localhost:9027/v1/version
```

A token passes when its signature checks out and its issuer, audience and expiry are right.

- `exp`, `iss` and `aud` are required.
- Only `RS256` and `ES256` are accepted.
- `RUSTICDOC_OIDC_ISSUER` and `RUSTICDOC_OIDC_JWKS_URL` must be `https`, or `http` on a loopback host.
- Keys are fetched when the first token arrives, kept for 5 minutes, and fetched at most once every 30 seconds.

The caller is the token's `sub`, so each subject has its own templates and webhook signing key. Tokens without a `sub` are treated as a caller of their own. Which users are admins is up to you (see [admins](#admins)).

This is tested with a local key server and RS256 tokens, where a good token passes and an expired token, a wrong audience or a wrong issuer gets a `401`. It isn't tested against a production identity provider yet.

## Using several together

You can configure them all. The service tries an API key first, then a tenant key, an admin key, basic auth and finally OpenID Connect, and the first one that matches decides who the caller is.

## Configuration mistakes stop the service

Rather than run half-protected, the service refuses to start and tells you why:

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

Error messages about tenant keys never repeat the entry, because it may hold a key.

An empty `RUSTICDOC_API_KEYS=` counts as unset, so the service stays open.

## What stays open

Once credentials are configured, each route returns:

| Route | Without credentials | With credentials |
|---|---|---|
| `GET /health` | `200`, `{"status":"up"}` and nothing else | `200`, with engines and pool numbers |
| `GET /ready` | `200` with the status, or `503` while fonts sync | The same, with details when ready |
| `GET /openapi.json` | `200` | `200` |
| `GET /metrics` | `401`, unless `RUSTICDOC_METRICS_PUBLIC=true` | `200` |
| `GET /docs` | `401`, or `200` with `RUSTICDOC_ENABLE_DOCS=true` | `404`, or `200` with `RUSTICDOC_ENABLE_DOCS=true` |
| Every `/v1/*` route | `401` | Served |

Responses from the open routes also carry no version header.

A `401` has a JSON body, `{"error":"unauthorized","message":"Unauthorized"}`.

## Admins

Only an admin can upload and delete [fonts](/docs/guides/fonts/#admin-keys), because a font changes everyone's documents. Once credentials are configured, nobody is an admin until you say who:

| Setting | Who is an admin |
|---|---|
| `RUSTICDOC_ADMIN_KEYS=ops-key` | A request that sends one of these keys |
| `RUSTICDOC_BASIC_AUTH_ADMIN=true` | The basic-auth user |
| `RUSTICDOC_OIDC_ADMIN_SUBJECTS=alice,bob` | Tokens whose `sub` is listed |
| `RUSTICDOC_OIDC_ADMIN_CLAIM=groups=rusticdoc-admins` | Tokens whose claim equals the value, or is a list holding it |

The claim can be a path into nested claims, like Keycloak's `realm_access.roles=rusticdoc-admin`, or a whole claim name with dots in it, like `https://example.com/roles=admin`. The value has to match a whole entry, so `groups=admin` doesn't match `admins`. You can combine any of these settings.

Being an admin changes nothing else. An admin is still the caller its credential makes it, with its own templates.

An admin key works anywhere a credential is asked for, as a caller of its own, so it has its own stored templates and doesn't see other callers'. If you set only `RUSTICDOC_ADMIN_KEYS`, with no other credentials, the service stays open to anyone except for font changes.

```bash
RUSTICDOC_API_KEYS=first-key
RUSTICDOC_ADMIN_KEYS=ops-key
```

## With no credentials

Every caller is the same principal, `public`, sharing one scope of templates, and webhooks sign with the secret itself. If the service has a store, the warning at start also says that anyone can store, read and delete templates and fonts. Only run it this way on a network you trust.
