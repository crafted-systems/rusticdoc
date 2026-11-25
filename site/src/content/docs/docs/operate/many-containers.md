---
title: Many containers
description: Run several rusticdoc replicas on one store, and see what happens when the store fails.
sidebar:
  order: 2
---

Replicas that get their templates in the request share nothing, so they don't need a store. You need one for stored templates, aliases, uploaded fonts and background jobs.

## Configure a store

Stored templates and uploaded fonts have to live somewhere every container can reach, and you have to say where. Until you do, those routes return `501` and the service stores nothing.

```bash
# AWS S3, or a store compatible with it
RUSTICDOC_STORE=s3://my-bucket/rusticdoc
# a volume every replica mounts
RUSTICDOC_STORE=file:///data/rusticdoc
```

Once it's set, a template behaves the same whichever replica handles the request:

```bash test
id=$(curl -F file=@invoice.docx http://localhost:9027/v1/templates |
  sed -E 's/.*"id":"([^"]+)".*/\1/')
curl -F templateId="$id" -F "data=<invoice.json" -F steps=convert \
  http://localhost:9027/v1/render -o invoice.pdf
curl -X DELETE "http://localhost:9027/v1/templates/$id"
```

See [Stored templates](/docs/guides/stored-templates/) for these routes.

## S3 or a volume

| | S3 bucket | Shared volume |
|---|---|---|
| Setting | `s3://bucket/prefix` | `file:///path` |
| Works on Kubernetes | Yes, and it's the one to use | Only with `ReadWriteMany`, which many clusters don't offer |
| Credentials | `AWS_*` variables, or the pod's role. Never in the URL | The file system's permissions |
| What every replica needs | The same URL and credentials | The same volume. If they don't all mount the same one the service can't tell, and each replica quietly has its own store |
| Who can read it | Whoever holds the keys | Anything running as the service's user, including a compromised LibreOffice |

For a store other than AWS, set `AWS_ENDPOINT`, plus `AWS_ALLOW_HTTP=true` if it's plain HTTP.

A Docker named volume starts out owned by root, but the service runs as user 1001, so it refuses to start with `Permission denied`. Set the owner once, before you start the replicas:

```bash
docker volume create rusticdoc-store
docker run --rm -v rusticdoc-store:/data --entrypoint sh --user root \
  ghcr.io/crafted-systems/rusticdoc:0.3.1 -c 'chown 1001:1001 /data'
```

`make replicas` runs several real containers against VersityGW, an S3 gateway, and then against a shared Docker volume.

Back up the bucket and turn on its encryption yourself. It's the only copy, and templates can hold whatever your documents hold.

## What reaches the other replicas

| | Stored as | Reaches other replicas |
|---|---|---|
| Templates | `templates/shared/`. The content is cached in memory and the metadata is read on every request | Immediately, and so does a delete |
| Fonts | `fonts/`, named by their hash, and copied to each replica's `RUSTICDOC_FONT_CACHE_DIR` because LibreOffice needs files | The uploading replica immediately, the others within `RUSTICDOC_STORE_SYNC_INTERVAL` (30 seconds by default) |
| Assets | `assets/`, named as they were uploaded, and copied to each replica's `RUSTICDOC_ASSET_CACHE_DIR` for the render workers to read | The uploading replica immediately, the others within `RUSTICDOC_STORE_SYNC_INTERVAL`, and so do a replacement and a delete |
| Aliases | `aliases/shared/<name>/`, one object per version, read on every request | Immediately, and so does a delete |
| Background jobs | `jobs/shared/<id>/`, one object per state and one for the result | Immediately. Every replica removes expired jobs now and then |

There are no messages, locks or leader between replicas. They don't need any coordination, because everything written to the store is immutable, except assets, where a replacement is one write and the last one holds. That also means you can't change a stored template, so store the new one and point an [alias](/docs/guides/stored-templates/#aliases-and-versions) at it. A background job runs on the replica that accepted it, but any replica can serve requests about it.

Each replica checks the limits on what a scope may hold on its own, so replicas storing at the same moment can go past `RUSTICDOC_MAX_TEMPLATES` and `RUSTICDOC_MAX_TEMPLATE_STORAGE` by whatever is in flight.

## Point the readiness probe at /ready

With a store, a new replica returns `503` and `{"status":"syncing"}` from `/ready` until it has copied the stored fonts and assets. A load balancer that checks `/ready` won't send it any documents before then, so none of them come out in a fallback font or without their images. `/health` returns `200` the whole time, so use it for liveness only.

Once a replica has synced, it stays ready. If a later sync fails, it keeps the fonts and assets it has.

## When the store fails

| What you do | What happens |
|---|---|
| Start a new replica | It runs, `/health` returns `200`, and `/ready` returns `503` for as long as the store is unreachable |
| Render by `templateId` | `503`, with the error `store_unavailable` |
| Render with the template sent as a file | Works as usual |
| A replica already in service | It keeps the fonts it has, logs `font sync failed; keeping the fonts in hand`, and counts it in `rusticdoc_store_font_syncs_total{outcome="failed"}`. Assets are the same, with `asset sync failed; keeping the assets in hand` and `rusticdoc_store_asset_syncs_total` |

A font that fails its hash, or an asset that isn't what its extension says, is left out and logged, so one bad object doesn't keep every new replica out of service.

## What the containers share

Templates, aliases and background jobs are one set for the whole store, so every container on it reads the same ones, and they stay when you replace the API key.

Fonts are one set per store too. Anyone with the API key can [change them](/docs/guides/fonts/#who-can-change-fonts), and a font decides what the digits look like in every document. So are [assets](/docs/guides/assets/#who-can-change-assets).
