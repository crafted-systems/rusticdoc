//! Conversion with LibreOffice.
//!
//! Each conversion runs its own `soffice` with a private profile directory,
//! which is simple and crash-isolated. The profile is copied from a seed
//! that is initialised once, because a brand-new profile makes LibreOffice
//! exit with code 81 and ask to be started again. A pool of warm workers
//! replaces the per-job process in a later phase; the interface stays.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{OnceCell, Semaphore};

use crate::error::{Error, Result};
use crate::job::{Document, Format, PdfaLevel};
use crate::process;

/// LibreOffice's exit code when it has just created a profile and wants a
/// second start.
const EXIT_RESTART_REQUESTED: i32 = 81;

#[derive(Debug, Clone)]
pub struct LibreOffice {
    bin: PathBuf,
    slots: Arc<Semaphore>,
    seed: Arc<OnceCell<PathBuf>>,
}

impl LibreOffice {
    pub fn new(bin: PathBuf, workers: usize) -> Self {
        Self {
            bin,
            slots: Arc::new(Semaphore::new(workers.max(1))),
            seed: Arc::new(OnceCell::new()),
        }
    }

    pub fn bin(&self) -> &PathBuf {
        &self.bin
    }

    /// Converts `doc` to `to`. A `pdfa` level makes the PDF export a PDF/A
    /// export; it is ignored for other targets.
    pub async fn convert(
        &self,
        doc: &Document,
        to: Format,
        pdfa: Option<PdfaLevel>,
        timeout: Duration,
    ) -> Result<Document> {
        let ext = doc.extension();
        if ext.is_empty() {
            return Err(Error::InvalidRequest(format!(
                "'{}' has no extension, so its format is unknown",
                doc.name
            )));
        }
        let _slot = self
            .slots
            .acquire()
            .await
            .map_err(|_| Error::EngineUnavailable("LibreOffice pool closed".into()))?;

        let dir = tempfile::Builder::new().prefix("rusticdoc-").tempdir()?;
        let input = dir.path().join(format!("input.{ext}"));
        tokio::fs::write(&input, &doc.bytes).await?;
        let out_dir = dir.path().join("out");
        tokio::fs::create_dir(&out_dir).await?;
        let profile = dir.path().join("profile");
        self.prepare_profile(&profile, timeout).await?;

        let filter = match (to, pdfa) {
            (Format::Pdf, Some(level)) => format!(
                "pdf:{}:{{\"SelectPdfVersion\":{{\"type\":\"long\",\"value\":\"{}\"}}}}",
                export_filter(&ext),
                level.libreoffice_version()
            ),
            (other, _) => other.extension().to_string(),
        };
        let args = vec![
            format!("-env:UserInstallation=file://{}", profile.display()),
            "--headless".into(),
            "--norestore".into(),
            "--nologo".into(),
            "--nodefault".into(),
            "--convert-to".into(),
            filter,
            "--outdir".into(),
            out_dir.display().to_string(),
            input.display().to_string(),
        ];
        let env = [("HOME", dir.path().display().to_string())];
        match process::run("libreoffice", &self.bin, &args, dir.path(), &env, timeout).await {
            Err(Error::Engine { message, .. }) if is_restart_request(&message) => {
                tracing::debug!("LibreOffice asked for a restart; running the conversion again");
                process::run("libreoffice", &self.bin, &args, dir.path(), &env, timeout).await?;
            }
            other => other?,
        }

        let produced = out_dir.join(format!("input.{}", to.extension()));
        let bytes = tokio::fs::read(&produced)
            .await
            .map_err(|_| Error::Engine {
                engine: "libreoffice",
                message: format!("no {} was produced for {}", to.extension(), doc.name),
            })?;
        Ok(Document::new(doc.with_extension(to.extension()), bytes))
    }

    /// Copies the initialised seed profile into `profile`. The seed is made
    /// on first use by starting LibreOffice once and letting it exit.
    async fn prepare_profile(&self, profile: &Path, timeout: Duration) -> Result<()> {
        let bin = self.bin.clone();
        let seed = self
            .seed
            .get_or_try_init(|| async move {
                let dir =
                    std::env::temp_dir().join(format!("rusticdoc-profile-{}", std::process::id()));
                let _ = tokio::fs::remove_dir_all(&dir).await;
                tokio::fs::create_dir_all(&dir).await?;
                let args = vec![
                    format!("-env:UserInstallation=file://{}", dir.display()),
                    "--headless".into(),
                    "--norestore".into(),
                    "--nologo".into(),
                    "--terminate_after_init".into(),
                ];
                let env = [("HOME", dir.display().to_string())];
                match process::run("libreoffice", &bin, &args, &dir, &env, timeout).await {
                    Ok(()) => {}
                    Err(Error::Engine { message, .. }) if is_restart_request(&message) => {}
                    Err(e) => return Err(e),
                }
                tracing::info!(path = %dir.display(), "LibreOffice profile initialised");
                Ok::<_, Error>(dir)
            })
            .await?
            .clone();
        let target = profile.to_path_buf();
        tokio::task::spawn_blocking(move || copy_dir(&seed, &target))
            .await
            .map_err(|e| Error::Engine {
                engine: "libreoffice",
                message: format!("profile copy: {e}"),
            })??;
        Ok(())
    }
}

fn is_restart_request(message: &str) -> bool {
    let code = message.strip_prefix("exit ").map(|rest| {
        rest.split(|c: char| !c.is_ascii_digit())
            .next()
            .unwrap_or("")
    });
    code.and_then(|c| c.parse::<i32>().ok()) == Some(EXIT_RESTART_REQUESTED)
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let dest = to.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

/// The PDF export filter for a source format, which decides which
/// application LibreOffice opens it with.
fn export_filter(ext: &str) -> &'static str {
    match ext {
        "xlsx" | "xls" | "ods" | "csv" | "tsv" | "fods" => "calc_pdf_Export",
        "pptx" | "ppt" | "odp" | "fodp" => "impress_pdf_Export",
        "odg" | "fodg" | "svg" | "vsd" | "vsdx" => "draw_pdf_Export",
        _ => "writer_pdf_Export",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_follow_the_application() {
        assert_eq!(export_filter("docx"), "writer_pdf_Export");
        assert_eq!(export_filter("xlsx"), "calc_pdf_Export");
        assert_eq!(export_filter("odp"), "impress_pdf_Export");
    }

    #[test]
    fn restart_requests_are_recognised() {
        assert!(is_restart_request("exit 81: some log line"));
        assert!(!is_restart_request("exit 1"));
        assert!(!is_restart_request("exit 811"));
    }

    #[tokio::test]
    async fn missing_binary_is_an_engine_error() {
        let lo = LibreOffice::new(PathBuf::from("/nonexistent/soffice"), 1);
        let doc = Document::new("a.txt", b"hi".to_vec());
        let err = lo
            .convert(&doc, Format::Pdf, None, Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                Error::Engine {
                    engine: "libreoffice",
                    ..
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn copy_dir_copies_nested_files() {
        let src = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(src.path().join("a/b")).unwrap();
        std::fs::write(src.path().join("a/b/c.txt"), b"x").unwrap();
        let dst = tempfile::tempdir().unwrap();
        copy_dir(src.path(), &dst.path().join("p")).unwrap();
        assert_eq!(std::fs::read(dst.path().join("p/a/b/c.txt")).unwrap(), b"x");
    }
}
