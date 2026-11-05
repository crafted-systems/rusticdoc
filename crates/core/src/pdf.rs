//! PDF operations through qpdf.
//!
//! The design keeps engines behind one interface with a fallback order; this
//! first version has qpdf alone, called as a process, for merge and encrypt.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::job::Document;
use crate::process;

#[derive(Debug, Clone)]
pub struct Qpdf {
    bin: PathBuf,
}

impl Qpdf {
    pub fn new(bin: PathBuf) -> Self {
        Self { bin }
    }

    pub fn bin(&self) -> &PathBuf {
        &self.bin
    }

    /// Concatenates the documents in order.
    pub async fn merge(&self, docs: &[Document], timeout: Duration) -> Result<Document> {
        if docs.is_empty() {
            return Err(Error::InvalidRequest("merge needs at least one PDF".into()));
        }
        let dir = tempfile::Builder::new().prefix("rusticdoc-").tempdir()?;
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
        process::run("qpdf", &self.bin, &args, dir.path(), &[], timeout).await?;
        Ok(Document::new(
            docs[0].with_extension("pdf"),
            tokio::fs::read(&out).await?,
        ))
    }

    /// AES-256 encryption with a user and an owner password.
    pub async fn encrypt(
        &self,
        doc: &Document,
        user_password: &str,
        owner_password: &str,
        timeout: Duration,
    ) -> Result<Document> {
        require_pdf(doc)?;
        if owner_password.is_empty() {
            return Err(Error::InvalidRequest(
                "encrypt needs an owner password".into(),
            ));
        }
        let dir = tempfile::Builder::new().prefix("rusticdoc-").tempdir()?;
        let input = dir.path().join("in.pdf");
        let out = dir.path().join("out.pdf");
        tokio::fs::write(&input, &doc.bytes).await?;
        let args = vec![
            "--encrypt".to_string(),
            user_password.to_string(),
            owner_password.to_string(),
            "256".to_string(),
            "--".to_string(),
            input.display().to_string(),
            out.display().to_string(),
        ];
        process::run("qpdf", &self.bin, &args, dir.path(), &[], timeout).await?;
        Ok(Document::new(
            doc.name.clone(),
            tokio::fs::read(&out).await?,
        ))
    }

    /// Metadata writing waits for the port of gotenberg-slim's qpdf-metadata
    /// engine, which keeps the Info dictionary and XMP in step for PDF/A.
    pub async fn write_metadata(
        &self,
        _doc: &Document,
        _fields: &BTreeMap<String, String>,
        _timeout: Duration,
    ) -> Result<Document> {
        Err(Error::Unsupported(
            "metadata writing arrives with the qpdf-metadata engine port in phase 0".into(),
        ))
    }
}

fn require_pdf(doc: &Document) -> Result<()> {
    if doc.extension() != "pdf" || !doc.bytes.starts_with(b"%PDF") {
        return Err(Error::InvalidRequest(format!(
            "'{}' is not a PDF",
            doc.name
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn non_pdf_input_is_rejected_before_running_anything() {
        let q = Qpdf::new(PathBuf::from("/nonexistent/qpdf"));
        let doc = Document::new("a.pdf", b"not a pdf".to_vec());
        let err = q
            .encrypt(&doc, "", "owner", Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::InvalidRequest(_)), "{err}");
    }
}
