//! The pipeline: template stage, then the steps in order.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::convert::{ConvertOptions, LibreOffice};
use crate::error::{Error, Result};
use crate::job;
use crate::job::{Document, Format, Output, RenderRequest, Step};
use crate::pdfops::MultiEngine;
use crate::template;

/// The engines the service found at start. Absent engines make their steps
/// fail with [`Error::EngineUnavailable`] rather than stopping the service.
#[derive(Debug, Default)]
pub struct Engines {
    pub libreoffice: Option<LibreOffice>,
    /// PDF operations, tried in order: lopdf, then qpdf and pdfcpu when
    /// configured.
    pub pdf: MultiEngine,
    /// PDF operations for Gotenberg's routes, in the order they use.
    pub forms: MultiEngine,
}

impl Engines {
    pub fn from_config(config: &Config) -> Self {
        Self {
            libreoffice: config.libreoffice_bin.clone().map(|bin| {
                LibreOffice::new(bin, config.workers).with_fonts_dir(config.fonts_dir.clone())
            }),
            pdf: MultiEngine::standard(config.qpdf_bin.clone(), config.pdfcpu_bin.clone())
                .with_lists(
                    config.qpdf_bin.clone(),
                    config.pdfcpu_bin.clone(),
                    &config.compat.engine_orders,
                ),
            forms: MultiEngine::for_forms(
                config.qpdf_bin.clone(),
                config.pdfcpu_bin.clone(),
                &config.compat.engine_orders,
            ),
        }
    }

    /// What `/version` reports.
    pub fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "libreoffice": self.libreoffice.as_ref().map(|l| l.bin().display().to_string()),
            "pdf": self.pdf.names(),
            "templates": { "text": template::TEXT_FORMATS, "office": template::OFFICE_FORMATS },
        })
    }

    fn libreoffice(&self) -> Result<&LibreOffice> {
        self.libreoffice.as_ref().ok_or_else(|| {
            Error::EngineUnavailable(
                "LibreOffice is not configured; set RUSTICDOC_LIBREOFFICE_BIN".into(),
            )
        })
    }
}

/// Most steps one request may run.
pub const MAX_STEPS: usize = 50;
/// Most documents one request may have in flight after splits.
pub const MAX_DOCUMENTS: usize = 1_000;

#[derive(Debug, Clone)]
pub struct Pipeline {
    engines: Arc<Engines>,
    timeout: Duration,
    /// Largest rendered template accepted, in bytes.
    max_output: usize,
}

impl Pipeline {
    pub fn new(engines: Arc<Engines>, timeout: Duration) -> Self {
        Self {
            engines,
            timeout,
            max_output: 256 * 1024 * 1024,
        }
    }

    pub fn with_max_output(mut self, bytes: usize) -> Self {
        self.max_output = bytes;
        self
    }

    pub fn engines(&self) -> &Engines {
        &self.engines
    }

    /// Runs the request. The caller applies the overall time limit; each
    /// engine call gets the configured timeout on its own.
    ///
    /// Documents flow as a list: `split` fans one out into several, and
    /// every later step runs on each; `merge` collapses them again. Several
    /// documents at the end come back as a ZIP.
    pub async fn run(&self, req: RenderRequest) -> Result<Output> {
        let started = Instant::now();
        let mut timings: Vec<(String, u128)> = Vec::new();
        req.convert_options.validate()?;
        if req.steps.len() > MAX_STEPS {
            return Err(Error::InvalidRequest(format!(
                "{} steps is more than the {MAX_STEPS} one request may run",
                req.steps.len()
            )));
        }

        let has_data = !is_empty_object(&req.data);
        let ext = req.template.extension();
        let first = if req.templating && (has_data || template::is_text_template(&ext)) {
            // Rendering is CPU work: it runs off the async threads so the
            // caller's timeout still fires, and its size is capped.
            let (t, d, c) = (req.template.clone(), req.data.clone(), req.ctx.clone());
            let rendered = tokio::task::spawn_blocking(move || template::render(&t, &d, &c))
                .await
                .map_err(|e| Error::Engine {
                    engine: "template",
                    message: e.to_string(),
                })??;
            if rendered.bytes.len() > self.max_output {
                return Err(Error::InvalidRequest(format!(
                    "rendered template is {} bytes, over the {} byte limit",
                    rendered.bytes.len(),
                    self.max_output
                )));
            }
            rendered
        } else {
            req.template.clone()
        };
        timings.push(("template".into(), started.elapsed().as_millis()));

        let mut current: Vec<Document> = vec![first];
        let mut extra: Vec<Document> = req.extra.clone();
        let timeout = self.timeout;
        for step in &req.steps {
            let t = Instant::now();
            current = match step {
                Step::Merge => {
                    let mut docs = std::mem::take(&mut current);
                    docs.append(&mut extra);
                    if docs.len() == 1 && docs[0].extension() == "pdf" {
                        docs
                    } else {
                        vec![self.engines.pdf.merge(&docs, timeout).await?]
                    }
                }
                Step::Split { mode } => {
                    let mut out = Vec::new();
                    for doc in &current {
                        out.extend(self.engines.pdf.split(doc, mode, timeout).await?);
                        if out.len() > MAX_DOCUMENTS {
                            return Err(Error::InvalidRequest(format!(
                                "splitting would leave more than {MAX_DOCUMENTS} documents in flight"
                            )));
                        }
                    }
                    out
                }
                _ => {
                    let mut out = Vec::with_capacity(current.len());
                    for doc in &current {
                        out.push(self.one(step, doc, &req.convert_options).await?);
                    }
                    out
                }
            };
            timings.push((step.name().into(), t.elapsed().as_millis()));
        }

        let stem = req.output_name.clone().unwrap_or_else(|| {
            std::path::Path::new(&req.template.name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("document")
                .to_string()
        });
        let result = match current.len() {
            0 => {
                return Err(Error::Engine {
                    engine: "pipeline",
                    message: "no document left".into(),
                });
            }
            1 => {
                let doc = current.remove(0);
                Document::new(format!("{stem}.{}", doc.extension()), doc.bytes)
            }
            _ => {
                let parts: Vec<Document> = current
                    .into_iter()
                    .enumerate()
                    .map(|(i, d)| {
                        Document::new(format!("{stem}_{}.{}", i + 1, d.extension()), d.bytes)
                    })
                    .collect();
                job::zip_documents(&stem, &parts)?
            }
        };
        tracing::info!(
            output = %result.name,
            total_ms = started.elapsed().as_millis(),
            ?timings,
            "pipeline finished"
        );
        Ok(Output {
            name: result.name.clone(),
            content_type: result.content_type(),
            bytes: result.bytes,
            timings,
        })
    }

    /// One document through one step; `Merge` and `Split` are handled by
    /// the caller because they change the number of documents.
    async fn one(&self, step: &Step, doc: &Document, options: &ConvertOptions) -> Result<Document> {
        let t = self.timeout;
        let pdf = &self.engines.pdf;
        Ok(match step {
            Step::Convert { to } => {
                self.engines
                    .libreoffice()?
                    .convert_with(doc, *to, options, t)
                    .await?
            }
            Step::Pdfa { level } => {
                let mut o = options.clone();
                o.pdfa = Some(*level);
                self.engines
                    .libreoffice()?
                    .convert_with(doc, Format::Pdf, &o, t)
                    .await?
            }
            Step::Pdfua => {
                let mut o = options.clone();
                o.pdfua = true;
                self.engines
                    .libreoffice()?
                    .convert_with(doc, Format::Pdf, &o, t)
                    .await?
            }
            Step::Watermark { overlay } => pdf.watermark(doc, overlay, t).await?,
            Step::Stamp { overlay } => pdf.stamp(doc, overlay, t).await?,
            Step::Rotate { rotation, pages } => pdf.rotate(doc, *rotation, pages, t).await?,
            Step::Flatten => pdf.flatten(doc, t).await?,
            Step::Optimize => pdf.optimize(doc, t).await?,
            Step::Bookmarks { bookmarks } => pdf.write_bookmarks(doc, bookmarks, t).await?,
            Step::Metadata { metadata } => pdf.write_metadata(doc, metadata, t).await?,
            Step::Embed { files } => pdf.embed(doc, files, t).await?,
            Step::Facturx { xml, profile } => pdf.facturx(doc, xml, *profile, t).await?,
            Step::Encrypt {
                user_password,
                owner_password,
            } => pdf.encrypt(doc, user_password, owner_password, t).await?,
            Step::Merge | Step::Split { .. } => unreachable!("handled by run"),
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
    async fn runaway_templates_are_stopped() {
        // A loop with 10^9 iterations exhausts the fuel budget.
        let mut req = RenderRequest::new(Document::new(
            "big.txt",
            b"{% for i in range(1000000000) %}x{% endfor %}".to_vec(),
        ));
        req.data = json!({"a": 1});
        let err = pipeline().run(req).await.unwrap_err();
        assert!(matches!(err, Error::Template(_)), "{err}");

        // Output over the cap is refused even when the fuel allows it.
        let mut req = RenderRequest::new(Document::new(
            "wide.txt",
            b"{% for i in range(2000) %}0123456789{% endfor %}".to_vec(),
        ));
        req.data = json!({"a": 1});
        let err = pipeline()
            .with_max_output(10_000)
            .run(req)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::InvalidRequest(_)), "{err}");
    }

    #[tokio::test]
    async fn office_file_without_data_passes_through_to_the_steps() {
        let mut req = RenderRequest::new(Document::new("report.docx", vec![0x50, 0x4b, 3, 4]));
        req.steps = vec![Step::Convert { to: Format::Pdf }];
        let err = pipeline().run(req).await.unwrap_err();
        assert!(matches!(err, Error::EngineUnavailable(_)), "{err}");
    }

    #[tokio::test]
    async fn office_file_with_data_goes_through_the_template_engine() {
        // Not a real ZIP, so the engine reports a template error rather
        // than converting it as it is.
        let mut req = RenderRequest::new(Document::new("report.docx", vec![0x50, 0x4b, 3, 4]));
        req.data = json!({"a": 1});
        let err = pipeline().run(req).await.unwrap_err();
        assert!(matches!(err, Error::Template(_)), "{err}");
    }
}
