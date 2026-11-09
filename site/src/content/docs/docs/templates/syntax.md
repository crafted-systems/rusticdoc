---
title: Template syntax
description: Write tags, loops, conditions and expressions, and know what a missing value does.
sidebar:
  order: 1
---

This page shows how to put values, conditions and loops in a template, how expressions evaluate, what happens when a value is missing, and which characters get escaped. The syntax is the same in Office, text and PDF templates, and is evaluated by [minijinja](https://docs.rs/minijinja).

## Fill your first template

`letter.txt` holds two tags, and `letter.json` holds the data:

```text
Dear {{ customer.name }},
Total: {{ total | currency("KES") }}
```

Send both to `/v1/render`. Adding `steps=convert` gives a PDF:

```bash test
curl -F template=@letter.txt -F "data=<letter.json" http://localhost:9027/v1/render -o filled.txt
grep -x 'Dear Ada,' filled.txt

curl -F template=@letter.txt -F "data=<letter.json" -F steps=convert \
  http://localhost:9027/v1/render -o letter.pdf
test "$(head -c 4 letter.pdf)" = "%PDF"
```

`-F "data=<letter.json"` sends the file's contents as a text field. `-F data=@letter.json` uploads it as a file and does the same thing, with any file name and in any position. A part that is not valid JSON is a `400`.

Two behaviours of the form can still surprise you:

- **The first file part left is the template.** Any file part counts, whatever its field name, once `data` and `ctx` are set aside.
- **An Office template with no data is returned as it is.** With `data` absent or `{}`, the service reads no tags in a DOCX, ODT, XLSX, ODS, PPTX or ODP, so it reports no errors either. If a template needs no values, send any non-empty object, such as `data={"_":1}`. Text templates are always rendered.

## What a template can see

| Name | Comes from |
|---|---|
| Every key of `data` | The `data` field, which must be a JSON object. Anything else is a `400` |
| `ctx` | The `ctx` field, also a JSON object. A `ctx` key inside `data` wins over it |
| `item`, `batch` | Added by [batch rendering](/docs/templates/batch/) |

## Tags

| Tag | Does | Example |
|---|---|---|
| `{{ ... }}` | Prints an expression | `{{ customer.name }}` |
| `{% ... %}` | Runs a statement | `{% if vip %}` |
| `{# ... #}` | A comment, left out of the output | `{# checked by finance #}` |

Statements include `for`, `if`, `set`, `with` and `macro`. A template is one file, so `include` and `extends` fail with `template not found`. Office templates also have [`{% settings %}`](/docs/templates/translations/).

## Loops

`{% for %}` repeats what sits between it and `{% endfor %}`. Inside a loop, `loop` gives the position. `{% else %}` runs when the list is empty, and an `if` on the `for` line filters the list.

```bash test
cat > items.txt <<'EOF'
{% for i in items if i.qty %}{{ loop.index }}/{{ loop.length }} {{ i.name }}{% if not loop.last %}, {% endif %}{% else %}Nothing{% endfor %}
EOF
curl -F template=@items.txt \
  -F 'data={"items":[{"name":"Lamp","qty":2},{"name":"Plug","qty":0},{"name":"Desk","qty":1}]}' \
  http://localhost:9027/v1/render -o out.txt
grep -x '1/2 Lamp, 2/2 Desk' out.txt
```

The names on `loop` include `index` (from 1), `index0` (from 0), `first`, `last` and `length`.

In an Office template, where the opening and closing tags sit decides what repeats: text, a table row, paragraphs, sheet rows or slides. See [Office formats](/docs/templates/office/).

## Conditions

```text
{% if total > 1000 %}Large{% elif total > 0 %}Small{% else %}None{% endif %}
```

An empty list, an empty object, an empty string, `0`, `false` and `null` count as false.

## Expressions

| Kind | Written |
|---|---|
| Lookups | `customer.name`, `items[0].name`, `items[1:]` |
| Literals | `"text"`, `12`, `1.5`, `true`, `[1, 2]`, `{"k": 1}` |
| Arithmetic | `+`, `-`, `*`, `/`, `//` (whole division), `%`, `**` |
| Joining text | `"INV-" ~ number` |
| Comparison | `==`, `!=`, `<`, `<=`, `>`, `>=` |
| Logic | `and`, `or`, `not`, `in`, `is` |
| Inline choice | `"yes" if vip else "no"` |
| Filters | `name \| upper`, `total \| currency("EUR")` |
| Assignment | `{% set net = qty * price %}` |

### A filter binds tighter than arithmetic

`a * b | currency("EUR")` applies the filter to `b` first, and then multiplies a string by a number, which repeats the string. Put the arithmetic in brackets:

```bash test
printf '{{ a * b | currency("EUR") }}\n{{ (a * b) | currency("EUR") }}\n' > total.txt
curl -F template=@total.txt -F 'data={"a":2,"b":3.5}' http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = "$(printf 'EUR 3.50EUR 3.50\nEUR 7.00')"
```

Nothing reports the first line as a mistake, so check totals in your first render.

## Missing values are errors

Undefined values stop the render. A missing variable never prints as nothing, so a typo cannot reach a customer as a blank.

```bash test=fail
printf 'Hello {{ nobody }}\n' > missing.txt
curl -F template=@missing.txt -F 'data={"somebody":"Ada"}' http://localhost:9027/v1/render
```

The answer is a `422`:

```json
{"error":"template_error","message":"template error: undefined value (in <string>:1) (line 1)"}
```

An Office template names the place instead of a line:

```text
template error: undefined value (word/document.xml paragraph 2: {{ nobody }})
```

The same goes for these, all of which fail on a missing `a`:

- `{% if a %}`, and `{% if a.b %}` when `a` has no `b`.
- `{% for i in a %}`.
- A filter that needs a value, such as `{{ a | upper }}` or `{{ a | currency }}`.

Three ways allow a value to be absent:

```bash test
cat > optional.txt <<'EOF'
{{ nickname | default("friend") }}
{{ customer.phone | default("no phone") }}
{% if vip is defined %}VIP{% else %}Regular{% endif %}
EOF
curl -F template=@optional.txt -F 'data={"customer":{"name":"Ada"}}' \
  http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = "$(printf 'friend\nno phone\nRegular')"
```

`default` covers one missing level. `{{ customer.phone.area | default("x") }}` still fails when `customer.phone` is missing.

### Print every missing value as nothing

When your data is sparse by design, `undefinedAsEmpty=true` on `/v1/render` or `/v1/render/batch` prints every missing value, and every missing field of one, as nothing for that request. `date`, `currency`, `round` and `abs` print nothing for a missing value too, and a spreadsheet cell holding only one is left empty.

```bash test
printf 'Dear {{ customer.name }}{{ customer.title }}.' > sparse.txt
curl -F template=@sparse.txt -F 'data={"customer":{"name":"Ada"}}' \
  -F undefinedAsEmpty=true http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = "Dear Ada."
```

This hides typos as well, so use it only where blanks are expected, and run [validation](/docs/templates/validate/) without it.

### Null is not missing

A key whose value is `null` is defined, so it does not raise an error and `default("x")` does not replace it. Use `default("x", true)`, which also replaces `""`, `0`, `false` and empty lists.

A `null` prints as nothing in an Office template and as `None` in a text template. `true` and `false` print as `True` and `False` in both.

## Escaping

Each format escapes values in its own way, so data cannot add markup or keys of its own.

| Template | What happens to a value |
|---|---|
| DOCX, ODT, XLSX, ODS, PPTX, ODP | `&`, `<` and `>` are escaped for XML, and characters XML forbids are dropped. A value cannot break the file |
| `.html`, `.htm`, `.xml`, `.svg` | `&`, `<`, `>`, `"`, `'` and `/` become character references, so a value is safe as text and in a quoted attribute |
| `.json` | A value is written as JSON, with its own quotes: `{{ name }}` gives `"Ada"`, and `{{ n }}` gives `5` |
| `.txt`, `.md`, `.csv` | Nothing is escaped |

`{{ value | safe }}` writes a value as it is, in any of these. Keep it away from data you did not write.

```bash test
printf '<p>{{ name }}</p><p>{{ name | safe }}</p>' > page.html
curl -F template=@page.html -F 'data={"name":"<b>&"}' http://localhost:9027/v1/render -o page.out
test "$(cat page.out)" = '<p>&lt;b&gt;&amp;</p><p><b>&</p>'

printf '{"name":{{ name }},"count":{{ count }}}' > out-template.json
curl -F template=@out-template.json -F 'data={"name":"say \"hi\"","count":5}' http://localhost:9027/v1/render -o json.out
jq -e '.name == "say \"hi\"" and .count == 5' json.out
```

In a `.json` template, do not put quotes around a tag. `"{{ name }}"` would come out with two sets of quotes, so the service refuses any `.json` template that does not render to valid JSON:

```bash test
printf '{"name":"{{ name }}"}' > quoted.json
curl -F template=@quoted.json -F 'data={"name":"Ada"}' \
  http://localhost:9027/v1/render -o error.json || true
jq -e '.error == "template_error" and (.message | contains("remove any quotes"))' error.json
```

:::caution[Where escaping does not help]
Escaping makes a value safe as HTML or XML text. It does not make a value safe inside a `<script>` or `<style>` block, in an unquoted attribute, or as a URL in `href` or `src`, where `javascript:` needs no special characters. Check such values in your application first. In a `<script>` block, use `{{ value | tojson }}`.
:::

In an Office template, `{{ value | safe }}` skips the XML escaping and writes the value into the document's XML. A value holding `<` then produces a broken file, so keep it away from data.

A newline in a value becomes a line break in DOCX and ODT. In the other formats it stays a newline character.

### Printing literal braces

To print `{{` in any template, write it as a string: `{{ "{{" }}`. A single `{` needs no care.

`{% raw %}...{% endraw %}` works in text templates. It is not supported in Office templates, where it leaves stray `{#` and `#}` marks in the output.

### Whitespace

In a text template, a line holding only a statement leaves its line break behind. Write `{%-` or `-%}` to remove the whitespace on that side. Office templates accept these marks and ignore them.

## Text formats

A file with one of these extensions is rendered whole, as UTF-8 text. Office files and [PDF templates](/docs/templates/pdf-templates/) have their own rules, and any other extension gets `501`.

| Extension | Response without `steps` | With `steps=convert` |
|---|---|---|
| `.txt` | `text/plain` | PDF |
| `.md` | `text/markdown` | PDF |
| `.html`, `.htm` | `text/html` | PDF |
| `.csv` | `text/csv` | PDF |
| `.xml` | `application/xml` | PDF |
| `.json` | `application/json` | PDF |
| `.svg` | `image/svg+xml` | PDF |

:::caution[Text templates have no rich content]
Text templates have `currency`, `number`, `date`, `t`, `{% settings %}` and the built-in filters. The rich content filters work only in Office templates, and in a text template they fail with a clear error. See [where each filter works](/docs/templates/filters/#where-each-filter-works).
:::

## Limits

| Limit | Value | Past it |
|---|---|---|
| Evaluation steps in one render | 5,000,000 | `422`, `engine ran out of fuel` |
| Text template output | `RUSTICDOC_MAX_RENDER_BYTES`, default 64 MB | `400` |
| Office package once rendered | 256 MiB | `422` |

[Errors](/docs/reference/errors/) lists the error shapes. To find every problem in a template before using it, see [Validate a template](/docs/templates/validate/).
