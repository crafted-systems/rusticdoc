---
title: Many containers
description: Run several rusticdoc replicas on one store, and know what happens when the store fails.
sidebar:
  order: 2
---

With this page you can run several replicas behind a load balancer, store templates and fonts for all of them, and know what each failure looks like.

Replicas that receive their templates in the request need no store, because they share nothing. You need a store for stored templates and uploaded fonts.

## Name a store

Stored templates and uploaded fonts need somewhere every container can reach, so you have to name it. Until you do, those routes answer `501` and the service stores nothing.

```bash
RUSTICDOC_STORE=s3://my-bucket/rusticdoc   # AWS S3, or a store compatible with it
RUSTICDOC_STORE=file:///data/rusticdoc     # a volume every replica mounts
```

Once it is set, one template behaves the same from any replica:

```bash test
id=$(curl -F file=@invoice.docx http://localhost:9027/v1/templates | sed -E 's/.*"id":"([^"]+)".*/\1/')
curl -F templateId="$id" -F "data=<invoice.json" -F steps=convert \
  http://localhost:9027/v1/render -o invoice.pdf
curl -X DELETE "http://localhost:9027/v1/templates/$id"
```

[Stored templates](/docs/guides/stored-templates/) covers these routes.

## S3 or a volume

| | S3 bucket | Shared volume |
|---|---|---|
| Setting | `s3://bucket/prefix` | `file:///path` |
| Works on Kubernetes | Yes, and this is the one to use | Only with `ReadWriteMany`, which many clusters do not offer |
| Credentials | `AWS_*` variables, or the pod's role. Never in the URL | The file system's permissions |
| What every replica needs | The same URL and credentials | The same volume. The service cannot tell if they do not mount the same one, and each would quietly have its own store |
| Who can read it | Whoever holds the keys | Anything running as the service's user, including a compromised LibreOffice |

For a store other than AWS, set `AWS_ENDPOINT`, and `AWS_ALLOW_HTTP=true` if it is plain HTTP.

A Docker named volume starts out owned by root, and the service runs as user 1001. The service then refuses to start, with `Permission denied`. Set the owner once, before you start the replicas:

```bash
docker volume create rusticdoc-store
docker run --rm -v rusticdoc-store:/data --entrypoint sh --user root \
  ghcr.io/vmosoti/rusticdoc:0.1.0 -c 'chown 1001:1001 /data'
```

`make replicas` runs several real containers against VersityGW, an S3 gateway, and then against a shared Docker volume. It has not been run against AWS itself.

Back up the bucket and turn on its encryption yourself. This is the only copy, and templates can hold what your documents hold.

## What reaches the other replicas

| | Stored as | Reaches other replicas |
|---|---|---|
| Templates | `templates/<caller>/`. The content is cached in memory, and the metadata is read on every request | At once. A delete is also seen at once |
| Fonts | `fonts/`, named by their hash, and copied to each replica's `RUSTICDOC_FONT_CACHE_DIR` because LibreOffice needs files | The uploading replica at once, the others within `RUSTICDOC_STORE_SYNC_INTERVAL`, 30 seconds by default |

Replicas have no messages, locks or leader between them. Everything written to the store is immutable, so they need no coordination. A stored template cannot be changed: store the new one and use its id.

Limits on what a scope may hold are checked by each replica. Replicas storing at the same moment can pass `RUSTICDOC_MAX_TEMPLATES` and `RUSTICDOC_MAX_TEMPLATE_STORAGE` by what is in flight.

## Point the readiness probe at /ready

With a store, a new replica answers `/ready` with `503` and `{"status":"syncing"}` until it has copied the stored fonts. A load balancer that uses `/ready` sends it no documents before then, so none of them come out in a fallback font. `/health` answers `200` during that time, so use it for liveness only.

Once a replica has synced, it stays ready. A later failed sync keeps the fonts it has.

## When the store fails

These results come from a container with an S3 endpoint nothing was listening on, and from reading the code for the case of a replica in service:

| What you do | What happens |
|---|---|
| Start a new replica | It runs, `/health` answers `200`, and `/ready` answers `503` for as long as the store is away |
| Render by `templateId` | `503`, with the error `store_unavailable` |
| Render with the template sent as a file | Works as usual |
| A replica already in service | It keeps the fonts it has, logs `font sync failed; keeping the fonts in hand`, and counts it in `rusticdoc_store_font_syncs_total{outcome="failed"}` |

A font that fails its hash is left out and logged, so one bad object does not hold every new replica out of service.

## Whose templates a caller sees

`RUSTICDOC_STORE_SCOPE` decides this.

| Value | Behaviour |
|---|---|
| `caller`, the default | Each credential sees only its own templates. Another caller's id answers `404`. Replacing a key leaves what the old key stored behind |
| `shared` | Every credential reads and deletes the same templates |

With no credentials configured, every caller is the same caller.

Fonts are one set per deployment, not per caller. Changing them takes a key from `RUSTICDOC_ADMIN_KEYS`, and any other key gets `403`, because a font decides what the digits look like in everyone's documents.
