---
title: Fonts
description: Add fonts by uploading them, mounting them or sending them with one request, and check which fonts a document used.
sidebar: { order: 5 }
---

When a document names a font the service doesn't have, it comes out in a fallback font and you get no error. There are three ways to add a font, and none of them needs a rebuild.

| Way | Needs | Applies to | Survives a restart |
|---|---|---|---|
| Upload to the store | `RUSTICDOC_STORE` and an admin | Every caller and replica | Yes |
| Mount at `/fonts` | A volume, then a reload | This replica, for every caller | As long as the mount does |
| Send with a request | Nothing | That request | No |

## Upload a ZIP

`POST /v1/fonts` takes one file in the `file` field, either a ZIP of TTF, OTF or TTC files or one such file, and returns `201`.

```bash test
curl -H "X-Api-Key: $ADMIN_KEY" -F file=@brand-fonts.zip \
  http://localhost:9027/v1/fonts -o uploaded.json
jq -e '.fonts[0].file == "font.ttf" and (.skipped | length) == 0' uploaded.json
curl -H "X-Api-Key: $ADMIN_KEY" -X DELETE \
  "http://localhost:9027/v1/fonts/$(jq -r '.fonts[0].id' uploaded.json)"
```

The response lists what was stored and what in the ZIP was left out:

```json
{
  "fonts": [
    {
      "id": "f1e899278b7b4491aba5b6a8253c4b04c050cc59b21865be5c37559a775153cd",
      "file": "font.ttf",
      "size": 81600,
      "faces": [{ "family": "Caladea", "style": "normal", "weight": 400 }],
      "uploadedAt": "2026-11-06T20:34:06Z"
    }
  ],
  "skipped": [{ "file": "LICENSE.txt", "reason": "not a font" }],
  "syncIntervalSeconds": 30
}
```

The `id` is the hex SHA-256 of the font file, so uploading the same font again changes nothing.

A file that has a font extension but isn't a readable font fails the whole upload with a `400`. Anything else in a ZIP, like a licence, is reported under `skipped`, and so are WOFF, WOFF2, EOT, PFB and PFA files, which aren't supported. A ZIP with no fonts in it gets a `400`, as does a form with more than one file.

The upload and each font can be at most 128 MB. A ZIP can expand to at most 256 MB and hold at most 2,000 entries.

The store holds at most `RUSTICDOC_MAX_STORED_FONTS` fonts (default `2000`) and `RUSTICDOC_MAX_FONT_STORAGE` bytes (default `1GB`), and an upload past either gets `403 quota_exceeded`. Without a store, you get `501 store_not_configured`.

## What reaches other replicas

| | This replica | Other replicas |
|---|---|---|
| Upload | In use when the request returns | Within `RUSTICDOC_STORE_SYNC_INTERVAL`, default `30s` |
| Delete | Gone when the request returns | Within the same interval |

LibreOffice needs fonts as files, so each replica copies the stored fonts to `RUSTICDOC_FONT_CACHE_DIR` and rebuilds that copy when it starts. `/ready` returns `503` until a new replica has its copy, so point your readiness probe at it (see [running many containers](/docs/operate/many-containers/)).

A mounted font reaches only the replicas that mount it and call reload, and fonts sent with a request reach nothing else.

## Mount and reload

Mount fonts at `/fonts`, or wherever `RUSTICDOC_FONTS_DIR` points, then ask the service to reload:

```bash
curl -X POST -H "X-Api-Key: $KEY" http://localhost:9027/v1/fonts/reload
```

```json
{ "families": 218, "faces": 346, "changed": true }
```

`changed` is `false` when the directories haven't changed, and then no LibreOffice worker restarts. Otherwise idle workers restart on the new set, and busy ones finish their current job first.

The route acts at most once every 10 seconds. A second call inside that gets `429 too_many_requests`, so retry after a few seconds. It takes any valid credential, so you don't need an admin key. With a store, it also syncs stored fonts on this replica.

## Fonts for one request

Send `fonts` files with a [`/v1/render`](/docs/guides/pipeline/) or [`/v1/convert`](/docs/guides/convert/) request and they apply to that conversion alone.

```bash test
curl -F files=@report.docx -F fonts=@font.ttf \
  http://localhost:9027/v1/convert -o report.pdf
head -c4 report.pdf | grep -q %PDF
```

A request can send at most 8 fonts of up to 32 MB each, as raw TTF, OTF or TTC files and not as a ZIP. The job runs on a worker of its own that's discarded afterwards, which makes it slower. One example took about 1.4 seconds instead of about 100 ms, so keep this in mind if your `RUSTICDOC_API_TIMEOUT` is short.

## List

`GET /v1/fonts` lists every face available, whether it comes from the image, the mount or the store. It takes any valid credential.

```bash test
curl http://localhost:9027/v1/fonts -o fonts.json
jq -e 'map(.family) | index("Caladea") != null' fonts.json
```

| Field | Meaning |
|---|---|
| `family`, `style`, `weight` | The face, like `Caladea`, `normal`, `400` |
| `file` | The file's name, without a path |
| `mounted` | `true` when it came from the mounted directory |
| `id` | Only on stored fonts. The id to delete it by |

## Delete

`DELETE /v1/fonts/{id}` removes a stored font and returns `204`, or `404` for an unknown id. Only an admin can call it. Documents that name the font then fall back to another one, with no error.

A font from the image or a mount has no `id`, so you can't delete it this way.

## Admin keys

Every caller shares the same fonts, and a font decides what the digits look like in everyone's documents, so changing them takes an admin.

| Service is configured with | Who can upload and delete fonts |
|---|---|
| `RUSTICDOC_ADMIN_KEYS` | Those keys, in `X-Api-Key` or `Authorization: Bearer` |
| `RUSTICDOC_BASIC_AUTH_ADMIN=true` | The basic-auth user |
| `RUSTICDOC_OIDC_ADMIN_SUBJECTS` or `RUSTICDOC_OIDC_ADMIN_CLAIM` | The OpenID Connect users they name |
| Other credentials but none of these | Nobody. The request gets `403 admin_required` |
| No credentials at all | Anyone who can reach the service, unless admin keys are set |

Any other API key, tenant key, basic-auth user or token is refused. See [admins](/docs/guides/authentication/#admins) for these settings.

## Chinese, Japanese and Korean fonts

The image has Noto Sans CJK but not Noto Serif CJK, which would add 53 MB. Windows CJK font names map to the Noto variant for their script, so Simplified Chinese never takes the Hong Kong forms:

| A document names | It gets | With Noto Serif CJK added |
|---|---|---|
| SimSun, NSimSun, FangSong, KaiTi | Noto Sans CJK SC | Noto Serif CJK SC |
| PMingLiU, MingLiU | Noto Sans CJK TC | Noto Serif CJK TC |
| MS Mincho, MS PMincho, Yu Mincho | Noto Sans CJK JP | Noto Serif CJK JP |
| Batang, BatangChe, Gungsuh | Noto Sans CJK KR | Noto Serif CJK KR |
| SimHei, Microsoft YaHei, Microsoft JhengHei, MS Gothic, Meiryo, Malgun Gothic, Gulim, Dotum | The Noto Sans CJK variant for its script | No change |

The names match in their own scripts too, like 宋体 and ＭＳ 明朝. None of these keep the original's line breaks, because no free font matches their widths.

To get the serif faces back, add `NotoSerifCJK-Regular.ttc` and `NotoSerifCJK-Bold.ttc` in any of the three ways above. They're under the SIL Open Font License, from the [Noto CJK releases](https://github.com/notofonts/noto-cjk/releases) or Debian's `fonts-noto-cjk`. Each is about 27 MB, under the 128 MB limit for one font. Nothing else needs to change, because the serif names above ask for Noto Serif CJK first.

## Find fonts a template needs

[Validation](/docs/templates/validate/) reports the fonts that a DOCX, XLSX, PPTX, ODT, ODS or ODP template names but the service can neither find nor substitute. They're listed in `missing_fonts`, with a warning for each:

```bash
curl -F template=@letter.docx \
  http://localhost:9027/v1/templates/validate | jq .missing_fonts
```

```json
["Aptos"]
```

A name counts as found when a loaded font has that family, or when the image maps it to a font with the same metrics, like Calibri to Carlito. The mapping is the image's fontconfig file, so the report agrees with what a conversion does. Text in a missing font still renders, but in a fallback font, so its line and page breaks can move. To fix that, upload or mount the font.

Validation reads the fonts named by the document's text, styles and theme. A font used only in an embedded object or a chart isn't checked.

## Check what a document used

A PDF response has an `x-fonts-used` header that lists the fonts named in that PDF.

```bash test
curl -F template=@report.docx -F steps=convert \
  http://localhost:9027/v1/render -D headers.txt -o report.pdf
grep -qi "^x-fonts-used: .*Carlito" headers.txt
```

The service doesn't report a template that names a font it lacks, so compare this header against the font you expected. If a font fell back, the fallback's name is what appears here. The header is absent on results that aren't PDFs, on PDFs over 32 MB, and when no names were found.
