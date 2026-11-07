---
title: Sizing
description: Work out how many workers and how much memory a rusticdoc container needs.
sidebar:
  order: 4
---

This page gives you the settings that decide capacity and the arithmetic behind the memory limit. It cannot give you throughput figures, because no load test has been run. Measure with your own documents.

## Workers

| Setting | Default | Controls |
|---|---|---|
| `RUSTICDOC_WORKERS` | `2` | LibreOffice conversions at once. Each is a long-lived process |
| `RUSTICDOC_LIBREOFFICE_MAX_JOBS` | `10` | Jobs a LibreOffice worker does before it restarts |
| `RUSTICDOC_RENDER_WORKERS` | `4` | Templates being evaluated at once. Each is a short-lived process |
| `RUSTICDOC_MAX_ASYNC_JOBS` | `32` | Webhook jobs running at once |
| `RUSTICDOC_MAX_ASYNC_JOBS_PER_CALLER` | `8` | The same, for one caller |

A request waits for a free worker. If it waits past `RUSTICDOC_API_TIMEOUT`, it answers `503`. So conversions that keep timing out under load, with a pool that is always busy, mean you need more LibreOffice workers, more replicas, or a longer timeout.

`GET /health` shows the pool:

```bash test
curl http://localhost:9027/health
```

Under `details.libreoffice.pool`, `busy` and `idle` show the workers at this moment, and `restarts` counts worker restarts. `rusticdoc_inflight{stage="pipeline"}` in [`/metrics`](/docs/operate/observability/) shows how many jobs are in progress.

## Render memory

`RUSTICDOC_RENDER_MEMORY` is the address space of one render worker, 1 GiB by default. A template that passes it gets `413`, and the service keeps running.

`data` counts against this limit once parsed. At the default, a list of 8 million small numbers rendered, and one of 16 million got `413`. Raise the limit for data that size.

The limit relies on Linux. It has not been shown to hold on macOS.

## The memory arithmetic

These are the ceilings at the default settings, and the reasons behind each:

| Part | Ceiling | From |
|---|---|---|
| Render workers | 4 GiB of address space | `RUSTICDOC_RENDER_WORKERS` × `RUSTICDOC_RENDER_MEMORY`, 4 × 1 GiB |
| Request bodies in memory | 2 GiB | `RUSTICDOC_MAX_INFLIGHT_BYTES` |
| LibreOffice | No limit set by the service | Depends on your documents. Two idle workers held about 115 MB of resident memory in one measurement |
| The service | No limit set by the service | About 10 MB of resident memory at rest in the same measurement |
| Temporary files, under Docker | 2.5 GiB with the Compose sizes | `/tmp` and the home directory on tmpfs count against the container's memory. On Kubernetes the manifest uses `emptyDir` on disk, so they do not |

`deploy/compose.yaml` sets 6 GiB, and the Kubernetes manifest limits a pod to 6 GiB. That covers the render workers at their limit and leaves 2 GiB for the rest.

Add the ceilings and you get more than 6 GiB, so the limit does not cover the worst case. How close real traffic comes has not been measured. An address space limit counts memory a process has reserved as well as memory it uses, so a worker's resident memory can be lower than its limit. When memory runs out, the kernel takes render workers first, because they run with the highest `oom_score_adj`. Their requests fail, and the service carries on.

For comparison, one container at rest after a first conversion showed 150 to 270 MiB in `docker stats`, on an arm64 build. This says little about load.

```bash
docker stats --no-stream rusticdoc
```

## What happened at less

Three pods sharing about 6 GB were killed for memory, and their conversions timed out. So do not read the 2 GiB per pod that the Kubernetes manifest requests as enough to run on. It is the amount the scheduler reserves. Each pod may grow to its 6 GiB limit, and a node that cannot supply that will kill pods under load.

No smaller setup than this has been measured as safe. If you have to run smaller, lower the ceilings together, so that the limit still holds what you allow:

| Setting | Effect |
|---|---|
| `RUSTICDOC_RENDER_WORKERS=2` | Half the render memory, and half the templates at once |
| `RUSTICDOC_RENDER_MEMORY=512MB` | Half the memory per template, and large data gets `413` sooner |
| `RUSTICDOC_WORKERS=1` | One conversion at a time |
| `RUSTICDOC_MAX_INFLIGHT_BYTES=512MB` | Less room for request bodies, so uploads wait sooner |

Write sizes as `512MB` or `1GB`. A value the service cannot parse, such as `2g` or `1.5GB`, is ignored for these settings and the default stays in place, with no warning. See the [configuration reference](/docs/reference/configuration/).

Then test with your largest documents and your busiest hour before you rely on it.

## Disk and temporary space

Conversions use `/tmp`, and each one may write up to `RUSTICDOC_LIBREOFFICE_MAX_DISK`, 512 MB by default, before it is stopped. Allow `RUSTICDOC_WORKERS` times that, plus the files being converted. The Compose default of 2 GB does, with two workers. Every replica with a store keeps a copy of all stored fonts in `RUSTICDOC_FONT_CACHE_DIR`. `RUSTICDOC_MAX_FONT_STORAGE`, 1 GiB by default, bounds that copy.
