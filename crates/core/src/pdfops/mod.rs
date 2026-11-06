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
use crate::limits::MAX_OVERLAY_TEXT;

mod draw;
pub mod exif;
pub mod fixtures;
pub mod images;
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
    /// Exact placement, overriding `position` and `scale`: how a signature
    /// lands on the line drawn for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<Placement>,
}

impl Overlay {
    /// Refuses what no engine should be given.
    pub fn validate(&self) -> Result<()> {
        if let OverlayKind::Text { text, .. } = &self.kind
            && text.len() > MAX_OVERLAY_TEXT
        {
            return Err(Error::InvalidRequest(format!(
                "stamp or watermark text is longer than {MAX_OVERLAY_TEXT} bytes"
            )));
        }
        Ok(())
    }
}

/// The lower-left corner of a mark in points from the page's lower-left
/// corner, and optionally its width.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub x: f32,
    pub y: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f32>,
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
#[derive(Debug, Clone, PartialEq)]
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

/// What a password-protected document still allows without its owner
/// password. Everything is allowed by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    pub printing: bool,
    pub copying: bool,
    pub modifying: bool,
    pub annotating: bool,
    pub filling_forms: bool,
    pub assembling: bool,
}

impl Default for Permissions {
    fn default() -> Self {
        Self {
            printing: true,
            copying: true,
            modifying: true,
            annotating: true,
            filling_forms: true,
            assembling: true,
        }
    }
}

impl Permissions {
    pub fn restricted(&self) -> bool {
        *self != Self::default()
    }
}

/// Passwords and permissions for [`PdfEngine::encrypt_with`]. An empty user
/// password with an owner password gives a document anyone can open but only
/// the owner can change.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct EncryptOptions {
    pub user_password: String,
    pub owner_password: String,
    pub permissions: Permissions,
}

impl std::fmt::Debug for EncryptOptions {
    /// The passwords are never printed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncryptOptions")
            .field("permissions", &self.permissions)
            .finish_non_exhaustive()
    }
}

/// The Factur-X or ZUGFeRD properties of a document, under the names and
/// values the standard's XMP schema uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacturxProperties {
    /// `MINIMUM`, `BASIC WL`, `BASIC`, `EN 16931`, `EXTENDED` or `XRECHNUNG`.
    pub conformance_level: String,
    /// `INVOICE`, `ORDER`, `ORDER_RESPONSE` or `ORDER_CHANGE`.
    pub document_type: String,
    /// `factur-x.xml`, or `xrechnung.xml` for XRECHNUNG.
    pub file_name: String,
    pub version: String,
}

/// What `embedsMetadata` may set on an embedded file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EmbedMetadata {
    pub mime_type: Option<String>,
    pub relationship: Option<String>,
}

/// An operation whose engine order can be configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Op {
    Merge,
    Split,
    Flatten,
    Rotate,
    Optimize,
    OptimizeImages,
    Encrypt,
    ReadMetadata,
    WriteMetadata,
    ReadBookmarks,
    WriteBookmarks,
    Embed,
    EmbedMetadata,
    Watermark,
    Stamp,
    Facturx,
}

impl Op {
    /// The operations a `PDFENGINES_*_ENGINES` variable decides.
    pub fn for_variable(variable: &str) -> &'static [Op] {
        match variable {
            "PDFENGINES_MERGE_ENGINES" => &[Op::Merge],
            "PDFENGINES_SPLIT_ENGINES" => &[Op::Split],
            "PDFENGINES_FLATTEN_ENGINES" => &[Op::Flatten],
            "PDFENGINES_OPTIMIZE_IMAGES_ENGINES" => &[Op::OptimizeImages],
            "PDFENGINES_READ_METADATA_ENGINES" => &[Op::ReadMetadata],
            "PDFENGINES_WRITE_METADATA_ENGINES" => &[Op::WriteMetadata],
            "PDFENGINES_ENCRYPT_ENGINES" => &[Op::Encrypt],
            "PDFENGINES_EMBED_ENGINES" => &[Op::Embed],
            "PDFENGINES_EMBED_METADATA_ENGINES" => &[Op::EmbedMetadata],
            "PDFENGINES_READ_BOOKMARKS_ENGINES" => &[Op::ReadBookmarks],
            "PDFENGINES_WRITE_BOOKMARKS_ENGINES" => &[Op::WriteBookmarks],
            "PDFENGINES_WATERMARK_ENGINES" => &[Op::Watermark],
            "PDFENGINES_STAMP_ENGINES" => &[Op::Stamp],
            "PDFENGINES_ROTATE_ENGINES" => &[Op::Rotate],
            "PDFENGINES_FACTUR_X_ENGINES" => &[Op::Facturx],
            _ => &[],
        }
    }
}

/// One PDF engine. Every method defaults to unsupported.
#[async_trait]
pub trait PdfEngine: Send + Sync {
    fn name(&self) -> &'static str;

    /// Encrypts with restrictions. Engines that cannot honour the
    /// permissions they are given say so instead of ignoring them.
    async fn encrypt_with(
        &self,
        doc: &Document,
        options: &EncryptOptions,
        timeout: Duration,
    ) -> Result<Document> {
        let _ = (doc, options, timeout);
        Err(unsupported(self.name(), "encrypt_with"))
    }
    /// Re-encodes large deflated images as JPEG at `quality` (1 to 100),
    /// leaving text, vectors and masked images alone.
    async fn optimize_images(
        &self,
        doc: &Document,
        quality: u8,
        timeout: Duration,
    ) -> Result<Document> {
        let _ = (doc, quality, timeout);
        Err(unsupported(self.name(), "optimize_images"))
    }
    /// Metadata under ExifTool's key names and value shapes.
    async fn read_exif(&self, doc: &Document, timeout: Duration) -> Result<Metadata> {
        let _ = (doc, timeout);
        Err(unsupported(self.name(), "read_exif"))
    }
    /// Writes metadata given under ExifTool's key names, with Info and XMP
    /// kept in step. Refuses keys and values ExifTool would be unsafe with.
    async fn write_exif(
        &self,
        doc: &Document,
        metadata: &Metadata,
        timeout: Duration,
    ) -> Result<Document> {
        let _ = (doc, metadata, timeout);
        Err(unsupported(self.name(), "write_exif"))
    }
    /// Sets the relationship and media type of files already embedded,
    /// keyed by file name, and lists them in the catalog's `/AF`.
    async fn set_embed_metadata(
        &self,
        doc: &Document,
        metadata: &BTreeMap<String, EmbedMetadata>,
        timeout: Duration,
    ) -> Result<Document> {
        let _ = (doc, metadata, timeout);
        Err(unsupported(self.name(), "set_embed_metadata"))
    }
    /// Embeds the invoice XML under the name in `properties`, as the
    /// `Alternative` of the document, and declares it in the XMP.
    async fn facturx_with(
        &self,
        doc: &Document,
        xml: &[u8],
        properties: &FacturxProperties,
        timeout: Duration,
    ) -> Result<Document> {
        let _ = (doc, xml, properties, timeout);
        Err(unsupported(self.name(), "facturx_with"))
    }

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

/// Tries each engine in order until one supports the operation. An operation
/// can have its own order, and with [`MultiEngine::falling_through`] an engine
/// that fails hands over to the next one as well.
#[derive(Clone, Default)]
pub struct MultiEngine {
    engines: Vec<Arc<dyn PdfEngine>>,
    orders: BTreeMap<Op, Vec<Arc<dyn PdfEngine>>>,
    fall_through: bool,
}

impl std::fmt::Debug for MultiEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.engines.iter().map(|e| e.name()))
            .finish()
    }
}

macro_rules! fallback {
    ($self:ident, $key:expr, $op:ident, $($arg:expr),*) => {{
        let mut last = Error::EngineUnavailable(concat!("no PDF engine can ", stringify!($op)).into());
        let mut failed = false;
        let engines = $self.engines_for($key);
        for engine in engines {
            match engine.$op($($arg),*).await {
                Err(Error::Unsupported(_)) => continue,
                Err(e @ Error::Timeout(_)) => return Err(e),
                Err(e) if !$self.fall_through => return Err(e),
                Err(e) => {
                    // The first failure says the most: later engines were
                    // only tried because of it.
                    if !failed {
                        last = e;
                        failed = true;
                    }
                }
                Ok(v) => return Ok(v),
            }
        }
        if engines.is_empty() {
            last = Error::EngineUnavailable("no PDF engine is configured".into());
        }
        Err(last)
    }};
}

impl MultiEngine {
    pub fn new(engines: Vec<Arc<dyn PdfEngine>>) -> Self {
        Self {
            engines,
            orders: BTreeMap::new(),
            fall_through: false,
        }
    }

    /// The engines to try for `op`, in order.
    fn engines_for(&self, op: Op) -> &[Arc<dyn PdfEngine>] {
        self.orders.get(&op).unwrap_or(&self.engines)
    }

    /// Gives `op` its own order.
    pub fn with_order(mut self, op: Op, engines: Vec<Arc<dyn PdfEngine>>) -> Self {
        self.orders.insert(op, engines);
        self
    }

    /// Moves on to the next engine after any failure but a timeout, not only
    /// after [`Error::Unsupported`].
    pub fn falling_through(mut self) -> Self {
        self.fall_through = true;
        self
    }

    /// The order Gotenberg's routes use: qpdf for merging, flattening and
    /// encryption, the in-process engine wherever it does the job, and the
    /// process engines behind it. `lists` holds the operations configured by
    /// name; a name without a binary is dropped.
    pub fn for_forms(
        qpdf_bin: Option<PathBuf>,
        pdfcpu_bin: Option<PathBuf>,
        lists: &BTreeMap<String, Vec<String>>,
    ) -> Self {
        let lopdf: Arc<dyn PdfEngine> = Arc::new(LopdfEngine::default());
        let qpdf: Option<Arc<dyn PdfEngine>> =
            qpdf_bin.map(|bin| Arc::new(QpdfEngine::new(bin)) as Arc<dyn PdfEngine>);
        let pdfcpu: Option<Arc<dyn PdfEngine>> =
            pdfcpu_bin.map(|bin| Arc::new(PdfcpuEngine::new(bin)) as Arc<dyn PdfEngine>);
        let named = |name: &str| -> Option<Arc<dyn PdfEngine>> {
            match name {
                "lopdf" | "qpdf-metadata" => Some(lopdf.clone()),
                "qpdf" => qpdf.clone(),
                "pdfcpu" => pdfcpu.clone(),
                _ => None,
            }
        };
        let order = |names: &[&str]| -> Vec<Arc<dyn PdfEngine>> {
            names.iter().filter_map(|n| named(n)).collect()
        };
        let mut multi = Self::new(order(&["lopdf", "qpdf", "pdfcpu"]))
            .falling_through()
            .with_order(Op::Merge, order(&["qpdf", "pdfcpu", "lopdf"]))
            .with_order(Op::Flatten, order(&["qpdf", "lopdf"]))
            .with_order(Op::Encrypt, order(&["qpdf", "pdfcpu"]));
        for (variable, names) in lists {
            let engines: Vec<Arc<dyn PdfEngine>> = names.iter().filter_map(|n| named(n)).collect();
            for op in Op::for_variable(variable) {
                multi = multi.with_order(*op, engines.clone());
            }
        }
        multi
    }

    /// Applies the configured engine lists on top of this order.
    pub fn with_lists(
        mut self,
        qpdf_bin: Option<PathBuf>,
        pdfcpu_bin: Option<PathBuf>,
        lists: &BTreeMap<String, Vec<String>>,
    ) -> Self {
        let forms = Self::for_forms(qpdf_bin, pdfcpu_bin, lists);
        for variable in lists.keys() {
            for op in Op::for_variable(variable) {
                if let Some(engines) = forms.orders.get(op) {
                    self.orders.insert(*op, engines.clone());
                }
            }
        }
        self
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
        fallback!(self, Op::Merge, merge, docs, t)
    }
    pub async fn split(
        &self,
        doc: &Document,
        mode: &SplitMode,
        t: Duration,
    ) -> Result<Vec<Document>> {
        fallback!(self, Op::Split, split, doc, mode, t)
    }
    pub async fn flatten(&self, doc: &Document, t: Duration) -> Result<Document> {
        fallback!(self, Op::Flatten, flatten, doc, t)
    }
    pub async fn rotate(
        &self,
        doc: &Document,
        r: Rotation,
        pages: &str,
        t: Duration,
    ) -> Result<Document> {
        fallback!(self, Op::Rotate, rotate, doc, r, pages, t)
    }
    pub async fn optimize(&self, doc: &Document, t: Duration) -> Result<Document> {
        fallback!(self, Op::Optimize, optimize, doc, t)
    }
    pub async fn encrypt(&self, doc: &Document, u: &str, o: &str, t: Duration) -> Result<Document> {
        fallback!(self, Op::Encrypt, encrypt, doc, u, o, t)
    }
    pub async fn read_metadata(&self, doc: &Document, t: Duration) -> Result<Metadata> {
        fallback!(self, Op::ReadMetadata, read_metadata, doc, t)
    }
    pub async fn write_metadata(
        &self,
        doc: &Document,
        m: &Metadata,
        t: Duration,
    ) -> Result<Document> {
        fallback!(self, Op::WriteMetadata, write_metadata, doc, m, t)
    }
    pub async fn read_bookmarks(&self, doc: &Document, t: Duration) -> Result<Vec<Bookmark>> {
        fallback!(self, Op::ReadBookmarks, read_bookmarks, doc, t)
    }
    pub async fn write_bookmarks(
        &self,
        doc: &Document,
        b: &[Bookmark],
        t: Duration,
    ) -> Result<Document> {
        fallback!(self, Op::WriteBookmarks, write_bookmarks, doc, b, t)
    }
    pub async fn embed(
        &self,
        doc: &Document,
        files: &[Attachment],
        t: Duration,
    ) -> Result<Document> {
        fallback!(self, Op::Embed, embed, doc, files, t)
    }
    pub async fn watermark(&self, doc: &Document, o: &Overlay, t: Duration) -> Result<Document> {
        o.validate()?;
        fallback!(self, Op::Watermark, watermark, doc, o, t)
    }
    pub async fn stamp(&self, doc: &Document, o: &Overlay, t: Duration) -> Result<Document> {
        o.validate()?;
        fallback!(self, Op::Stamp, stamp, doc, o, t)
    }
    pub async fn facturx(
        &self,
        doc: &Document,
        xml: &[u8],
        p: FacturxProfile,
        t: Duration,
    ) -> Result<Document> {
        fallback!(self, Op::Facturx, facturx, doc, xml, p, t)
    }
    pub async fn encrypt_with(
        &self,
        doc: &Document,
        o: &EncryptOptions,
        t: Duration,
    ) -> Result<Document> {
        fallback!(self, Op::Encrypt, encrypt_with, doc, o, t)
    }
    pub async fn optimize_images(&self, doc: &Document, q: u8, t: Duration) -> Result<Document> {
        fallback!(self, Op::OptimizeImages, optimize_images, doc, q, t)
    }
    pub async fn read_exif(&self, doc: &Document, t: Duration) -> Result<Metadata> {
        fallback!(self, Op::ReadMetadata, read_exif, doc, t)
    }
    pub async fn write_exif(&self, doc: &Document, m: &Metadata, t: Duration) -> Result<Document> {
        fallback!(self, Op::WriteMetadata, write_exif, doc, m, t)
    }
    pub async fn set_embed_metadata(
        &self,
        doc: &Document,
        m: &BTreeMap<String, EmbedMetadata>,
        t: Duration,
    ) -> Result<Document> {
        fallback!(self, Op::EmbedMetadata, set_embed_metadata, doc, m, t)
    }
    pub async fn facturx_with(
        &self,
        doc: &Document,
        xml: &[u8],
        p: &FacturxProperties,
        t: Duration,
    ) -> Result<Document> {
        fallback!(self, Op::Facturx, facturx_with, doc, xml, p, t)
    }
}

/// The PDF/A part a document's XMP declares, such as `3`.
pub fn pdfa_part(doc: &Document) -> Option<String> {
    let pdf = lopdf::load(doc).ok()?;
    let packet = lopdf::xmp_packet(&pdf)?;
    xmp::get(&packet, "pdfaid:part")
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
