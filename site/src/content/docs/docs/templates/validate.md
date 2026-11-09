---
title: Validate a template
description: List every problem in a template against sample data, with where each one sits, without producing a file.
sidebar:
  order: 7
---

This page shows how to check a template before anyone relies on it. Validation finds syntax errors, misplaced loops, values missing from your sample data and bad arguments to filters, and tells you where each one sits. It converts nothing and writes no file.

Run it when you change a template, and in your CI against a realistic sample of your data.

## Over HTTP

`POST /v1/templates/validate` takes a template as a file part or a stored `templateId`, plus `data` and `ctx`.

```bash test
curl -F template=@invoice.docx -F "data=<invoice.json" \
  http://localhost:9027/v1/templates/validate -o result.json
jq -e '.ok == true and .diagnostics == []' result.json
```

```json
{
  "ok": true,
  "template_hash": "6326050fad2f6d1a262317b52516df8e1be1e7da48e75cdd16c9583abcca70d0",
  "format": "docx",
  "engine": "office",
  "error": null,
  "diagnostics": [],
  "missing_fonts": []
}
```

The status is `200` whatever it finds, so read `ok`. A `400` means the request itself is wrong, such as having no template.

## The result

| Field | Holds |
|---|---|
| `ok` | `true` when there are no errors. Warnings do not change it |
| `template_hash` | The hex SHA-256 of the template |
| `format` | The file's extension |
| `engine` | `office`, `text` or `none` |
| `error` | The first error as one line, or `null` |
| `diagnostics` | Every finding |
| `missing_fonts` | Fonts the template names that the service lacks, as in [fonts](/docs/guides/fonts/#find-fonts-a-template-needs) |

Each finding has these fields:

| Field | Holds |
|---|---|
| `severity` | `error` or `warning` |
| `message` | What is wrong |
| `location` | `part`, `place` and `snippet`, or `null` when there is no place to name |

`part` is the file inside the package. `place` says where in plain words, and `snippet` is the tag:

| Format | A `place` looks like |
|---|---|
| DOCX | `paragraph 12`, `table 1 row 1 paragraph 4` |
| XLSX | `sheet "Sheet1" row 1 (cell A1)` |
| PPTX | `slide 2 paragraph 2` |
| ODS | `table 1 row 1 paragraph 1` |
| ODP | `slide 3 paragraph 3` |

## A template with problems

Sample data that lacks most of what the invoice needs gives one finding per missing value:

```bash test
curl -F template=@invoice.docx -F 'data={"number":"INV-1"}' \
  http://localhost:9027/v1/templates/validate -o result.json
jq -e '.ok == false and (.diagnostics | length) == 9' result.json
jq -e '.diagnostics[0].location.snippet == "{{ seller.name }}"' result.json
```

Shortened, that result reads:

```json
{
  "ok": false,
  "format": "docx",
  "engine": "office",
  "error": "undefined value (word/document.xml table 1 row 1 paragraph 1: {{ seller.name }})",
  "diagnostics": [
    {
      "location": {
        "part": "word/document.xml",
        "place": "table 1 row 1 paragraph 1",
        "snippet": "{{ seller.name }}"
      },
      "message": "undefined value",
      "severity": "error"
    },
    {
      "location": {
        "part": "word/document.xml",
        "place": "table 1 row 1 paragraph 2",
        "snippet": "{{ seller.address }}"
      },
      "message": "value is undefined: {{ seller.address }}",
      "severity": "error"
    }
  ]
}
```

## What it finds

In an Office template:

| Problem | Message |
|---|---|
| A tag that cannot parse | `cannot parse tag: syntax error: ...` |
| A block that never closes | `'{% for a in b %}' is never closed; add '{% endfor %}'` |
| A closing tag with no opener | `'{% endfor %}' has no matching '{% for %}'` |
| Tags where no rule applies | `'{% else %}' ... needs its own paragraph between ...`, with both positions named |
| A filter that does not exist | `unknown filter: filter nosuch is unknown` |
| A missing value | `undefined value`, or `value is undefined: {{ w }}` |
| A bad argument to a rich filter | `image: too many arguments: unknown keyword argument 'foo'` |
| A loop over something that is not a list | A warning: `loop over a number rather than an array: {% for i in n %}` |
| Text that looks like a broken tag | A warning: `text looks like an unterminated tag` |
| A font the service has neither loaded nor mapped to a substitute | A warning: `the template uses the font 'Aptos', which the service does not have; ...` |

Syntax problems stop the check before the test render, so missing values show up after they are fixed. In the same way, a value that reaches a built-in filter such as `upper` may be reported only after the errors before it are gone. **Fix what it lists and run it again until `ok` is `true`.**

## Text and PDF templates

A text template reports only its first error, with no location. The message holds the line number.

```bash test
printf 'line one\n{{ nobody }}\n' > missing.txt
curl -F template=@missing.txt -F 'data={"a":1}' \
  http://localhost:9027/v1/templates/validate -o result.json
jq -e '.ok == false and .engine == "text"
  and .diagnostics[0].location == null' result.json
jq -e '.error | contains("line 2")' result.json
```

Without its hash, the result reads:

```json
{
  "ok": false,
  "format": "txt",
  "engine": "text",
  "error": "template error: undefined value (in <string>:2) (line 2)",
  "diagnostics": [
    {
      "location": null,
      "message": "template error: undefined value (in <string>:2) (line 2)",
      "severity": "error"
    }
  ]
}
```

PDF templates are not read. A PDF, or any other format, gives `"engine": "none"` and `ok: false`:

```bash test
curl -F template=@form.pdf -F 'data={"name":"Ada"}' \
  http://localhost:9027/v1/templates/validate -o result.json
jq -e '.engine == "none" and .ok == false' result.json
```

To check a PDF form, list its fields with [`/v1/pdf/forms/fields`](/docs/templates/pdf-templates/#list-the-fields).

## Validation and rendering differ on empty data

Validation always reads the tags. A render with `data` absent or `{}` returns an Office template as it is, so a template with errors can come back with a `200`. Run validation to find them.

Validation also renders a template only against the data you send. A branch your sample does not take, or a loop over an empty list, is not evaluated, so values used only there are not reported. Give it a sample that reaches everything.

Storing a template with `POST /v1/templates` checks only its syntax, since the data comes later. See [stored templates](/docs/guides/stored-templates/).

## On the command line

The same check runs without the service, on the machine with the file:

```text
rusticdoc validate invoice.docx --data sample.json
```

`--data`, or `-d`, is optional. It prints the list of findings as JSON, with none of the other fields:

```json
[
  {
    "severity": "error",
    "message": "undefined value",
    "location": {
      "part": "word/document.xml",
      "place": "table 1 row 1 paragraph 1",
      "snippet": "{{ seller.name }}"
    }
  }
]
```

A clean template prints `[]`. The exit code is `1` when there is an error and `0` otherwise, including when there are only warnings, so it fits a CI step.
