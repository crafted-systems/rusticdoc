---
title: HTML and email
seoTitle: "Generate HTML emails and self-contained HTML from templates"
description: Get a document or an HTML template back as one HTML file with its images inside, or as an email ready to send.
sidebar: { order: 3 }
---

Two outputs are made to travel on their own:

- `to=html` gives one HTML file, with its images and stylesheets inside it.
- `to=eml` gives an email: a MIME message with an HTML part, its images attached to it, and a plain-text part. Hand it to your mail server or provider as it is.

Both work on `/v1/convert`, in a `convert` step on [`/v1/render`](/docs/guides/pipeline/), and with `--to` in the [CLI](/docs/reference/cli/). rusticdoc doesn't send email, and nothing is fetched to build one.

## An email from an HTML template

For an email you design yourself, write it as an `.html` template. It's filled with your data and packed as it is, with no LibreOffice involved, so the markup you write is the markup that's sent. The `<title>` becomes the subject.

```bash test
cat > shipped.html <<'EOF'
<!doctype html>
<html><head><title>Order {{ order }} has shipped</title></head>
<body>
<img src="logo.png" alt="Kahawa Roasters" width="120">
<p>Dear {{ name }},</p>
<p>Your order is on its way. <a href="https://example.com/track/{{ order }}">Track it</a></p>
</body></html>
EOF
curl -F template=@shipped.html -F assets=@logo.png \
  -F 'data={"name":"Amani","order":"2041"}' -F steps=convert -F to=eml \
  http://localhost:9027/v1/render -o shipped.eml
grep -q '^Subject: Order 2041 has shipped' shipped.eml
grep -q '^Content-ID: <1.logo.png@rusticdoc>' shipped.eml
```

The images and stylesheets come from the same places as for [HTML to PDF](/docs/guides/html/#assets): `assets` sent with the request, or [assets the service holds](/docs/guides/assets/).

## A document as an email

A Word, OpenDocument or Excel file goes through LibreOffice's HTML export first, and its pictures and charts come along.

```bash test
curl -F files=@drop.docx -F to=eml http://localhost:9027/v1/convert -o drop.eml
grep -q '^Content-Type: multipart/alternative' drop.eml
grep -q '^Content-ID: <' drop.eml
```

LibreOffice's HTML is plain. It keeps text, tables, colours and pictures, but not page layout, so a document that relies on columns, text boxes or exact positions looks different in an inbox. For anything with a designed look, write an HTML template instead.

A presentation comes out as its slides' text, each slide under a heading, with no pictures or layout. `to=html` and `to=eml` suit word processing files and spreadsheets better than `pptx` or `odp`.

## What's in the email

```text
MIME-Version: 1.0
Subject: Order 2041 has shipped
X-Unsent: 1
Content-Type: multipart/alternative

  text/plain        the text of the page
  multipart/related
    text/html       the page, with <img src="cid:1.logo.png@rusticdoc">
    image/png       logo.png, Content-ID <1.logo.png@rusticdoc>
```

- The subject is the page's `<title>`. A title with no text means no `Subject` line. Line breaks in it become spaces, and text that isn't ASCII is encoded.
- There's no `From`, `To`, `Date` or `Message-ID`. Add them yourself, or let your mail server do it. Amazon SES's `SendRawEmail`, an SMTP `DATA` command and most provider APIs that take a raw message accept the file once the addresses are added.
- `X-Unsent: 1` makes Outlook open the file as a draft, ready to address and send.
- Each image the page uses is attached once, however often it appears, and named by `cid:`. Images the page doesn't use are left out. Email clients drop `data:` images, which is why the email doesn't use them.
- The text part has paragraphs on their own lines, table cells apart by a tab, list items starting with `- `, and each link followed by its address in brackets.
- A stylesheet the page links to becomes a `<style>` in the head. Most clients read it, but some only apply styles written on each element, so inline the important ones in the template.

An image named by an `https://` address stays as it is, and the reader's client fetches it when the email is opened. Many clients hide such images until the reader allows them.

## One HTML file

`to=html` puts each image inside the page as a `data:` URI, and each linked stylesheet as a `<style>`, so the file opens anywhere with nothing beside it.

```bash test
curl -F files=@invoice.html -F assets=@invoice.css -F assets=@logo.png -F to=html \
  http://localhost:9027/v1/convert -o packed.html
grep -q 'src="data:image/png;base64,' packed.html
grep -q '<style>' packed.html
```

The same works for a document LibreOffice exports, whose pictures would otherwise be lost:

```bash test
curl -F files=@drop.docx -F to=html http://localhost:9027/v1/convert -o drop.html
grep -q 'src="data:image/' drop.html
```

## Rules for both

- A name that wasn't sent and that the service doesn't hold stays as written, as do `file://` and `http://` addresses. Nothing is read or fetched for them.
- A stylesheet sent with the request and not linked by the page is added at the end of the head, so it applies after the page's own `<style>`, as it does in a PDF.
- `htmlEngine` doesn't apply, because no engine lays the page out.
- These outputs aren't PDFs, so the PDF steps, like `watermark` or `encrypt`, can't follow them, and `merge=true` needs `to=pdf`.
- The limits are those of HTML to PDF: at most 200 assets, and the usual request size. Each `data:` URI is a copy, so a page that names a large image many times can pass the 512 MB a request may produce, and is refused.
