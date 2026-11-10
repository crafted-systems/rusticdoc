---
title: Webhooks
description: Get results by HTTP callback instead of holding a request open, and verify that a delivery came from your service.
sidebar: { order: 6 }
---

Add a webhook header to a request and the service returns `204` straight away, then posts the result to your URL when it's ready. If you can't host a receiver, run the request as a [background job](/docs/guides/background-jobs/) and fetch the result instead.

```bash
curl -H "X-Rusticdoc-Webhook-Url: https://receiver.example.com/documents" \
  -H "X-Rusticdoc-Webhook-Error-Url: https://receiver.example.com/failures" \
  -F template=@invoice.docx -F "data=<invoice.json" -F steps=convert \
  http://localhost:9027/v1/render
```

## Request headers

| Header | Required | Meaning |
|---|---|---|
| `X-Rusticdoc-Webhook-Url` | Yes | Where the result goes |
| `X-Rusticdoc-Webhook-Error-Url` | No | Where failures go. Without it, they go to the result URL |
| `X-Rusticdoc-Webhook-Method` | No | `POST` (default), `PUT` or `PATCH`, for both URLs |
| `X-Rusticdoc-Webhook-Extra-Headers` | No | A JSON object of headers to add to deliveries, like `{"Authorization":"Bearer abc"}` |

Extra headers can't set `host`, `content-length`, `content-type`, `transfer-encoding`, `connection`, `x-request-id` or anything starting with `x-rusticdoc-`. If you try, you get a `400`.

### Routes that take a webhook

Webhooks work on every route that produces a file: `/v1/render`, `/v1/render/batch`, `/v1/convert` and `/v1/pdf/*`. Routes that return JSON, like `/v1/pdf/metadata/read`, `/v1/pdf/bookmarks/read` and `/v1/pdf/forms/fields`, ignore the headers and respond straight away.

## The immediate response

| Status | When |
|---|---|
| `204` | Accepted. The body is empty, and `x-request-id` identifies the job |
| `400` | The URL, method or extra headers are wrong, the form is wrong, or the URL points at a private address |
| `403` | The URL fails the allow or deny list (`url_not_allowed`) |
| `429` | Too many jobs are running (`too_many_jobs`) |

A mistake in the form is reported here, not by webhook. A failure while the job runs, like a template error, goes by webhook after the `204`.

## What a delivery looks like

A result arrives with the file as the body:

```http
POST /documents HTTP/1.1
content-type: application/pdf
content-disposition: attachment; filename="letter.pdf"
x-rusticdoc-event: render.completed
x-request-id: my-id-1
x-rusticdoc-timestamp: 1793997426
x-rusticdoc-attempt: 1
x-rusticdoc-signature: v1=7ced1242...
user-agent: rusticdoc/0.1.1
```

| Header | Meaning |
|---|---|
| `x-rusticdoc-event` | `render.completed` or `render.failed` |
| `x-request-id` | The id the `204` carried |
| `x-rusticdoc-timestamp` | Unix seconds when this attempt was sent. It changes on each attempt |
| `x-rusticdoc-attempt` | `1`, `2` or `3` |
| `x-rusticdoc-signature` | Present when `RUSTICDOC_WEBHOOK_SECRET` is set |
| `content-disposition` | The file's name. Absent on failures |

A failure arrives as JSON, at the error URL if you gave one:

```json
{
  "error": "template_error",
  "message": "template error: undefined value (in <string>:1) (line 1)",
  "status": 422,
  "requestId": "4740dce6-331f-4c2f-88f4-51d6c1c4b317"
}
```

`error` and `status` are the same as on a synchronous response (see [errors](/docs/reference/errors/)). Here too, an engine failure is reported by the name of its status, with the detail in the log under the request id.

## Retries

The service makes up to 3 attempts in total and stops at the first `2xx`. Any other response, or a connection failure, leads to the next attempt. The second comes 1 second after the first, and the third 3 seconds after the second. If the third fails too, the job is dropped, the log says `webhook gave up`, and the result isn't kept anywhere.

`RUSTICDOC_WEBHOOK_ATTEMPTS` changes the number of attempts, and each attempt has `RUSTICDOC_API_TIMEOUT` to finish. Redirects aren't followed, so a `3xx` counts as a failure.

A delivery can arrive twice if your receiver processed it but its response was lost. Use `x-request-id` to spot a repeat.

## Limits

| Setting | Default | Meaning |
|---|---|---|
| `RUSTICDOC_MAX_ASYNC_JOBS` | `32` | Total webhook and background jobs running at the same time |
| `RUSTICDOC_MAX_ASYNC_JOBS_PER_CALLER` | `8` | The same, for one caller |

Past either limit, a request gets `429 too_many_jobs` before its body is read. A job holds its slot through its delivery attempts, so a slow or failing receiver uses up slots.

A job has the same time limit as a synchronous request. Nothing is queued on disk, so a job doesn't survive a restart of the service. If you need a result, treat a missing callback as a failure and send the request again.

## Which URLs are allowed

- Only `http` and `https` URLs are accepted, with no user or password in them. Use extra headers for credentials.
- Private addresses are refused with a `400`: loopback, private ranges, link-local, and names like `localhost` or `*.internal`. `RUSTICDOC_WEBHOOK_ALLOW_PRIVATE=true` allows them.
- The address is checked when the request arrives and the delivery is pinned to it, so a DNS change can't redirect it.
- `RUSTICDOC_WEBHOOK_ALLOW_LIST` and `RUSTICDOC_WEBHOOK_DENY_LIST` (and `RUSTICDOC_WEBHOOK_ERROR_ALLOW_LIST`, `RUSTICDOC_WEBHOOK_ERROR_DENY_LIST`) are comma-separated regular expressions. A URL they refuse gets a `403`.

`RUSTICDOC_WEBHOOK_DISABLED=true` makes the service ignore the headers on every route, so the request runs synchronously.

## Signatures

Set `RUSTICDOC_WEBHOOK_SECRET` to sign deliveries. Without it they aren't signed, and the service warns at start.

The signature is `v1=` followed by the hex HMAC-SHA256 of this string, over the raw body bytes:

```text
<x-rusticdoc-timestamp>.<x-rusticdoc-event>.<x-request-id>.<body>
```

### The signing key

Each caller signs with its own key, so one tenant can't send another's receiver something that verifies. Every key of a tenant in `RUSTICDOC_TENANT_KEYS` signs with the tenant's key, so receivers don't need to change when you replace a key.

| Service | Principal | Key |
|---|---|---|
| With credentials | `key:<hex SHA-256 of the API key>`, `tenant:<name>` for a [tenant key](/docs/guides/authentication/#tenant-keys), `basic:<hex SHA-256 of the user>` or `oidc:<hex SHA-256 of the token's subject>` | `HMAC-SHA256(secret, principal)` |
| No credentials | `public` | The secret's bytes |

Don't derive the key by hand. Run the command with the service's secret set, and give the receiver the hex it prints:

```bash
# In a container with RUSTICDOC_WEBHOOK_SECRET set
rusticdoc webhook-secret --api-key "$KEY"
rusticdoc webhook-secret --tenant acme
rusticdoc webhook-secret --basic-user ada
rusticdoc webhook-secret --oidc-subject "user-123"
# With no credentials configured on the service
rusticdoc webhook-secret
```

### Verify in Node

This needs no dependencies and runs with Node 26. Read the raw body, because verifying a parsed or re-encoded body fails.

```js title="receiver.mjs"
import { createServer } from 'node:http';
import { createHmac, timingSafeEqual } from 'node:crypto';

// The hex that `rusticdoc webhook-secret` printed for this caller.
const key = Buffer.from(process.env.RUSTICDOC_SIGNING_KEY, 'hex');
const MAX_AGE_SECONDS = 300;

function verified(headers, body) {
  const timestamp = headers['x-rusticdoc-timestamp'];
  const event = headers['x-rusticdoc-event'];
  const requestId = headers['x-request-id'];
  const signature = headers['x-rusticdoc-signature'];
  if (!timestamp || !event || !requestId || !signature?.startsWith('v1=')) {
    return false;
  }

  const age = Math.abs(Date.now() / 1000 - Number(timestamp));
  if (!(age <= MAX_AGE_SECONDS)) return false;

  const expected = createHmac('sha256', key)
    .update(`${timestamp}.${event}.${requestId}.`)
    .update(body)
    .digest('hex');
  const a = Buffer.from(signature.slice(3));
  const b = Buffer.from(expected);
  return a.length === b.length && timingSafeEqual(a, b);
}

createServer((req, res) => {
  const chunks = [];
  req.on('data', (chunk) => chunks.push(chunk));
  req.on('end', () => {
    const body = Buffer.concat(chunks);
    const ok = verified(req.headers, body);
    console.log(
      ok ? 'verified' : 'REJECTED',
      req.headers['x-rusticdoc-event'],
      req.headers['x-request-id'],
      body.length,
    );
    res.statusCode = ok ? 204 : 401;
    res.end();
  });
}).listen(Number(process.env.PORT ?? 8000));
```

### Verify in Python

This uses only the standard library and runs with Python 3.13.

```python title="receiver.py"
import hashlib
import hmac
import os
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

# The hex that `rusticdoc webhook-secret` printed for this caller.
KEY = bytes.fromhex(os.environ["RUSTICDOC_SIGNING_KEY"])
MAX_AGE_SECONDS = 300


def verified(headers, body):
    timestamp = headers.get("X-Rusticdoc-Timestamp", "")
    event = headers.get("X-Rusticdoc-Event", "")
    request_id = headers.get("X-Request-Id", "")
    signature = headers.get("X-Rusticdoc-Signature", "")
    if not (timestamp.isdigit() and event and request_id
            and signature.startswith("v1=")):
        return False

    if abs(time.time() - int(timestamp)) > MAX_AGE_SECONDS:
        return False

    message = f"{timestamp}.{event}.{request_id}.".encode() + body
    expected = hmac.new(KEY, message, hashlib.sha256).hexdigest()
    return hmac.compare_digest(signature[3:].encode(), expected.encode())


class Receiver(BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        ok = verified(self.headers, body)
        print("verified" if ok else "REJECTED",
              self.headers.get("X-Rusticdoc-Event"),
              self.headers.get("X-Request-Id"), len(body), flush=True)
        self.send_response(204 if ok else 401)
        self.end_headers()


port = int(os.environ.get("PORT", 8000))
ThreadingHTTPServer(("0.0.0.0", port), Receiver).serve_forever()
```

### Try them

Both receivers are tested against real deliveries, signed for an API-key caller, a basic-auth user and a service with no credentials, for both `render.completed` and `render.failed`. They accept those, and they reject a changed body, a changed event or request id, a stale timestamp and another caller's key. To try it yourself:

```bash
docker run -d --name rd-webhooks -p 127.0.0.1:9028:9027 \
  -e RUSTICDOC_API_KEYS=my-key \
  -e RUSTICDOC_WEBHOOK_SECRET=change-me \
  -e RUSTICDOC_WEBHOOK_ALLOW_PRIVATE=true \
  ghcr.io/crafted-systems/rusticdoc:0.1.1

export RUSTICDOC_SIGNING_KEY=$(docker exec rd-webhooks \
  rusticdoc webhook-secret --api-key my-key)
PORT=8000 node receiver.mjs &

curl -H "X-Api-Key: my-key" \
  -H "X-Rusticdoc-Webhook-Url: http://host.docker.internal:8000/documents" \
  -F template=@letter.txt -F "data=<letter.json" -F steps=convert \
  http://localhost:9028/v1/render
```

`host.docker.internal` reaches the host from Docker Desktop. On Linux, add `--add-host=host.docker.internal:host-gateway` to `docker run`.
