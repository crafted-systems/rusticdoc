---
title: Certificates in bulk
description: Render a certificate or badge for every person in a JSON list with one request, and get a ZIP of PDFs back, each with its own name, photo or QR code.
sidebar:
  order: 4
---

A course, an event or a staff list needs the same certificate or badge for many people, each with a name, a date and maybe a photo or a code to check it by. Design one certificate in Word, type tags where the details go, and send the list as JSON. `POST /v1/render/batch` fills the template once per person and returns a ZIP. The design stays in the template, and your code only sends the list.

## The template

A certificate is a Word file with a heading and a line of tags. Inside a batch, each render reads its person as `item`, and everything else in the data stays as it is:

```text
Certificate of completion
Awarded to {{ item.name }} for {{ course }} on {{ issued | date("%d %B %Y") }}.
{{ (verify_url ~ item.id) | barcode("qr", size=72) }}
```

The QR code joins a base address and the certificate's id, so anyone can scan it to check the certificate on your site.

## The data

The list sits under one key, and the details every certificate shares sit next to it:

```json
{
  "course": "Barista basics",
  "issued": "2026-11-14",
  "verify_url": "https://example.com/verify/",
  "people": [
    {"id": "C-101", "name": "Ada Lovelace"},
    {"id": "C-102", "name": "Grace Hopper"}
  ]
}
```

## Render the batch

`splitBy` names the list, `batchName` names each file, and `steps=convert` turns each one into a PDF. The example builds the template from the sample `report.docx`, which does the same as typing the tags in Word:

```bash test
mkdir t && cd t && unzip -q ../report.docx
export TITLE='Certificate of completion'
export LINE='Awarded to {{ item.name }} for {{ course }} on'
LINE+=' {{ issued | date("%d %B %Y") }}.'
LINE+=' {{ (verify_url ~ item.id) | barcode("qr", size=72) }}'
perl -pi -e 's{Quarterly report}{$ENV{TITLE}};' \
  -e 's{A document with no tags in it.}{$ENV{LINE}}' word/document.xml
zip -qr ../certificate.docx . && cd ..

cat > people.json <<'EOF'
{
  "course": "Barista basics",
  "issued": "2026-11-14",
  "verify_url": "https://example.com/verify/",
  "people": [
    {"id": "C-101", "name": "Ada Lovelace"},
    {"id": "C-102", "name": "Grace Hopper"}
  ]
}
EOF
curl -F template=@certificate.docx -F data=@people.json \
  -F splitBy=people -F 'batchName=certificate-{{ item.id }}' \
  -F steps=convert -F outputFilename=certificates \
  http://localhost:9027/v1/render/batch -o certificates.zip
unzip -l certificates.zip | grep -o 'certificate-C-101.pdf'
unzip -l certificates.zip | grep -o 'certificate-C-102.pdf'
```

You get `certificates.zip`, with one PDF per person.

## The fields

| Field | What it does |
|---|---|
| `splitBy` | The path to the list, like `people` or `event.attendees` |
| `batchName` | A template for each file's name, without the extension. The default is `{{ batch.index }}`, which gives `1.pdf`, `2.pdf` and so on |
| `steps` | Steps to run on each file, like `convert`, `pdfa` or `encrypt` |
| `outputFilename` | The ZIP's name |

`batchName` sees the same data as the render but only gets the text filters, so `{{ item.name | lower | replace(" ", "-") }}` works and `date` doesn't. Two files with the same name are both kept, and the second gets ` (2)`. Each render can also print `{{ batch.index }}` and `{{ batch.count }}`, like `Certificate 3 of 40`.

## Badges with a photo

For a badge, put a picture in the template where the photo goes and type the tag in its alt text. The sample `badge.docx` has a grey square whose alt text is `{{ photo | image(fit="cover") }}`. Here it reads `item.photo` instead, so each badge gets its own:

```bash test
mkdir t && cd t && unzip -q ../badge.docx
perl -pi -e 's/>Badge</>{{ item.name }}</;' \
  -e 's/\{\{ photo \|/{{ item.photo |/' word/document.xml
zip -qr ../badge-item.docx . && cd ..

photo="$(base64 < logo.png | tr -d '\n')"
jq -n --arg p "$photo" \
  '{staff: [{name: "Ada Lovelace", photo: $p}, {name: "Grace Hopper", photo: $p}]}' \
  > staff.json
curl -F template=@badge-item.docx -F data=@staff.json -F splitBy=staff \
  -F 'batchName={{ item.name }}' -F steps=convert \
  http://localhost:9027/v1/render/batch -o badges.zip
unzip -l badges.zip | grep -o 'Grace Hopper.pdf'
```

The photo takes the picture's frame, position and border, and `fit="cover"` crops it to fill the frame. Photos travel in the data as base64, because the service doesn't fetch URLs. See [placeholder pictures](/docs/templates/rich-content/#placeholder-pictures).

## One PDF with a page each

If you'd rather print one file, skip the batch and loop in a single render. Put the opening tag in the first paragraph and the closing tag in the last, with a page break between people:

```bash test
mkdir t && cd t && unzip -q ../report.docx
export OPEN='{% for p in people %}Certificate of completion'
export CLOSE='Awarded to {{ p.name }}.'
CLOSE+='{% if not loop.last %}{{ "" | pagebreak }}{% endif %}{% endfor %}'
perl -pi -e 's{Quarterly report}{$ENV{OPEN}};' \
  -e 's{A document with no tags in it.}{$ENV{CLOSE}}' word/document.xml
zip -qr ../all.docx . && cd ..

curl -F template=@all.docx \
  -F 'data={"people":[{"name":"Ada"},{"name":"Grace"},{"name":"Alan"}]}' \
  http://localhost:9027/v1/render -o all.docx.out
unzip -p all.docx.out word/document.xml | grep -o '<w:br w:type="page"/>' |
  wc -l | grep -q 2
```

Three people give three pages. Add `steps=convert` for a PDF. See [paragraph loops](/docs/templates/office/#paragraphs).

## Limits

| Limit | Value |
|---|---|
| People in one batch | 100, set with `RUSTICDOC_BATCH_LIMIT` |
| Time for the whole batch | 5 minutes, set with `RUSTICDOC_BATCH_TIMEOUT` |

A batch is all or nothing. If one person's certificate fails, for example because their record lacks a name, you get that error instead of a partial ZIP. For a longer list, send several requests, or add `X-Rusticdoc-Webhook-Url` to get the ZIP by [webhook](/docs/guides/webhooks/). Batch rendering takes Office and text templates, not PDF forms.

## Next

- [Batch rendering](/docs/templates/batch/) covers every field, file naming and the errors.
- [Rich content](/docs/templates/rich-content/) covers images, barcodes and QR codes.
- [Validate a template](/docs/templates/validate/) checks a template against data shaped like one element.
