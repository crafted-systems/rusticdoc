---
title: Calculations
description: Do arithmetic, totals, averages, counts, groups, running balances, lookups and date arithmetic inside a template.
sidebar:
  order: 1.6
---

This page shows how to calculate inside a template: arithmetic, totals and averages, counts, totals per group, running balances, lookups between lists, and dates. Everything here works in Office, text and PDF templates. It ends with a [worked invoice](#worked-example-an-invoice).

You can also do the sums in your application and send the results in `data`. Doing them in the template keeps the data plain and the layout's logic in one place.

## Arithmetic

| Operator | Does | Example | Prints |
|---|---|---|---|
| `+`, `-`, `*` | Add, subtract, multiply | `{{ 2 + 3 * 4 }}` | `14` |
| `/` | Divide, always giving a decimal | `{{ 10 / 4 }}`, `{{ 4 / 2 }}` | `2.5`, `2.0` |
| `//` | Divide and round down | `{{ 10 // 3 }}`, `{{ -7 // 2 }}` | `3`, `-4` |
| `%` | Remainder | `{{ 10 % 3 }}` | `1` |
| `**` | Power | `{{ 2 ** 10 }}` | `1024` |
| `( )` | Group | `{{ (a + b) * c }}` | |

```bash test
cat > math.txt <<'EOF'
{{ (a + b) * c }}
{{ a + b * c }}
{{ 10 / 4 }}
{{ 10 // 3 }}
{{ 10 % 3 }}
{{ 2 ** 10 }}
EOF
curl -F template=@math.txt -F 'data={"a":1,"b":2,"c":3}' \
  http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = "$(printf '9\n7\n2.5\n3\n1\n1024')"
```

### Precedence

From the loosest to the tightest:

1. `or`
2. `and`
3. `not`
4. Comparisons: `==`, `!=`, `<`, `<=`, `>`, `>=`, `in`, `not in`, `is`
5. `+`, `-`
6. `~`, which joins text
7. `*`, `/`, `//`, `%`
8. `**`, read from left to right: `2 ** 3 ** 2` is `64`
9. A filter: `|`

Three of these surprise people:

- **A filter binds tightest.** `a * b | currency` formats `b` alone. Write `(a * b) | currency`. See [a filter binds tighter than arithmetic](/docs/templates/syntax/#a-filter-binds-tighter-than-arithmetic).
- **`~` binds tighter than `+`.** `"INV-" ~ n + 1` joins first and then fails to add a number to text. Write `"INV-" ~ (n + 1)`.
- **A minus sign binds tighter than `**`.** `-2 ** 2` is `4`.

When in doubt, add brackets.

```bash test
cat > order.txt <<'EOF'
{{ "INV-" ~ (n + 1) }}|{{ (a * b) | round(1) }}|{{ 2 ** 3 ** 2 }}
EOF
curl -F template=@order.txt -F 'data={"n":41,"a":2,"b":3.33}' \
  http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = 'INV-42|6.7|64'
```

### Decimals

Numbers with decimals are binary floating point, as in most languages, so `0.1 + 0.2` prints `0.30000000000000004`. Round before you print: `round(2)` or [`currency`](/docs/templates/filters/#currency) does it.

Dividing by zero stops the render with `unable to calculate 1 / 0`. Guard it with a condition: `{{ (done / total * 100) | round if total else 0 }}`.

### Rounding

| Filter | Example | Prints |
|---|---|---|
| `round` | `{{ 2.567 \| round(2) }}` | `2.57` |
| `round` with a method | `{{ 2.561 \| round(2, "ceil") }}`, `{{ 2.569 \| round(2, "floor") }}` | `2.57`, `2.56` |
| `ceil` | `{{ 2.1 \| ceil }}`, `{{ -2.1 \| ceil }}` | `3`, `-2` |
| `floor` | `{{ 2.9 \| floor }}`, `{{ -2.1 \| floor }}` | `2`, `-3` |
| `abs` | `{{ (0 - 5) \| abs }}` | `5` |

`round` always gives a decimal, and sends a half to the even neighbour: `{{ 2.5 | round }}` prints `2.0`, `{{ 3.5 | round }}` prints `4.0`, and `{{ 0.125 | round(2) }}` prints `0.12`. `currency` sends a half away from zero instead, so `{{ 0.125 | currency }}` prints `0.13`. Use the same one for every figure on a document, or totals may differ by a cent.

```bash test
cat > round.txt <<'EOF'
{{ 2.567 | round(2) }}
{{ 2.561 | round(2, "ceil") }}
{{ 2.569 | round(2, "floor") }}
{{ -2.1 | ceil }}
{{ -2.1 | floor }}
{{ 2.5 | round }}
{{ 3.5 | round }}
{{ 0.125 | round(2) }}
{{ 0.125 | currency }}
EOF
curl -F template=@round.txt -F 'data={}' \
  http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = \
  "$(printf '%s\n' 2.57 2.57 2.56 -2 -3 2.0 4.0 0.12 0.13)"
```

`ceil` and `floor` give a whole number, which prints without `.0`, so use them for counts such as the boxes needed for 25 items, 12 to a box:

```bash test
cat > boxes.txt <<'EOF'
{{ (items / per_box) | ceil }} boxes, {{ items // per_box }} full
EOF
curl -F template=@boxes.txt -F 'data={"items":25,"per_box":12}' \
  http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = '3 boxes, 2 full'
```

## Totals, averages, minimum and maximum

For a list of objects, name the attribute to add up. For these examples `lines` is:

```json
[
  {"name": "Lamp", "group": "Home", "qty": 2, "price": 19.5},
  {"name": "Tea", "group": "Food", "qty": 3, "price": 4},
  {"name": "Rice", "group": "Food", "qty": 1, "price": 2.5}
]
```

| Want | Write | Gives |
|---|---|---|
| Total | `lines \| sum(attribute="price")` | `26.0` |
| Average | `lines \| avg(attribute="price")` | `8.666…` |
| Lowest and highest value | `lines \| map(attribute="price") \| min` | `2.5` |
| The item with the lowest value | `(lines \| min(attribute="price")).name` | `Rice` |
| Number of items | `lines \| length` | `3` |

```bash test
cat > totals.txt <<'EOF'
{{ lines | sum(attribute="price") }}
{{ lines | avg(attribute="price") | round(2) }}
{{ lines | map(attribute="price") | min }}
{{ lines | map(attribute="price") | max }}
{{ (lines | max(attribute="price")).name }}
{{ lines | length }}
EOF
cat > lines.json <<'EOF'
{
  "lines": [
    {"name": "Lamp", "group": "Home", "qty": 2, "price": 19.5},
    {"name": "Tea", "group": "Food", "qty": 3, "price": 4},
    {"name": "Rice", "group": "Food", "qty": 1, "price": 2.5}
  ]
}
EOF
curl -F template=@totals.txt -F data=@lines.json \
  http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = "$(printf '26.0\n8.67\n2.5\n19.5\nLamp\n3')"
```

- A list of plain numbers needs no attribute: `{{ [1, 2, 4] | sum }}`, `{{ [1, 2, 4] | avg }}`.
- An attribute can be a path: `sum(attribute="totals.net")`.
- `sum` skips items that lack the attribute, and gives `0` for an empty list.
- `avg` skips items whose value is missing or `null`, and gives `null` for an empty list, so `{{ lines | avg(attribute="price") | default(0, true) }}` prints `0` then.
- `min` and `max` of an empty list are undefined, which stops the render when printed. Add `| default(0)`.

### Totals of a calculation

`sum` adds one attribute. For quantity times price, add up in a loop with a [namespace](#store-values-with-set-and-namespace):

```bash test
cat > net.txt <<'EOF'
{% set ns = namespace(total=0) -%}
{% for l in lines -%}
  {% set ns.total = ns.total + l.qty * l.price -%}
{% endfor -%}
{{ ns.total | currency("EUR") }}
EOF
curl -F template=@net.txt \
  -F 'data={"lines":[{"qty":2,"price":19.5},{"qty":3,"price":4}]}' \
  http://localhost:9027/v1/render -o out.txt
grep -x 'EUR 51.00' out.txt
```

## Counts

| Want | Write |
|---|---|
| Items | `lines \| length` |
| Items that pass a test | `lines \| selectattr("qty", "gt", 1) \| list \| length` |
| Items where a value is set and true | `lines \| selectattr("paid") \| list \| length` |
| Distinct values | `lines \| map(attribute="group") \| unique \| list \| length` |
| Distinct values, listed | `lines \| map(attribute="group") \| unique \| join(", ")` |

```bash test
cat > counts.txt <<'EOF'
{{ lines | length }} lines
{{ lines | selectattr("qty", "gt", 1) | list | length }} with more than one
{{ lines | map(attribute="group") | unique | list | length }} groups
{{ lines | map(attribute="group") | unique | sort | join(", ") }}
EOF
cat > lines.json <<'EOF'
{
  "lines": [
    {"group": "Home", "qty": 2},
    {"group": "Food", "qty": 3},
    {"group": "Food", "qty": 1}
  ]
}
EOF
curl -F template=@counts.txt -F data=@lines.json \
  http://localhost:9027/v1/render -o out.txt
cat > expected.txt <<'EOF'
3 lines
2 with more than one
2 groups
Food, Home
EOF
test "$(cat out.txt)" = "$(cat expected.txt)"
```

`selectattr(attribute, test, value)` keeps the items that pass a [test](/docs/templates/conditions/#tests). The comparison tests are `eq`, `ne`, `lt`, `le`, `gt` and `ge`, and `in` checks a value against a list. `rejectattr` keeps the others.

## Filtered and per-group totals

Filter first, then add up:

```text
{{ lines | selectattr("group", "eq", "Food") | sum(attribute="price") }}
{{ lines | selectattr("group", "in", ["Food", "Drinks"]) | sum(attribute="price") }}
```

To get a total for each group, `groupby` splits the list by an attribute. Each group has `grouper`, the shared value, and `list`, its items. Groups come out sorted by that value.

```bash test
cat > groups.txt <<'EOF'
{% for g in lines | groupby("group") -%}
  {% set total = g.list | sum(attribute="price") -%}
  {{ g.grouper }}: {{ g.list | length }} lines, {{ total }}
{% endfor -%}
{% set food = lines | selectattr("group", "eq", "Food") -%}
Food only: {{ food | sum(attribute="price") }}
EOF
cat > lines.json <<'EOF'
{
  "lines": [
    {"group": "Home", "price": 19.5},
    {"group": "Food", "price": 4},
    {"group": "Food", "price": 2.5}
  ]
}
EOF
curl -F template=@groups.txt -F data=@lines.json \
  http://localhost:9027/v1/render -o out.txt
cat > expected.txt <<'EOF'
Food: 2 lines, 6.5
Home: 1 lines, 19.5
Food only: 6.5
EOF
test "$(cat out.txt)" = "$(cat expected.txt)"
```

In an Office template, put the group loop around a table and the line loop on a table row, as [Office formats](/docs/templates/office/#what-a-block-repeats) explains.

## Running totals

A running total, or balance, grows from one line to the next. Two ways work.

Keep it in a namespace, which a loop can change:

```text
{% set ns = namespace(balance=0) %}
{% for t in moves -%}
  {% set ns.balance = ns.balance + t.amount -%}
  {{ t.label }} {{ ns.balance }}
{%- endfor %}
```

Or add up the lines so far, with a slice of the list. `moves[:loop.index]` is every line up to this one:

```text
{% for t in moves -%}
  {{ t.label }} {{ moves[:loop.index] | sum(attribute="amount") }}
{%- endfor %}
```

The namespace is quicker on long lists, because the slice adds every earlier line again on each row.

```bash test
cat > balance.txt <<'EOF'
{% set ns = namespace(balance=100) -%}
{% for t in moves -%}
  {% set ns.balance = ns.balance + t.amount -%}
  {% set so_far = moves[:loop.index] | sum(attribute="amount") -%}
  {{ t.label }} {{ t.amount }} {{ ns.balance }} {{ so_far }}
{% endfor %}
EOF
cat > moves.json <<'EOF'
{
  "moves": [
    {"label": "Rent", "amount": -80},
    {"label": "Sale", "amount": 250},
    {"label": "Fee", "amount": -5}
  ]
}
EOF
curl -F template=@balance.txt -F data=@moves.json \
  http://localhost:9027/v1/render -o out.txt
cat > expected.txt <<'EOF'
Rent -80 20 -80
Sale 250 270 170
Fee -5 265 165
EOF
test "$(cat out.txt)" = "$(cat expected.txt)"
```

`loop.previtem` and `loop.nextitem` give the neighbouring items, to show a change from the line before: `{{ t.amount - loop.previtem.amount if not loop.first }}`.

## Sorting

```text
{{ lines | sort(attribute="price") }}
{{ lines | sort(attribute="price", reverse=true) }}
{{ lines | sort(attribute="group,price") }}
```

A comma between attributes sorts by the first, then by the second within it. Text sorts without regard to case unless you add `case_sensitive=true`. To take the top three, sort and slice: `(lines | sort(attribute="price", reverse=true))[:3]`.

```bash test
cat > sorted.txt <<'EOF'
{% set top = (lines | sort(attribute="price", reverse=true))[:2] -%}
{{ lines | sort(attribute="group,price") | map(attribute="name") | join(", ") }}
{{ top | map(attribute="name") | join(", ") }}
EOF
cat > lines.json <<'EOF'
{
  "lines": [
    {"name": "Lamp", "group": "Home", "price": 19.5},
    {"name": "Tea", "group": "Food", "price": 4},
    {"name": "Rice", "group": "Food", "price": 2.5}
  ]
}
EOF
curl -F template=@sorted.txt -F data=@lines.json \
  http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = "$(printf 'Rice, Tea, Lamp\nLamp, Tea')"
```

## Lookups and joins between lists

When one list refers to another by an id, find the matching item with `selectattr` and `first`:

```bash test
cat > join.txt <<'EOF'
{% for o in orders -%}
  {% set c = customers | selectattr("id", "eq", o.customer) | first -%}
  {{ o.number }}: {{ c.name if c is defined else "unknown customer" }}
{% endfor %}
EOF
cat > orders.json <<'EOF'
{
  "customers": [
    {"id": 7, "name": "Ada"},
    {"id": 9, "name": "Grace"}
  ],
  "orders": [
    {"number": "A-1", "customer": 9},
    {"number": "A-2", "customer": 7},
    {"number": "A-3", "customer": 4}
  ]
}
EOF
curl -F template=@join.txt -F data=@orders.json \
  http://localhost:9027/v1/render -o out.txt
cat > expected.txt <<'EOF'
A-1: Grace
A-2: Ada
A-3: unknown customer
EOF
test "$(cat out.txt)" = "$(cat expected.txt)"
```

`first` of an empty list is undefined, so test it with `is defined` before reading from it.

If the data can carry an object keyed by id instead of a list, the lookup is direct: with `customers` as `{"7": {"name": "Ada"}}`, write `{{ customers[o.customer | string].name }}`. JSON keys are always text, so a number id needs `| string`.

The first and last items are `items[0]` and `items[-1]`, or `items | first` and `items | last`.

## Store values with set and namespace

`{% set %}` names a value, so you calculate it once:

```text
{% set net = lines | sum(attribute="price") %}
{% set tax = (net * 0.16) | round(2) %}
Net {{ net }}, tax {{ tax }}, total {{ net + tax }}
```

`{% set name %}...{% endset %}` captures the text between the tags.

A `set` inside a loop lasts only for that pass of the loop. A value set before the loop is unchanged after it:

```bash test
cat > scope.txt <<'EOF'
{% set count = 0 -%}
{% set ns = namespace(count=0) -%}
{% for i in items -%}
  {% set count = count + 1 -%}
  {% set ns.count = ns.count + 1 -%}
{% endfor -%}
{{ count }} {{ ns.count }}
EOF
curl -F template=@scope.txt -F 'data={"items":[1,2,3]}' \
  http://localhost:9027/v1/render -o out.txt
grep -x '0 3' out.txt
```

To carry a value out of a loop, use `namespace()` and set its attributes, as the second counter does. A namespace can hold several values: `namespace(total=0, count=0, last=none)`.

A macro is a calculation you name and reuse:

```text
{% macro net(l) %}{{ (l.qty * l.price) | round(2) }}{% endmacro %}
{{ net(lines[0]) }}
```

A macro returns text. To use its result in arithmetic, `{{ net(l) | float }}` turns it back into a number.

## Number helpers

| Filter | Example | Prints |
|---|---|---|
| `avg`, `avg(attribute=...)` | `{{ [1, 2, 6] \| avg }}` | `3` |
| `ceil`, `floor` | `{{ 2.1 \| ceil }}` | `3` |
| `words` | `{{ 1234 \| words }}` | `one thousand two hundred thirty-four` |
| `words(ordinal=true)` | `{{ 21 \| words(ordinal=true) }}` | `twenty-first` |
| `words(currency="EUR")` | `{{ 12.5 \| words(currency="EUR") }}` | `twelve euros and fifty cents` |

[`words`](/docs/templates/filters/#words) spells a number in English, French or Ukrainian, following the template's `lang` setting. Amounts on cheques and contracts use it:

```bash test
cat > words.txt <<'EOF'
{{ total | currency("EUR") }} ({{ total | words(currency="EUR") }})
{{ rank | words(ordinal=true) }}
EOF
curl -F template=@words.txt -F 'data={"total":1250.5,"rank":3}' \
  http://localhost:9027/v1/render -o out.txt
cat > expected.txt <<'EOF'
EUR 1,250.50 (one thousand two hundred fifty euros and fifty cents)
third
EOF
test "$(cat out.txt)" = "$(cat expected.txt)"
```

## Dates

Dates in data are text such as `2026-11-06` or `2026-11-06T10:20:00Z`, or unix seconds. These filters calculate with them, and return text that the [`date`](/docs/templates/filters/#date) filter can print in any format.

| Filter | Example | Gives |
|---|---|---|
| `date_add(amount, unit)` | `{{ "2026-01-31" \| date_add(1, "month") }}` | `2026-02-28` |
| `date_sub(amount, unit)` | `{{ "2026-03-31" \| date_sub(1, "month") }}` | `2026-02-28` |
| `start_of(unit)` | `{{ "2026-11-06" \| start_of("month") }}` | `2026-11-01` |
| `end_of(unit)` | `{{ "2026-11-06" \| end_of("quarter") }}` | `2026-12-31` |
| `date_diff(other, unit)` | `{{ "2026-12-06" \| date_diff("2026-11-06", "days") }}` | `30` |
| `now()` | `{{ now() }}` | `2026-11-06T10:20:00Z` |

The units are `year`, `quarter`, `month`, `week`, `day`, `hour`, `minute` and `second`, in the singular or the plural. Without a unit, `date_add`, `date_sub` and `date_diff` count days.

```bash test
cat > dates.txt <<'EOF'
{{ issued | date_add(30) }}
{{ issued | date_add(1, "month") }}
{{ issued | end_of("month") }}
{{ due | date_diff(issued) }}
{{ issued | date_add(36, "hours") }}
EOF
curl -F template=@dates.txt \
  -F 'data={"issued":"2028-01-31","due":"2028-03-01"}' \
  http://localhost:9027/v1/render -o out.txt
cat > expected.txt <<'EOF'
2028-03-01
2028-02-29
2028-01-31
30
2028-02-01T12:00:00
EOF
test "$(cat out.txt)" = "$(cat expected.txt)"
```

What each one does:

- **Months and years keep the day of the month** when the target month has it, and use the month's last day when it does not: 31 January plus one month is 28 February, or 29 in a leap year. A quarter is three months.
- **A date stays a date.** `2026-11-06` plus a day is `2026-11-07`. Adding hours, minutes or seconds to a date gives a time: `2026-11-07T12:00:00`.
- **A time keeps its zone.** `2026-11-06T22:00:00+03:00` plus 4 hours is `2026-11-07T02:00:00+03:00`. Zones are fixed offsets, so there is no daylight saving to skip or repeat. Unix seconds come back as UTC text, such as `2026-11-06T10:20:00Z`.
- **`start_of` and `end_of`** give the first and last second of the period: `start_of("day")` is midnight and `end_of("day")` is `23:59:59`. A week runs from Monday to Sunday. For a date with no time, `end_of("month")` is the month's last day.
- **`date_diff` counts whole units** from the argument to the value, so it is negative when the value is earlier. Part of a unit is dropped: 23 hours is `0` days. Months follow the same rule as `date_add`, so 31 January to 29 February 2028 is one month. A value with no zone counts as UTC when compared with one that has a zone.

A result outside the years chrono can hold, about 262,000 years either side of year 0, is refused with `the date is out of range`. A unit that does not exist is refused with the list of units.

### Today's date

`now()` gives the time of the render in UTC, to the second, such as `2026-11-06T10:20:00Z`. Pass it to other filters: `{{ now() | date_add(14) | date("%d %B %Y") }}`.

Output that uses `now()` changes on every run, which breaks comparisons with a saved file. Send `ctx.now` to fix it for one request:

```bash test
cat > today.txt <<'EOF'
Printed {{ now() }}
valid until {{ now() | date_add(14) }}
{{ due | date_diff(now()) }} days left
EOF
curl -F template=@today.txt -F 'data={"due":"2026-12-01"}' \
  -F 'ctx={"now":"2026-11-06T10:20:00Z"}' \
  http://localhost:9027/v1/render -o out.txt
cat > expected.txt <<'EOF'
Printed 2026-11-06T10:20:00Z
valid until 2026-11-20T10:20:00Z
24 days left
EOF
test "$(cat out.txt)" = "$(cat expected.txt)"
```

`ctx.now` accepts anything `date` accepts. A date with no time, such as `2026-11-06`, makes `now()` a date too. A value that cannot be read is an error naming `ctx.now`. As with any `ctx` key, a `ctx` object inside `data` wins.

### Format the result

In an Office template, print a calculated date with `date`:

```bash test
mkdir t && cd t && unzip -q ../report.docx
export TAG='Pay by {{ issued | date_add(30) | date("%d %B %Y") }},'
TAG+=' {{ issued | date_diff(start, "weeks") }} weeks in.'
perl -pi -e 's{A document with no tags in it.}{$ENV{TAG}}' word/document.xml
zip -qr ../dated.docx . && cd ..

curl -F template=@dated.docx \
  -F 'data={"issued":"2026-11-06","start":"2026-09-01"}' \
  http://localhost:9027/v1/render -o out.docx
unzip -p out.docx word/document.xml |
  grep -o 'Pay by 06 December 2026, 9 weeks in.'
```

### Durations

`duration` prints a length of time given in milliseconds, or as an ISO 8601 duration such as `P1DT2H30M`. With no argument it gives words, and with a unit it gives a number:

| Value | Tag | Prints |
|---|---|---|
| `7200000` | `duration` | `2 hours` |
| `30000` | `duration` | `a few seconds` |
| `5400000` | `duration("hours")` | `1.5` |
| `"PT1H30M"` | `duration("minutes")` | `90` |
| `"P2W"` | `duration("days")` | `14` |

```bash test
cat > took.txt <<'EOF'
{{ ms | duration }}
{{ ms | duration("minutes") }}
{{ iso | duration }}
{{ iso | duration("hours") | round(1) }}
EOF
curl -F template=@took.txt -F 'data={"ms":5400000,"iso":"P1DT12H"}' \
  http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = "$(printf '2 hours\n90\n2 days\n36')"
```

The units are `years`, `months`, `weeks`, `days`, `hours`, `minutes`, `seconds` and `milliseconds` (or `ms`). In an ISO duration, and for the `months` and `years` units, a month counts 30 days and a year 365. The words are in English, rounded the way people say them: `5 minutes`, `an hour`, `3 days`, `a month`, `2 years`. The sign is ignored, so `-7200000` is also `2 hours`.

To print the time between two dates, take the difference in a unit, `{{ end | date_diff(start, "minutes") }} minutes`, or turn seconds into milliseconds for `duration`: `{{ ((end | date_diff(start, "seconds")) * 1000) | duration }}`.

## Worked example: an invoice

This invoice groups its lines, shows a subtotal for each group and a running balance on each line, adds tax, and prints the due date and the amount in words. It is a text template so the whole output fits on this page. The same tags work in a DOCX, with the line loop on a table row.

```bash test
cat > invoice.txt <<'EOF'
{%- set ns = namespace(balance=0) -%}
{%- set due = issued | date_add(30) -%}
Invoice {{ number | pad_left(6, "0") }}, issued {{ issued }}, due {{ due }}
{% for g in lines | groupby("group") %}
{{ g.grouper | upper }}
{%- set sub = namespace(total=0) %}
{%- for l in g.list %}
{%- set amount = l.qty * l.price %}
{%- set sub.total = sub.total + amount %}
{%- set ns.balance = ns.balance + amount %}
{%- set name = l.name | pad_right(14) %}
{%- set qty = l.qty | pad_left(4) %}
{%- set price = "%7.2f" | format(l.price) %}
{%- set cost = "%9.2f" | format(amount) %}
{%- set balance = "%9.2f" | format(ns.balance) %}
  {{ name }}{{ qty }} x {{ price }} {{ cost }} {{ balance }}
{%- endfor %}
  {{ "Subtotal" | pad_right(29) }}{{ "%9.2f" | format(sub.total) }}
{% endfor %}
{%- set tax = (ns.balance * vat) | round(2) %}
{%- set total = ns.balance + tax %}
Net   {{ ns.balance | currency("KES") }}
VAT   {{ tax | currency("KES") }} ({{ (vat * 100) | round | int }}%)
Total {{ total | currency("KES") }}
{{ total | words(currency="DOLLAR")
         | replace("dollar", "shilling")
         | capitalize }}.
{% if paid is not empty -%}
  {% set paid_total = paid | sum(attribute="amount") | currency("KES") -%}
  Paid {{ paid_total }} on {{ paid | map(attribute="date") | join(", ") }}.
{%- endif %}
EOF
cat > invoice-data.json <<'EOF'
{
  "number": 42,
  "issued": "2026-11-06",
  "vat": 0.16,
  "lines": [
    {"group": "Design", "name": "Logo", "qty": 1, "price": 800},
    {"group": "Printing", "name": "Flyers", "qty": 500, "price": 0.4},
    {"group": "Design", "name": "Cards", "qty": 2, "price": 150},
    {"group": "Printing", "name": "Posters", "qty": 20, "price": 3.5}
  ],
  "paid": [{"date": "2026-11-10", "amount": 500}]
}
EOF
curl -F template=@invoice.txt -F "data=<invoice-data.json" \
  http://localhost:9027/v1/render -o out.txt
cat > expected.txt <<'EOF'
Invoice 000042, issued 2026-11-06, due 2026-12-06

DESIGN
  Logo             1 x  800.00    800.00    800.00
  Cards            2 x  150.00    300.00   1100.00
  Subtotal                       1100.00

PRINTING
  Flyers         500 x    0.40    200.00   1300.00
  Posters         20 x    3.50     70.00   1370.00
  Subtotal                        270.00

Net   KES 1,370.00
VAT   KES 219.20 (16%)
Total KES 1,589.20
One thousand five hundred eighty-nine shillings and twenty cents.
Paid KES 500.00 on 2026-11-10.
EOF
test "$(cat out.txt)" = "$(cat expected.txt)"
```

Each part of it:

- `namespace(balance=0)` holds the running balance across both loops, and `sub` holds each group's subtotal.
- `groupby("group")` sorts the groups, so Design comes before Printing whatever the order of the lines.
- `"%9.2f" | format(x)` pads numbers to line up; `pad_left` and `pad_right` do the same for text.
- `words(currency="DOLLAR")` gives `dollars` and `cents`, which `replace` turns into shillings, since Kenyan shillings are not one of the [currencies `words` knows](/docs/templates/filters/#words).
- `{%-` and `-%}` remove the line breaks the statements would leave in a text template.
