---
title: Concepts
description: How a request moves through the template, data, steps and engines, and what the service keeps.
sidebar:
  order: 3
---

This page gives you the model behind every request, so that you can predict what the service does with yours and where to look when it goes wrong.

## The flow of a request

Every job is one multipart `POST`. A request to `/v1/render` goes through these stages:

1. **Entry.** The service checks credentials and holds the request until there is room for its body in memory.
2. **Form.** It reads the form parts: the template, the data, the steps and the options for them.
3. **Template stage.** A render worker fills the template with the data.
4. **Steps.** Each step in `steps` runs in the order you list, each on the output of the one before.
5. **Answer.** The result comes back in the response, or goes to your webhook.

```bash test
curl -F template=@letter.txt -F "data=<letter.json" \
  -F steps=convert,pdfa -F pdfa=PDF/A-2b \
  http://localhost:9027/v1/render -o letter.pdf -D -
```

The response headers tell you what happened:

| Header | Says |
|---|---|
| `x-request-id` | The id of this request. The same id is in the logs |
| `server-timing` | How long each stage took, such as `template;dur=4, convert;dur=222, pdfa;dur=228` |
| `x-fonts-used` | The fonts the converted document used |

## Template

The template is a file you send as the `template` part, or the `templateId` of one you stored. It can be DOCX, ODT, XLSX, ODS, PPTX, ODP, a PDF with form fields, or a text format such as `.txt`, `.md`, `.html`, `.csv`, `.xml`, `.json` or `.svg`.

See [template syntax](/docs/templates/syntax/) for the tags and [Office templates](/docs/templates/office/) for how they behave in a document.

## Data

`data` is a form field that holds JSON. These rules matter:

- Bad JSON answers `400`.
- A tag whose value is missing answers `422` and says where it is.
- With no `data`, or with `{}`, the service skips the template stage for an Office file and passes it to the steps as it came. It answers `200`, with the tags still in the output.
- A text template always goes through the template stage, so the same mistake answers `422`.

```bash test=fail
curl -F template=@letter.txt -F 'data={}' http://localhost:9027/v1/render
```

The [quickstart](/docs/start/quickstart/) shows the most common way to send no data by accident.

## Steps

`steps` is a comma-separated list. The order is yours, and each step works on the output of the last one. The steps are `convert`, `pdfa`, `pdfua`, `merge`, `split`, `watermark`, `stamp`, `rotate`, `flatten`, `optimize`, `bookmarks`, `metadata`, `embed`, `factur-x` and `encrypt`.

With no steps, you get the filled template in its own format. [The pipeline](/docs/guides/pipeline/) covers each step and its options.

## Engines

The service finds its engines when it starts. If one is missing, the steps that need it answer `503` and the service keeps running.

| Engine | Does | Runs as |
|---|---|---|
| LibreOffice | Conversion, through a small Python bridge | A pool of warm processes, `RUSTICDOC_WORKERS` of them, each restarted after `RUSTICDOC_LIBREOFFICE_MAX_JOBS` jobs |
| `lopdf` | Most PDF operations | Inside the service |
| qpdf | PDF operations, and encryption | A child process per operation |
| pdfcpu | PDF operations | A child process per operation |

Child engines start from an almost empty environment, so your keys and secrets do not reach them. On `/v1/*`, PDF operations try `lopdf`, then qpdf, then pdfcpu. You can change the order, as the [configuration reference](/docs/reference/configuration/) explains.

Documents cannot reach the network through LibreOffice. [Security](/docs/operate/security/) says how, and how that is checked.

## Worker process

A template is code from your client. It can build a value it never prints, and in one process that could exhaust the service's memory. So under `serve`, every template is evaluated in a child process, `rusticdoc __render-worker`, which has:

- No environment.
- An address space limit, `RUSTICDOC_RENDER_MEMORY`, 1 GiB by default. It relies on Linux.
- Limits on CPU time and open files, and no core dumps.
- The highest `oom_score_adj`, so the kernel kills it first under memory pressure.

At most `RUSTICDOC_RENDER_WORKERS` of these run at once, four by default. A template that passes its limit gets `413`, and the service keeps running.

The CLI does not do this. It renders in its own process, because there the template is yours.

## Stateless, or with a store

By default the service stores nothing. Each request carries its template and its data. The stored template and font routes answer `501`, because a default on a container's own disk would work on one replica and answer `404` on the next.

Set `RUSTICDOC_STORE` to a bucket or a shared volume, and the service can hold templates and fonts for you. Replicas share only that store. [Many containers](/docs/operate/many-containers/) explains how it behaves.

One thing you can use lasts beyond a request without a store. An in-memory cache lets the same caller send `templateHash`, the hex SHA-256 of a template it recently sent, in place of the file. A restart clears it, and another replica does not have it.

## Synchronous, or by webhook

By default the response holds the result, and the request has until `RUSTICDOC_API_TIMEOUT` to finish, 30 seconds unless you change it. Past that it answers `503`.

Add `X-Rusticdoc-Webhook-Url` and the service answers `204` at once and posts the result to that URL. See [webhooks](/docs/guides/webhooks/).
