//! `rusticdoc pdf ...`: the PDF operations from the command line, through
//! the same engines the service uses.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use clap::{Args, Subcommand};
use rusticdoc_core::pdfform;
use rusticdoc_core::pdfops::{
    Attachment, Bookmark, FacturxProfile, Metadata, MultiEngine, Overlay, OverlayKind, Position,
    Rotation, SplitMode, sign,
};
use rusticdoc_core::{Config, Document, Engines};

#[derive(Args)]
pub struct PdfArgs {
    #[command(subcommand)]
    pub op: PdfOp,
}

#[derive(Subcommand)]
pub enum PdfOp {
    /// Merge PDFs in the order given.
    Merge {
        files: Vec<PathBuf>,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Split by page ranges (`1-3,5`) or into intervals of N pages.
    Split {
        file: PathBuf,
        /// `pages` or `intervals`.
        #[arg(long, default_value = "pages")]
        mode: String,
        /// Ranges for `pages`, a page count for `intervals`.
        #[arg(long)]
        span: String,
        /// One file holding every range instead of one per range.
        #[arg(long)]
        unify: bool,
        /// Directory for the parts.
        #[arg(short, long, default_value = ".")]
        output: PathBuf,
    },
    /// Rotate pages by 90, 180 or 270 degrees.
    Rotate {
        file: PathBuf,
        #[arg(long, default_value = "90")]
        angle: String,
        /// Page selection such as `1-3,5`; all when empty.
        #[arg(long, default_value = "")]
        pages: String,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Flatten forms and annotations into the page content.
    Flatten {
        file: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Recompress streams and drop unused objects.
    Optimize {
        file: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Encrypt with AES-256.
    Encrypt {
        file: PathBuf,
        #[arg(long, default_value = "")]
        user_password: String,
        #[arg(long)]
        owner_password: Option<String>,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Read metadata as JSON, or write keys from a JSON object.
    Metadata {
        file: PathBuf,
        /// JSON object to write, such as `{"Title":"Invoice"}`.
        #[arg(long)]
        set: Option<String>,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Read the outline as JSON, or replace it from a JSON array.
    Bookmarks {
        file: PathBuf,
        /// JSON array of `{title, page, children}`.
        #[arg(long)]
        set: Option<String>,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Attach files to a PDF.
    Embed {
        file: PathBuf,
        attachments: Vec<PathBuf>,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Put text, an image or a PDF under the page content.
    Watermark {
        file: PathBuf,
        #[command(flatten)]
        mark: MarkArgs,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Put text, an image or a PDF over the page content.
    Stamp {
        file: PathBuf,
        #[command(flatten)]
        mark: MarkArgs,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Sign cryptographically with --key and --cert (PAdES B-B to B-LTA,
    /// invisible unless --image or --x and --y are given), or, without them,
    /// place a signature image at exact coordinates, which is not a
    /// cryptographic signature.
    ///
    /// --level b-t and above take the timestamp authority from --tsa-url or
    /// RUSTICDOC_SIGNING_TSA_*, and every request (timestamps, OCSP, CRLs)
    /// follows RUSTICDOC_SIGNING_OUTBOUND_*: private addresses are refused
    /// unless RUSTICDOC_SIGNING_OUTBOUND_ALLOW_PRIVATE=true.
    Sign {
        file: PathBuf,
        /// PEM private key (PKCS#8, PKCS#1 or SEC1; RSA or ECDSA P-256), or
        /// `pkcs11:<label>` for a key on the token RUSTICDOC_PKCS11_MODULE,
        /// _TOKEN or _SLOT, and _PIN_FILE or _PIN describe.
        #[arg(long, requires = "cert")]
        key: Option<PathBuf>,
        /// PEM certificate for --key, then any chain certificates.
        #[arg(long, requires = "key")]
        cert: Option<PathBuf>,
        /// PNG or JPEG. With --key, the visible appearance of the signature.
        #[arg(long, requires_all = ["x", "y"])]
        image: Option<PathBuf>,
        #[arg(long, default_value_t = 1)]
        page: u32,
        /// Points from the page's left edge. With --key and no --image, the
        /// signature is shown there as text: signer, date and reason.
        #[arg(long, requires = "y")]
        x: Option<f32>,
        /// Points from the page's bottom edge.
        #[arg(long, requires = "x")]
        y: Option<f32>,
        /// Width in points; default the image's natural size, or 200 for text.
        #[arg(long, requires = "x")]
        width: Option<f32>,
        /// PAdES level (with --key): b-b, b-t (RFC 3161 timestamp), b-lt
        /// (and OCSP or CRL validation data in a DSS) or b-lta (and a document
        /// timestamp). Default RUSTICDOC_SIGNING_LEVEL, or b-b.
        #[arg(long, requires = "key")]
        level: Option<String>,
        /// Certify rather than approve (with --key), with DocMDP permission 1
        /// (no changes), 2 (form filling and signing) or 3 (also annotations).
        #[arg(long, requires = "key")]
        certify: Option<String>,
        /// The RFC 3161 timestamp authority (with --key), instead of
        /// RUSTICDOC_SIGNING_TSA_URL. https, unless
        /// RUSTICDOC_SIGNING_TSA_ALLOW_HTTP=true.
        #[arg(long, requires_all = ["key", "tsa_cert"])]
        tsa_url: Option<String>,
        /// PEM certificate the --tsa-url authority signs with, or the CA it
        /// chains to, instead of RUSTICDOC_SIGNING_TSA_CERT. Tokens from any
        /// other signer are refused.
        #[arg(long, requires = "tsa_url")]
        tsa_cert: Option<PathBuf>,
        /// Why the document was signed (with --key).
        #[arg(long, requires = "key")]
        reason: Option<String>,
        /// Where it was signed (with --key).
        #[arg(long, requires = "key")]
        location: Option<String>,
        /// How to reach the signer (with --key).
        #[arg(long, requires = "key")]
        contact_info: Option<String>,
        /// The signer's name as shown; validators read the certificate (with --key).
        #[arg(long, requires = "key")]
        signer_name: Option<String>,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Embed Factur-X invoice XML.
    Facturx {
        file: PathBuf,
        #[arg(long)]
        xml: PathBuf,
        #[arg(long, default_value = "EN16931")]
        profile: String,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// List a form's fields as JSON.
    Fields { file: PathBuf },
    /// Fill a form from JSON data; tags in annotations are applied too.
    Fill {
        file: PathBuf,
        #[arg(short, long)]
        data: PathBuf,
        #[arg(long)]
        flatten: bool,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Lay one PDF over another, page by page.
    Overlay {
        base: PathBuf,
        overlay: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
    },
}

#[derive(Args)]
pub struct MarkArgs {
    /// Text for a text mark.
    #[arg(long)]
    text: Option<String>,
    /// PNG, JPEG or PDF file for an image or PDF mark.
    #[arg(long = "mark")]
    mark_file: Option<PathBuf>,
    /// top-left, top-center, top-right, center, bottom-left, bottom-center, bottom-right.
    #[arg(long, default_value = "center")]
    position: String,
    #[arg(long, default_value_t = 1.0)]
    opacity: f32,
    #[arg(long, default_value_t = 0.0)]
    rotation: f32,
    /// Relative to the page width, 0 to 1.
    #[arg(long, default_value_t = 0.5)]
    scale: f32,
    #[arg(long, default_value_t = 24.0)]
    font_size: f32,
    #[arg(long, default_value = "#808080")]
    colour: String,
    #[arg(long, default_value = "")]
    pages: String,
}

impl MarkArgs {
    fn overlay(&self) -> anyhow::Result<Overlay> {
        let kind = match (&self.text, &self.mark_file) {
            (Some(text), _) => OverlayKind::Text {
                text: text.clone(),
                font_size: self.font_size,
                colour: self.colour.clone(),
            },
            (None, Some(path)) => {
                let doc = read(path)?;
                match doc.extension().as_str() {
                    "pdf" => OverlayKind::Pdf {
                        bytes: doc.bytes.to_vec(),
                        name: doc.name,
                    },
                    "png" | "jpg" | "jpeg" => OverlayKind::Image {
                        bytes: doc.bytes.to_vec(),
                        name: doc.name,
                    },
                    other => bail!("a mark file must be PNG, JPEG or PDF, not .{other}"),
                }
            }
            (None, None) => bail!("give --text, or --mark with an image or PDF"),
        };
        let position = match self.position.as_str() {
            "top-left" => Position::TopLeft,
            "top-center" => Position::TopCenter,
            "top-right" => Position::TopRight,
            "center" => Position::Center,
            "bottom-left" => Position::BottomLeft,
            "bottom-center" => Position::BottomCenter,
            "bottom-right" => Position::BottomRight,
            other => bail!("unknown position '{other}'"),
        };
        Ok(Overlay {
            placement: None,
            kind,
            position,
            opacity: self.opacity.clamp(0.0, 1.0),
            rotation: self.rotation,
            scale: self.scale.clamp(0.01, 1.0),
            pages: self.pages.clone(),
        })
    }
}

fn read(path: &std::path::Path) -> anyhow::Result<Document> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("document")
        .to_string();
    Ok(Document::new(name, bytes))
}

fn write(path: &std::path::Path, doc: &Document) -> anyhow::Result<()> {
    std::fs::write(path, &doc.bytes).with_context(|| format!("write {}", path.display()))?;
    eprintln!("wrote {} ({} bytes)", path.display(), doc.bytes.len());
    Ok(())
}

pub async fn run(config: &Config, args: PdfArgs) -> anyhow::Result<()> {
    let engines = Engines::from_config(config);
    let pdf: &MultiEngine = &engines.pdf;
    let t: Duration = config.api_timeout;
    match args.op {
        PdfOp::Merge { files, output } => {
            let docs = files
                .iter()
                .map(|f| read(f))
                .collect::<anyhow::Result<Vec<_>>>()?;
            write(&output, &pdf.merge(&docs, t).await?)
        }
        PdfOp::Split {
            file,
            mode,
            span,
            unify,
            output,
        } => {
            let mode = match mode.as_str() {
                "pages" => SplitMode::Pages {
                    ranges: span,
                    unify,
                },
                "intervals" => SplitMode::Intervals {
                    span: span.parse().context("--span must be a page count")?,
                },
                other => bail!("--mode must be pages or intervals, not '{other}'"),
            };
            let doc = read(&file)?;
            let stem = file
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("part")
                .to_string();
            std::fs::create_dir_all(&output)?;
            for (i, part) in pdf.split(&doc, &mode, t).await?.iter().enumerate() {
                write(&output.join(format!("{stem}_{}.pdf", i + 1)), part)?;
            }
            Ok(())
        }
        PdfOp::Rotate {
            file,
            angle,
            pages,
            output,
        } => write(
            &output,
            &pdf.rotate(&read(&file)?, Rotation::parse(&angle)?, &pages, t)
                .await?,
        ),
        PdfOp::Flatten { file, output } => write(&output, &pdf.flatten(&read(&file)?, t).await?),
        PdfOp::Optimize { file, output } => write(&output, &pdf.optimize(&read(&file)?, t).await?),
        PdfOp::Encrypt {
            file,
            user_password,
            owner_password,
            output,
        } => {
            let owner = owner_password.unwrap_or_else(|| user_password.clone());
            write(
                &output,
                &pdf.encrypt(&read(&file)?, &user_password, &owner, t)
                    .await?,
            )
        }
        PdfOp::Metadata { file, set, output } => match set {
            None => {
                let meta = pdf.read_metadata(&read(&file)?, t).await?;
                println!("{}", serde_json::to_string_pretty(&meta)?);
                Ok(())
            }
            Some(json) => {
                let map: serde_json::Map<String, serde_json::Value> =
                    serde_json::from_str(&json).context("--set must be a JSON object")?;
                let meta: Metadata = map.into_iter().collect();
                let out = output.unwrap_or_else(|| file.clone());
                write(&out, &pdf.write_metadata(&read(&file)?, &meta, t).await?)
            }
        },
        PdfOp::Bookmarks { file, set, output } => match set {
            None => {
                let marks = pdf.read_bookmarks(&read(&file)?, t).await?;
                println!("{}", serde_json::to_string_pretty(&marks)?);
                Ok(())
            }
            Some(json) => {
                let marks: Vec<Bookmark> =
                    serde_json::from_str(&json).context("--set must be a JSON array")?;
                let out = output.unwrap_or_else(|| file.clone());
                write(&out, &pdf.write_bookmarks(&read(&file)?, &marks, t).await?)
            }
        },
        PdfOp::Embed {
            file,
            attachments,
            output,
        } => {
            let files = attachments
                .iter()
                .map(|p| {
                    read(p).map(|d| Attachment {
                        mime: d.content_type().to_string(),
                        name: d.name,
                        bytes: d.bytes.to_vec(),
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            write(&output, &pdf.embed(&read(&file)?, &files, t).await?)
        }
        PdfOp::Watermark { file, mark, output } => write(
            &output,
            &pdf.watermark(&read(&file)?, &mark.overlay()?, t).await?,
        ),
        PdfOp::Stamp { file, mark, output } => write(
            &output,
            &pdf.stamp(&read(&file)?, &mark.overlay()?, t).await?,
        ),
        PdfOp::Sign {
            file,
            key,
            cert,
            image,
            page,
            x,
            y,
            width,
            reason,
            location,
            contact_info,
            signer_name,
            level,
            certify,
            tsa_url,
            tsa_cert,
            output,
        } => {
            if let (Some(key), Some(cert)) = (key, cert) {
                let signer = sign::load_key(&key.to_string_lossy(), &cert, &config.signing.pkcs11)
                    .map_err(anyhow::Error::msg)?;
                for warning in signer.warnings() {
                    eprintln!("warning: {warning}");
                }
                let appearance = match (image, x, y) {
                    (image, Some(x), Some(y)) => {
                        let mark = image.map(|path| read(&path)).transpose()?;
                        Some(sign::Appearance {
                            page,
                            x,
                            y,
                            width,
                            name: mark
                                .as_ref()
                                .map_or_else(|| "text".to_string(), |m| m.name.clone()),
                            image: mark.map(|m| m.bytes.to_vec()),
                        })
                    }
                    _ => None,
                };
                let options = sign::SignOptions {
                    reason,
                    location,
                    contact_info,
                    signer_name,
                    appearance,
                    level: level.as_deref().map(sign::Level::parse).transpose()?,
                    certify: certify.as_deref().map(sign::DocMdp::parse).transpose()?,
                    key: None,
                };
                let mut signing = sign::Signing::with_default(signer)
                    .with_level(config.signing.level)
                    .with_limits(config.signing.revocation.clone());
                let tsa = match tsa_url {
                    Some(url) => {
                        sign::check_tsa_url(&url, config.signing.tsa_allow_http)
                            .map_err(anyhow::Error::msg)?;
                        let mut tsa = sign::Tsa::new(url, std::time::Duration::from_secs(10));
                        tsa.cert_file = tsa_cert;
                        Some(tsa)
                    }
                    None => config.signing.tsa.clone(),
                };
                if let Some(mut tsa) = tsa {
                    tsa.load_anchors().map_err(anyhow::Error::msg)?;
                    signing = signing.with_tsa(tsa);
                }
                signing.set_outbound(Arc::new(
                    rusticdoc_server::signing::PolicyOutbound::from_config(config)
                        .map_err(anyhow::Error::msg)?,
                ));
                let signed = signing.sign(&read(&file)?, &options, t).await?;
                return write(&output, &signed);
            }
            let (Some(image), Some(x), Some(y)) = (image, x, y) else {
                bail!(
                    "give --key and --cert for a cryptographic signature, or --image, --x and --y to place an image"
                );
            };
            let mark = read(&image)?;
            let overlay = Overlay {
                placement: Some(rusticdoc_core::pdfops::Placement { x, y, width }),
                kind: OverlayKind::Image {
                    bytes: mark.bytes.to_vec(),
                    name: mark.name,
                },
                position: Position::Center,
                opacity: 1.0,
                rotation: 0.0,
                scale: 0.0,
                pages: page.to_string(),
            };
            write(&output, &pdf.stamp(&read(&file)?, &overlay, t).await?)
        }
        PdfOp::Facturx {
            file,
            xml,
            profile,
            output,
        } => {
            let xml = std::fs::read(&xml).with_context(|| format!("read {}", xml.display()))?;
            write(
                &output,
                &pdf.facturx(&read(&file)?, &xml, FacturxProfile::parse(&profile)?, t)
                    .await?,
            )
        }
        PdfOp::Fields { file } => {
            let fields = pdfform::fields(&read(&file)?.bytes)?;
            println!("{}", serde_json::to_string_pretty(&fields)?);
            Ok(())
        }
        PdfOp::Fill {
            file,
            data,
            flatten,
            output,
        } => {
            let text = std::fs::read_to_string(&data)
                .with_context(|| format!("read {}", data.display()))?;
            let data: serde_json::Value =
                serde_json::from_str(&text).context("data is not valid JSON")?;
            let env = rusticdoc_core::template::environment();
            let options = pdfform::FillOptions {
                flatten,
                strip_tag_annotations: true,
            };
            let bytes = pdfform::fill(&read(&file)?.bytes, &data, &env, &options)?;
            write(&output, &Document::new("filled.pdf", bytes))
        }
        PdfOp::Overlay {
            base,
            overlay,
            output,
        } => {
            let bytes = pdfform::overlay(&read(&base)?.bytes, &read(&overlay)?.bytes)?;
            write(&output, &Document::new("overlaid.pdf", bytes))
        }
    }
}
