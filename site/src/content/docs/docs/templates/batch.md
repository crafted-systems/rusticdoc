---
title: Batch rendering
description: Render one template once per element of an array in your data, and get the files back in a ZIP.
sidebar:
  order: 8
---

To turn one template and one list into many files, like a letter per customer or an invoice per order, use `POST /v1/render/batch`. It renders the template once per element and returns a ZIP.

## Render a batch

```bash test
cat > customers.json <<'EOF'
{
  "company": "Mawingu Coffee Co.",
  "customers": [
    { "name": "Ada", "total": 1500 },
    { "name": "Grace", "total": 90 }
  ]
}
EOF
cat > letter-one.txt <<'EOF'
Dear {{ item.name }},
You owe {{ company }} {{ item.total | currency("KES") }}.
({{ batch.index }} of {{ batch.count }})
EOF

curl -F template=@letter-one.txt -F "data=<customers.json" \
  -F splitBy=customers -F 'batchName={{ item.name }}' -F steps=convert \
  http://localhost:9027/v1/render/batch -o letters.zip

unzip -l letters.zip | grep -o 'Ada.pdf'
unzip -l letters.zip | grep -o 'Grace.pdf'
```

## The form

| Field | Required | Meaning |
|---|---|---|
| A file part, or `templateId` | Yes | The template, or the id of a [stored template](/docs/guides/stored-templates/) |
| `data` | Yes | The JSON object holding the array |
| `splitBy` | Yes | The dotted path to the array, like `items` or `order.lines`, or `a.0.b` to go through a list |
| `batchName` | No | A template for each file's name. The default is `{{ batch.index }}` |
| `ctx` | No | A JSON object visible as `ctx` in every render |
| `steps` | No | Steps to run on each file, in order |
| `outputFilename` | No | The ZIP's name. The default is the template's name |

## What each render sees

Each render gets a copy of the whole data, with three changes:

| Name | What it holds |
|---|---|
| `item` | The element for this file |
| `batch.index`, `batch.count` | Its position, counting from 1, and the total |
| The array itself | Replaced by a list holding only this element |

So with `splitBy=customers`, `{{ customers | length }}` is `1` in every file and `{{ customers[0].name }}` is the same as `{{ item.name }}`. Everything else in the data stays, which means a template can loop over `order.lines` and print one line per file when you split on it, with the same tags as a normal render.

Tags in an Office template work as they do in any [Office template](/docs/templates/office/):

```bash test
jq '. + {invoices: [{number: "INV-1"}, {number: "INV-2"}]}' invoice.json \
  > invoices.json
curl -F template=@invoice.docx -F "data=<invoices.json" -F splitBy=invoices \
  -F 'batchName=invoice-{{ item.number }}' -F outputFilename=november \
  http://localhost:9027/v1/render/batch -o november.zip

unzip -l november.zip | grep -o 'invoice-INV-1.docx'
unzip -l november.zip | grep -o 'invoice-INV-2.docx'
```

The sample invoice reads `number` from the top of the data, so both of those files show the same number. To get a different one in each, write `{{ item.number }}` in the template.

## File names

`batchName` is a template that sees the same data as the render, but it only gets the text template filters, which are `currency` and the [built-in filters](/docs/templates/filters/#built-in-filters). `date` and `t` aren't available, and neither is `ctx`.

Leave the extension out, because it's added for you: `batchName=invoice-1.pdf` gives `invoice-1.pdf.pdf`. A file with no steps keeps the template's extension, and `steps=convert` gives `.pdf`.

Directories are removed, so `../x/{{ item.name }}` gives `Ada.txt`, and characters like `:`, `*`, `?` and `|` are dropped. Two files with the same name are both kept, and the second becomes `Ada (2).txt`.

Without `batchName`, the files are `1.txt`, `2.txt` and so on.

## Steps

`steps` takes `convert`, `pdfa`, `pdfua`, `metadata`, `encrypt`, `merge`, `flatten`, `optimize`, `rotate`, `split` and `bookmarks`, which work as they do in the [pipeline](/docs/guides/pipeline/). Their fields, like `userPassword`, apply to every file.

`watermark`, `stamp`, `embed` and `factur-x` are refused in a batch, because they carry files or repeat:

```bash test=fail
curl -F template=@letter.txt -F 'data={"customers":[{"name":"Ada"}]}' \
  -F splitBy=customers -F steps=watermark -F watermarkText=DRAFT \
  http://localhost:9027/v1/render/batch -o out.zip
```

## Limits

| Limit | Value | If you go over |
|---|---|---|
| Elements in one batch | 100, set with `RUSTICDOC_BATCH_LIMIT` | `400`, `'c' has 101 elements; the limit per batch is 100` |
| Time for the whole batch | 5 minutes, set with `RUSTICDOC_BATCH_TIMEOUT` | `503` |
| Elements times the size of `data` | 4 times `RUSTICDOC_BODY_LIMIT`, since each render copies the data | `400`. Split the request |
| One `batchName` once rendered | 4 KB | `400` |

For a large list, send several requests, or add `X-Rusticdoc-Webhook-Url` to get a `204` straight away and the ZIP by webhook.

## When it fails

A batch is all or nothing. If one element fails, the whole batch stops and you get the error instead of a partial ZIP.

| Problem | Response |
|---|---|
| No `splitBy` | `400`, `'splitBy' names the array to split on` |
| The path isn't in the data | `400`, `'nope' is not in the data` |
| The path isn't an array | `400`, `'company' is not an array in the data` |
| The array is empty | `400`, `'customers' is empty` |
| A tag fails for one element | `422`, as in a [normal render](/docs/templates/syntax/#missing-values-are-errors) |
| `batchName` fails | `422`, with `batchName:` in the message |
| A PDF template | `501`. Batch rendering takes Office and text templates |

To check a template first, [validate it](/docs/templates/validate/) against data shaped like one element, with `item` and `batch` in it.
