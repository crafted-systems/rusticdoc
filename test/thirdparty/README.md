# Third-party templates

This is the check behind phase 2's gate: templates written for other tools, rewritten in this syntax, should give what those tools give.

The templates come from two MIT-licensed projects, at pinned commits:

- [xlsx-template](https://github.com/optilude/xlsx-template) at `7a95c8f`, four workbooks from `test/templates/`.
- [docxtemplater](https://github.com/open-xml-templating/docxtemplater) at `e1d61c6`, three decks from `examples/`, each with the `expected-*.pptx` its own tests compare against.

None of their files are kept here. `fetch.sh` downloads the ten it needs into `upstream/` and stops if a SHA-256 sum differs.

`make thirdparty` (or `test/thirdparty/run.sh`) then does three things:

1. `port.py` rewrites each template's tags by exact string replacement inside the file. Nothing else in the file changes, and a replacement that matches a different number of times stops the run.
2. The CLI fills each port with the data in `data/`, which is the data upstream's tests use. No engine is needed.
3. `check.py` compares the results with what upstream expects. For a workbook, that is the cell values xlsx-template's tests assert in `test/crud-test.ts`. For a deck, it is the paragraphs of docxtemplater's expected file.

A difference listed under `KNOWN` in `check.py` is printed but does not fail the run. Anything else does.

## What is ported

| Port | Upstream file | What it covers | Result |
|---|---|---|---|
| `xlsx-array` | `test-array.xlsx` | Values picked from a list by index | 2 of 2 checks |
| `xlsx-nested` | `test-nested-arrays.xlsx` | A number from a list inside a list of rows | 1 of 1 |
| `xlsx-sheets` | `multple-sheets-arrays.xlsx` | The same sheet filled once per item | 4 of 4 |
| `xlsx-plan` | `t1.xlsx` | A plan table: scalars, a row per person, a column per day | 21 of 21 |
| `pptx-loop` | `loop-example.pptx` | A loop across two paragraphs of one text box | 1 of 2 |
| `pptx-table` | `loop-table.pptx` | A table row per product | 1 of 1 |
| `pptx-paragraphs` | `paragraph-loop.pptx` | A paragraph per user | 1 of 1 |

## What the ports had to change

Rewriting the tags was not always enough. These are the places where the two syntaxes do not line up:

- **A column loop for a list across columns.** `t1.xlsx` spreads lists across columns with `${dates}` and `${table:planData.days}`. The port writes each as a column loop, as in `{% col for d in p.days %}{{ d }}{% endcol %}`, and ends the row loop in the same cell so that no empty cell follows the last copy.
- **`excel_date` for dates.** xlsx-template writes a date as the serial number Excel stores. The port passes each date through `excel_date`, and the data holds ISO dates like `2013-01-01`.
- **One sheet in place of two.** Upstream's workbook has two identical sheets and fills each from its own data. The port keeps one sheet and repeats it with `{% sheet for %}`.
- **Tag-only paragraphs.** docxtemplater removes a paragraph that holds only a loop tag. Here it stays as an empty paragraph unless the tag is followed by `{{ drop("paragraph") }}`, which the port adds.
- **Inline strings.** Filled text is written as inline strings, where xlsx-template's tests look values up in the shared strings table. The values are the same, and `check.py` reads both.

When the gate first ran, there was no column loop and no `excel_date`. That port added a cell per column and indexed the list, and its data held the serial numbers. Both features were added afterwards, and the port now uses them.

The data for `paragraph-loop.pptx` is not in docxtemplater's tests, so `data/pptx-paragraphs.json` was worked out from its expected file.

## The one difference

`loop-example.pptx` opens a loop in one paragraph and closes it in the next. docxtemplater repeats the text between the tags and gives three paragraphs. A loop here repeats whole paragraphs, so the same template gives four: the text is the same, ` Doe  John `, but the names land in separate paragraphs. That is how a loop across paragraphs is documented to work, so it is recorded as a difference and not a fault.

## Last run

10 November 2026, a build of the repository on macOS 27: 31 of 32 checks match, with the one known difference and no failures.

The seven filled files were also converted to PDF by LibreOffice in the image, and the text came through. They open in Excel and PowerPoint 16.114.4 for Mac with no repair prompt. The plan table was converted and opened again after its port moved to the column loop.

The plan table needs the column loop and `excel_date`, which came in 0.1.1, so the 0.1.0 CLI can't fill it.

This is seven small templates from two projects. It shows the common cases carry over, not that every template written for those tools will.
