---
title: Contracts and letters from a Word template
description: Build contracts and letters from a Word template with conditional clauses and amounts in words, then watermark, encrypt or sign the PDF.
sidebar:
  order: 5
---

Contracts and letters change by clause, not just by name: a renewal term for one client, a confidentiality paragraph for another. Keep the wording in a Word template your legal or sales team can edit, put the conditions in tags, and send the facts as JSON. The clauses live in the document, not in string concatenation in your code.

## The template

Conditions decide which words and paragraphs appear. This agreement prints the fee in figures and in words, and adds a renewal clause only when the data asks for it:

```text
Service agreement with {{ client.name }}

The fee is {{ fee | currency("EUR") }} ({{ fee | words(currency="EUR") }}) a month{% if auto_renew %}, and the agreement renews each year unless either party gives notice{% endif %}.
```

The sentence about the fee is one paragraph, so the `if` only adds or removes the words between its tags.

## The data

```json
{
  "client": { "name": "Amani Otieno" },
  "fee": 1250.5,
  "auto_renew": true
}
```

## Fill it

The example builds the template from the sample `report.docx`, which does the same as typing the tags in Word:

```bash test
mkdir t && cd t && unzip -q ../report.docx
export TITLE='Service agreement with {{ client.name }}'
export CLAUSE='The fee is {{ fee | currency("EUR") }}'
CLAUSE+=' ({{ fee | words(currency="EUR") }}) a month{% if auto_renew %},'
CLAUSE+=' and the agreement renews each year unless either party gives'
CLAUSE+=' notice{% endif %}.'
perl -pi -e 's{Quarterly report}{$ENV{TITLE}};' \
  -e 's{A document with no tags in it.}{$ENV{CLAUSE}}' word/document.xml
zip -qr ../agreement.docx . && cd ..

cat > agreement.json <<'EOF'
{
  "client": { "name": "Amani Otieno" },
  "fee": 1250.5,
  "auto_renew": true
}
EOF
curl -F template=@agreement.docx -F data=@agreement.json \
  http://localhost:9027/v1/render -o agreement-filled.docx
unzip -p agreement-filled.docx word/document.xml |
  grep -o 'fifty euros and fifty cents) a month, and the agreement renews'

jq '.auto_renew = false' agreement.json > fixed-term.json
curl -F template=@agreement.docx -F data=@fixed-term.json \
  http://localhost:9027/v1/render -o fixed-term.docx
unzip -p fixed-term.docx word/document.xml | grep -o 'fifty cents) a month.'
```

The first contract reads "EUR 1,250.50 (one thousand two hundred fifty euros and fifty cents) a month, and the agreement renews each year unless either party gives notice." The second stops after "a month."

## Clauses

| To | Write | See |
|---|---|---|
| Add words on a condition | `{% if auto_renew %}…{% endif %}` in one paragraph | [Conditions](/docs/templates/conditions/) |
| Add whole paragraphs or table rows | Put `{% if %}` in the first paragraph and `{% endif %}` in the last | [Conditions in Office templates](/docs/templates/conditions/#conditions-in-office-templates) |
| Choose between versions | `{% if %}`, `{% elif %}` and `{% else %}`, each in a paragraph of its own | [else and elif](/docs/templates/office/#else-and-elif) |
| Remove one paragraph | `{{ drop("paragraph") if not nda }}` anywhere in it | [Remove an element](/docs/templates/office/#remove-an-element) |
| Pick a word | `{{ "monthly" if monthly else "yearly" }}` | [Inline conditions](/docs/templates/conditions/#inline-conditions) |
| Spell an amount out | `{{ fee \| words(currency="EUR") }}`, in English, French or Ukrainian | [words](/docs/templates/filters/#words) |
| Print a date | `{{ start \| date("%d %B %Y") }}`, or the end of a term with `{{ start \| date_add(12, "months") \| date("%d %B %Y") }}` | [Dates](/docs/templates/calculations/#dates) |
| List the parties | `{% for p in parties %}…{% endfor %}` | [Loops](/docs/templates/syntax/#loops) |

A missing value stops the render instead of printing a blank, so a contract can't go out with a gap where a name should be. To allow a value to be absent, use `default` or `is defined`. See [missing values are errors](/docs/templates/syntax/#missing-values-are-errors).

## Mark a draft

Add steps after `convert` to finish the PDF in the same request. A `watermark` step draws text under the page content:

```bash test
mkdir t && cd t && unzip -q ../report.docx
perl -pi -e 's{Quarterly report}{Agreement with {{ client.name }}}' \
  word/document.xml
zip -qr ../agreement.docx . && cd ..

curl -F template=@agreement.docx -F 'data={"client":{"name":"Amani Otieno"}}' \
  -F steps=convert,watermark -F watermarkSource=text \
  -F watermarkExpression=DRAFT \
  -F 'watermarkOptions={"op":"0.3","rot":"45","fontsize":"72"}' \
  http://localhost:9027/v1/render -o draft.pdf
head -c 5 draft.pdf
```

Use `stamp` instead to draw the mark over the content, like `PAID` or `COPY`. See [watermark and stamp fields](/docs/guides/pipeline/#watermark-and-stamp-fields).

## Archive and protect the final copy

`pdfa` converts to PDF/A for archiving, `metadata` sets the title and `encrypt` sets a password. `encrypt` goes last:

```bash test
mkdir t && cd t && unzip -q ../report.docx
perl -pi -e 's{Quarterly report}{Agreement with {{ client.name }}}' \
  word/document.xml
zip -qr ../agreement.docx . && cd ..

curl -F template=@agreement.docx -F 'data={"client":{"name":"Amani Otieno"}}' \
  -F steps=pdfa,metadata,encrypt \
  -F 'metadata={"Title":"Service agreement"}' -F userPassword=open-sesame \
  http://localhost:9027/v1/render -o final.pdf
grep -aq '/Encrypt' final.pdf
```

## Sign with a certificate

The `sign` step signs the PDF cryptographically with the deployment's key, so a PDF reader that validates signatures shows the document as signed, and any later change breaks the signature. It needs `RUSTICDOC_SIGNING_KEY` and `RUSTICDOC_SIGNING_CERT` on the service, and without them the request gets a `503`:

```bash
curl -F template=@agreement.docx -F data=@agreement.json \
  -F steps=convert,sign -F reason=Agreed -F location=Nairobi \
  http://localhost:9027/v1/render -o signed.pdf
```

`sign` must be the last step, and it can't be combined with `encrypt`. Send `page`, `x` and `y` for a visible signature, and `level` for a timestamped one. See [signing with a certificate](/docs/guides/pdf-operations/#with-a-certificate) and [configuration](/docs/reference/configuration/#pdf-signing).

## Letters on letterhead

A letter is the same idea with less logic. The `underlay` step lays the converted letter over a PDF of your letterhead, page by page:

```bash test
curl -F template=@letter.txt -F "data=<letter.json" -F underlay=@a.pdf \
  -F steps=convert,underlay http://localhost:9027/v1/render -o letter.pdf
head -c 5 letter.pdf
```

Here `letter.txt` is a plain text template and `a.pdf` stands in for the letterhead. A Word template works the same way. For a letter to each customer in a list, use [batch rendering](/docs/templates/batch/).

## Next

- [Conditions](/docs/templates/conditions/) covers `if`, `elif`, tests and what counts as true.
- [Office formats](/docs/templates/office/) explains how far a block reaches in a Word document.
- [The pipeline](/docs/guides/pipeline/) lists every step and its fields.
- [PDF operations](/docs/guides/pdf-operations/) covers the same finishing on PDFs you already have.
