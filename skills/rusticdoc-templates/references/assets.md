# Assets

<!-- Generated from site/src/content/docs/docs/guides/assets.md by site/scripts/generate-skills.mjs. Do not edit. -->

Mount or upload logos, images and stylesheets once, and name them from any template or HTML page instead of sending them with every request.

An asset is an image or a stylesheet the service holds, so a template can name your logo instead of every request carrying it. There are three ways to provide one, and none of them needs a rebuild.

| Way | Needs | Applies to | Survives a restart |
|---|---|---|---|
| Upload to the store | `RUSTICDOC_STORE` | Every caller and replica | Yes |
| Mount at `/assets` | A volume | This replica, for every caller | As long as the mount does |
| Send with a request, as [`assets`](html.md#assets) | Nothing | HTML in that request | No |

When more than one has the same name, the file sent with the request wins, then the mounted one, then the stored one.

Fonts aren't assets. They have [ways of their own](https://rusticdoc.com/docs/guides/fonts/).

## Names

An asset is called by its file name, or by its path when it's in a folder, like `brand/logo.png`.

| Asset | Extensions |
|---|---|
| Image | `png`, `jpg`, `jpeg`, `gif`, `svg`, `webp` |
| Stylesheet | `css` |

A name holds ASCII letters, digits, `.`, `_` and `-`, with `/` between folders. It can be 200 bytes long and 8 folders deep, and no part of it starts with a dot. Upper and lower case are different. Each asset can be at most 20 MB.

## Mount

Mount a directory at `/assets`, or wherever `RUSTICDOC_ASSETS_DIR` points:

```bash
docker run -d -p 127.0.0.1:9027:9027 \
  -v "$PWD/assets:/assets:ro" \
  ghcr.io/crafted-systems/rusticdoc:0.3.0
```

Nothing needs reloading. A file is read when a template or page names it, so a file you add, change or remove counts from the next request. Files with other extensions or names that aren't allowed are passed over.

Links are followed, so a Kubernetes ConfigMap works as a mount. Don't put links in the directory that lead to files you wouldn't publish.

`rusticdoc render` and `rusticdoc convert` read the same directory, so set `RUSTICDOC_ASSETS_DIR` for those too.

## Upload

`POST /v1/assets` takes one file in the `file` field, either an image, a stylesheet or a ZIP of them, and returns `201`.

```bash
curl -H "X-Api-Key: $KEY" -F file=@logo.png -F name=brand/logo.png \
  http://localhost:9027/v1/assets -o uploaded.json
jq -e '.assets[0].name == "brand/logo.png"' uploaded.json
```

```json
{
  "assets": [
    {
      "name": "brand/logo.png",
      "type": "image",
      "size": 1843,
      "mounted": false,
      "uploadedAt": "2026-11-25T09:12:44Z"
    }
  ],
  "skipped": [],
  "syncIntervalSeconds": 30
}
```

`name` is what to store one file under. Without it, the file keeps its own name. In a ZIP, each file's path is its name, so `name` is refused there.

Uploading a name again replaces the asset. Documents rendered after that use the new one.

A file whose content doesn't match its extension, or whose name isn't allowed, fails the whole upload with a `400`. Anything in a ZIP that isn't an image or a stylesheet, like a licence or a font, is reported under `skipped`. A name that's mounted on the replica gets a `409 asset_mounted`, because the upload would never be used there.

A ZIP can be at most 128 MB, expand to at most 256 MB and hold at most 2,000 entries. The request also has to fit `RUSTICDOC_BODY_LIMIT`.

The store holds at most `RUSTICDOC_MAX_STORED_ASSETS` assets (default `2000`) and `RUSTICDOC_MAX_ASSET_STORAGE` bytes (default `1GB`), and an upload past either gets `403 quota_exceeded`. Without a store, you get `501 store_not_configured`.

## What reaches other replicas

| | This replica | Other replicas |
|---|---|---|
| Upload or replacement | In use when the request returns | Within `RUSTICDOC_STORE_SYNC_INTERVAL`, default `30s` |
| Delete | Gone when the request returns | Within the same interval |

Each replica copies the stored assets to `RUSTICDOC_ASSET_CACHE_DIR` and rebuilds that copy when it starts. `/ready` returns `503` until a new replica has its copy, so point your readiness probe at it (see [running many containers](https://rusticdoc.com/docs/operate/many-containers/)).

A mounted asset reaches only the replicas that mount it.

## Use one in an HTML page

Name it where the page would name a file:

```bash
curl -H "X-Api-Key: $KEY" -F file=@logo.png -F name=brand/logo.png \
  http://localhost:9027/v1/assets -o uploaded.json
cat > letter.html <<'EOF'
<img src="brand/logo.png" width="120" height="40">
<p>Dear {{ name }},</p>
EOF
curl -F template=@letter.html -F 'data={"name":"Amani"}' -F steps=convert \
  http://localhost:9027/v1/render -o letter.pdf
grep -aq '/Subtype */Image' letter.pdf
curl -H "X-Api-Key: $KEY" -X DELETE http://localhost:9027/v1/assets/brand/logo.png
```

| Asset | How the page uses it |
|---|---|
| Image | `<img src="brand/logo.png">`, or `url(brand/logo.png)` in CSS |
| Stylesheet | `<link rel="stylesheet" href="brand/page.css">` |

The service sends the layout engine only the assets a page names, so a large library costs a page nothing. Paths always start at the top of the library, including in a stylesheet that is itself in a folder. A leading `./` is ignored, but a leading `/` or `..` finds nothing, and neither does an `http://` address. HTML can't show WebP.

Linked stylesheets apply in the page's order, after the page's own `<style>` and before any sent with the request. A link with `media="screen"` is left out.

A page that names an asset nobody has shows nothing there, with no error. To get an error instead, name it with `asset`:

```html
<img src="{{ asset('brand/logo.png') }}" width="120" height="40">
```

`asset` only looks at the mounted and stored assets, not at files sent with the request. This is for pages Fulgur lays out, so [`htmlEngine=libreoffice`](html.md#choose-libreoffice-instead) and PDF/A ignore assets.

## Use one in an Office template

`asset` gives the [`image`](rich-content.md#image) filter a picture, and [`barcode`](rich-content.md#logos) a logo:

```text
{{ asset("brand/logo.png") | image(width=120) }}
{{ asset(customer.brand ~ ".svg") | image(width=120) }}
{{ url | barcode("qr", logo=asset("brand/mark.svg")) }}
```

A name that isn't there fails the render with a `422`, and [validation](validate.md) reports it:

```text
asset: there is no asset "brand/logo.png"; mount it or upload it
```

Only `asset` reads the library. A name in the data without it, like `{{ logo | image }}` with `{"logo": "logo.png"}`, is refused as before, because it isn't base64.

An Office template sent with no `data`, or an empty one, isn't filled at all, so send some even when the template only uses assets.

## List, download and delete

`GET /v1/assets` lists the mounted and stored assets by name:

| Field | Meaning |
|---|---|
| `name` | What templates and pages call it |
| `type` | `image` or `stylesheet` |
| `size` | Bytes |
| `mounted` | `true` when it's in the mounted directory. If a stored asset has the same name, both are listed and the mounted one is used |
| `uploadedAt` | Only on stored assets. When the content was uploaded |

```bash
curl http://localhost:9027/v1/assets -o assets.json
jq -e 'type == "array"' assets.json
```

`GET /v1/assets/{name}` downloads an asset, with its folders in the path, like `/v1/assets/brand/logo.png`. `DELETE /v1/assets/{name}` removes a stored one and returns `204`, or `404` for an unknown name. A mounted asset can't be deleted this way.

## Who can change assets

Anyone with the [API key](https://rusticdoc.com/docs/guides/authentication/) can upload, replace and delete stored assets, and read every asset. With no key configured, anyone who can reach the service can. Every service on the same store uses the same stored assets. Mounted assets can't be changed through the API, so mount the ones that mustn't change.
