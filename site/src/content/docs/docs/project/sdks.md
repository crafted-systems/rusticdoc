---
title: SDKs
description: The state of rusticdoc's generated client libraries, and how to call it with plain HTTP today.
sidebar:
  order: 4
---

No SDK is published yet. You can generate one from a checkout of the repository, or use plain HTTP.

## The state of the SDKs

The client libraries are generated from the service's own OpenAPI description.

| Language | Generated with | State |
|---|---|---|
| TypeScript, `fetch`-based | `make sdk-typescript` | Compiles and passes the test below |
| Go | `make sdk-go` | Compiles and passes it. The form field `ctx` is `TemplateCtx` here, because Go keeps `ctx` for the request context |
| Python | `make sdk-python` | Passes it |

The test is five calls against a local service with a [store](/docs/guides/stored-templates/): read the version, upload a template, render it, write a PDF's metadata and read it back. That's a small part of the API, so treat a generated client as a starting point.

The generated `render` sends a JSON body, so it renders a stored template by its `templateId`. To send the template file in the same request, use plain HTTP as shown below.

The 0.1.0 description gave TypeScript and Go clients that didn't compile. The fixes came after 0.1.0, so generate from the current repository.

`make sdk` generates all three, and `make openapi` refreshes `openapi.json` first. Generation runs `openapi-generator` from its Docker image, so you don't need to install anything. The generated folders aren't committed.

## Use plain HTTP

Every endpoint takes one multipart request, so any HTTP client works. Here it is in TypeScript, with Node's built-in `fetch`:

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

A string is the simplest way to send `data`. A `Blob` or a file works too, as long as it goes in the `data` field. In any other field, a file is only read as data when its filename is `data.json`. The [quickstart](/docs/start/quickstart/) shows both ways.

The same request with `curl`:

```bash test
curl -H "X-Api-Key: $KEY" -F template=@invoice.docx -F "data=<invoice.json" \
  -F steps=convert,pdfa -F pdfa=PDF/A-2b \
  http://localhost:9027/v1/render -o invoice.pdf
```

Every route is in the [HTTP API reference](/docs/reference/api/), and a running service serves the description itself at `/openapi.json`:

```bash test
curl http://localhost:9027/openapi.json -o openapi.json
```

When you need to find a request in the logs, check `x-request-id` on its response. Verifying a webhook delivery's signature only needs an HMAC, and [Webhooks](/docs/guides/webhooks/) shows how.
