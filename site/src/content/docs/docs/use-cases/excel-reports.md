---
title: Excel reports from JSON
description: Fill an Excel or ODS report template from JSON, with a row per record, SUM totals that grow with the rows, one sheet per group, and a PDF of the result.
sidebar:
  order: 3
---

A report usually has a fixed header, a row per record and totals underneath. Build that sheet in Excel or LibreOffice Calc with its formats, widths and formulas, type tags in the cells, and send the records as JSON. The layout and the formulas stay in the workbook, not in your code.

## The template

The sample `sales.xlsx` has one sheet with a title, a header row, a row of tags and a total row:

| Row | A | B | C | D |
|---|---|---|---|---|
| 1 | `Sales report, {{ month }}` | | | |
| 2 | `Region` | `Units` | `Revenue` | |
| 3 | `{% for r in rows %}{{ r.region }}` | `{{ r.units }}` | `{{ r.revenue }}` | `{% endfor %}` |
| 4 | `Total` | `=SUM(B3:B3)` | `=SUM(C3:C3)` | |

The loop's opening and closing tags sit in cells of row 3, so row 3 repeats once per record. Column C has a number format of `#,##0.00`, which every copy keeps.

## The data

```json
{
  "month": "November 2026",
  "rows": [
    {"region": "Nairobi", "units": 120, "revenue": 288000},
    {"region": "Mombasa", "units": 45, "revenue": 108000},
    {"region": "Kisumu", "units": 30, "revenue": 72000}
  ]
}
```

## Fill it

```bash test
cat > sales.json <<'EOF'
{
  "month": "November 2026",
  "rows": [
    {"region": "Nairobi", "units": 120, "revenue": 288000},
    {"region": "Mombasa", "units": 45, "revenue": 108000},
    {"region": "Kisumu", "units": 30, "revenue": 72000}
  ]
}
EOF
curl -F template=@sales.xlsx -F data=@sales.json \
  http://localhost:9027/v1/render -o report.xlsx

unzip -p report.xlsx xl/worksheets/sheet1.xml > sheet.xml
grep -o '<c r="C5"[^>]*><v>72000</v>' sheet.xml
grep -o '<f>SUM(C3:C5)</f>' sheet.xml
```

Here's what happened to the sheet:

| In the template | In the result |
|---|---|
| Row 3, the loop | Rows 3, 4 and 5, one per record |
| `{{ r.units }}`, alone in its cell | A real number, so formats and formulas work on it |
| Row 4, `=SUM(C3:C3)` | Row 6, `=SUM(C3:C5)`, because a range grows over every copy |
| Column D, holding `{% endfor %}` | Empty |

The workbook is set to recalculate when it's opened, so the totals show 195 units and 468,000.00. A cell with text around its tag, like the title, stays text.

## Get a PDF of the report

Add `steps=convert` and LibreOffice prints the filled sheet to PDF:

```bash test
cat > sales.json <<'EOF'
{"month": "November 2026",
 "rows": [{"region": "Nairobi", "units": 120, "revenue": 288000}]}
EOF
curl -F template=@sales.xlsx -F data=@sales.json -F steps=convert \
  http://localhost:9027/v1/render -o report.pdf
head -c 5 report.pdf
```

A print area moves with the rows too, so a converted sheet shows every copy. Set the page size, orientation and print area in the template, the same way you would before printing it by hand.

## ODS

An ODS template works the same way. This converts the sample to ODS and fills it:

```bash test
curl -F files=@sales.xlsx -F to=ods \
  http://localhost:9027/v1/convert -o sales.ods
curl -F template=@sales.ods \
  -F 'data={"month":"November 2026","rows":[{"region":"Kisumu","units":30,"revenue":72000}]}' \
  http://localhost:9027/v1/render -o report.ods
unzip -p report.ods content.xml | grep -o 'of:=SUM(\[.C3:.C3\])'
```

The difference is that cells stay text in an ODS, so a number arrives as text. LibreOffice reads text that looks like a number as one in arithmetic, but `SUM` skips text cells, so do the sums in your data or with arithmetic in the formula. See [ODS](/docs/templates/office/#ods).

## Variations

| To | Do this |
|---|---|
| Fill a real date | `{{ r.date \| excel_date }}` alone in a cell formatted as a date. A date string would arrive as text. See [`excel_date`](/docs/templates/filters/#excel_date) |
| Give each group its own sheet | `{% sheet for s in sites %}` in any cell, and `{{ s.name }}` as the sheet's name. See [repeating a sheet](/docs/templates/office/#repeating-a-sheet) |
| Put a column per month | `{% col for m in months %}{{ m }}{% endcol %}`, in XLSX only. See [repeating columns](/docs/templates/office/#repeating-columns) |
| Add subtotals per group | A row loop inside another, with `SUBTOTAL` so the grand total doesn't count them twice. See [rows inside rows](/docs/templates/office/#rows-inside-rows) |
| Hide a column or a row | `{{ drop("column") if not show_costs }}` in the column. Formulas that point past it follow. See [in sheets](/docs/templates/office/#in-sheets) |
| Add a chart from data | `{{ rows \| chart(type="column", x="region", y=["revenue"]) }}`. See [chart](/docs/templates/rich-content/#chart) |
| Allow blanks in sparse data | Send `undefinedAsEmpty=true`, and a cell that holds only a missing value is left empty. See [missing values](/docs/templates/syntax/#print-every-missing-value-as-nothing) |

## What doesn't follow the rows

Charts, conditional formatting, data validations and hyperlinks that already point at looped rows don't move with them, and a formula on another sheet that points at looped rows isn't re-based. Keep totals on the same sheet as the rows they add up, or use the `chart` filter, which builds the chart from the data. See [not supported](/docs/templates/office/#not-supported).

## Next

- [Office formats](/docs/templates/office/#xlsx) covers everything a sheet can do.
- [Calculations](/docs/templates/calculations/) shows totals, averages and counts worked out in the template.
- [Filters](/docs/templates/filters/#number) covers `number`, `currency` and `date`.
