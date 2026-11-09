---
title: Concepts
description: How a request moves through the template, data, steps and engines, and what the service keeps.
sidebar:
  order: 3
---

Every request goes through the same few stages. Once you know them, you can predict what the service will do with yours and where to look when something goes wrong.

## The flow of a request

Every job is one multipart `POST`. A request to `/v1/render` goes through these stages:

1. The service checks your credentials and holds the request until there's room for its body in memory.
2. It reads the form parts: the template, the data, the steps and their options.
3. A render worker fills the template with the data. This is the template stage.
4. Each step in `steps` runs in the order you listed, on the output of the one before.
5. The result comes back in the response, or goes to your webhook.

```bash test
curl -F template=@letter.txt -F "data=<letter.json" \
  -F steps=convert,pdfa -F pdfa=PDF/A-2b \
  http://localhost:9027/v1/render -o letter.pdf -D -
```

The response headers tell you what happened:

| Header | What it tells you |
|---|---|
| `x-request-id` | The id of this request. The same id appears in the logs |
| `server-timing` | How long each stage took, such as `template;dur=4, convert;dur=222, pdfa;dur=228` |
| `x-fonts-used` | The fonts the converted document used |

## Template

The template is either a file you send as the `template` part or the `templateId` of one you've stored. It can be DOCX, ODT, XLSX, ODS, PPTX, ODP, a PDF with form fields, or a text format such as `.txt`, `.md`, `.html`, `.csv`, `.xml`, `.json` or `.svg`.

[Template syntax](/docs/templates/syntax/) covers the tags, and [Office templates](/docs/templates/office/) covers how they behave inside a document.

## Data

`data` is a form field that holds JSON. A few rules are worth knowing:

- Invalid JSON returns `400`.
- A tag with no value in the data returns `422`, and the error tells you where the tag is.
- If you send no `data`, or `{}`, the service skips the template stage for an Office file and hands it to the steps untouched. You get `200`, with the tags still in the output.
- A text template always goes through the template stage, so the same mistake returns `422`.

```bash test=fail
curl -F template=@letter.txt -F 'data={}' http://localhost:9027/v1/render
```

The usual way to send no data by accident is a shell quoting slip, which the [quickstart](/docs/start/quickstart/) covers.

## Steps

`steps` is a comma-separated list. You choose the order, and each step works on the output of the one before. The steps are `convert`, `pdfa`, `pdfua`, `merge`, `split`, `watermark`, `stamp`, `rotate`, `flatten`, `optimize`, `bookmarks`, `metadata`, `embed`, `factur-x` and `encrypt`.

With no steps, you get the filled template back in its own format. [The pipeline](/docs/guides/pipeline/) covers each step and its options.

## Engines

The service looks for its engines when it starts. If one is missing, the steps that need it return `503` and everything else keeps working.

| Engine | What it does | How it runs |
|---|---|---|
| LibreOffice | Conversion, through a small Python bridge | A pool of warm processes, `RUSTICDOC_WORKERS` of them, each restarted after `RUSTICDOC_LIBREOFFICE_MAX_JOBS` jobs |
| `lopdf` | Most PDF operations | Inside the service |
| qpdf | PDF operations, and encryption | A child process per operation |
| pdfcpu | PDF operations | A child process per operation |

Child engines start from an almost empty environment, so your keys and secrets don't reach them. PDF operations try `lopdf` first, then qpdf, then pdfcpu. You can change that order, as the [configuration reference](/docs/reference/configuration/#pdf-engine-order) explains.

Documents can't reach the network through LibreOffice. [Security](/docs/operate/security/) explains how that's enforced and how it's checked.

## Worker process

A template is code that comes from your client. It can build a value it never prints, and if it ran inside the service that could use up all the service's memory. So under `serve`, every template is evaluated in a child process, `rusticdoc __render-worker`, which has:

- No environment.
- An address space limit, `RUSTICDOC_RENDER_MEMORY`, which is 1 GiB by default. It relies on Linux.
- Limits on CPU time and open files, and no core dumps.
- The highest `oom_score_adj`, so the kernel kills it first when memory runs short.

At most `RUSTICDOC_RENDER_WORKERS` of these run at once, four by default. A template that goes over its limit gets `413`, and the service keeps running.

The CLI doesn't do any of this. It renders in its own process, because there the template is yours.

## Stateless, or with a store

By default the service stores nothing, and each request carries its own template and data. The stored template and font routes return `501`. There's no default store on the container's own disk, because it would work on one replica and return `404` on the next.

Set `RUSTICDOC_STORE` to a bucket or a shared volume and the service can hold templates and fonts for you. That store is the only thing replicas share. [Many containers](/docs/operate/many-containers/) explains how it behaves.

One thing does outlast a request without a store. An in-memory cache lets the same caller send `templateHash`, the hex SHA-256 of a template it sent recently, in place of the file. A restart clears the cache, and other replicas don't have it.

## Synchronous, or by webhook

By default the response holds the result, and the request has until `RUSTICDOC_API_TIMEOUT` to finish, which is 30 seconds unless you change it. After that it returns `503`.

Add `X-Rusticdoc-Webhook-Url` and the service returns `204` straight away, then posts the result to that URL. See [webhooks](/docs/guides/webhooks/).
