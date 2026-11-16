---
title: Template syntax cheat sheet
description: "Template tags on one page: values, loops, conditions, money and date filters, calculations, missing values and rich content, each linked to its full page."
sidebar:
  order: 1.1
---

Every template, whether it's a Word, Excel, PowerPoint, text or PDF file, uses the same tags. This page lists the ones you'll reach for most, and each row links to the page that explains it in full.

## Tags

| Write | What it does | More |
|---|---|---|
| `{{ customer.name }}` | Prints a value | [Tags](/docs/templates/syntax/#tags) |
| `{% if vip %}` | Runs a statement: `for`, `if`, `set`, `with`, `macro` | [Tags](/docs/templates/syntax/#tags) |
| `{# checked by finance #}` | A comment, left out of the output | [Tags](/docs/templates/syntax/#tags) |
| `{{ "{{" }}` | Prints literal braces | [Literal braces](/docs/templates/syntax/#printing-literal-braces) |
| `{%-`, `-%}` | Removes the whitespace on that side, in text templates | [Whitespace](/docs/templates/syntax/#whitespace) |

## Values

| Write | What it does | More |
|---|---|---|
| `customer.name`, `items[0].name`, `items[-1]` | Looks up a key or a position | [Expressions](/docs/templates/syntax/#expressions) |
| `"INV-" ~ number` | Joins text | [Expressions](/docs/templates/syntax/#expressions) |
| `"Paid" if paid else "Due"` | Chooses between two values | [Inline conditions](/docs/templates/conditions/#inline-conditions) |
| `nickname or name` | The first value that's true | [Inline conditions](/docs/templates/conditions/#inline-conditions) |
| `ctx.region` | Reads the request's `ctx` field | [What a template can see](/docs/templates/syntax/#what-a-template-can-see) |

## Missing values

A missing value stops the render with a `422`. These let a value be absent:

| Write | What it does | More |
|---|---|---|
| `{{ phone \| default("none") }}` | Replaces a missing value | [Missing values](/docs/templates/syntax/#missing-values-are-errors) |
| `{{ phone \| default("none", true) }}` | Also replaces `null`, `""`, `0`, `false` and empty lists | [Null isn't missing](/docs/templates/syntax/#null-isnt-missing) |
| `{% if vip is defined %}` | Tests whether a value exists | [Tests](/docs/templates/conditions/#tests) |
| `{% if notes is not empty %}` | False for missing, `null`, `""`, `[]` and `{}`, true for `0` | [empty](/docs/templates/conditions/#empty) |
| `undefinedAsEmpty=true` | A request field that prints every missing value as nothing | [Print missing values as nothing](/docs/templates/syntax/#print-every-missing-value-as-nothing) |

## Loops

| Write | What it does | More |
|---|---|---|
| `{% for i in items %}…{% endfor %}` | Repeats once per item | [Loops](/docs/templates/syntax/#loops) |
| `{% for i in items if i.qty %}` | Skips items that fail the condition | [Loops](/docs/templates/syntax/#loops) |
| `{% for … %}…{% else %}None{% endfor %}` | Prints the `else` part for an empty list | [Loops](/docs/templates/syntax/#loops) |
| `loop.index`, `loop.index0`, `loop.first`, `loop.last`, `loop.length` | The position in the loop | [Loops](/docs/templates/syntax/#loops) |
| `loop.previtem`, `loop.nextitem` | The neighbouring items | [Running totals](/docs/templates/calculations/#running-totals) |

## Conditions

| Write | What it does | More |
|---|---|---|
| `{% if a %}…{% elif b %}…{% else %}…{% endif %}` | The first true branch wins | [Several cases](/docs/templates/conditions/#choose-between-several-cases) |
| `a and b`, `a or b`, `not a`, `0 < qty <= 10` | Combines conditions | [Combine conditions](/docs/templates/conditions/#combine-conditions) |
| `"EU" in regions`, `country not in ["KE", "UG"]` | Membership in a list, text or object | [Membership](/docs/templates/conditions/#membership) |
| `qty is odd`, `v is number`, `name is startingwith("A")` | Asks a question about a value | [Tests](/docs/templates/conditions/#tests) |
| `{{ {"paid": "Paid"}[status] \| default("Unknown") }}` | Maps a code to a label | [Several cases](/docs/templates/conditions/#choose-between-several-cases) |

`false`, `null`, `0`, `""`, `[]` and `{}` count as false. See [what counts as true](/docs/templates/conditions/#what-counts-as-true).

## Money, numbers, dates and labels

| Write | Prints | More |
|---|---|---|
| `{{ 1234.5 \| currency("USD") }}` | `USD 1,234.50` | [currency](/docs/templates/filters/#currency) |
| `{{ 1234.5 \| number(2) }}` | `1,234.50` | [number](/docs/templates/filters/#number) |
| `{{ "2026-11-06" \| date("%d %B %Y") }}` | `06 November 2026` | [date](/docs/templates/filters/#date) |
| `{{ 1250.5 \| words(currency="EUR") }}` | `one thousand two hundred fifty euros and fifty cents` | [words](/docs/templates/filters/#words) |
| `{{ "total" \| t }}` | The label translated into the request's language | [t](/docs/templates/filters/#t) |
| `lang=fr`, `timezone`, `currency` | Request fields that set how these print | [Translations and settings](/docs/templates/translations/) |

## Text

| Write | Prints | More |
|---|---|---|
| `upper`, `lower`, `title`, `capitalize`, `trim` | Changes case or trims | [Text filters](/docs/templates/filters/#text) |
| `{{ "a bc" \| replace("a", "b") }}` | `b bc` | [Text filters](/docs/templates/filters/#text) |
| `{{ title \| truncate(40, words=true) }}` | At most 40 characters, cut at a space | [truncate](/docs/templates/filters/#truncate) |
| `{{ name \| unaccent }}` | `creme brulee` for `crème brûlée` | [unaccent](/docs/templates/filters/#unaccent) |
| `{{ 42 \| pad_left(6, "0") }}` | `000042` | [pad_left](/docs/templates/filters/#pad_left-and-pad_right) |

## Lists and totals

| Write | What it does | More |
|---|---|---|
| `items \| length` | Counts the items | [Lists](/docs/templates/filters/#lists) |
| `items \| sum(attribute="price")` | Adds up one attribute | [Totals](/docs/templates/calculations/#totals-averages-minimum-and-maximum) |
| `items \| avg(attribute="price")` | The mean, or `null` for an empty list | [avg](/docs/templates/filters/#avg) |
| `items \| map(attribute="name") \| join(", ")` | Lists one attribute | [Lists of objects](/docs/templates/filters/#lists-of-objects) |
| `items \| selectattr("qty", "gt", 1) \| list` | Keeps the items that pass a test | [Counts](/docs/templates/calculations/#counts) |
| `items \| sort(attribute="price", reverse=true)` | Sorts | [Sorting](/docs/templates/calculations/#sorting) |
| `{% for g in items \| groupby("group") %}` | Groups, with `g.grouper` and `g.list` | [Per-group totals](/docs/templates/calculations/#filtered-and-per-group-totals) |
| `customers \| selectattr("id", "eq", o.customer) \| first` | Finds a matching item in another list | [Lookups](/docs/templates/calculations/#lookups-and-joins-between-lists) |

## Calculations

| Write | What it does | More |
|---|---|---|
| `+`, `-`, `*`, `/`, `//`, `%`, `**` | Arithmetic | [Arithmetic](/docs/templates/calculations/#arithmetic) |
| `{{ (qty * price) \| currency("EUR") }}` | Brackets first, because a filter binds tighter than `*` | [Filters bind tighter](/docs/templates/syntax/#a-filter-binds-tighter-than-arithmetic) |
| `round(2)`, `round(2, "ceil")`, `ceil`, `floor`, `abs` | Rounds | [Rounding](/docs/templates/calculations/#rounding) |
| `{% set net = qty * price %}` | Names a value | [set and namespace](/docs/templates/calculations/#store-values-with-set-and-namespace) |
| `{% set ns = namespace(total=0) %}`, then `{% set ns.total = ns.total + x %}` | Carries a value out of a loop | [set and namespace](/docs/templates/calculations/#store-values-with-set-and-namespace) |

## Dates

| Write | What it does | More |
|---|---|---|
| `issued \| date_add(30)`, `date_add(1, "month")` | Adds days or other units | [Dates](/docs/templates/calculations/#dates) |
| `date_sub`, `start_of("month")`, `end_of("quarter")` | Moves back, or to the edge of a period | [Date helpers](/docs/templates/filters/#date-helpers) |
| `due \| date_diff(issued, "days")` | Whole units between two dates | [Dates](/docs/templates/calculations/#dates) |
| `now()` | The time of the render. Pin it with `ctx={"now":"…"}` | [Today's date](/docs/templates/calculations/#todays-date) |
| `ms \| duration`, `duration("hours")` | A length of time in words or a unit | [Durations](/docs/templates/calculations/#durations) |

## Office documents

Where you put a block's opening and closing tags decides what repeats or disappears:

| Tags sit | What repeats | More |
|---|---|---|
| In one paragraph | The text between them | [What a block repeats](/docs/templates/office/#what-a-block-repeats) |
| In a table row's first and last cells | That row | [Table rows](/docs/templates/office/#table-rows) |
| In different paragraphs | Those paragraphs | [Paragraphs](/docs/templates/office/#paragraphs) |
| In rows of a sheet | Those rows, with formulas following | [XLSX](/docs/templates/office/#xlsx) |
| On different slides | Those slides | [Slide loops](/docs/templates/office/#slide-loops) |
| `{% sheet for s in sites %}` in a cell | The sheet, named by `{{ s.name }}` | [Repeating a sheet](/docs/templates/office/#repeating-a-sheet) |
| `{% col for d in days %}…{% endcol %}` | Cells to the right, in XLSX | [Repeating columns](/docs/templates/office/#repeating-columns) |
| `{% for p in team \| shift(y="3cm") %}` | Shapes on one slide, each moved down | [Shapes that move](/docs/templates/office/#shapes-that-move-down-or-across-a-slide) |

`{{ drop("row") if hidden }}` removes the element that holds it: a paragraph, row, column, table, picture, chart, shape, slide or sheet. See [remove an element](/docs/templates/office/#remove-an-element).

## Rich content

These work in Office templates only:

| Write | What it prints | More |
|---|---|---|
| `{{ logo \| image(width=120) }}` | An image, from base64 in the data | [image](/docs/templates/rich-content/#image) |
| `{{ photo \| image(fit="cover") }}` in a picture's alt text | Replaces that picture, keeping its frame | [Placeholder pictures](/docs/templates/rich-content/#placeholder-pictures) |
| `{{ url \| barcode("qr", size=90) }}` | A QR code or barcode | [barcode](/docs/templates/rich-content/#barcode) |
| `{{ url \| link("Pay now") }}` | A link | [link](/docs/templates/rich-content/#link) |
| `{{ status \| color("#c00") }}` | Coloured text, or a shaded cell or row with `scope=` | [color](/docs/templates/rich-content/#color) |
| `{{ body \| html }}` | Formatted text from a subset of HTML | [html](/docs/templates/rich-content/#html) |
| `{{ rows \| chart(type="bar", x="month", y=["sales"]) }}` | A native chart | [chart](/docs/templates/rich-content/#chart) |
| `{{ "" \| pagebreak }}` | A page break, in DOCX and ODT | [pagebreak](/docs/templates/rich-content/#pagebreak) |
| `{{ issued \| excel_date }}` | A real date in a spreadsheet cell | [excel_date](/docs/templates/filters/#excel_date) |

## PDF forms and batches

| Write | What it does | More |
|---|---|---|
| A field named `customer.name` | Takes the value at that path in the data | [How values match fields](/docs/templates/pdf-templates/#how-values-match-fields) |
| `{{ name \| fill }}`, `{{ vip \| check }}` in an annotation | Fills the field under it, or draws on the page | [Tags in annotations](/docs/templates/pdf-templates/#tags-in-annotations) |
| `item`, `batch.index`, `batch.count` | The element and its position in a batch render | [What each render sees](/docs/templates/batch/#what-each-render-sees) |

## Try it

This text template uses a loop with a condition, a namespace total, `currency`, `words`, a date calculation and `default`:

```bash test
cat > try.txt <<'EOF'
{% set ns = namespace(total=0) -%}
{% for i in items if i.qty -%}
{{ loop.index }}. {{ i.name | upper }} x{{ i.qty }}
{% set ns.total = ns.total + i.qty * i.price -%}
{% endfor -%}
Total {{ ns.total | currency("EUR") }} ({{ ns.total | words(currency="EUR") }})
Due {{ issued | date_add(30) | date("%d %B %Y") }}
{{ "Paid" if paid else "Unpaid" }}, {{ note | default("no note") }}
EOF
cat > try.json <<'EOF'
{
  "issued": "2026-11-06",
  "paid": false,
  "items": [
    {"name": "Lamp", "qty": 2, "price": 19.5},
    {"name": "Plug", "qty": 0, "price": 1},
    {"name": "Desk", "qty": 1, "price": 120}
  ]
}
EOF
curl -F template=@try.txt -F data=@try.json \
  http://localhost:9027/v1/render -o out.txt
cat > expected.txt <<'EOF'
1. LAMP x2
2. DESK x1
Total EUR 159.00 (one hundred fifty-nine euros)
Due 06 December 2026
Unpaid, no note
EOF
test "$(cat out.txt)" = "$(cat expected.txt)"
```

To find every mistake in a template before you use it, [validate it](/docs/templates/validate/).
