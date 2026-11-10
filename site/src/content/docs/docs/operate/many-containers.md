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
  ghcr.io/vmosoti/rusticdoc:0.1.0 -c 'chown 1001:1001 /data'
```

`make replicas` runs several real containers against VersityGW, an S3 gateway, and then against a shared Docker volume.

Back up the bucket and turn on its encryption yourself. It's the only copy, and templates can hold whatever your documents hold.

## What reaches the other replicas

| | Stored as | Reaches other replicas |
|---|---|---|
| Templates | `templates/<caller>/`. The content is cached in memory and the metadata is read on every request | Immediately, and so does a delete |
| Fonts | `fonts/`, named by their hash, and copied to each replica's `RUSTICDOC_FONT_CACHE_DIR` because LibreOffice needs files | The uploading replica immediately, the others within `RUSTICDOC_STORE_SYNC_INTERVAL` (30 seconds by default) |
| Aliases | `aliases/<caller>/<name>/`, one object per version, read on every request | Immediately, and so does a delete |
| Background jobs | `jobs/<caller>/<id>/`, one object per state and one for the result | Immediately. Every replica removes expired jobs now and then |

There are no messages, locks or leader between replicas. They don't need any coordination, because everything written to the store is immutable. That also means you can't change a stored template, so store the new one and point an [alias](/docs/guides/stored-templates/#aliases-and-versions) at it. A background job runs on the replica that accepted it, but any replica can serve requests about it.

Each replica checks the limits on what a scope may hold on its own, so replicas storing at the same moment can go past `RUSTICDOC_MAX_TEMPLATES` and `RUSTICDOC_MAX_TEMPLATE_STORAGE` by whatever is in flight.

## Point the readiness probe at /ready

With a store, a new replica returns `503` and `{"status":"syncing"}` from `/ready` until it has copied the stored fonts. A load balancer that checks `/ready` won't send it any documents before then, so none of them come out in a fallback font. `/health` returns `200` the whole time, so use it for liveness only.

Once a replica has synced, it stays ready. If a later sync fails, it keeps the fonts it has.

## When the store fails

| What you do | What happens |
|---|---|
| Start a new replica | It runs, `/health` returns `200`, and `/ready` returns `503` for as long as the store is unreachable |
| Render by `templateId` | `503`, with the error `store_unavailable` |
| Render with the template sent as a file | Works as usual |
| A replica already in service | It keeps the fonts it has, logs `font sync failed; keeping the fonts in hand`, and counts it in `rusticdoc_store_font_syncs_total{outcome="failed"}` |

A font that fails its hash is left out and logged, so one bad object doesn't keep every new replica out of service.

## Whose templates a caller sees

This is set by `RUSTICDOC_STORE_SCOPE`.

| Value | Behaviour |
|---|---|
| `caller`, the default | Each credential sees only its own templates and aliases, and another caller's id returns `404`. If you replace a key, whatever the old key stored is left behind, unless it's a [tenant key](/docs/guides/authentication/#tenant-keys) |
| `shared` | Every credential reads and deletes the same templates and aliases |

Background jobs always belong to the caller that submitted them. With no credentials configured, every caller is the same caller.

Fonts are one set per deployment, not per caller. Changing them takes an [admin](/docs/guides/authentication/#admins) and any other credential gets a `403`, because a font decides what the digits look like in everyone's documents.
