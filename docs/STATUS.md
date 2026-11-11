# Status

What hasn't been checked yet. The [roadmap](ROADMAP.md) lists what's left to build, and [known limits](https://rusticdoc.com/docs/project/known-limits/) lists every limit.

The site has no status page: `/status` and `/docs/project/status` redirect to known limits.

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
| Outside review | Nobody outside the project has audited the service or run a penetration test against it |
| OpenID Connect | Token checks and admin rules were tested with a local key server and RS256 tokens, not against a real identity provider |
| Macros and OLE objects | Taking them out was tested on stand-ins, not on working macros made in Office |
| Hardware key modules | Signing with a key in a PKCS#11 token was tested with SoftHSM, not with a hardware module |
| Render memory limit on macOS | The limit relies on Linux and hasn't been shown to hold on macOS |
| Smaller setups | No CPU minimum and no container smaller than 6 GiB has been measured as safe |
| Other `AWS_*` settings | The store's client library reads them. They're passed through unchanged and aren't tested |
| Store failure on a running replica | New replicas and renders were tested against an S3 endpoint with nothing listening. What a replica already in service does comes from reading the code |
| `pdfcpu` advisory | One advisory is in a module `pdfcpu` builds against. Whether `pdfcpu` reaches it hasn't been checked |
