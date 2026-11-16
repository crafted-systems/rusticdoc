//! The pdfcpu engine, run as a process. It embeds fonts for stamps, so it
//! is the engine for non-Latin stamp and watermark text, and it backs
//! rotate, optimise, split, merge and bookmarks.
//!
//! pdfcpu 0.16 has a cobra-style command line: `pdfcpu stamp add --mode
//! text -p 1-3 -- "text" "description" in.pdf out.pdf`. The config
//! directory is disabled (`-c disable`) so nothing is written outside the
//! job's temp dir.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::qpdf::workdir;
use super::{
    Bookmark, EncryptOptions, Overlay, OverlayKind, PdfEngine, Position, Rotation, SplitMode,
    page_count_off_thread, pages, require_pdf,
};
use crate::error::{Error, Result};
use crate::job::Document;
use crate::limits::{MAX_OUTPUT_BYTES, charge_output};
use crate::process;

const ENGINE: &str = "pdfcpu";

/// pdfcpu as a child process.
#[derive(Debug, Clone)]
pub struct PdfcpuEngine {
    bin: PathBuf,
    /// Soft heap cap for the Go child, in bytes; `None` leaves it uncapped.
    memory: Option<u64>,
}

impl PdfcpuEngine {
    pub fn new(bin: PathBuf) -> Self {
        Self { bin, memory: None }
    }

    /// Caps the child's memory at `bytes`; 0 leaves it uncapped. pdfcpu is a
    /// Go binary, whose runtime reserves a large virtual address space at
    /// startup, so a hard `RLIMIT_AS` would abort it; its heap is bounded with
    /// `GOMEMLIMIT` instead. That is a soft limit the runtime keeps to by
    /// collecting more often, backed by the request timeout and the
    /// container's own memory limit.
    pub fn with_memory(mut self, bytes: u64) -> Self {
        self.memory = (bytes > 0).then_some(bytes);
        self
    }

    pub fn bin(&self) -> &Path {
        &self.bin
    }

    async fn run(&self, dir: &Path, args: &[String], timeout: Duration) -> Result<()> {
        let mut full: Vec<String> = vec!["-c".into(), "disable".into()];
        full.extend_from_slice(args);
        // GOMEMLIMIT takes a byte count; a bare number is unambiguous bytes.
        let env: Vec<(&str, String)> = match self.memory {
            Some(bytes) => vec![("GOMEMLIMIT", bytes.to_string())],
            None => Vec::new(),
        };
        process::run(ENGINE, &self.bin, &full, dir, &env, timeout).await
    }

    /// Writes the input, runs `args` with `in.pdf out.pdf` appended, and
    /// reads the output back.
    async fn transform(
        &self,
        doc: &Document,
        args: &[String],
        timeout: Duration,
    ) -> Result<Document> {
        require_pdf(doc)?;
        let dir = workdir()?;
        let input = dir.path().join("in.pdf");
        let out = dir.path().join("out.pdf");
        tokio::fs::write(&input, &doc.bytes).await?;
        let mut full = args.to_vec();
        full.push(input.display().to_string());
        full.push(out.display().to_string());
        self.run(dir.path(), &full, timeout).await?;
        Ok(Document::new(
            doc.name.clone(),
            tokio::fs::read(&out).await?,
        ))
    }

    /// `pdfcpu stamp|watermark add --mode M [-p pages] -- content desc in out`
    async fn overlay(
        &self,
        command: &str,
        doc: &Document,
        overlay: &Overlay,
        timeout: Duration,
    ) -> Result<Document> {
        require_pdf(doc)?;
        let dir = workdir()?;
        let (mode, content) = match &overlay.kind {
            OverlayKind::Text { text, .. } => ("text", text.clone()),
            OverlayKind::Image { bytes, name } => {
                let ext = Path::new(name)
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase())
                    .filter(|e| {
                        matches!(e.as_str(), "jpg" | "jpeg" | "png" | "tif" | "tiff" | "webp")
                    })
                    .unwrap_or_else(|| "png".into());
                let path = dir.path().join(format!("overlay.{ext}"));
                tokio::fs::write(&path, bytes).await?;
                ("image", path.display().to_string())
            }
            OverlayKind::Pdf { bytes, .. } => {
                let path = dir.path().join("overlay.pdf");
                tokio::fs::write(&path, bytes).await?;
                ("pdf", path.display().to_string())
            }
        };
        let mut args: Vec<String> =
            vec![command.into(), "add".into(), "--mode".into(), mode.into()];
        if !overlay.pages.trim().is_empty() {
            let ranges = pages::parse(&overlay.pages, page_count_off_thread(doc, timeout).await?)?;
            args.push("-p".into());
            args.push(pages::join(&ranges));
        }
        args.push("--".into());
        args.push(content);
        args.push(description(overlay));
        let input = dir.path().join("in.pdf");
        let out = dir.path().join("out.pdf");
        tokio::fs::write(&input, &doc.bytes).await?;
        args.push(input.display().to_string());
        args.push(out.display().to_string());
        self.run(dir.path(), &args, timeout).await?;
        Ok(Document::new(
            doc.name.clone(),
            tokio::fs::read(&out).await?,
        ))
    }
}

/// pdfcpu's description string: `pos:c, rot:45, scale:0.5 rel, op:0.3`,
/// plus the font settings for text.
pub(super) fn description(overlay: &Overlay) -> String {
    let pos = match overlay.position {
        Position::TopLeft => "tl",
        Position::TopCenter => "tc",
        Position::TopRight => "tr",
        Position::Center => "c",
        Position::BottomLeft => "bl",
        Position::BottomCenter => "bc",
        Position::BottomRight => "br",
    };
    let mut rot = overlay.rotation % 360.0;
    if rot > 180.0 {
        rot -= 360.0;
    } else if rot < -180.0 {
        rot += 360.0;
    }
    let scale = if overlay.scale > 0.0 {
        format!("{:.3} rel", overlay.scale.min(1.0))
    } else {
        "1 abs".to_string()
    };
    let mut desc = format!(
        "pos:{pos}, rot:{rot:.1}, scale:{scale}, op:{:.2}",
        overlay.opacity.clamp(0.0, 1.0)
    );
    if let OverlayKind::Text {
        font_size, colour, ..
    } = &overlay.kind
    {
        let colour = colour.trim();
        let colour = if colour.starts_with('#') {
            colour.to_string()
        } else {
            format!("#{colour}")
        };
        desc.push_str(&format!(
            ", font:Helvetica, points:{}, fillcolor:{colour}",
            font_size.max(1.0).round() as i64
        ));
    }
    desc
}

/// One bookmark as pdfcpu's JSON lists it.
#[derive(Debug, Default, Serialize, Deserialize)]
struct PdfcpuBookmark {
    title: String,
    page: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    kids: Vec<PdfcpuBookmark>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PdfcpuBookmarks {
    #[serde(default)]
    bookmarks: Vec<PdfcpuBookmark>,
}

fn to_pdfcpu(bookmarks: &[Bookmark]) -> Vec<PdfcpuBookmark> {
    bookmarks
        .iter()
        .map(|b| PdfcpuBookmark {
            title: b.title.clone(),
            page: b.page.max(1),
            kids: to_pdfcpu(&b.children),
        })
        .collect()
}

fn from_pdfcpu(bookmarks: Vec<PdfcpuBookmark>) -> Vec<Bookmark> {
    bookmarks
        .into_iter()
        .map(|b| Bookmark {
            title: b.title,
            page: b.page,
            children: from_pdfcpu(b.kids),
        })
        .collect()
}

#[async_trait]
impl PdfEngine for PdfcpuEngine {
    fn name(&self) -> &'static str {
        ENGINE
    }

    /// `pdfcpu encrypt --mode aes --upw U --opw O --perm all|none in out`.
    ///
    /// pdfcpu only takes passwords as arguments, so they are visible to this
    /// user's other processes while it runs. It only gets here when a deployment
    /// names it in `RUSTICDOC_PDF_ENGINES_ENCRYPT` or has no qpdf: encryption does not
    /// fall through to it after a qpdf failure. Arguments are not logged either
    /// way (see [`process::run`]).
    async fn encrypt_with(
        &self,
        doc: &Document,
        options: &EncryptOptions,
        timeout: Duration,
    ) -> Result<Document> {
        let owner = if options.owner_password.is_empty() {
            options.user_password.clone()
        } else {
            options.owner_password.clone()
        };
        if owner.is_empty() {
            return Err(Error::InvalidRequest(
                "encrypt needs an owner password".into(),
            ));
        }
        let perm = if options.permissions.restricted() {
            "none"
        } else {
            "all"
        };
        let args: Vec<String> = vec![
            "encrypt".into(),
            "--mode".into(),
            "aes".into(),
            "--upw".into(),
            options.user_password.clone(),
            "--opw".into(),
            owner,
            "--perm".into(),
            perm.into(),
        ];
        self.transform(doc, &args, timeout).await
    }

    /// `pdfcpu merge --bookmark-mode preserve out.pdf a.pdf b.pdf`
    async fn merge(&self, docs: &[Document], timeout: Duration) -> Result<Document> {
        if docs.is_empty() {
            return Err(Error::InvalidRequest("merge needs at least one PDF".into()));
        }
        let dir = workdir()?;
        let out = dir.path().join("merged.pdf");
        let mut args: Vec<String> = vec![
            "merge".into(),
            "--bookmark-mode".into(),
            "preserve".into(),
            out.display().to_string(),
        ];
        for (i, doc) in docs.iter().enumerate() {
            require_pdf(doc)?;
            let path = dir.path().join(format!("{i}.pdf"));
            tokio::fs::write(&path, &doc.bytes).await?;
            args.push(path.display().to_string());
        }
        self.run(dir.path(), &args, timeout).await?;
        Ok(Document::new(
            docs[0].with_extension("pdf"),
            tokio::fs::read(&out).await?,
        ))
    }

    /// `pdfcpu trim -p 1-3 in out` per range; `pdfcpu split -m span in dir N`
    /// for intervals.
    async fn split(
        &self,
        doc: &Document,
        mode: &SplitMode,
        timeout: Duration,
    ) -> Result<Vec<Document>> {
        require_pdf(doc)?;
        let count = page_count_off_thread(doc, timeout).await?;
        let stem = doc.with_extension("");
        let stem = stem.trim_end_matches('.').to_string();
        let mut outputs = Vec::new();
        let mut produced = 0;
        match mode {
            SplitMode::Pages { ranges, unify } => {
                let ranges = pages::parse(ranges, count)?;
                let specs: Vec<String> = if *unify {
                    vec![pages::join(&ranges)]
                } else {
                    ranges.iter().map(|r| r.label()).collect()
                };
                for spec in specs {
                    let part = self
                        .transform(doc, &["trim".into(), "-p".into(), spec.clone()], timeout)
                        .await?;
                    charge_output(&mut produced, part.bytes.len(), MAX_OUTPUT_BYTES)?;
                    outputs.push(Document::new(
                        format!("{stem}-{}.pdf", spec.replace(',', "_")),
                        part.bytes,
                    ));
                }
            }
            SplitMode::Intervals { span } => {
                let ranges = pages::intervals(*span, count)?;
                let dir = workdir()?;
                let input = dir.path().join("in.pdf");
                let outdir = dir.path().join("parts");
                tokio::fs::create_dir(&outdir).await?;
                tokio::fs::write(&input, &doc.bytes).await?;
                let args: Vec<String> = vec![
                    "split".into(),
                    "-m".into(),
                    "span".into(),
                    input.display().to_string(),
                    outdir.display().to_string(),
                    span.to_string(),
                ];
                self.run(dir.path(), &args, timeout).await?;
                // Parts are named `in_1-2.pdf`; order them by first page.
                let mut parts: Vec<(u32, PathBuf)> = std::fs::read_dir(&outdir)?
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter_map(|p| {
                        let name = p.file_stem()?.to_str()?.to_string();
                        let first = name.rsplit('_').next()?.split('-').next()?.parse().ok()?;
                        Some((first, p))
                    })
                    .collect();
                parts.sort();
                if parts.len() != ranges.len() {
                    return Err(Error::Engine {
                        engine: ENGINE,
                        message: format!("expected {} parts, found {}", ranges.len(), parts.len()),
                    });
                }
                for ((_, part), range) in parts.iter().zip(&ranges) {
                    let bytes = tokio::fs::read(part).await?;
                    charge_output(&mut produced, bytes.len(), MAX_OUTPUT_BYTES)?;
                    outputs.push(Document::new(
                        format!("{stem}-{}.pdf", range.label()),
                        bytes,
                    ));
                }
            }
        }
        Ok(outputs)
    }

    /// `pdfcpu rotate [-p pages] in 90 out`
    async fn rotate(
        &self,
        doc: &Document,
        rotation: Rotation,
        pages: &str,
        timeout: Duration,
    ) -> Result<Document> {
        require_pdf(doc)?;
        let dir = workdir()?;
        let input = dir.path().join("in.pdf");
        let out = dir.path().join("out.pdf");
        tokio::fs::write(&input, &doc.bytes).await?;
        let mut args: Vec<String> = vec!["rotate".into()];
        if !pages.trim().is_empty() {
            let ranges = pages::parse(pages, page_count_off_thread(doc, timeout).await?)?;
            args.push("-p".into());
            args.push(pages::join(&ranges));
        }
        args.push(input.display().to_string());
        args.push(rotation.degrees().to_string());
        args.push(out.display().to_string());
        self.run(dir.path(), &args, timeout).await?;
        Ok(Document::new(
            doc.name.clone(),
            tokio::fs::read(&out).await?,
        ))
    }

    /// `pdfcpu optimize in out`
    async fn optimize(&self, doc: &Document, timeout: Duration) -> Result<Document> {
        self.transform(doc, &["optimize".into()], timeout).await
    }

    /// `pdfcpu bookmarks export in out.json`
    async fn read_bookmarks(&self, doc: &Document, timeout: Duration) -> Result<Vec<Bookmark>> {
        require_pdf(doc)?;
        let dir = workdir()?;
        let input = dir.path().join("in.pdf");
        let out = dir.path().join("bookmarks.json");
        tokio::fs::write(&input, &doc.bytes).await?;
        let args: Vec<String> = vec![
            "bookmarks".into(),
            "export".into(),
            input.display().to_string(),
            out.display().to_string(),
        ];
        self.run(dir.path(), &args, timeout).await?;
        let json = tokio::fs::read(&out).await.unwrap_or_default();
        if json.is_empty() {
            return Ok(Vec::new());
        }
        let list: PdfcpuBookmarks = serde_json::from_slice(&json).map_err(|e| Error::Engine {
            engine: ENGINE,
            message: format!("unexpected bookmark JSON: {e}"),
        })?;
        Ok(from_pdfcpu(list.bookmarks))
    }

    /// `pdfcpu bookmarks import --replace in bookmarks.json out`
    async fn write_bookmarks(
        &self,
        doc: &Document,
        bookmarks: &[Bookmark],
        timeout: Duration,
    ) -> Result<Document> {
        require_pdf(doc)?;
        let dir = workdir()?;
        let input = dir.path().join("in.pdf");
        let json = dir.path().join("bookmarks.json");
        let out = dir.path().join("out.pdf");
        tokio::fs::write(&input, &doc.bytes).await?;
        let list = PdfcpuBookmarks {
            bookmarks: to_pdfcpu(bookmarks),
        };
        tokio::fs::write(
            &json,
            serde_json::to_vec(&list).map_err(|e| Error::InvalidRequest(e.to_string()))?,
        )
        .await?;
        let args: Vec<String> = vec![
            "bookmarks".into(),
            "import".into(),
            "--replace".into(),
            input.display().to_string(),
            json.display().to_string(),
            out.display().to_string(),
        ];
        self.run(dir.path(), &args, timeout).await?;
        Ok(Document::new(
            doc.name.clone(),
            tokio::fs::read(&out).await?,
        ))
    }

    async fn watermark(
        &self,
        doc: &Document,
        overlay: &Overlay,
        timeout: Duration,
    ) -> Result<Document> {
        self.overlay("watermark", doc, overlay, timeout).await
    }

    async fn stamp(
        &self,
        doc: &Document,
        overlay: &Overlay,
        timeout: Duration,
    ) -> Result<Document> {
        self.overlay("stamp", doc, overlay, timeout).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_cap_is_opt_in() {
        let engine = PdfcpuEngine::new("pdfcpu".into());
        assert_eq!(engine.memory, None, "uncapped by default");
        assert_eq!(engine.clone().with_memory(0).memory, None, "0 means no cap");
        assert_eq!(engine.with_memory(512).memory, Some(512));
    }

    #[test]
    fn descriptions_follow_pdfcpu_syntax() {
        let overlay = Overlay {
            placement: None,
            kind: OverlayKind::Text {
                text: "DRAFT".into(),
                font_size: 24.0,
                colour: "#808080".into(),
            },
            position: Position::Center,
            opacity: 0.3,
            rotation: 45.0,
            scale: 0.5,
            pages: String::new(),
        };
        assert_eq!(
            description(&overlay),
            "pos:c, rot:45.0, scale:0.500 rel, op:0.30, font:Helvetica, points:24, fillcolor:#808080"
        );
        let image = Overlay {
            placement: None,
            kind: OverlayKind::Image {
                bytes: vec![],
                name: "logo.png".into(),
            },
            position: Position::BottomRight,
            scale: 0.0,
            rotation: 270.0,
            ..overlay
        };
        assert_eq!(
            description(&image),
            "pos:br, rot:-90.0, scale:1 abs, op:0.30"
        );
    }
}
