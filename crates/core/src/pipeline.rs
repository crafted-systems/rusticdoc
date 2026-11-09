//! The pipeline: template stage, then the steps in order.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::convert::{ConvertOptions, LibreOffice};
use crate::error::{Error, Result};
use crate::job;
use crate::job::{Document, Format, Output, RenderRequest, Step};
use crate::limits;
use crate::pdfops::MultiEngine;
use crate::pdfops::sign::{self, Signer};
use crate::render::Renderer;
use crate::template;

/// The engines the service found at start. Absent engines make their steps
/// fail with [`Error::EngineUnavailable`] rather than stopping the service.
#[derive(Debug, Default)]
pub struct Engines {
    pub libreoffice: Option<LibreOffice>,
    /// PDF operations, tried in order: lopdf, then qpdf and pdfcpu when
    /// configured.
    pub pdf: MultiEngine,
    /// `RUSTICDOC_CONVERT_DISABLED`: conversion steps refuse.
    pub office_routes_disabled: bool,
    /// The key for cryptographic signatures, from `RUSTICDOC_SIGNING_KEY` and
    /// `RUSTICDOC_SIGNING_CERT`. Without it, `sign` steps answer 503.
    pub signer: Option<Arc<Signer>>,
}

impl Engines {
    pub fn from_config(config: &Config) -> Self {
        Self {
            libreoffice: config.libreoffice_bin.clone().map(|bin| {
                let pool = LibreOffice::new(bin, config.workers)
                    .with_max_disk(config.libreoffice_max_disk)
                    .with_fonts_dir(config.fonts_dir.clone());
                if config.store.is_some() {
                    pool.with_fonts_dir(config.font_cache_dir.clone())
                } else {
                    pool
                }
            }),
            pdf: MultiEngine::standard(config.qpdf_bin.clone(), config.pdfcpu_bin.clone())
                .with_lists(
                    config.qpdf_bin.clone(),
                    config.pdfcpu_bin.clone(),
                    &config.engine_orders,
                ),
            office_routes_disabled: config.convert_disabled,
            // `Config::check` has refused a key that does not load before a
            // service starts; a library or CLI caller that skipped it gets no
            // signer and a 503 that says why.
            signer: match config.signer() {
                Ok(signer) => signer.map(Arc::new),
                Err(problem) => {
                    tracing::error!("{problem}");
                    None
                }
            },
        }
    }

    /// What `/v1/version` reports.
    pub fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "libreoffice": self.libreoffice.as_ref().map(|l| l.bin().display().to_string()),
            "pdf": self.pdf.names(),
            "templates": { "text": template::TEXT_FORMATS, "office": template::OFFICE_FORMATS },
            "signing": self.signer.is_some(),
        })
    }

    /// The signing key, or why there is none: a 503 whose text the client
    /// reads, since it names a setting and nothing internal.
    pub fn signer(&self) -> Result<&Arc<Signer>> {
        self.signer.as_ref().ok_or_else(|| {
            Error::Status(
                503,
                "cryptographic signing is not configured on this service; its operator sets RUSTICDOC_SIGNING_KEY and RUSTICDOC_SIGNING_CERT"
                    .into(),
            )
        })
    }

    /// Refuses a step list before any work is done: `sign` must come last,
    /// cannot be combined with `encrypt`, and needs a key.
    pub fn check_steps(&self, steps: &[Step]) -> Result<()> {
        check_steps(steps)?;
        if steps.iter().any(|s| matches!(s, Step::Sign { .. })) {
            self.signer()?;
        }
        Ok(())
    }

    fn libreoffice(&self) -> Result<&LibreOffice> {
        if self.office_routes_disabled {
            return Err(Error::EngineUnavailable(
                "LibreOffice conversion is disabled on this service".into(),
            ));
        }
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
    /// Most bytes a text template may write.
    max_output: usize,
    /// Where template code runs.
    renderer: Renderer,
}

impl Pipeline {
    pub fn new(engines: Arc<Engines>, timeout: Duration) -> Self {
        Self {
            engines,
            timeout,
            max_output: crate::limits::DEFAULT_MAX_RENDER_BYTES,
            renderer: Renderer::in_process(),
        }
    }

    pub fn with_max_output(mut self, bytes: usize) -> Self {
        self.max_output = bytes;
        self
    }

    pub fn with_renderer(mut self, renderer: Renderer) -> Self {
        self.renderer = renderer;
        self
    }

    pub fn renderer(&self) -> &Renderer {
        &self.renderer
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
        self.engines.check_steps(&req.steps)?;

        let has_data = !is_empty_object(&req.data);
        let ext = req.template.extension();
        let first = if req.templating && (has_data || template::is_text_template(&ext)) {
            // Rendering is CPU work on client code: it runs in a worker or off
            // the async threads, and its output is capped as it is written.
            self.renderer
                .render(
                    &req.template,
                    &req.data,
                    &req.ctx,
                    self.max_output,
                    self.timeout,
                )
                .await?
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
                    let mut out: Vec<Document> = Vec::new();
                    let mut produced = 0;
                    for doc in &current {
                        let parts = self.engines.pdf.split(doc, mode, timeout).await?;
                        for part in &parts {
                            limits::charge_output(
                                &mut produced,
                                part.bytes.len(),
                                limits::MAX_OUTPUT_BYTES,
                            )?;
                        }
                        out.extend(parts);
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
                job::zip_off_thread(&stem, parts, timeout).await?
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

    /// True when Fulgur lays out `doc`: HTML, Fulgur chosen, and no PDF/A,
    /// which only LibreOffice writes.
    fn fulgur_takes(&self, doc: &Document, options: &ConvertOptions) -> bool {
        crate::html::is_html(doc)
            && options.html_engine == crate::html::HtmlEngine::Fulgur
            && options.pdfa.is_none()
    }

    /// HTML to PDF in the render workers, with the request's assets and fonts.
    async fn fulgur(
        &self,
        doc: &Document,
        options: &ConvertOptions,
        pdfua: bool,
    ) -> Result<Document> {
        if self.engines.office_routes_disabled {
            return Err(Error::EngineUnavailable(
                "conversion is disabled on this service".into(),
            ));
        }
        let assets: Vec<Document> = options
            .assets
            .iter()
            .map(|(name, bytes)| Document::new(name.clone(), bytes.clone()))
            .chain(
                options
                    .fonts
                    .iter()
                    .map(|(name, bytes)| Document::new(name.clone(), bytes.clone())),
            )
            .collect();
        let html_options = crate::html::HtmlOptions {
            landscape: options.landscape,
            pdfua,
        };
        let pdf = self
            .renderer
            .html_to_pdf(doc, &assets, &html_options, self.timeout)
            .await?;
        Ok(Document::new(doc.with_extension("pdf"), pdf))
    }

    /// One document through one step; `Merge` and `Split` are handled by
    /// the caller because they change the number of documents.
    async fn one(&self, step: &Step, doc: &Document, options: &ConvertOptions) -> Result<Document> {
        let t = self.timeout;
        let pdf = &self.engines.pdf;
        Ok(match step {
            Step::Convert { to: Format::Pdf } if self.fulgur_takes(doc, options) => {
                self.fulgur(doc, options, options.pdfua).await?
            }
            Step::Convert { to } => {
                check_convertible(doc)?;
                self.engines
                    .libreoffice()?
                    .convert_with(doc, *to, options, t)
                    .await?
            }
            Step::Pdfua if self.fulgur_takes(doc, options) => {
                self.fulgur(doc, options, true).await?
            }
            Step::Pdfa { level } => {
                check_convertible(doc)?;
                let mut o = options.clone();
                o.pdfa = Some(*level);
                self.engines
                    .libreoffice()?
                    .convert_with(doc, Format::Pdf, &o, t)
                    .await?
            }
            Step::Pdfua => {
                check_convertible(doc)?;
                let mut o = options.clone();
                o.pdfua = true;
                self.engines
                    .libreoffice()?
                    .convert_with(doc, Format::Pdf, &o, t)
                    .await?
            }
            Step::Watermark { overlay } => pdf.watermark(doc, overlay, t).await?,
            Step::Stamp { overlay } => pdf.stamp(doc, overlay, t).await?,
            Step::Underlay { base, name } => underlay(doc, base, name, t).await?,
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
            Step::Sign { options } => {
                sign::sign(doc, self.engines.signer()?.clone(), options, t).await?
            }
            Step::Merge | Step::Split { .. } => unreachable!("handled by run"),
        })
    }
}

/// Lays every page of `doc` over the matching page of the base PDF, as
/// `/v1/pdf/overlay` does. The result is the base, with its page sizes and
/// any pages past the end of `doc`; pages of `doc` past the end of the base
/// are appended.
async fn underlay(doc: &Document, base: &[u8], name: &str, timeout: Duration) -> Result<Document> {
    crate::pdfops::require_pdf(doc).map_err(|_| {
        Error::InvalidRequest(format!(
            "'underlay' needs a PDF, and '{}' is not one; put 'convert' before it",
            doc.name
        ))
    })?;
    let base = Document::new(name, base.to_vec());
    crate::pdfops::require_pdf(&base)?;
    let over = doc.bytes.clone();
    let bytes = crate::pdfops::blocking(timeout, move |_| {
        crate::pdfform::overlay(&base.bytes, &over)
    })
    .await?;
    Ok(Document::new(doc.with_extension("pdf"), bytes))
}

/// The order rules of `sign`, which need no engine to check. A signature
/// covers the exact bytes it is made over, so any step after it would break
/// it; and signing an encrypted file, or encrypting a signed one, is not
/// something this service does.
pub fn check_steps(steps: &[Step]) -> Result<()> {
    let Some(at) = steps.iter().position(|s| matches!(s, Step::Sign { .. })) else {
        return Ok(());
    };
    if steps.iter().any(|s| matches!(s, Step::Encrypt { .. })) {
        return Err(Error::InvalidRequest(
            "'sign' and 'encrypt' cannot be combined: this service does not sign encrypted files or encrypt signed ones".into(),
        ));
    }
    if at != steps.len() - 1 {
        return Err(Error::InvalidRequest(format!(
            "'sign' must be the last step, and here '{}' follows it: a later step would change the signed bytes",
            steps[at + 1].name()
        )));
    }
    Ok(())
}

/// A step may send LibreOffice what `/v1/convert` would, or a text template it
/// has just filled.
fn check_convertible(doc: &Document) -> Result<()> {
    if crate::convert::accepts(doc) || template::is_text_template(&doc.extension()) {
        return Ok(());
    }
    Err(Error::InvalidRequest(format!(
        "'{}' is not a file type this service converts",
        doc.name
    )))
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

    fn sign_step() -> Step {
        Step::Sign {
            options: sign::SignOptions::default(),
        }
    }

    #[test]
    fn sign_comes_last_and_never_with_encrypt() {
        let encrypt = Step::Encrypt {
            user_password: "u".into(),
            owner_password: "o".into(),
        };
        assert!(check_steps(&[Step::Flatten, sign_step()]).is_ok());
        assert!(check_steps(&[Step::Flatten, encrypt.clone()]).is_ok());
        let err = check_steps(&[sign_step(), Step::Flatten]).unwrap_err();
        assert!(err.is_client_error());
        assert!(err.to_string().contains("last step"), "{err}");
        for steps in [
            vec![encrypt.clone(), sign_step()],
            vec![sign_step(), encrypt.clone()],
        ] {
            let err = check_steps(&steps).unwrap_err();
            assert!(err.to_string().contains("cannot be combined"), "{err}");
        }
    }

    #[tokio::test]
    async fn sign_without_a_key_is_refused_before_any_work() {
        let mut req =
            RenderRequest::new(Document::new("in.pdf", crate::pdfops::fixtures::pages(1)));
        req.steps = vec![sign_step()];
        let err = pipeline().run(req).await.unwrap_err();
        assert!(matches!(err, Error::Status(503, _)), "{err}");
    }

    #[tokio::test]
    async fn underlay_lays_the_document_over_the_base_page_by_page() {
        // One rendered page over a two-page letterhead: the result keeps
        // both base pages and draws the document over the first.
        let mut req = RenderRequest::new(Document::new(
            "letter.pdf",
            crate::pdfops::fixtures::pages(1),
        ));
        req.templating = false;
        req.steps = vec![Step::Underlay {
            base: crate::pdfops::fixtures::pages(2),
            name: "letterhead.pdf".into(),
        }];
        let out = pipeline().run(req).await.unwrap();
        assert_eq!(out.name, "letter.pdf");
        let doc = lopdf::Document::load_mem(&out.bytes).unwrap();
        let pages = doc.get_pages();
        assert_eq!(pages.len(), 2);
        let first = String::from_utf8_lossy(&doc.get_page_content(pages[&1])).into_owned();
        assert!(first.contains("/RdOv1 Do"), "{first}");
        let second = String::from_utf8_lossy(&doc.get_page_content(pages[&2])).into_owned();
        assert!(!second.contains(" Do"), "{second}");

        // A document that is not a PDF yet is refused with what to do.
        let mut req = RenderRequest::new(Document::new("letter.txt", b"Hello".to_vec()));
        req.templating = false;
        req.steps = vec![Step::Underlay {
            base: crate::pdfops::fixtures::pages(1),
            name: "letterhead.pdf".into(),
        }];
        let err = pipeline().run(req).await.unwrap_err();
        assert!(err.to_string().contains("put 'convert' before it"), "{err}");

        // So is a base that is not a PDF.
        let mut req = RenderRequest::new(Document::new(
            "letter.pdf",
            crate::pdfops::fixtures::pages(1),
        ));
        req.templating = false;
        req.steps = vec![Step::Underlay {
            base: b"not a pdf".to_vec(),
            name: "letterhead.png".into(),
        }];
        let err = pipeline().run(req).await.unwrap_err();
        assert!(err.to_string().contains("letterhead.png"), "{err}");
    }

    #[tokio::test]
    async fn sign_as_a_step_appends_a_signature() {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let cert = rcgen::CertificateParams::new(vec!["signer.test".to_string()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        let signer =
            Signer::from_pem(key.serialize_pem().as_bytes(), cert.pem().as_bytes()).unwrap();
        let engines = Engines {
            signer: Some(Arc::new(signer)),
            ..Engines::default()
        };
        let original = crate::pdfops::fixtures::pages(1);
        let mut req = RenderRequest::new(Document::new("in.pdf", original.clone()));
        req.templating = false;
        req.steps = vec![sign_step()];
        let out = Pipeline::new(Arc::new(engines), Duration::from_secs(5))
            .run(req)
            .await
            .unwrap();
        assert!(out.bytes.starts_with(&original));
        assert!(out.bytes.windows(19).any(|w| w == b"ETSI.CAdES.detached"));
    }
}
