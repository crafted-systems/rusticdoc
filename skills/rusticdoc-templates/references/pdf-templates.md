# PDF templates

<!-- Generated from site/src/content/docs/docs/templates/pdf-templates.md by site/scripts/generate-skills.mjs. Do not edit. -->

Fill PDF form fields from JSON, use tags in fields and annotations, flatten the result, and overlay one PDF on another.

You can fill an existing PDF through its form fields, through tags written in fields and annotations, or by laying a second PDF over it. That's useful for forms you didn't design and can't rebuild, like a government form or a supplier's order sheet.

## List the fields

List a PDF's fields before you fill it. If it has no form, the list is empty.

```bash
curl -F files=@form.pdf http://localhost:9027/v1/pdf/forms/fields -o fields.json
jq -e 'map(.name) == ["name", "date", "agree"]' fields.json
```

```json
[
  { "name": "name", "kind": "text", "page": 1, "value": null },
  { "name": "date", "kind": "text", "page": 1, "value": null },
  { "name": "agree", "kind": "checkbox", "page": 1, "value": "Off" }
]
```

`kind` is `text`, `checkbox`, `radio`, `choice`, `signature` or `button`, and `page` starts at 1. A choice field also lists its `options`.

## Fill a form

Two routes do this:

| Route | Send the PDF as | Then |
|---|---|---|
| `POST /v1/pdf/forms/fill` | `files` | Returns the filled PDF |
| `POST /v1/render` | `template`, with non-empty `data` | Runs any `steps` on the filled PDF, as with any other document |

Both take `data`, `ctx`, `flatten` and `stripAnnotations`, plus `outputFilename` to name the result.

```bash
curl -F files=@form.pdf \
  -F 'data={"name":"Ada","date":"2026-11-06","agree":true}' \
  http://localhost:9027/v1/pdf/forms/fill -o filled.pdf

curl -F files=@filled.pdf \
  http://localhost:9027/v1/pdf/forms/fields -o fields.json
jq -e '.[0].value == "Ada" and .[2].value == "Yes"' fields.json
```

```bash
curl -F template=@form.pdf -F 'data={"name":"Ada"}' \
  http://localhost:9027/v1/render -o filled.pdf
curl -F files=@filled.pdf \
  http://localhost:9027/v1/pdf/forms/fields -o fields.json
jq -e '.[0].value == "Ada"' fields.json
```

On `/v1/render`, a PDF sent with empty `data` comes back unchanged.

### How values match fields

A field's full name is a path into the data, so a field named `customer.name` takes `{"customer":{"name":"Ada"}}` and `items.0.qty` takes the first element's `qty`. A field with no match keeps what it had, and data with no matching field is ignored.

| Kind | What the value does |
|---|---|
| `text` | Stored as text. Numbers and booleans are written out |
| `checkbox` | On for `true`, a non-zero number, the strings `true`, `yes`, `on` and `1` in any case, or the checkbox's own on-state. Anything else is off |
| `radio` | Selects the button whose on-state equals the value. `""` and `Off` clear it. Any other value fails with a `400` that lists the options |
| `choice` | A value or a list of values. A value that isn't an option fails with a `400`, unless the field allows free text |
| `signature`, `button` | Left alone |

## Tags in fields

A field whose value or default value holds a `{{ }}` or `{% %}` tag is rendered, unless the data already has a value under the field's name, so a field holding `Dear {{ customer.name }}` becomes `Dear Ada`. You type the tag as the field's value in your PDF editor.

## Tags in annotations

A `Text`, `FreeText` or `Square` annotation is rendered too when its contents hold a tag. Other annotations, and any without a tag, stay as they are.

If the annotation sits over a form field, the rendered value fills the field it overlaps most. If there's nothing under it, the value is drawn on the page inside the annotation's rectangle.

A tag can end in one of two shorthands:

| Ending | Over a field | Over nothing |
|---|---|---|
| `{{ customer.name \| fill }}` | Fills it | Draws the value |
| `{{ vip \| check }}` | Fills it, so a checkbox follows the checkbox rules above | Draws `X` when the value counts as true, and nothing otherwise |

`fill` and `check` only work as the last filter in a tag, and that tag has to be the annotation's whole content.

Tagged annotations are removed from the output unless you send `stripAnnotations=false`.

### Filters in PDF tags

PDF tags can use `currency`, `number`, `date`, `t` and the [built-in filters](filters.md#built-in-filters), like `upper` and `default`. A PDF holds no `{% settings %}`, so these filters read the request's `lang`, `timezone`, `currency` and `translations` fields and the service's dictionaries instead (see [Translations and settings](translations.md)).

```bash
curl -F template=@form.pdf \
  -F 'data={"name":"Ada","date":"2026-05-01","agree":true}' \
  -F lang=de http://localhost:9027/v1/render -o filled.pdf
```

The rich content filters don't exist here. A tag that uses one fails with a `422`, and the message says where it is:

```text
template error: annotation on page 1: unknown filter: filter image is unknown (in <string>:1)
```

A missing value is also an error that names the place: ``template error: field `greeting`: undefined value``.

## Flatten

`flatten=true` draws the filled values into the page and removes the fields, so the values can no longer be changed.

```bash
curl -F files=@form.pdf -F 'data={"name":"Ada","agree":true}' -F flatten=true \
  http://localhost:9027/v1/pdf/forms/fill -o flat.pdf

curl -F files=@flat.pdf http://localhost:9027/v1/pdf/forms/fields -o fields.json
jq -e 'length == 0' fields.json
```

## Text in any script

Field appearances and annotation values are text the service draws itself, and how it draws them depends on the script.

Windows Latin text, which covers Western European languages and symbols like `€` and `“”`, is drawn in Helvetica. Every viewer has Helvetica built in, so nothing is embedded.

Anything else, like Arabic, Hebrew, Greek, Cyrillic, Chinese, Japanese or Korean, is shaped and drawn with the service's own fonts. Arabic letters join, right-to-left text runs right to left, and a value that mixes scripts gets a font for each. Only the glyphs used are embedded, so a Chinese name adds kilobytes rather than the font's 16 MB. Copying or extracting the text gives the value back. The one known exception is Poppler's `pdftotext`, which swaps the two letters of an Arabic lam-alef ligature even though the file maps them in order.

| Value | Drawn with (in the image) | Added to the file |
|---|---|---|
| `é € ok` | Helvetica, built into viewers | Nothing |
| `مرحبا بالعالم` | DejaVu Sans, which fontconfig picks for Arabic | About 13 KB |
| `你好，世界` | Noto Sans CJK SC | About 4 KB |

The fonts are the same ones [conversions use](https://rusticdoc.com/docs/guides/fonts/): the image's, the mounted fonts directory and uploaded fonts. Latin letters in a value like this use Liberation Sans, Serif or Mono, whichever matches the field's font, and so do Greek, Cyrillic and Hebrew, because those fonts cover them. Other scripts get the font fontconfig picks for them. Chinese characters use Noto Sans CJK, in its Japanese form when the value has kana, its Korean form when it has Hangul, and its Simplified Chinese form otherwise. A character that no font has is drawn as an empty box.

Either way, the field's font size (`0` picks the largest that fits), alignment and multiline setting apply, and the text is cut off at the edge of the box. An annotation value is drawn from the start of its rectangle, which is the left for left-to-right text and the right for right-to-left text.

A filled form normally asks viewers to redraw its fields. A form with text outside Latin doesn't, because viewers that redraw fields, like Poppler's, show that text as blanks or `?`. The appearances the service drew are kept, whether the form is editable or flattened.

To keep the work for one form bounded, only the first 4,096 characters of a value are drawn, though the field still stores all of it. A form can draw at most 2,000 boxes and 100,000 characters of this kind of text, and anything more fails with a `400`.

## Overlay

`POST /v1/pdf/overlay` takes exactly two PDFs in `files`, the base first and then the overlay, and draws each overlay page over the matching base page. Extra base pages stay as they are, and extra overlay pages are added at the end. The result is named after the base unless you send `outputFilename`.

This puts a rendered letter over a letterhead:

```bash
curl -F template=@letter.txt -F "data=<letter.json" -F steps=convert \
  http://localhost:9027/v1/render -o letter.pdf

curl -F files=@a.pdf -F files=@letter.pdf -F outputFilename=on-letterhead \
  http://localhost:9027/v1/pdf/overlay -o on-letterhead.pdf
test "$(head -c 4 on-letterhead.pdf)" = "%PDF"
```

Anything other than two PDFs gets a `400`.

To do both in one request, render and convert the template on `/v1/render` with the [`underlay` step](https://rusticdoc.com/docs/guides/pipeline/#underlay), which lays the result over a base PDF in the same way.

## What isn't supported

- `/v1/templates/validate` doesn't read PDFs. It returns `ok: false` with `'.pdf' is not a template format`.
- `/v1/render/batch` doesn't take a PDF template, and returns `501`.
- Encrypted PDF forms fail with `501`.
- Rich text, comb fields, date formats and JavaScript actions in a form aren't interpreted.

## On the command line

```text
rusticdoc pdf fields form.pdf
rusticdoc pdf fill form.pdf --data values.json --flatten -o filled.pdf
rusticdoc pdf overlay base.pdf letter.pdf -o combined.pdf
```
