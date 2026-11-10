---
title: Sizing
description: Work out how many workers and how much memory a rusticdoc container needs.
sidebar:
  order: 4
---

These are the settings that decide capacity, the arithmetic behind the memory limit, and the figures from one load check. The check used one small document on one machine, so measure with your own documents.

## Workers

| Setting | Default | What it controls |
|---|---|---|
| `RUSTICDOC_WORKERS` | `2` | LibreOffice conversions running at the same time. Each worker is a long-lived process |
| `RUSTICDOC_LIBREOFFICE_MAX_JOBS` | `10` | Jobs a LibreOffice worker does before it restarts |
| `RUSTICDOC_RENDER_WORKERS` | `4` | Templates being evaluated at the same time. Each worker is a short-lived process |
| `RUSTICDOC_MAX_ASYNC_JOBS` | `32` | Webhook jobs running at the same time |
| `RUSTICDOC_MAX_ASYNC_JOBS_PER_CALLER` | `8` | The same, for one caller |

A request waits for a free worker, and if it's still waiting at `RUSTICDOC_API_TIMEOUT` it fails with `503`. So if conversions keep timing out under load and the pool is always busy, you need more LibreOffice workers, more replicas or a longer timeout.

`GET /health` shows the pool:

```bash test
curl http://localhost:9027/health
```

Under `details.libreoffice.pool`, `busy` and `idle` show the workers right now, and `restarts` counts worker restarts. For the number of jobs in progress, look at `rusticdoc_inflight{stage="pipeline"}` in [`/metrics`](/docs/operate/observability/).

## Measured under load

One load check has been run, on 10 November 2026. It posted a one-page invoice to `/v1/render` with ApacheBench, at 1, 4, 8 and 16 connections, each for 15 to 30 seconds. The container had the Compose flags and limits, two LibreOffice workers that restart after 10 jobs, four render workers and a 60 second timeout.

| Template and steps | Connections | Requests/s | p50, ms | p95, ms | p99, ms | Peak memory |
|---|---|---|---|---|---|---|
| Text, no steps | 1 | 509 | 2 | 3 | 5 | 86 MiB |
| Text, no steps | 4 | 1,281 | 3 | 5 | 8 | 91 MiB |
| Text, no steps | 8 | 1,694 | 5 | 6 | 8 | 97 MiB |
| Text, no steps | 16 | 1,592 | 10 | 12 | 19 | 97 MiB |
| DOCX, no steps | 1 | 149 | 6 | 8 | 14 | 95 MiB |
| DOCX, no steps | 4 | 541 | 7 | 10 | 13 | 106 MiB |
| DOCX, no steps | 8 | 630 | 12 | 17 | 33 | 110 MiB |
| DOCX, no steps | 16 | 649 | 24 | 30 | 41 | 112 MiB |
| DOCX, `convert` | 1 | 16.1 | 50 | 155 | 170 | 167 MiB |
| DOCX, `convert` | 4 | 27.9 | 131 | 276 | 341 | 226 MiB |
| DOCX, `convert` | 8 | 27.5 | 202 | 501 | 627 | 237 MiB |
| DOCX, `convert` | 16 | 27.2 | 604 | 722 | 940 | 241 MiB |
| DOCX, `convert,pdfa` | 1 | 7.8 | 107 | 215 | 253 | 236 MiB |
| DOCX, `convert,pdfa` | 4 | 13.3 | 251 | 496 | 588 | 243 MiB |
| DOCX, `convert,pdfa` | 8 | 12.7 | 605 | 850 | 1,590 | 246 MiB |
| DOCX, `convert,pdfa` | 16 | 14.2 | 1,154 | 1,284 | 1,492 | 249 MiB |
| DOCX, `pdfa` | 1 | 15.4 | 52 | 159 | 179 | 216 MiB |
| DOCX, `pdfa` | 4 | 27.4 | 130 | 293 | 389 | 244 MiB |
| DOCX, `pdfa` | 8 | 28.0 | 194 | 494 | 661 | 245 MiB |
| DOCX, `pdfa` | 16 | 24.1 | 586 | 1,388 | 2,604 | 242 MiB |

All 136,330 requests got a `200`. No `503` was seen, but 16 connections doesn't reach the limits, so the check doesn't show where they start.

What the figures say:

- **Conversion stops gaining at 4 connections.** Two LibreOffice workers convert about 28 of these invoices a second. More connections only wait longer. One more run with `RUSTICDOC_WORKERS=4` gave about 49 a second from 4 connections up, and peaked at 313 MiB.
- **Worker restarts are part of that cost.** A worker restarts after 10 jobs, which is why the 95th percentile at one connection is three times the median. Two more runs with `RUSTICDOC_LIBREOFFICE_MAX_JOBS=1000`, so no worker restarted, gave 38 to 46 a second with the same two workers.
- **`convert,pdfa` is half the speed of `pdfa`.** It runs LibreOffice twice. `steps=pdfa` alone gives PDF/A at the rate of a plain `convert`.
- **Filling without LibreOffice levels off between 4 and 8 connections**, with four render workers. The check didn't try other values.

Read the figures with these limits in mind:

- It was one Mac, an Apple M4 Pro, with the container inside Docker Desktop's VM of 8 CPUs and 5.78 GiB.
- ApacheBench ran on the same machine and took CPU from the service. The Mac was in use for other work too.
- It was one small document: a 37 KB DOCX, 556 bytes of data and a 55 KB PDF.
- Three full runs that day differed by up to 30% on the same row.

So use the numbers to compare one kind of request with another. They aren't a promise of capacity. The check is in `test/load/`, and `make load` runs it against your own image in about eight minutes.

## Render memory

`RUSTICDOC_RENDER_MEMORY` is the address space one render worker gets, 1 GiB by default. A template that goes past it gets a `413`, and the service keeps running.

`data` counts against this limit once it's parsed. At the default, a list of 8 million small numbers rendered and a list of 16 million got a `413`, so raise the limit if your data is that size.

The limit relies on Linux, and isn't confirmed to hold on macOS.

## The memory arithmetic

The two resident memory figures in this table were measured. The rest are worked out from the default settings:

| Part | Ceiling | Where it comes from |
|---|---|---|
| Render workers | 4 GiB of address space | `RUSTICDOC_RENDER_WORKERS` × `RUSTICDOC_RENDER_MEMORY`, 4 × 1 GiB |
| Request bodies in memory | 2 GiB | `RUSTICDOC_MAX_INFLIGHT_BYTES` |
| LibreOffice | No limit set by the service | Depends on your documents. In one measurement, two idle workers held about 115 MB of resident memory |
| The service | No limit set by the service | About 10 MB of resident memory at rest, in the same measurement |
| Temporary files, under Docker | 2.5 GiB with the Compose sizes | `/tmp` and the home directory are on tmpfs, which counts against the container's memory. On Kubernetes the manifest uses `emptyDir` on disk, so they don't count |

`deploy/compose.yaml` and the Kubernetes manifest both set a limit of 6 GiB, which covers the render workers at their limit and leaves 2 GiB for everything else.

The ceilings add up to more than 6 GiB, so the limit doesn't cover the worst case. The load check above peaked at 249 MiB, but with one small document. Large documents and large data come closer to the limit. An address space limit counts memory a process has reserved as well as memory it uses, so a worker's resident memory can be lower than its limit. If memory does run out, the kernel kills render workers first because they run with the highest `oom_score_adj`. Their requests fail and the service carries on.

For comparison, one container at rest after its first conversion showed 150 to 270 MiB in `docker stats`, on an arm64 build. That tells you little about load.

```bash
docker stats --no-stream rusticdoc
```

## Running with less

Three pods sharing about 6 GB were killed for memory, and their conversions timed out. So don't read the 2 GiB per pod that the Kubernetes manifest requests as enough to run on. That's what the scheduler reserves. Each pod can grow to its 6 GiB limit, and a node that can't supply that will kill pods under load.

If you have to run smaller, lower the ceilings together so that the limit still holds what you allow:

| Setting | Effect |
|---|---|
| `RUSTICDOC_RENDER_WORKERS=2` | Half the render memory, and half as many templates at a time |
| `RUSTICDOC_RENDER_MEMORY=512MB` | Half the memory per template, so large data gets a `413` sooner |
| `RUSTICDOC_WORKERS=1` | One conversion at a time |
| `RUSTICDOC_MAX_INFLIGHT_BYTES=512MB` | Less room for request bodies, so uploads wait sooner |

Write sizes as `512MB` or `1GB`. For these settings, a value the service can't parse, like `2g` or `1.5GB`, is ignored without a warning and the default stays in place. See the [configuration reference](/docs/reference/configuration/).

Then test with your largest documents and your busiest hour before you rely on it.

## Disk and temporary space

Conversions use `/tmp`, and each one can write up to `RUSTICDOC_LIBREOFFICE_MAX_DISK`, 512 MB by default, before it's stopped. Allow `RUSTICDOC_WORKERS` times that, plus the files being converted. The Compose default of 2 GB is enough for two workers.

Every replica with a store also keeps a copy of all stored fonts in `RUSTICDOC_FONT_CACHE_DIR`, and `RUSTICDOC_MAX_FONT_STORAGE`, 1 GiB by default, bounds that copy.
