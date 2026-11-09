---
title: Quickstart
description: Go from nothing to a filled invoice PDF in a few commands.
sidebar:
  order: 1
---

On this page you start the service, send it a Word template and some JSON, and get back a filled invoice as a PDF. You need Docker and `curl`.

## Start the service

```bash
docker run -d --name rusticdoc \
  --read-only --cap-drop ALL --security-opt no-new-privileges \
  --tmpfs /tmp --tmpfs /home/rusticdoc:uid=1001,gid=1001 \
  -p 127.0.0.1:9027:9027 ghcr.io/vmosoti/rusticdoc:0.1.0
```

:::note[Pulling the image needs access for now]
`ghcr.io/vmosoti/rusticdoc:0.1.0` is private while the repository is. Sign in first with `docker login ghcr.io`, using a GitHub token that can read packages, or build the image from a checkout of the repository with `docker build -t rusticdoc .` and use `rusticdoc` as the image name in the command above.
:::

With no credentials configured, the service answers anyone who can reach it. The `-p 127.0.0.1:` part keeps it on your machine. [Install](/docs/start/install/) explains the other flags.

Check that it is up:

```bash test
curl http://localhost:9027/ready
```

The answer is JSON that starts with `{"status":"up"`.

## Get the sample

The sample is a Word file with tags such as `{{ customer.name }}` in it, and a JSON file with the values:

```bash
curl -o invoice.docx https://rusticdoc.com/samples/invoice-template.docx
curl -O https://rusticdoc.com/samples/invoice.json
```

## Fill it and convert it

```bash test
curl -F template=@invoice.docx -F "data=<invoice.json" -F steps=convert \
  http://localhost:9027/v1/render -o invoice.pdf
head -c 5 invoice.pdf
```

The last line prints `%PDF-`. Open `invoice.pdf` and you will see invoice INV-2041 for Amani Otieno, with a row for each item and a total of KES 36,300.00.

The request has three parts:

| Part | What it does |
|---|---|
| `template=@invoice.docx` | Uploads the file as the template |
| `data=<invoice.json` | Sends the contents of the file as the text of the `data` field |
| `steps=convert` | Runs one step after filling the template: convert to PDF |

Leave out `steps` to get the filled Word file back:

```bash test
curl -F template=@invoice.docx -F "data=<invoice.json" \
  http://localhost:9027/v1/render -o filled.docx
unzip -p filled.docx word/document.xml | grep -c 'Amani Otieno'
```

`-F "data=<invoice.json"` sends the file's contents as a text field, and needs the quotes so that your shell does not read the `<`. Uploading it as a file does the same thing:

```bash test
curl -F template=@invoice.docx -F data=@invoice.json \
  http://localhost:9027/v1/render -o filled.docx
unzip -p filled.docx word/document.xml | grep -c 'Amani Otieno'
```

## Stop the service

```bash
docker rm -f rusticdoc
```

## Next

- [Concepts](/docs/start/concepts/) explains what happened in that request.
- [Template syntax](/docs/templates/syntax/) shows how to write your own template.
- [Install](/docs/start/install/) covers Compose, building from source and the CLI.
- [Security](/docs/operate/security/) lists what to set before anyone else can reach the service.
