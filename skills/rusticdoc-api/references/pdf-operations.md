# PDF operations

<!-- Generated from site/src/content/docs/docs/guides/pdf-operations.md by site/scripts/generate-skills.mjs. Do not edit. -->

Merge, split, rotate, mark, sign with an image or a certificate, secure, describe and fill PDFs through the /v1/pdf routes.

Each `/v1/pdf/*` route does one job on PDFs you already have. If you want several of them on one document, a single [`/v1/render`](pipeline.md) request runs them in order.

Every route takes `multipart/form-data`, with the files in `files`, in the order the route expects. Each input needs a `.pdf` name and must start with `%PDF`, or you get a `400` that says `is not a PDF`.

Routes that return a PDF also take `outputFilename`, without an extension. Without it, the result keeps the first input's name. All routes accept the [webhook headers](webhooks.md) except the ones that return JSON, which ignore them and respond straight away.

## Combine and divide

### `POST /v1/pdf/merge`

Joins PDFs in the order you send them.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | One or more PDFs |

```bash
curl -F files=@a.pdf -F files=@b.pdf -F outputFilename=both \
  http://localhost:9027/v1/pdf/merge -o both.pdf
head -c4 both.pdf | grep -q %PDF
```

After merging, this route also runs the finishing fields listed in [Convert documents](convert.md#finish-the-result-in-one-request), in the same fixed order.

### `POST /v1/pdf/split`

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | One PDF |
| `splitSpan` | Yes | With `pages`, ranges like `1-3,5`. With `intervals`, the number of pages per output |
| `splitMode` | No | `pages` (default) or `intervals` |
| `splitUnify` | No | With `pages`, `true` puts every range in one file |

A single output comes back as a PDF. Several come back as a ZIP of `<name>_1.pdf`, `<name>_2.pdf` and so on.

```bash
curl -F files=@a.pdf -F files=@b.pdf \
  http://localhost:9027/v1/pdf/merge -o two.pdf
curl -F files=@two.pdf -F splitMode=intervals -F splitSpan=1 \
  http://localhost:9027/v1/pdf/split -o parts.zip
head -c2 parts.zip | grep -q PK
```

A page outside the document gets a `400`, and one split can make at most 1,000 parts.

### `POST /v1/pdf/overlay`

Lays the second PDF over the first, page by page.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | Exactly two PDFs, the base first and then the overlay |

```bash
curl -F files=@a.pdf -F files=@b.pdf \
  http://localhost:9027/v1/pdf/overlay -o laid.pdf
head -c4 laid.pdf | grep -q %PDF
```

## Change pages

### `POST /v1/pdf/rotate`

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | One PDF |
| `rotation` | No | `90` (default), `180`, `270` or `-90` |
| `pages` | No | A page selection like `1-3,5,7-`. All pages by default |

The fields here are `rotation` and `pages`. The `/v1/render` names for them, `rotateAngle` and `rotatePages`, have no effect on this route.

```bash
curl -F files=@a.pdf -F rotation=180 \
  http://localhost:9027/v1/pdf/rotate -o turned.pdf
head -c4 turned.pdf | grep -q %PDF
```

### `POST /v1/pdf/watermark` and `POST /v1/pdf/stamp`

`watermark` draws a mark under the page content and `stamp` draws it over. Both take the same fields.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | The PDF first. Without `text`, a PNG, JPEG or PDF follows as the mark |
| `text` | No | The text of a text mark, up to 64 KB. If it's present, it's the mark |
| `position` | No | `top-left`, `top-center`, `top-right`, `center` (default), `bottom-left`, `bottom-center`, `bottom-right`. `top` and `bottom` also work |
| `opacity` | No | 0 to 1, default 1 |
| `rotation` | No | Degrees, default 0 |
| `scale` | No | Relative to the page width, 0.01 to 1, default 0.5 |
| `fontSize` | No | For text marks, default 24 |
| `colour` or `color` | No | For text marks, `#rrggbb`, default `#808080` |
| `pages` | No | A page selection. All pages by default |

```bash
curl -F files=@a.pdf -F text=DRAFT -F opacity=0.3 -F rotation=45 \
  -F fontSize=48 -F colour=#ff0000 \
  http://localhost:9027/v1/pdf/watermark -o draft.pdf
curl -F files=@a.pdf -F files=@logo.png -F position=top-right -F scale=0.2 \
  http://localhost:9027/v1/pdf/stamp -o branded.pdf
head -c4 draft.pdf | grep -q %PDF
head -c4 branded.pdf | grep -q %PDF
```

Text marks are drawn in Helvetica (see [known limits](https://rusticdoc.com/docs/project/known-limits/)).

### `POST /v1/pdf/sign`

The `signature` field chooses what the route does:

- `signature=image`, the default, places images of signatures at exact coordinates. This is **not** a cryptographic signature.
- `signature=certificate` signs cryptographically with the deployment's key. A PDF reader that validates signatures shows the document as signed, and any later change to the signed bytes breaks the signature.
- `signature=renew` adds a fresh timestamp to a file that's already signed at B-LTA. It signs nothing and uses no key. See [renewing a B-LTA file](#renewing-a-b-lta-file).

#### With an image

> **Caution**
>
> This draws a picture on the page. It adds no certificate, no timestamp and no protection against later changes, and a PDF reader won't show the document as signed.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | The PDF first, then 1 to 50 PNG or JPEG images |
| `page` | Yes, per image | Page number, counting from 1 |
| `x` | Yes, per image | Points from the page's left edge to the image's left edge |
| `y` | Yes, per image | Points from the page's bottom edge to the image's bottom edge |
| `width` | No, per image | Width in points. Defaults to the image's natural size |

Repeat `page`, `x`, `y` and `width` for each image, in the same order.

```bash
curl -F files=@a.pdf -F files=@logo.png -F page=1 -F x=72 -F y=72 -F width=120 \
  http://localhost:9027/v1/pdf/sign -o signed.pdf
head -c4 signed.pdf | grep -q %PDF
```

#### With a certificate

The service signs with the key and certificate in `RUSTICDOC_SIGNING_KEY` and `RUSTICDOC_SIGNING_CERT`, or with a named key the request picks (see [configuration](https://rusticdoc.com/docs/reference/configuration/#pdf-signing)). A request never carries a key. If none is configured, you get a `503` that says so.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | The PDF first, then at most one PNG or JPEG to show the signature with |
| `signature` | Yes | `certificate` |
| `page`, `x`, `y`, `width` | No | Where the signature is shown. If you sent an image, it goes there, as above. Otherwise a box of text goes there with who signed (`signerName`, or the certificate's common name), the date and time, and the reason and location if given, and `width` defaults to 200 points. `page` defaults to 1. Leave all four out for an invisible signature |
| `level` | No | `b-b`, `b-t`, `b-lt` or `b-lta` (see [levels](#levels)). Defaults to `RUSTICDOC_SIGNING_LEVEL`, which is `b-b` unless you set it |
| `certify` | No | Certifies the document instead of approving it, with a DocMDP permission: `1` for no changes at all, `2` for filling forms and signing, `3` for those plus annotations. Only a document's first signature can certify |
| `signingKey` | No | A named key from `RUSTICDOC_SIGNING_KEYS` instead of the default one. A caller who isn't on its allow list gets a `403` |
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

The service writes:

- An incremental update. The original bytes stay untouched at the start of the file and the signature follows them, so an earlier signature in the file stays valid for its revision.
- A signature field in the AcroForm, with its widget on the page, and a `/Sig` dictionary with `/Filter /Adobe.PPKLite`, `/SubFilter /ETSI.CAdES.detached`, a `/ByteRange` that covers the whole file except the signature, and the signing time in `/M`.
- In `/Contents`, a detached CMS signature over the SHA-256 of those byte ranges. Its signed attributes are content-type, message-digest and ESS signing-certificate-v2. There's no signing-time attribute, because PAdES puts the time in `/M`. The signer's certificate and any chain certificates are included.
- The image or box of text, if there is one, as the widget's appearance, so it belongs to the signature and not to the page.
- When certifying, a `/Reference` to the DocMDP transform with the permission in its parameters, and the catalog's `/Perms /DocMDP` pointing at the signature. A document certified with permission 1 refuses later approval signatures with a `400`.

##### Levels

| `level` | What it adds | What it needs |
|---|---|---|
| `b-b` | The signature alone. The only evidence of when it was made is the signer's own claim | A key |
| `b-t` | An RFC 3161 timestamp over the signature value, from the timestamp authority, stored in the `id-aa-signatureTimeStampToken` unsigned attribute. A third party now attests the signing time | `RUSTICDOC_SIGNING_TSA_URL`, plus `RUSTICDOC_SIGNING_TSA_CERT` to trust its tokens |
| `b-lt` | A second incremental update with a `/DSS`. It holds every certificate in the signer's and the authority's chains, an OCSP response for each one that names a responder, a CRL for each one that names only a distribution point, and a `/VRI` entry for the signature keyed by the SHA-1 of its `/Contents`. The signature can then be validated from the file alone, even after the certificates expire or the responders go away | Responders and distribution points the service can reach |
| `b-lta` | A third update with a document timestamp (`/Type /DocTimeStamp`, `/SubFilter /ETSI.RFC3161`) over the whole file, DSS included. It can be [renewed later](#renewing-a-b-lta-file) to keep the evidence alive | The same as `b-lt` |

The service reaches the timestamp authority, OCSP responders, CRL distribution points and certificate issuers under the [outbound rules](https://rusticdoc.com/docs/reference/configuration/#timestamps-and-validation-data) for signing, which means allow and deny lists, private addresses refused unless allowed, no redirects, and time and size limits. Nothing they send is trusted unchecked. A timestamp token must chain to `RUSTICDOC_SIGNING_TSA_CERT` with the timestamping usage, and OCSP responses and CRLs must be current and signed by the certificate's issuer or a responder it authorised (see [configuration](https://rusticdoc.com/docs/reference/configuration/#timestamps-and-validation-data)).

If any of these fails, the request fails with a `502` that names it, and you never get a file at a lower level than you asked for. A certificate that an OCSP responder or CRL reports as revoked fails the request with a `503`.

Every level validates in poppler's `pdfsig` and in pyHanko 0.38, which reports `VALID`. `pdfsig` reports the signature as valid over the total document for B-B and B-T, and over its revision for B-LT and B-LTA, because it doesn't check their later updates. With the timestamp authority and revocation servers stopped, pyHanko still validates the B-LT and B-LTA files with revocation checking on, from what they hold, the B-LTA document timestamp included. `make sign-check` repeats these checks against a local timestamp authority, OCSP responder and CRL.

> **Limits**
>
> - Keys are RSA (PKCS#1 v1.5, SHA-256) or ECDSA P-256, from PEM files or a PKCS#11 token. Anyone who can call this route signs with the default key unless `RUSTICDOC_SIGNING_ALLOW` restricts it, so put the route behind [authentication](authentication.md).
> - Each request adds one signature. To sign again, send the signed file back.
> - With a self-signed certificate the signature validates as intact, but the signer is reported as untrusted and there's nothing to put in a DSS.
> - Text appearances use Helvetica, so characters outside Windows-1252 show as `?`.
> - Nothing renews a B-LTA document timestamp on a schedule. [Renew the file](#renewing-a-b-lta-file) before the authority's certificate expires.

`sign` is also a [pipeline step](pipeline.md) with the same options. From the command line, `rusticdoc pdf sign --key key.pem --cert cert.pem` signs with local files or a `pkcs11:` key, and takes `--level`, `--certify` and `--tsa-url`.

#### Renewing a B-LTA file

A B-LTA file can be validated for as long as the certificate behind its last document timestamp is valid. Renew the file before then, and the evidence carries on under a newer timestamp:

```bash
curl -F files=@signed.pdf -F signature=renew \
  http://localhost:9027/v1/pdf/sign -o renewed.pdf
```

Renewal adds one incremental update. It puts validation data for the last document timestamp into the `/DSS`, then adds a new document timestamp over the whole file. The signed file's bytes stay untouched at the start, so the signatures in it stay valid.

It uses no signing key, only `RUSTICDOC_SIGNING_TSA_URL` and `RUSTICDOC_SIGNING_TSA_CERT`. Send the PDF with `signature=renew` and nothing else, apart from `outputFilename` if you want one.

| Status | When |
|---|---|
| `400` | The file isn't signed at B-LTA, or it doesn't end in a document timestamp that the service trusts and that covers the file unchanged |
| `403` | Signing keys are configured and the caller is on no key's allow list. With no key configured, any caller of the API can renew |
| `502` | The timestamp authority or a revocation source failed. You never get a file that's only partly renewed |
| `503` | No timestamp authority is configured |

From the command line, `rusticdoc pdf sign signed.pdf --renew -o renewed.pdf` does the same, and takes `--tsa-url` and `--tsa-cert`.

Renewal isn't a pipeline step, and nothing runs it for you, so schedule it yourself. It was added in 0.1.1.

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

```bash
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

```bash
curl -F files=@a.pdf -F userPassword=open -F ownerPassword=admin \
  http://localhost:9027/v1/pdf/encrypt -o locked.pdf
head -c4 locked.pdf | grep -q %PDF
```

### `POST /v1/pdf/convert`

Exports existing PDFs as PDF/A or PDF/UA through LibreOffice, which re-imports the page content. Fonts can change as a result, so compare `x-fonts-used` before and after.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | One or more PDFs. Several come back as a ZIP |
| `pdfa` | One of the two | `PDF/A-1b`, `PDF/A-2b`, `PDF/A-3b` or `PDF/A-4` |
| `pdfua` | One of the two | `true` for PDF/UA |

```bash
curl -F files=@a.pdf -F pdfa=PDF/A-2b \
  http://localhost:9027/v1/pdf/convert -o archive.pdf
grep -aq pdfaid archive.pdf
```

### `POST /v1/pdf/factur-x`

Embeds Factur-X invoice XML and marks the PDF for it. The XML isn't checked against a Factur-X schema.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | Exactly one `.pdf` and one `.xml`, in either order |
| `profile` | No | `MINIMUM`, `BASIC_WL`, `BASIC`, `EN16931` (default) or `EXTENDED` |

```bash
cat > factur-x.xml <<'EOF'
<rsm:CrossIndustryInvoice
  xmlns:rsm="urn:un:unece:uncefact:data:standard:CrossIndustryInvoice:100"/>
EOF
curl -F files=@a.pdf -F files=@factur-x.xml -F profile=EN16931 \
  http://localhost:9027/v1/pdf/factur-x -o invoice.pdf
grep -aq factur-x.xml invoice.pdf
```

### `POST /v1/pdf/embed`

Attaches files to a PDF.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | The PDF first, then at least one file to attach |

```bash
curl -F files=@a.pdf -F files=@invoice.json \
  http://localhost:9027/v1/pdf/embed -o attached.pdf
grep -aq invoice.json attached.pdf
```

## Metadata and outlines

### `POST /v1/pdf/metadata/read`

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | One or more PDFs |

Returns JSON, keyed by file name.

### `POST /v1/pdf/metadata/write`

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | One PDF |
| `metadata` | Yes | A non-empty JSON object with keys like `Title`, `Author`, `Subject` and `Keywords`, plus your own. Keys you don't name are kept |

```bash
curl -F files=@a.pdf -F 'metadata={"Title":"Invoice","Author":"Ada"}' \
  http://localhost:9027/v1/pdf/metadata/write -o titled.pdf
curl -F files=@titled.pdf \
  http://localhost:9027/v1/pdf/metadata/read -o meta.json
jq -e '."titled.pdf".Title == "Invoice"' meta.json
```

### `POST /v1/pdf/bookmarks/read`

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | One or more PDFs |

Returns JSON in which each file name maps to an array of `{"title", "page", "children"}`.

### `POST /v1/pdf/bookmarks/write`

Replaces the outline.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | One PDF |
| `bookmarks` | Yes | A JSON array of `{"title", "page", "children"}`. `page` counts from 1, and `children` is optional |

```bash
cat > bookmarks.json <<'EOF'
[
  {"title": "Start", "page": 1, "children": [{"title": "Detail", "page": 1}]}
]
EOF
curl -F files=@a.pdf -F bookmarks=@bookmarks.json \
  http://localhost:9027/v1/pdf/bookmarks/write -o marked.pdf
curl -F files=@marked.pdf \
  http://localhost:9027/v1/pdf/bookmarks/read -o outline.json
jq -e '."marked.pdf"[0].children[0].title == "Detail"' outline.json
```

A `page` past the end of the document isn't refused. In a one-page PDF, a bookmark to page 99 reads back as page 1, so check your page numbers before you send them.

## Forms

### `POST /v1/pdf/forms/fields`

Lists a PDF's form fields.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | One PDF |

Each entry has `name`, `kind` (`text`, `checkbox`, `radio`, `choice`, `signature` or `button`), `page`, `value`, and `options` if the field has any.

```bash
curl -F files=@form.pdf http://localhost:9027/v1/pdf/forms/fields -o fields.json
jq -e 'map(.name) == ["name", "date", "agree"]' fields.json
```

### `POST /v1/pdf/forms/fill`

Fills form fields from JSON, and applies any tags in annotations.

| Field | Required | Meaning |
|---|---|---|
| `files` | Yes | One PDF |
| `data` | No | A JSON object. A field named `customer.name` takes the value at that path |
| `ctx` | No | JSON that tags can read as `ctx` |
| `flatten` | No | `true` fixes the values into the page content, so no fields remain |
| `stripAnnotations` | No | `false` keeps the annotations that held tags. They're removed by default |

```bash
curl -F files=@form.pdf \
  -F 'data={"name":"Ada","date":"2026-11-06","agree":true}' \
  http://localhost:9027/v1/pdf/forms/fill -o filled.pdf
curl -F files=@filled.pdf \
  http://localhost:9027/v1/pdf/forms/fields -o fields.json
jq -e '.[0].value == "Ada" and .[2].value == "Yes"' fields.json
```

See [PDF templates](https://rusticdoc.com/docs/templates/pdf-templates/) for more on form templates.
