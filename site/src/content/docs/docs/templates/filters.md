---
title: Filters
description: Format numbers, money and dates, translate labels, and transform values with every filter a template can use.
sidebar:
  order: 2
---

This page lists every filter, so you can format numbers, money and dates, translate labels, and sum or sort lists from inside a template. A filter follows a value with `|` and may take arguments: `{{ total | currency("KES") }}`. Filters chain from left to right.

## Where each filter works

| Filter | Office templates | Text templates | PDF tags and `batchName` |
|---|---|---|---|
| `currency`, `number`, `date`, `t` | Yes | Yes | Yes |
| `image`, `barcode`, `link`, `color`, `html`, `chart`, `pagebreak` | Yes | No | No |
| [Built-in filters](#built-in-filters) | Yes | Yes | Yes |

`currency`, `number`, `date` and `t` read the same settings everywhere: the request's `lang`, `timezone`, `currency` and `translations` fields, then the template's `{% settings %}` and document properties. See [Translations and settings](/docs/templates/translations/).

Using a filter where it does not exist is a `422`:

```bash test=fail
printf '{{ logo | image }}\n' > logo.txt
curl -F template=@logo.txt -F 'data={"logo":"x"}' http://localhost:9027/v1/render
```

The [rich content filters](/docs/templates/rich-content/) have their own page.

## Language

Without a language, `number`, `currency` and `date` print as they always have: `,` between thousands, `.` before decimals, the currency code in front, and English names. With one, from the request's `lang` field or the template's `lang` setting, they follow that language's [CLDR](https://cldr.unicode.org/) data:

| `lang` | `1234.5 \| number(2)` | `1500 \| currency("EUR")` | `"2026-01-05" \| date("%A %d %B")` |
|---|---|---|---|
| none | `1,234.50` | `EUR 1,500.00` | `Monday 05 January` |
| `en-us` | `1,234.50` | `€1,500.00` | `Monday 05 January` |
| `fr` | `1 234,50` | `1 500,00 €` | `lundi 05 janvier` |
| `de` | `1.234,50` | `1.500,00 €` | `Montag 05 Januar` |
| `de-ch` | `1'234.50` | `EUR 1'500.00` | `Montag 05 Januar` |
| `es` | `1234,50` | `1500,00 €` | `lunes 05 enero` |
| `ja` | `1,234.50` | `€1,500.00` | `月曜日 05 1月` |

The spaces are the ones CLDR uses: a narrow no-break space (U+202F) between thousands in French, and a no-break space (U+00A0) before `€`. Spanish and Polish group only from five digits, so `1234` stays together.

Languages with data: `ar`, `bg`, `ca`, `cs`, `da`, `de` (and `de-at`, `de-ch`, `de-li`), `el`, `en` (and `en-au`, `en-ca`, `en-gb`, `en-ie`, `en-in`, `en-nz`, `en-us`, `en-za`), `es` (and `es-419`, `es-mx`, `es-us`), `et`, `eu`, `fi`, `fr` (and `fr-be`, `fr-ca`, `fr-ch`), `gl`, `he`, `hi`, `hr`, `hu`, `id`, `is`, `it` (and `it-ch`), `ja`, `ko`, `lt`, `lv`, `ms`, `nb`, `nl` (and `nl-be`), `nn`, `no`, `pl`, `pt` (and `pt-br`, `pt-pt`), `ro`, `ru`, `sk`, `sl`, `sr`, `sv` (and `sv-fi`), `th`, `tr`, `uk`, `vi`, `zh` (and `zh-hk`, `zh-tw`). Another region falls back to its language, so `fr-lu` formats as `fr`. A language with no data at all formats as if none were set, though `t` still uses it.

## currency

```text
{{ value | currency }}
{{ value | currency("KES") }}
```

Without a language, it prints a number with two decimals, commas between thousands, and the code in front.

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

With a language, the symbol goes where the language puts it, and the amount has the currency's own decimals: none for JPY, three for KWD, two for most.

| `lang` | Tag | Prints |
|---|---|---|
| `en-us` | `1500 \| currency("USD")` | `$1,500.00` |
| `fr` | `1500 \| currency("EUR")` | `1 500,00 €` |
| `de` | `-1500 \| currency("EUR")` | `-1.500,00 €` |
| `ja` | `1500 \| currency("JPY")` | `￥1,500` |
| `en` | `1500 \| currency("KWD")` | `KWD 1,500.000` |

```bash test
printf '{{ 1500 | currency("EUR") }}|{{ 1500 | currency("JPY") }}' > money.txt
curl -F template=@money.txt -F lang=de http://localhost:9027/v1/render -o out.txt
# The no-break spaces CLDR puts before the symbol, shown here as _.
test "$(perl -CSD -pe 's/\x{a0}/_/g' out.txt)" = '1.500,00_€|1.500_¥'
```

- The code is any text you pass. A code CLDR does not know prints as written, with two decimals.
- The code can come from data, as in `{{ total | currency(currency) }}`.
- With no argument, the request's `currency` field applies, then the template's [`currency` setting](/docs/templates/translations/). `currency("")` prints no code even when one is set.
- The value must be a number. A string such as `"12"` is an error: `cannot convert string to f64`. So is `null`.

## number

```text
{{ value | number }}
{{ value | number(2) }}
```

Prints a number with separators between thousands: up to three decimals with trailing zeros dropped, or exactly as many as the argument says, from 0 to 20. Halves round away from zero.

| Value | Tag | No language | `fr` | `de-ch` |
|---|---|---|---|---|
| `1234.5` | `number` | `1,234.5` | `1 234,5` | `1'234.5` |
| `1234.5` | `number(2)` | `1,234.50` | `1 234,50` | `1'234.50` |
| `0.125` | `number(2)` | `0.13` | `0,13` | `0.13` |
| `-1234567.891` | `number(0)` | `-1,234,568` | `-1 234 568` | `-1'234'568` |

```bash test
printf '{{ a | number }}|{{ a | number(2) }}' > n.txt
curl -F template=@n.txt -F 'data={"a":1234.5}' -F lang=de http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = '1.234,5|1.234,50'
```

The value must be a number, as for `currency`.

## date

```text
{{ issued | date }}
{{ issued | date("%d %B %Y") }}
```

The one argument is a format. Without it the format is `%Y-%m-%d`. A second argument is an error (`too many arguments`): the time zone comes from the request's `timezone` field or the template's [`timezone` setting](/docs/templates/translations/#timezone).

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
| `%p`, `%P` | `AM`, `am` |
| `%H`, `%M`, `%S` | `10`, `20`, `30` |
| `%j` | `310`, the day of the year |

With a language, `%B`, `%b`, `%A`, `%a`, `%p` and `%P` print CLDR's names in it, and everything else stays as it is. In languages where a month changes form inside a date, `%B` takes the form for a date (`5 января` in Russian) when the format has a day of the month (`%d`, `%e`), and the form for a month alone (`январь 2026`) when it does not.

```bash test
printf '{{ d | date("%%A %%d %%B %%Y") }}|{{ d | date("%%B %%Y") }}' > dated.txt
curl -F template=@dated.txt -F 'data={"d":"2026-01-05"}' -F lang=ru http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = 'понедельник 05 января 2026|январь 2026'
```

### Time zones

The time zone is an offset such as `+02:00` or `-05:00`, or an IANA zone such as `Europe/Paris` or `America/New_York`, which follows daylight saving. The default is UTC. It changes only values that are a moment: those with a zone, and unix seconds. A value with no zone prints as written.

```bash test
printf '{{ "2026-03-28T12:00:00Z" | date("%%H:%%M") }} {{ "2026-03-30T12:00:00Z" | date("%%H:%%M") }}' > tz.txt
curl -F template=@tz.txt -F timezone=Europe/Paris http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = '13:00 14:00'
```

A zone that is neither is refused: `timezone "Mars/Olympus" is neither an offset like +02:00 nor an IANA zone like Europe/Paris`. As a request field that is a `400`; in a template setting, a `422` when `date` runs.

The sample invoice uses `date` in its issue and payment lines:

```bash test
curl -F template=@invoice.docx -F "data=<invoice.json" http://localhost:9027/v1/render -o invoice-filled.docx
unzip -p invoice-filled.docx word/document.xml | grep -o 'Issued 06 November 2026'
```

## t

It takes no arguments.

```text
{{ "greeting" | t }}
{{ label | t }}
```

It looks the key up in the language, then in the language it falls back to (`fr` for `fr-ca`), then in the template's translations with no language, and prints the key itself when none has it. The request's `translations` come first, then the service's dictionaries, then the template's own. See [Translations and settings](/docs/templates/translations/).

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
