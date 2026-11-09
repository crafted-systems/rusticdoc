---
title: Security overview
description: How rusticdoc handles hostile documents, what it was tested against, and what stays your responsibility.
sidebar:
  order: 0.5
---

A document service reads complicated files from people it doesn't know, and rusticdoc is built to contain what those files try to do. The settings and the detail are in the [security guide](/docs/operate/security/).

## What it was tested against

An OWASP review found 20 issues, 6 of them high severity, and all of them are fixed. An automated review of those fixes found more gaps, and those are fixed too. These attacks were run at full size against a container limited to 3 GiB:

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

`make outbound` and `make isolation` keep the stylesheet and secrets rows true, and `make disk` does the same for the row above them. You can run all three yourself.

The features added since then (the template sanitiser, sheet copies, previews, underlays, SVG images, barcodes and signing) each went through the same automated review, and every finding was fixed with a test.

## What the service does

- Runs client templates and HTML layout in a worker process with no environment and limits on memory, CPU and files. A template or page that exhausts its limit gets a `413`, and the service keeps running.
- Caps input before it expands, including request bodies, form parts, template output, font ZIPs, SVG references and what one conversion may write to disk.
- Escapes values by file type, so data can't add markup to Office, HTML, XML and SVG templates, or keys to JSON ones.
- Removes anything that could run or reach out from every Office output: macro projects, OLE objects and ActiveX controls, external links and links to private addresses. A part it can't read with certainty is refused instead of passed through. A request that trusts its template can keep them.
- Keeps secrets out of child processes. LibreOffice, qpdf, pdfcpu and pdftoppm start from an empty environment.
- Stops documents reaching out. LibreOffice fetches nothing, HTML is laid out by Fulgur with no network, and images and SVG references are never fetched.
- Refuses private addresses for webhooks, `downloadFrom`, timestamp authorities and revocation servers, pins the address it checked and doesn't follow redirects.
- Checks what signing relies on. Timestamps must come from an authority you trust, and OCSP responses and CRLs must be current and signed by the right issuer.
- Signs webhooks per caller, so one tenant can't send another's receiver something that verifies.
- Runs hardened by default, as a non-root user with a read-only root filesystem and every capability dropped.

## What is left to you

- Set credentials. With none configured, anyone who can reach the service can use it, and it warns at every start.
- Terminate TLS in front of it, because the service speaks plain HTTP.
- Rate limit at a proxy. The limits bound what one client can hold, but many clients together can still fill them for as long as the timeouts allow.
- Rebuild the image regularly to pick up Debian's fixes.
- Back up and encrypt your bucket. It holds the only copy of stored templates.

## Where protection stops

- A compromised LibreOffice runs as the service's user. It can't read the service's secrets or memory, but it can see other requests' temporary files while they exist. Treat the container as the boundary between tenants that don't trust each other, and run one per tenant if that matters.
- Escaping covers text and quoted attributes. It doesn't make a value safe in a `<script>` or `<style>` block, in an unquoted attribute or as a URL, and `.csv` and `.md` templates aren't escaped at all. Check values like those in your application.
- The render memory limit has only been shown to hold on Linux.
- The image has known CVEs in Debian's packages. Trivy reports 691 findings, 1 critical and 108 high, none with a Debian fix at the time of the scan. See the [security guide](/docs/operate/security/) for the details.
