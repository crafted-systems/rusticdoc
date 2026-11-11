# Roadmap

What isn't done yet, and what's planned.

## Checks not run

1. Open the output in Office for Windows.
2. Open filled forms, signed files and a renewed file in Adobe Acrobat, and validate a renewed file with pyHanko.
3. Scan the barcodes with a hardware or phone reader.
4. Run the store against AWS itself, and the Kubernetes manifest on a production cluster.
5. Load test with larger documents on more than one machine.
6. Check that the render memory limit holds outside Linux.
7. Check whether `pdfcpu` reaches the advisory in a module it builds against.

## To build

- Publish the SDK packages for TypeScript, Python and Go.
- Signing keys in a cloud KMS, and encrypted key files.

## Open issues

- LibreOffice's PDFs differ between amd64 and arm64 by hundredths of a point, so golden PDFs are byte-identical across the two only for output the service writes itself.
- A PowerPoint loop across two paragraphs gives four paragraphs where docxtemplater gives three.
- A compromised LibreOffice runs as the service's user. It can't read the service's secrets or memory, but it can see other requests' temporary files.
- There's no rate limiting, so many clients together can fill the connection and memory budgets for as long as the timeouts allow. A proxy in front has to do it.
- Trivy reports 691 findings in the image's Debian packages, 1 critical and 108 high, none with a Debian fix at the time of the scan. Rebuild when fixes ship.

## Waiting on other projects

- `der` 0.8 and `x509-cert` 0.3 need `cms` 0.3, which is only a release candidate. Dependabot skips them until it's released.
- `parley` and `krilla` move when Fulgur moves to newer versions, since form shaping uses the ones inside it.
