---
title: Status
description: What has been proven in rusticdoc 0.1 by running it, and what has not been checked yet.
sidebar:
  order: 0
---

Version 0.1 is the first release. Before you depend on it, this page shows what has been run and what has not. [Known limits](/docs/project/known-limits/) lists every limit, and the [roadmap](/docs/project/roadmap/) what comes next.

## Proven by running it

| Area | What was run |
|---|---|
| Automated tests | The unit and integration tests pass on every commit, with the linter and a dependency audit |
| Documentation examples | The shell examples marked as tested run against the real image |
| Several containers on one store | Three replicas on minikube, and against an S3 gateway and a shared volume |
| Attacks at full size | Run in a container limited to 3 GiB. See the [security overview](/docs/project/security-overview/) |
| Microsoft Office on a Mac | Templates typed in Word, Excel and PowerPoint 16.114 for Mac were filled and open in the same apps with no repair prompt, and so do outputs with removed rows, columns, tables, pictures and slides |
| PDF viewers | Filled forms, editable and flattened, show their values in Chrome 154 and in Preview, and text in Arabic, Hebrew, Chinese, Japanese, Korean and Cyrillic draws correctly in Preview |
| Barcodes | All fifteen symbologies decode to their exact values with rxing, from the image and from LibreOffice's PDF, and ZBar reads the linear codes and QR |
| Output across architectures | Fourteen outputs from the amd64 and arm64 images: Office and text files and a filled PDF form are byte-identical, and LibreOffice's PDFs match as page images or in layout within 0.25 pt |
| PDF signatures | Signed files validate in poppler's `pdfsig` and in pyHanko at PAdES B-B, B-T, B-LT and B-LTA, also with keys on a SoftHSM token |
| Languages | Numbers, money and dates match CLDR's output for 62 languages and regions, in more than 2,000 checked cases |

## Not checked yet

| Area | What is missing |
|---|---|
| Office for Windows | Only the Mac versions of Word, Excel and PowerPoint have opened the output |
| Adobe Acrobat | Filled forms and signed files have not been opened in Acrobat or Acrobat Reader |
| Chrome and other scripts | Filled forms in scripts other than Latin have been checked in Preview, not in Chrome |
| Barcode scanners | Software decoders read every code. No hardware or phone scanner has been tried |
| Placeholder pictures in Office | Checked in LibreOffice, not in Word, Excel or PowerPoint |
| AWS and production Kubernetes | Storage has run on an S3 gateway and minikube, not on AWS itself or a production cluster |
| Load | No load test has been run, so the docs give no throughput figures |
| Production use | This is a first release, and it has not carried production traffic yet |
