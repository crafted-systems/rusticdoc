# Office formats

<!-- Generated from site/src/content/docs/docs/templates/office.md by site/scripts/generate-skills.mjs. Do not edit. -->

Where to put loops and conditions in DOCX, ODT, XLSX, ODS, PPTX and ODP templates, and what each format supports.

A template is an ordinary Word, LibreOffice, Excel or PowerPoint file with tags typed into its text. You don't need an add-in, a field or a special style. What matters is where you put them, so that a loop repeats the row, paragraph or slide you meant.

## Typing tags in a word processor

Word and other editors change text as you type, and the service undoes the changes that matter.

Word often splits text into several runs because of spell-check marks, edits or formatting. A tag split this way still works, because the runs of a paragraph are joined before tags are read. The joined text takes the formatting of the run it starts in, so format the whole tag or its first character.

Curly quotes are read as straight ones, so `{{ “x” | default(‘y’) }}` works. A non-breaking space in a tag counts as a normal space, and a line break typed inside one as whitespace. Entities like `&quot;` in the file's XML are decoded.

## What a block repeats

There are no special loop tags, apart from the sheet loop. What repeats depends on where a block's opening and closing tags sit, for `{% for %}` and `{% if %}` alike.

| Where the opening and closing tags sit | What repeats |
|---|---|
| In the same paragraph | The text between them |
| In the same table row, in any cells | That row |
| In two rows of one table | Those rows, and everything between |
| In different paragraphs | Those paragraphs, and everything between |
| On different slides (PPTX) or pages (ODP) | That run of slides |
| In different shapes of one slide (PPTX) | That slide |
| On one slide, with `\| shift(…)` (PPTX) | Those shapes, each copy moved by a step |
| In rows of a sheet (XLSX, ODS) | Those rows |
| `{% sheet for s in sites %}` in any cell of a sheet (XLSX, ODS) | That sheet, once per item. See [repeating a sheet](#repeating-a-sheet) |

Tags in different parts of a package can't form a block, like a `{% for %}` in the body with its `{% endfor %}` in a header, or tags on two sheets of an XLSX. That fails with `a block cannot span parts`. PPTX slides are the exception, as the table shows.

### Table rows

Put the opening tag in the row's first cell and the closing tag in its last. That's what the sample invoice does, and its middle cells hold no block tags:

| Item | Qty | Unit price | Amount |
|---|---|---|---|
| `{% for item in items %}{{ item.name }}` | `{{ item.qty }}` | `{{ item.price \| currency(currency) }}` | `{{ (item.qty * item.price) \| currency(currency) }}{% endfor %}` |

```bash
curl -F template=@invoice.docx -F "data=<invoice.json" \
  http://localhost:9027/v1/render -o out.docx
unzip -p out.docx word/document.xml > out.xml
grep -o 'Single-origin beans' out.xml
grep -o 'Delivery to Mombasa' out.xml
grep -o 'KES 28,800.00' out.xml
```

An empty list removes the row. Tags in two rows repeat both rows as a group, which suits a line with a description under it.

### Paragraphs

Put the opening tag in the first paragraph you want repeated, and the closing tag in the last.

```text
{% for i in items %}Item {{ i.name }}
Some text about it.
Last line{% endfor %}
```

You get all three paragraphs once per item. To start each item on a new page in a DOCX or ODT, add a paragraph holding `{{ "" | pagebreak }}` before the closing tag.

A paragraph that holds only a block tag stays in each copy as an empty paragraph. To avoid the blank lines, write the tags in paragraphs that also hold text, as above.

### A loop around a table

Tags in the paragraphs before and after a table repeat the whole table along with those two paragraphs. Each of them holds only a tag, so every copy gets two empty paragraphs.

### Text in one paragraph

With both tags in one paragraph, only the text between them repeats, and it stays one paragraph:

```text
{% for t in tags %}{{ t }}{% if not loop.last %}, {% endif %}{% endfor %}
```

### else and elif

`{% else %}` and `{% elif %}` follow the same rule. When a block spans rows or paragraphs, each of them needs a row or paragraph of its own, and its branch starts there. The closing tag's row or paragraph belongs to the last branch.

```text
{% if vip %}Thank you for being a member.
{% else %}Join us today.
Terms apply.{% endif %}
```

If the block spans paragraphs and you put `{% else %}` in the same paragraph as its opening or closing tag, the template is refused:

```text
'{% else %}' (word/document.xml paragraph 1) needs its own paragraph between '{% if a %}' (word/document.xml paragraph 1) and '{% endif %}' (word/document.xml paragraph 2)
```

You can nest a row loop in a paragraph loop, a row loop in another row loop, or an inline loop in a row.

## Remove an element

A block needs two tags, and it only repeats rows, paragraphs and slides. To take out one thing on a condition, like a table column, a whole table from inside it, a picture or a sheet, use `drop`. It prints nothing and removes the element that holds the tag:

```text
{{ drop("column") if not show_discount }}
{% if items | length < 6 %}{{ drop("table") }}{% endif %}
{{ drop("paragraph", 3) if not terms }}
```

`drop` is an ordinary expression, so you can put the condition in the same tag with `if`, or around it in a block. When the condition is false, nothing is removed.

The first argument names the element, and the tag's position says which one:

| Element | Where the tag sits | DOCX | ODT | XLSX | ODS | PPTX | ODP |
|---|---|---|---|---|---|---|---|
| `paragraph` | In the paragraph | Yes | Yes | Refused | Refused | Yes | Yes |
| `row` | In any cell of the table row or sheet row | Yes | Yes | Yes | Yes | Yes | Yes |
| `column` | In any cell of the column | Yes | Yes | Yes | Yes | Yes | Yes |
| `table` | In any cell of the table | Yes | Yes | Refused | Refused | Yes | Yes |
| `picture` | In the picture's alt text | Yes | Yes | Yes | Yes | Yes | Yes |
| `chart` | In the chart's alt text | Yes | Yes | Yes | Yes | Yes | Yes |
| `shape` | In the shape's alt text, or in its text | Yes | Yes | Alt text only | Yes | Yes | Yes |
| `slide` | Anywhere on the slide | Refused | Refused | Refused | Refused | Yes | Yes |
| `sheet` | In any cell of the sheet | Refused | Refused | Yes | Yes | Refused | Refused |

A shape is any drawn object. That covers a text box, a rectangle or a group, and also a picture, chart or table frame, so `drop("shape")` removes whichever object holds the tag.

The sample document `drop.docx` has a tag for each element, each on a condition that its data sets. Its items table has a discount column whose header cell holds `Discount{{ drop("column") if not discount }}`:

```bash
jq '.discount = false' drop.json > data.json
curl -F template=@drop.docx -F "data=<data.json" \
  http://localhost:9027/v1/render -o out.docx
unzip -p out.docx word/document.xml > out.xml
! grep -q 'Discount' out.xml
grep -o 'Amount' out.xml
```

### Paragraphs and rows

`drop("paragraph", n)` removes the tag's paragraph and the `n - 1` after it, and `drop("row", n)` removes the tag's row and the `n - 1` below it. Without a count it's one. The other elements take no count.

Inside a loop each copy has its own tag, so a drop removes only that copy:

```bash
mkdir t && cd t && unzip -q ../invoice.docx
export TAG='{{ item.qty }}{{ drop("row") if item.qty == 1 }}'
perl -pi -e 's/\{\{ item\.qty \}\}/$ENV{TAG}/' word/document.xml
zip -qr ../tagged.docx . && cd ..

curl -F template=@tagged.docx -F "data=<invoice.json" \
  http://localhost:9027/v1/render -o out.docx
unzip -p out.docx word/document.xml > out.xml
grep -o 'Paper filters, box of 100' out.xml
! grep -q 'Delivery to Mombasa' out.xml
```

The file is kept valid after a drop:

- A table cell, text box, header or slide text left without a paragraph gets an empty one, because Word, PowerPoint and LibreOffice need it.
- A Word paragraph that ends a section loses its text but keeps its section settings.
- A table left with no rows is removed, and so is a list item left empty in an ODT.
- A row that holds part of a cell merged down across rows that stay can't be removed: `cannot remove a row that holds part of a cell merged down across rows that stay`.

### Columns

`drop("column")` removes that column's cell from every row, and the column from the table's grid. A Word table with a fixed width, a PowerPoint table and an ODP table frame get narrower by the column's width. Everywhere else, the other columns share the width.

A column that a merged cell spans can't be removed, and you get `drop("column") cannot remove column 3: a cell merged across columns spans it`. Either unmerge the cells or put the merged cell's text in a column that stays. The sample invoice's total row spans its first three columns, so its unit price column can't go:

```bash
mkdir t && cd t && unzip -q ../invoice.docx
perl -pi -e 's/>Unit price</>Unit price{{ drop("column") }}</' word/document.xml
zip -qr ../tagged.docx . && cd ..

curl -F template=@tagged.docx -F "data=<invoice.json" \
  http://localhost:9027/v1/render -o out.docx
```

### Pictures, charts and shapes

Type the tag in the object's alt text. In Word, Excel and PowerPoint, right-click the object and choose View Alt Text. In LibreOffice, choose Description, which has a title and a description, and either field works.

```text
{{ drop("picture") if not show_logo }}
```

Alt text can hold other tags too, like `Logo of {{ company }}`. Those print into the alt text, while the `drop` prints nothing. A block in alt text has to open and close there, or you get `a block in alt text must open and close in the same alt text`.

A shape's tag can also go in the text it holds, like `Stamp{{ drop("shape") if paid }}` in a text box. Text in a drawing isn't read in an XLSX, so use the shape's alt text there.

LibreOffice leaves out a chart's alt text when it saves a PPTX or an XLSX, so type the tag in PowerPoint or Excel, or keep the template in ODP or ODS.

The sample deck has a tag for each element. This request leaves out its picture and its appendix slide:

```bash
jq '.logo = false | .appendix = false' drop.json > data.json
curl -F template=@drop.pptx -F "data=<data.json" \
  http://localhost:9027/v1/render -o out.pptx
test "$(unzip -Z1 out.pptx | grep -c '^ppt/slides/slide[0-9]*\.xml$')" = 2
! unzip -p out.pptx ppt/slides/slide1.xml | grep -q '<p:pic>'
! unzip -Z1 out.pptx | grep -q '^ppt/media/'
```

### In sheets

Rows and columns are removed after loops have numbered the rows, and the cells below or to the right move up or left. Every reference to a moved cell follows it. That covers formulas on the sheet and on other sheets, defined names like the print area, charts, conditional formats, data validations, hyperlinks, merged cells, column widths, tables and the anchors of pictures. In an ODS, formulas, print ranges and named ranges follow.

A range loses the rows and columns that went, so `=SUM(D2:D5)` becomes `=SUM(D2:D4)` when one of its rows goes. If all of a range's rows went, it becomes the row before them, as it does after a loop over an empty list. A reference that would point at a removed cell is refused instead of being left broken:

```text
drop("column") in sheet "Summary" removes cell C4, which the formula in D6 (C4*2) points at; remove that reference or keep the cells
```

A whole-column reference like `=SUM(C:C)` is refused when a column goes, and so is a whole-row one like `=SUM(3:3)` when a row goes. Write the range with rows instead. Merged cells that span a removed column are refused, while merged cells over removed rows shrink. A column of an Excel table is removed from the table too, but the table's header row, or all of its rows, can't go.

The sample workbook drops its discount column and the row of each item marked hidden, and the totals follow:

```bash
jq '.discount = false' drop.json > data.json
curl -F template=@drop.xlsx -F "data=<data.json" \
  http://localhost:9027/v1/render -o out.xlsx
unzip -p out.xlsx xl/worksheets/sheet1.xml > sheet.xml
! grep -q 'Discount' sheet.xml
grep -o '<f[^>]*>SUM(C2:C4)</f>' sheet.xml
grep -o '<f[^>]*>C5\*2</f>' sheet.xml
```

`drop("sheet")` removes the sheet with its drawings, charts, tables and comments. In a [sheet loop](#repeating-a-sheet) it removes only the copy that holds it. These are refused:

- Removing every sheet: `drop("sheet") removes every sheet, and a workbook needs one`.
- Removing a sheet that a formula or a defined name on another sheet points at.
- In an XLSX, a tag in the alt text of a picture on a sheet that a sheet loop repeats, because a drawing is read once for all copies.

### Slides

`drop("slide")` removes the slide with its speaker notes and anything only it used, the same as a slide loop over an empty list. You can't remove every slide: `drop("slide") removes every slide, and a presentation needs one`.

### What goes with an element

Pictures, chart parts, embedded objects, relationships, content types and ODF manifest entries that only the removed element used go with it, so the file opens without a repair. Anything another element still uses stays.

### Mistakes

An element written in the tag is checked when the template is compiled, and [validation](validate.md) reports the tag and where it sits. A name that comes from data, as in `drop(kind)`, is checked when the file is put together.

| Mistake | Error |
|---|---|
| An element the format doesn't have | `a DOCX has no slides; drop("slide") works in PPTX and ODP` |
| `row` or `column` outside a table | `drop("row") must sit in a table cell` |
| `picture` in body text | `drop("picture") must sit in the alt text (title or description) of a picture` |
| `picture` in a chart's alt text | `sits in a chart, not a picture` |
| A row or paragraph in alt text | `sits in alt text, where drop("row") has no row to remove` |
| An unknown element | `"cell" is not something drop removes; use paragraph, row, column, table, picture, chart, shape, slide, sheet` |
| A count on another element | `drop("table") takes no count; only paragraph and row do` |
| A count past the last paragraph | `drop("paragraph", 3) needs 3 paragraphs in a row, but only 2 come before a table` |

```bash
mkdir t && cd t && unzip -q ../invoice.docx
perl -pi -e 's/>Billed to</>Billed to{{ drop("slide") }}</' word/document.xml
zip -qr ../tagged.docx . && cd ..

curl -F template=@tagged.docx -F "data=<invoice.json" \
  http://localhost:9027/v1/render -o out.docx
```

## DOCX

Tags are read in the body, headers, footers, footnotes and endnotes, and in text boxes, but a block can't span a text box and the body. They aren't read in document properties, field codes, comments or attributes like a hyperlink's address.

A newline in a value becomes a line break.

## ODT

Tags are read in `content.xml` and in `styles.xml`, which holds headers and footers. Everything under [what a block repeats](#what-a-block-repeats) applies, and a newline in a value becomes a line break.

To try it with a DOCX, convert one first. The tags survive the conversion:

```bash
curl -F files=@invoice.docx -F to=odt \
  http://localhost:9027/v1/convert -o invoice.odt
curl -F template=@invoice.odt -F "data=<invoice.json" \
  http://localhost:9027/v1/render -o out.odt
unzip -p out.odt content.xml | grep -o 'KES 28,800.00'
```

Charts are refused in ODT.

## XLSX

Tags are read in the cells of every sheet, in shared strings or inline text. A sheet's name can hold `{{ }}` expressions too, as [repeating a sheet](#repeating-a-sheet) shows. Tags aren't read in headers and footers, comments, charts or drawings.

A cell that holds only `{{ expr }}` becomes a real number or boolean when the value is one, so totals and formats work, and a `null` gives an empty cell. If there's other text around the tag, the cell stays text. A newline in a value stays a newline character, and the cell needs wrap text to show it.

Tags in rows repeat those rows. Copied rows are renumbered, references in formulas move with them, and a range like `SUM(D3:D3)` grows over every copy. Merged cells follow their rows, and the workbook is set to recalculate when it's opened.

| Before | After three items |
|---|---|
| Row 3: `{% for i in items %}{{ i.n }}`, `{{ i.q }}`, `=B3*2`, `{% endfor %}` | Rows 3, 4 and 5, with `=B3*2`, `=B4*2` and `=B5*2` |
| Row 4: `=SUM(C3:C3)` | Row 6: `=SUM(C3:C5)` |

This example turns the header row of a small sheet into a row loop:

```bash
mkdir t && cd t && unzip -q ../sheet.xlsx
perl -pi -e 's/Month/{% for r in rows %}{{ r.month }}/;' \
  -e 's/Sales/{{ r.sales }}{% endfor %}/' xl/worksheets/sheet1.xml
zip -qr ../tagged.xlsx . && cd ..

cat > rows.json <<'EOF'
{
  "rows": [
    {"month": "March", "sales": 90},
    {"month": "April", "sales": 120}
  ]
}
EOF
curl -F template=@tagged.xlsx -F data=@rows.json \
  http://localhost:9027/v1/render -o out.xlsx
unzip -p out.xlsx xl/worksheets/sheet1.xml > sheet.xml
grep -o '<c r="B1"[^>]*><v>90</v>' sheet.xml
grep -o '<c r="B2"[^>]*><v>120</v>' sheet.xml
grep -o '<c r="A3"[^>]*>' sheet.xml
```

The print area and other defined names move with the rows too, so a printed or converted sheet shows every copy.

### Rows inside rows

A row loop can sit inside another row block, like items inside groups. Put the inner loop's opening and closing tags in different cells of its row, because in one cell they repeat text in that cell.

| Row | Cells |
|---|---|
| 3 | `{% for g in groups %}{{ g.name }}` |
| 4 | `{% for i in g.items %}{{ i.name }}`, `{{ i.qty }}`, `{{ i.price }}`, `=B4*C4`, `{% endfor %}` |
| 5 | `Subtotal`, `=SUBTOTAL(9,D3:D4)`, `{% endfor %}` |
| 6 | `Total`, `=SUBTOTAL(9,D3:D5)` |

Each group gets its header, its items and its subtotal. A formula's reference into the loop it sits in stays in its own copy, so each subtotal covers its own group, and an empty group's subtotal covers only its header row. From outside a loop, a range runs from the first copy to the last, so the total covers every group. `SUBTOTAL` skips the other `SUBTOTAL` cells in its range, which is why the total doesn't count the subtotals twice the way `SUM` would.

Start a range on the group's header row, as in `D3:D4`, rather than writing `D4:D4`, because LibreOffice may save a one-cell range like that as `D4`, which then names one row.

### Repeating columns

`{% col for d in days %}` repeats cells to the right, one copy for each item, and `{% endcol %}` ends the loop. Both tags go in cells of the same row. They can share a cell, which repeats that one cell, or sit in different cells, which repeats every cell from the first to the second. Cells to the right move along to make room.

| Row | Cells |
|---|---|
| 1 | `Name`, `{% col for d in days %}{{ d }}{% endcol %}`, `Total` |
| 2 | `{% for p in staff %}{{ p.name }}`, `{% col for h in p.hours %}{{ h }}{% endcol %}`, `=SUM(B2:B2)`, `{% endfor %}` |

With three days, row 1 becomes `Name`, `Mon`, `Tue`, `Wed`, `Total`. Each person's row gets a cell per entry in `hours`, and the sum moves to column E as `=SUM(B2:D2)`. A cell that holds only `{{ h }}` and the loop's tags is still a number cell. A list with no items takes the loop's cells out, and the cells to the right move left.

Formulas follow the copies:

| Where the formula is | What its references do |
|---|---|
| Inside the loop | They point at their own copy, so `=B1*C1` reads `=E1*F1` in the second copy |
| In the same row, outside the loop | A range grows over every copy, so `SUM(B2:B2)` becomes `SUM(B2:D2)`. A single cell points at the first copy |
| Inside the loop, pointing at another row | The column moves with the copy, so `=B2*B$1` reads `=C2*C$1` in the second copy. A fixed column like `$H$1` stays |

A formula cell can't hold a tag. To end the loop on a formula, put `{% endcol %}` in a cell of its own to the right of it. The loop then ends at the cell before the tag, and the tag's cell is left empty after the last copy:

| Cells in row 1 | After two days |
|---|---|
| `{% col for d in days %}{{ d.hours }}`, `{{ d.rate }}`, `=B1*C1`, `{% endcol %}` | `2`, `10`, `=B1*C1`, `3`, `20`, `=E1*F1` |

A row can hold several column loops side by side, and a column loop can sit in a row that a row loop repeats, as above. These are refused, and the error names the cell:

- A column loop inside another one: `column loops cannot nest`.
- Tags in different rows: `must sit in cells of the same row`.
- `{% col %}` in any other format, ODS included: `repeats columns, so it works in XLSX only`.
- More copies than a sheet has columns: `column loops fill more than 16,384 columns`.

Only the row that holds the loop moves. The rows above and below keep their columns, so keep any cell that another row points at to the left of the loop. Merged cells, column widths, conditional formats, tables and print areas don't move or widen with a column loop.

Column loops were added in 0.1.1.

### Repeating a sheet

Put `{% sheet for s in sites %}` in any cell of a sheet, and give the sheet a name with an expression, like `{{ s.name }}`. The sheet is copied once per item with `s` set, and each copy takes its name from the data. There's no closing tag, because the loop ends with the sheet.

The loop goes in a cell because Excel caps a sheet name at 31 characters and refuses `: \ / ? * [ ]`, so `{% for s in sites %}{{ s.name }}` can't be a name. To name the sheet, double-click its tab in Excel or LibreOffice and type `{{ s.name }}`, or text around it like `Site {{ s.code }}`.

| Sheet name | Cell A1 | Data | Sheets |
|---|---|---|---|
| `{{ s.name }}` | `{% sheet for s in sites %}Report for {{ s.name }}` | `{"sites":[{"name":"Paris"},{"name":"Lyon"}]}` | `Paris`, `Lyon` |

```bash
mkdir t && cd t && unzip -q ../sheet.xlsx
perl -pi -e 's/name="Sheet"/name="{{ r.month }}"/' xl/workbook.xml
export CELL='<c r="B2" t="inlineStr"><is><t>{{ r.sales }}</t></is></c>'
perl -pi -e 's/>Month</>{% sheet for r in rows %}Month</;' \
  -e 's/>January</>{{ r.month }}</;' \
  -e 's/<c r="B2" t="n"><v>120<\/v><\/c>/$ENV{CELL}/' \
  xl/worksheets/sheet1.xml
zip -qr ../tagged.xlsx . && cd ..

cat > rows.json <<'EOF'
{
  "rows": [
    {"month": "March", "sales": 90},
    {"month": "April", "sales": 120}
  ]
}
EOF
curl -F template=@tagged.xlsx -F data=@rows.json \
  http://localhost:9027/v1/render -o out.xlsx
unzip -p out.xlsx xl/workbook.xml | grep -o '<sheet [^>]*name="March"'
unzip -p out.xlsx xl/workbook.xml | grep -o '<sheet [^>]*name="April"'
unzip -p out.xlsx xl/worksheets/sheet2.xml |
  grep -o '<c r="B2"[^>]*><v>120</v>'
```

Names are made valid, so a forbidden character becomes `_`, a name longer than 31 characters is cut, and a name that another sheet or an earlier copy already has gets ` (2)`, ` (3)` and so on, as Excel does when it copies a sheet. Case doesn't count, so `paris` clashes with `Paris`, and an empty name becomes `Sheet`. If a looped sheet's name holds no expression, the first copy keeps the name and the others get ` (2)`, ` (3)` and so on.

Each copy is a whole sheet, so its rows, row loops, formulas and merged cells work as in any sheet. In an XLSX each copy also gets its own drawings, charts, tables and comments, and its own print area and other names that belong to the sheet. A chart or print area that names the sheet names the copy instead, while pictures are shared. A copied table is renamed, as in `Items_2`, because table names are unique in a workbook, and formulas in the copy follow the new name.

A formula on another sheet that names the template sheet, like `='{{ s.name }}'!B2`, points at the first copy.

An empty list removes the sheet. A workbook needs a sheet, so a list that removes every sheet fails with `the sheet loops left the workbook with no sheets`.

A sheet name can also hold an expression without a loop, to name one sheet from the data, like `{{ month }} sales`.

These are refused, and the error names the place:

- A second sheet loop in one sheet: `a sheet takes one sheet loop`.
- A sheet loop inside another block, like a row loop: `a sheet loop repeats the whole sheet, so no other block can hold it`.
- A statement in a sheet name: `a sheet name can hold only {{ }} expressions`.
- `{% sheet %}` in a DOCX, ODT, PPTX or ODP: `repeats a sheet, so it works in XLSX and ODS only`.

Copies count against the size limits of a render, so a loop over a very long list fails instead of building a file too large to open.

### Not supported

Sheet hyperlinks, data validations, conditional formatting and charts that point at looped rows don't move with them, and a formula on one sheet that points at looped rows on another isn't re-based. Page breaks aren't supported either, because a sheet has none.

A [column loop](#repeating-columns) moves only its own row, and it isn't available in ODS. A date string fills a cell as text, so for a real date use [`excel_date`](filters.md#excel_date) and format the cell as a date.

## ODS

Tags are read in `content.xml`, but not in headers and footers.

Tags in rows repeat those rows, and [rows inside rows](#rows-inside-rows) and [repeating a sheet](#repeating-a-sheet) work as they do in XLSX. Formulas like `of:=SUM([.D3:.D4])` and the sheet's print ranges are re-based the same way, and a formula's saved result is dropped so that LibreOffice computes it when it opens the file. A reference that names a sheet, like `[$Other.B2]`, is left as written.

ODS differs from XLSX in two ways.

Cells stay text. A cell holding `{{ qty }}` stays a text cell, so a number arrives as text. Do your sums in the data or in formulas, where LibreOffice reads text that looks like a number as one in arithmetic, though `SUM` skips text cells.

Embedded objects aren't copied either. A chart or other object on a sheet that a sheet loop repeats gets no copy of its own, so keep such sheets free of them.

A loop whose tags sit on two sheets isn't refused in ODS, but it isn't supported either. To repeat a sheet, use `{% sheet for %}`.

Charts and page breaks are refused in ODS. A newline in a value stays a newline character.

## PPTX

Tags are read in every slide's shapes and tables, but not in layouts, masters or speaker notes.

### Slide loops

Put the opening tag on one slide and the closing tag on the same or a later slide:

| Tags | What repeats |
|---|---|
| `{% for %}` on slide 2, `{% endfor %}` on slide 3 | Slides 2 and 3, once per item, in order |
| Both in one shape, in the same paragraph | The text between them |
| Both in one shape, in different paragraphs | Those paragraphs |
| In different shapes on one slide, such as the title and the body | That slide, once per item |
| On one slide, with `\| shift(…)` on the list | Those shapes, once per item, each moved by the step. See [below](#shapes-that-move-down-or-across-a-slide) |
| In rows of a table | Those rows |

Each copy of a slide gets its own parts, relationships and ids, and the original's speaker notes aren't shared with the copies. An empty list removes the slides, and slides with no tags stay where they are.

For a condition across slides, the slide holding `{% else %}` starts the second branch, even if there's text before the tag on that slide. A block that repeats one slide can't hold an `{% else %}` or `{% elif %}`, and fails with `cannot divide a slide`.

### Shapes that move down or across a slide

To lay a list out on one slide, like a card per person, add `shift` to the loop's list with the distance from one copy to the next:

```text
{% for person in team | shift(y="3cm") %}{{ person.name }}
{{ person.role }}{% endfor %}
```

The loop then repeats the shapes that hold its tags instead of the slide, and moves each copy by the step. The first copy stays where the shapes are, the second sits 3 cm lower and the third 6 cm lower. The tags can sit in one text box, which repeats that box, or in several shapes, which repeat together. For example, a filled rectangle holding `{% for … %}{{ person.name }}` and a text box beside it holding `{{ person.role }}{% endfor %}` repeat as a pair. A group counts as one shape and moves as a whole.

| Step | Moves each copy |
|---|---|
| `shift(y="3cm")` | Down |
| `shift(x="6cm")` | Across, to the right |
| `shift(x="6cm", y="1cm")` | Across and down |
| `shift(y="-2cm")` | Up |

A step is a quoted number and a unit, one of `cm`, `mm`, `in`, `pt` or `emu` (914,400 to the inch). PowerPoint's curly quotes work as well as straight ones. The step is read when the template is compiled, so it has to be written in the tag and can't come from data.

Two shifted loops nest into a grid. Here each row moves down and each cell across, and every row starts again at the left:

```text
{% for row in rows | shift(y="2cm") %}{% for cell in row | shift(x="4cm") %}{{ cell }}{% endfor %}{% endfor %}
```

Each copy gets a shape id of its own, and pictures and charts printed in a copy go at that copy's position. A placeholder that PowerPoint hasn't moved takes the position its layout gives it before it moves. Nothing stops copies from running off the slide, so a long list needs a smaller step, or a slide loop.

These are refused, and the error names the tag's slide:

| Mistake | Error |
|---|---|
| No unit | `y="4" has no unit` |
| A step from data, like `shift(y=gap)` | `the step is read when the template is compiled, so it cannot come from data` |
| An unknown argument | `shift takes x= and y=, not down=` |
| A step longer than the largest slide | `is longer than the largest slide (56 in, 142.24 cm)` |
| `{% else %}` in the loop | `cannot sit in a loop that moves shapes with shift` |
| The tags on two slides | `shift moves shapes on one slide` |
| `shift` in another format | `it does not work in DOCX templates` |

```bash
mkdir t && cd t && unzip -q ../deck.pptx
perl -pi -e 's/A slide/{{ title }}/' ppt/slides/slide1.xml
zip -qr ../tagged.pptx . && cd ..

curl -F template=@tagged.pptx -F 'data={"title":"Q3 results"}' \
  http://localhost:9027/v1/render -o out.pptx
unzip -p out.pptx ppt/slides/slide1.xml | grep -o '<a:t>Q3 results'

curl -F template=@tagged.pptx -F 'data={"title":"Q3 results"}' \
  -F steps=convert http://localhost:9027/v1/render -o out.pdf
test "$(head -c 4 out.pdf)" = "%PDF"
```

Page breaks are refused, `html` becomes plain text, and a newline in a value stays a newline character. A picture in a placeholder that inherits its position lands at the slide's top left corner, as [rich content](rich-content.md#where-the-picture-goes) explains.

## ODP

Tags are read in `content.xml`, which holds the pages and their speaker notes, but not in masters. Tags on different pages repeat that run of pages, and the copies keep the page name the original had.

Page breaks and `shift` are refused, `html` becomes plain text, and a newline in a value stays a newline character. A chart is an embedded chart object at the position of the shape that held the tag.

```bash
curl -F files=@deck.pptx -F to=odp http://localhost:9027/v1/convert -o deck.odp
mkdir t && cd t && unzip -q ../deck.odp
perl -pi -e 's/>A slide</>{{ title }}</' content.xml
zip -qr ../tagged.odp . && cd ..

curl -F template=@tagged.odp -F 'data={"title":"Q3 results"}' \
  http://localhost:9027/v1/render -o out.odp
unzip -p out.odp content.xml | grep -o 'Q3 results'
```

## Not supported in any format

Tags in attributes, field codes or document properties aren't read and stay as written. Sheet names and the alt text of pictures, charts and shapes are the exceptions, and custom document properties can hold [settings](translations.md#settings-in-document-properties).

`{% raw %}` isn't supported either. To print braces, see [printing literal braces](syntax.md#printing-literal-braces).

## What rendering takes out

A template comes from whoever sends it and the document is read by someone else, so a render takes out of every output anything that could run or reach out when the document is opened:

| What's removed | Formats | What's left |
|---|---|---|
| Macro projects: `vbaProject.bin` with its data and signatures, and Basic or script libraries (`Basic/`, `Scripts/`, `Dialogs/`) | All | A macro-enabled DOCX, XLSX or PPTX gets the plain content type, so the output is an ordinary file. In ODF, the event bindings and links that call a macro go too, and a macro link keeps its text |
| OLE objects, ActiveX controls, applets and plugins | All | The object's preview picture, where it has one. Charts and formulas are content and stay, once their own content shows they are what the manifest says |
| External links: linked pictures, linked data and workbooks, attached templates, linked sections, DDE links, Word `INCLUDEPICTURE`, `INCLUDETEXT`, `LINK`, `DDE` and `DDEAUTO` fields, and formulas using `WEBSERVICE`, `RTD`, `CALL`, `REGISTER`, DDE or another file | All | A linked picture keeps its frame in DOCX and PPTX and goes in ODF. A field keeps its last result as text. A formula goes and its cell keeps the last value where the file holds one. Rendering an XLSX drops cached formula values, so there the cell is left empty |
| Hyperlinks to private or local networks: `localhost`, `*.local`, `*.internal`, `0/8`, `10/8`, `100.64/10`, `127/8`, `169.254/16`, `172.16/12`, `192.0.0/24`, `192.168/16`, `198.18/15`, multicast and reserved addresses, `::1`, `::`, `fc00::/7`, `fe80::/10`, `fec0::/10`, IPv6 forms of those IPv4 ranges, `file:`, drive and UNC paths. Hyperlinks written as fields, and `HYPERLINK` formulas whose target isn't a plain text value | All | The link's text. Addresses are read as a browser reads them, so `http://2130706433/`, `http:\\10.0.0.1` and an address with a tab in it count too, and one a browser can't read counts as private |

This covers what the template held and what the data put in, like a `link` filter given an intranet address. Every relationship, content type and manifest entry that pointed at something removed goes with it, so the file opens without a repair. Public hyperlinks, embedded pictures and the template's own parts stay as they were.

Anything the sanitiser can't read with certainty is refused, with an error naming the part, instead of being passed through:

- A part that is neither UTF-8 nor UTF-16, or that declares an encoding its bytes aren't in.
- A part that gives the standard namespaces prefixes Office and LibreOffice never write.
- Two parts whose names differ only in case or percent-encoding. Part names are compared as OPC compares them, percent-decoded and without case.
- A content-type override that names no part.
- A part that a relationship reads as XML but the package doesn't type as XML.

An Excel 4.0 macro or dialog sheet is refused unless the request keeps macros, and so is a data connection or query table unless it keeps external links, because neither can be taken out without breaking the workbook.

### Keep what a template needs

If you trust a template and it needs one of these, send `keep` on `/v1/render` or `/v1/render/batch` with a comma-separated list of `macros`, `objects`, `externalLinks` and `privateLinks`, or with `all`. Anything else gets a `400`.

```bash
curl -F template=@invoice.docx -F "data=<invoice.json" -F keep=privateLinks \
  http://localhost:9027/v1/render -o out.docx
unzip -tq out.docx
```

`keep` only changes what the template stage takes out. LibreOffice still runs no macros and fetches nothing when it converts.

## Check a template

When tags sit in the wrong place, the error names both positions. [Validate a template](validate.md) lists every mistake in one go, without rendering a file.
