//! The qpdf engine, run as a process. It owns encryption and optimisation,
//! and backs merge, split, rotate and annotation flattening when the
//! pure-Rust engine is not in front of it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;

use super::{
    EncryptOptions, PdfEngine, Rotation, SplitMode, page_count_off_thread, pages, require_pdf,
};
use crate::error::{Error, Result};
use crate::job::Document;
use crate::limits::{MAX_OUTPUT_BYTES, charge_output};
use crate::process;

const ENGINE: &str = "qpdf";

/// qpdf as a child process.
#[derive(Debug, Clone)]
pub struct QpdfEngine {
    bin: PathBuf,
    linearize: bool,
}

impl QpdfEngine {
    pub fn new(bin: PathBuf) -> Self {
        Self {
            bin,
            linearize: false,
        }
    }

    /// Also linearize when optimising (`--linearize`).
    pub fn with_linearize(mut self, linearize: bool) -> Self {
        self.linearize = linearize;
        self
    }

    pub fn bin(&self) -> &Path {
        &self.bin
    }

    /// Runs qpdf in `dir`. Warnings (exit 3) are not failures.
    async fn run(&self, dir: &Path, args: &[String], timeout: Duration) -> Result<()> {
        let mut full = vec!["--warning-exit-0".to_string()];
        full.extend_from_slice(args);
        process::run(ENGINE, &self.bin, &full, dir, &[], timeout).await
    }

    /// One input, one output, with `flags` between.
    async fn transform(
        &self,
        doc: &Document,
        flags: &[String],
        timeout: Duration,
    ) -> Result<Document> {
        require_pdf(doc)?;
        let dir = workdir()?;
        let input = dir.path().join("in.pdf");
        let out = dir.path().join("out.pdf");
        tokio::fs::write(&input, &doc.bytes).await?;
        let mut args = flags.to_vec();
        args.push(input.display().to_string());
        args.push(out.display().to_string());
        self.run(dir.path(), &args, timeout).await?;
        Ok(Document::new(
            doc.name.clone(),
            tokio::fs::read(&out).await?,
        ))
    }
}

pub(super) fn workdir() -> Result<tempfile::TempDir> {
    Ok(tempfile::Builder::new().prefix("rusticdoc-").tempdir()?)
}

/// Writes a file readable by this user only.
async fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).await?;
    tokio::io::AsyncWriteExt::write_all(&mut file, bytes).await?;
    Ok(())
}

#[async_trait]
impl PdfEngine for QpdfEngine {
    fn name(&self) -> &'static str {
        ENGINE
    }

    /// `qpdf --empty --pages a.pdf b.pdf -- out.pdf`
    async fn merge(&self, docs: &[Document], timeout: Duration) -> Result<Document> {
        if docs.is_empty() {
            return Err(Error::InvalidRequest("merge needs at least one PDF".into()));
        }
        let dir = workdir()?;
        let mut args = vec!["--empty".to_string(), "--pages".to_string()];
        for (i, doc) in docs.iter().enumerate() {
            require_pdf(doc)?;
            let path = dir.path().join(format!("{i}.pdf"));
            tokio::fs::write(&path, &doc.bytes).await?;
            args.push(path.display().to_string());
        }
        args.push("--".to_string());
        let out = dir.path().join("merged.pdf");
        args.push(out.display().to_string());
        self.run(dir.path(), &args, timeout).await?;
        Ok(Document::new(
            docs[0].with_extension("pdf"),
            tokio::fs::read(&out).await?,
        ))
    }

    /// `qpdf --empty --pages in.pdf 1-3 -- out.pdf` per range, or
    /// `qpdf --split-pages=N in.pdf out-%d.pdf` for intervals.
    async fn split(
        &self,
        doc: &Document,
        mode: &SplitMode,
        timeout: Duration,
    ) -> Result<Vec<Document>> {
        require_pdf(doc)?;
        let count = page_count_off_thread(doc, timeout).await?;
        let dir = workdir()?;
        let input = dir.path().join("in.pdf");
        tokio::fs::write(&input, &doc.bytes).await?;
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
                for (i, spec) in specs.iter().enumerate() {
                    let out = dir.path().join(format!("part-{i}.pdf"));
                    let args = vec![
                        "--empty".to_string(),
                        "--pages".to_string(),
                        input.display().to_string(),
                        spec.clone(),
                        "--".to_string(),
                        out.display().to_string(),
                    ];
                    self.run(dir.path(), &args, timeout).await?;
                    let label = spec.replace(',', "_");
                    let bytes = tokio::fs::read(&out).await?;
                    charge_output(&mut produced, bytes.len(), MAX_OUTPUT_BYTES)?;
                    outputs.push(Document::new(format!("{stem}-{label}.pdf"), bytes));
                }
            }
            SplitMode::Intervals { span } => {
                let ranges = pages::intervals(*span, count)?;
                let args = vec![
                    format!("--split-pages={span}"),
                    input.display().to_string(),
                    dir.path().join("part-%d.pdf").display().to_string(),
                ];
                self.run(dir.path(), &args, timeout).await?;
                // Parts are named `part-01-02.pdf`; order them by first page.
                let mut parts: Vec<(u32, PathBuf)> = std::fs::read_dir(dir.path())?
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter_map(|p| {
                        let name = p.file_stem()?.to_str()?.strip_prefix("part-")?.to_string();
                        let first = name.split('-').next()?.parse().ok()?;
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

    /// `qpdf --generate-appearances --flatten-annotations=all`
    async fn flatten(&self, doc: &Document, timeout: Duration) -> Result<Document> {
        self.transform(
            doc,
            &[
                "--generate-appearances".into(),
                "--flatten-annotations=all".into(),
            ],
            timeout,
        )
        .await
    }

    /// `qpdf --rotate=+90:1-3`
    async fn rotate(
        &self,
        doc: &Document,
        rotation: Rotation,
        pages: &str,
        timeout: Duration,
    ) -> Result<Document> {
        require_pdf(doc)?;
        let flag = if pages.trim().is_empty() {
            format!("--rotate=+{}", rotation.degrees())
        } else {
            let ranges = pages::parse(pages, page_count_off_thread(doc, timeout).await?)?;
            format!("--rotate=+{}:{}", rotation.degrees(), pages::join(&ranges))
        };
        self.transform(doc, &[flag], timeout).await
    }

    /// `qpdf --object-streams=generate --recompress-flate --compression-level=9 [--linearize]`
    async fn optimize(&self, doc: &Document, timeout: Duration) -> Result<Document> {
        let mut flags = vec![
            "--object-streams=generate".to_string(),
            "--recompress-flate".to_string(),
            "--compression-level=9".to_string(),
        ];
        if self.linearize {
            flags.push("--linearize".to_string());
        }
        self.transform(doc, &flags, timeout).await
    }

    /// AES-256 with a user and an owner password. The passwords travel in
    /// an argument file readable by this user only, never on the command
    /// line.
    async fn encrypt(
        &self,
        doc: &Document,
        user_password: &str,
        owner_password: &str,
        timeout: Duration,
    ) -> Result<Document> {
        self.encrypt_with(
            doc,
            &EncryptOptions {
                user_password: user_password.to_string(),
                owner_password: owner_password.to_string(),
                ..EncryptOptions::default()
            },
            timeout,
        )
        .await
    }

    /// As `encrypt`, with restrictions. An absent owner password takes the
    /// user password's value.
    async fn encrypt_with(
        &self,
        doc: &Document,
        options: &EncryptOptions,
        timeout: Duration,
    ) -> Result<Document> {
        require_pdf(doc)?;
        let user_password = options.user_password.as_str();
        let owner_password = if options.owner_password.is_empty() {
            user_password
        } else {
            options.owner_password.as_str()
        };
        if owner_password.is_empty() {
            return Err(Error::InvalidRequest(
                "encrypt needs an owner password".into(),
            ));
        }
        // The argument file holds one argument per line, so a password with
        // a line break or NUL would become extra arguments.
        for (name, value) in [
            ("userPassword", user_password),
            ("ownerPassword", owner_password),
        ] {
            if value.contains(['\n', '\r', '\0']) {
                return Err(Error::InvalidRequest(format!(
                    "'{name}' must not contain line breaks or NUL characters"
                )));
            }
        }
        let dir = workdir()?;
        let argfile = dir.path().join("args");
        let mut contents = format!(
            "--encrypt\n--user-password={user_password}\n--owner-password={owner_password}\n--bits=256\n"
        );
        let permissions = &options.permissions;
        if permissions.restricted() {
            let yes_no = |allowed: bool| if allowed { "y" } else { "n" };
            contents.push_str(&format!(
                "--print={}\n--extract={}\n--modify-other={}\n--annotate={}\n--form={}\n--assemble={}\n",
                if permissions.printing { "full" } else { "none" },
                yes_no(permissions.copying),
                yes_no(permissions.modifying),
                yes_no(permissions.annotating),
                yes_no(permissions.filling_forms),
                yes_no(permissions.assembling),
            ));
        }
        contents.push_str("--\n");
        write_private(&argfile, contents.as_bytes()).await?;
        let input = dir.path().join("in.pdf");
        let out = dir.path().join("out.pdf");
        tokio::fs::write(&input, &doc.bytes).await?;
        let args = vec![
            format!("@{}", argfile.display()),
            input.display().to_string(),
            out.display().to_string(),
        ];
        self.run(dir.path(), &args, timeout).await?;
        Ok(Document::new(
            doc.name.clone(),
            tokio::fs::read(&out).await?,
        ))
    }
}
