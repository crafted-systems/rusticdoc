# Validate a template

<!-- Generated from site/src/content/docs/docs/templates/validate.md by site/scripts/generate-skills.mjs. Do not edit. -->

Check a template against sample data and get a list of every problem and where it is, without producing a file.

Validation checks a template before anyone relies on it. It finds syntax errors, misplaced loops, values missing from your sample data and bad arguments to filters, and tells you where each one is. Nothing is converted and no file is written.

Run it whenever you change a template, and in CI against a realistic sample of your data.

## Over HTTP

`POST /v1/templates/validate` takes a template, as a file part or a stored `templateId`, plus `data` and `ctx`.

```bash
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

The status is `200` whatever it finds, so check `ok` instead. A `400` means the request itself is wrong, for example because there's no template.

## The result

| Field | Contains |
|---|---|
| `ok` | `true` when there are no errors. Warnings don't change it |
| `template_hash` | The template's SHA-256, in hex |
| `format` | The file's extension |
| `engine` | `office`, `text` or `none` |
| `error` | The first error on one line, or `null` |
| `diagnostics` | Every finding |
| `missing_fonts` | Fonts the template names that the service doesn't have, as in [fonts](https://rusticdoc.com/docs/guides/fonts/#find-fonts-a-template-needs) |

Each finding has these fields:

| Field | Contains |
|---|---|
| `severity` | `error` or `warning` |
| `message` | What's wrong |
| `location` | `part`, `place` and `snippet`, or `null` when there's no place to name |

`part` is the file inside the package, `snippet` is the tag, and `place` describes the position in plain words:

| Format | Example `place` |
|---|---|
| DOCX | `paragraph 12`, `table 1 row 1 paragraph 4` |
| XLSX | `sheet "Sheet1" row 1 (cell A1)` |
| PPTX | `slide 2 paragraph 2` |
| ODS | `table 1 row 1 paragraph 1` |
| ODP | `slide 3 paragraph 3` |

## A template with problems

If your sample data is missing most of what the invoice needs, you get one finding for each missing value:

```bash
curl -F template=@invoice.docx -F 'data={"number":"INV-1"}' \
  http://localhost:9027/v1/templates/validate -o result.json
jq -e '.ok == false and (.diagnostics | length) == 9' result.json
jq -e '.diagnostics[0].location.snippet == "{{ seller.name }}"' result.json
```

Here's that result, shortened:

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
| A tag that can't be parsed | `cannot parse tag: syntax error: ...` |
| A block that never closes | `'{% for a in b %}' is never closed; add '{% endfor %}'` |
| A closing tag with no opener | `'{% endfor %}' has no matching '{% for %}'` |
| Tags where no rule applies | `'{% else %}' ... needs its own paragraph between ...`, with both positions named |
| A filter that doesn't exist | `unknown filter: filter nosuch is unknown` |
| A missing value | `undefined value`, or `value is undefined: {{ w }}` |
| A bad argument to a rich filter | `image: too many arguments: unknown keyword argument 'foo'` |
| A loop over something that isn't a list | A warning: `loop over a number rather than an array: {% for i in n %}` |
| Text that looks like a broken tag | A warning: `text looks like an unterminated tag` |
| A font the service has neither loaded nor mapped to a substitute | A warning: `the template uses the font 'Aptos', which the service does not have; ...` |

Syntax problems stop the check before the test render, so missing values only show up once the syntax is fixed. Likewise, a value that reaches a built-in filter like `upper` might not be reported until the errors before it are gone. Fix what it lists and run it again until `ok` is `true`.

## Text and PDF templates

A text template reports only its first error. There's no location, but the message includes the line number.

```bash
printf 'line one\n{{ nobody }}\n' > missing.txt
curl -F template=@missing.txt -F 'data={"a":1}' \
  http://localhost:9027/v1/templates/validate -o result.json
jq -e '.ok == false and .engine == "text"
  and .diagnostics[0].location == null' result.json
jq -e '.error | contains("line 2")' result.json
```

The result, without its hash:

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

PDF templates aren't read. A PDF, or a file in any other format, gives `"engine": "none"` and `ok: false`:

```bash
curl -F template=@form.pdf -F 'data={"name":"Ada"}' \
  http://localhost:9027/v1/templates/validate -o result.json
jq -e '.engine == "none" and .ok == false' result.json
```

To check a PDF form, list its fields with [`/v1/pdf/forms/fields`](pdf-templates.md#list-the-fields).

## Validation and rendering differ on empty data

Validation always reads the tags. A render with `data` absent or `{}` returns an Office template unchanged, so a template with errors can still come back with a `200`, and you need validation to find them.

Validation only renders the template against the data you send, though. A branch your sample doesn't take, or a loop over an empty list, isn't evaluated, so values used only there aren't reported. Give it a sample that reaches everything.

Storing a template with `POST /v1/templates` only checks its syntax, since the data comes later (see [stored templates](https://rusticdoc.com/docs/guides/stored-templates/)).

## On the command line

You can run the same check without the service, on the machine that has the file:

```text
rusticdoc validate invoice.docx --data sample.json
```

`--data` (or `-d`) is optional. The command prints only the list of findings as JSON, without the other fields:

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

A clean template prints `[]`. The exit code is `1` when there's an error and `0` otherwise, including when there are only warnings, so it works as a CI step.
