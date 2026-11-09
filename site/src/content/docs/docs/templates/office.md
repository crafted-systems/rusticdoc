---
title: Office formats
description: Place loops and conditions in DOCX, ODT, XLSX, ODS, PPTX and ODP templates, and see what each format supports.
sidebar:
  order: 4
---

This page shows where to put tags in Word, LibreOffice, Excel and PowerPoint files, so that a loop repeats the row, paragraph or slide you meant. It ends with what each format supports and what it does not.

A template is an ordinary file with tags typed into its text. You need no add-in, field or special style.

:::note
Templates typed in Word, Excel and PowerPoint 16.114 for Mac have been filled, and the results open in those apps without a repair prompt. Office for Windows has not been tried. See [known limits](/docs/project/known-limits/).
:::

## Typing tags in a word processor

Word and other editors do things to text as you type. The service undoes the ones that matter:

- **Split runs.** Word often splits text into several runs because of spell-check marks, edits or formatting. A tag split this way still works, because the runs of a paragraph are joined before tags are read. The joined text takes the formatting of the run it starts in, so format the whole tag or its first character.
- **Curly quotes.** `{{ “x” | default(‘y’) }}` is read with straight quotes.
- **Non-breaking spaces** in a tag are read as spaces.
- **Entities** such as `&quot;` in the file's XML are decoded.

A line break typed inside a tag is read as whitespace too.

## What a block repeats

Apart from the sheet loop, there are no special loop tags. The positions of a block's opening and closing tags decide what repeats, for `{% for %}` and `{% if %}` alike.

| Opening and closing tag sit | Repeats |
|---|---|
| In the same paragraph | The text between them |
| In the same table row, in any cells | That row |
| In two rows of one table | Those rows, and everything between |
| In different paragraphs | Those paragraphs, and everything between |
| On different slides (PPTX) or pages (ODP) | That run of slides |
| In different shapes of one slide (PPTX) | That slide |
| On one slide, with `\| shift(…)` (PPTX) | Those shapes, each copy moved by a step |
| In rows of a sheet (XLSX, ODS) | Those rows |
| `{% sheet for s in sites %}` in any cell of a sheet (XLSX, ODS) | That sheet, once per item: see [repeating a sheet](#repeating-a-sheet) |

Tags in different parts of a package cannot form a block, such as a `{% for %}` in the body and its `{% endfor %}` in a header, or tags on two sheets of an XLSX: `a block cannot span parts`. PPTX slides are the exception, as the table says.

### Table rows

Put the opening tag in the row's first cell and the closing tag in its last. The sample invoice does this, and the middle cells hold no block tags:

| Item | Qty | Unit price | Amount |
|---|---|---|---|
| `{% for item in items %}{{ item.name }}` | `{{ item.qty }}` | `{{ item.price \| currency(currency) }}` | `{{ (item.qty * item.price) \| currency(currency) }}{% endfor %}` |

```bash test
curl -F template=@invoice.docx -F "data=<invoice.json" http://localhost:9027/v1/render -o out.docx
unzip -p out.docx word/document.xml > out.xml
grep -o 'Single-origin beans' out.xml
grep -o 'Delivery to Mombasa' out.xml
grep -o 'KES 28,800.00' out.xml
```

An empty list removes the row. Tags in two rows repeat both rows as a group, for a line with a description under it.

### Paragraphs

Put the opening tag in the first paragraph to repeat, and the closing tag in the last.

```text
{% for i in items %}Item {{ i.name }}
Some text about it.
Last line{% endfor %}
```

This gives all three paragraphs once per item. To start each item on a new page in a DOCX or ODT, add a paragraph holding `{{ "" | pagebreak }}` before the closing tag.

A paragraph holding only a block tag stays, and is empty, in each copy. To avoid blank lines, write the tags in paragraphs that hold text, as above.

### A loop around a table

Tags in paragraphs before and after a table repeat the whole table with those two paragraphs. Since each holds only a tag, each copy gets two empty paragraphs.

### Text in one paragraph

Tags in one paragraph repeat only the text between them, and the paragraph stays one paragraph:

```text
{% for t in tags %}{{ t }}{% if not loop.last %}, {% endif %}{% endfor %}
```

### else and elif

`{% else %}` and `{% elif %}` follow the same rule. When a block spans rows or paragraphs, each needs a row or paragraph of its own, starting that branch. The closing tag's row or paragraph belongs to the last branch.

```text
{% if vip %}Thank you for being a member.
{% else %}Join us today.
Terms apply.{% endif %}
```

Putting `{% else %}` in the same paragraph as the block's opening or closing tag, when the block spans paragraphs, is refused:

```text
'{% else %}' (word/document.xml paragraph 1) needs its own paragraph between '{% if a %}' (word/document.xml paragraph 1) and '{% endif %}' (word/document.xml paragraph 2)
```

Nesting works: a row loop in a paragraph loop, a row loop in another row loop, or an inline loop in a row.

## DOCX

Tags are read in the body, headers, footers, footnotes and endnotes, and in text boxes. A block cannot span a text box and the body.

They are not read in document properties, field codes, comments or attributes such as a hyperlink's address.

A newline in a value becomes a line break.

## ODT

Tags are read in `content.xml` and `styles.xml`, which holds headers and footers. Everything under [what a block repeats](#what-a-block-repeats) applies, and a newline in a value becomes a line break.

To try it from a DOCX, convert one. The tags survive the conversion:

```bash test
curl -F files=@invoice.docx -F to=odt http://localhost:9027/v1/convert -o invoice.odt
curl -F template=@invoice.odt -F "data=<invoice.json" http://localhost:9027/v1/render -o out.odt
unzip -p out.odt content.xml | grep -o 'KES 28,800.00'
```

Charts are refused in ODT.

## XLSX

Tags are read in the cells of every sheet, in shared strings or inline text. A sheet's name can hold `{{ }}` expressions, as [repeating a sheet](#repeating-a-sheet) shows. Tags are not read in headers and footers, comments, charts or drawings.

A cell holding only `{{ expr }}` becomes a real number or boolean when the value is one, so totals and formats work. A `null` gives an empty cell. A cell with other text around the tag stays text. A newline in a value stays a newline character, and the cell needs wrap text to show it.

Tags in rows repeat those rows. Copied rows are renumbered, references in formulas move with them, and a range such as `SUM(D3:D3)` grows over every copy. The service also sets the workbook to recalculate when opened, and merged cells follow their rows.

| Before | After three items |
|---|---|
| Row 3: `{% for i in items %}{{ i.n }}`, `{{ i.q }}`, `=B3*2`, `{% endfor %}` | Rows 3, 4 and 5, with `=B3*2`, `=B4*2` and `=B5*2` |
| Row 4: `=SUM(C3:C3)` | Row 6: `=SUM(C3:C5)` |

This example turns the header row of a small sheet into a row loop:

```bash test
mkdir t && cd t && unzip -q ../sheet.xlsx
perl -pi -e 's/Month/{% for r in rows %}{{ r.month }}/; s/Sales/{{ r.sales }}{% endfor %}/' xl/worksheets/sheet1.xml
zip -qr ../tagged.xlsx . && cd ..

curl -F template=@tagged.xlsx -F 'data={"rows":[{"month":"March","sales":90},{"month":"April","sales":120}]}' \
  http://localhost:9027/v1/render -o out.xlsx
unzip -p out.xlsx xl/worksheets/sheet1.xml > sheet.xml
grep -o '<c r="B1"[^>]*><v>90</v>' sheet.xml
grep -o '<c r="B2"[^>]*><v>120</v>' sheet.xml
grep -o '<c r="A3"[^>]*>' sheet.xml
```

The print area and other defined names move with the rows too, so a printed or converted sheet shows every copy.

### Rows inside rows

A row loop can sit inside another row block, such as items inside groups. Put the inner loop's opening and closing tags in different cells of its row; in one cell they repeat text in that cell.

| Row | Cells |
|---|---|
| 3 | `{% for g in groups %}{{ g.name }}` |
| 4 | `{% for i in g.items %}{{ i.name }}`, `{{ i.qty }}`, `{{ i.price }}`, `=B4*C4`, `{% endfor %}` |
| 5 | `Subtotal`, `=SUBTOTAL(9,D3:D4)`, `{% endfor %}` |
| 6 | `Total`, `=SUBTOTAL(9,D3:D5)` |

Each group gets its header, its items and its subtotal. A formula's reference into the loop it sits in stays in its own copy, so each subtotal covers its own group, and an empty group's subtotal covers only its header row. From outside a loop, a range runs from the first copy to the last, so the total covers every group. `SUBTOTAL` skips the other `SUBTOTAL` cells in its range, so the total does not count the subtotals twice, where `SUM` would.

Start a range on the group's header row, as in `D3:D4`, rather than `D4:D4`. LibreOffice may save a one-cell range such as `D4:D4` as `D4`, which then names one row.

### Repeating a sheet

Put `{% sheet for s in sites %}` in any cell of a sheet, and give the sheet a name with an expression, such as `{{ s.name }}`. The sheet is copied once per item with `s` set, and each copy takes its name from the data. There is no closing tag: the loop ends with the sheet.

The loop sits in a cell because Excel caps a sheet name at 31 characters and refuses `: \ / ? * [ ]`, so `{% for s in sites %}{{ s.name }}` cannot be a name. To name the sheet, double-click its tab in Excel or LibreOffice and type `{{ s.name }}`, or text around it such as `Site {{ s.code }}`.

| Sheet name | Cell A1 | Data | Sheets |
|---|---|---|---|
| `{{ s.name }}` | `{% sheet for s in sites %}Report for {{ s.name }}` | `{"sites":[{"name":"Paris"},{"name":"Lyon"}]}` | `Paris`, `Lyon` |

```bash test
mkdir t && cd t && unzip -q ../sheet.xlsx
perl -pi -e 's/name="Sheet"/name="{{ r.month }}"/' xl/workbook.xml
perl -pi -e 's/>Month</>{% sheet for r in rows %}Month</; s/>January</>{{ r.month }}</; s/<c r="B2" t="n"><v>120<\/v><\/c>/<c r="B2" t="inlineStr"><is><t>{{ r.sales }}<\/t><\/is><\/c>/' xl/worksheets/sheet1.xml
zip -qr ../tagged.xlsx . && cd ..

curl -F template=@tagged.xlsx -F 'data={"rows":[{"month":"March","sales":90},{"month":"April","sales":120}]}' \
  http://localhost:9027/v1/render -o out.xlsx
unzip -p out.xlsx xl/workbook.xml | grep -o '<sheet [^>]*name="March"'
unzip -p out.xlsx xl/workbook.xml | grep -o '<sheet [^>]*name="April"'
unzip -p out.xlsx xl/worksheets/sheet2.xml | grep -o '<c r="B2"[^>]*><v>120</v>'
```

Names are made valid: a forbidden character becomes `_`, a name longer than 31 characters is cut, and a name another sheet or an earlier copy already has gets ` (2)`, ` (3)` and so on, as Excel does when it copies a sheet. Case does not count, so `paris` clashes with `Paris`. An empty name becomes `Sheet`. A looped sheet whose name holds no expression keeps its name for the first copy, and the others get ` (2)`, ` (3)` and so on.

Each copy is a whole sheet: its rows, row loops, formulas and merged cells work as in any sheet. In an XLSX each copy also gets its own drawings, charts, tables and comments, and its own print area and other names that belong to the sheet. A chart or print area that names the sheet names the copy instead. Pictures are shared. A copied table is renamed, as in `Items_2`, since table names are unique in a workbook, and formulas in the copy follow the new name.

A formula on another sheet that names the template sheet, such as `='{{ s.name }}'!B2`, points at the first copy.

An empty list removes the sheet. A workbook needs a sheet, so a list that removes every sheet fails: `the sheet loops left the workbook with no sheets`.

A sheet name can hold an expression without a loop, to name one sheet from the data, such as `{{ month }} sales`.

These are refused, with the place named:

- A second sheet loop in one sheet: `a sheet takes one sheet loop`.
- A sheet loop inside another block, such as a row loop: `a sheet loop repeats the whole sheet, so no other block can hold it`.
- A statement in a sheet name: `a sheet name can hold only {{ }} expressions`.
- `{% sheet %}` in a DOCX, ODT, PPTX or ODP: `repeats a sheet, so it works in XLSX and ODS only`.

Copies count against the size limits of a render, so a loop over a very long list fails instead of building a file too large to open.

### Not supported

- **Sheet hyperlinks, data validations, conditional formatting and charts** that point at looped rows do not move with them.
- **A formula on one sheet that points at looped rows on another** is not re-based.
- **Page breaks.** A sheet has none.

## ODS

Tags are read in `content.xml`. Headers and footers are not read.

Tags in rows repeat those rows, and [rows inside rows](#rows-inside-rows) and [repeating a sheet](#repeating-a-sheet) work as in XLSX. Formulas such as `of:=SUM([.D3:.D4])` and the sheet's print ranges are re-based the same way, and a formula's saved result is dropped so that LibreOffice computes it when it opens the file. A reference that names a sheet, such as `[$Other.B2]`, is left as written.

ODS differs from XLSX in two ways:

- **Cells stay text.** A cell holding `{{ qty }}` stays a text cell, so a number arrives as text. Do your sums in the data, or in formulas: LibreOffice reads text that looks like a number as one in arithmetic, though `SUM` skips text cells.
- **Embedded objects are not copied.** A chart or other object on a sheet that a sheet loop repeats gets no copy of its own, so keep such sheets free of them.

A loop whose tags sit on two sheets is not refused in ODS, but it is not supported. To repeat a sheet, use `{% sheet for %}`.

Charts and page breaks are refused in ODS. A newline in a value stays a newline character.

## PPTX

Tags are read in every slide's shapes and tables. They are not read in layouts, masters or speaker notes.

### Slide loops

Put the opening tag on one slide and the closing tag on the same or a later slide:

| Tags | Repeats |
|---|---|
| `{% for %}` on slide 2, `{% endfor %}` on slide 3 | Slides 2 and 3, once per item, in order |
| Both in one shape, in the same paragraph | The text between them |
| Both in one shape, in different paragraphs | Those paragraphs |
| In different shapes on one slide, such as the title and the body | That slide, once per item |
| On one slide, with `\| shift(…)` on the list | Those shapes, once per item, each moved by the step. See [below](#shapes-that-move-down-or-across-a-slide) |
| In rows of a table | Those rows |

Each copy of a slide gets its own parts, relationships and ids, and the original's speaker notes are not shared with the copies. An empty list removes the slides, and slides with no tags stay where they are.

For a condition across slides, the slide holding `{% else %}` starts the second branch, even if text sits before the tag on that slide. A block that repeats one slide cannot hold an `{% else %}` or `{% elif %}`: `cannot divide a slide`.

### Shapes that move down or across a slide

To lay a list out on one slide, such as a card per person, add `shift` to the loop's list with the distance from one copy to the next:

```text
{% for person in team | shift(y="3cm") %}{{ person.name }}
{{ person.role }}{% endfor %}
```

The loop then repeats the shapes that hold its tags, not the slide, and moves each copy by the step: the first copy stays where the shapes are, the second sits 3 cm lower, the third 6 cm lower. The tags can sit in one text box, which repeats that box, or in several shapes, such as a filled rectangle holding `{% for … %}{{ person.name }}` and a text box beside it holding `{{ person.role }}{% endfor %}`, which repeats both as a pair. A group counts as one shape and moves as a whole.

| Step | Moves each copy |
|---|---|
| `shift(y="3cm")` | Down |
| `shift(x="6cm")` | Across, to the right |
| `shift(x="6cm", y="1cm")` | Both ways at once |
| `shift(y="-2cm")` | Up |

A step is a quoted number and a unit: `cm`, `mm`, `in`, `pt` or `emu` (914,400 to the inch). PowerPoint's curly quotes work as well as straight ones. The step is read when the template is compiled, so it must be written in the tag and cannot come from data.

Two shifted loops nest into a grid. Here each row moves down and each cell across, and every row starts again at the left:

```text
{% for row in rows | shift(y="2cm") %}{% for cell in row | shift(x="4cm") %}{{ cell }}{% endfor %}{% endfor %}
```

Each copy gets a shape id of its own, and pictures and charts printed in a copy go at that copy's position. A placeholder that PowerPoint has not moved takes the position its layout gives it before it moves. Copies that run off the slide are not stopped: a long list needs a smaller step, or a slide loop.

Refused, with the tag's slide named:

| Mistake | Error |
|---|---|
| No unit | `y="4" has no unit` |
| A step from data, such as `shift(y=gap)` | `the step is read when the template is compiled, so it cannot come from data` |
| An unknown argument | `shift takes x= and y=, not down=` |
| A step longer than the largest slide | `is longer than the largest slide (56 in, 142.24 cm)` |
| `{% else %}` in the loop | `cannot sit in a loop that moves shapes with shift` |
| The tags on two slides | `shift moves shapes on one slide` |
| `shift` in another format | `it does not work in DOCX templates` |

```bash test
mkdir t && cd t && unzip -q ../deck.pptx
perl -pi -e 's/A slide/{{ title }}/' ppt/slides/slide1.xml
zip -qr ../tagged.pptx . && cd ..

curl -F template=@tagged.pptx -F 'data={"title":"Q3 results"}' http://localhost:9027/v1/render -o out.pptx
unzip -p out.pptx ppt/slides/slide1.xml | grep -o '<a:t>Q3 results'

curl -F template=@tagged.pptx -F 'data={"title":"Q3 results"}' -F steps=convert \
  http://localhost:9027/v1/render -o out.pdf
test "$(head -c 4 out.pdf)" = "%PDF"
```

Page breaks are refused, `html` becomes plain text, and a newline in a value stays a newline character. A picture in a placeholder that inherits its position lands at the slide's top left corner, as [rich content](/docs/templates/rich-content/#where-the-picture-goes) explains.

## ODP

Tags are read in `content.xml`, which holds the pages and their speaker notes. Masters are not read. Tags on different pages repeat that run of pages, and copies keep the page name the original had.

Page breaks and `shift` are refused, `html` becomes plain text, and a newline in a value stays a newline character. A chart is an embedded chart object at the position of the shape that held the tag.

```bash test
curl -F files=@deck.pptx -F to=odp http://localhost:9027/v1/convert -o deck.odp
mkdir t && cd t && unzip -q ../deck.odp
perl -pi -e 's/>A slide</>{{ title }}</' content.xml
zip -qr ../tagged.odp . && cd ..

curl -F template=@tagged.odp -F 'data={"title":"Q3 results"}' http://localhost:9027/v1/render -o out.odp
unzip -p out.odp content.xml | grep -o 'Q3 results'
```

## Not supported in any format

- Tags in attributes, field codes or document properties, apart from sheet names. They stay as written. Custom document properties can hold [settings](/docs/templates/translations/#settings-in-document-properties).
- `{% raw %}`. To print braces, see [printing literal braces](/docs/templates/syntax/#printing-literal-braces).

## What rendering takes out

A template comes from whoever sends it, and the document is read by someone else, so a render takes out of every output what could run or reach out when the document is opened:

| What | Formats | What is left |
|---|---|---|
| Macro projects: `vbaProject.bin` with its data and signatures, and Basic or script libraries (`Basic/`, `Scripts/`, `Dialogs/`) | All | A macro-enabled DOCX, XLSX or PPTX gets the plain content type, so the output is an ordinary file. In ODF, the event bindings and links that call a macro go too, and a macro link keeps its text |
| OLE objects, ActiveX controls, applets and plugins | All | The object's preview picture, where it has one. Charts and formulas are content and stay, once their own content shows they are what the manifest says |
| External links: linked pictures, linked data and workbooks, attached templates, linked sections, DDE links, Word `INCLUDEPICTURE`, `INCLUDETEXT`, `LINK`, `DDE` and `DDEAUTO` fields, and formulas using `WEBSERVICE`, `RTD`, `CALL`, `REGISTER`, DDE or another file | All | A linked picture keeps its frame in DOCX and PPTX and goes in ODF. A field keeps its last result as text. A formula goes and its cell keeps the last value where the file holds one; rendering an XLSX drops cached formula values, so there the cell is left empty |
| Hyperlinks to private or local networks: `localhost`, `*.local`, `*.internal`, `0/8`, `10/8`, `100.64/10`, `127/8`, `169.254/16`, `172.16/12`, `192.0.0/24`, `192.168/16`, `198.18/15`, multicast and reserved addresses, `::1`, `::`, `fc00::/7`, `fe80::/10`, `fec0::/10`, IPv6 forms of those IPv4 ranges, `file:`, drive and UNC paths. Hyperlinks written as fields, and `HYPERLINK` formulas whose target is not a plain text value | All | The link's text. Addresses are read as a browser reads them, so `http://2130706433/`, `http:\\10.0.0.1` and an address with a tab in it count too, and one a browser cannot read counts as private |

This covers what the template held and what data put in, such as a `link` filter given an intranet address. Every relationship, content type and manifest entry that pointed at something removed goes with it, so the file opens without a repair. Public hyperlinks, embedded pictures and the template's own parts stay as they were.

What the sanitiser cannot read with certainty is refused with an error naming the part, rather than passed through: a part that is neither UTF-8 nor UTF-16 or declares an encoding its bytes are not, one that gives the standard namespaces prefixes Office and LibreOffice never write, two parts whose names differ only in case or percent-encoding, a content-type override that names no part, and a part that a relationship reads as XML but the package does not type as XML. Part names are compared as OPC compares them, percent-decoded and without case. An Excel 4.0 macro or dialog sheet is refused unless the request keeps macros, and a data connection or query table unless it keeps external links, since neither can be taken out without breaking the workbook.

### Keep what a template needs

When you trust a template and it needs one of these, send `keep` on `/v1/render` or `/v1/render/batch` with a comma-separated list: `macros`, `objects`, `externalLinks`, `privateLinks`, or `all`. Anything else is a `400`.

```bash test
curl -F template=@invoice.docx -F "data=<invoice.json" -F keep=privateLinks \
  http://localhost:9027/v1/render -o out.docx
unzip -tq out.docx
```

`keep` changes only what the template stage takes out. LibreOffice still runs no macros and fetches nothing when it converts.

## Check a template

Mistakes in where tags sit are reported with both positions named. [Validate a template](/docs/templates/validate/) lists them all at once, without rendering a file.
