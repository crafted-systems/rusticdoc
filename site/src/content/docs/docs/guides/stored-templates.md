---
title: Stored templates
description: Upload a template once, then render it by id from any replica, and list, download and delete what you stored.
sidebar: { order: 4 }
---

Stored templates let you upload a file once and then send only its id. After this page you can store a template, render it by `templateId`, page through what you stored and remove it.

## Turn it on

The routes need `RUSTICDOC_STORE`, which names a place every replica can reach:

```bash
RUSTICDOC_STORE=s3://my-bucket/rusticdoc   # S3 or a store compatible with it
RUSTICDOC_STORE=file:///data/rusticdoc     # a volume every replica mounts
```

Without it each route answers `501 store_not_configured` and the service stores nothing. See [running many containers](/docs/operate/many-containers/).

## Upload a template

`POST /v1/templates` takes one file and answers `201`, with the template's location in a `Location` header.

| Field | Required | Meaning |
|---|---|---|
| `template` | Yes | The file: DOCX, XLSX, PPTX, ODT, ODS, ODP, PDF, or a text format (`txt`, `md`, `html`, `htm`, `csv`, `xml`, `json`, `svg`) |
| `name` | No | A name in place of the file's. The extension stays the file's |

```bash test
curl -F template=@invoice.docx -F name=invoice-2026 \
  http://localhost:9027/v1/templates -o stored.json
jq -e '.name == "invoice-2026.docx" and .format == "docx"' stored.json
curl -X DELETE "http://localhost:9027/v1/templates/$(jq -r .id stored.json)"
```

The response describes it:

```json
{
  "id": "01a1b291-787f-748b-ad1a-e899ffc5aa28",
  "name": "invoice-2026.docx",
  "format": "docx",
  "size": 37435,
  "sha256": "6326050fad2f6d1a262317b52516df8e1be1e7da48e75cdd16c9583abcca70d0",
  "createdAt": "2026-11-06T20:34:00Z"
}
```

| Field | Meaning |
|---|---|
| `id` | A UUID. Pass it as `templateId` |
| `name` | The name it was stored under |
| `format` | The file's extension |
| `size` | Bytes |
| `sha256` | Hex SHA-256 of the content |
| `createdAt` | RFC 3339 |

Syntax is checked at upload, without data, so a broken template gets `422 template_error` here rather than at its first render. A file in another format gets a `400`. A PDF is only checked to be a PDF. Sending two files is a `400`.

## Render by id

Send `templateId` in place of the file. It works on [`/v1/render`](/docs/guides/pipeline/), [`/v1/render/batch`](/docs/templates/batch/) and [`/v1/templates/validate`](/docs/templates/validate/).

```bash test
curl -F template=@letter.txt http://localhost:9027/v1/templates -o stored.json
ID=$(jq -r .id stored.json)

curl -F templateId="$ID" -F "data=<letter.json" -F steps=convert \
  http://localhost:9027/v1/render -o letter.pdf
head -c4 letter.pdf | grep -q %PDF

curl -X DELETE "http://localhost:9027/v1/templates/$ID"
```

Sending both `templateId` and a `template` file part is a `400`. With `templateId`, any files you do send are further inputs for the `merge` step.

An id this caller does not have answers `404 not_found`. So does an id that is not a UUID.

## List

`GET /v1/templates` lists this caller's templates, oldest first.

| Query | Default | Meaning |
|---|---|---|
| `limit` | `50` | How many to return. Values outside 1 to 200 are moved to the nearest of those |
| `cursor` | None | `nextCursor` from the page before |

The response has `templates`, and `nextCursor` when there is another page. It has no total count. To read everything, follow `nextCursor` until it is absent.

```bash test
curl -F template=@letter.txt http://localhost:9027/v1/templates -o one.json
curl -F template=@letter.txt http://localhost:9027/v1/templates -o two.json

curl "http://localhost:9027/v1/templates?limit=1" -o page.json
jq -e '(.templates | length) == 1 and (.nextCursor | type) == "string"' page.json
curl "http://localhost:9027/v1/templates?limit=1&cursor=$(jq -r .nextCursor page.json)" -o next.json
jq -e '.templates | type == "array"' next.json

curl -X DELETE "http://localhost:9027/v1/templates/$(jq -r .id one.json)"
curl -X DELETE "http://localhost:9027/v1/templates/$(jq -r .id two.json)"
```

## Show and download

`GET /v1/templates/{id}` returns the same JSON as the upload.

`GET /v1/templates/{id}/content` returns the file as it was uploaded. It is always `application/octet-stream` with `content-disposition: attachment`, so a stored HTML template cannot run on the service's origin.

```bash test
curl -F template=@letter.txt http://localhost:9027/v1/templates -o stored.json
ID=$(jq -r .id stored.json)

curl "http://localhost:9027/v1/templates/$ID" -o shown.json
jq -e '.name == "letter.txt"' shown.json

curl "http://localhost:9027/v1/templates/$ID/content" -o copy.txt
grep -q "customer.name" copy.txt

curl -X DELETE "http://localhost:9027/v1/templates/$ID"
```

## Delete

`DELETE /v1/templates/{id}` answers `204`. Every replica stops serving the template at once, because metadata is read on every request. A later request for the id gets `404`.

```bash test=fail
curl -F template=@letter.txt http://localhost:9027/v1/templates -o stored.json
ID=$(jq -r .id stored.json)
curl -X DELETE "http://localhost:9027/v1/templates/$ID"
curl "http://localhost:9027/v1/templates/$ID"
```

## A stored template cannot change

There is no route to replace a template's content, and there are no versions or aliases yet. To change a template, store the new file, use its new id, and delete the old one.

The id is a new UUID on every upload. Uploading the same bytes twice stores two templates with one `sha256`.

So your application has to hold the mapping from a name such as `invoice` to the current id. You can roll back by pointing it at the older id, as long as you have not deleted that template.

## Scope

`RUSTICDOC_STORE_SCOPE` decides who sees what.

| Value | Behaviour |
|---|---|
| `caller` (default) | Each credential sees only its own templates. Another caller's id answers `404`, not `403`. An API key's scope follows the key, so replacing a key leaves its templates behind |
| `shared` | Every credential reads and deletes the same templates |

An admin key is a credential with its own scope, and it cannot see other callers' templates either. With no credentials configured there is one scope, shared by everyone who can reach the service. See [authentication](/docs/guides/authentication/).

## Limits

| Setting | Default | What happens past it |
|---|---|---|
| `RUSTICDOC_MAX_TEMPLATE_BYTES` | `50MB` | `413 template_too_large` |
| `RUSTICDOC_MAX_TEMPLATES` | `10000`, per scope | `403 quota_exceeded` |
| `RUSTICDOC_MAX_TEMPLATE_STORAGE` | `5GB`, per scope | `403 quota_exceeded` |
| `RUSTICDOC_BODY_LIMIT` | `64MB` | `400 invalid_request`, because the whole form did not fit |

Replicas storing at the same moment can pass the quotas by what is in flight, because they do not coordinate.

If the store cannot be reached, these routes and renders by id answer `503 store_unavailable`. Requests that send their template as a file keep working.
