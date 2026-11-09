---
title: Sizing
description: Work out how many workers and how much memory a rusticdoc container needs.
sidebar:
  order: 4
---

These are the settings that decide capacity, and the arithmetic behind the memory limit. There are no throughput figures because no load test has been run, so measure with your own documents.

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

The ceilings add up to more than 6 GiB, so the limit doesn't cover the worst case, and how close real traffic comes to it hasn't been measured. An address space limit counts memory a process has reserved as well as memory it uses, so a worker's resident memory can be lower than its limit. If memory does run out, the kernel kills render workers first because they run with the highest `oom_score_adj`. Their requests fail and the service carries on.

For comparison, one container at rest after its first conversion showed 150 to 270 MiB in `docker stats`, on an arm64 build. That tells you little about load.

```bash
docker stats --no-stream rusticdoc
```

## Running with less

Three pods sharing about 6 GB were killed for memory, and their conversions timed out. So don't read the 2 GiB per pod that the Kubernetes manifest requests as enough to run on. That's what the scheduler reserves. Each pod can grow to its 6 GiB limit, and a node that can't supply that will kill pods under load.

No setup smaller than this has been measured as safe. If you have to run smaller, lower the ceilings together so that the limit still holds what you allow:

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
