---
title: Filters
description: Format numbers, money and dates, translate labels, and transform values with every filter a template can use.
sidebar:
  order: 2
---

Filters format numbers, money and dates, translate labels, sum or sort lists, shorten and pad text, and calculate with dates, all from inside a template. A filter follows a value with `|` and can take arguments, as in `{{ total | currency("KES") }}`, and filters chain from left to right. Every filter, test and function is listed below.

## Where each filter works

| Filter | Office templates | Text templates | PDF tags and `batchName` |
|---|---|---|---|
| `currency`, `number`, `date`, `excel_date`, `t` | Yes | Yes | Yes |
| `image`, `barcode`, `link`, `color`, `html`, `chart`, `pagebreak` | Yes | No | No |
| [Text](#text-helpers), [number](#number-helpers) and [date](#date-helpers) helpers: `truncate`, `unaccent`, `pad_left`, `pad_right`, `avg`, `ceil`, `floor`, `words`, `date_add`, `date_sub`, `start_of`, `end_of`, `date_diff`, `duration` | Yes | Yes | Yes |
| [Built-in filters](#built-in-filters) | Yes | Yes | Yes |

`currency`, `number`, `date`, `excel_date` and `t` read the same settings everywhere: the request's `lang`, `timezone`, `currency` and `translations` fields, then the template's `{% settings %}` and document properties. See [Translations and settings](/docs/templates/translations/).

The Office-only filters are the [rich content filters](/docs/templates/rich-content/), which have their own page. Use a filter where it doesn't exist and you get a `422`:

```bash test=fail
printf '{{ logo | image }}\n' > logo.txt
curl -F template=@logo.txt -F 'data={"logo":"x"}' \
  http://localhost:9027/v1/render
```

## Language

With no language set, `number`, `currency` and `date` print as they always have, with `,` between thousands, `.` before decimals, the currency code in front, and English names. Set one, in the request's `lang` field or the template's `lang` setting, and they follow that language's [CLDR](https://cldr.unicode.org/) data:

| `lang` | `1234.5 \| number(2)` | `1500 \| currency("EUR")` | `"2026-01-05" \| date("%A %d %B")` |
|---|---|---|---|
| none | `1,234.50` | `EUR 1,500.00` | `Monday 05 January` |
| `en-us` | `1,234.50` | `€1,500.00` | `Monday 05 January` |
| `fr` | `1 234,50` | `1 500,00 €` | `lundi 05 janvier` |
| `de` | `1.234,50` | `1.500,00 €` | `Montag 05 Januar` |
| `de-ch` | `1'234.50` | `EUR 1'500.00` | `Montag 05 Januar` |
| `es` | `1234,50` | `1500,00 €` | `lunes 05 enero` |
| `ja` | `1,234.50` | `€1,500.00` | `月曜日 05 1月` |

The spaces are the ones CLDR uses: a narrow no-break space (U+202F) between thousands in French, and a no-break space (U+00A0) before `€`. Spanish and Polish only group from five digits, so `1234` stays together.

The languages with data are `ar`, `bg`, `ca`, `cs`, `da`, `de` (and `de-at`, `de-ch`, `de-li`), `el`, `en` (and `en-au`, `en-ca`, `en-gb`, `en-ie`, `en-in`, `en-nz`, `en-us`, `en-za`), `es` (and `es-419`, `es-mx`, `es-us`), `et`, `eu`, `fi`, `fr` (and `fr-be`, `fr-ca`, `fr-ch`), `gl`, `he`, `hi`, `hr`, `hu`, `id`, `is`, `it` (and `it-ch`), `ja`, `ko`, `lt`, `lv`, `ms`, `nb`, `nl` (and `nl-be`), `nn`, `no`, `pl`, `pt` (and `pt-br`, `pt-pt`), `ro`, `ru`, `sk`, `sl`, `sr`, `sv` (and `sv-fi`), `th`, `tr`, `uk`, `vi` and `zh` (and `zh-hk`, `zh-tw`). Any other region falls back to its language, so `fr-lu` formats as `fr`. A language with no data at all formats as if none were set, though `t` still uses it.

## currency

```text
{{ value | currency }}
{{ value | currency("KES") }}
```

Without a language, `currency` prints a number with two decimals, commas between thousands, and the code in front.

| Value | Tag | Prints |
|---|---|---|
| `1234.5` | `currency("USD")` | `USD 1,234.50` |
| `-1234567.891` | `currency("USD")` | `-USD 1,234,567.89` |
| `999.999` | `currency("EUR")` | `EUR 1,000.00` |
| `0.5` | `currency` | `0.50` |
| `2` | `currency("")` | `2.00` |

```bash test
cat > money.txt <<'EOF'
{{ a | currency("USD") }}|{{ b | currency }}|{{ c | currency("EUR") }}
EOF
curl -F template=@money.txt -F 'data={"a":1234.5,"b":0.5,"c":999.999}' \
  http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = 'USD 1,234.50|0.50|EUR 1,000.00'
```

With a language, the symbol goes where that language puts it, and the amount has the currency's own decimals: none for JPY, three for KWD, two for most.

| `lang` | Tag | Prints |
|---|---|---|
| `en-us` | `1500 \| currency("USD")` | `$1,500.00` |
| `fr` | `1500 \| currency("EUR")` | `1 500,00 €` |
| `de` | `-1500 \| currency("EUR")` | `-1.500,00 €` |
| `ja` | `1500 \| currency("JPY")` | `￥1,500` |
| `en` | `1500 \| currency("KWD")` | `KWD 1,500.000` |

```bash test
printf '{{ 1500 | currency("EUR") }}|{{ 1500 | currency("JPY") }}' > money.txt
curl -F template=@money.txt -F lang=de \
  http://localhost:9027/v1/render -o out.txt
# The no-break spaces CLDR puts before the symbol, shown here as _.
test "$(perl -CSD -pe 's/\x{a0}/_/g' out.txt)" = '1.500,00_€|1.500_¥'
```

The code is any text you pass, and it can come from data, as in `{{ total | currency(currency) }}`. A code CLDR doesn't know prints as written, with two decimals. With no argument, the request's `currency` field applies, then the template's [`currency` setting](/docs/templates/translations/), and `currency("")` prints no code even when one is set.

The value must be a number. A string like `"12"` is an error (`cannot convert string to f64`), and so is `null`.

## number

```text
{{ value | number }}
{{ value | number(2) }}
```

`number` prints a number with separators between thousands and up to three decimals, with trailing zeros dropped. Pass an argument, from 0 to 20, to print exactly that many decimals instead. Halves round away from zero.

| Value | Tag | No language | `fr` | `de-ch` |
|---|---|---|---|---|
| `1234.5` | `number` | `1,234.5` | `1 234,5` | `1'234.5` |
| `1234.5` | `number(2)` | `1,234.50` | `1 234,50` | `1'234.50` |
| `0.125` | `number(2)` | `0.13` | `0,13` | `0.13` |
| `-1234567.891` | `number(0)` | `-1,234,568` | `-1 234 568` | `-1'234'568` |

```bash test
printf '{{ a | number }}|{{ a | number(2) }}' > n.txt
curl -F template=@n.txt -F 'data={"a":1234.5}' -F lang=de \
  http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = '1.234,5|1.234,50'
```

The value must be a number, as for `currency`.

## date

```text
{{ issued | date }}
{{ issued | date("%d %B %Y") }}
```

The only argument is a format, which defaults to `%Y-%m-%d`. A second argument is an error (`too many arguments`), because the time zone comes from the request's `timezone` field or the template's [`timezone` setting](/docs/templates/translations/#the-four-settings).

### What it accepts

| Value | Treated as |
|---|---|
| `2026-11-06` | That date, at midnight |
| `2026-11-06T10:20` | Local time, with no conversion |
| `2026-11-06T10:20:30`, `2026-11-06 10:20:30` | Local time, with no conversion. Fractions of a second are allowed |
| `2026-11-06T10:20:30Z`, `2026-11-06T10:20:30+03:00` | A moment, shown in the time zone |
| `2026-11-06 10:20:30+0300` | A moment, shown in the time zone |
| `1793980800`, `1793980800.5` | Unix seconds, shown in the time zone |

Anything else, like `06/11/2026`, fails with `date: cannot parse "06/11/2026"; use ISO 8601 or unix seconds`.

### Format strings

Formats follow [chrono's `strftime` syntax](https://docs.rs/chrono/latest/chrono/format/strftime/index.html), and an unknown specifier is an error: `date: invalid format "%Q"`.

| Specifier | Prints, for 6 November 2026 at 10:20:30 |
|---|---|
| `%Y`, `%y` | `2026`, `26` |
| `%m`, `%d` | `11`, `06` |
| `%B`, `%b` | `November`, `Nov` |
| `%A`, `%a` | `Friday`, `Fri` |
| `%p`, `%P` | `AM`, `am` |
| `%H`, `%M`, `%S` | `10`, `20`, `30` |
| `%j` | `310`, the day of the year |

With a language, `%B`, `%b`, `%A`, `%a`, `%p` and `%P` print CLDR's names in it, and everything else stays as it is. In languages where a month changes form inside a date, `%B` takes the form for a date (`5 января` in Russian) when the format has a day of the month (`%d`, `%e`), and the form for a month alone (`январь 2026`) when it doesn't.

```bash test
printf '{{ d | date("%%A %%d %%B %%Y") }}|{{ d | date("%%B %%Y") }}' > dated.txt
curl -F template=@dated.txt -F 'data={"d":"2026-01-05"}' -F lang=ru \
  http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = 'понедельник 05 января 2026|январь 2026'
```

### Time zones

The time zone is an offset like `+02:00` or `-05:00`, or an IANA zone like `Europe/Paris` or `America/New_York`, which follows daylight saving. The default is UTC. It only changes values that are a moment (those with a zone, and unix seconds), so a value with no zone prints as written.

```bash test
cat > tz.txt <<'EOF'
{{ "2026-03-28T12:00:00Z" | date("%H:%M") }}
{{ "2026-03-30T12:00:00Z" | date("%H:%M") }}
EOF
curl -F template=@tz.txt -F timezone=Europe/Paris \
  http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = "$(printf '13:00\n14:00')"
```

A zone that's neither fails with `timezone "Mars/Olympus" is neither an offset like +02:00 nor an IANA zone like Europe/Paris`. In a request field that's a `400`, and in a template setting it's a `422` when `date` runs.

The sample invoice uses `date` in its issue and payment lines:

```bash test
curl -F template=@invoice.docx -F "data=<invoice.json" \
  http://localhost:9027/v1/render -o invoice-filled.docx
unzip -p invoice-filled.docx word/document.xml |
  grep -o 'Issued 06 November 2026'
```

## t

`t` takes no arguments.

```text
{{ "greeting" | t }}
{{ label | t }}
```

It looks the key up in the language, then in the language it falls back to (`fr` for `fr-ca`), then in the template's translations with no language, and prints the key itself when none of them has it. The request's `translations` come first, then the service's dictionaries, then the template's own. See [Translations and settings](/docs/templates/translations/).

## Built-in filters

These come from minijinja and work in every template. Each example here was run as written.

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

`map` can also apply a filter to each item, as in `{{ ["a"] | map("upper") | list }}`.

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

| Filter | Example | What it does |
|---|---|---|
| `default`, `d` | `{{ nope \| default("x") }}` | Replaces an undefined value. `default("x", true)` also replaces anything false, including `null` |
| `escape`, `e` | `{{ "<" \| escape }}` | Prints `&lt;`. See [escaping](/docs/templates/syntax/#escaping) |
| `safe` | `{{ "<" \| safe }}` | Writes a value as it is, with no escaping |

### A worked total

```bash test
cat > total.txt <<'EOF'
{% set total = items | sum(attribute="price") -%}
{{ items | map(attribute="name") | join(", ") }}: {{ total | currency("EUR") }}
EOF
cat > items.json <<'EOF'
{
  "items": [
    {"name": "Lamp", "price": 19.5},
    {"name": "Plug", "price": 0.5}
  ]
}
EOF
curl -F template=@total.txt -F data=@items.json \
  http://localhost:9027/v1/render -o out.txt
grep -x 'Lamp, Plug: EUR 20.00' out.txt
```

### Filters that don't exist

`urlencode`, `wordcount`, `striptags`, `center` and `nl2br` aren't in this build, and each one gives `unknown filter`.

## Text helpers

These work in every template. They count characters, not bytes, so `é` is one character, but a letter written as a base letter and a separate accent mark counts as two.

They take text or a number. `null` counts as empty text, and a list or an object is an error: `expected text or a number`.

### truncate

```text
{{ title | truncate(40) }}
{{ title | truncate(40, end="...") }}
{{ title | truncate(40, words=true) }}
```

`truncate` shortens text to at most `length` characters, `end` included, and leaves text that fits unchanged. `end` is `…` unless you pass something else, as the second argument or as `end=`, and `end=""` cuts with no mark.

With `words=true`, the cut never falls inside a word, because the text stops at the last space that fits. If the first word alone is too long, only `end` remains.

| Value | Tag | Prints |
|---|---|---|
| `"crème brûlée au café"` | `truncate(9)` | `crème br…` |
| `"crème brûlée au café"` | `truncate(9, words=true)` | `crème…` |
| `"crème brûlée au café"` | `truncate(13, "...", words=true)` | `crème...` |
| `"crème brûlée au café"` | `truncate(30)` | `crème brûlée au café` |

A negative length is an error.

### unaccent

```text
{{ name | unaccent }}
```

`unaccent` removes accents and other marks above and below letters, so `crème brûlée` gives `creme brulee` and `Łódź` gives `Lodz`. Letters that aren't a base letter plus a mark are spelled with plain letters: `ß` gives `ss`, `æ` gives `ae` and `ø` gives `o`. Scripts whose marks change the letter, like Devanagari vowel signs or Japanese kana, are left as they are.

Use it for file names, identifiers and systems that only accept ASCII, and chain it with other filters: `{{ name | unaccent | lower | replace(" ", "-") }}`.

### pad_left and pad_right

```text
{{ number | pad_left(6, "0") }}
{{ name | pad_right(20) }}
{{ name | pad_right(20, char=".") }}
```

These fill text out to `width` characters, with spaces or the one character you pass. `pad_left` adds the fill in front, which right-aligns the text, and `pad_right` adds it after. Text that's already that long is left as it is, never cut, so chain `truncate` first for a fixed width.

| Value | Tag | Prints |
|---|---|---|
| `42` | `pad_left(6, "0")` | `000042` |
| `"é"` | `pad_right(3, ".")` | `é..` |
| `"abcdef"` | `pad_left(3)` | `abcdef` |

The width must be between 0 and 10,000, and the fill must be exactly one character. Anything else is an error, like `pad_left: the width must be between 0 and 10000, got 100000`.

```bash test
cat > text.txt <<'EOF'
[{{ t | truncate(9) }}]
[{{ t | truncate(9, words=true) }}]
[{{ t | unaccent }}]
[{{ n | pad_left(6, "0") }}]
[{{ "é" | pad_right(3, ".") }}]
EOF
curl -F template=@text.txt -F 'data={"t":"crème brûlée au café","n":42}' \
  http://localhost:9027/v1/render -o out.txt
cat > expected.txt <<'EOF'
[crème br…]
[crème…]
[creme brulee au cafe]
[000042]
[é..]
EOF
test "$(cat out.txt)" = "$(cat expected.txt)"
```

## Number helpers

These work in every template, and [Calculations](/docs/templates/calculations/) shows them alongside the built-in arithmetic.

### avg

```text
{{ scores | avg }}
{{ lines | avg(attribute="price") }}
```

`avg` gives the mean of a list of numbers, or of one attribute of a list of objects. The attribute can be a path like `"totals.net"`. Items whose value is missing or `null` are left out of the count. An empty list, or one with no values, gives `null`, so add `| default(0, true)` to print `0` instead. A value that isn't a number is an error: `avg: can only average numbers, got string`.

A whole result prints without decimals: `{{ [1, 2, 6] | avg }}` prints `3`, while `{{ [1, 2] | avg }}` prints `1.5`.

### ceil and floor

```text
{{ 2.1 | ceil }}
{{ 2.9 | floor }}
```

These give the next whole number up or down, as a whole number, so you get `3` and `2` rather than `3.0`. For negative numbers, up is towards zero, so `{{ -2.1 | ceil }}` is `-2`. A value that isn't a number is an error. To round to decimals instead, use `round(2, "ceil")`.

### words

```text
{{ 1234 | words }}
{{ 21 | words(ordinal=true) }}
{{ 1250.5 | words(currency="EUR") }}
{{ 1234 | words(lang="fr") }}
```

`words` spells a number out.

| Value | Tag | Prints |
|---|---|---|
| `1234` | `words` | `one thousand two hundred thirty-four` |
| `-7` | `words` | `minus seven` |
| `1.5` | `words` | `one point five` |
| `21` | `words(ordinal=true)` | `twenty-first` |
| `1250.5` | `words(currency="EUR")` | `one thousand two hundred fifty euros and fifty cents` |
| `1` | `words(currency="USD")` | `one US dollar` |
| `81` | `words(lang="fr")` | `quatre-vingt-un` |
| `70` | `words(lang="fr-BE")` | `septante` |

The languages are English (`en`), French (`fr`, with `fr-BE` and `fr-CH` for Belgian and Swiss numbers) and Ukrainian (`uk`). Without `lang=`, the template's [`lang` setting](/docs/templates/translations/) applies, and you get English if that language has no words or there's no setting. If you pass `lang=` a language that has no words, though, it's an error: `words: no words for language "de"; use en, fr or uk`.

`currency=` takes an ISO 4217 code, in any case. The codes with words are AED, ARS, AUD, BRL, CAD, CHF, CLP, CNY, COP, CRC, DZD, EUR, GBP, HKD, IDR, ILS, INR, JPY, KRW, KWD, KZT, MXN, MYR, NOK, NZD, PEN, PHP, PLN, QAR, RUB, SAR, SGD, THB, TRY, TWD, UAH, USD, UYU, VND and ZAR, plus the generic `DOLLAR`, `DINAR`, `PESO` and `RIYAL`. The amount is rounded to cents first. Any other code is an error, so for a currency with no words, use `DOLLAR` and `replace`, as the [worked invoice](/docs/templates/calculations/#worked-example-an-invoice) does.

Numbers can go up to 999,999,999,999,999 either side of zero, and a larger one fails with `words: 1e15 is too large`. An ordinal must be a whole number of 0 or more, and you can't combine `ordinal` and `currency`.

The value must be a number, so text like `"12"` is an error. Convert it with `| int` or `| float`.

```bash test
cat > words.txt <<'EOF'
{{ 1234 | words }}
{{ 21 | words(ordinal=true) }}
{{ 1250.5 | words(currency="EUR") }}
{{ 81 | words(lang="fr") }}
EOF
curl -F template=@words.txt -F 'data={}' \
  http://localhost:9027/v1/render -o out.txt
cat > expected.txt <<'EOF'
one thousand two hundred thirty-four
twenty-first
one thousand two hundred fifty euros and fifty cents
quatre-vingt-un
EOF
test "$(cat out.txt)" = "$(cat expected.txt)"
```

## excel_date

```text
{{ issued | excel_date }}
```

A spreadsheet stores a date as a number: the days since 30 December 1899, with the time of day as a fraction. `excel_date` turns a date into that number. Put the tag in a cell by itself and give the cell a date format, and Excel and LibreOffice treat it as a real date that sorts, filters and works in formulas.

| Value | Result |
|---|---|
| `2013-01-01` | `41275` |
| `2026-11-06T12:00` | `46332.5` |
| `2026-11-06T22:30:00Z`, with `timezone` set to `Africa/Nairobi` | `46333.0625` |

It reads values the way [`date`](#what-it-accepts) does and uses the same time zone. It takes no arguments. Dates before 1 March 1900 are an error, because Excel counts a 29 February 1900 that never happened and the numbers before it differ between applications. The number is for the 1900 date system, which is the default in Excel and what LibreOffice uses for XLSX.

`excel_date` was added in 0.1.1.

## Date helpers

These work in every template. They read dates the way [`date`](#what-it-accepts) does, as ISO 8601 text or unix seconds, and return ISO 8601 text, which `date` can print in any format. [Calculations](/docs/templates/calculations/#dates) explains each rule with examples.

| Filter | What it does | Example | Result |
|---|---|---|---|
| `date_add(amount, unit)` | Adds whole units | `"2028-01-31" \| date_add(1, "month")` | `2028-02-29` |
| `date_sub(amount, unit)` | Subtracts whole units | `"2028-03-31" \| date_sub(1, "month")` | `2028-02-29` |
| `start_of(unit)` | The first second of the period | `"2026-11-06T10:20:00Z" \| start_of("day")` | `2026-11-06T00:00:00Z` |
| `end_of(unit)` | The last second of the period | `"2026-11-06" \| end_of("month")` | `2026-11-30` |
| `date_diff(other, unit)` | Whole units from `other` to the value | `"2026-12-25" \| date_diff("2026-11-06", "weeks")` | `7` |
| `duration(unit)` | An interval in words or in a unit | `5400000 \| duration` | `2 hours` |

- The units are `year`, `quarter`, `month`, `week`, `day`, `hour`, `minute` and `second`, singular or plural. `date_add`, `date_sub` and `date_diff` count days when you leave the unit out.
- Adding months keeps the day of the month, or uses the month's last day when the target month is shorter.
- A date stays a date, a time with no zone stays one, and a time with a zone keeps its offset. Unix seconds come back as UTC.
- A week starts on Monday.
- `date_diff` is negative when the value is before `other`, and drops part of a unit.
- A result out of range is an error, `date_add: the date is out of range`, and so is an unknown unit.

```bash test
cat > dates.txt <<'EOF'
{{ d | date_add(1, "month") }}
{{ d | end_of("month") }}
{{ "2026-12-25" | date_diff("2026-11-06", "weeks") }}
{{ 5400000 | duration }}
EOF
curl -F template=@dates.txt -F 'data={"d":"2028-01-31"}' \
  http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = "$(printf '2028-02-29\n2028-01-31\n7\n2 hours')"
```

### duration

```text
{{ elapsed_ms | duration }}
{{ elapsed_ms | duration("hours") }}
{{ "P1DT12H" | duration("hours") }}
```

`duration` takes milliseconds, or an ISO 8601 duration like `P1Y2M3DT4H5M6S` or `PT1H30M`. With no argument, or with `"human"`, it prints words: `a few seconds`, `a minute`, `5 minutes`, `an hour`, `3 hours`, `a day`, `12 days`, `a month`, `4 months`, `a year`, `2 years`. The words are in English and ignore the sign.

With a unit, which is one of `years`, `months`, `weeks`, `days`, `hours`, `minutes`, `seconds` or `milliseconds` (`ms`), it prints a number, with decimals when needed, so `5400000 | duration("hours")` is `1.5`. A month counts as 30 days and a year as 365. Text that isn't an ISO duration, like `"2 hours"`, is an error.

## Tests

`is` asks a question about a value, as in `{% if qty is odd %}`. The tests available are `defined`, `undefined`, `none`, `empty`, `boolean`, `number`, `integer` (or `int`), `float`, `string`, `sequence`, `iterable`, `mapping`, `odd`, `even`, `divisibleby`, `startingwith`, `endingwith`, `lower`, `upper`, `in`, `true`, `false`, `safe`, `sameas`, `eq`, `ne`, `lt`, `le`, `gt` and `ge`.

`empty` works in every template. It's true for `null`, a missing value, `""`, `[]` and `{}`, and false for `0` and `false`, so you can write `{% if notes is not empty %}`. [Conditions](/docs/templates/conditions/#tests) explains each test.

## Functions

`range(3)` gives `[0, 1, 2]`, `dict(a=1)` gives an object, and `namespace(x=1)` gives an object a loop can change with `{% set ns.x = ... %}`.

`now()` works in every template and gives the time of the render in UTC, to the second, like `2026-11-06T10:20:00Z`. To pin it, send `ctx={"now":"2026-11-06T10:20:00Z"}` with the request, so the output is the same on every run:

```bash test
printf '{{ now() }}|{{ now() | date_add(30) }}' > now.txt
curl -F template=@now.txt -F 'data={}' \
  -F 'ctx={"now":"2026-11-06T10:20:00Z"}' \
  http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = '2026-11-06T10:20:00Z|2026-12-06T10:20:00Z'
```
