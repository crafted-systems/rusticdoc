# Template syntax

<!-- Generated from site/src/content/docs/docs/templates/syntax.md by site/scripts/generate-skills.mjs. Do not edit. -->

How to write tags, loops, conditions and expressions, what a missing value does, and how each format escapes values.

Office, text and PDF templates all share one syntax for values, conditions and loops, and [minijinja](https://docs.rs/minijinja) evaluates it.

## Fill your first template

`letter.txt` has two tags in it, and `letter.json` holds the data:

```text
Dear {{ customer.name }},
Total: {{ total | currency("KES") }}
```

Send both to `/v1/render`, and add `steps=convert` if you want a PDF:

```bash
curl -F template=@letter.txt -F "data=<letter.json" \
  http://localhost:9027/v1/render -o filled.txt
grep -x 'Dear Ada,' filled.txt

curl -F template=@letter.txt -F "data=<letter.json" -F steps=convert \
  http://localhost:9027/v1/render -o letter.pdf
test "$(head -c 4 letter.pdf)" = "%PDF"
```

`-F "data=<letter.json"` sends the file's contents as a text field. Uploading it as a file with `-F data=@letter.json` does the same thing, whatever the file is called and wherever the part sits in the form. If a part isn't valid JSON, you get a `400`.

Two things about the form can still catch you out. The template is the first file part left once `data` and `ctx` are set aside, whatever its field name. And an Office template sent with no data comes back unchanged, because when `data` is absent or `{}` the service doesn't read any tags in a DOCX, ODT, XLSX, ODS, PPTX or ODP, so it doesn't report any errors either. If a template needs no values, send any non-empty object, like `data={"_":1}`. Text templates are always rendered.

## What a template can see

| Name | Comes from |
|---|---|
| Every key of `data` | The `data` field, which must be a JSON object. Anything else gets a `400` |
| `ctx` | The `ctx` field, also a JSON object. If `data` has a `ctx` key of its own, that one wins |
| `item`, `batch` | Added by [batch rendering](batch.md) |

## Tags

| Tag | What it does | Example |
|---|---|---|
| `{{ ... }}` | Prints an expression | `{{ customer.name }}` |
| `{% ... %}` | Runs a statement | `{% if vip %}` |
| `{# ... #}` | A comment, left out of the output | `{# checked by finance #}` |

Statements include `for`, `if`, `set`, `with` and `macro`. A template is a single file, so `include` and `extends` fail with `template not found`. Office templates also have [`{% settings %}`](translations.md).

## Loops

`{% for %}` repeats whatever sits between it and `{% endfor %}`, and inside the loop `loop` gives you the position. An `if` on the `for` line filters the list, and `{% else %}` runs when the list is empty.

```bash
cat > items.txt <<'EOF'
{% for i in items if i.qty -%}
  {{ loop.index }}/{{ loop.length }} {{ i.name }}
  {%- if not loop.last %}, {% endif %}
{%- else -%}
  Nothing
{%- endfor %}
EOF
cat > items.json <<'EOF'
{
  "items": [
    {"name": "Lamp", "qty": 2},
    {"name": "Plug", "qty": 0},
    {"name": "Desk", "qty": 1}
  ]
}
EOF
curl -F template=@items.txt -F data=@items.json \
  http://localhost:9027/v1/render -o out.txt
grep -x '1/2 Lamp, 2/2 Desk' out.txt
```

The names on `loop` include `index` (from 1), `index0` (from 0), `first`, `last` and `length`.

In an Office template, what repeats (text, a table row, paragraphs, sheet rows or slides) depends on where you put the opening and closing tags. See [Office formats](office.md).

## Conditions

```text
{% if total > 1000 %}Large{% elif total > 0 %}Small{% else %}None{% endif %}
```

An empty list, an empty object, an empty string, `0`, `false` and `null` all count as false.

## Expressions

| Kind | How you write it |
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

`a * b | currency("EUR")` applies the filter to `b` first and then multiplies a string by a number, which repeats the string. Put the arithmetic in brackets:

```bash
cat > total.txt <<'EOF'
{{ a * b | currency("EUR") }}
{{ (a * b) | currency("EUR") }}
EOF
curl -F template=@total.txt -F 'data={"a":2,"b":3.5}' \
  http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = "$(printf 'EUR 3.50EUR 3.50\nEUR 7.00')"
```

Nothing flags the first line as a mistake, so check the totals the first time you render a template.

## Missing values are errors

An undefined value stops the render. A missing variable never prints as nothing, so a typo can't reach a customer as a blank.

```bash
printf 'Hello {{ nobody }}\n' > missing.txt
curl -F template=@missing.txt -F 'data={"somebody":"Ada"}' \
  http://localhost:9027/v1/render
```

You get a `422`:

```json
{
  "error": "template_error",
  "message": "template error: undefined value (in <string>:1) (line 1)"
}
```

For an Office template, the message names the place instead of a line:

```text
template error: undefined value (word/document.xml paragraph 2: {{ nobody }})
```

All of these fail the same way when `a` is missing:

- `{% if a %}`, and `{% if a.b %}` when `a` has no `b`.
- `{% for i in a %}`.
- A filter that needs a value, like `{{ a | upper }}` or `{{ a | currency }}`.

There are three ways to let a value be absent:

```bash
cat > optional.txt <<'EOF'
{{ nickname | default("friend") }}
{{ customer.phone | default("no phone") }}
{% if vip is defined %}VIP{% else %}Regular{% endif %}
EOF
curl -F template=@optional.txt -F 'data={"customer":{"name":"Ada"}}' \
  http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = "$(printf 'friend\nno phone\nRegular')"
```

`default` only covers one missing level, so `{{ customer.phone.area | default("x") }}` still fails when `customer.phone` is missing.

### Print every missing value as nothing

If your data is sparse by design, send `undefinedAsEmpty=true` to `/v1/render` or `/v1/render/batch`. For that request, every missing value prints as nothing, and so does every missing field of one. `date`, `currency`, `round` and `abs` print nothing for a missing value too, and a spreadsheet cell that holds only a missing value is left empty.

```bash
printf 'Dear {{ customer.name }}{{ customer.title }}.' > sparse.txt
curl -F template=@sparse.txt -F 'data={"customer":{"name":"Ada"}}' \
  -F undefinedAsEmpty=true http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = "Dear Ada."
```

It hides typos as well, so only use it where you expect blanks, and run [validation](validate.md) without it.

### Null isn't missing

A key whose value is `null` is defined, so it doesn't raise an error and `default("x")` doesn't replace it. Use `default("x", true)` instead, which also replaces `""`, `0`, `false` and empty lists.

A `null` prints as nothing in an Office template and as `None` in a text template. `true` and `false` print as `True` and `False` in both.

## Escaping

Each format escapes values in its own way, so data can't add markup or keys of its own.

| Template | What happens to a value |
|---|---|
| DOCX, ODT, XLSX, ODS, PPTX, ODP | `&`, `<` and `>` are escaped for XML, and characters XML forbids are dropped. A value can't break the file |
| `.html`, `.htm`, `.xml`, `.svg` | `&`, `<`, `>`, `"`, `'` and `/` become character references, so a value is safe as text and in a quoted attribute |
| `.json` | A value is written as JSON, with its own quotes: `{{ name }}` gives `"Ada"`, and `{{ n }}` gives `5` |
| `.txt`, `.md`, `.csv` | Nothing is escaped |

`{{ value | safe }}` writes a value as it is in any of these, so keep it away from data you didn't write. In an Office template it skips the XML escaping and writes the value into the document's XML, so a value with a `<` in it produces a broken file. Keep it away from data there.

```bash
printf '<p>{{ name }}</p><p>{{ name | safe }}</p>' > page.html
curl -F template=@page.html -F 'data={"name":"<b>&"}' \
  http://localhost:9027/v1/render -o page.out
test "$(cat page.out)" = '<p>&lt;b&gt;&amp;</p><p><b>&</p>'

printf '{"name":{{ name }},"count":{{ count }}}' > out-template.json
curl -F template=@out-template.json \
  -F 'data={"name":"say \"hi\"","count":5}' \
  http://localhost:9027/v1/render -o json.out
jq -e '.name == "say \"hi\"" and .count == 5' json.out
```

In a `.json` template, don't put quotes around a tag. `"{{ name }}"` would come out with two sets of quotes, so the service refuses any `.json` template that doesn't render to valid JSON:

```bash
printf '{"name":"{{ name }}"}' > quoted.json
curl -F template=@quoted.json -F 'data={"name":"Ada"}' \
  http://localhost:9027/v1/render -o error.json || true
jq -e '.error == "template_error"
  and (.message | contains("remove any quotes"))' error.json
```

> **Where escaping doesn't help**
>
> Escaping makes a value safe as HTML or XML text. It doesn't make it safe inside a `<script>` or `<style>` block, in an unquoted attribute, or as a URL in `href` or `src`, where `javascript:` needs no special characters. Check values like these in your application first, and in a `<script>` block use `{{ value | tojson }}`.

A newline in a value becomes a line break in DOCX and ODT, and stays a newline character in the other formats.

### Printing literal braces

To print `{{` in any template, write it as a string, `{{ "{{" }}`. A single `{` is fine on its own.

`{% raw %}...{% endraw %}` works in text templates. It isn't supported in Office templates, where it leaves stray `{#` and `#}` marks in the output.

### Whitespace

In a text template, a line that holds only a statement leaves its line break behind. Write `{%-` or `-%}` to remove the whitespace on that side. Office templates accept these marks and ignore them.

## Text formats

A file with one of these extensions is rendered whole, as UTF-8 text. Office files and [PDF templates](pdf-templates.md) have their own rules, and any other extension gets a `501`.

| Extension | Response without `steps` | With `steps=convert` |
|---|---|---|
| `.txt` | `text/plain` | PDF |
| `.md` | `text/markdown` | PDF |
| `.html`, `.htm` | `text/html` | PDF |
| `.csv` | `text/csv` | PDF |
| `.xml` | `application/xml` | PDF |
| `.json` | `application/json` | PDF |
| `.svg` | `image/svg+xml` | PDF |

> **Text templates have no rich content**
>
> Text templates have `currency`, `number`, `date`, `t`, `{% settings %}`, the [calculation helpers](calculations.md) and the built-in filters. The rich content filters only work in Office templates, and fail with a clear error in a text template. See [where each filter works](filters.md#where-each-filter-works).

## Limits

| Limit | Value | If you go over |
|---|---|---|
| Evaluation steps in one render | 5,000,000 | `422`, `engine ran out of fuel` |
| Text template output | `RUSTICDOC_MAX_RENDER_BYTES`, default 64 MB | `400` |
| Office package once rendered | 256 MiB | `422` |

[Errors](https://rusticdoc.com/docs/reference/errors/) lists the error shapes, and [Validate a template](validate.md) shows how to find every problem in a template before you use it.
