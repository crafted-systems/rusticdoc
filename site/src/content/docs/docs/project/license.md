---
title: Licence
description: rusticdoc's licence and the licences of what it ships with.
sidebar:
  order: 5
---

## rusticdoc

rusticdoc is licensed under the [Apache License 2.0](https://github.com/crafted-systems/rusticdoc/blob/main/LICENSE), and the image carries a copy at `/usr/share/doc/rusticdoc/LICENSE`.

## Third-party software

The Rust dependencies use permissive licences like MIT, Apache-2.0, BSD, ISC, Zlib and Unicode-3.0.

The image also carries these, which run as separate processes:

| Software | Licence |
|---|---|
| LibreOffice | MPL-2.0 |
| qpdf | Apache-2.0 |
| pdfcpu | Apache-2.0 |

The image is built on Debian 13 and installs other Debian packages, including Python and fonts. Each package's terms are in the image at `/usr/share/doc/<package>/copyright`.
