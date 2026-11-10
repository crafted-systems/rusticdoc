# Status

Version 0.1 is the first release. This is what has been run and what hasn't. [Known limits](https://rusticdoc.com/docs/project/known-limits/) lists every limit.

This file stays in the repository, next to the [roadmap](ROADMAP.md). The site has no status page: `/status` and `/docs/project/status` redirect to known limits.

## Proven by running it

| Area | What was run |
|---|---|
| Automated tests | The unit and integration tests pass on every commit, along with the linter and a dependency audit |
| Documentation examples | The shell examples marked as tested run against the real image |
| Several containers on one store | Three replicas on minikube, and against an S3 gateway and a shared volume |
| Attacks at full size | Run in a container limited to 3 GiB. See [attacks run at full size](#attacks-run-at-full-size) |
| Microsoft Office on a Mac | Templates typed in Word, Excel and PowerPoint 16.114 for Mac were filled and open in the same apps with no repair prompt. So do outputs with removed rows, columns, tables, pictures and slides |
| PDF viewers | Filled forms, editable and flattened, show their values in Chrome and in Preview. Text in Arabic, Hebrew, Chinese, Japanese, Korean and Cyrillic draws correctly in both, checked in Chrome 157 |
| Third-party templates | Seven XLSX and PPTX templates from xlsx-template and docxtemplater were rewritten in this syntax and filled with those projects' own test data. Six give the cell values or paragraphs upstream expects. One PowerPoint loop gives the same text in four paragraphs where docxtemplater gives three. All seven open in Excel and PowerPoint 16.114 for Mac with no repair prompt |
| Column loops | Workbooks filled with a [column loop](https://rusticdoc.com/docs/templates/office/#repeating-columns), with formulas that follow the copies and dates from `excel_date`, open in Excel 16.114 for Mac with no repair prompt. Both were added after 0.1.0 |
| Barcodes | All fifteen symbologies decode to their exact values with rxing, from the image and from LibreOffice's PDF, and ZBar reads the linear codes and QR |
| Output across architectures | Fourteen outputs from the amd64 and arm64 images were compared. Office and text files and a filled PDF form are byte-identical, and LibreOffice's PDFs match as page images or in layout within 0.25 pt |
| PDF signatures | Signed files validate in poppler's `pdfsig` and in pyHanko at PAdES B-B, B-T, B-LT and B-LTA, including with keys on a SoftHSM token. A file with a [renewed timestamp](https://rusticdoc.com/docs/guides/pdf-operations/#renewing-a-b-lta-file) was checked with `pdfsig` and OpenSSL |
| Load | The sample invoice was posted at 1 to 16 connections to one container, and all 136,330 requests succeeded. [Sizing](https://rusticdoc.com/docs/operate/sizing/#measured-under-load) has the figures, which are for one small document on one machine |
| SDKs | The generated TypeScript, Python and Go clients compile and pass the same five calls against a running service. See [SDKs](https://rusticdoc.com/docs/project/sdks/) |
| Languages | Numbers, money and dates match CLDR's output for 62 languages and regions, in more than 2,000 checked cases |

## Not checked yet

| Area | What is missing |
|---|---|
| Office for Windows | Only the Mac versions of Word, Excel and PowerPoint have opened the output |
| Adobe Acrobat | Filled forms and signed files haven't been opened in Acrobat or Acrobat Reader |
| Renewed timestamps in other validators | A file with a renewed timestamp hasn't been checked with pyHanko or Acrobat |
| Barcode scanners | Software decoders read every code, but no hardware or phone scanner has been tested |
| Placeholder pictures in Office | Checked in LibreOffice, but not in Word, Excel or PowerPoint |
| AWS and production Kubernetes | Storage has run on an S3 gateway and minikube, not on AWS itself or a production cluster |
| Load at scale | The load test used one small document and one container. Large documents and several replicas haven't been measured |
| Published SDKs | The clients are generated and tested, but no package is published |
| Cloud key stores | Signing keys in a cloud KMS aren't supported. Keys are PEM files or live in a PKCS#11 token, tested with SoftHSM |
| Production use | This is a first release, and it hasn't carried production traffic yet |
| Outside review | Nobody outside the project has audited the service or run a penetration test against it |
| OpenID Connect | Token checks and admin rules were tested with a local key server and RS256 tokens, not against a real identity provider |
| Macros and OLE objects | Taking them out was tested on stand-ins, not on working macros made in Office |
| Hardware key modules | Signing with a key in a PKCS#11 token was tested with SoftHSM, not with a hardware module |
| Render memory limit on macOS | The limit relies on Linux and hasn't been shown to hold on macOS |
| Smaller setups | No CPU minimum and no container smaller than 6 GiB has been measured as safe |
| Other `AWS_*` settings | The store's client library reads them. They're passed through unchanged and aren't tested |
| Store failure on a running replica | New replicas and renders were tested against an S3 endpoint with nothing listening. What a replica already in service does comes from reading the code |
| `pdfcpu` advisory | One advisory is in a module `pdfcpu` builds against. Whether `pdfcpu` reaches it hasn't been checked |

## Attacks run at full size

An OWASP review found 20 issues, 6 of them high severity, and all of them are fixed. A review of those fixes found more gaps, and those are fixed too. These attacks were then run against a container limited to 3 GiB:

| Attack | Before | Now |
|---|---|---|
| 40 doublings of a template variable | Service killed, out of memory | `413` in under a second, service up |
| 40 concurrent 60 MB uploads | Service killed, out of memory | Refused cleanly at a 1.31 GiB peak, service up |
| 100 MB value printed in a loop | Service killed, out of memory | `400`, service up |
| 840,000 same-named form parts | No response in 150 seconds | `400` in 6 ms |
| Document that makes LibreOffice write without stopping | 512 MB of `/tmp` filled in under 15 seconds | `400` at the disk limit, space freed, other conversions unaffected |
| Stylesheet link in a converted HTML file | Fetched by LibreOffice | No request leaves the container |
| Service secrets in `/proc/1/environ` | Readable by LibreOffice | Permission denied |
| An HTML page nested 20,000 levels deep | Not run before | `413` within two seconds at a 256 MB limit, service up |

`make outbound`, `make isolation` and `make disk` keep the last three results true. The features added since (the template sanitiser, sheet copies, previews, underlays, SVG images, barcodes and signing) each went through the same review, and every finding was fixed with a test.
