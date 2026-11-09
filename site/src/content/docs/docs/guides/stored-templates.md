---
title: Stored templates
description: Upload a template once, then render it by id or alias, roll versions forward and back, list the data it reads and preview it as a PNG.
sidebar: { order: 4 }
---

Stored templates let you upload a file once and then send only its id or a name. You can give a template an alias and roll its versions forward and back, render it with a form or with JSON, list the data it reads and draw a page of it.

## Turn it on

The routes need `RUSTICDOC_STORE`, which points at a place every replica can reach:

```bash
RUSTICDOC_STORE=s3://my-bucket/rusticdoc   # S3 or a store compatible with it
RUSTICDOC_STORE=file:///data/rusticdoc     # a volume every replica mounts
```

Without it, each route returns `501 store_not_configured` and the service stores nothing. See [running many containers](/docs/operate/many-containers/) for setting up a store.

## Upload a template

`POST /v1/templates` takes one file and returns `201`, with the template's location in a `Location` header.

| Field | Required | Meaning |
|---|---|---|
| `template` | Yes | The file: DOCX, XLSX, PPTX, ODT, ODS, ODP, PDF, or a text format (`txt`, `md`, `html`, `htm`, `csv`, `xml`, `json`, `svg`) |
| `name` | No | A name to use instead of the file's. The file keeps its own extension |

```bash test
curl -F template=@invoice.docx -F name=invoice-2026 \
  http://localhost:9027/v1/templates -o stored.json
jq -e '.name == "invoice-2026.docx" and .format == "docx"' stored.json
curl -X DELETE "http://localhost:9027/v1/templates/$(jq -r .id stored.json)"
```

The response describes the stored template:

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

Syntax is checked at upload, without data, so a broken template gets `422 template_error` here instead of at its first render. A PDF is only checked to be a PDF. A file in a format that isn't in the table gets a `400`, and so does sending two files.

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

Sending both `templateId` and a `template` file part gets a `400`. Any files you do send with `templateId` are extra inputs for the `merge` step.

`templateId` also takes an [alias](#aliases-and-versions), written as `invoice` or `invoice@2`.

An id this caller doesn't have returns `404 not_found`, and so does one that's neither a UUID nor an alias.

## Send JSON instead of a form

A render by id has no files to send, so `/v1/render`, `/v1/render/batch` and `/v1/templates/validate` also take a JSON object with `Content-Type: application/json`:

```bash test
curl -F template=@letter.txt http://localhost:9027/v1/templates -o stored.json
ID=$(jq -r .id stored.json)

cat > body.json <<EOF
{
  "templateId": "$ID",
  "data": {"customer": {"name": "Ada"}, "total": 1500},
  "outputFilename": "ada"
}
EOF
curl -H "Content-Type: application/json" -d @body.json \
  http://localhost:9027/v1/render -o ada.txt
grep -q "Dear Ada" ada.txt

curl -X DELETE "http://localhost:9027/v1/templates/$ID"
```

Every text field of the form becomes a member with the same name, the same meaning and the same limits:

| Member | As JSON |
|---|---|
| `templateId` | A string, either an id or an alias |
| `data`, `ctx`, `metadata`, `bookmarks`, `downloadFrom` | Objects and arrays as they are, not as text |
| `steps` | A list like `["convert","pdfa"]`, or the form's comma-separated text |
| Anything else, like `to`, `pdfa`, `outputFilename`, `splitBy`, `flatten` | A string, number or boolean |

A JSON body can't carry files, so a member named `template`, `files`, `assets`, `embeds`, `watermark` or `stamp` gets a `400`, and so does a body that isn't an object or isn't valid JSON. The body limit and the limit of 2000 fields apply as they do for a form. Without a store, JSON still works with a `templateHash`.

## List

`GET /v1/templates` lists this caller's templates, oldest first.

| Query | Default | Meaning |
|---|---|---|
| `limit` | `50` | How many to return. A value outside 1 to 200 is moved to the nearer of the two |
| `cursor` | None | The `nextCursor` from the previous page |

The response has `templates`, plus `nextCursor` when there's another page. There's no total count, so to read everything, follow `nextCursor` until it's absent.

```bash test
curl -F template=@letter.txt http://localhost:9027/v1/templates -o one.json
curl -F template=@letter.txt http://localhost:9027/v1/templates -o two.json

curl "http://localhost:9027/v1/templates?limit=1" -o page.json
jq -e '(.templates | length) == 1
  and (.nextCursor | type) == "string"' page.json
cursor=$(jq -r .nextCursor page.json)
curl "http://localhost:9027/v1/templates?limit=1&cursor=$cursor" -o next.json
jq -e '.templates | type == "array"' next.json

curl -X DELETE "http://localhost:9027/v1/templates/$(jq -r .id one.json)"
curl -X DELETE "http://localhost:9027/v1/templates/$(jq -r .id two.json)"
```

## Show and download

`GET /v1/templates/{id}` returns the same JSON as the upload.

`GET /v1/templates/{id}/content` returns the file as it was uploaded. It's always served as `application/octet-stream` with `content-disposition: attachment`, so a stored HTML template can't run on the service's origin.

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

`DELETE /v1/templates/{id}` returns `204`. Metadata is read on every request, so every replica stops serving the template immediately, and a later request for the id gets a `404`.

```bash test=fail
curl -F template=@letter.txt http://localhost:9027/v1/templates -o stored.json
ID=$(jq -r .id stored.json)
curl -X DELETE "http://localhost:9027/v1/templates/$ID"
curl "http://localhost:9027/v1/templates/$ID"
```

## A stored template can't change

There's no route to replace a template's content. Every upload gets a new UUID as its id, so uploading the same bytes twice stores two templates with one `sha256`. To change a template, store the new file and point an alias at it.

## Aliases and versions

An alias is a name for a template, like `invoice`, that your application renders instead of an id. Pointing the alias at a newer template makes a new version, and the earlier versions stay listed and can still be rendered.

`PUT /v1/aliases/{name}` takes `templateId`, as JSON or a form field. A new alias returns `201` at version 1, and pointing an existing one somewhere else returns `200` with the next version. Every replica sees the change immediately.

```bash test
curl -F template=@letter.txt http://localhost:9027/v1/templates -o one.json
curl -F template=@letter.txt http://localhost:9027/v1/templates -o two.json

cat > alias.json <<EOF
{"templateId": "$(jq -r .id one.json)"}
EOF
curl -X PUT -H "Content-Type: application/json" -d @alias.json \
  http://localhost:9027/v1/aliases/letter -o v1.json
jq -e '.version == 1' v1.json
curl -X PUT -F templateId="$(jq -r .id two.json)" \
  http://localhost:9027/v1/aliases/letter -o v2.json
jq -e '.version == 2' v2.json

curl -F templateId=letter -F "data=<letter.json" \
  http://localhost:9027/v1/render -o now.txt
curl -F templateId=letter@1 -F "data=<letter.json" \
  http://localhost:9027/v1/render -o before.txt
grep -q "Dear Ada" now.txt && grep -q "Dear Ada" before.txt

curl http://localhost:9027/v1/aliases/letter -o history.json
jq -e --arg id "$(jq -r .id two.json)" \
  '(.versions | length) == 2 and .templateId == $id' history.json

curl -X DELETE http://localhost:9027/v1/aliases/letter
curl -X DELETE "http://localhost:9027/v1/templates/$(jq -r .id one.json)"
curl -X DELETE "http://localhost:9027/v1/templates/$(jq -r .id two.json)"
```

An alias works anywhere an id is accepted:

| Written as | Meaning |
|---|---|
| `invoice` | The template the alias names now |
| `invoice@2` | The template its version 2 named |

That covers `templateId` on every render route and in JSON, as well as `GET /v1/templates/{id}`, `/content`, `/variables` and `/preview`. The exception is deleting a template, which is by id only. The `templateId` in `PUT /v1/aliases/{name}` can itself be an alias, which copies where that alias points now.

| Route | Returns |
|---|---|
| `GET /v1/aliases/{name}` | `name`, the current `templateId`, `version` and `setAt`, plus `versions`, which holds every version, oldest first, each with `version`, `templateId` and `setAt` |
| `GET /v1/aliases` | `aliases`, by name, each with its current `templateId` and `version`, plus `nextCursor`. It takes `limit` and `cursor` like the template list |
| `DELETE /v1/aliases/{name}` | `204`. The alias and its history are gone from every replica immediately, but the templates stay. Setting it again starts at version 1 |

To roll back, point the alias at the older template again, which adds a version, or render `invoice@1` while you decide.

An alias whose template has since been deleted returns a `404` that says which template it names.

An alias is 1 to 64 letters, digits, `.`, `_` and `-`. It has to start with a letter or digit and can't be shaped like a UUID. Any other name gets a `400`. Each caller has its own aliases, scoped the way templates are, so two callers can both have an `invoice`.

Nothing in the store is modified to make this work, because each version is an object of its own and the newest one is the alias. A delete writes a marker first, then removes the older records.

## See what a template reads

`GET /v1/templates/{id}/variables` lists the data a template reads, worked out from its tags, so your application can build its data or check what it has:

```bash test
curl -F template=@letter.txt http://localhost:9027/v1/templates -o stored.json
ID=$(jq -r .id stored.json)
curl "http://localhost:9027/v1/templates/$ID/variables" -o variables.json
jq -e '.example == {"customer": {"name": ""}, "total": ""}' variables.json
curl -X DELETE "http://localhost:9027/v1/templates/$ID"
```

For a template with `{% for line in lines %}{{ line.sku }}{% endfor %}` it returns:

```json
{
  "templateId": "01a1b291-787f-748b-ad1a-e899ffc5aa28",
  "format": "docx",
  "variables": [
    { "path": "lines", "type": "array" },
    { "path": "lines[]", "type": "object" },
    { "path": "lines[].sku", "type": "value" }
  ],
  "example": { "lines": [{ "sku": "" }] }
}
```

| Field | Meaning |
|---|---|
| `variables` | Every path the template reads, parents before their fields, in the order the template first names them |
| `path` | Dotted, with `[]` for each item of an array. `lines[]` describes what each item is |
| `type` | `array` when the template loops over it, `object` when it reads fields of it, `value` otherwise |
| `example` | Data of the right shape, with empty strings where values go and one item in each array |

Names the template creates itself, like a loop variable, a `{% set %}` or a macro's arguments, aren't data, so they're left out. A filter that names a field, like `items | sum(attribute="price")` or `selectattr("paid")`, adds that field to the items. A path built at render time, like `row[column]`, is only listed as far as it's written. A template with syntax errors returns `422`.

For a PDF form, the variables are its fields' names, read as paths (`items.0.qty` becomes `items[].qty`), and the tags in the fields' values. Tags in annotations aren't listed.

## Preview a page

`POST /v1/templates/{id}/preview` renders the template with sample data and draws one page as a PNG. It takes JSON or a form:

| Field | Default | Meaning |
|---|---|---|
| `data`, `ctx` | `{}` | The sample data |
| `page` | `1` | The page to draw |
| `width` | `800` | The image's width in pixels, from 16 to 2000. The height follows the page |
| `dpi` | None | Draws at this resolution instead, from 18 to 300. Send `width` or `dpi`, not both |
| `htmlEngine` | `fulgur` | For HTML templates, the same as on a render |

```bash test
curl -F template=@invoice.html http://localhost:9027/v1/templates -o stored.json
ID=$(jq -r .id stored.json)
curl -s -D headers.txt -H "Content-Type: application/json" -d '{"width": 400}' \
  "http://localhost:9027/v1/templates/$ID/preview" -o page.png
[ "$(head -c 4 page.png | tail -c 3)" = PNG ]
grep -qi "^x-page-count: 1" headers.txt
curl -X DELETE "http://localhost:9027/v1/templates/$ID"
```

The response is `image/png`, with the rendered document's page count in `X-Page-Count`. Office templates are converted with LibreOffice and HTML with its HTML engine, and a PDF form is filled when `data` isn't empty. The page is then drawn by poppler's `pdftoppm`.

| Status | When |
|---|---|
| `400` | The page is past the end, or a size is out of range. An image can never be more than 4000 pixels on a side, so a large page at a high `dpi` is refused |
| `422` | The template failed to render with this data |
| `503 engine_unavailable` | `pdftoppm` isn't installed, or LibreOffice is needed and missing. Without `pdftoppm`, the error comes back before anything is rendered |

The container image includes `pdftoppm`. Anywhere else, install poppler or set `RUSTICDOC_PDFTOPPM_BIN`.

## Scope

`RUSTICDOC_STORE_SCOPE` decides who sees what.

| Value | Behaviour |
|---|---|
| `caller` (default) | Each credential only sees its own templates and aliases. Another caller's id returns `404`, not `403`. An API key's scope follows the key, so replacing a key leaves its templates behind, unless it's a [tenant key](/docs/guides/authentication/#tenant-keys) |
| `shared` | Every credential reads and deletes the same templates and aliases. Background jobs still belong to each caller |

An admin key is a credential with its own scope, so it can't see other callers' templates either. With no [credentials configured](/docs/guides/authentication/), there's one scope, shared by everyone who can reach the service.

## Limits

| Setting | Default | What happens past it |
|---|---|---|
| `RUSTICDOC_MAX_TEMPLATE_BYTES` | `50MB` | `413 template_too_large` |
| `RUSTICDOC_MAX_TEMPLATES` | `10000`, per scope | `403 quota_exceeded`. It also caps aliases, which are counted separately from templates |
| Versions of one alias | `1000` | `403 quota_exceeded`. Delete the alias to start again |
| `RUSTICDOC_MAX_TEMPLATE_STORAGE` | `5GB`, per scope | `403 quota_exceeded` |
| `RUSTICDOC_BODY_LIMIT` | `64MB` | `400 invalid_request`, because the whole form didn't fit |

Replicas don't coordinate, so when several store at the same moment they can go past the quotas by whatever is in flight.

If the store can't be reached, these routes and renders by id return `503 store_unavailable`. Requests that send their template as a file keep working.
