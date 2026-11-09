---
title: Errors
description: Every error status and code, the JSON body, and what to do about each.
---

Use this page to work out what an error means and what to change. Errors are JSON on every route, apart from the few answers listed under [responses that are not JSON](#responses-that-are-not-json).

## The JSON body

```json
{
  "error": "invalid_request",
  "message": "invalid request: unknown step 'teleport'; file-carrying steps (watermark, stamp, embed, factur-x) take their own fields"
}
```

| Field | Meaning |
|---|---|
| `error` | A code your program can match on |
| `message` | What went wrong, for a person. Do not match on this text |

Every response carries `x-request-id`. Send your own `X-Request-Id` to set it. Log it: the service logs the full error under the same id.

A request's own mistakes say what they were. A `5xx` reads as the status's name, such as `Internal Server Error`, and the detail is only in the log. That keeps engine output and temp paths out of responses. So for a `5xx`, look in the log under the request id.

```bash test
curl -F template=@letter.txt -F steps=teleport \
  http://localhost:9027/v1/render -o error.json || true
jq -e '.error == "invalid_request"' error.json
```

## Codes by status

### 400

| Code | Meaning | What to do |
|---|---|---|
| `invalid_request` | The request is wrong: a missing file, an unknown step, a bad option, invalid JSON, a file that is not a PDF, a bad webhook URL, a conversion past `RUSTICDOC_LIBREOFFICE_MAX_DISK`. Also a form over `RUSTICDOC_BODY_LIMIT`, with `multipart form: Request payload is too large` | Read `message`, which names the field |

```bash test=fail
curl -F template=@letter.txt -F 'data=not json' http://localhost:9027/v1/render
```

A step that needs a PDF, run on something else, is also here: `'letter.txt' is not a PDF`. Put `convert` before it. See [the pipeline guide](/docs/guides/pipeline/#steps).

### 401

| Code | Meaning | What to do |
|---|---|---|
| `unauthorized` | No valid credentials | Send a key or token. See [authentication](/docs/guides/authentication/). The message is always `Unauthorized`, whatever was wrong, and the log has what was presented as a digest prefix |

### 403

| Code | Meaning | What to do |
|---|---|---|
| `admin_required` | This route changes fonts, and the caller is not an admin | Send one of `RUSTICDOC_ADMIN_KEYS`, or make the caller an admin with `RUSTICDOC_BASIC_AUTH_ADMIN`, `RUSTICDOC_OIDC_ADMIN_SUBJECTS` or `RUSTICDOC_OIDC_ADMIN_CLAIM`. See [authentication](/docs/guides/authentication/) |
| `quota_exceeded` | Stored templates or fonts are at a limit | Delete some, or raise `RUSTICDOC_MAX_TEMPLATES`, `RUSTICDOC_MAX_TEMPLATE_STORAGE`, `RUSTICDOC_MAX_STORED_FONTS` or `RUSTICDOC_MAX_FONT_STORAGE` |
| `url_not_allowed` | A webhook or `downloadFrom` URL fails an allow or deny list | Change the URL, or the lists in `RUSTICDOC_WEBHOOK_ALLOW_LIST`, `RUSTICDOC_WEBHOOK_DENY_LIST`, `RUSTICDOC_DOWNLOAD_FROM_ALLOW_LIST` or `RUSTICDOC_DOWNLOAD_FROM_DENY_LIST` |

A private address is a `400`, not a `403`.

### 404

| Code | Meaning | What to do |
|---|---|---|
| `not_found` | No stored template, alias, font or job with that id or name for this caller, including another caller's, and a job whose result has expired | Check the id and the credential. See [scope](/docs/guides/stored-templates/#scope) |
| `template_not_cached` | `templateHash` is not in this replica's memory | Send the file. See [sending a hash](/docs/guides/pipeline/#sending-a-hash) |

```bash test=fail
curl -F templateHash=deadbeef -F "data=<letter.json" \
  http://localhost:9027/v1/render
```

### 409

| Code | Meaning | What to do |
|---|---|---|
| `job_not_done` | The [background job](/docs/guides/background-jobs/)'s result was asked for before the job ended | Poll `GET /v1/jobs/{id}` until `status` is `done`, waiting as `Retry-After` says |
| `job_failed` | The job failed, so it has no result | Read `error` in `GET /v1/jobs/{id}`. A job that stopped because its container did has the code `job_lost` there; send the request again |

### 413

| Code | Meaning | What to do |
|---|---|---|
| `payload_too_large` | A JSON request body is over `RUSTICDOC_BODY_LIMIT` | Send less data, or raise the limit |
| `template_too_large` | A template to store is over `RUSTICDOC_MAX_TEMPLATE_BYTES` | Make it smaller, or raise the limit |
| `reported` | A template, or an HTML page being laid out, needed more memory than one render may use, with the message `The document needs more memory than one render may use.` | Reduce the data or the page, or raise `RUSTICDOC_RENDER_MEMORY` |

### 422

| Code | Meaning | What to do |
|---|---|---|
| `template_error` | The template has a syntax error, uses an undefined value or an unknown filter, or a filter refused a value. The message names the tag and, in Office files, where it sits | Fix it. [`/v1/templates/validate`](/docs/templates/validate/) lists every problem at once |
| `reported` | The render worker stopped: `The document used more CPU time than one render may use.` or `The document could not be rendered.` | Simplify the template or the page. Look in the log under the request id |

```bash test=fail
curl -F template=@letter.txt -F 'data={"customer":{}}' \
  http://localhost:9027/v1/render
```

### 429

| Code | Meaning | What to do |
|---|---|---|
| `too_many_jobs` | Too many webhook jobs are running, in all or for this caller | Retry later, or raise `RUSTICDOC_MAX_ASYNC_JOBS` or `RUSTICDOC_MAX_ASYNC_JOBS_PER_CALLER`. See [webhooks](/docs/guides/webhooks/#limits) |
| `too_many_requests` | Fonts were reloaded in the last 10 seconds | Retry in a few seconds |

### 500

| Code | Meaning | What to do |
|---|---|---|
| `engine_failed` | LibreOffice, qpdf or another engine ran and failed. This includes a conversion LibreOffice cannot do, such as DOCX to XLSX, and a wrong `password` | Look in the log under the request id. Check the input and the target format |
| `io_error` | The service hit a file or system error | Look in the log |
| `store_integrity` | What the store returned does not match its hash | Look in the log. Restore the object from a backup |

```bash test=fail
curl -F files=@report.docx -F to=xlsx http://localhost:9027/v1/convert
```

An unhandled fault also answers `500`, with no JSON body.

### 501

| Code | Meaning | What to do |
|---|---|---|
| `store_not_configured` | The service has no store, so it cannot do this | Set `RUSTICDOC_STORE`. See [stored templates](/docs/guides/stored-templates/#turn-it-on) |
| `not_supported` | The template's format cannot hold tags, such as a `.bin` file sent with `data` | Use a [supported format](/docs/guides/stored-templates/#upload-a-template). The message reads `Not Implemented`, and the log has the format |

### 503

| Code | Meaning | What to do |
|---|---|---|
| `timeout` | The job ran past its limit | Raise `RUSTICDOC_API_TIMEOUT` (default `30s`) or `RUSTICDOC_BATCH_TIMEOUT` (default `5m`), or send less work |
| `engine_unavailable` | An engine is not configured, or conversion is disabled with `RUSTICDOC_CONVERT_DISABLED` | Check `GET /v1/version` and the settings |
| `busy` | Request bodies in memory have reached `RUSTICDOC_MAX_INFLIGHT_BYTES`, or the service is shutting down. Comes with `Retry-After: 5` | Retry after that time |
| `store_unavailable` | The store cannot be reached | Retry. Check the store. Requests that send their template as a file keep working |

`/ready` answers `503` with `{"status":"syncing"}` while a new replica copies its fonts. That is not an error body. See [running many containers](/docs/operate/many-containers/).

## Responses that are not JSON

Some answers come from the HTTP layer, before a route's code runs, so they are not in the JSON shape. Check `content-type` before you parse.

| Case | Status | Body |
|---|---|---|
| A query value is wrong, such as `?limit=abc` on `/v1/templates` | `400` | Text, starting `Failed to deserialize query string` |
| An unknown path | `404` | Empty |
| A known path with the wrong method | `405` | Empty, with an `allow` header |
| A request still running 5 seconds past its time limit | `503` | Empty |

```bash test
curl -s http://localhost:9027/v1/nope -D headers.txt -o error.txt || true
grep -q "^HTTP/1.1 404" headers.txt
test ! -s error.txt
```

A body that is neither multipart nor JSON is a route's own `400`, in the JSON shape.

## Failures after a 204

With a webhook, a failure during the job goes to your URL as JSON, with the same `error` and `message`, plus `status` and `requestId`. See [webhooks](/docs/guides/webhooks/#what-a-delivery-looks-like). Mistakes in the form are still answered at once.
