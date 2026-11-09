---
title: HTML to PDF
description: Turn HTML and CSS you write for print into PDF with Fulgur, a layout engine with no browser and no network access.
sidebar: { order: 2 }
---

HTML goes to [Fulgur](https://github.com/fulgur-rs/fulgur), a layout engine written in Rust that writes the PDF itself. It needs no browser, runs no JavaScript and fetches nothing, so the images, stylesheets and fonts a page uses travel with the request.

```bash test
curl -F files=@invoice.html -F assets=@invoice.css -F assets=@logo.png \
  http://localhost:9027/v1/convert -o invoice.pdf
head -c4 invoice.pdf | grep -q %PDF
```

`.html`, `.htm` and `.xhtml` files go to Fulgur whenever the output is PDF, both on `/v1/convert` and in a `convert` step on [`/v1/render`](/docs/guides/pipeline/). Every other format still goes to LibreOffice.

## What it is for

Fulgur suits HTML written to be printed: invoices, statements, reports and letters you control. It doesn't suit live web pages.

| Works | Doesn't work |
|---|---|
| Flexbox, grid, tables, borders, backgrounds and rounded corners | JavaScript, which never runs |
| `@page` size and margins, page counters, headers and footers in margin boxes, running elements | Fetching a URL. There's no route that renders one |
| `break-before`, `break-after`, `break-inside`, orphans and widows | `<link href>`, `@import` and `url()` that name a file. Send the file as an asset instead |
| PNG, JPEG, GIF and SVG images sent as assets | |
| Bookmarks from headings, and tagged PDF/UA-1 | |

Fulgur's own [CSS support reference](https://github.com/fulgur-rs/fulgur/blob/main/docs/css-support.md) lists every property. Fulgur hasn't reached version 1.0 yet, so check a new template's output before you rely on it.

## Assets

Send each file the page needs as `assets`. The service checks them before it lays anything out.

| Asset | Extensions | How the page uses it |
|---|---|---|
| Image | `png`, `jpg`, `jpeg`, `gif`, `svg` | By file name, as `<img src="logo.png">` |
| Stylesheet | `css` | Applies to the whole page, after the page's own `<style>` |
| Font | `ttf`, `otf`, `ttc`, `woff2` | By its family name in `font-family` |

Anything else in `assets`, like a `.js` file, gets a `400` that names the file. A request can hold at most 200 assets.

If a page names a file it wasn't sent, nothing shows there and there's no error. A `file://` or `http://` address shows nothing either, and nothing is read or fetched.

## Page layout

The page's CSS decides the layout, and A4 portrait is the default.

```css
@page {
  size: A4;
  margin: 20mm;
  @bottom-center { content: "Page " counter(page) " of " counter(pages) }
}
h2 { break-before: page }
```

`landscape=true` turns the default page to landscape, but an `@page { size }` rule wins over it.

## HTML templates

An `.html` template is filled first and then laid out. Its values are [escaped for HTML](/docs/templates/syntax/#escaping).

```bash test
cat > letter.html <<'EOF'
<p>Dear {{ name }},</p><p>Your order has shipped.</p>
EOF
curl -F template=@letter.html -F 'data={"name":"Amani"}' -F steps=convert \
  http://localhost:9027/v1/render -o letter.pdf
head -c4 letter.pdf | grep -q %PDF
```

## PDF/UA and PDF/A

`pdfua=true` on `/v1/convert`, or a `pdfua` step on `/v1/render`, makes Fulgur write tagged PDF/UA-1 with bookmarks from the headings. Give the page a language with `<html lang="en">`.

PDF/A is written by LibreOffice, so with `pdfa` set, HTML goes to LibreOffice instead and gets its layout limits.

## Fonts

Fulgur uses the same fonts as LibreOffice (the ones in the image, the ones in the mounted directory and the stored fonts), plus any sent as `fonts` or `assets`. See [fonts](/docs/guides/fonts/).

A family the service doesn't have falls back to another font with no error. `x-fonts-used` on the response names the fonts in the PDF.

## Choose LibreOffice instead

`htmlEngine=libreoffice` sends HTML to LibreOffice, which is how it worked before Fulgur. LibreOffice ignores `assets` and only handles simple layouts. Any other value gets a `400`.

## Limits

These come from the layout engine and showed up in testing:

- Margin boxes ignore their alignment, so text in `@top-right` or `@bottom-center` starts at the left of its box.
- An image in a flex row needs both its width and its height. With only one set, like `height: 40px`, it isn't drawn. Set both, in CSS or as `width` and `height` attributes.
- The CLI prints a line like `ERROR: Unexpected token` for each malformed tag it parses. The page still converts, and the service discards these lines.

Layout runs in the same render workers that fill templates, so `RUSTICDOC_RENDER_WORKERS` and `RUSTICDOC_RENDER_MEMORY` apply to it. A page that's too large or complex to lay out fails with `413` or `422`, and the service carries on. In testing with a 256 MB limit, a page nested 20,000 levels deep, a 40,000-row table and 10 MB of text each got a `413` within two seconds.
