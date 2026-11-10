# SDKs

Client libraries are generated from the service's own OpenAPI description, so they never drift from the API.

| Step | Command |
|---|---|
| Refresh the description | `make openapi` (writes `openapi.json`; CI fails if it is stale) |
| TypeScript, `fetch`-based | `make sdk-typescript` → `sdk/typescript` |
| Python | `make sdk-python` → `sdk/python` |
| Go | `make sdk-go` → `sdk/go` |
| All three | `make sdk` |

Generation runs `openapi-generator` from its Docker image, so nothing needs installing. The generated folders are not committed, and no package is published yet.

All three clients compile and pass five calls against a local service with a store: version, template upload, render from the stored template, metadata write and metadata read. The generated `render` sends a JSON body, so it takes a stored template's `templateId` and not a file. In Go the form field `ctx` is `TemplateCtx`, because `ctx` is the request context.

## Plain HTTP

Every endpoint is one multipart request, so a plain HTTP client works without an SDK. For example, in TypeScript:

```ts
const form = new FormData();
form.append("template", new Blob([templateBytes]), "invoice.docx");
form.append("data", JSON.stringify({ customer: { name: "Ada" } }));
form.append("steps", "convert,pdfa");
form.append("pdfa", "PDF/A-2b");

const res = await fetch("http://localhost:9027/v1/render", {
  method: "POST",
  headers: { "X-Api-Key": process.env.RUSTICDOC_API_KEY! },
  body: form,
});
const pdf = await res.arrayBuffer();
```

## Webhooks

Add `X-Rusticdoc-Webhook-Url` (and optionally `X-Rusticdoc-Webhook-Error-Url`) to any request to get `204` at once and the result delivered by HTTP. When `RUSTICDOC_WEBHOOK_SECRET` is set, each delivery and event carries `X-Rusticdoc-Timestamp`, `X-Rusticdoc-Event` and `X-Rusticdoc-Signature: v1=<hex HMAC-SHA256 of "<timestamp>.<event>.<request id>.<body>">`, with the request id from `X-Request-Id`. Each caller signs with its own key, `HMAC-SHA256(secret, principal)`; with no credentials configured, the secret itself. `rusticdoc webhook-secret --api-key <key>` prints a caller's key in hex, and a receiver verifies with the decoded bytes. Verify the signature with a constant-time comparison and reject timestamps older than your freshness window, which stops replays. Deliveries retry three times with backoff. The SDKs will include a signature verifier.
