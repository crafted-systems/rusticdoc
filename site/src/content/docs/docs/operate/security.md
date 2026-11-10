---
title: Security
description: What to set up around rusticdoc, what to do where tenants don't trust each other, and how to check the image you run.
sidebar:
  order: 5
---

rusticdoc contains what a hostile document tries to do, and the [security overview](/docs/project/security-overview/) says how. This page is what you do around it.

## Before anyone else can reach it

- Set credentials with `RUSTICDOC_API_KEYS`, basic auth or OpenID Connect, and set `RUSTICDOC_WEBHOOK_SECRET`. With none, anyone can use the service and it warns at every start. A half-configured setup stops it from starting. See [authentication](/docs/guides/authentication/).
- Rate limit at a reverse proxy. The per-caller and connection limits bound what one client can hold, but many clients together can still fill them for as long as the timeouts allow.
- Terminate TLS in front of it, because the service speaks plain HTTP.
- Rebuild the image regularly, because fixes from Debian only arrive when you rebuild. `make docker` uses the digests pinned in the `Dockerfile`.
- Run it with the flags under [Install](/docs/start/install/), and do not add `--init`, which puts a process at PID 1 that exposes the environment.
- Protect the store. Back up the bucket and turn on its encryption. It's the only copy, and templates can hold whatever your documents hold.
- Give `/tmp` room. The disk limit applies to each conversion and is measured every 100 ms, so several conversions together can use `RUSTICDOC_WORKERS` times the limit and a little more.
- Run it on Linux. The render memory limit relies on it.
- Keep `docker exec` for people you'd trust with the secrets. Anyone who can run it can read the environment.

## Where tenants don't trust each other

- Run one container per tenant. The container is the boundary between them: a compromised LibreOffice runs as the service's user, and it can see other requests' temporary files while they exist.
- Prefer a bucket with keys in the environment. A compromised LibreOffice can read anything else the service's user can, like a `file://` store or a web identity token mounted for a pod's S3 role.
- Give each tenant its own key. A caller reaches only its own stored templates, aliases and jobs, and webhooks are signed per caller. See [authentication](/docs/guides/authentication/).
- Keep the admin key to yourself. Fonts are one set for the whole deployment, and a font decides what the digits look like in everyone's documents.

## Templates and files

- Check values yourself where escaping doesn't reach. In `.html`, `.xml` and `.svg` templates it covers text and quoted attributes, not a `<script>` or `<style>` block, an unquoted attribute or a URL. Nothing is escaped in `.csv` and `.md` templates, where a value starting with `=` can run as a formula in a spreadsheet. See [escaping](/docs/templates/syntax/#escaping).
- Don't use a conversion to clean a document. Rendering takes macros, OLE objects, ActiveX controls, external links and hyperlinks into private networks out of its output (see [what rendering takes out](/docs/templates/office/#what-rendering-takes-out)). A file sent to a conversion route is converted as it is, though LibreOffice doesn't run its macros or fetch its links.
- Send `keep` only with templates you wrote or trust. It leaves in what rendering would take out.

## Check the image you run

Scan it. The last Trivy scan reported 691 findings in Debian packages, 1 critical and 108 high, none with a Debian fix at the time of the scan. One more advisory, with no severity, is in a module `pdfcpu` builds against. These numbers will change.

Three `make` targets attack a running image and fail if it gives way. They need Docker and the image, and they start their own containers:

```bash
make outbound IMAGE=rusticdoc:local
make isolation IMAGE=rusticdoc:local
make disk IMAGE=rusticdoc:local
```

- `make outbound` sends crafted documents through the conversion routes: remote images, linked sections, `WEBSERVICE()`, external cell links and `file://` references, in DOCX, PPTX, XLSX, ODP, the flat ODF formats, HTML, XHTML, SVG and RTF. Before that it checks that the container can reach its listener, because otherwise the run would prove nothing. It fails if any document makes a request or leaks a local file.
- `make disk` converts documents that make LibreOffice write without stopping, and checks that the disk limit stops them, frees the space and leaves other conversions running.
- `make isolation` checks that the environments of `soffice`, its bridge and a render worker hold no secrets, and that the service's own `/proc/<pid>/environ`, `mem` and `maps` are closed to that user.

To see a limit hold, send a template that doubles a variable 40 times. It fails with `413`, and the next request works:

```bash test=fail
python3 -c 'print("{% set x = \"aaaaaaaa\" %}"
  + "{% set x = x ~ x %}" * 40 + "{{ x | length }}")' > bomb.txt
curl -F template=@bomb.txt -F 'data={"a":1}' http://localhost:9027/v1/render
```

[Known limits](/docs/project/known-limits/) lists what doesn't work and what could surprise you.
