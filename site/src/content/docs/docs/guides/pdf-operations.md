---
title: PDF operations
description: Merge, split, rotate, mark, sign with an image or a certificate, secure, describe and fill PDFs through the /v1/pdf routes.
sidebar: { order: 3 }
---

The `/v1/pdf/*` routes each do one job on PDFs you already have. After this page you can pick the route for a job and send it the right fields.

Every route takes `multipart/form-data`. Files go in `files`, in the order the route expects. Each input must have a `.pdf` name and start with `%PDF`, or the response is a `400` that says `is not a PDF`.

Routes that return a PDF also take `outputFilename`, without an extension. Without it the result keeps the first input's name. They accept the [webhook headers](/docs/guides/webhooks/), except the routes that answer in JSON, which ignore them and answer at once.

If you want several of these on one document, a single [`/v1/render`](/docs/guides/pipeline/) request runs them in order.

## Combine and divide

### `POST /v1/pdf/merge`

Joins PDFs in the order sent.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | One or more PDFs |

```bash test
curl -F files=@a.pdf -F files=@b.pdf -F outputFilename=both \
  http://localhost:9027/v1/pdf/merge -o both.pdf
head -c4 both.pdf | grep -q %PDF
```

This route also runs the finishing fields listed in [Convert documents](/docs/guides/convert/#finish-the-result-in-one-request) after merging, in the same fixed order.

### `POST /v1/pdf/split`

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | One PDF |
| `splitSpan` | Yes | With `pages`, ranges such as `1-3,5`. With `intervals`, the pages per output |
| `splitMode` | No | `pages`, the default, or `intervals` |
| `splitUnify` | No | With `pages`: `true` puts every range in one file |

One output comes back as a PDF. Several come back as a ZIP of `<name>_1.pdf`, `<name>_2.pdf` and so on.

```bash test
curl -F files=@a.pdf -F files=@b.pdf http://localhost:9027/v1/pdf/merge -o two.pdf
curl -F files=@two.pdf -F splitMode=intervals -F splitSpan=1 \
  http://localhost:9027/v1/pdf/split -o parts.zip
head -c2 parts.zip | grep -q PK
```

A page outside the document is a `400`. One split may make at most 1,000 parts.

### `POST /v1/pdf/overlay`

Lays the second PDF over the first, page by page.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | Exactly two PDFs: the base, then the overlay |

```bash test
curl -F files=@a.pdf -F files=@b.pdf http://localhost:9027/v1/pdf/overlay -o laid.pdf
head -c4 laid.pdf | grep -q %PDF
```

## Change pages

### `POST /v1/pdf/rotate`

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | One PDF |
| `rotation` | No | `90`, the default, `180`, `270` or `-90` |
| `pages` | No | A page selection such as `1-3,5,7-`. All pages when absent |

This route reads `rotation` and `pages`. `rotateAngle` and `rotatePages` are the names on `/v1/render`, and here they have no effect.

```bash test
curl -F files=@a.pdf -F rotation=180 http://localhost:9027/v1/pdf/rotate -o turned.pdf
head -c4 turned.pdf | grep -q %PDF
```

### `POST /v1/pdf/watermark` and `POST /v1/pdf/stamp`

`watermark` draws a mark under the page content, and `stamp` draws it over. The fields are the same.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | The PDF first. Without `text`, then a PNG, a JPEG or a PDF as the mark |
| `text` | No | The text of a text mark, up to 64 KB. If present, it is the mark |
| `position` | No | `top-left`, `top-center`, `top-right`, `center` (default), `bottom-left`, `bottom-center`, `bottom-right`. `top` and `bottom` also work |
| `opacity` | No | 0 to 1, default 1 |
| `rotation` | No | Degrees, default 0 |
| `scale` | No | Relative to the page width, 0.01 to 1, default 0.5 |
| `fontSize` | No | Text marks, default 24 |
| `colour` or `color` | No | Text marks, `#rrggbb`, default `#808080` |
| `pages` | No | A page selection. All pages when absent |

```bash test
curl -F files=@a.pdf -F text=DRAFT -F opacity=0.3 -F rotation=45 -F fontSize=48 -F colour=#ff0000 \
  http://localhost:9027/v1/pdf/watermark -o draft.pdf
curl -F files=@a.pdf -F files=@logo.png -F position=top-right -F scale=0.2 \
  http://localhost:9027/v1/pdf/stamp -o branded.pdf
head -c4 draft.pdf | grep -q %PDF
head -c4 branded.pdf | grep -q %PDF
```

Text a mark draws uses Helvetica. See [known limits](/docs/project/known-limits/).

### `POST /v1/pdf/sign`

Signs a PDF in one of two ways, chosen by the `signature` field:

- `signature=image`, the default, places images of signatures at exact coordinates. It draws a picture on the page and is **not** a cryptographic signature.
- `signature=certificate` signs cryptographically with the deployment's key. A PDF reader that validates signatures shows the document as signed, and any later change to the signed bytes breaks the signature.

#### With an image

:::caution
This draws a picture on the page. It adds no certificate, no timestamp and no protection against later changes, and a PDF reader will not show the document as signed.
:::

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | The PDF first, then 1 to 50 PNG or JPEG images |
| `page` | Yes, per image | Page number, from 1 |
| `x` | Yes, per image | Points from the page's left edge to the image's left edge |
| `y` | Yes, per image | Points from the page's bottom edge to the image's bottom edge |
| `width` | No, per image | Width in points. Default is the image's natural size |

Repeat `page`, `x`, `y` and `width` for each image, in the same order.

```bash test
curl -F files=@a.pdf -F files=@logo.png -F page=1 -F x=72 -F y=72 -F width=120 \
  http://localhost:9027/v1/pdf/sign -o signed.pdf
head -c4 signed.pdf | grep -q %PDF
```

#### With a certificate

The service signs with the key and certificate in `RUSTICDOC_SIGNING_KEY` and `RUSTICDOC_SIGNING_CERT`, or with a named key the request chooses (see [configuration](/docs/reference/configuration/#pdf-signing)). A request never carries a key. Without one configured, this answers `503` and says so.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | The PDF first, then at most one PNG or JPEG to show the signature with |
| `signature` | Yes | `certificate` |
| `page`, `x`, `y`, `width` | No | Where the signature is shown. With an image, the image goes there, as above. With no image, a box of text goes there: who signed (`signerName`, or the certificate's common name), the date and time, and the reason and location when given; `width` defaults to 200 points. `page` defaults to 1. Leave them all out for an invisible signature |
| `level` | No | `b-b`, `b-t`, `b-lt` or `b-lta`; see [levels](#levels). Default `RUSTICDOC_SIGNING_LEVEL`, which is `b-b` unless set |
| `certify` | No | Certify the document instead of approving it, with a DocMDP permission: `1` no changes at all, `2` filling forms and signing, `3` that and annotations. Only a document's first signature can certify |
| `signingKey` | No | A named key from `RUSTICDOC_SIGNING_KEYS` instead of the default one. A caller not on its allow list gets `403` |
| `reason` | No | Why the document was signed |
| `location` | No | Where it was signed |
| `contactInfo` | No | How to reach the signer |
| `signerName` | No | The signer's name as shown. Validators identify the signer by the certificate, not by this |

Each text field holds at most 1,024 bytes.

```bash
curl -F files=@contract.pdf -F signature=certificate \
  -F reason=Approved -F location=Nairobi -F level=b-lt \
  -F x=360 -F y=72 \
  http://localhost:9027/v1/pdf/sign -o signed.pdf
pdfsig signed.pdf
```

What the service writes:

- An incremental update. The original bytes stay as they are, at the start of the file, and the signature follows them, so an earlier signature in the file stays valid for its revision.
- A signature field in the AcroForm, with its widget on the page, and a `/Sig` dictionary with `/Filter /Adobe.PPKLite`, `/SubFilter /ETSI.CAdES.detached`, `/ByteRange` covering the whole file but the signature, and the signing time in `/M`.
- In `/Contents`, a detached CMS signature over the SHA-256 of those byte ranges. Its signed attributes are content-type, message-digest and ESS signing-certificate-v2. It carries no signing-time attribute, because PAdES puts the time in `/M`. The signer's certificate and any chain certificates are included.
- With an image or text, that as the widget's appearance, so it is part of the signature rather than of the page.
- When certifying, a `/Reference` to the DocMDP transform with the permission in its parameters, and the catalog's `/Perms /DocMDP` pointing at the signature. A document certified with permission 1 refuses later approval signatures with `400`.

##### Levels

| `level` | Adds | Needs |
|---|---|---|
| `b-b` | The signature alone. A validator knows when it was made only from the signer's claim | A key |
| `b-t` | An RFC 3161 timestamp over the signature value, from the timestamp authority, in the `id-aa-signatureTimeStampToken` unsigned attribute. The signing time is now attested by a third party | `RUSTICDOC_SIGNING_TSA_URL` |
| `b-lt` | A second incremental update with a `/DSS`: every certificate of the signer's and the authority's chains, an OCSP response for each that names a responder and a CRL for each that names only a distribution point, and a `/VRI` entry for the signature keyed by the SHA-1 of its `/Contents`. The signature can be validated from the file alone, after the certificates expire or the responders go away | Responders and distribution points the service can reach |
| `b-lta` | A third update: a document timestamp (`/Type /DocTimeStamp`, `/SubFilter /ETSI.RFC3161`) over the whole file, DSS included, which can be renewed later to keep the evidence alive | The same |

The timestamp authority, OCSP responders, CRL distribution points and certificate issuers are reached under the [outbound rules](/docs/reference/configuration/#timestamps-and-validation-data) for signing: allow and deny lists, private addresses refused unless allowed, no redirects, and time and size limits. If any of them fails, the request fails with `502` and names it; there is no file at a lower level than asked for. A certificate an OCSP responder or CRL reports revoked fails the request with `503`.

Each level has been checked with poppler's `pdfsig`, which reports the signature valid (covering the total document for B-B and B-T, and its revision for B-LT and B-LTA, whose later updates it does not check), and with pyHanko 0.38, which reports it `VALID`. With the timestamp authority and revocation servers stopped, pyHanko validates the B-LT and B-LTA files with revocation checking on from what they hold, the B-LTA document timestamp included. `make sign-check` repeats this against a local timestamp authority, OCSP responder and CRL. Nothing has been opened in Adobe Acrobat.

:::note[Limits]
- RSA (PKCS#1 v1.5, SHA-256) or ECDSA P-256 keys, from PEM files or a PKCS#11 token. Everyone who can call this route signs with the default key unless `RUSTICDOC_SIGNING_ALLOW` restricts it, so put it behind [authentication](/docs/guides/authentication/).
- One signature per request. To sign again, send the signed file back.
- A self-signed certificate gives a signature that validates as intact, with the signer reported as untrusted, and has nothing to put in a DSS.
- Text appearances use Helvetica, so characters outside Windows-1252 show as `?`.
- The document timestamp of B-LTA is not renewed by the service; renew it with a tool such as pyHanko's `ltaupdate` before the authority's certificate expires.
:::

`sign` is also a [pipeline step](/docs/guides/pipeline/) with the same options, and `rusticdoc pdf sign --key key.pem --cert cert.pem` signs from the command line with local files or a `pkcs11:` key, with `--level`, `--certify` and `--tsa-url`.

## Finish and secure

### `POST /v1/pdf/flatten`

Fixes forms and annotations into the page content.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | One PDF |

### `POST /v1/pdf/optimize`

Recompresses streams and drops unused objects.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | One PDF |

```bash test
curl -F files=@form.pdf http://localhost:9027/v1/pdf/flatten -o flat.pdf
curl -F files=@a.pdf http://localhost:9027/v1/pdf/optimize -o small.pdf
head -c4 flat.pdf | grep -q %PDF
head -c4 small.pdf | grep -q %PDF
```

### `POST /v1/pdf/encrypt`

Encrypts with AES-256.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | One PDF |
| `userPassword` | One of the two | The password that opens the file |
| `ownerPassword` | One of the two | The password that lifts restrictions. Defaults to `userPassword` |

```bash test
curl -F files=@a.pdf -F userPassword=open -F ownerPassword=admin \
  http://localhost:9027/v1/pdf/encrypt -o locked.pdf
head -c4 locked.pdf | grep -q %PDF
```

### `POST /v1/pdf/convert`

Exports existing PDFs as PDF/A or PDF/UA through LibreOffice. LibreOffice re-imports the page content, so fonts can change. Compare `x-fonts-used` before and after.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | One or more PDFs. Several come back as a ZIP |
| `pdfa` | One of the two | `PDF/A-1b`, `PDF/A-2b`, `PDF/A-3b` or `PDF/A-4` |
| `pdfua` | One of the two | `true` for PDF/UA |

```bash test
curl -F files=@a.pdf -F pdfa=PDF/A-2b http://localhost:9027/v1/pdf/convert -o archive.pdf
grep -aq pdfaid archive.pdf
```

### `POST /v1/pdf/factur-x`

Embeds Factur-X invoice XML and marks the PDF for it. The service does not check the XML against a Factur-X schema.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | Exactly one `.pdf` and one `.xml`, in either order |
| `profile` | No | `MINIMUM`, `BASIC_WL`, `BASIC`, `EN16931` (default) or `EXTENDED` |

```bash test
printf '<rsm:CrossIndustryInvoice xmlns:rsm="urn:un:unece:uncefact:data:standard:CrossIndustryInvoice:100"/>' > factur-x.xml
curl -F files=@a.pdf -F files=@factur-x.xml -F profile=EN16931 \
  http://localhost:9027/v1/pdf/factur-x -o invoice.pdf
grep -aq factur-x.xml invoice.pdf
```

### `POST /v1/pdf/embed`

Attaches files.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | The PDF first, then at least one file to attach |

```bash test
curl -F files=@a.pdf -F files=@invoice.json http://localhost:9027/v1/pdf/embed -o attached.pdf
grep -aq invoice.json attached.pdf
```

## Metadata and outlines

### `POST /v1/pdf/metadata/read`

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | One or more PDFs |

Answers JSON, keyed by file name.

### `POST /v1/pdf/metadata/write`

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | One PDF |
| `metadata` | Yes | A non-empty JSON object of keys such as `Title`, `Author`, `Subject`, `Keywords`, plus your own. Keys you do not name are kept |

```bash test
curl -F files=@a.pdf -F 'metadata={"Title":"Invoice","Author":"Ada"}' \
  http://localhost:9027/v1/pdf/metadata/write -o titled.pdf
curl -F files=@titled.pdf http://localhost:9027/v1/pdf/metadata/read -o meta.json
jq -e '."titled.pdf".Title == "Invoice"' meta.json
```

### `POST /v1/pdf/bookmarks/read`

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | One or more PDFs |

Answers JSON: each file name maps to an array of `{"title", "page", "children"}`.

### `POST /v1/pdf/bookmarks/write`

Replaces the outline.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | One PDF |
| `bookmarks` | Yes | A JSON array of `{"title", "page", "children"}`. `page` counts from 1, and `children` is optional |

```bash test
curl -F files=@a.pdf -F 'bookmarks=[{"title":"Start","page":1,"children":[{"title":"Detail","page":1}]}]' \
  http://localhost:9027/v1/pdf/bookmarks/write -o marked.pdf
curl -F files=@marked.pdf http://localhost:9027/v1/pdf/bookmarks/read -o outline.json
jq -e '."marked.pdf"[0].children[0].title == "Detail"' outline.json
```

A `page` past the end of the document is not refused. In a one-page PDF, a bookmark to page 99 reads back as page 1, so check your page numbers before you send them.

## Forms

### `POST /v1/pdf/forms/fields`

Lists a PDF's form fields.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | One PDF |

Each entry has `name`, `kind` (`text`, `checkbox`, `radio`, `choice`, `signature` or `button`), `page`, `value`, and `options` where the field has them.

```bash test
curl -F files=@form.pdf http://localhost:9027/v1/pdf/forms/fields -o fields.json
jq -e 'map(.name) == ["name", "date", "agree"]' fields.json
```

### `POST /v1/pdf/forms/fill`

Fills form fields from JSON, and applies any tags in annotations.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | One PDF |
| `data` | No | A JSON object. A field named `customer.name` takes that path |
| `ctx` | No | JSON visible to tags as `ctx` |
| `flatten` | No | `true` fixes the values into the page content, so no fields remain |
| `stripAnnotations` | No | `false` keeps the annotations that held tags. Default is to remove them |

```bash test
curl -F files=@form.pdf -F 'data={"name":"Ada","date":"2026-11-06","agree":true}' \
  http://localhost:9027/v1/pdf/forms/fill -o filled.pdf
curl -F files=@filled.pdf http://localhost:9027/v1/pdf/forms/fields -o fields.json
jq -e '.[0].value == "Ada" and .[2].value == "Yes"' fields.json
```

More on form templates is in [PDF templates](/docs/templates/pdf-templates/).
