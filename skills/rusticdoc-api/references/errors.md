# Errors

<!-- Generated from site/src/content/docs/docs/reference/errors.md by site/scripts/generate-skills.mjs. Do not edit. -->

Every error status and code, the JSON body, and what to do about each.

Errors are JSON on every route, apart from [a few responses that aren't](#responses-that-are-not-json).

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
| `message` | What went wrong, for a person. Don't match on this text |

Every response carries `x-request-id`, which you can set by sending your own `X-Request-Id`. Log it, because the service logs the full error under the same id.

When the request itself is wrong, the message says what the mistake was. For a `5xx`, the message is only the status's name, like `Internal Server Error`, which keeps engine output and temp paths out of responses. The detail is only in the log, under the request id.

```bash
curl -F template=@letter.txt -F steps=teleport \
  http://localhost:9027/v1/render -o error.json || true
jq -e '.error == "invalid_request"' error.json
```

## Codes by status

### 400

| Code | Meaning | What to do |
|---|---|---|
| `invalid_request` | The request is wrong: a missing file, an unknown step, a bad option, invalid JSON, a file that isn't a PDF, a bad webhook URL, a conversion past `RUSTICDOC_LIBREOFFICE_MAX_DISK`. Also a form over `RUSTICDOC_BODY_LIMIT`, with `multipart form: Request payload is too large` | Read `message`, which names the field |

```bash
curl -F template=@letter.txt -F 'data=not json' http://localhost:9027/v1/render
```

You also get this code when a step that needs a PDF runs on something else, with a message like `'letter.txt' is not a PDF`. Put `convert` before that step (see [the pipeline guide](pipeline.md#steps)).

### 401

| Code | Meaning | What to do |
|---|---|---|
| `unauthorized` | No valid credentials | Send a key or token. See [authentication](authentication.md). The message is always `Unauthorized`, whatever was wrong, and the log has what was presented as a digest prefix |

### 403

| Code | Meaning | What to do |
|---|---|---|
| `admin_required` | This route changes fonts, and the caller isn't an admin | Send one of `RUSTICDOC_ADMIN_KEYS`, or make the caller an admin with `RUSTICDOC_BASIC_AUTH_ADMIN`, `RUSTICDOC_OIDC_ADMIN_SUBJECTS` or `RUSTICDOC_OIDC_ADMIN_CLAIM`. See [authentication](authentication.md) |
| `quota_exceeded` | Stored templates, fonts or background-job results are at a limit | Delete some, or raise `RUSTICDOC_MAX_TEMPLATES`, `RUSTICDOC_MAX_TEMPLATE_STORAGE`, `RUSTICDOC_MAX_STORED_FONTS` or `RUSTICDOC_MAX_FONT_STORAGE`. For job results, wait for earlier ones to expire or raise `RUSTICDOC_MAX_JOB_STORAGE` |
| `url_not_allowed` | A webhook or `downloadFrom` URL fails an allow or deny list | Change the URL, or the lists in `RUSTICDOC_WEBHOOK_ALLOW_LIST`, `RUSTICDOC_WEBHOOK_DENY_LIST`, `RUSTICDOC_DOWNLOAD_FROM_ALLOW_LIST` or `RUSTICDOC_DOWNLOAD_FROM_DENY_LIST` |

A private address gets a `400`, not a `403`.

### 404

| Code | Meaning | What to do |
|---|---|---|
| `not_found` | No stored template, alias, font or job with that id or name for this caller, including another caller's, and a job whose result has expired | Check the id and the credential. See [scope](stored-templates.md#scope) |
| `template_not_cached` | `templateHash` isn't in this replica's memory | Send the file. See [sending a hash](pipeline.md#sending-a-hash) |

```bash
curl -F templateHash=deadbeef -F "data=<letter.json" \
  http://localhost:9027/v1/render
```

### 409

| Code | Meaning | What to do |
|---|---|---|
| `job_not_done` | The [background job](background-jobs.md)'s result was asked for before the job ended | Poll `GET /v1/jobs/{id}` until `status` is `done`, waiting as `Retry-After` says |
| `job_failed` | The job failed, so it has no result | Read `error` in `GET /v1/jobs/{id}`. A job that stopped because its container did has the code `job_lost` there, and you should send the request again |

### 413

| Code | Meaning | What to do |
|---|---|---|
| `payload_too_large` | A JSON request body is over `RUSTICDOC_BODY_LIMIT` | Send less data, or raise the limit |
| `template_too_large` | A template to store is over `RUSTICDOC_MAX_TEMPLATE_BYTES` | Make it smaller, or raise the limit |
| `reported` | A template, or an HTML page being laid out, needed more memory than one render may use, with the message `The document needs more memory than one render may use.` | Reduce the data or the page, or raise `RUSTICDOC_RENDER_MEMORY` |

### 422

| Code | Meaning | What to do |
|---|---|---|
| `template_error` | The template has a syntax error, uses an undefined value or an unknown filter, or a filter refused a value. The message names the tag and, in Office files, where it sits | Fix it. [`/v1/templates/validate`](https://rusticdoc.com/docs/templates/validate/) lists every problem in one go |
| `reported` | The render worker stopped: `The document used more CPU time than one render may use.` or `The document could not be rendered.` | Simplify the template or the page. Look in the log under the request id |

```bash
curl -F template=@letter.txt -F 'data={"customer":{}}' \
  http://localhost:9027/v1/render
```

### 429

| Code | Meaning | What to do |
|---|---|---|
| `too_many_jobs` | Too many background or webhook jobs are running, in total or for this caller | Retry later, or raise `RUSTICDOC_MAX_ASYNC_JOBS` or `RUSTICDOC_MAX_ASYNC_JOBS_PER_CALLER`. See [webhooks](webhooks.md#limits) |
| `too_many_requests` | Fonts were reloaded in the last 10 seconds | Retry in a few seconds |

### 500

| Code | Meaning | What to do |
|---|---|---|
| `engine_failed` | LibreOffice, qpdf or another engine ran and failed. This includes a conversion LibreOffice can't do, like DOCX to XLSX, and a wrong `password` | Look in the log under the request id. Check the input and the target format |
| `io_error` | The service hit a file or system error | Look in the log |
| `store_integrity` | What the store returned doesn't match its hash | Look in the log. Restore the object from a backup |

```bash
curl -F files=@report.docx -F to=xlsx http://localhost:9027/v1/convert
```

An unhandled fault also returns `500`, with no JSON body.

### 501

| Code | Meaning | What to do |
|---|---|---|
| `store_not_configured` | The service has no store, so it can't do this | Set `RUSTICDOC_STORE`. See [stored templates](stored-templates.md#turn-it-on) |
| `not_supported` | The template's format can't hold tags, like a `.bin` file sent with `data` | Use a [supported format](stored-templates.md#upload-a-template). The message reads `Not Implemented`, and the log has the format |

### 503

| Code | Meaning | What to do |
|---|---|---|
| `timeout` | The job ran past its limit | Raise `RUSTICDOC_API_TIMEOUT` (default `30s`) or `RUSTICDOC_BATCH_TIMEOUT` (default `5m`), or send less work |
| `engine_unavailable` | An engine isn't configured, or conversion is disabled with `RUSTICDOC_CONVERT_DISABLED` | Check `GET /v1/version` and the settings |
| `busy` | Request bodies in memory have reached `RUSTICDOC_MAX_INFLIGHT_BYTES`, or the service is shutting down. Comes with `Retry-After: 5` | Retry after that time |
| `store_unavailable` | The store can't be reached | Retry. Check the store. Requests that send their template as a file keep working |

`/ready` also returns `503`, with `{"status":"syncing"}`, while a new replica copies its fonts. That isn't an error body (see [running many containers](https://rusticdoc.com/docs/operate/many-containers/)).

## Responses that are not JSON

A few responses come from the HTTP layer, before a route's code runs, so they aren't in the JSON shape. Check `content-type` before you parse.

| Case | Status | Body |
|---|---|---|
| A query value is wrong, like `?limit=abc` on `/v1/templates` | `400` | Text, starting `Failed to deserialize query string` |
| An unknown path | `404` | Empty |
| A known path with the wrong method | `405` | Empty, with an `allow` header |
| A request still running 5 seconds past its time limit | `503` | Empty |

```bash
curl -s http://localhost:9027/v1/nope -D headers.txt -o error.txt || true
grep -q "^HTTP/1.1 404" headers.txt
test ! -s error.txt
```

A body that's neither multipart nor JSON gets the route's own `400`, in the JSON shape.

## Failures after a 204

With a webhook, a failure during the job goes to your URL as JSON, with the same `error` and `message`, plus `status` and `requestId`. [Webhooks](webhooks.md#what-a-delivery-looks-like) shows what that delivery looks like. Mistakes in the form still get a response straight away.
