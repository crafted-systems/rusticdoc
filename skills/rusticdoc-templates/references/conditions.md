# Conditions

<!-- Generated from site/src/content/docs/docs/templates/conditions.md by site/scripts/generate-skills.mjs. Do not edit. -->

Show, hide or choose text with inline conditions, if blocks, elif chains, tests and membership checks, in every kind of template.

Inline conditions and `if` blocks show, hide or choose content, and they're written the same way in Office, text and PDF templates. In an Office template, where you put the tags also decides how much of the document a block covers, which [conditions in Office templates](#conditions-in-office-templates) explains.

## Inline conditions

`a if condition else b` chooses between two values inside one tag:

```text
{{ "Paid" if paid else "Due" }}
{{ "Dear " ~ name if name else "Dear customer" }}
```

Without `else`, a false condition prints nothing. You can chain inline conditions, for up to three or four choices:

```bash
cat > inline.txt <<'EOF'
{{ "Paid" if paid else "Due" }}
{{ " (late)" if late }}
{{ "Gold" if points >= 1000 else "Silver" if points >= 500 else "Bronze" }}
EOF
curl -F template=@inline.txt \
  -F 'data={"paid":false,"late":false,"points":700}' \
  http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = "$(printf 'Due\n\nSilver')"
```

`or` gives you the first value that's true, which makes a short fallback: `{{ nickname or name }}`. It treats `0` and `""` as false, so `{{ discount or "none" }}` prints `none` for a discount of `0`.

## Blocks

`{% if %}` keeps what sits between it and `{% endif %}` only when the condition is true, and `{% else %}` covers the other case:

```text
{% if total > 1000 %}Thank you for your large order.{% else %}Thank you for your order.{% endif %}
```

### Choose between several cases

There's no `switch` statement, but an `elif` chain does the same job. The first true branch wins:

```bash
cat > status.txt <<'EOF'
{% for o in orders -%}
{{ o.id }}: {% if o.status == "paid" -%}
  Paid, thank you
{%- elif o.status == "partial" -%}
  Part paid
{%- elif o.status in ["due", "overdue"] -%}
  Payment due
{%- else -%}
  Unknown status {{ o.status }}
{%- endif %}
{% endfor %}
EOF
cat > orders.json <<'EOF'
{
  "orders": [
    {"id": 1, "status": "paid"},
    {"id": 2, "status": "overdue"},
    {"id": 3, "status": "lost"}
  ]
}
EOF
curl -F template=@status.txt -F data=@orders.json \
  http://localhost:9027/v1/render -o out.txt
cat > expected.txt <<'EOF'
1: Paid, thank you
2: Payment due
3: Unknown status lost
EOF
test "$(cat out.txt)" = "$(cat expected.txt)"
```

When each case only maps a code to a label, an object literal is shorter. A code that isn't in the object is undefined, so `default` catches it:

```text
{{ {"paid": "Paid", "due": "Payment due"}[status] | default("Unknown") }}
```

## What counts as true

| Value | In a condition |
|---|---|
| `false`, `null`, `0`, `0.0` | False |
| `""`, `[]`, `{}` | False |
| Any other number, text, list or object | True |
| A missing value | An error, unless you test it with `is defined` or use `default` |

So `{% if items %}` is true for a list with at least one item. A missing value stops the render instead of counting as false, so a typo can't hide a block (see [missing values are errors](syntax.md#missing-values-are-errors)).

## Combine conditions

| Write | True when |
|---|---|
| `a and b` | Both are true |
| `a or b` | Either is true |
| `not a` | `a` is false |
| `(a or b) and not c` | Brackets group as in arithmetic |
| `0 < qty <= 10` | Comparisons chain: `qty` is above 0 and at most 10 |

`not` binds tighter than `and`, and `and` tighter than `or`, so `a or b and c` means `a or (b and c)`. Add brackets when you mean something else.

The comparisons are `==`, `!=`, `<`, `<=`, `>` and `>=`. Text compares letter by letter with capitals sorting before small letters, so `"Zoe" < "adam"` is true. Text and a number are never equal, which makes `"10" == 10` false. If your data holds numbers as text, convert them first with `| int` or `| float`.

```bash
cat > combine.txt <<'EOF'
{% for c in customers -%}
{{ c.name }}: {% if c.vip and not c.blocked -%}
  VIP
{%- elif c.orders > 10 or c.spent >= 5000 -%}
  Regular
{%- else -%}
  New
{%- endif %}
{%- if 0 < c.orders <= 2 %}, first orders{% endif %}
{% endfor %}
EOF
cat > customers.json <<'EOF'
{
  "customers": [
    {"name": "Ada", "vip": true, "blocked": false, "orders": 30, "spent": 9000},
    {"name": "Bo", "vip": true, "blocked": true, "orders": 2, "spent": 6000},
    {"name": "Cy", "vip": false, "blocked": false, "orders": 1, "spent": 20}
  ]
}
EOF
curl -F template=@combine.txt -F data=@customers.json \
  http://localhost:9027/v1/render -o out.txt
cat > expected.txt <<'EOF'
Ada: VIP
Bo: Regular, first orders
Cy: New, first orders
EOF
test "$(cat out.txt)" = "$(cat expected.txt)"
```

## Tests

`is` asks a question about a value, and `is not` asks the opposite: `{% if notes is not empty %}`.

| Test | True for |
|---|---|
| `empty` | `null`, a missing value, `""`, `[]` and `{}`. Not `0` or `false` |
| `defined`, `undefined` | A value that exists, or doesn't. `a.b is defined` checks one level |
| `none` | `null` |
| `string`, `number`, `boolean` | Text, any number, `true` or `false` |
| `integer` (or `int`), `float` | A whole number, a number with a decimal point |
| `sequence`, `mapping`, `iterable` | A list, an object, anything a loop can walk |
| `odd`, `even`, `divisibleby(n)` | Whole numbers |
| `startingwith(s)`, `endingwith(s)` | Text that starts or ends with `s` |
| `lower`, `upper` | Text in small or capital letters |
| `in(list)` | A value in the list, the same as `x in list` |
| `true`, `false` | Exactly `true` or `false`, not `1` or `0` |
| `eq`, `ne`, `lt`, `le`, `gt`, `ge` | Comparisons, mostly for `selectattr`: `gt(5)` |
| `sameas(x)` | The same value as `x` |

### empty

`empty` is the test for "nothing to show". It's true for `null`, a missing value, empty text, an empty list and an empty object, and false for everything else, including `0` and `false`:

```bash
cat > empty.txt <<'EOF'
{% for k in ["note", "tags", "meta", "count", "flag", "gone"] -%}
  {{ k }}={{ "empty" if values[k] is empty else "set" }} {% endfor %}
{% if note is not empty %}Note: {{ note }}{% else %}No note{% endif %}
EOF
cat > values.json <<'EOF'
{
  "values": {"note": "", "tags": [], "meta": {}, "count": 0, "flag": false},
  "note": null
}
EOF
curl -F template=@empty.txt -F data=@values.json \
  http://localhost:9027/v1/render -o out.txt
first='note=empty tags=empty meta=empty count=set flag=set gone=empty '
test "$(cat out.txt)" = "$(printf '%s\nNo note' "$first")"
```

Here's how it compares with the other ways to check:

| Value | `if x` | `x is empty` | `x is none` | `x is defined` |
|---|---|---|---|---|
| Missing | Error | True | False | False |
| `null` | False | True | True | True |
| `""`, `[]`, `{}` | False | True | False | True |
| `0`, `false` | False | False | False | True |
| `"text"`, `[1]`, `5` | True | False | False | True |

So `{% if amount is not empty %}` prints a block for an amount of `0`, where `{% if amount %}` would hide it. `x is empty` accepts a missing `x` but not a missing parent, so `customer.phone is empty` still fails when `customer` itself is missing.

### Type tests

Type tests help when a value can take more than one shape, like one email or a list of them:

```bash
cat > types.txt <<'EOF'
{% for v in values -%}
  {% if v is none %}null
  {%- elif v is boolean %}yes/no
  {%- elif v is number %}number
  {%- elif v is string %}text
  {%- elif v is mapping %}object
  {%- elif v is sequence %}list
  {%- endif %} {% endfor %}
EOF
curl -F template=@types.txt \
  -F 'data={"values":[null,true,4.5,"a",{"k":1},[1]]}' \
  http://localhost:9027/v1/render -o out.txt
grep -x 'null yes/no number text object list ' out.txt
```

If your data can mix them, test `boolean` before `number`, and `string` before `sequence`.

## Membership

`in` and `not in` ask whether a value is part of another:

| Write | True when |
|---|---|
| `"EU" in regions` | The list `regions` holds `"EU"` |
| `"ltd" in name \| lower` | The text holds `ltd` |
| `"vat" in totals` | The object `totals` has a key `vat` |
| `country not in ["KE", "UG"]` | The value isn't in the list |

```bash
cat > member.txt <<'EOF'
{% for c in clients -%}
  {{ c.name }}: {{ "EU" if c.country in ["FR", "DE", "IT"]
                   else "outside the EU" }}
  {{- ", company" if "ltd" in c.name | lower }}
  {{- ", VAT" if "vat" in c.totals }}
{% endfor %}
EOF
cat > clients.json <<'EOF'
{
  "clients": [
    {"name": "Acme Ltd", "country": "DE", "totals": {"vat": 19}},
    {"name": "Ada", "country": "KE", "totals": {}}
  ]
}
EOF
curl -F template=@member.txt -F data=@clients.json \
  http://localhost:9027/v1/render -o out.txt
cat > expected.txt <<'EOF'
Acme Ltd: EU, company, VAT
Ada: outside the EU
EOF
test "$(cat out.txt)" = "$(cat expected.txt)"
```

`"ltd" in c.name | lower` works without brackets because the filter binds tighter than `in`.

## Conditions in loops

A loop can filter its items, and has its own `else` for an empty list:

```text
{% for l in lines if l.qty > 0 %}{{ l.name }}{% else %}No lines{% endfor %}
```

`loop.first`, `loop.last` and `loop.index` let a condition depend on the position, as in `{% if not loop.last %}, {% endif %}`. To filter a list once and reuse it, write `{% set open = tasks | rejectattr("done") | list %}`. To count or total the items that pass a test, see [Calculations](calculations.md#counts).

## Conditions in Office templates

In a DOCX, ODT, XLSX, ODS, PPTX or ODP template, `{% if %}` follows the same placement rules as `{% for %}`, so where you put its opening and closing tags decides what the block covers.

| Where the opening and closing tags sit | What a false condition removes |
|---|---|
| In the same paragraph | The text between them |
| In the same table row | That row |
| In different paragraphs | Those paragraphs, and everything between |
| In rows of a sheet | Those rows |
| On different slides | Those slides |

When a block spans several paragraphs, rows or slides, `{% elif %}` and `{% else %}` each need one of their own. The paragraph holding `{% endif %}` belongs to the last branch, along with any text after the tag. See [Office formats](office.md#what-a-block-repeats) for the details and the error messages.

This DOCX keeps or drops two paragraphs, the heading and the line below it:

```bash
mkdir t && cd t && unzip -q ../report.docx
export OPEN='{% if members %}Members report'
export CLOSE='For {{ members | length }} members.{% endif %}'
perl -pi -e 's{Quarterly report}{$ENV{OPEN}}' word/document.xml
perl -pi -e 's{A document with no tags in it.}{$ENV{CLOSE}}' word/document.xml
zip -qr ../members.docx . && cd ..

curl -F template=@members.docx -F 'data={"members":["Ada","Bo"]}' \
  http://localhost:9027/v1/render -o kept.docx
unzip -p kept.docx word/document.xml | grep -o 'For 2 members.'

curl -F template=@members.docx -F 'data={"members":[]}' \
  http://localhost:9027/v1/render -o dropped.docx
test "$(unzip -p dropped.docx word/document.xml | grep -c 'Members report')" = 0
```

In a sheet, a condition across a row's cells keeps or drops that row:

```bash
mkdir t && cd t && unzip -q ../sheet.xlsx
perl -pi -e 's/Month/{% if header %}Month/; s/Sales/Sales{% endif %}/' \
  xl/worksheets/sheet1.xml
zip -qr ../maybe.xlsx . && cd ..

curl -F template=@maybe.xlsx -F 'data={"header":false}' \
  http://localhost:9027/v1/render -o out.xlsx
test "$(unzip -p out.xlsx xl/worksheets/sheet1.xml | grep -c 'Month')" = 0
unzip -p out.xlsx xl/worksheets/sheet1.xml | grep -q 'January'
```

An inline condition like `{{ "x" if c else "y" }}` prints inside the text it sits in and never removes anything around it.

## Remove an element

To remove a picture, a shape, a table or a chart when a condition is false, without wrapping it in block tags, see [remove an element](office.md#remove-an-element) in Office formats.
