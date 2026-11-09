---
title: Security
description: What rusticdoc protects by itself, what it leaves to you, and the risks that remain.
sidebar:
  order: 5
---

## What the service does

- Client templates run in a worker process with no environment and with limits on memory, CPU, files and core dumps. Under memory pressure the kernel kills it first. A template that exhausts its limit gets a `413` and the service keeps running. The memory limit relies on Linux, and hasn't been shown to hold on macOS.
- Input is capped before it expands. That covers request bodies held in memory, form parts, page ranges, split output, stamp text, template output, PDF tags and webhook jobs. The settings are in the [configuration reference](/docs/reference/configuration/).
- Secrets stay out of child processes. LibreOffice, Python, qpdf and pdfcpu start from an allow-listed environment with no secrets in it, and on Linux the service closes its own `/proc` entries to them. Anyone who can `docker exec` into the container can still read the environment.
- Documents can't reach out. LibreOffice is configured to fetch nothing, and its web and CMIS content providers are removed from the image.
- Webhooks and `downloadFrom` refuse private addresses, pin the address they checked and don't follow redirects.
- A caller reaches only its own stored templates, and another caller's id returns `404`. Ids are checked for form before they touch a path, content is checked against its hash when it's read, and names inside a font ZIP are labels, never paths. ZIPs are capped by what actually comes out of them. Uploads and deletes are logged with `log_type: audit`.
- Fonts take an admin key, because a font decides what the digits look like in everyone's documents.

## What it leaves to you

- Set credentials with `RUSTICDOC_API_KEYS`, basic auth or OpenID Connect, and set `RUSTICDOC_WEBHOOK_SECRET`. With none, anyone can use the service and it warns at every start. A half-configured setup stops it from starting. See [authentication](/docs/guides/authentication/).
- Rate limit at a reverse proxy. The per-caller and connection limits bound what one client can hold, but many clients together can still fill them for as long as the timeouts allow.
- Terminate TLS in front of it, because the service speaks plain HTTP.
- Rebuild the image regularly, because fixes from Debian only arrive when you rebuild. `make docker` uses the digests pinned in the `Dockerfile`.
- Run it with the flags under [Install](/docs/start/install/), and do not add `--init`, which puts a process at PID 1 that exposes the environment.
- Protect the store. Back up the bucket and turn on its encryption. It's the only copy, and templates can hold whatever your documents hold.

## Attacks that were run

An OWASP review found 20 issues, 6 of them High, and all of them are fixed. A review of those fixes found more gaps, which are fixed too. After that, these full-size attacks were run in a container limited to 3 GiB:

| Attack | Before | After |
|---|---|---|
| 40 doublings of a template variable | Service killed, out of memory | `413` in under a second, service up |
| 40 concurrent 60 MB uploads | Service killed, out of memory | Refused cleanly at a 1.31 GiB peak, service up |
| 100 MB value printed in a loop | Service killed, out of memory | `400`, service up |
| 840,000 same-named form parts | No response in 150 s | `400` in 6 ms |
| Stylesheet link in a converted HTML file | Fetched by LibreOffice | No request leaves the container |
| Service secrets in `/proc/1/environ` | Readable by LibreOffice | Permission denied |

You can try the first one yourself. It fails with `413`, and the next request works:

```bash test=fail
python3 -c 'print("{% set x = \"aaaaaaaa\" %}"
  + "{% set x = x ~ x %}" * 40 + "{{ x | length }}")' > bomb.txt
curl -F template=@bomb.txt -F 'data={"a":1}' http://localhost:9027/v1/render
```

## Check it yourself

`make outbound` and `make isolation` keep the last two rows of that table true, and `make disk` checks the limit on what a conversion can write to disk. They need Docker and the image, and they start their own containers:

```bash
make outbound IMAGE=rusticdoc:local
make isolation IMAGE=rusticdoc:local
make disk IMAGE=rusticdoc:local
```

- `make outbound` sends crafted documents through the conversion routes: remote images, linked sections, `WEBSERVICE()`, external cell links and `file://` references, in DOCX, PPTX, XLSX, ODP, the flat ODF formats, HTML, XHTML, SVG and RTF. Before that it checks that the container can reach its listener, because otherwise the run would prove nothing. It fails if any document makes a request or leaks a local file.
- `make disk` converts documents that once made LibreOffice write without stopping. It then takes the fix out of the bridge to check that the disk limit stops the real runaway, frees the space and leaves other conversions running.
- `make isolation` checks that the environments of `soffice`, its bridge and a render worker hold no secrets, and that the service's own `/proc/<pid>/environ`, `mem` and `maps` are closed to that user.

All three passed on the image these docs were written against.

## Risks that remain

- Many clients together can fill the connection and memory budgets for as long as the timeouts allow. Rate limiting at a proxy is how you deal with that.
- A compromised LibreOffice runs as the service's user, in the service's container. It can't read the service's secrets or memory, but it can see other requests' temporary files while they exist, and anything else that user can read, like a `file://` store or a web identity token mounted for a pod's S3 role. Where tenants don't trust each other, prefer a bucket with keys in the environment.
- The container is the boundary between tenants. If tenants don't trust each other and that matters to you, run one container per tenant.
- The render memory limit has only been shown to hold on Linux.
- Macros, objects and outside links in a template are taken out of rendered output, but not out of conversions. A render removes macro projects, OLE objects and ActiveX controls, external links and hyperlinks into private networks (see [what rendering takes out](/docs/templates/office/#what-rendering-takes-out) for the list). It also refuses a template it can't read with certainty or clean safely, like one holding an Excel 4.0 macro sheet. A file sent to a conversion route is converted as it is, though LibreOffice doesn't run its macros or fetch its links, which `make outbound` checks.
- The image has known CVEs. The last Trivy scan reported 691 findings in Debian packages, 1 critical and 108 high, none with a Debian fix at the time of the scan. One more advisory, with no severity, is in a module `pdfcpu` builds against, and whether `pdfcpu` reaches it hasn't been checked. These numbers will change, so scan the image you run.

[Known limits](/docs/project/known-limits/) lists what hasn't been proven more generally.
