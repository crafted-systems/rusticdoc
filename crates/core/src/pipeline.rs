//! The pipeline: template stage, then the steps in order.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::convert::LibreOffice;
use crate::error::{Error, Result};
use crate::job::{Document, Format, Output, RenderRequest, Step};
use crate::pdf::Qpdf;
use crate::template;

/// The engines the service found at start. Absent engines make their steps
/// fail with [`Error::EngineUnavailable`] rather than stopping the service.
#[derive(Debug, Default)]
pub struct Engines {
    pub libreoffice: Option<LibreOffice>,
    pub qpdf: Option<Qpdf>,
}

impl Engines {
    pub fn from_config(config: &Config) -> Self {
        Self {
            libreoffice: config
                .libreoffice_bin
                .clone()
                .map(|bin| LibreOffice::new(bin, config.workers)),
            qpdf: config.qpdf_bin.clone().map(Qpdf::new),
        }
    }

    /// What `/version` reports.
    pub fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "libreoffice": self.libreoffice.as_ref().map(|l| l.bin().display().to_string()),
            "qpdf": self.qpdf.as_ref().map(|q| q.bin().display().to_string()),
            "templates": { "text": template::TEXT_FORMATS, "office": "phase 1" },
        })
    }

    fn libreoffice(&self) -> Result<&LibreOffice> {
        self.libreoffice.as_ref().ok_or_else(|| {
            Error::EngineUnavailable(
                "LibreOffice is not configured; set RUSTICDOC_LIBREOFFICE_BIN".into(),
            )
        })
    }

    fn qpdf(&self) -> Result<&Qpdf> {
        self.qpdf.as_ref().ok_or_else(|| {
            Error::EngineUnavailable("qpdf is not configured; set RUSTICDOC_QPDF_BIN".into())
        })
    }
}

#[derive(Debug, Clone)]
pub struct Pipeline {
    engines: Arc<Engines>,
    timeout: Duration,
}

impl Pipeline {
    pub fn new(engines: Arc<Engines>, timeout: Duration) -> Self {
        Self { engines, timeout }
    }

    pub fn engines(&self) -> &Engines {
        &self.engines
    }

    /// Runs the request. The caller applies the overall time limit; each
    /// engine call gets the configured timeout on its own.
    pub async fn run(&self, req: RenderRequest) -> Result<Output> {
        let started = Instant::now();
        let mut timings: Vec<(String, u128)> = Vec::new();

        let has_data = !is_empty_object(&req.data);
        let ext = req.template.extension();
        let mut current = if req.templating && (has_data || template::is_text_template(&ext)) {
            template::render(&req.template, &req.data, &req.ctx)?
        } else {
            req.template.clone()
        };
        timings.push(("template".into(), started.elapsed().as_millis()));

        for step in &req.steps {
            let t = Instant::now();
            current = match step {
                Step::Convert { to } => {
                    self.engines
                        .libreoffice()?
                        .convert(&current, *to, None, self.timeout)
                        .await?
                }
                Step::Pdfa { level } => {
                    self.engines
                        .libreoffice()?
                        .convert(&current, Format::Pdf, Some(*level), self.timeout)
                        .await?
                }
                Step::Metadata { fields } => {
                    self.engines
                        .qpdf()?
                        .write_metadata(&current, fields, self.timeout)
                        .await?
                }
                Step::Encrypt {
                    user_password,
                    owner_password,
                } => {
                    self.engines
                        .qpdf()?
                        .encrypt(&current, user_password, owner_password, self.timeout)
                        .await?
                }
                Step::Merge => {
                    let mut docs = Vec::with_capacity(1 + req.extra.len());
                    docs.push(current.clone());
                    docs.extend(req.extra.iter().cloned());
                    self.engines.qpdf()?.merge(&docs, self.timeout).await?
                }
            };
            timings.push((step.name().into(), t.elapsed().as_millis()));
        }

        let name = match &req.output_name {
            Some(stem) => format!("{stem}.{}", current.extension()),
            None => current.name.clone(),
        };
        let content_type = current.content_type();
        tracing::info!(
            output = %name,
            total_ms = started.elapsed().as_millis(),
            ?timings,
            "pipeline finished"
        );
        Ok(Output {
            name,
            content_type,
            bytes: current.bytes,
            timings,
        })
    }
}

fn is_empty_object(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => true,
        serde_json::Value::Object(m) => m.is_empty(),
        _ => false,
    }
}

/// A document with no steps and no data is returned as it came in.
pub fn passthrough(doc: Document) -> Output {
    Output {
        name: doc.name.clone(),
        content_type: doc.content_type(),
        bytes: doc.bytes,
        timings: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pipeline() -> Pipeline {
        Pipeline::new(Arc::new(Engines::default()), Duration::from_secs(5))
    }

    #[tokio::test]
    async fn text_template_without_steps_returns_the_filled_text() {
        let mut req = RenderRequest::new(Document::new("hello.txt", b"Hi {{ name }}".to_vec()));
        req.data = json!({"name": "Ada"});
        req.output_name = Some("greeting".into());
        let out = pipeline().run(req).await.unwrap();
        assert_eq!(out.name, "greeting.txt");
        assert_eq!(&out.bytes[..], b"Hi Ada");
        assert_eq!(out.timings[0].0, "template");
    }

    #[tokio::test]
    async fn office_file_without_data_passes_through_to_the_steps() {
        let mut req = RenderRequest::new(Document::new("report.docx", vec![0x50, 0x4b, 3, 4]));
        req.steps = vec![Step::Convert { to: Format::Pdf }];
        let err = pipeline().run(req).await.unwrap_err();
        assert!(matches!(err, Error::EngineUnavailable(_)), "{err}");
    }

    #[tokio::test]
    async fn office_file_with_data_is_refused_as_unsupported() {
        let mut req = RenderRequest::new(Document::new("report.docx", vec![0x50, 0x4b, 3, 4]));
        req.data = json!({"a": 1});
        let err = pipeline().run(req).await.unwrap_err();
        assert!(matches!(err, Error::Unsupported(_)), "{err}");
    }
}
