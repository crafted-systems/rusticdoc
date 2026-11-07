---
title: PDF templates
description: Fill PDF form fields from JSON, use tags in fields and annotations, flatten the result, and overlay one PDF on another.
sidebar:
  order: 5
---

This page shows how to fill an existing PDF: its form fields, tags written in fields and annotations, and a second PDF laid over it. Use it for forms you did not design and cannot rebuild, such as a government form or a supplier's order sheet.

## List the fields

Find out what a PDF has before filling it. A PDF with no form lists nothing.

```bash test
curl -F files=@form.pdf http://localhost:9027/v1/pdf/forms/fields -o fields.json
jq -e 'map(.name) == ["name", "date", "agree"]' fields.json
```

```json
[
  { "name": "name", "kind": "text", "page": 1, "value": null },
  { "name": "date", "kind": "text", "page": 1, "value": null },
  { "name": "agree", "kind": "checkbox", "page": 1, "value": "Off" }
]
```

`kind` is `text`, `checkbox`, `radio`, `choice`, `signature` or `button`. `page` starts at 1, and a choice field also lists its `options`.

## Fill a form

Two routes do this:

| Route | The PDF is sent as | Then |
|---|---|---|
| `POST /v1/pdf/forms/fill` | `files` | Returns the filled PDF |
| `POST /v1/render` | `template`, with non-empty `data` | Runs any `steps` on the filled PDF, like any other document |

Both take `data`, `ctx`, `flatten` and `stripAnnotations`, and `outputFilename` names the result.

```bash test
curl -F files=@form.pdf -F 'data={"name":"Ada","date":"2026-11-06","agree":true}' \
  http://localhost:9027/v1/pdf/forms/fill -o filled.pdf

curl -F files=@filled.pdf http://localhost:9027/v1/pdf/forms/fields -o fields.json
jq -e '.[0].value == "Ada" and .[2].value == "Yes"' fields.json
```

```bash test
curl -F template=@form.pdf -F 'data={"name":"Ada"}' http://localhost:9027/v1/render -o filled.pdf
curl -F files=@filled.pdf http://localhost:9027/v1/pdf/forms/fields -o fields.json
jq -e '.[0].value == "Ada"' fields.json
```

On `/v1/render`, a PDF with empty `data` is returned unchanged.

### How values match fields

A field's full name is a path into the data. A field named `customer.name` takes `{"customer":{"name":"Ada"}}`, and `items.0.qty` takes the first element's `qty`. Fields with no match keep what they had, and data with no matching field is ignored.

| Kind | What the value does |
|---|---|
| `text` | Stored as text. Numbers and booleans are written out |
| `checkbox` | On for `true`, a non-zero number, or the strings `true`, `yes`, `on` and `1` in any case, and the checkbox's own on-state. Anything else is off |
| `radio` | Selects the button whose on-state equals the value. `""` and `Off` clear it. Any other value is a `400` that lists the options |
| `choice` | A value or a list of values. A value that is not an option is a `400`, unless the field allows free text |
| `signature`, `button` | Left alone |

## Tags in fields

A field whose value or default value holds a `{{ }}` or `{% %}` tag is rendered, unless the data already has a value under the field's name. A field holding `Dear {{ customer.name }}` becomes `Dear Ada`.

Type the tag in as the field's value in your PDF editor.

## Tags in annotations

A `Text`, `FreeText` or `Square` annotation whose contents hold a tag is rendered too. Other annotations, and those with no tag, stay as they are.

- **Over a form field**, the rendered value fills the field with the largest overlap.
- **Over nothing**, the value is drawn on the page inside the annotation's rectangle.

A tag may end in one of two shorthands:

| Ending | Over a field | Over nothing |
|---|---|---|
| `{{ customer.name \| fill }}` | Fills it | Draws the value |
| `{{ vip \| check }}` | Fills it, so a checkbox follows the checkbox rules above | Draws `X` when the value counts as true, and nothing otherwise |

`fill` and `check` work only as the last filter in a tag that is the annotation's whole content.

Tagged annotations are removed from the output. Send `stripAnnotations=false` to keep them.

### Filters in PDF tags

PDF tags have `currency` and the [built-in filters](/docs/templates/filters/#built-in-filters), such as `upper` and `default`. `date`, `t` and the rich content filters do not exist here. A tag using one is a `422`, with its place in the message:

```text
template error: annotation on page 1: unknown filter: filter date is unknown (in <string>:1)
```

A missing value is an error too, and names the place: ``template error: field `greeting`: undefined value``. Format dates before sending them.

## Flatten

`flatten=true` draws the filled values into the page, and removes the fields, so they can no longer be changed.

```bash test
curl -F files=@form.pdf -F 'data={"name":"Ada","agree":true}' -F flatten=true \
  http://localhost:9027/v1/pdf/forms/fill -o flat.pdf

curl -F files=@flat.pdf http://localhost:9027/v1/pdf/forms/fields -o fields.json
jq -e 'length == 0' fields.json
```

## Text is drawn in Latin only

Text the service draws itself, meaning field appearances and annotation values, uses Helvetica with Windows Latin encoding. That covers Western European text and symbols such as `€`. Other characters draw as `?`.

| Value | Stored in the field | Drawn, including when flattened |
|---|---|---|
| `é € ok` | `é € ok` | `é € ok` |
| `Ωμέγα 日本` | `Ωμέγα 日本` | `????? ??` |

The value itself is stored in full, so a viewer that redraws the field can still show it. The service asks viewers to redraw, but flattening fixes the `?` characters into the page.

For text outside Latin, build the document from an [Office template](/docs/templates/office/) and convert it, with the [fonts](/docs/guides/fonts/) it needs.

## Overlay

`POST /v1/pdf/overlay` takes exactly two PDFs in `files`: the base, then the overlay. Each overlay page is drawn over the matching base page. Extra base pages stay as they are, and extra overlay pages are added at the end. The result is named after the base, unless you send `outputFilename`.

This puts a rendered letter over a letterhead:

```bash test
curl -F template=@letter.txt -F "data=<letter.json" -F steps=convert \
  http://localhost:9027/v1/render -o letter.pdf

curl -F files=@a.pdf -F files=@letter.pdf -F outputFilename=on-letterhead \
  http://localhost:9027/v1/pdf/overlay -o on-letterhead.pdf
test "$(head -c 4 on-letterhead.pdf)" = "%PDF"
```

Anything other than two PDFs is a `400`.

## What is not supported

- `/v1/templates/validate` does not read PDFs. It answers `ok: false` with `'.pdf' is not a template format`.
- `/v1/render/batch` does not take a PDF template. It answers `501`.
- Encrypted PDF forms answer `501`.
- Rich text, comb fields, date formats and JavaScript actions in a form are not interpreted.
- Filled forms have not been viewed in Chrome, Acrobat or Preview. See [known limits](/docs/project/known-limits/).

## On the command line

```text
rusticdoc pdf fields form.pdf
rusticdoc pdf fill form.pdf --data values.json --flatten -o filled.pdf
rusticdoc pdf overlay base.pdf letter.pdf -o combined.pdf
```
