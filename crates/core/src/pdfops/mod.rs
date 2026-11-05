//! PDF operations behind one interface, with engines tried in order.
//!
//! An engine implements the operations it can; the rest return
//! [`Error::Unsupported`], and [`MultiEngine`] moves on to the next engine.
//! This is how Gotenberg's `PDFENGINES_*_ENGINES` lists work, with the
//! order fixed in code rather than configuration.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::job::Document;

mod draw;
pub mod fixtures;
pub mod lopdf;
pub mod pages;
pub mod pdfcpu;
pub mod qpdf;
#[cfg(test)]
mod tests;
pub mod xmp;

pub use lopdf::LopdfEngine;
pub use pdfcpu::PdfcpuEngine;
pub use qpdf::QpdfEngine;

/// When blocking work must stop. `tokio::time::timeout` cannot interrupt
/// CPU work, so every loop over pages, outline items, name-tree nodes or
/// split parts checks the deadline and gives up with [`Error::Timeout`].
#[derive(Debug, Clone, Copy)]
pub struct Deadline {
    at: Instant,
    timeout: Duration,
}

impl Deadline {
    pub fn new(timeout: Duration) -> Self {
        let at = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(|| Instant::now() + Duration::from_secs(86_400));
        Self { at, timeout }
    }

    /// `Ok` while there is time left.
    pub fn check(&self) -> Result<()> {
        if Instant::now() >= self.at {
            Err(Error::Timeout(self.timeout))
        } else {
            Ok(())
        }
    }
}

/// The number of pages, read with lopdf. The process engines use it to
/// validate page selections before spawning anything.
pub fn page_count(doc: &Document) -> Result<u32> {
    Ok(lopdf::load(doc)?.get_pages().len() as u32)
}

/// Document metadata as the API exposes it: Info dictionary keys such as
/// `Title`, `Author`, `Subject`, `Keywords`, `Creator`, `Producer`,
/// `CreationDate`, `ModDate`, plus custom keys.
pub type Metadata = BTreeMap<String, serde_json::Value>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bookmark {
    pub title: String,
    /// 1-based page number.
    pub page: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Bookmark>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum SplitMode {
    /// Page ranges such as `1-3,5`, one output per range, or one output of
    /// all ranges when `unify` is set.
    Pages { ranges: String, unify: bool },
    /// Every `span` pages into a new file.
    Intervals { span: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rotation {
    #[serde(rename = "90")]
    R90,
    #[serde(rename = "180")]
    R180,
    #[serde(rename = "270")]
    R270,
}

impl Rotation {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s.trim() {
            "90" => Rotation::R90,
            "180" => Rotation::R180,
            "270" => Rotation::R270,
            "-90" => Rotation::R270,
            other => {
                return Err(Error::InvalidRequest(format!(
                    "rotation must be 90, 180 or 270, not '{other}'"
                )));
            }
        })
    }

    pub fn degrees(self) -> i64 {
        match self {
            Rotation::R90 => 90,
            Rotation::R180 => 180,
            Rotation::R270 => 270,
        }
    }
}

/// Where a stamp or watermark sits on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Position {
    TopLeft,
    TopCenter,
    TopRight,
    #[default]
    Center,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

/// Text or an image laid over (stamp) or under (watermark) the content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Overlay {
    #[serde(flatten)]
    pub kind: OverlayKind,
    #[serde(default)]
    pub position: Position,
    /// 0.0 to 1.0.
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    /// Degrees, counter-clockwise.
    #[serde(default)]
    pub rotation: f32,
    /// Relative to the page width, 0.0 to 1.0.
    #[serde(default = "default_scale")]
    pub scale: f32,
    /// Page selection such as `1-3,5`; all pages when empty.
    #[serde(default)]
    pub pages: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum OverlayKind {
    Text {
        text: String,
        #[serde(default = "default_font_size")]
        font_size: f32,
        /// Hex such as `#ff0000`.
        #[serde(default = "default_colour")]
        colour: String,
    },
    Image {
        /// PNG or JPEG bytes.
        #[serde(skip)]
        bytes: Vec<u8>,
        name: String,
    },
    /// Every page of this PDF over the matching page of the target: how a
    /// rendered template lands on a base PDF.
    Pdf {
        #[serde(skip)]
        bytes: Vec<u8>,
        name: String,
    },
}

fn default_opacity() -> f32 {
    1.0
}
fn default_scale() -> f32 {
    0.5
}
fn default_font_size() -> f32 {
    24.0
}
fn default_colour() -> String {
    "#808080".into()
}

/// A file to embed as an attachment.
#[derive(Debug, Clone)]
pub struct Attachment {
    pub name: String,
    pub bytes: Vec<u8>,
    pub mime: String,
}

/// Factur-X profiles, from the least to the most detailed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum FacturxProfile {
    Minimum,
    BasicWl,
    Basic,
    En16931,
    Extended,
}

impl FacturxProfile {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(
            match s.trim().to_ascii_uppercase().replace('-', "_").as_str() {
                "MINIMUM" => FacturxProfile::Minimum,
                "BASIC_WL" | "BASICWL" => FacturxProfile::BasicWl,
                "BASIC" => FacturxProfile::Basic,
                "EN16931" | "EN_16931" | "COMFORT" => FacturxProfile::En16931,
                "EXTENDED" => FacturxProfile::Extended,
                other => {
                    return Err(Error::InvalidRequest(format!(
                        "unknown Factur-X profile '{other}'"
                    )));
                }
            },
        )
    }
}

/// One PDF engine. Every method defaults to unsupported.
#[async_trait]
pub trait PdfEngine: Send + Sync {
    fn name(&self) -> &'static str;

    async fn merge(&self, docs: &[Document], timeout: Duration) -> Result<Document> {
        let _ = (docs, timeout);
        Err(unsupported(self.name(), "merge"))
    }
    async fn split(
        &self,
        doc: &Document,
        mode: &SplitMode,
        timeout: Duration,
    ) -> Result<Vec<Document>> {
        let _ = (doc, mode, timeout);
        Err(unsupported(self.name(), "split"))
    }
    async fn flatten(&self, doc: &Document, timeout: Duration) -> Result<Document> {
        let _ = (doc, timeout);
        Err(unsupported(self.name(), "flatten"))
    }
    async fn rotate(
        &self,
        doc: &Document,
        rotation: Rotation,
        pages: &str,
        timeout: Duration,
    ) -> Result<Document> {
        let _ = (doc, rotation, pages, timeout);
        Err(unsupported(self.name(), "rotate"))
    }
    async fn optimize(&self, doc: &Document, timeout: Duration) -> Result<Document> {
        let _ = (doc, timeout);
        Err(unsupported(self.name(), "optimize"))
    }
    async fn encrypt(
        &self,
        doc: &Document,
        user_password: &str,
        owner_password: &str,
        timeout: Duration,
    ) -> Result<Document> {
        let _ = (doc, user_password, owner_password, timeout);
        Err(unsupported(self.name(), "encrypt"))
    }
    async fn read_metadata(&self, doc: &Document, timeout: Duration) -> Result<Metadata> {
        let _ = (doc, timeout);
        Err(unsupported(self.name(), "read_metadata"))
    }
    async fn write_metadata(
        &self,
        doc: &Document,
        metadata: &Metadata,
        timeout: Duration,
    ) -> Result<Document> {
        let _ = (doc, metadata, timeout);
        Err(unsupported(self.name(), "write_metadata"))
    }
    async fn read_bookmarks(&self, doc: &Document, timeout: Duration) -> Result<Vec<Bookmark>> {
        let _ = (doc, timeout);
        Err(unsupported(self.name(), "read_bookmarks"))
    }
    async fn write_bookmarks(
        &self,
        doc: &Document,
        bookmarks: &[Bookmark],
        timeout: Duration,
    ) -> Result<Document> {
        let _ = (doc, bookmarks, timeout);
        Err(unsupported(self.name(), "write_bookmarks"))
    }
    async fn embed(
        &self,
        doc: &Document,
        files: &[Attachment],
        timeout: Duration,
    ) -> Result<Document> {
        let _ = (doc, files, timeout);
        Err(unsupported(self.name(), "embed"))
    }
    async fn watermark(
        &self,
        doc: &Document,
        overlay: &Overlay,
        timeout: Duration,
    ) -> Result<Document> {
        let _ = (doc, overlay, timeout);
        Err(unsupported(self.name(), "watermark"))
    }
    async fn stamp(
        &self,
        doc: &Document,
        overlay: &Overlay,
        timeout: Duration,
    ) -> Result<Document> {
        let _ = (doc, overlay, timeout);
        Err(unsupported(self.name(), "stamp"))
    }
    /// Embeds the Factur-X XML and writes the PDF/A-3 metadata for it.
    async fn facturx(
        &self,
        doc: &Document,
        xml: &[u8],
        profile: FacturxProfile,
        timeout: Duration,
    ) -> Result<Document> {
        let _ = (doc, xml, profile, timeout);
        Err(unsupported(self.name(), "facturx"))
    }
}

fn unsupported(engine: &str, op: &str) -> Error {
    Error::Unsupported(format!("{engine} cannot {op}"))
}

/// Tries each engine in order until one supports the operation.
#[derive(Clone, Default)]
pub struct MultiEngine {
    engines: Vec<Arc<dyn PdfEngine>>,
}

impl std::fmt::Debug for MultiEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.engines.iter().map(|e| e.name()))
            .finish()
    }
}

macro_rules! fallback {
    ($self:ident, $op:ident, $($arg:expr),*) => {{
        let mut last = Error::EngineUnavailable(concat!("no PDF engine can ", stringify!($op)).into());
        for engine in &$self.engines {
            match engine.$op($($arg),*).await {
                Err(Error::Unsupported(_)) => continue,
                Err(e) => return Err(e),
                Ok(v) => return Ok(v),
            }
        }
        if $self.engines.is_empty() {
            last = Error::EngineUnavailable("no PDF engine is configured".into());
        }
        Err(last)
    }};
}

impl MultiEngine {
    pub fn new(engines: Vec<Arc<dyn PdfEngine>>) -> Self {
        Self { engines }
    }

    /// The standard order: lopdf first, then qpdf and pdfcpu when their
    /// binaries are configured.
    pub fn standard(qpdf_bin: Option<PathBuf>, pdfcpu_bin: Option<PathBuf>) -> Self {
        let mut engines: Vec<Arc<dyn PdfEngine>> = vec![Arc::new(LopdfEngine::default())];
        if let Some(bin) = qpdf_bin {
            engines.push(Arc::new(QpdfEngine::new(bin)));
        }
        if let Some(bin) = pdfcpu_bin {
            engines.push(Arc::new(PdfcpuEngine::new(bin)));
        }
        Self::new(engines)
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.engines.iter().map(|e| e.name()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.engines.is_empty()
    }

    pub async fn merge(&self, docs: &[Document], t: Duration) -> Result<Document> {
        fallback!(self, merge, docs, t)
    }
    pub async fn split(
        &self,
        doc: &Document,
        mode: &SplitMode,
        t: Duration,
    ) -> Result<Vec<Document>> {
        fallback!(self, split, doc, mode, t)
    }
    pub async fn flatten(&self, doc: &Document, t: Duration) -> Result<Document> {
        fallback!(self, flatten, doc, t)
    }
    pub async fn rotate(
        &self,
        doc: &Document,
        r: Rotation,
        pages: &str,
        t: Duration,
    ) -> Result<Document> {
        fallback!(self, rotate, doc, r, pages, t)
    }
    pub async fn optimize(&self, doc: &Document, t: Duration) -> Result<Document> {
        fallback!(self, optimize, doc, t)
    }
    pub async fn encrypt(&self, doc: &Document, u: &str, o: &str, t: Duration) -> Result<Document> {
        fallback!(self, encrypt, doc, u, o, t)
    }
    pub async fn read_metadata(&self, doc: &Document, t: Duration) -> Result<Metadata> {
        fallback!(self, read_metadata, doc, t)
    }
    pub async fn write_metadata(
        &self,
        doc: &Document,
        m: &Metadata,
        t: Duration,
    ) -> Result<Document> {
        fallback!(self, write_metadata, doc, m, t)
    }
    pub async fn read_bookmarks(&self, doc: &Document, t: Duration) -> Result<Vec<Bookmark>> {
        fallback!(self, read_bookmarks, doc, t)
    }
    pub async fn write_bookmarks(
        &self,
        doc: &Document,
        b: &[Bookmark],
        t: Duration,
    ) -> Result<Document> {
        fallback!(self, write_bookmarks, doc, b, t)
    }
    pub async fn embed(
        &self,
        doc: &Document,
        files: &[Attachment],
        t: Duration,
    ) -> Result<Document> {
        fallback!(self, embed, doc, files, t)
    }
    pub async fn watermark(&self, doc: &Document, o: &Overlay, t: Duration) -> Result<Document> {
        fallback!(self, watermark, doc, o, t)
    }
    pub async fn stamp(&self, doc: &Document, o: &Overlay, t: Duration) -> Result<Document> {
        fallback!(self, stamp, doc, o, t)
    }
    pub async fn facturx(
        &self,
        doc: &Document,
        xml: &[u8],
        p: FacturxProfile,
        t: Duration,
    ) -> Result<Document> {
        fallback!(self, facturx, doc, xml, p, t)
    }
}

/// Refuses anything that is not a PDF before an engine sees it.
pub fn require_pdf(doc: &Document) -> Result<()> {
    if doc.extension() != "pdf" || !doc.bytes.starts_with(b"%PDF") {
        return Err(Error::InvalidRequest(format!(
            "'{}' is not a PDF",
            doc.name
        )));
    }
    Ok(())
}
