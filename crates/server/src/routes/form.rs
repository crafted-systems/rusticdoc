//! Reading a multipart form into documents and fields, fetching
//! `downloadFrom` URLs, and writing a document back as a download.

use std::collections::BTreeMap;

use axum::body::Body;
use axum::extract::Multipart;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use rusticdoc_core::{Document, Output};
use serde::Deserialize;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::webhook;

/// File parts under these field names are not inputs to convert; routes
/// read them from [`Form::by_field`].
const SPECIAL_FIELDS: &[&str] = &[
    "embeds",
    "watermark",
    "stamp",
    "facturx",
    "facturX",
    "facturxXml",
    "fonts",
];

#[derive(Debug, Default)]
pub struct Form {
    /// Every file part that is an input, in order.
    pub files: Vec<Document>,
    /// Every file part by its field name, including the inputs.
    pub by_field: BTreeMap<String, Vec<Document>>,
    /// Every text part, last value wins.
    pub fields: BTreeMap<String, String>,
    /// Every value of every text part, in order, for fields that repeat.
    pub multi: BTreeMap<String, Vec<String>>,
}

/// One entry of Gotenberg's `downloadFrom` field.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadFrom {
    url: String,
    #[serde(default)]
    extra_http_headers: BTreeMap<String, String>,
    /// Routes the file to a field bucket: `embeds`, `watermark`, `stamp`.
    #[serde(default)]
    field: String,
    #[serde(default)]
    embedded: bool,
}

impl Form {
    pub async fn read(mut multipart: Multipart, state: &AppState) -> ApiResult<Self> {
        let mut form = Form::default();
        while let Some(field) = multipart.next_field().await? {
            let name = field.name().unwrap_or("").to_string();
            match field.file_name().map(|f| f.to_string()) {
                Some(file_name) if !file_name.is_empty() => {
                    let bytes = field.bytes().await?;
                    form.add_file(&name, Document::new(sanitize(&file_name), bytes));
                }
                _ => {
                    let text = field.text().await?;
                    form.multi
                        .entry(name.clone())
                        .or_default()
                        .push(text.clone());
                    form.fields.insert(name, text);
                }
            }
        }
        if let Some(raw) = form.fields.remove("downloadFrom") {
            form.download_from(&raw, state).await?;
        }
        Ok(form)
    }

    fn add_file(&mut self, field: &str, doc: Document) {
        if !SPECIAL_FIELDS.contains(&field) {
            self.files.push(doc.clone());
        }
        self.by_field
            .entry(field.to_string())
            .or_default()
            .push(doc);
    }

    /// Fetches each `downloadFrom` entry through the same vetting as
    /// webhooks: resolved up front, private addresses refused unless
    /// allowed, pinned to the vetted addresses, no redirects, and capped at
    /// the body limit.
    async fn download_from(&mut self, raw: &str, state: &AppState) -> ApiResult<()> {
        let config = &state.config;
        if config.download_from_disabled {
            return Err(ApiError::bad_request(
                "'downloadFrom' is disabled on this service",
            ));
        }
        let entries: Vec<DownloadFrom> = serde_json::from_str(raw).map_err(|e| {
            ApiError::bad_request(format!(
                "'downloadFrom' is not a JSON array of {{url}}: {e}"
            ))
        })?;
        if entries.len() > config.download_from_max_entries {
            return Err(ApiError::bad_request(format!(
                "'downloadFrom' has {} entries; the limit is {}",
                entries.len(),
                config.download_from_max_entries
            )));
        }
        // One budget for every download in the request, spent as bytes
        // arrive, so a response cannot be read past it.
        let mut budget = config.body_limit;
        for entry in entries {
            let target = webhook::resolve(&entry.url, config.download_from_allow_private).await?;
            let shown = webhook::redact(&entry.url);
            let client = webhook::client_for(&target, config.api_timeout).map_err(|e| {
                ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "io_error", e.to_string())
            })?;
            let mut req = client.get(&target.url);
            for (k, v) in &entry.extra_http_headers {
                if webhook::is_reserved_header(k) || k.eq_ignore_ascii_case("content-disposition") {
                    return Err(ApiError::bad_request(format!(
                        "'downloadFrom' header '{k}' cannot be set"
                    )));
                }
                req = req.header(k, v);
            }
            let mut res = req
                .send()
                .await
                .map_err(|e| ApiError::bad_request(format!("download '{shown}': {e}")))?;
            if !res.status().is_success() {
                return Err(ApiError::bad_request(format!(
                    "download '{shown}': status {}",
                    res.status().as_u16()
                )));
            }
            if res.content_length().unwrap_or(0) as usize > budget {
                return Err(ApiError::bad_request(format!(
                    "download '{shown}' is larger than the body limit"
                )));
            }
            let name = res
                .headers()
                .get(header::CONTENT_DISPOSITION)
                .and_then(|v| v.to_str().ok())
                .and_then(disposition_filename)
                .or_else(|| {
                    reqwest::Url::parse(&target.url)
                        .ok()
                        .and_then(|u| {
                            u.path_segments()
                                .and_then(|mut s| s.next_back().map(str::to_string))
                        })
                        .filter(|s| !s.is_empty())
                })
                .unwrap_or_else(|| "download".to_string());
            let mut bytes = Vec::new();
            while let Some(chunk) = res
                .chunk()
                .await
                .map_err(|e| ApiError::bad_request(format!("download '{shown}': {e}")))?
            {
                if chunk.len() > budget {
                    return Err(ApiError::bad_request(format!(
                        "download '{shown}' is larger than the body limit"
                    )));
                }
                budget -= chunk.len();
                bytes.extend_from_slice(&chunk);
            }
            let field = if entry.embedded || entry.field == "embedded" {
                "embeds".to_string()
            } else if entry.field.is_empty() {
                "files".to_string()
            } else {
                entry.field.clone()
            };
            self.add_file(&field, Document::new(sanitize(&name), bytes));
        }
        Ok(())
    }

    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }

    /// Every value a repeated text field was sent with.
    pub fn all(&self, name: &str) -> Vec<String> {
        self.multi.get(name).cloned().unwrap_or_default()
    }

    /// A field holding JSON, or an empty object when absent. A file part
    /// named like the field, such as `data.json`, is accepted too.
    pub fn json(&mut self, name: &str) -> ApiResult<serde_json::Value> {
        if let Some(text) = self.fields.get(name) {
            return serde_json::from_str(text).map_err(|e| {
                ApiError::bad_request(format!("field '{name}' is not valid JSON: {e}"))
            });
        }
        if let Some(pos) = self
            .files
            .iter()
            .position(|f| f.name == format!("{name}.json"))
        {
            let file = self.files.remove(pos);
            return serde_json::from_slice(&file.bytes).map_err(|e| {
                ApiError::bad_request(format!("file '{}' is not valid JSON: {e}", file.name))
            });
        }
        Ok(serde_json::Value::Object(Default::default()))
    }

    pub fn bool(&self, name: &str) -> bool {
        matches!(
            self.field(name)
                .map(|v| v.trim().to_ascii_lowercase())
                .as_deref(),
            Some("true" | "1" | "yes")
        )
    }

    pub fn require_file(&mut self, what: &str) -> ApiResult<Document> {
        if self.files.is_empty() {
            return Err(ApiError::bad_request(format!(
                "no file in the form; send {what}"
            )));
        }
        Ok(self.files.remove(0))
    }

    /// File parts sent under a field name such as `embeds`.
    pub fn take_field(&mut self, field: &str) -> Vec<Document> {
        self.by_field.remove(field).unwrap_or_default()
    }
}

fn disposition_filename(value: &str) -> Option<String> {
    value.split(';').map(str::trim).find_map(|part| {
        part.strip_prefix("filename=")
            .map(|f| f.trim_matches('"').to_string())
            .filter(|f| !f.is_empty())
    })
}

/// Keeps the base name only, without control characters, so a client cannot
/// steer a path or a log line.
pub fn sanitize(name: &str) -> String {
    let base: String = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    let base = base.trim();
    if base.is_empty() || base == "." || base == ".." {
        "upload".to_string()
    } else {
        base.to_string()
    }
}

/// `attachment; filename="..."`, with the RFC 5987 form for a name a quoted
/// string cannot carry.
pub fn disposition(name: &str) -> String {
    let clean: String = name
        .chars()
        .filter(|c| !c.is_control() && *c != '"' && *c != '\\')
        .collect();
    if clean.is_ascii() {
        return format!("attachment; filename=\"{clean}\"");
    }
    let plain: String = clean
        .chars()
        .map(|c| if c.is_ascii() { c } else { '_' })
        .collect();
    let mut encoded = String::new();
    for byte in clean.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    format!("attachment; filename=\"{plain}\"; filename*=UTF-8''{encoded}")
}

/// The request id the middleware assigned.
pub fn request_id(headers: &HeaderMap) -> String {
    headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-")
        .to_string()
}

/// A file download with the pipeline timings in a header.
pub fn download(out: Output) -> Response {
    // Bytes are cheap to clone. Very large files skip the font scan.
    let pdf_head = if out.bytes.len() <= 32 * 1024 * 1024 {
        out.bytes.clone()
    } else {
        bytes::Bytes::new()
    };
    let mut res = Response::new(Body::from(out.bytes));
    *res.status_mut() = StatusCode::OK;
    let headers = res.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(out.content_type),
    );
    if let Ok(v) = HeaderValue::from_str(&disposition(&out.name)) {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    if out.content_type == "application/pdf" {
        // Font names are printable ASCII; anything else skips the header.
        let fonts = rusticdoc_core::fonts::pdf_font_names(&pdf_head);
        if !fonts.is_empty()
            && let Ok(v) = HeaderValue::from_str(&fonts.join(", "))
        {
            headers.insert("x-fonts-used", v);
        }
    }
    let timings: Vec<String> = out
        .timings
        .iter()
        .map(|(s, ms)| format!("{s};dur={ms}"))
        .collect();
    if let Ok(v) = HeaderValue::from_str(&timings.join(", ")) {
        headers.insert("server-timing", v);
    }
    res
}

/// Several outputs as one ZIP, as Gotenberg returns them.
pub fn zip(name: &str, outputs: &[Output]) -> ApiResult<Output> {
    use std::io::Write as _;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for out in outputs {
            zw.start_file(out.name.clone(), opts).map_err(|e| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "zip_failed",
                    e.to_string(),
                )
            })?;
            zw.write_all(&out.bytes).map_err(|e| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "zip_failed",
                    e.to_string(),
                )
            })?;
        }
        zw.finish().map_err(|e| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "zip_failed",
                e.to_string(),
            )
        })?;
    }
    Ok(Output {
        name: format!("{name}.zip"),
        content_type: "application/zip",
        bytes: buf.into_inner().into(),
        timings: outputs.iter().flat_map(|o| o.timings.clone()).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_lose_their_directories() {
        assert_eq!(sanitize("../../etc/passwd"), "passwd");
        assert_eq!(sanitize("C:\\Users\\x\\report.docx"), "report.docx");
        assert_eq!(sanitize(".."), "upload");
    }

    #[test]
    fn control_characters_go_from_names() {
        assert_eq!(sanitize("a\u{7}b\n.pdf"), "ab.pdf");
        assert_eq!(sanitize("\u{1b}"), "upload");
    }

    #[test]
    fn dispositions_carry_any_name() {
        assert_eq!(disposition("a b.pdf"), "attachment; filename=\"a b.pdf\"");
        assert_eq!(disposition("a\"b\n.pdf"), "attachment; filename=\"ab.pdf\"");
        let unicode = disposition("ß.pdf");
        assert!(unicode.contains("filename=\"_.pdf\""), "{unicode}");
        assert!(unicode.contains("UTF-8''%C3%9F.pdf"), "{unicode}");
        assert!(HeaderValue::from_str(&unicode).is_ok());
    }

    #[test]
    fn disposition_names() {
        assert_eq!(
            disposition_filename("attachment; filename=\"a b.pdf\""),
            Some("a b.pdf".into())
        );
        assert_eq!(disposition_filename("inline"), None);
    }
}
