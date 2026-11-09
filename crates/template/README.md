# rusticdoc-template

Office files as templates. A template is an ordinary DOCX, ODT, XLSX, ODS,
PPTX or ODP whose text holds Jinja-style tags; `compile` parses it once,
`render` fills it with JSON, `validate` lists every problem it can find.

## Tags

Tags are evaluated by [minijinja](https://docs.rs/minijinja). Undefined
names are errors, reported with the part, place and tag text, unless a
caller renders with `render_with` and `RenderOptions { undefined_as_empty:
true }`, which prints them as nothing for that render.

| Tag | Example |
| --- | --- |
| Print | `{{ customer.name }}`, `{{ items \| length }}` |
| Condition | `{% if vip %}…{% elif %}…{% else %}…{% endif %}` |
| Loop | `{% for item in items %}…{% endfor %}` (`loop.index`, `loop.last`) |
| Assign | `{% set total = items \| sum(attribute="price") %}` |
| Settings | `{% settings lang="fr" currency="EUR" timezone="+02:00" t.hello="Bonjour" %}` |
| Sheet loop | `{% sheet for s in sites %}` in a cell, `{{ s.name }}` as the sheet's name |
| Comment | `{# removed from the output #}` |

Settings may also be custom document properties of the same names
(`docProps/custom.xml`, or `meta:user-defined` in ODF `meta.xml`); the tag
wins where both set the same thing.

A tag may be typed in Word with spell-check marks, different formatting or
curly quotes: the engine joins the runs of a paragraph before reading tags,
keeps the first run's formatting for the joined text, straightens quotes
and decodes `&quot;`-style entities. Printed values are XML-escaped; a
newline in a value becomes a line break in DOCX and ODT.

### Filters

- `currency("EUR")` → `EUR 1,234.50` with no language; without an argument
  the settings currency applies.
- `number` → `1,234.5`, `number(2)` → `1,234.50`.
- `date("%d/%m/%Y")` accepts ISO 8601 strings and unix seconds; zoned
  values are shown in the settings `timezone` (an offset or an IANA zone;
  default UTC).
- `t` looks a key up in the request's translations, the service's
  dictionaries and the settings, for the language and then each shorter tag
  (`fr-ca`, `fr`), then `key`, and falls back to the key.
- With a `lang` CLDR has data for (`src/locale_data.rs`, written by
  `scripts/locale-data.mjs`), `number`, `currency` and `date` use its
  separators, symbol placement and month and day names. `RenderOptions`
  carries a request's `lang`, `timezone`, `currency` and translations, which
  win over the template's.
- Everything minijinja ships: `upper`, `default`, `join`, `sum`, `round`, …
- From `compute` (every template kind, text and PDF tags too): the `empty`
  test; `truncate`, `unaccent`, `pad_left`, `pad_right`; `avg`, `ceil`,
  `floor`, `words`; `date_add`, `date_sub`, `start_of`, `end_of`,
  `date_diff`, `duration`; and `now()`, which `ctx.now` fixes.

## Rich content

These filters produce content other than text. Print the result directly:
`{{ logo | image(width=120) }}`. It may be assigned first (`{% set x = … %}`),
but going through another filter, `~`, a macro or a `{% set %}…{% endset %}`
block is an error, because a rich value is not a string.

| Filter | Example |
| --- | --- |
| `image` | `{{ logo \| image(width=120) }}` |
| `barcode` | `{{ sku \| barcode("code128") }}`, `{{ url \| barcode("qr", size=90) }}` |
| `link` | `{{ url \| link("Click here") }}` |
| `color` | `{{ status \| color("#ff0000", background="#ffee00") }}` |
| `html` | `{{ body \| html }}` |
| `chart` | `{{ rows \| chart(type="bar", x="month", y=["sales","costs"], title="Revenue") }}` |
| `pagebreak` | `{{ "" \| pagebreak }}` |

What each becomes:

| Filter | DOCX | PPTX | XLSX | ODT | ODS | ODP |
| --- | --- | --- | --- | --- | --- | --- |
| `image`, `barcode` | inline `w:drawing` | `p:pic` at the tag's shape | `xdr:oneCellAnchor` at the tag's cell | inline `draw:frame` | `draw:frame` on the cell | `draw:frame` on the page at the tag's shape |
| `link` | `w:hyperlink` | `a:hlinkClick` on the run | `hyperlinks` entry on the cell | `text:a` | `text:a` | `text:a` |
| `color` | `w:color`, `w:shd` on the run | `a:solidFill`, `a:highlight` on the run | new font, fill and format for the whole cell | `text:span` with a style | new style for the whole cell | `text:span` with a style |
| `html` | native paragraphs and tables | plain text with line breaks | plain text with line breaks | native paragraphs, lists and tables | plain text with line breaks | plain text with line breaks |
| `chart` | native chart | native chart at the tag's shape | native chart at the tag's cell, data on a hidden sheet | inline `draw:frame` with an embedded chart object (`Object N/`, data in its local table) | the same on the cell | the same on the page at the tag's shape |
| `pagebreak` | page break | error | error | page break | error | error |

**`image`** takes a data URI, raw base64, or an object
`{"data": base64, "mime": ..., "width": ..., "height": ...}`. The value must
carry the bytes: this crate never fetches a URL, so the caller resolves those
first. PNG and JPEG only, checked by their magic bytes. `width` and `height`
are in points, and the aspect is kept when one is given. Arguments win over the
object's sizes. The default is the natural size at 96 dpi, capped at the page
text width (450 pt). Identical bytes share one media part.

**`barcode`** takes `qr`, `code128` (printable ASCII), `code39`, `ean13` or
`ean8` (with or without the check digit, which is verified when present). It
draws a PNG with a whole number of pixels per module and places it like an
image. `size` is the printed width in points. No human-readable text is drawn.

**`link`** shows its argument, or the address without one. Only `http`, `https`
and `mailto` are allowed; anything else, such as `javascript:` or `file:`, is
an error.

**`color`** takes `#rgb` or `#rrggbb`, strictly, and may come from data. Sheets
have no runs here, so in XLSX and ODS it colours the whole cell, and the last
`color` in a cell wins.

**`html`** reads `p`, `br`, `b`/`strong`, `i`/`em`, `u`, `s`, `sub`, `sup`, `ul`,
`ol`, `li`, `h1`..`h6`, `a`, `span style="color:#hex"`, `table`, `tr`, `td` and
`th`. Other tags are dropped and their text kept; `script` and `style` go whole.
No attribute is copied except a checked `href` and colour. An `a` with a scheme
`link` would refuse is an error, while a `span` with any other style just loses
the style. In DOCX and ODT the tag's paragraph is replaced by generated ones
using its properties, and text around the tag stays in paragraphs of its own;
lists are indents with a literal bullet or number in DOCX and `text:list` in
ODT, headings are bold and sized, and in DOCX the tag's `w:r` run properties
carry over. PPTX, XLSX, ODS and ODP keep no structure: the text is flattened
with a line break per paragraph, item or row (`•` and `1.` stay, table cells
are separated by tabs), and XLSX sets wrap text on the cell.

**`chart`** takes a list of objects. `type` is `bar`, `column`, `line` or `pie`
(one `y`); `x` and `y` name keys; `title`, `width` and `height` (points, default
450 by 270) are optional. A `null` or missing value is a gap, while a series
with no value at all is an error. DOCX and PPTX charts carry their values
inline in `c:strCache` and `c:numCache` with no embedded workbook: the chart
renders, but its data cannot be edited in Word or PowerPoint. Excel reads a chart
from cells and ignores caches, so an XLSX chart also adds a hidden sheet,
`RichChartData`, holding the values the references point at.

**`pagebreak`** is useful between loop iterations. In ODT a tag with nothing
after it breaks after its paragraph, and otherwise the text after it starts a
new paragraph on the new page.

### Limits

| What | Limit |
| --- | --- |
| One image | 20 MB |
| Images and barcodes in one render | 200 |
| Barcode value | 4 KB, 4096 modules when linear |
| Barcode image | 4 megapixels (the module gets smaller, the printed size does not) |
| Charts in one render | 200 |
| Values in one chart | 1,000, all finite numbers |
| One `html` value | 1 MB, nested at most 64 deep |
| One HTML table | 256 columns, 10,000 rows, 50,000 cells counting padded ones |
| All HTML tables in one render | 100,000 cells counting padded ones |
| Rich values in one render | 100,000 |
| Text held by rich values, and generated markup, in one render | 256 MB each |
| The whole package with what was added | 256 MB |

### In text templates

`environment()` is for text and has no package, so there the rich filters fail
with a clear error. `rich_environment(format)` has the real filters for checking
tags outside a compiled template; their results print as nothing, so no marker
ever leaves the crate. Only `Compiled::render` places content.

### How it works

A rich filter checks its value, stores it in a table belonging to that render
and prints a private marker. The marker is built only from characters XML
forbids, which the formatter strips from every other value, so data cannot
forge one. After rendering, each part with markers is rewritten and media,
parts, relationships and content types are added; no name, path or relationship
target is taken from data except a checked URL. Parts without markers stay
byte-identical. Bad values are template errors, placed at their tag.

## What a loop or condition repeats

Apart from the sheet loop, there are no special tags; where the opening and
closing tags sit decides:

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
nest (a row loop inside a paragraph loop, a row loop inside another row
block, an inline loop inside a row).

Sheets: a cell holding only `{{ expr }}` becomes a number or boolean cell
when the value is one. Copied rows are renumbered; references in formulas
(XLSX and ODS), print areas and other defined names move with them: a
reference into the loop a formula sits in stays in its own copy, and from
outside a range such as `SUM(D4:D4)` grows from the first copy to the last.
In XLSX `fullCalcOnLoad` is set and in ODS cached results are dropped, so
values recompute on open. Shared formulas are expanded.

`{% sheet for s in sites %}` in any cell repeats its whole sheet, once per
item; the sheet's name may hold `{{ }}` expressions, and each copy's name is
made valid (forbidden characters become `_`, at most 31 UTF-16 units) and
unique (` (2)`, ` (3)`, without regard to case). XLSX copies get their own
parts, rels, content types, drawings, charts, tables (renamed, new ids),
comments and sheet-scoped defined names; calcChain is dropped and
`docProps/app.xml` lists the new names. An empty list removes the sheet.

Slides: copies get their own parts, rels, ids and content types; the notes
of the original are not shared with the copies.

PPTX shapes: `{% for c in cards | shift(y="3cm") %}` … `{% endfor %}` on one
slide repeats the top-level shapes holding the two tags (a group is one
shape) instead of the slide, and moves copy *n* by *n* steps. `x=` and `y=`
take a quoted length in `cm`, `mm`, `in`, `pt` or `emu`, read at compile
time (`rich::shift::of_for`). The compiler hoists the tags onto the shapes
with a comment marking each copy; `rich::shift::apply` moves the shapes
between the comments after rendering, before rich values are placed, gives
copies fresh shape ids, and drops the comments. Nested shifted loops make a
grid. The `shift` filter itself only checks its arguments and passes the
list on.

## Sanitising

`Compiled::render` ends with `sanitize::sanitize`, which takes out of the
package what `Keep` does not keep (by default, everything listed here):
VBA projects and their parts, with the main content type made plain; ODF
`Basic/`, `Scripts/` and `Dialogs/` with event listeners and macro links;
OLE objects, ActiveX controls, applets and plugins, leaving their preview
picture (charts and formulas stay); external relationships other than
hyperlinks, XLSX `externalLink` parts and the formulas that used them, and
ODF linked images, sections and DDE links; and hyperlinks to private or
local addresses, which keep their text; Word fields that link out or pull
content in (simple and complex), and formulas that reach outside. Dangling
relationships, overrides and manifest entries go with what they named.
`Compiled::render_with` takes a `Keep`; the free function `sanitize` cleans
any package. It fails closed: parts it must read are decoded as UTF-8 or
UTF-16 and refused otherwise, the namespaces it matches must carry their
standard prefixes, OPC names compare without case and a package with two
names differing only in case is refused, an embedded ODF object is kept as
a chart or formula only when its own content is one (and is then walked
too), and addresses are parsed by the WHATWG rules (`url` crate) with
unparseable ones counted as private.

## Validation

`validate` reports, without stopping: unparseable tags, unbalanced or
mismatched blocks, blocks whose tags sit where no rule applies (both
positions named), undefined values met during a test render, and a warning
for a loop over a value that is not an array.
Rich filters are valid tags; their bad arguments are reported whatever the
sample data holds, and bad values in the sample data like any other error.

## Not supported yet

- Re-basing sheet hyperlinks, data validations and conditional formatting;
  charts pointing at looped rows; formulas on one sheet pointing at looped
  rows on another.
- Copying embedded objects (charts) of an ODS sheet that a sheet loop
  repeats.
- Named time zones (`Europe/Paris`); only fixed offsets.
- Tags in attributes, field codes or document properties; `{% raw %}`.
- Tags inside DOCX text boxes nested in drawings are read, but a block
  cannot span a text box and the body.
- Editing a chart's data in Word or PowerPoint.
- Sanitising Excel 4.0 macro sheets, data connections and query tables:
  a package holding one is refused unless the request keeps that kind.
- A picture in a PPTX placeholder that inherits its position from the layout
  lands at the slide's origin: the offset is read from the shape itself.
- `html` inside a DOCX hyperlink or content control, and native structure for
  `html` in PPTX, XLSX, ODS and ODP.
