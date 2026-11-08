# pdfops: the PDF engines

`MultiEngine` tries engines in a fixed order and skips the ones that return
`Error::Unsupported`: **lopdf** (pure Rust, always present), then **qpdf**
and **pdfcpu** when their binaries are configured (`RUSTICDOC_QPDF_BIN`,
`RUSTICDOC_PDFCPU_BIN`). Any other error stops the chain.

| Operation        | lopdf                                                      | qpdf                                                    | pdfcpu                                            |
|------------------|------------------------------------------------------------|---------------------------------------------------------|---------------------------------------------------|
| merge            | yes: pages in order, outlines merged, AcroForm fields kept | `--empty --pages a b -- out`                            | `merge --bookmark-mode preserve`                  |
| split            | yes: ranges, unified ranges, intervals; outlines follow    | `--empty --pages in 1-3 --`, `--split-pages=N`          | `trim -p 1-3`, `split -m span in dir N`           |
| rotate           | yes: adds to the page's `/Rotate`                          | `--rotate=+90:1-3`                                      | `rotate -p 1-3 in 90 out`                         |
| flatten          | yes: widget `/AP /N` drawn as form XObjects                | `--generate-appearances --flatten-annotations=all`      | no                                                |
| optimize         | no                                                         | `--object-streams=generate --recompress-flate --compression-level=9 [--linearize]` | `optimize`             |
| encrypt          | no                                                         | AES-256 through a 0600 `@argfile`                       | no                                                |
| read_metadata    | yes: Info plus `xmp` sub-object                            | no                                                      | no                                                |
| write_metadata   | yes: Info and the XMP packet kept in step                  | no                                                      | no                                                |
| read_bookmarks   | yes                                                        | no                                                      | `bookmarks export` (JSON)                         |
| write_bookmarks  | yes                                                        | no                                                      | `bookmarks import --replace`                      |
| embed            | yes: `EmbeddedFiles` tree, `/AFRelationship /Data`, `/AF`  | no                                                      | no                                                |
| stamp, watermark | yes: text (Helvetica), PNG/JPEG, PDF pages                 | no                                                      | `stamp add`, `watermark add` (text, image, pdf)   |
| facturx          | yes: `factur-x.xml`, `/AF`, `fx:` XMP, extension schema    | no                                                      | no                                                |

Because lopdf is first, the process engines only ever serve `optimize` and
`encrypt` in the standard order; the rest of their implementations are
exercised by `tests/pdfops-container.sh`, which runs the exact flags and
description strings against the real binaries, and by the engine tests when
a binary exists on the machine.

`RUSTICDOC_PDF_ENGINES_*` sets another order for one operation
(`MultiEngine::with_lists`). Engines also implement `read_exif` and
`write_exif` (ExifTool's key names and value shapes, in `exif.rs`),
`optimize_images` (`images.rs`), `encrypt_with`, `set_embed_metadata` and
`facturx_with`.

## Limits

- Documents must be unencrypted PDFs no larger than 256 MB
  (`LopdfEngine::with_max_bytes`); overlay images, overlay PDFs and
  attachments count against the same limit.
- Compressed streams are bounded before they are decoded: XMP packets at
  16 MiB (a larger packet counts as no packet), image data at what its dictionary
  says it holds, the page content of an imported or overlaid PDF at 256 MiB in
  all, and object streams at lopdf's own 256 MiB per stream.
- Each step runs under its timeout: the lopdf engine checks a deadline in
  every loop over pages, outline items, name-tree nodes and split parts, at
  most four lopdf jobs run at once, a split may produce at most 1,000 parts,
  and name-tree walks visit at most 10,000 nodes or entries.
- Text stamps use the base-14 Helvetica with WinAnsi encoding: Latin-1 text
  (plus the usual typographic punctuation) needs no embedded font; other
  characters are written as `?`. Non-Latin text needs the pdfcpu engine,
  which embeds a font.
- Stamp `scale` is the text or image width as a fraction of the page width;
  with `scale: 0` the text uses `font_size` points and an image its pixel
  size. PDF overlays are drawn 1:1 over the page of the same number;
  position, scale and rotation do not apply to them, and overlay pages
  beyond the document's page count are ignored.
- Merge drops named destinations and links between the merged documents'
  pages; outlines are re-created from page numbers. Split keeps only the
  bookmarks that point at kept pages.
- Flatten leaves widgets without an appearance stream (and the AcroForm
  while any remain) untouched; hidden and NoView widgets are removed without
  drawing. Radio and checkbox states follow `/AS`.
- `write_metadata` writes `Title`, `Author`, `Subject`, `Keywords`,
  `Creator`, `Producer`, `CreationDate` and `ModDate` to the Info dictionary
  and, when the document has an XMP packet, to `dc:title`, `dc:creator`,
  `dc:description`, `pdf:Keywords`, `xmp:CreatorTool`, `pdf:Producer`,
  `xmp:CreateDate` and `xmp:ModifyDate`; unknown keys become custom Info
  entries. A `null` value removes the key. Dates are accepted as PDF
  (`D:20240102030405Z`) or ISO 8601 and written in the form each place
  expects. A document without XMP gets none, so a plain PDF stays plain.
- `facturx` embeds `factur-x.xml` (`/AFRelationship /Data` for MINIMUM and
  BASIC WL, `/Alternative` otherwise), adds the Factur-X extension schema
  and `fx:` properties, and sets `pdfaid:conformance B` only when the input
  already declares `pdfaid:part 3`. A valid PDF/A-3 result needs an input
  exported as PDF/A-3 (`steps=pdfa` with `PDF/A-3b`); the engine does not
  embed fonts or add output intents.
- The `xmp` sub-object of `read_metadata` lists the XMP properties found
  (`dc:title`, `dc:creator`, `pdf:Producer`, `xmp:CreateDate`, ...,
  `pdfaid:part`, `pdfaid:conformance`); Info values win for the top-level
  keys and XMP fills the gaps.

## Files

- `mod.rs`: the `PdfEngine` trait, `MultiEngine`, the operation types,
  `Deadline`, `page_count`.
- `lopdf.rs`, `draw.rs`: the pure-Rust engine (structure, metadata, outlines,
  attachments; content-stream drawing).
- `qpdf.rs`, `pdfcpu.rs`: the process engines.
- `pages.rs`: page selections (`1-3,5`). `xmp.rs`: XMP packet editing.
- `sign/`: cryptographic signatures, outside the engine chain because they
  need the deployment's key: `keys.rs` (PEM loading and checks, signing in
  aws-lc-rs), `cms.rs` (the detached CMS `SignedData`), `incremental.rs` (the
  update appended after the original bytes), `tests.rs` (independent
  verification of each signature).
- `fixtures.rs`: PDFs built in code for the tests and
  `examples/pdfops_fixtures.rs`; `tests.rs`: the engine tests.
