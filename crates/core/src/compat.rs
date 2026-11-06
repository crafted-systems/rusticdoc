//! Gotenberg's routes as operations on lists of documents.
//!
//! Each route is a fixed order of steps over the documents in flight: the main
//! operation, then watermarks, stamps, rotation, flattening, image optimization,
//! PDF/A and PDF/UA, bookmarks, metadata, embeds, Factur-X and encryption. A
//! document's name is the name it will have in the response: file naming here
//! follows Gotenberg's, including `<input>.pdf` from LibreOffice and zero-based
//! `<stem>_<n>.pdf` split parts. The HTTP layer only reads forms into these
//! structures; failures that a client should read verbatim carry their text in
//! [`Error::InvalidRequest`] or [`Error::Status`].

use std::collections::BTreeMap;
use std::time::Duration;

use crate::convert::ConvertOptions;
use crate::convert::protection::{self, Protection};
use crate::error::{Error, Result};
use crate::job::{Document, Format, PdfaLevel};
use crate::limits;
use crate::pdfops::lopdf::MAX_SPLIT_PARTS;
use crate::pdfops::{
    self, Attachment, Bookmark, EmbedMetadata, EncryptOptions, FacturxProperties, Metadata,
    Overlay, Rotation, SplitMode, pages,
};
use crate::pipeline::{Engines, MAX_DOCUMENTS};

/// `pdfa` and `pdfua` as sent. A PDF/A name is only judged when it is used.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PdfFormats {
    pub pdfa: String,
    pub pdfua: bool,
}

impl PdfFormats {
    pub fn is_empty(&self) -> bool {
        self.pdfa.is_empty() && !self.pdfua
    }

    pub fn is_pdfa3(&self) -> bool {
        matches!(self.pdfa.as_str(), "PDF/A-3a" | "PDF/A-3b" | "PDF/A-3u")
    }

    /// The level LibreOffice exports, or `None` when no PDF/A was asked for.
    pub fn level(&self) -> std::result::Result<Option<PdfaLevel>, UnknownFormat> {
        Ok(match self.pdfa.as_str() {
            "" => None,
            "PDF/A-1b" => Some(PdfaLevel::A1b),
            "PDF/A-2b" => Some(PdfaLevel::A2b),
            "PDF/A-3b" => Some(PdfaLevel::A3b),
            "PDF/A-4" => Some(PdfaLevel::A4),
            _ => return Err(UnknownFormat),
        })
    }
}

/// A `pdfa` value LibreOffice has no export for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownFormat;

/// Text for a format no engine can apply to an existing PDF.
pub const FORMAT_NOT_SUPPORTED: &str = "The requested PDF format is not supported, or no PDF engine could apply it. Valid formats include PDF/A-1b, PDF/A-2b, PDF/A-3b, PDF/A-4, and PDF/UA.";
/// Text for a page selection no split engine understands.
pub const SPLIT_NOT_SUPPORTED: &str = "The requested split mode is not supported, or no PDF engine could process it. Valid modes: 'intervals', 'pages'.";

fn bad(message: impl Into<String>) -> Error {
    Error::InvalidRequest(message.into())
}

/// `splitMode`, `splitSpan` and `splitUnify`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    /// `intervals` or `pages`.
    pub mode: String,
    pub span: String,
    pub unify: bool,
}

/// `bookmarks`: one outline for every document, or one per input name.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Bookmarks {
    #[default]
    None,
    List(Vec<Bookmark>),
    Map(BTreeMap<String, Vec<Bookmark>>),
}

/// The Factur-X fields, once both the level and the XML are present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facturx {
    pub properties: FacturxProperties,
    pub xml: Vec<u8>,
}

/// The name the invoice XML takes inside the PDF.
pub fn facturx_file_name(level: &str) -> &'static str {
    if level == "XRECHNUNG" {
        "xrechnung.xml"
    } else {
        "factur-x.xml"
    }
}

/// The steps that follow a route's main operation. Empty by default.
#[derive(Debug, Clone, Default)]
pub struct PostSteps {
    pub watermarks: Vec<Overlay>,
    pub stamps: Vec<Overlay>,
    pub rotate: Option<(Rotation, String)>,
    pub flatten: bool,
    /// JPEG quality to re-encode images at.
    pub optimize_images: Option<u8>,
    pub formats: PdfFormats,
    pub metadata: Option<Metadata>,
    pub embeds: Vec<Attachment>,
    pub embeds_metadata: BTreeMap<String, EmbedMetadata>,
    pub facturx: Option<Facturx>,
    pub encrypt: EncryptOptions,
}

impl PostSteps {
    /// Anything that would degrade a natively exported PDF/A.
    fn has_post_processing(&self) -> bool {
        !self.watermarks.is_empty()
            || !self.stamps.is_empty()
            || self.rotate.is_some()
            || !self.embeds.is_empty()
            || self.metadata.as_ref().is_some_and(|m| !m.is_empty())
            || self.flatten
            || self.facturx.is_some()
    }
}

/// The combinations no route accepts, in the order Gotenberg checks them.
pub fn validate_steps(steps: &PostSteps) -> Result<()> {
    validate_formats(&steps.formats, &steps.encrypt.user_password, &steps.embeds)?;
    validate_permissions(&steps.encrypt)?;
    if !steps.formats.pdfa.is_empty() && steps.facturx.is_some() && !steps.formats.is_pdfa3() {
        return Err(bad(format!(
            "Invalid form data: Factur-X requires a PDF/A-3 variant (PDF/A-3a, PDF/A-3b, or PDF/A-3u), got '{}'",
            steps.formats.pdfa
        )));
    }
    Ok(())
}

/// PDF/A forbids encryption, and only PDF/A-3 allows attachments.
pub fn validate_formats(
    formats: &PdfFormats,
    user_password: &str,
    embeds: &[Attachment],
) -> Result<()> {
    if !formats.pdfa.is_empty() && !user_password.is_empty() {
        return Err(bad(
            "Invalid form data: PDF/A format is incompatible with encryption",
        ));
    }
    if !formats.pdfa.is_empty() && !embeds.is_empty() && !formats.is_pdfa3() {
        return Err(bad(format!(
            "Invalid form data: PDF format '{}' does not support embedded files; only PDF/A-3 variants allow attachments",
            formats.pdfa
        )));
    }
    Ok(())
}

/// Restrictions mean nothing without a password to anchor them.
pub fn validate_permissions(encrypt: &EncryptOptions) -> Result<()> {
    if encrypt.permissions.restricted()
        && encrypt.user_password.is_empty()
        && encrypt.owner_password.is_empty()
    {
        return Err(bad(
            "Invalid form data: permission restrictions require a 'userPassword' or 'ownerPassword'",
        ));
    }
    Ok(())
}

/// The formats to convert to so the output meets Factur-X's PDF/A-3
/// requirement. An existing PDF that is PDF/A-3 already stays as it is.
pub fn facturx_formats(
    has_facturx: bool,
    formats: &PdfFormats,
    source_is_office: bool,
    inputs: &[Document],
) -> PdfFormats {
    let mut formats = formats.clone();
    if !has_facturx || formats.is_pdfa3() {
        return formats;
    }
    let all_pdfa3 = !source_is_office
        && inputs
            .iter()
            .all(|doc| pdfops::pdfa_part(doc).as_deref() == Some("3"));
    if !all_pdfa3 {
        formats.pdfa = "PDF/A-3b".into();
    }
    formats
}

/// Runs the routes against a service's engines.
#[derive(Debug, Clone, Copy)]
pub struct Compat<'a> {
    pub engines: &'a Engines,
    pub timeout: Duration,
}

impl Compat<'_> {
    fn forms(&self) -> &pdfops::MultiEngine {
        &self.engines.forms
    }

    // ---------------------------------------------------------------- stubs

    /// Merges in order. One document merges into itself.
    pub async fn merge_stub(&self, docs: &[Document]) -> Result<Document> {
        match docs {
            [] => Err(bad("no input paths")),
            [one] => Ok(one.clone()),
            _ => self.forms().merge(docs, self.timeout).await,
        }
    }

    /// Splits each document. Parts keep the stem of their document, numbered
    /// from zero; a unified part keeps the stem alone.
    pub async fn split_stub(&self, split: &Split, docs: &[Document]) -> Result<Vec<Document>> {
        let mut out = Vec::new();
        let mut produced = 0;
        for doc in docs {
            let stem = stem_of(&doc.name);
            let mode = split_mode(split, doc)?;
            let parts = self.forms().split(doc, &mode, self.timeout).await?;
            for part in &parts {
                limits::charge_output(&mut produced, part.bytes.len(), limits::MAX_OUTPUT_BYTES)?;
            }
            if split.unify && split.mode == "pages" {
                let first = parts
                    .into_iter()
                    .next()
                    .ok_or_else(|| bad(SPLIT_NOT_SUPPORTED))?;
                out.push(Document::new(format!("{stem}.pdf"), first.bytes));
            } else {
                for (i, part) in parts.into_iter().enumerate() {
                    out.push(Document::new(format!("{stem}_{i}.pdf"), part.bytes));
                }
            }
            if out.len() > MAX_DOCUMENTS {
                return Err(bad(format!(
                    "splitting would leave more than {MAX_DOCUMENTS} documents in flight"
                )));
            }
        }
        Ok(out)
    }

    async fn each<F, Fut>(&self, docs: Vec<Document>, mut op: F) -> Result<Vec<Document>>
    where
        F: FnMut(Document) -> Fut,
        Fut: std::future::Future<Output = Result<Document>>,
    {
        let mut out = Vec::with_capacity(docs.len());
        for doc in docs {
            let name = doc.name.clone();
            let mut done = op(doc).await?;
            done.name = name;
            out.push(done);
        }
        Ok(out)
    }

    pub async fn watermark_stub(
        &self,
        marks: &[Overlay],
        mut docs: Vec<Document>,
    ) -> Result<Vec<Document>> {
        for mark in marks {
            docs = self
                .each(docs, |doc| async move {
                    self.forms().watermark(&doc, mark, self.timeout).await
                })
                .await?;
        }
        Ok(docs)
    }

    pub async fn stamp_stub(
        &self,
        marks: &[Overlay],
        mut docs: Vec<Document>,
    ) -> Result<Vec<Document>> {
        for mark in marks {
            docs = self
                .each(docs, |doc| async move {
                    self.forms().stamp(&doc, mark, self.timeout).await
                })
                .await?;
        }
        Ok(docs)
    }

    pub async fn rotate_stub(
        &self,
        rotation: Rotation,
        selection: &str,
        docs: Vec<Document>,
    ) -> Result<Vec<Document>> {
        self.each(docs, |doc| async move {
            self.forms()
                .rotate(&doc, rotation, selection, self.timeout)
                .await
        })
        .await
    }

    pub async fn flatten_stub(&self, docs: Vec<Document>) -> Result<Vec<Document>> {
        self.each(docs, |doc| async move {
            self.forms().flatten(&doc, self.timeout).await
        })
        .await
    }

    pub async fn optimize_stub(&self, quality: u8, docs: Vec<Document>) -> Result<Vec<Document>> {
        self.each(docs, |doc| async move {
            self.forms()
                .optimize_images(&doc, quality, self.timeout)
                .await
        })
        .await
    }

    /// Converts existing PDFs to PDF/A or PDF/UA with LibreOffice. Sequential
    /// on purpose: every conversion needs a worker of the same small pool.
    pub async fn convert_stub(
        &self,
        formats: &PdfFormats,
        docs: Vec<Document>,
    ) -> Result<Vec<Document>> {
        if formats.is_empty() {
            return Ok(docs);
        }
        let level = formats.level().map_err(|_| bad(FORMAT_NOT_SUPPORTED))?;
        let office = self
            .engines
            .libreoffice
            .as_ref()
            .ok_or_else(|| Error::EngineUnavailable("LibreOffice is not configured".into()))?;
        let options = ConvertOptions {
            pdfa: level,
            pdfua: formats.pdfua,
            ..ConvertOptions::default()
        };
        self.each(docs, |doc| {
            let options = options.clone();
            async move {
                office
                    .convert_with(&doc, Format::Pdf, &options, self.timeout)
                    .await
            }
        })
        .await
    }

    pub async fn write_metadata_stub(
        &self,
        metadata: Option<&Metadata>,
        docs: Vec<Document>,
    ) -> Result<Vec<Document>> {
        let Some(metadata) = metadata.filter(|m| !m.is_empty()) else {
            return Ok(docs);
        };
        self.each(docs, |doc| async move {
            self.forms().write_exif(&doc, metadata, self.timeout).await
        })
        .await
    }

    pub async fn write_bookmarks_stub(
        &self,
        bookmarks: &Bookmarks,
        docs: Vec<Document>,
    ) -> Result<Vec<Document>> {
        match bookmarks {
            Bookmarks::None => Ok(docs),
            Bookmarks::List(list) if list.is_empty() => Ok(docs),
            Bookmarks::List(list) => {
                self.each(docs, |doc| async move {
                    self.forms().write_bookmarks(&doc, list, self.timeout).await
                })
                .await
            }
            Bookmarks::Map(map) => {
                self.each(docs, |doc| async move {
                    match map.get(&doc.name) {
                        Some(list) => self.forms().write_bookmarks(&doc, list, self.timeout).await,
                        None => Ok(doc),
                    }
                })
                .await
            }
        }
    }

    pub async fn embed_stub(
        &self,
        embeds: &[Attachment],
        metadata: &BTreeMap<String, EmbedMetadata>,
        mut docs: Vec<Document>,
    ) -> Result<Vec<Document>> {
        if !embeds.is_empty() {
            docs = self
                .each(docs, |doc| async move {
                    self.forms().embed(&doc, embeds, self.timeout).await
                })
                .await?;
        }
        if !metadata.is_empty() {
            docs = self
                .each(docs, |doc| async move {
                    self.forms()
                        .set_embed_metadata(&doc, metadata, self.timeout)
                        .await
                })
                .await?;
        }
        Ok(docs)
    }

    pub async fn facturx_stub(
        &self,
        facturx: Option<&Facturx>,
        docs: Vec<Document>,
    ) -> Result<Vec<Document>> {
        let Some(facturx) = facturx else {
            return Ok(docs);
        };
        self.each(docs, |doc| async move {
            self.forms()
                .facturx_with(&doc, &facturx.xml, &facturx.properties, self.timeout)
                .await
        })
        .await
    }

    pub async fn encrypt_stub(
        &self,
        encrypt: &EncryptOptions,
        docs: Vec<Document>,
    ) -> Result<Vec<Document>> {
        if encrypt.user_password.is_empty() && encrypt.owner_password.is_empty() {
            return Ok(docs);
        }
        self.each(docs, |doc| async move {
            self.forms().encrypt_with(&doc, encrypt, self.timeout).await
        })
        .await
    }

    /// Watermarks, stamps, rotation, flattening and optimization.
    async fn before_convert(
        &self,
        steps: &PostSteps,
        mut docs: Vec<Document>,
    ) -> Result<Vec<Document>> {
        docs = self.watermark_stub(&steps.watermarks, docs).await?;
        docs = self.stamp_stub(&steps.stamps, docs).await?;
        if let Some((rotation, selection)) = &steps.rotate {
            docs = self.rotate_stub(*rotation, selection, docs).await?;
        }
        if steps.flatten {
            docs = self.flatten_stub(docs).await?;
        }
        if let Some(quality) = steps.optimize_images {
            docs = self.optimize_stub(quality, docs).await?;
        }
        Ok(docs)
    }

    /// Metadata, embeds, Factur-X and encryption. These come after any
    /// conversion, since LibreOffice drops them on the way to PDF/A.
    async fn after_convert(
        &self,
        steps: &PostSteps,
        mut docs: Vec<Document>,
    ) -> Result<Vec<Document>> {
        docs = self
            .write_metadata_stub(steps.metadata.as_ref(), docs)
            .await?;
        docs = self
            .embed_stub(&steps.embeds, &steps.embeds_metadata, docs)
            .await?;
        docs = self.facturx_stub(steps.facturx.as_ref(), docs).await?;
        self.encrypt_stub(&steps.encrypt, docs).await
    }

    // --------------------------------------------------------------- routes

    /// `/forms/pdfengines/merge`
    pub async fn merge(
        &self,
        inputs: Vec<Document>,
        steps: &PostSteps,
        bookmarks: &Bookmarks,
        auto_index_bookmarks: bool,
        title_bookmarks: bool,
    ) -> Result<Vec<Document>> {
        let merged = self.forms().merge(&inputs, self.timeout).await?;
        let mut docs = vec![Document::new("merged.pdf", merged.bytes)];
        docs = self.before_convert(steps, docs).await?;
        let formats = facturx_formats(steps.facturx.is_some(), &steps.formats, false, &docs);
        docs = self.convert_stub(&formats, docs).await?;
        let outline = self
            .merged_outline(&inputs, bookmarks, auto_index_bookmarks, title_bookmarks)
            .await?;
        if !outline.is_empty() {
            docs = self
                .write_bookmarks_stub(&Bookmarks::List(outline), docs)
                .await?;
        }
        self.after_convert(steps, docs).await
    }

    /// The outline of a merged document: as given, or per input with page
    /// numbers shifted past the inputs before it.
    async fn merged_outline(
        &self,
        inputs: &[Document],
        bookmarks: &Bookmarks,
        auto_index: bool,
        titles: bool,
    ) -> Result<Vec<Bookmark>> {
        let map = match bookmarks {
            Bookmarks::List(list) => return Ok(list.clone()),
            Bookmarks::Map(map) => Some(map),
            Bookmarks::None if auto_index || titles => None,
            Bookmarks::None => return Ok(Vec::new()),
        };
        let mut outline = Vec::new();
        let mut offset = 0u32;
        for input in inputs {
            let mut own = map
                .and_then(|m| m.get(&input.name))
                .cloned()
                .unwrap_or_default();
            // Titles nest each input's own outline under its entry.
            if own.is_empty() && (auto_index || titles) {
                own = self.forms().read_bookmarks(input, self.timeout).await?;
            }
            shift_pages(&mut own, offset);
            if titles {
                outline.push(Bookmark {
                    title: self.document_title(input).await,
                    page: offset + 1,
                    children: own,
                });
            } else {
                outline.append(&mut own);
            }
            offset += pdfops::page_count(input)?;
        }
        Ok(outline)
    }

    /// The input's Title, or its file name without the extension.
    async fn document_title(&self, doc: &Document) -> String {
        let title = self
            .forms()
            .read_exif(doc, self.timeout)
            .await
            .ok()
            .and_then(|metadata| {
                metadata
                    .get("Title")
                    .and_then(|t| t.as_str())
                    .map(str::to_string)
            })
            .filter(|t| !t.trim().is_empty());
        title.unwrap_or_else(|| stem_of(&doc.name))
    }

    /// `/forms/pdfengines/split`
    pub async fn split(
        &self,
        split: &Split,
        inputs: Vec<Document>,
        steps: &PostSteps,
    ) -> Result<Vec<Document>> {
        let docs = self.split_stub(split, &inputs).await?;
        let docs = self.before_convert(steps, docs).await?;
        let formats = facturx_formats(steps.facturx.is_some(), &steps.formats, false, &docs);
        let docs = self.convert_stub(&formats, docs).await?;
        self.after_convert(steps, docs).await
    }

    /// `/forms/pdfengines/embed` and `/forms/pdfengines/factur-x`
    pub async fn embed(
        &self,
        inputs: Vec<Document>,
        formats: &PdfFormats,
        embeds: &[Attachment],
        metadata: &BTreeMap<String, EmbedMetadata>,
        facturx: Option<&Facturx>,
    ) -> Result<Vec<Document>> {
        let formats = facturx_formats(facturx.is_some(), formats, false, &inputs);
        let docs = self.convert_stub(&formats, inputs).await?;
        let docs = self.embed_stub(embeds, metadata, docs).await?;
        self.facturx_stub(facturx, docs).await
    }

    /// `/forms/libreoffice/convert`
    pub async fn libreoffice(
        &self,
        inputs: Vec<Document>,
        base: &ConvertOptions,
        native_formats: bool,
        merge: bool,
        split: Option<&Split>,
        steps: &PostSteps,
    ) -> Result<Vec<Document>> {
        let formats = facturx_formats(steps.facturx.is_some(), &steps.formats, true, &[]);
        let post_processing = steps.has_post_processing();
        // PDF/A applied natively survives only if nothing edits the file later.
        let native = native_formats && split.is_none() && !post_processing;
        let mut options = base.clone();
        if native {
            options.pdfua = formats.pdfua;
            options.pdfa = formats.level().map_err(|_| {
                bad(format!(
                    "The PDF format '{}' is not supported. Valid formats include PDF/A-1b, PDF/A-2b, PDF/A-3b, PDF/A-4, and PDF/UA.",
                    formats.pdfa
                ))
            })?;
        }
        let office = self
            .engines
            .libreoffice
            .as_ref()
            .ok_or_else(|| Error::EngineUnavailable("LibreOffice is not configured".into()))?;
        let mut docs = Vec::with_capacity(inputs.len());
        for input in &inputs {
            precheck(input, &options)?;
            let converted = office
                .convert_with(input, Format::Pdf, &options, self.timeout)
                .await
                .map_err(|e| explain_failure(e, input, &options))?;
            docs.push(Document::new(
                format!("{}.pdf", input.name),
                converted.bytes,
            ));
        }
        if merge {
            let merged = self.merge_stub(&docs).await?;
            let name = if docs.len() == 1 {
                docs[0].name.clone()
            } else {
                "merged.pdf".to_string()
            };
            docs = vec![Document::new(name, merged.bytes)];
        }
        if let Some(split) = split {
            docs = self.split_stub(split, &docs).await?;
        }
        docs = self.before_convert(steps, docs).await?;
        if !native {
            docs = self.convert_stub(&formats, docs).await?;
        }
        self.after_convert(steps, docs).await
    }
}

/// Complains about what the bytes already tell, so a bad password never reaches
/// a LibreOffice worker.
fn precheck(input: &Document, options: &ConvertOptions) -> Result<()> {
    if let Some(ranges) = options.native_page_ranges.as_deref().filter(|r| {
        !r.chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '-' | ',' | ' '))
    }) {
        return Err(page_ranges_error(ranges, &input.name));
    }
    let protection = protection::detect(input);
    let has_password = options.password.as_deref().is_some_and(|p| !p.is_empty());
    match protection {
        Protection::Required if !has_password => Err(bad(format!(
            "The document '{}' is password-protected. Provide its password in the 'password' form field.",
            input.name
        ))),
        Protection::None if has_password => Err(bad(format!(
            "The document '{}' is not password-protected. Remove the 'password' form field.",
            input.name
        ))),
        _ => Ok(()),
    }
}

/// LibreOffice's errors name the exception it caught rather than a cause, so
/// the client is only blamed when one of its inputs is actually implicated.
fn explain_failure(error: Error, input: &Document, options: &ConvertOptions) -> Error {
    if !matches!(error, Error::Engine { .. }) {
        return error;
    }
    tracing::warn!(error = %error, "LibreOffice could not convert a document");
    let name = &input.name;
    if options.password.as_deref().is_some_and(|p| !p.is_empty()) {
        return if protection::detect(input) == Protection::Required {
            bad(format!(
                "The password for the document '{name}' is incorrect. Check the 'password' form field."
            ))
        } else {
            bad(format!(
                "LibreOffice could not open the document '{name}' with the given password. Check the 'password' form field, and omit it if the document is not password-protected."
            ))
        };
    }
    if let Some(ranges) = options.native_page_ranges.as_deref() {
        return page_ranges_error(ranges, name);
    }
    Error::Status(
        500,
        format!(
            "LibreOffice failed to convert the document '{name}'. This is usually a resource issue: increase the container's memory and CPU, or reduce the document's size. The request is valid and may be retried."
        ),
    )
}

/// The text for page ranges LibreOffice cannot apply.
pub fn page_ranges_error(ranges: &str, name: &str) -> Error {
    bad(format!(
        "LibreOffice could not apply the page ranges '{ranges}' to the document '{name}'. Check the 'nativePageRanges' form field; valid values look like '1-4', '2' or '1,3,5-7'."
    ))
}

fn shift_pages(bookmarks: &mut [Bookmark], offset: u32) {
    for bookmark in bookmarks {
        bookmark.page += offset;
        shift_pages(&mut bookmark.children, offset);
    }
}

/// A file name without its last extension.
pub fn stem_of(name: &str) -> String {
    match name.rfind('.') {
        Some(at) if at > 0 => name[..at].to_string(),
        _ => name.to_string(),
    }
}

/// The engines' split mode for one document. Page selections are resolved here:
/// Gotenberg's clients send pdfcpu's and qpdf's forms (`2-`, `2-z`, `r1`), and
/// outside `unify` every selected page is a part of its own.
fn split_mode(split: &Split, doc: &Document) -> Result<SplitMode> {
    match split.mode.as_str() {
        "intervals" => Ok(SplitMode::Intervals {
            span: split.span.parse().map_err(|_| bad(SPLIT_NOT_SUPPORTED))?,
        }),
        "pages" => {
            let count = pdfops::page_count(doc)?;
            let ranges = selection(&split.span, count).ok_or_else(|| bad(SPLIT_NOT_SUPPORTED))?;
            let parsed = pages::parse(&ranges, count).map_err(|_| bad(SPLIT_NOT_SUPPORTED))?;
            if split.unify {
                Ok(SplitMode::Pages {
                    ranges: pages::join(&parsed),
                    unify: true,
                })
            } else {
                // Counted before anything is expanded. As Gotenberg's engines
                // do, a page selected twice is one part.
                if pages::distinct(&parsed) > MAX_SPLIT_PARTS as u64 {
                    return Err(bad(format!(
                        "split would produce more than {MAX_SPLIT_PARTS} parts"
                    )));
                }
                let mut seen = std::collections::BTreeSet::new();
                let each: Vec<String> = parsed
                    .iter()
                    .flat_map(pages::PageRange::pages)
                    .filter(|p| seen.insert(*p))
                    .map(|p| p.to_string())
                    .collect();
                Ok(SplitMode::Pages {
                    ranges: each.join(","),
                    unify: false,
                })
            }
        }
        _ => Err(bad(SPLIT_NOT_SUPPORTED)),
    }
}

/// Rewrites a selection in pdfcpu's or qpdf's syntax as plain ranges, or `None`
/// if it is neither.
pub fn selection(span: &str, count: u32) -> Option<String> {
    let page = |item: &str| -> Option<u32> {
        match item {
            "z" => Some(count),
            _ => match item.strip_prefix('r') {
                Some(back) => {
                    let back: u32 = back.parse().ok()?;
                    (back >= 1 && back <= count).then(|| count + 1 - back)
                }
                None => item.parse().ok(),
            },
        }
    };
    let mut out = Vec::new();
    for item in span.split(',') {
        let item = item.trim();
        if item.is_empty() || out.len() >= pages::MAX_RANGES {
            return None;
        }
        out.push(match item.split_once('-') {
            None => page(item)?.to_string(),
            Some(("", last)) => format!("1-{}", page(last)?),
            Some((first, "")) => format!("{}-{count}", page(first)?),
            Some((first, last)) => format!("{}-{}", page(first)?, page(last)?),
        });
    }
    Some(out.join(","))
}

/// Gotenberg's order of input files: by the number a name starts or ends with
/// when it has one, else by name, so `page_2` comes before `page_10`.
pub fn alphanumeric_less(a: &str, b: &str) -> bool {
    let (num_a, rest_a, prefix_a) = number_in(a);
    let (num_b, rest_b, prefix_b) = number_in(b);
    match (num_a, num_b) {
        (Some(x), Some(y)) => {
            if prefix_a && prefix_b {
                return if x != y { x < y } else { rest_a < rest_b };
            }
            if rest_a != rest_b {
                return rest_a < rest_b;
            }
            if !prefix_a && !prefix_b {
                return if x != y { x < y } else { a < b };
            }
            prefix_a
        }
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => a < b,
    }
}

/// The number in a name, the name without it, and whether it was a prefix.
fn number_in(name: &str) -> (Option<u64>, String, bool) {
    let digits = name.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0
        && let Ok(n) = name[..digits].parse()
    {
        return (Some(n), name[digits..].to_string(), true);
    }
    let (stem, ext) = match name.rfind('.') {
        Some(at) if at > 0 && at + 1 < name.len() => name.split_at(at),
        _ => (name, ""),
    };
    let tail = stem.bytes().rev().take_while(u8::is_ascii_digit).count();
    if tail > 0 {
        let (rest, number) = stem.split_at(stem.len() - tail);
        if let Ok(n) = number.parse() {
            return (Some(n), format!("{rest}{ext}"), false);
        }
    }
    (None, name.to_string(), false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdfops::fixtures;
    use std::sync::Arc;

    fn compat(engines: &Engines) -> Compat<'_> {
        Compat {
            engines,
            timeout: Duration::from_secs(30),
        }
    }

    fn engines() -> Engines {
        Engines {
            libreoffice: None,
            pdf: pdfops::MultiEngine::standard(None, None),
            forms: pdfops::MultiEngine::for_forms(None, None, &BTreeMap::new()),
        }
    }

    #[test]
    fn selections_in_either_syntax() {
        assert_eq!(selection("2-", 3).unwrap(), "2-3");
        assert_eq!(selection("2-z", 3).unwrap(), "2-3");
        assert_eq!(selection("-2", 3).unwrap(), "1-2");
        assert_eq!(selection("r1", 3).unwrap(), "3");
        assert_eq!(selection("1,3", 3).unwrap(), "1,3");
        assert_eq!(selection("foo", 3), None);
        assert_eq!(selection("", 3), None);
    }

    #[test]
    fn inputs_sort_by_their_numbers() {
        assert!(alphanumeric_less("page_2.pdf", "page_10.pdf"));
        assert!(alphanumeric_less("1_a.pdf", "2_a.pdf"));
        assert!(alphanumeric_less("a.pdf", "b.pdf"));
        assert!(alphanumeric_less("3.pdf", "a.pdf"));
        assert!(!alphanumeric_less("page_10.pdf", "page_2.pdf"));
        let mut names = vec!["pages_12.docx", "pages_3.docx"];
        names.sort_by(|a, b| {
            if alphanumeric_less(a, b) {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            }
        });
        assert_eq!(names, ["pages_3.docx", "pages_12.docx"]);
    }

    #[test]
    fn stems() {
        assert_eq!(stem_of("pages_3.docx.pdf"), "pages_3.docx");
        assert_eq!(stem_of("plain"), "plain");
    }

    #[tokio::test]
    async fn split_names_are_zero_based() {
        let engines = engines();
        let doc = Document::new("pages_3.pdf", fixtures::pages(3));
        let parts = compat(&engines)
            .split_stub(
                &Split {
                    mode: "pages".into(),
                    span: "2-".into(),
                    unify: false,
                },
                std::slice::from_ref(&doc),
            )
            .await
            .unwrap();
        let names: Vec<&str> = parts.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["pages_3_0.pdf", "pages_3_1.pdf"]);
        let unified = compat(&engines)
            .split_stub(
                &Split {
                    mode: "pages".into(),
                    span: "2-".into(),
                    unify: true,
                },
                &[doc],
            )
            .await
            .unwrap();
        assert_eq!(unified[0].name, "pages_3.pdf");
        assert_eq!(pdfops::page_count(&unified[0]).unwrap(), 2);
    }

    #[tokio::test]
    async fn bad_selections_get_the_split_text() {
        let engines = engines();
        let doc = Document::new("a.pdf", fixtures::pages(3));
        let err = compat(&engines)
            .split_stub(
                &Split {
                    mode: "pages".into(),
                    span: "foo".into(),
                    unify: false,
                },
                &[doc],
            )
            .await
            .unwrap_err();
        assert!(matches!(err, Error::InvalidRequest(m) if m == SPLIT_NOT_SUPPORTED));
    }

    #[tokio::test]
    async fn merged_outlines_shift_and_nest() {
        let engines = engines();
        let inputs = [
            Document::new("a.pdf", fixtures::pages(2)),
            Document::new("b.pdf", fixtures::pages(1)),
        ];
        let map = BTreeMap::from([(
            "b.pdf".to_string(),
            vec![Bookmark {
                title: "B".into(),
                page: 1,
                children: Vec::new(),
            }],
        )]);
        let outline = compat(&engines)
            .merged_outline(&inputs, &Bookmarks::Map(map), false, false)
            .await
            .unwrap();
        assert_eq!(outline[0].page, 3);
        let titled = compat(&engines)
            .merged_outline(&inputs, &Bookmarks::None, false, true)
            .await
            .unwrap();
        assert_eq!(titled.len(), 2);
        assert_eq!(titled[1].page, 3);
        assert_eq!(titled[0].title, "a");
    }

    #[test]
    fn formats_for_facturx() {
        let plain = PdfFormats::default();
        assert_eq!(
            facturx_formats(true, &plain, true, &[]).pdfa,
            "PDF/A-3b",
            "an office document becomes PDF/A-3"
        );
        assert_eq!(facturx_formats(false, &plain, true, &[]), plain);
        let doc = Document::new("a.pdf", fixtures::pages(1));
        assert_eq!(
            facturx_formats(true, &plain, false, &[doc]).pdfa,
            "PDF/A-3b"
        );
    }

    #[test]
    fn combinations_are_checked() {
        let mut steps = PostSteps {
            formats: PdfFormats {
                pdfa: "PDF/A-1b".into(),
                pdfua: false,
            },
            ..PostSteps::default()
        };
        steps.encrypt.user_password = "x".into();
        assert!(validate_steps(&steps).is_err());
        steps.encrypt.user_password.clear();
        steps.embeds.push(Attachment {
            name: "a.xml".into(),
            bytes: Vec::new(),
            mime: "text/xml".into(),
        });
        assert!(validate_steps(&steps).is_err());
        steps.formats.pdfa = "PDF/A-3b".into();
        assert!(validate_steps(&steps).is_ok());
        let mut restricted = PostSteps::default();
        restricted.encrypt.permissions.printing = false;
        assert!(validate_steps(&restricted).is_err());
        let _ = Arc::new(0);
    }

    #[test]
    fn protection_is_checked_first() {
        let protected = Document::new(
            "a.docx",
            vec![0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1],
        );
        let options = ConvertOptions::default();
        assert!(
            matches!(precheck(&protected, &options), Err(Error::InvalidRequest(m)) if m.contains("password-protected"))
        );
        let with_password = ConvertOptions {
            password: Some("x".into()),
            ..ConvertOptions::default()
        };
        assert!(precheck(&protected, &with_password).is_ok());
    }
}
