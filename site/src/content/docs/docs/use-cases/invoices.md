---
title: Generate invoices from a Word template
description: Fill a Word invoice template from JSON and get a PDF back, with a row per line item, totals, dates, a payment QR code, PDF/A and Factur-X.
sidebar:
  order: 1
---

An invoice has a fixed layout and data that changes on every one. Design the invoice in Word, type tags where the values go, and send the data as JSON. The layout stays in the `.docx` file, so changing a logo or a column is an edit in Word, not a change to your code.

## The template

The [sample invoice](/samples/invoice-template.docx) is an ordinary Word file. These are the tags typed into it:

```text
{{ seller.name }}
{{ seller.address }}
Invoice {{ number }}
Issued {{ issued | date("%d %B %Y") }}

Billed to
{{ customer.name }}
{{ customer.address }}
```

The items table has one row of tags. The opening tag sits in its first cell and the closing tag in its last, so the row repeats once per item:

| Item | Qty | Unit price | Amount |
|---|---|---|---|
| `{% for item in items %}{{ item.name }}` | `{{ item.qty }}` | `{{ item.price \| currency(currency) }}` | `{{ (item.qty * item.price) \| currency(currency) }}{% endfor %}` |

Under the table come the total, a QR code for payment and the due date:

```text
Total due  {{ total | currency(currency) }}
{{ payment_url | barcode("qr", size=68) }}
Pay by {{ due | date("%d %B %Y") }}
```

## The data

The JSON holds the values, with one object per line item:

```json
{
  "seller": { "name": "Mawingu Coffee Co.", "address": "12 Example Road, Nairobi" },
  "number": "INV-2041",
  "issued": "2026-11-06",
  "due": "2026-12-06",
  "currency": "KES",
  "customer": { "name": "Amani Otieno", "address": "48 Sample Avenue, Mombasa" },
  "items": [
    { "name": "Single-origin beans, 1 kg", "qty": 12, "price": 2400 },
    { "name": "Paper filters, box of 100", "qty": 6, "price": 950 },
    { "name": "Delivery to Mombasa", "qty": 1, "price": 1800 }
  ],
  "total": 36300,
  "payment_url": "https://pay.example.com/INV-2041"
}
```

## Fill it and convert it

```bash test
curl -F template=@invoice.docx -F "data=<invoice.json" -F steps=convert \
  -F outputFilename=INV-2041 \
  http://localhost:9027/v1/render -D headers.txt -o INV-2041.pdf
head -c 5 INV-2041.pdf
grep -i 'filename="INV-2041.pdf"' headers.txt
```

You get a PDF with a row for each item and a total of KES 36,300.00. Without `steps=convert`, you get the filled `.docx` back instead, which is useful when someone needs to edit the invoice before it goes out.

## What each tag does

| Tag | What it does |
|---|---|
| `{{ customer.name }}` | Prints a value. A dot reads a key inside an object |
| `{% for item in items %}` … `{% endfor %}` | Repeats the table row. An empty list removes it |
| `currency(currency)` | Formats a number as money, with the code from the data |
| `(item.qty * item.price)` | Calculates the amount. The brackets matter, because a filter binds tighter than `*` |
| `date("%d %B %Y")` | Prints `2026-11-06` as `06 November 2026` |
| `barcode("qr", size=68)` | Draws the payment link as a QR code |

A missing value stops the render with a `422` that names the tag, so a typo can't reach a customer as a blank. See [missing values are errors](/docs/templates/syntax/#missing-values-are-errors).

## Calculate the total in the template

The sample sends `total` in the data. You can work it out in the template instead, with a [namespace](/docs/templates/calculations/#totals-of-a-calculation) that adds up each line. This replaces the total's tag and sends the data without `total`:

```bash test
mkdir t && cd t && unzip -q ../invoice.docx
export TAG='{% set ns = namespace(sum=0) %}{% for i in items %}'
TAG+='{% set ns.sum = ns.sum + i.qty * i.price %}{% endfor %}'
TAG+='{{ ns.sum | currency(currency) }}'
perl -pi -e 's/\{\{ total \| currency\(currency\) \}\}/$ENV{TAG}/' \
  word/document.xml
zip -qr ../summed.docx . && cd ..

jq 'del(.total)' invoice.json > no-total.json
curl -F template=@summed.docx -F "data=<no-total.json" \
  http://localhost:9027/v1/render -o out.docx
unzip -p out.docx word/document.xml | grep -o 'KES 36,300.00'
```

Tax, discounts and amounts in words work the same way. The [worked invoice](/docs/templates/calculations/#worked-example-an-invoice) shows subtotals per group, tax and `words`.

## One invoice per order

[Batch rendering](/docs/templates/batch/) fills the template once per element of a list and returns a ZIP. Each render reads its element as `item`, so the template uses `{{ item.number }}` where it used `{{ number }}`:

```bash test
mkdir t && cd t && unzip -q ../invoice.docx
perl -pi -e 's/\{\{ number \}\}/{{ item.number }}/g' word/document.xml
zip -qr ../per-order.docx . && cd ..

jq '. + {orders: [{number: "INV-2041"}, {number: "INV-2042"}]}' \
  invoice.json > orders.json
curl -F template=@per-order.docx -F "data=<orders.json" -F splitBy=orders \
  -F 'batchName={{ item.number }}' -F steps=convert \
  http://localhost:9027/v1/render/batch -o invoices.zip
unzip -l invoices.zip | grep -o 'INV-2042.pdf'
```

In a real batch, put everything that changes per invoice in each element, like the customer and the items, and read it through `item`.

## PDF/A and Factur-X

For archiving, `steps=pdfa` converts straight to PDF/A. For an electronic invoice, the `factur-x` step embeds the invoice XML you send as `facturxXml`:

```bash test
cat > factur-x.xml <<'EOF'
<rsm:CrossIndustryInvoice
  xmlns:rsm="urn:un:unece:uncefact:data:standard:CrossIndustryInvoice:100"/>
EOF
curl -F template=@invoice.docx -F "data=<invoice.json" \
  -F steps=pdfa,factur-x -F pdfa=PDF/A-3b -F facturxXml=@factur-x.xml \
  http://localhost:9027/v1/render -o e-invoice.pdf
grep -aq pdfaid e-invoice.pdf
grep -aq factur-x.xml e-invoice.pdf
```

The service doesn't check the XML against a Factur-X schema, so you generate and validate it yourself. `facturxConformanceLevel` sets the profile, and `EN16931` is the default.

## Other variations

| To | Do this |
|---|---|
| Print amounts and dates in another language | Send `lang=fr`, and `currency` and `date` follow French conventions. See [translations and settings](/docs/templates/translations/) |
| Upload the template once | [Store it](/docs/guides/stored-templates/) and send `templateId` instead of the file |
| Add the company logo from data | `{{ logo \| image(width=120) }}`, with the image's bytes in the data. See [rich content](/docs/templates/rich-content/#image) |
| Hide a discount column when there's no discount | `{{ drop("column") if not show_discount }}` in the column's header cell. See [remove an element](/docs/templates/office/#remove-an-element) |
| Protect the PDF with a password | Add `encrypt` as the last step, with `userPassword`. See [the pipeline](/docs/guides/pipeline/#steps) |
| Get the result later, by webhook | Add `X-Rusticdoc-Webhook-Url`. See [webhooks](/docs/guides/webhooks/) |

## Next

- [Office formats](/docs/templates/office/#table-rows) explains where loop tags go in a table.
- [Filters](/docs/templates/filters/) covers `currency`, `number` and `date` in full.
- [Calculations](/docs/templates/calculations/) covers totals, tax, rounding and due dates.
- [Validate a template](/docs/templates/validate/) finds every mistake in a template before you use it.
