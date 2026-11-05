# rusticdoc

A document engine in Rust: fills Office and PDF templates from JSON with `{{customer.name}}` tags, converts the result with LibreOffice, and finishes it with the PDF operations Gotenberg has (PDF/A, merge, split, metadata, encrypt, watermark, stamp, Factur-X).

It takes templating from Carbone and conversion and PDF handling from Gotenberg, covers documents only (no HTML or URL rendering), and ships as one hardened image with no Node, Java or Chromium.

Design: https://claude.ai/artifact/4ZBnc3L7jTH1ov5TKH8qs3

Status: design only. Nothing here runs yet.
