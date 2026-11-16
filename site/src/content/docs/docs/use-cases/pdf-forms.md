---
title: Fill PDF forms from JSON
description: Fill the AcroForm fields of an existing PDF from JSON, tick checkboxes, flatten the result so it can't be edited, and add more PDF steps in one request.
sidebar:
  order: 2
---

Some forms only exist as PDFs you can't rebuild, like a government form or a supplier's order sheet. If the PDF has form fields, those fields are already the template: you send JSON keyed by field name and get the filled PDF back. The form's layout stays in the PDF, and your code only knows the field names.

## List the fields

Start by asking the service what fields the PDF has. The sample `form.pdf` has two text fields and a checkbox:

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

`kind` is `text`, `checkbox`, `radio`, `choice`, `signature` or `button`. A choice field also lists its `options`. If the PDF has no form, the list is empty.

## Fill the fields

Send the PDF in `files` and the values in `data`, with the field names as keys:

```bash test
curl -F files=@form.pdf \
  -F 'data={"name":"Ada Lovelace","date":"2026-11-06","agree":true}' \
  http://localhost:9027/v1/pdf/forms/fill -o filled.pdf

curl -F files=@filled.pdf \
  http://localhost:9027/v1/pdf/forms/fields -o fields.json
jq -e '.[0].value == "Ada Lovelace" and .[2].value == "Yes"' fields.json
```

The result is the same PDF with the values in its fields, still editable. A field with no value in the data keeps what it had, and data with no matching field is ignored.

## How values match fields

A field's full name is a path into the data, so a field named `customer.name` takes `{"customer":{"name":"Ada"}}`, and `items.0.qty` takes the first element's `qty`. That lets you send the same nested JSON your application already has.

| Kind | What the value does |
|---|---|
| `text` | Stored as text. Numbers and booleans are written out |
| `checkbox` | Ticked for `true`, a non-zero number, the strings `true`, `yes`, `on` and `1` in any case, or the checkbox's own on-state |
| `radio` | Selects the button whose on-state equals the value. Any other value fails with a `400` that lists the options |
| `choice` | A value, or a list of values. A value that isn't an option fails with a `400`, unless the field allows free text |
| `signature`, `button` | Left alone |

## Flatten the result

`flatten=true` draws the values into the page and removes the fields, so nobody can change them afterwards:

```bash test
curl -F files=@form.pdf -F 'data={"name":"Ada Lovelace","agree":true}' \
  -F flatten=true http://localhost:9027/v1/pdf/forms/fill -o flat.pdf

curl -F files=@flat.pdf http://localhost:9027/v1/pdf/forms/fields -o fields.json
jq -e 'length == 0' fields.json
```

## Fill and finish in one request

`/v1/render` takes a PDF form as its `template`, fills it from `data` and then runs any [steps](/docs/guides/pipeline/#steps) on it. This one fills and flattens the form, appends another PDF after it, and encrypts the result:

```bash test
curl -F template=@form.pdf \
  -F 'data={"name":"Ada Lovelace","date":"2026-11-06","agree":true}' \
  -F flatten=true -F files=@a.pdf \
  -F steps=merge,encrypt -F userPassword=open-sesame \
  http://localhost:9027/v1/render -o application.pdf
head -c 5 application.pdf
```

On `/v1/render`, a PDF sent with empty `data` comes back unchanged.

## Text the form has no field for

A field whose value holds a tag, like `Dear {{ customer.name }}`, is rendered from the data, so you can type tags into fields in your PDF editor. A `Text`, `FreeText` or `Square` annotation that holds a tag is rendered too. Over a field, it fills that field. Over nothing, its value is drawn on the page inside the annotation's rectangle:

```text
{{ customer.name | fill }}
{{ vip | check }}
```

`check` draws an `X` when the value counts as true. These tags can use `currency`, `number`, `date` and the built-in filters, and read `lang`, `timezone` and `currency` from the request. See [tags in annotations](/docs/templates/pdf-templates/#tags-in-annotations).

Values aren't limited to Latin text. Arabic, Hebrew, Greek, Cyrillic, Chinese, Japanese and Korean are shaped and drawn with the service's own fonts, and only the glyphs used are embedded. See [text in any script](/docs/templates/pdf-templates/#text-in-any-script).

## What isn't supported

- [Batch rendering](/docs/templates/batch/) doesn't take a PDF template, and returns `501`. Send one request per form.
- Encrypted PDF forms fail with `501`.
- Rich text, comb fields, date formats and JavaScript actions in a form aren't interpreted.
- [Validation](/docs/templates/validate/) doesn't read PDFs.

If a PDF has no fields at all, you can still fill it by laying a second PDF over it, as [overlay](/docs/templates/pdf-templates/#overlay) shows.

## On the command line

```text
rusticdoc pdf fields form.pdf
rusticdoc pdf fill form.pdf --data values.json --flatten -o filled.pdf
```

## Next

- [PDF templates](/docs/templates/pdf-templates/) covers forms, tags in fields and annotations, and overlays in full.
- [PDF operations](/docs/guides/pdf-operations/) lists every route for finishing a PDF.
- [The pipeline](/docs/guides/pipeline/) explains how steps run after a form is filled.
