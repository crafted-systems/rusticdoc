---
title: Quickstart
description: Start the service, send it a Word template and some JSON, and get a filled invoice back as a PDF.
sidebar:
  order: 1
---

You'll start the service, send it a Word template with some JSON, and get a filled invoice back as a PDF. All you need is Docker and `curl`.

## Start the service

```bash
docker run -d --name rusticdoc \
  --read-only --cap-drop ALL --security-opt no-new-privileges \
  --tmpfs /tmp --tmpfs /home/rusticdoc:uid=1001,gid=1001 \
  -p 127.0.0.1:9027:9027 ghcr.io/crafted-systems/rusticdoc:0.1.1
```

There's no authentication set up yet, so anyone who can reach the port can use the service. That's why the command publishes it with `-p 127.0.0.1:`, which keeps it on your machine. [Install](/docs/start/install/) explains the other flags.

Check that it's running:

```bash test
curl http://localhost:9027/ready
```

You should get JSON back that starts with `{"status":"up"`.

## Get the sample

The sample is a Word file with tags like `{{ customer.name }}` typed into it, plus a JSON file with the values to fill them:

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

The last line should print `%PDF-`. Open `invoice.pdf` and you'll find invoice INV-2041 for Amani Otieno, with a row for each item and a total of KES 36,300.00.

Here's what each part of that request does:

| Part | What it does |
|---|---|
| `template=@invoice.docx` | Uploads the file as the template |
| `data=<invoice.json` | Sends the file's contents as the text of the `data` field |
| `steps=convert` | Converts the filled template to PDF |

If you leave out `steps`, you get the filled Word file back instead:

```bash test
curl -F template=@invoice.docx -F "data=<invoice.json" \
  http://localhost:9027/v1/render -o filled.docx
unzip -p filled.docx word/document.xml | grep -c 'Amani Otieno'
```

Keep the quotes around `-F "data=<invoice.json"`, or your shell will try to read the `<` as a redirect. You can also upload the JSON as a file, which does the same thing:

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

- [Concepts](/docs/start/concepts/) walks through what happened in that request.
- [Template syntax](/docs/templates/syntax/) shows you how to write your own template.
- [Install](/docs/start/install/) covers Compose, building from source and the CLI.
- [Security](/docs/operate/security/) lists what to set before anyone else can reach the service.
