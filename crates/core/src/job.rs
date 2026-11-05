use std::collections::BTreeMap;
use std::path::Path;

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::convert::ConvertOptions;
use crate::error::{Error, Result};
use crate::pdfops;

/// A file in flight: its name decides its format.
#[derive(Debug, Clone)]
pub struct Document {
    pub name: String,
    pub bytes: Bytes,
}

impl Document {
    pub fn new(name: impl Into<String>, bytes: impl Into<Bytes>) -> Self {
        Self {
            name: name.into(),
            bytes: bytes.into(),
        }
    }

    /// Lower-case extension without the dot, or empty.
    pub fn extension(&self) -> String {
        Path::new(&self.name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default()
    }

    /// The name with its extension swapped.
    pub fn with_extension(&self, ext: &str) -> String {
        let stem = Path::new(&self.name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document");
        format!("{stem}.{ext}")
    }

    pub fn content_type(&self) -> &'static str {
        content_type_for(&self.extension())
    }
}

pub fn content_type_for(ext: &str) -> &'static str {
    match ext {
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "html" | "htm" => "text/html; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "json" => "application/json",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// Output formats the convert step accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Pdf,
    Docx,
    Xlsx,
    Pptx,
    Odt,
    Ods,
    Odp,
    Html,
    Txt,
}

impl Format {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "pdf" => Format::Pdf,
            "docx" => Format::Docx,
            "xlsx" => Format::Xlsx,
            "pptx" => Format::Pptx,
            "odt" => Format::Odt,
            "ods" => Format::Ods,
            "odp" => Format::Odp,
            "html" => Format::Html,
            "txt" => Format::Txt,
            other => {
                return Err(Error::InvalidRequest(format!(
                    "unknown output format '{other}'"
                )));
            }
        })
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Pdf => "pdf",
            Format::Docx => "docx",
            Format::Xlsx => "xlsx",
            Format::Pptx => "pptx",
            Format::Odt => "odt",
            Format::Ods => "ods",
            Format::Odp => "odp",
            Format::Html => "html",
            Format::Txt => "txt",
        }
    }
}

/// PDF/A conformance levels LibreOffice can export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PdfaLevel {
    #[serde(rename = "PDF/A-1b")]
    A1b,
    #[serde(rename = "PDF/A-2b")]
    A2b,
    #[serde(rename = "PDF/A-3b")]
    A3b,
    #[serde(rename = "PDF/A-4")]
    A4,
}

impl PdfaLevel {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s.trim().to_ascii_uppercase().as_str() {
            "PDF/A-1B" | "1B" | "A-1B" => PdfaLevel::A1b,
            "PDF/A-2B" | "2B" | "A-2B" => PdfaLevel::A2b,
            "PDF/A-3B" | "3B" | "A-3B" => PdfaLevel::A3b,
            "PDF/A-4" | "4" | "A-4" => PdfaLevel::A4,
            other => {
                return Err(Error::InvalidRequest(format!(
                    "unknown PDF/A level '{other}'"
                )));
            }
        })
    }

    /// LibreOffice's `SelectPdfVersion` value.
    pub fn libreoffice_version(self) -> u32 {
        match self {
            PdfaLevel::A1b => 1,
            PdfaLevel::A2b => 2,
            PdfaLevel::A3b => 3,
            PdfaLevel::A4 => 4,
        }
    }
}

/// One stage of a pipeline, in the order the client lists them.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Convert with LibreOffice, using the request's conversion options.
    Convert {
        to: Format,
    },
    /// Export as PDF/A at the given level. Implies a conversion.
    Pdfa {
        level: PdfaLevel,
    },
    /// Export as PDF/UA. Implies a conversion.
    Pdfua,
    /// Merge the extra documents of the request after the main one, or
    /// every document in flight when a split came before.
    Merge,
    /// Split into several documents; every later step runs on each.
    Split {
        mode: pdfops::SplitMode,
    },
    /// Text, an image or a PDF under the page content.
    Watermark {
        overlay: pdfops::Overlay,
    },
    /// Text, an image or a PDF over the page content.
    Stamp {
        overlay: pdfops::Overlay,
    },
    Rotate {
        rotation: pdfops::Rotation,
        pages: String,
    },
    Flatten,
    Optimize,
    /// Replace the outline.
    Bookmarks {
        bookmarks: Vec<pdfops::Bookmark>,
    },
    /// Write document metadata.
    Metadata {
        metadata: pdfops::Metadata,
    },
    /// Attach files.
    Embed {
        files: Vec<pdfops::Attachment>,
    },
    /// Embed Factur-X invoice XML.
    Facturx {
        xml: Vec<u8>,
        profile: pdfops::FacturxProfile,
    },
    /// Encrypt with a user and owner password.
    Encrypt {
        user_password: String,
        owner_password: String,
    },
}

impl Step {
    pub fn name(&self) -> &'static str {
        match self {
            Step::Convert { .. } => "convert",
            Step::Pdfa { .. } => "pdfa",
            Step::Pdfua => "pdfua",
            Step::Merge => "merge",
            Step::Split { .. } => "split",
            Step::Watermark { .. } => "watermark",
            Step::Stamp { .. } => "stamp",
            Step::Rotate { .. } => "rotate",
            Step::Flatten => "flatten",
            Step::Optimize => "optimize",
            Step::Bookmarks { .. } => "bookmarks",
            Step::Metadata { .. } => "metadata",
            Step::Embed { .. } => "embed",
            Step::Facturx { .. } => "factur-x",
            Step::Encrypt { .. } => "encrypt",
        }
    }
}

/// Everything a render needs. The server builds this from a multipart form;
/// the CLI from its arguments.
#[derive(Debug, Clone)]
pub struct RenderRequest {
    /// The template, or a document to convert when there is no templating.
    pub template: Document,
    /// Further documents, used by `merge`.
    pub extra: Vec<Document>,
    /// The data the template sees as its root.
    pub data: serde_json::Value,
    /// Request-level context, visible as `ctx`.
    pub ctx: serde_json::Value,
    /// Steps to run after templating, in order.
    pub steps: Vec<Step>,
    /// LibreOffice options for every conversion step.
    pub convert_options: ConvertOptions,
    /// File name for the result, without extension. Defaults to the
    /// template's stem.
    pub output_name: Option<String>,
    /// Run the template stage. Off for plain conversions and PDF
    /// operations, whose inputs are documents, not templates.
    pub templating: bool,
}

impl RenderRequest {
    pub fn new(template: Document) -> Self {
        Self {
            template,
            extra: Vec::new(),
            data: serde_json::Value::Object(Default::default()),
            ctx: serde_json::Value::Object(Default::default()),
            steps: Vec::new(),
            convert_options: ConvertOptions::default(),
            output_name: None,
            templating: true,
        }
    }
}

/// The finished file.
#[derive(Debug, Clone)]
pub struct Output {
    pub name: String,
    pub content_type: &'static str,
    pub bytes: Bytes,
    /// Milliseconds per stage, for logs and metrics.
    pub timings: Vec<(String, u128)>,
}

/// Several documents as one ZIP, deflated.
pub fn zip_documents(name: &str, docs: &[Document]) -> Result<Document> {
    use std::io::Write as _;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for doc in docs {
            zw.start_file(doc.name.clone(), opts)?;
            zw.write_all(&doc.bytes)?;
        }
        zw.finish()?;
    }
    Ok(Document::new(format!("{name}.zip"), buf.into_inner()))
}

impl From<zip::result::ZipError> for Error {
    fn from(e: zip::result::ZipError) -> Self {
        Error::Engine {
            engine: "zip",
            message: e.to_string(),
        }
    }
}

/// Parses the form's `steps` field: a comma-separated list, each item a step
/// name whose parameters come from the other form fields.
pub fn parse_steps(list: &str, fields: &BTreeMap<String, String>) -> Result<Vec<Step>> {
    let mut steps = Vec::new();
    for raw in list.split(',') {
        let name = raw.trim().to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }
        let step = match name.as_str() {
            "convert" => Step::Convert {
                to: Format::parse(fields.get("to").map(String::as_str).unwrap_or("pdf"))?,
            },
            "pdfa" => Step::Pdfa {
                level: PdfaLevel::parse(
                    fields.get("pdfa").map(String::as_str).unwrap_or("PDF/A-2b"),
                )?,
            },
            "pdfua" => Step::Pdfua,
            "metadata" => {
                let raw = fields.get("metadata").ok_or_else(|| {
                    Error::InvalidRequest("step 'metadata' needs a 'metadata' JSON object".into())
                })?;
                let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(raw)
                    .map_err(|e| {
                        Error::InvalidRequest(format!("'metadata' is not a JSON object: {e}"))
                    })?;
                Step::Metadata {
                    metadata: map.into_iter().collect(),
                }
            }
            "encrypt" => {
                let user = fields.get("userPassword").cloned().unwrap_or_default();
                let owner = fields
                    .get("ownerPassword")
                    .cloned()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| user.clone());
                if owner.is_empty() {
                    return Err(Error::InvalidRequest(
                        "step 'encrypt' needs 'userPassword' or 'ownerPassword'".into(),
                    ));
                }
                Step::Encrypt {
                    user_password: user,
                    owner_password: owner,
                }
            }
            "merge" => Step::Merge,
            "flatten" => Step::Flatten,
            "optimize" => Step::Optimize,
            "rotate" => Step::Rotate {
                rotation: pdfops::Rotation::parse(
                    fields
                        .get("rotateAngle")
                        .map(String::as_str)
                        .unwrap_or("90"),
                )?,
                pages: fields.get("rotatePages").cloned().unwrap_or_default(),
            },
            "split" => {
                let span = fields.get("splitSpan").cloned().unwrap_or_default();
                if span.is_empty() {
                    return Err(Error::InvalidRequest(
                        "step 'split' needs 'splitSpan'".into(),
                    ));
                }
                let mode = match fields
                    .get("splitMode")
                    .map(String::as_str)
                    .unwrap_or("pages")
                {
                    "pages" => pdfops::SplitMode::Pages {
                        ranges: span,
                        unify: matches!(
                            fields
                                .get("splitUnify")
                                .map(|s| s.trim().to_ascii_lowercase())
                                .as_deref(),
                            Some("true" | "1" | "yes")
                        ),
                    },
                    "intervals" => pdfops::SplitMode::Intervals {
                        span: span.parse().map_err(|_| {
                            Error::InvalidRequest(
                                "'splitSpan' must be a page count for intervals".into(),
                            )
                        })?,
                    },
                    other => {
                        return Err(Error::InvalidRequest(format!(
                            "'splitMode' must be pages or intervals, not '{other}'"
                        )));
                    }
                };
                Step::Split { mode }
            }
            "bookmarks" => {
                let raw = fields.get("bookmarks").ok_or_else(|| {
                    Error::InvalidRequest("step 'bookmarks' needs a 'bookmarks' JSON array".into())
                })?;
                Step::Bookmarks {
                    bookmarks: serde_json::from_str(raw).map_err(|e| {
                        Error::InvalidRequest(format!("'bookmarks' is not valid: {e}"))
                    })?,
                }
            }
            other => {
                return Err(Error::InvalidRequest(format!(
                    "unknown step '{other}'; file-carrying steps (watermark, stamp, embed, factur-x) take their own fields"
                )));
            }
        };
        steps.push(step);
    }
    Ok(steps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_parse_in_order_with_their_fields() {
        let mut f = BTreeMap::new();
        f.insert("pdfa".to_string(), "PDF/A-3b".to_string());
        f.insert(
            "metadata".to_string(),
            r#"{"Title":"Invoice","Pages":3}"#.to_string(),
        );
        f.insert("ownerPassword".to_string(), "o".to_string());
        let steps = parse_steps("convert, pdfa,metadata,encrypt", &f).unwrap();
        assert_eq!(steps.len(), 4);
        assert_eq!(steps[0], Step::Convert { to: Format::Pdf });
        assert_eq!(
            steps[1],
            Step::Pdfa {
                level: PdfaLevel::A3b
            }
        );
        match &steps[2] {
            Step::Metadata { metadata } => {
                assert_eq!(metadata["Title"], "Invoice");
                assert_eq!(metadata["Pages"], 3);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(steps[3].name(), "encrypt");
    }

    #[test]
    fn unknown_step_is_a_client_error() {
        let err = parse_steps("convert,teleport", &BTreeMap::new()).unwrap_err();
        assert!(err.is_client_error());
        assert!(err.to_string().contains("teleport"));
    }

    #[test]
    fn document_names() {
        let d = Document::new("Report.DOCX", Bytes::new());
        assert_eq!(d.extension(), "docx");
        assert_eq!(d.with_extension("pdf"), "Report.pdf");
        assert!(d.content_type().contains("wordprocessingml"));
    }
}
