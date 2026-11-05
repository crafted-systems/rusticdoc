# rusticdoc-template

Office files as templates. A template is an ordinary DOCX, ODT, XLSX, ODS,
PPTX or ODP whose text holds Jinja-style tags; `compile` parses it once,
`render` fills it with JSON, `validate` lists every problem it can find.

## Tags

Tags are evaluated by [minijinja](https://docs.rs/minijinja). Undefined
names are errors, reported with the part, place and tag text.

| Tag | Example |
| --- | --- |
| Print | `{{ customer.name }}`, `{{ items \| length }}` |
| Condition | `{% if vip %}…{% elif %}…{% else %}…{% endif %}` |
| Loop | `{% for item in items %}…{% endfor %}` (`loop.index`, `loop.last`) |
| Assign | `{% set total = items \| sum(attribute="price") %}` |
| Settings | `{% settings lang="fr" currency="EUR" timezone="+02:00" t.hello="Bonjour" %}` |
| Comment | `{# removed from the output #}` |

A tag may be typed in Word with spell-check marks, different formatting or
curly quotes: the engine joins the runs of a paragraph before reading tags,
keeps the first run's formatting for the joined text, straightens quotes
and decodes `&quot;`-style entities. Printed values are XML-escaped; a
newline in a value becomes a line break in DOCX and ODT.

### Filters

- `currency("EUR")` → `EUR 1,234.50`; without an argument the settings
  currency applies.
- `date("%d/%m/%Y")` accepts ISO 8601 strings and unix seconds; zoned
  values are shown in the settings `timezone` (a fixed offset; default UTC).
- `t` looks a key up in the settings translations, trying `key.<lang>` then
  `key`, and falls back to the key.
- Everything minijinja ships: `upper`, `default`, `join`, `sum`, `round`, …

## What a loop or condition repeats

There are no special tags; where the opening and closing tags sit decides:

| Opening and closing tag … | Repeats |
| --- | --- |
| in the same paragraph | the text between them |
| in the same table row (any cells) | that row |
| in two rows of one table | those rows and everything between |
| in different paragraphs | those paragraphs and everything between |
| on different slides (PPTX) or pages (ODP) | that run of slides |
| in rows of a sheet (XLSX, ODS) | those rows |

`else` and `elif` follow the same rule and need a row or paragraph of their
own; the closing tag's row or paragraph belongs to the last branch. Loops
nest (a row loop inside a paragraph loop, an inline loop inside a row).

Sheets: a cell holding only `{{ expr }}` becomes a number or boolean cell
when the value is one. Copied rows are renumbered; references in formulas
move with them, a range such as `SUM(D4:D4)` grows over every copy, and
`fullCalcOnLoad` is set so values recompute on open. Shared formulas are
expanded. Slides: copies get their own parts, rels, ids and content types;
the notes of the original are not shared with the copies.

## Validation

`validate` reports, without stopping: unparseable tags, unbalanced or
mismatched blocks, blocks whose tags sit where no rule applies (both
positions named), undefined values met during a test render, and a warning
for a loop over a value that is not an array.

## Not supported yet

- A `for` in a sheet name (sheet repetition) and a row loop nested in another
  row block of the same sheet.
- Re-basing ODS formulas, sheet hyperlinks, data validations and
  conditional formatting; charts pointing at looped rows.
- Named time zones (`Europe/Paris`); only fixed offsets.
- Tags in attributes, field codes or document properties; `{% raw %}`.
- Tags inside DOCX text boxes nested in drawings are read, but a block
  cannot span a text box and the body.
