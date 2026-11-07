---
title: Filters
description: Format money and dates, translate labels, and transform values with every filter a template can use.
sidebar:
  order: 2
---

This page lists every filter, so you can format money and dates, translate labels, and sum or sort lists from inside a template. A filter follows a value with `|` and may take arguments: `{{ total | currency("KES") }}`. Filters chain from left to right.

## Where each filter works

| Filter | Office templates | Text templates | PDF tags and `batchName` |
|---|---|---|---|
| `currency` | Yes | Yes | Yes |
| `date`, `t` | Yes | No | No |
| `image`, `barcode`, `link`, `color`, `html`, `chart`, `pagebreak` | Yes | No | No |
| [Built-in filters](#built-in-filters) | Yes | Yes | Yes |

Using a filter where it does not exist is a `422`:

```bash test=fail
printf '{{ issued | date("%%d/%%m/%%Y") }}\n' > dated.txt
curl -F template=@dated.txt -F 'data={"issued":"2026-11-06"}' http://localhost:9027/v1/render
```

```json
{"error":"template_error","message":"template error: unknown filter: filter date is unknown (in <string>:1) (line 1)"}
```

To print a date in a text template, format it before sending it, or use an Office template. The [rich content filters](/docs/templates/rich-content/) have their own page.

## currency

```text
{{ value | currency }}
{{ value | currency("KES") }}
```

Prints a number with two decimals, commas between thousands, and the code in front.

| Value | Tag | Prints |
|---|---|---|
| `1234.5` | `currency("USD")` | `USD 1,234.50` |
| `-1234567.891` | `currency("USD")` | `-USD 1,234,567.89` |
| `999.999` | `currency("EUR")` | `EUR 1,000.00` |
| `0.5` | `currency` | `0.50` |
| `2` | `currency("")` | `2.00` |

```bash test
printf '{{ a | currency("USD") }}|{{ b | currency }}|{{ c | currency("EUR") }}' > money.txt
curl -F template=@money.txt -F 'data={"a":1234.5,"b":0.5,"c":999.999}' \
  http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = 'USD 1,234.50|0.50|EUR 1,000.00'
```

- The code is any text you pass, and is not checked. There are no currency symbols or locale formats, so the separators are always `,` and `.`.
- The code can come from data, as in `{{ total | currency(currency) }}`.
- In an Office template with no argument, the [`currency` setting](/docs/templates/translations/) applies. `currency("")` prints no code even when a setting exists.
- The value must be a number. A string such as `"12"` is an error: `cannot convert string to f64`. So is `null`.

## date

Office templates only.

```text
{{ issued | date }}
{{ issued | date("%d %B %Y") }}
```

The one argument is a format. Without it the format is `%Y-%m-%d`. A second argument is an error (`too many arguments`): the time zone comes from the [`timezone` setting](/docs/templates/translations/#timezone).

### What it accepts

| Value | Treated as |
|---|---|
| `2026-11-06` | That date, at midnight |
| `2026-11-06T10:20` | Local time, with no conversion |
| `2026-11-06T10:20:30`, `2026-11-06 10:20:30` | Local time, with no conversion. Fractions of a second are allowed |
| `2026-11-06T10:20:30Z`, `2026-11-06T10:20:30+03:00` | A moment, shown in the time zone |
| `2026-11-06 10:20:30+0300` | A moment, shown in the time zone |
| `1793980800`, `1793980800.5` | Unix seconds, shown in the time zone |

Anything else, such as `06/11/2026`, is refused: `date: cannot parse "06/11/2026"; use ISO 8601 or unix seconds`.

### Format strings

Formats follow [chrono's `strftime` syntax](https://docs.rs/chrono/latest/chrono/format/strftime/index.html). An unknown specifier is an error: `date: invalid format "%Q"`.

| Specifier | Prints, for 6 November 2026 at 10:20:30 |
|---|---|
| `%Y`, `%y` | `2026`, `26` |
| `%m`, `%d` | `11`, `06` |
| `%B`, `%b` | `November`, `Nov` |
| `%A`, `%a` | `Friday`, `Fri` |
| `%H`, `%M`, `%S` | `10`, `20`, `30` |
| `%j` | `310`, the day of the year |

Month and day names are always in English, whatever `lang` says. To print them in another language, send the text in your data.

### Time zones

The time zone is a fixed offset such as `+02:00` or `-05:00`, and the default is UTC. It changes only values that are a moment: those with a zone, and unix seconds. A value with no zone prints as written.

Named zones such as `Europe/Paris` are refused, because a fixed offset cannot follow daylight saving. If you need that, work out the offset before the request and format the date yourself.

The sample invoice uses `date` in its issue and payment lines:

```bash test
curl -F template=@invoice.docx -F "data=<invoice.json" http://localhost:9027/v1/render -o invoice-filled.docx
unzip -p invoice-filled.docx word/document.xml | grep -o 'Issued 06 November 2026'
```

## t

Office templates only. It takes no arguments.

```text
{{ "greeting" | t }}
```

It looks up the key in the template's translations, and tries `greeting.<lang>`, then `greeting`, then prints the key. See [Translations and settings](/docs/templates/translations/).

## Built-in filters

These come from minijinja and work in every template. Each example here ran as written.

### Text

| Filter | Example | Prints |
|---|---|---|
| `upper`, `lower` | `{{ "ab" \| upper }}` | `AB` |
| `title` | `{{ "a bc" \| title }}` | `A Bc` |
| `capitalize` | `{{ "a bc" \| capitalize }}` | `A bc` |
| `trim` | `{{ "--x-" \| trim("-") }}` | `x`. With no argument it trims whitespace |
| `replace` | `{{ "a bc" \| replace("a", "b") }}` | `b bc` |
| `split` | `{{ "a bc" \| split(" ") \| join("+") }}` | `a+bc` |
| `lines` | `{{ text \| lines \| length }}` | The number of lines |
| `indent` | `{{ text \| indent(2) }}` | Indents every line after the first |
| `format` | `{{ "%s-%s" \| format(1, 2) }}` | `1-2` |

### Lists

| Filter | Example | Prints, for `l` = `[1,2,2,3]` |
|---|---|---|
| `length`, `count` | `{{ l \| length }}` | `4`. Works on text and objects too |
| `first`, `last` | `{{ l \| last }}` | `3` |
| `min`, `max` | `{{ l \| max }}` | `3` |
| `sum` | `{{ l \| sum }}` | `8` |
| `join` | `{{ l \| join(",") }}` | `1,2,2,3` |
| `sort` | `{{ l \| sort(reverse=true) }}` | `[3, 2, 2, 1]`. Also takes `attribute="price"` |
| `reverse` | `{{ l \| reverse \| list }}` | `[3, 2, 2, 1]` |
| `unique` | `{{ l \| unique \| list }}` | `[1, 2, 3]` |
| `list` | `{{ "ab" \| list }}` | `['a', 'b']` |
| `batch` | `{{ [1,2,3] \| batch(2) \| list }}` | `[[1, 2], [3]]` |
| `slice` | `{{ [1,2,3] \| slice(2) \| list }}` | `[[1, 2], [3]]` |
| `chain` | `{{ [1] \| chain([2]) \| list }}` | `[1, 2]` |
| `zip` | `{{ [1] \| zip(["a"]) \| list }}` | `[(1, 'a')]` |

### Lists of objects

These are the ones to use for totals. For `o` = `[{"a":1},{"a":0}]`:

| Filter | Example | Prints |
|---|---|---|
| `sum` | `{{ o \| sum(attribute="a") }}` | `1` |
| `map` | `{{ o \| map(attribute="a") \| list }}` | `[1, 0]` |
| `selectattr` | `{{ o \| selectattr("a") \| list \| length }}` | `1`. Also takes a test: `selectattr("a", "eq", 0)` |
| `rejectattr` | `{{ o \| rejectattr("a") \| list \| length }}` | `1` |
| `select`, `reject` | `{{ [1,2] \| select("odd") \| list }}` | `[1]` |
| `sort` | `{{ o \| sort(attribute="a") \| map(attribute="a") \| list }}` | `[0, 1]` |
| `groupby` | `{% for g in o \| groupby("a") %}{{ g.grouper }}:{{ g.list \| length }} {% endfor %}` | `0:1 1:1 ` |

`map` also applies a filter to each item: `{{ ["a"] | map("upper") | list }}`.

### Numbers and conversion

| Filter | Example | Prints |
|---|---|---|
| `round` | `{{ 2.567 \| round(1) }}` | `2.6` |
| `abs` | `{{ (-2) \| abs }}` | `2` |
| `int`, `float` | `{{ "3" \| int }}` | `3` |
| `bool` | `{{ 1 \| bool }}` | `True` |
| `string` | `{{ 1 \| string }}` | `1` |

### Objects

| Filter | Example | Prints, for `m` = `{"k":"v"}` |
|---|---|---|
| `items` | `{{ m \| items \| list }}` | `[('k', 'v')]` |
| `dictsort` | `{{ m \| dictsort }}` | `[('k', 'v')]` |
| `attr` | `{{ m \| attr("k") }}` | `v` |
| `tojson` | `{{ m \| tojson }}` | `{"k": "v"}` |
| `pprint` | `{{ m \| pprint }}` | The value spread over several lines |

### Missing values and escaping

| Filter | Example | Does |
|---|---|---|
| `default`, `d` | `{{ nope \| default("x") }}` | Replaces an undefined value. `default("x", true)` also replaces anything false, including `null` |
| `escape`, `e` | `{{ "<" \| escape }}` | Prints `&lt;`. See [escaping](/docs/templates/syntax/#escaping) |
| `safe` | `{{ "<" \| safe }}` | Writes a value as it is, with no escaping |

### A worked total

```bash test
cat > total.txt <<'EOF'
{% set total = items | sum(attribute="price") %}{{ items | map(attribute="name") | join(", ") }}: {{ total | currency("EUR") }}
EOF
curl -F template=@total.txt \
  -F 'data={"items":[{"name":"Lamp","price":19.5},{"name":"Plug","price":0.5}]}' \
  http://localhost:9027/v1/render -o out.txt
grep -x 'Lamp, Plug: EUR 20.00' out.txt
```

### Filters that do not exist

`urlencode`, `truncate`, `wordcount`, `striptags`, `center` and `nl2br` are not in this build. Each gives `unknown filter`.

## Tests

`is` asks a question about a value: `{% if qty is odd %}`. The tests available are `defined`, `undefined`, `none`, `boolean`, `number`, `integer` (or `int`), `float`, `string`, `sequence`, `iterable`, `mapping`, `odd`, `even`, `divisibleby`, `startingwith`, `endingwith`, `lower`, `upper`, `in`, `true`, `false`, `safe`, `sameas`, `eq`, `ne`, `lt`, `le`, `gt` and `ge`.

## Functions

`range(3)` gives `[0, 1, 2]`, `dict(a=1)` gives an object, and `namespace(x=1)` gives an object a loop can change with `{% set ns.x = ... %}`.
