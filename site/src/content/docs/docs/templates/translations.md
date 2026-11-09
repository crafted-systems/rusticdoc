---
title: Translations and settings
description: Set the language, translated labels, time zone and default currency of an Office template, and handle per-request values with data.
sidebar:
  order: 6
---

This page shows how to translate labels, and set a time zone and a default currency, in an Office template. It also shows what to do when these must change from one request to the next.

## Settings live in the template

The `{% settings %}` tag holds four kinds of setting:

```text
{% settings lang="fr" currency="EUR" timezone="+02:00" t.greeting="Bonjour" %}
```

| Setting | Does |
|---|---|
| `lang` | Picks which translation `t` uses |
| `currency` | The code `currency` uses when given none |
| `timezone`, or `tz` | The offset `date` shows moments in |
| `t.<key>` | A translation for `t` |

- Type it anywhere in the document. It applies to the whole file wherever it sits, and is removed from the output.
- A paragraph holding only this tag stays as an empty paragraph, so put it in a line that has other text.
- Several `{% settings %}` tags add up. For a repeated setting, the last one wins.
- Values must be quoted text. `lang=language` is ignored with no error, because settings are read when the template is parsed, before any data exists.

:::caution[Office templates only, and no request fields]
There are no `lang`, `timezone`, `currency` or `translations` fields on `/v1/render`. The service ignores them if you send them.

In a text template or a PDF tag, `{% settings %}` is a syntax error (`unknown statement settings`), and `t` and `date` do not exist.
:::

This example puts all of the settings in the sample invoice:

```bash test
mkdir t && cd t && unzip -q ../invoice.docx
perl -pi -e 's/Billed to/{% settings lang="fr" currency="EUR" timezone="+02:00" t.billed.fr="Facturé à" t.billed.de="Rechnung an" %}{{ "billed" | t }}, {{ 5 | currency }}, {{ "2026-11-06T23:30:00Z" | date("%d\/%m %H:%M") }}/' word/document.xml
zip -qr ../tagged.docx . && cd ..

curl -F template=@tagged.docx -F "data=<invoice.json" http://localhost:9027/v1/render -o out.docx
unzip -p out.docx word/document.xml | grep -o 'Facturé à, EUR 5.00, 07/11 01:30'
```

## Settings in document properties

The same settings can be custom document properties instead, so that nothing has to be typed on the page. Name each property after its setting (`lang`, `currency`, `timezone` or `tz`, `t.<key>`) and give it a text value:

- In Word, Excel and PowerPoint: File > Properties > Custom.
- In LibreOffice: File > Properties > Custom Properties.

A `{% settings %}` tag wins over a property that sets the same thing, so a tag on the page can override what the file's properties say. Translations add up: a `t.<key>` property and a `t.<key>` in the tag both count, and for the same key the tag's wins. Properties with other names are ignored.

They are read from `docProps/custom.xml` in a DOCX, XLSX or PPTX, and from `meta.xml` in an ODT, ODS or ODP. This example writes the property by hand, as Word would save it:

```bash test
mkdir t && cd t && unzip -q ../invoice.docx
mkdir -p docProps
cat > docProps/custom.xml <<'EOF'
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"><property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="2" name="currency"><vt:lpwstr>EUR</vt:lpwstr></property></Properties>
EOF
perl -pi -e 's/Billed to/{{ 5 | currency }}/' word/document.xml
zip -qr ../tagged.docx . && cd ..

curl -F template=@tagged.docx -F "data=<invoice.json" http://localhost:9027/v1/render -o out.docx
unzip -p out.docx word/document.xml | grep -o 'EUR 5.00'
```

## lang and translations

`{{ "key" | t }}` looks for a translation in this order:

1. `key.<lang>`, when `lang` is set.
2. `key`.
3. The key itself, printed as it is, with no error.

So one file can hold every language, and changing `lang` changes which one prints:

```text
{% settings lang="de" t.total="Total" t.total.fr="Montant" t.total.de="Summe" %}
{{ "total" | t }}
```

This prints `Summe`, and a template with `lang="es"` would print `Total`.

`lang` affects `t` and nothing else. `date` prints month and day names in English, and `currency` always uses `,` and `.`.

## timezone

`timezone` is a fixed offset such as `+02:00` or `-05:00`, or `UTC`. It applies to a value that is a moment, meaning one with a zone or unix seconds. A value with no zone prints as written.

Named zones are refused when `date` runs:

```bash test=fail
mkdir t && cd t && unzip -q ../invoice.docx
perl -pi -e 's/Billed to/{% settings timezone="Europe\/Paris" %}Billed to/' word/document.xml
zip -qr ../tagged.docx . && cd ..

curl -F template=@tagged.docx -F "data=<invoice.json" http://localhost:9027/v1/render -o out.docx
```

```text
timezone "Europe/Paris" is not a fixed offset like +02:00 (named zones are not supported)
```

## currency

`currency="EUR"` sets the code for `{{ total | currency }}`. An argument wins over it, and `currency("")` prints no code. See [`currency`](/docs/templates/filters/#currency).

## Changing these per request

Since settings are fixed in the file, send what varies as data. This works in Office and text templates alike.

**Labels.** Send the words the document needs:

```bash test
cat > labels.txt <<'EOF'
{{ labels.total }}: {{ total | currency(code) }}
EOF
curl -F template=@labels.txt \
  -F 'data={"labels":{"total":"Montant"},"total":1500,"code":"EUR"}' \
  http://localhost:9027/v1/render -o out.txt
grep -x 'Montant: EUR 1,500.00' out.txt
```

**Several languages in one file.** Send the language and pick from a table in the data:

```bash test
cat > pick.txt <<'EOF'
{{ labels[lang].total }}
EOF
curl -F template=@pick.txt \
  -F 'data={"lang":"de","labels":{"fr":{"total":"Montant"},"de":{"total":"Summe"}}}' \
  http://localhost:9027/v1/render -o out.txt
grep -x 'Summe' out.txt
```

**Currency.** Pass the code to the filter, as the sample invoice does with `{{ total | currency(currency) }}`.

**Dates and time zones.** Format the date in your own code, in the zone you want, and send the text. This is also the way to get month names in another language, or a named zone with daylight saving.

**A whole template per language** also works: store one per language, and pick the [stored template](/docs/guides/stored-templates/) by id.
