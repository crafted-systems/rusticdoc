---
title: SDKs
description: The state of rusticdoc's generated client libraries, and how to call it with plain HTTP today.
sidebar:
  order: 4
---

This page tells you what state the SDKs are in and shows you how to call the service without one. For now, use plain HTTP.

## The state of the SDKs

Client libraries are generated from the service's own OpenAPI description. None is published yet.

| Language | Generated with | State |
|---|---|---|
| TypeScript, `fetch`-based | `make sdk-typescript` | Does not compile. `tsc` reports `Cannot find name 'anyToJSON'` wherever a form field has an `any` type, such as `data` |
| Go | `make sdk-go` | Does not compile. The form field `ctx` collides with Go's request context |
| Python | `make sdk-python` | Generates and byte-compiles. It has not been run against the service |

`make sdk` generates all three, and `make openapi` refreshes `openapi.json` first. Generation runs `openapi-generator` from its Docker image, so nothing needs installing. The generated folders are not committed.

## Use plain HTTP

Every endpoint is one multipart request, so any HTTP client works. In TypeScript, with Node's built-in `fetch`:

```ts
import { readFile } from "node:fs/promises";

const template = await readFile("invoice.docx");
const data = await readFile("invoice.json", "utf8");

const form = new FormData();
form.append("template", new Blob([template]), "invoice.docx");
form.append("data", data);
form.append("steps", "convert,pdfa");
form.append("pdfa", "PDF/A-2b");

const res = await fetch("http://localhost:9027/v1/render", {
  method: "POST",
  headers: { "X-Api-Key": process.env.API_KEY ?? "" },
  body: form,
});
if (!res.ok) {
  throw new Error(`${res.status}: ${await res.text()}`);
}
const pdf = new Uint8Array(await res.arrayBuffer());
```

Send `data` as a string, not as a `Blob` or a file. A file part is not read as data unless its filename is `data.json`, as the [quickstart](/docs/start/quickstart/) explains.

The same request with `curl`:

```bash test
curl -H "X-Api-Key: $KEY" -F template=@invoice.docx -F "data=<invoice.json" \
  -F steps=convert,pdfa -F pdfa=PDF/A-2b \
  http://localhost:9027/v1/render -o invoice.pdf
```

The [HTTP API reference](/docs/reference/api/) has every route, and the description itself is at `/openapi.json` on a running service:

```bash test
curl http://localhost:9027/openapi.json -o openapi.json
```

Check `x-request-id` on any response when you need to find a request in the logs. [Webhooks](/docs/guides/webhooks/) shows how to verify a delivery's signature, which needs only an HMAC.
