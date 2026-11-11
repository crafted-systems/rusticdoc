# Translations and settings

<!-- Generated from site/src/content/docs/docs/templates/translations.md by site/scripts/generate-skills.mjs. Do not edit. -->

Set a template's language, translated labels, time zone and default currency, per template or per request, and keep translations in dictionaries.

One template can print an invoice in French for one request and in German for the next. You do that by translating its labels and setting a language, a time zone and a default currency, which can live in the template, in each request, and in dictionaries the service holds.

## The four settings

| Setting | What it does |
|---|---|
| `lang` | Picks the translations `t` uses. If CLDR has data for the language, it also sets how [`number`, `currency` and `date`](filters.md#language) write numbers, money and names |
| `timezone`, or `tz` | The zone `date` shows moments in, either an offset like `+02:00` or an IANA zone like `Europe/Paris` |
| `currency` | The code `currency` uses when you don't give it one |
| translations | The text `t` prints for each key |

Each can be set in three places, and when more than one sets the same thing, the first in this list wins:

1. The request's fields: `lang`, `timezone`, `currency` and `translations`.
2. A `{% settings %}` tag in the template.
3. The template's custom document properties.

Translations add up instead of replacing each other. See [how `t` looks a key up](#how-t-finds-a-translation).

Settings work the same in Office templates, text templates and PDF tags, except that a PDF holds no settings of its own, so only the request applies there.

## Settings in the request

`/v1/render`, `/v1/render/batch` and `/v1/templates/validate` take these fields, either as multipart fields or as members of a [JSON body](https://rusticdoc.com/docs/guides/stored-templates/):

| Field | Example | Notes |
|---|---|---|
| `lang` | `fr-CA` | Any case, with `-` or `_`. It becomes `fr-ca` |
| `translations` | `{"fr": {"total": "Montant"}, "en-gb": {"total": "Total"}}` | JSON, by language, each a flat object of keys and texts |
| `timezone` | `Europe/Paris` | An offset, `UTC`, or an IANA zone |
| `currency` | `EUR` | At most 16 characters |

```bash
cat > label.txt <<'EOF'
{{ "total" | t }}: {{ total | currency }}, {{ issued | date("%d %B %Y %H:%M") }}
EOF
curl -F template=@label.txt \
  -F 'data={"total":1500,"issued":"2026-07-01T10:00:00Z"}' \
  -F lang=de -F currency=EUR -F timezone=Europe/Berlin \
  -F 'translations={"de": {"total": "Summe"}}' \
  http://localhost:9027/v1/render -o out.txt
# The no-break space CLDR puts before the euro sign, shown here as _.
test "$(perl -CSD -pe 's/\x{a0}/_/g' out.txt)" = \
  'Summe: 1.500,00_€, 01 Juli 2026 12:00'
```

A field the service can't use gets a `400` that names it, so a typo isn't silently ignored:

| Sent | Message |
|---|---|
| `lang=French` | `'lang' is "French", which is not a language tag such as fr, fr-CA or en-us` |
| `timezone=Mars/Olympus` | `'timezone': timezone "Mars/Olympus" is neither an offset like +02:00 nor an IANA zone like Europe/Paris` |
| `translations={"fr": {"total": 5}}` | `'translations.fr' has a number for "total"; texts are strings` |

`translations` holds at most 10,000 entries and 2 MiB in all, with keys of up to 256 bytes and texts of up to 16 KiB.

## Settings in the template

The `{% settings %}` tag sets the same things inside the template, with `t.<key>` for a translation and `t.<key>.<lang>` for one in a language:

```text
{% settings lang="fr" currency="EUR" timezone="Europe/Paris" t.billed.fr="Facturé à" t.billed.de="Rechnung an" %}
```

You can type the tag anywhere in the document. It applies to the whole file wherever it sits, and it's removed from the output. In an Office template, a paragraph that holds only this tag stays behind as an empty paragraph, so put it in a line that has other text. In a text template the tag leaves nothing, not even its line's text, though a line break after it stays.

Several `{% settings %}` tags add up, and for a repeated setting the last one wins.

Values must be quoted text. `lang=language` is ignored with no error, because settings are read when the template is parsed, before any data exists. Send anything that varies as a request field instead.

This example puts the settings in the sample invoice:

```bash
mkdir t && cd t && unzip -q ../invoice.docx
export TAG='{% settings lang="fr" currency="EUR" timezone="+02:00"'
TAG+=' t.billed.fr="Facturé à" t.billed.de="Rechnung an" %}'
TAG+='{{ "billed" | t }}, {{ 5 | currency }},'
TAG+=' {{ "2026-11-06T23:30:00Z" | date("%d/%m %H:%M") }}'
perl -pi -e 's/Billed to/$ENV{TAG}/' word/document.xml
zip -qr ../tagged.docx . && cd ..

curl -F template=@tagged.docx -F "data=<invoice.json" \
  http://localhost:9027/v1/render -o out.docx
unzip -p out.docx word/document.xml | perl -CSD -pe 's/\x{a0}/_/g' |
  grep -o 'Facturé à, 5,00_€, 07/11 01:30'

# The request's language wins over the template's.
curl -F template=@tagged.docx -F "data=<invoice.json" -F lang=de \
  http://localhost:9027/v1/render -o out.docx
unzip -p out.docx word/document.xml | perl -CSD -pe 's/\x{a0}/_/g' |
  grep -o 'Rechnung an, 5,00_€, 07/11 01:30'
```

> **Note**
>
> A template whose `lang` has CLDR data now writes numbers, money and dates in that language, as above (`5,00 €` where it used to print `EUR 5.00`). A template with no `lang`, rendered without one, prints exactly as before.

### Settings in document properties

You can set the same things as custom document properties instead, so nothing has to be typed on the page. Name each property after its setting (`lang`, `currency`, `timezone` or `tz`, `t.<key>`) and give it a text value. In Word, Excel and PowerPoint that's under File > Properties > Custom, and in LibreOffice under File > Properties > Custom Properties.

A `{% settings %}` tag wins over a property that sets the same thing. Translations add up, so a `t.<key>` property and a `t.<key>` in the tag both count, and for the same key the tag's wins. Properties with other names are ignored.

Properties are read from `docProps/custom.xml` in a DOCX, XLSX or PPTX, and from `meta.xml` in an ODT, ODS or ODP. This example writes one by hand, the way Word would save it:

```bash
mkdir t && cd t && unzip -q ../invoice.docx
mkdir -p docProps
cat > docProps/custom.xml <<'EOF'
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties
  xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties"
  xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
  <property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="2"
            name="currency">
    <vt:lpwstr>EUR</vt:lpwstr>
  </property>
</Properties>
EOF
perl -pi -e 's/Billed to/{{ 5 | currency }}/' word/document.xml
zip -qr ../tagged.docx . && cd ..

curl -F template=@tagged.docx -F "data=<invoice.json" \
  http://localhost:9027/v1/render -o out.docx
unzip -p out.docx word/document.xml | grep -o 'EUR 5.00'
```

## How `t` finds a translation

`{{ "key" | t }}`, or `{{ value | t }}` with a key from the data, looks in the language first, then in each shorter tag it falls back to, then in the template's translations that have no language. If nothing has the key, it prints the key itself. For `lang=fr-ca` the order is:

1. `fr-ca`: the request's `translations`, then the service's dictionaries, then the template's `t.key.fr-ca`.
2. `fr`: the same three, in the same order.
3. The template's `t.key`, with no language.
4. The key, printed as it is, with no error.

So a more exact language beats a higher source, and a template's `t.total.fr-ca` wins over a request's `fr` entry. Within one language, the request beats the dictionaries and the dictionaries beat the template.

```bash
cat > t.txt <<'EOF'
{% settings t.total="Total" t.tax.fr-ca="TPS" t.note="Note" %}
{{- "total" | t }}|{{ "tax" | t }}|{{ "note" | t }}|{{ "other" | t }}
EOF
curl -F template=@t.txt -F lang=fr-CA \
  -F 'translations={"fr": {"total": "Montant", "tax": "Taxe"}}' \
  http://localhost:9027/v1/render -o out.txt
test "$(cat out.txt)" = 'Montant|TPS|Note|other'
```

Templates written before languages fell back keep working. `lang="fr-FR"` with `t.total.fr-FR` still finds it, and a `lang` that isn't a language tag still finds `t.key.<lang>` as written.

## Dictionaries on the service

Set `RUSTICDOC_TRANSLATIONS_DIR` to a directory of `<lang>.json` files, each a flat object of keys and texts:

```json
{"total": "Montant", "due": "Échéance"}
```

File names are language tags in any case, like `fr.json` or `fr-CA.json`. Other files, like notes, are ignored as long as they don't end in `.json`.

The files are read when the service starts. A file that isn't a flat object of strings, a `.json` file whose name isn't a language, or a missing directory stops the service from starting, with a message that names the file, so you notice a broken deploy straight away.

They're read again when a file changes. A request checks the directory's sizes and times at most once a second, so nothing is parsed per request. If a file turns bad while the service is running, it's logged as a warning and its last good version stays in use. A deleted file stops applying.

Each file can be up to 1 MiB, with the same limits per entry as the request field, and there can be up to 200 files and 4 MiB in all.

When a request has a `lang`, the render gets the dictionaries for that language and the ones it falls back to. Without one it gets every dictionary, because the template's own `lang` may pick one.

## Finding the keys to translate

`rusticdoc translate` reads Office and text templates, collects the keys they pass to `t` as literal text (`"total" | t`, wherever it sits in a tag) and the keys their settings translate, and writes them to `<dir>/<lang>.json`:

```bash
rusticdoc translate --lang fr --out translations templates/
```

The file is created if it's missing. If it exists, its translations are kept, and new keys are added with an empty text for a translator to fill in. Keys in the file that no template uses any more are listed, and kept.

Directories are searched recursively for `.docx`, `.xlsx`, `.pptx`, `.odt`, `.ods`, `.odp` and text templates. A key that comes from data, as in `{{ label | t }}`, can't be known before a render, so it isn't listed.

The result is a dictionary ready for `RUSTICDOC_TRANSLATIONS_DIR`. `rusticdoc render` takes `--lang`, `--translations FILE` and `--timezone`, and reads the same directory, so the command line renders the same way the service does.

## Checking translations

When `/v1/templates/validate` has a `lang`, from the request or the template, it warns about each tag that passes `t` a literal key with no translation in that language or one it falls back to. A translation with no language doesn't count, since that's what the warning is about. Warnings don't make `ok` false.

```bash
printf '{{ "total" | t }} {{ "due" | t }}' > v.txt
curl -F template=@v.txt -F lang=fr \
  -F 'translations={"fr": {"total": "Montant"}}' \
  http://localhost:9027/v1/templates/validate -o v.json
jq -e '.ok
  and (.diagnostics | length == 1)
  and (.diagnostics[0].message | startswith("no fr translation for \"due\""))
' v.json
```

`rusticdoc validate --lang fr` does the same on the command line.
