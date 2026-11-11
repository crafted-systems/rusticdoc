---
title: Security overview
description: How rusticdoc handles hostile documents, and where its protection stops.
sidebar:
  order: 0.5
---

A document service reads complicated files from people it doesn't know, and rusticdoc is built to contain what those files try to do. The [security guide](/docs/operate/security/) says what to set up around it.

## What the service does

- Runs client templates and HTML layout in a worker process with no environment and limits on memory, CPU and files. A template or page that exhausts its limit gets a `413`, and the service keeps running.
- Caps input before it expands, including request bodies, form parts, template output, font ZIPs, SVG references and what one conversion may write to disk.
- Escapes values by file type, so data can't add markup to Office, HTML, XML and SVG templates, or keys to JSON ones.
- Removes anything that could run or reach out from every Office output: macro projects, OLE objects and ActiveX controls, external links and links to private addresses. A part it can't read with certainty is refused instead of passed through. A request that trusts its template can keep them.
- Keeps secrets out of child processes. LibreOffice, qpdf, pdfcpu and pdftoppm start from an environment with no secrets in it, and on Linux the service closes its own `/proc` entries to them.
- Stops documents reaching out. LibreOffice fetches nothing, HTML is laid out by Fulgur with no network, and images and SVG references are never fetched.
- Refuses private addresses for webhooks, `downloadFrom`, timestamp authorities and revocation servers, pins the address it checked and doesn't follow redirects.
- Checks what signing relies on. Timestamps must come from an authority you trust, and OCSP responses and CRLs must be current and signed by the right issuer.
- Signs webhooks per caller, so one tenant can't send another's receiver something that verifies.
- Keeps callers apart in the store. A caller reaches only its own stored templates, and another caller's id returns `404`. Uploads and deletes are logged with `log_type: audit`.
- Asks for an admin key to change fonts, because a font decides what the digits look like in everyone's documents.
- Runs hardened by default, as a non-root user with a read-only root filesystem and every capability dropped.

## What is left to you

- Set credentials. With none configured, anyone who can reach the service can use it, and it warns at every start.
- Terminate TLS in front of it, because the service speaks plain HTTP.
- Rate limit at a proxy. The limits bound what one client can hold, but many clients together can still fill them for as long as the timeouts allow.
- Rebuild the image regularly to pick up Debian's fixes.
- Back up and encrypt your bucket. It holds the only copy of stored templates.

The [security guide](/docs/operate/security/) goes through each of these.

## Where protection stops

- A compromised LibreOffice runs as the service's user. It can't read the service's secrets or memory, but it can see other requests' temporary files while they exist. Treat the container as the boundary between tenants that don't trust each other, and run one per tenant if that matters.
- Escaping covers text and quoted attributes. It doesn't make a value safe in a `<script>` or `<style>` block, in an unquoted attribute or as a URL, and `.csv` and `.md` templates aren't escaped at all. Check values like those in your application.
- A file sent to a conversion route is converted as it is. Only rendering takes macros, objects and outside links out.
- Anyone who can `docker exec` into the container can read its environment.
- The image has known CVEs in Debian's packages. Grype reports 761 findings, 31 critical and 204 high, none with a Debian fix at the time of the scan. Scan the image you run.
