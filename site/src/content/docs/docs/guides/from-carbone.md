---
title: Moving from Carbone
description: Rewrite Carbone templates for rusticdoc, with a mapping of tags, loops, conditions and common formatters, and where there is no equivalent.
sidebar: { order: 9 }
---

rusticdoc templates and [Carbone](https://carbone.io/documentation/design/overview/getting-started.html) templates both start from an ordinary Office file, and use different tag languages. After this page you can rewrite a Carbone template by hand, and you will know which features have no equivalent.

There is no converter, and rusticdoc does not read Carbone tags. `{d.customer.name}` in a template comes out as literal text. Each table below links to the Carbone documentation it was checked against.

The tag language here is [minijinja](https://docs.rs/minijinja), which follows Jinja2. Syntax is in [template syntax](/docs/templates/syntax/), and filters in [filters](/docs/templates/filters/).

## The first change

`{d.customer.name}` becomes `{{ customer.name }}`. Drop `d.`, because the data is the root.

```bash test
printf 'Dear {{ customer.name }},\n' > letter.txt
curl -F template=@letter.txt -F 'data={"customer":{"name":"Ada"}}' \
  http://localhost:9027/v1/render -o out.txt
grep -q "Dear Ada," out.txt
```

## Differences to plan for

- **A missing value is an error.** `{{ missing }}` gets `422`. Use `{{ missing | default("") }}`, or check with [`/v1/templates/validate`](/docs/templates/validate/) as you rewrite.
- **Filters use a pipe and parentheses.** `:upperCase` becomes `| upper`, and arguments are quoted: `| currency("EUR")`.
- **Paths inside a loop are relative to the loop variable**, not the document root.
- **Dates and translations are only in Office templates.** `date`, `t` and `{% settings %}` work in DOCX, XLSX, PPTX, ODT, ODS and ODP. In a text template such as `.txt` or `.html` they get `422`.

## Values and data

[Carbone's tag types](https://carbone.io/documentation/design/overview/getting-started.html).

| Carbone | rusticdoc | Notes |
|---|---|---|
| `{d.customer.name}` | `{{ customer.name }}` | |
| `{d.items[0].name}` | `{{ items[0].name }}` | |
| `{c.lang}` | `{{ ctx.lang }}` | Send `ctx` in place of `complement` |
| `{#total = d.a}`, `{$total}` | `{% set total = a %}`, `{{ total }}` | A `set` inside a loop does not outlive the loop |
| `{o.…}` options | None | |

## Loops

[Carbone's repetitions](https://carbone.io/documentation/design/repetitions/with-arrays.html). Carbone marks a pattern with `[i]` and `[i+1]`. Here, an opening and a closing tag mark it.

| Carbone | rusticdoc |
|---|---|
| `{d.items[i].name}` … `{d.items[i+1].name}` | `{% for item in items %}` … `{{ item.name }}` … `{% endfor %}` |
| Nested `{d[i].models[i].size}` | `{% for car in cars %}{% for model in car.models %}{{ model.size }}{% endfor %}{% endfor %}` |
| `{d.items[i].name:count()}` | `{{ loop.index }}` |
| [Filter](https://carbone.io/documentation/design/repetitions/filtering.html): `{d.items[i+1, qty > 1].name}` | `{% for item in items if item.qty > 1 %}` |
| [Sort](https://carbone.io/documentation/design/repetitions/sorting.html): `{d.items[qty, i].name}` | `{% for item in items \| sort(attribute="qty") %}`. Add `reverse=true` to sort descending |
| [Group](https://carbone.io/documentation/design/repetitions/grouping.html) | `{% for g in items \| groupby("group") %}` with `g.grouper` and `g.list` |
| `{d.items[].qty:aggSum}` | `{{ items \| sum(attribute="qty") }}` |

What a loop repeats depends on where its tags sit, not on special tags. Tags in one table row repeat the row, and tags in their own paragraphs repeat the paragraphs between them. Details and the limits for sheets are in [Office templates](/docs/templates/office/).

```bash test
printf '{%% for item in items | sort(attribute="qty") if item.qty > 1 %%}{{ loop.index }}. {{ item.name }}\n{%% endfor %%}' > list.txt
curl -F template=@list.txt \
  -F 'data={"items":[{"name":"Beans","qty":5},{"name":"Cups","qty":0},{"name":"Filters","qty":2}]}' \
  http://localhost:9027/v1/render -o out.txt
grep -q "1. Filters" out.txt
grep -q "2. Beans" out.txt
```

## Conditions

[Carbone's conditions](https://carbone.io/documentation/design/conditions/overview.html). Carbone chains formatters. Here, a condition is an expression.

| Carbone | rusticdoc |
|---|---|
| `{d.status:ifEQ('paid'):show('Paid'):elseShow('Due')}` | `{{ "Paid" if status == "paid" else "Due" }}` |
| `ifNE`, `ifGT`, `ifGTE`, `ifLT`, `ifLTE` | `!=`, `>`, `>=`, `<`, `<=` |
| `ifIN('a,b')`, `ifNIN` | `status in ["a", "b"]`, `status not in [...]` |
| `ifEM()`, `ifNEM()` | `not value`, `value` |
| `and`, `or` | `and`, `or` |
| `:showBegin` … `:showEnd` | `{% if … %}` … `{% endif %}` |
| `:hideBegin` … `:hideEnd` | `{% if not … %}` … `{% endif %}` |
| [`:drop(row)`, `:keep(row)`](https://carbone.io/documentation/design/conditions/smart-conditions.html) | `{% if … %}` and `{% endif %}` in the same row keep or remove that row |

A `{% if %}` follows the same placement rules as a loop, so tags in one row control that row.

## Common formatters

Here is what the code does, as run on the image. If a formatter is not in these tables, assume there is no equivalent.

### Text

[Carbone's text formatters](https://carbone.io/documentation/design/formatters/text.html).

| Carbone | rusticdoc | Notes |
|---|---|---|
| `:upperCase`, `:lowerCase` | `\| upper`, `\| lower` | |
| `:ucFirst` | `\| capitalize` | Also lowers the rest of the text |
| `:ucWords` | `\| title` | |
| `:prepend('a')`, `:append('a')` | `"a" ~ value`, `value ~ "a"` | |
| `:replace('a','b')` | `\| replace("a", "b")` | |
| `:substr(0, 3)` | `value[0:3]` | No word mode |
| `:split(',')` | `\| split(",")` | |
| `:len` | `\| length` | |
| `:printJSON` | `\| tojson` | |
| `:print('x')` | `{{ "x" }}` | |
| `:convCRLF` | None needed | A newline in a value becomes a line break in DOCX and ODT |
| `:padl`, `:padr`, `:ellipsis`, `:unaccent`, `:formatR` | None | Prepare the value in your data |

### Numbers and currency

[Carbone's number](https://carbone.io/documentation/design/formatters/number.html) and [currency](https://carbone.io/documentation/design/formatters/currency.html) formatters.

| Carbone | rusticdoc | Notes |
|---|---|---|
| `:add(1)`, `:sub(1)`, `:mul(2)`, `:div(2)`, `:mod(2)` | `value + 1`, `- 1`, `* 2`, `/ 2`, `% 2` | |
| `:round(2)` | `\| round(2)` | |
| `:abs` | `\| abs` | |
| `:int` | `\| int` | |
| `:formatN(2)` | `{{ "%.2f" \| format(value) }}` | No thousands separator and no locale |
| `:formatC(2)` | `\| currency("EUR")` | Always 2 decimals, comma thousands and a dot, printing `EUR 1,234.57`. Not locale-aware, and no symbol |
| `:convCurr`, `:formatC(2,'USD')` as a conversion | None | `currency` only labels a number and never converts. Convert in your data |
| `:toWord`, `:ceil`, `:floor` | None | Compute in your data |

### Dates

[Carbone's date formatters](https://carbone.io/documentation/design/formatters/date.html). Office templates only.

| Carbone | rusticdoc | Notes |
|---|---|---|
| `:formatD('DD/MM/YYYY')` | `\| date("%d/%m/%Y")` | Patterns use `strftime` codes, not Day.js tokens. Default `%Y-%m-%d` |
| `:formatD('dddd D MMMM YYYY')` | `\| date("%A %d %B %Y")` | Names are in English whatever the language |
| `timezone` in the request | `{% settings timezone="+03:00" %}` in the template | A fixed offset, not a zone name such as `Africa/Nairobi` |
| Localized patterns such as `LL` | None | |
| `:addD`, `:subD`, `:startOfD`, `:endOfD`, `:diffD` | None | Compute in your data |

Input must be ISO 8601 or unix seconds. `patternIn` has no equivalent.

### Arrays

[Carbone's array formatters](https://carbone.io/documentation/design/formatters/array.html).

| Carbone | rusticdoc |
|---|---|
| `:arrayJoin(', ')` | `\| join(", ")` |
| `:arrayMap` | `items \| map(attribute="name") \| join(", ")` |

## Translations

[Carbone's translations](https://carbone.io/documentation/design/advanced-features/translations-i18n.html). Office templates only.

| Carbone | rusticdoc |
|---|---|
| `{t(Hello)}` | `{{ "hello" \| t }}` |
| `{d.status:t}` | `{{ status \| t }}` |
| `translations` and `lang` in the request | `{% settings lang="fr" t.hello.fr="Bonjour" t.hello.en="Hello" %}` in the template |

The dictionary lives in the template, not in the request, and there is no CLI to extract keys. A missing key prints the key, as in Carbone. To vary the language per request, keep one template per language, or pass translated text in `data`.

## Pictures, barcodes and other rich content

[Carbone's pictures](https://carbone.io/documentation/design/advanced-features/pictures.html) and [barcodes](https://carbone.io/documentation/design/advanced-features/barcode.html). Full details are in [rich content](/docs/templates/rich-content/). These work in Office templates, not text templates.

| Carbone | rusticdoc | Notes |
|---|---|---|
| `{d.image}` in a placeholder picture's alt text | `{{ logo \| image(width=120) }}` in the text | No placeholder picture |
| A public URL as the image value | None | Only base64 or a data URL, never a URL. Fetch the image first and send its bytes. PNG and JPEG only |
| `:imageFit(contain)` | `width=` and `height=`, in points | The aspect ratio is kept when one is given |
| `:barcode(qrcode)` | `\| barcode("qr")` | Also `code128`, `code39`, `ean13`, `ean8`. Other symbologies have no equivalent |
| [Charts](https://carbone.io/documentation/design/advanced-features/charts.html) | `{{ rows \| chart(type="bar", x="month", y=["sales"]) }}` | `bar`, `column`, `line`, `pie`, in DOCX, PPTX and XLSX. Not in ODF. Built from the filter's arguments, not from a chart in the template |
| [`:html`](https://carbone.io/documentation/design/advanced-features/html.html) | `\| html` | A safe subset of tags |
| [`:color`](https://carbone.io/documentation/design/advanced-features/colors.html) | `\| color("#c00")` | Colours the text. `background=` too. In XLSX and ODS, the whole cell |
| [Hyperlinks](https://carbone.io/documentation/design/advanced-features/hyperlinks.html) | `\| link("Open")` | `http`, `https`, `mailto` only. A bad URL is an error, with no `:defaultURL` fallback |

## The API

[Carbone's generate reports reference](https://carbone.io/documentation/developer/http-api/generate-reports.html).

| Carbone | rusticdoc |
|---|---|
| [`POST /template`](https://carbone.io/documentation/developer/http-api/manage-templates.html) | [`POST /v1/templates`](/docs/guides/stored-templates/). There is no version history |
| `POST /render/{templateId}`, then `GET /render/{renderId}` | [`POST /v1/render`](/docs/guides/pipeline/), which returns the file in the response. Nothing is stored for later download |
| `?download=true` | The default behaviour |
| JSON body with a base64 `template` | A multipart form with a `template` file |
| `data` | `data`, sent as `-F "data=<file.json"` |
| `complement` | `ctx` |
| `convertTo: "pdf"` | `steps=convert` and `to=pdf` |
| `reportName` | `outputFilename`, with no tags in it |
| `batchSplitBy` | `splitBy` on [`/v1/render/batch`](/docs/templates/batch/) |
| `batchReportName` | `batchName` |
| `batchOutput: "pdf"` | None. Batches come back as a ZIP |
| `carbone-webhook-url` | [`X-Rusticdoc-Webhook-Url`](/docs/guides/webhooks/) |
| `enum`, `currencySource`, `currencyTarget`, `currencyRates`, `hardRefresh` | None |

## Rewrite a template

1. List the tags in your template, and rewrite each with the tables above. Keep the template's layout and formatting. A tag split across runs by Word still works.
2. Check the result against sample data. `/v1/templates/validate` lists every problem, with where it sits:

   ```bash
   curl -F template=@invoice.docx -F "data=<sample.json" \
     http://localhost:9027/v1/templates/validate | jq .
   ```

3. Render it and compare against a Carbone output. Generated Office files have been opened in LibreOffice, not in Microsoft Office, as [known limits](/docs/project/known-limits/) says.
