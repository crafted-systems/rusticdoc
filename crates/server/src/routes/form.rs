//! Reading a multipart form into documents and fields, fetching
//! `downloadFrom` URLs, and writing a document back as a download.

use std::collections::BTreeMap;
use std::time::Duration;

use axum::body::Body;
use axum::extract::Multipart;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use rusticdoc_core::limits::MAX_TEXT_PARTS;
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
    "underlay",
    "facturx",
    "facturX",
    "facturxXml",
    "fonts",
    "assets",
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

/// One entry of the `downloadFrom` field.
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
        let max_files = state.config.max_form_parts;
        let (mut files, mut texts) = (0usize, 0usize);
        while let Some(field) = multipart.next_field().await? {
            let name = field.name().unwrap_or("").to_string();
            match field.file_name().map(|f| f.to_string()) {
                Some(file_name) if !file_name.is_empty() => {
                    files += 1;
                    if files > max_files {
                        return Err(ApiError::bad_request(format!(
                            "the form has more than {max_files} files"
                        )));
                    }
                    let bytes = field.bytes().await?;
                    form.add_file(&name, Document::new(sanitize(&file_name), bytes));
                }
                _ => {
                    texts += 1;
                    if texts > MAX_TEXT_PARTS {
                        return Err(ApiError::bad_request(format!(
                            "the form has more than {MAX_TEXT_PARTS} fields"
                        )));
                    }
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
            let target = webhook::resolve(&entry.url, &state.url_policies.download).await?;
            let shown = webhook::redact(&entry.url);
            let client = webhook::client_for(
                &target,
                config.api_timeout,
                config.outbound_proxy.as_deref(),
            )
            .map_err(|e| {
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
            let mut res = req.send().await.map_err(|e| {
                let e = e.without_url();
                ApiError::bad_request(format!("download '{shown}': {e}"))
            })?;
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
            while let Some(chunk) = res.chunk().await.map_err(|e| {
                let e = e.without_url();
                ApiError::bad_request(format!("download '{shown}': {e}"))
            })? {
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

    /// The `outputFilename` the client asked for, as a safe base name.
    pub fn output_name(&self) -> Option<String> {
        self.field("outputFilename")
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(sanitize)
    }

    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }

    /// Every value a repeated text field was sent with.
    pub fn all(&self, name: &str) -> Vec<String> {
        self.multi.get(name).cloned().unwrap_or_default()
    }

    /// A field holding JSON, or an empty object when absent. It may come as text
    /// (`-F "data=<f.json"`), as a file part in that field (`-F data=@f.json`), or
    /// as a file part with the field's name in any field (`data.json`). A file part
    /// read here stops being an input, so read these before choosing a template.
    pub fn json(&mut self, name: &str) -> ApiResult<serde_json::Value> {
        let parts = self.by_field.remove(name).unwrap_or_default();
        for part in &parts {
            if let Some(pos) = self
                .files
                .iter()
                .position(|f| f.name == part.name && f.bytes == part.bytes)
            {
                self.files.remove(pos);
            }
        }
        let text = self.fields.get(name);
        if parts.len() + usize::from(text.is_some()) > 1 {
            return Err(ApiError::bad_request(format!(
                "field '{name}' was sent more than once; send it as text or as one file"
            )));
        }
        if let Some(text) = text {
            return serde_json::from_str(text).map_err(|e| {
                ApiError::bad_request(format!("field '{name}' is not valid JSON: {e}"))
            });
        }
        let named = self
            .files
            .iter()
            .position(|f| f.name == format!("{name}.json"));
        let file = match (parts.into_iter().next(), named) {
            (Some(file), _) => file,
            (None, Some(pos)) => self.files.remove(pos),
            (None, None) => return Ok(serde_json::Value::Object(Default::default())),
        };
        serde_json::from_slice(&file.bytes).map_err(|e| {
            ApiError::bad_request(format!(
                "file '{}' sent as '{name}' is not valid JSON: {e}",
                file.name
            ))
        })
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
    rusticdoc_core::job::safe_file_name(name, "upload")
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
pub async fn download(out: Output) -> Response {
    // Bytes are cheap to clone. Very large files skip the font scan, and the rest
    // are scanned off the async threads.
    let fonts = if out.content_type == "application/pdf" && out.bytes.len() <= 32 * 1024 * 1024 {
        let scanned = out.bytes.clone();
        tokio::task::spawn_blocking(move || rusticdoc_core::fonts::pdf_font_names(&scanned))
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
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
    // Font names are printable ASCII; anything else skips the header.
    if !fonts.is_empty()
        && let Ok(v) = HeaderValue::from_str(&fonts.join(", "))
    {
        headers.insert("x-fonts-used", v);
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

/// Several outputs as one ZIP.
pub async fn zip(name: &str, outputs: &[Output], timeout: Duration) -> ApiResult<Output> {
    let docs = outputs
        .iter()
        .map(|out| Document::new(out.name.clone(), out.bytes.clone()))
        .collect();
    let archive = rusticdoc_core::job::zip_off_thread(name, docs, timeout).await?;
    Ok(Output {
        name: archive.name,
        content_type: "application/zip",
        bytes: archive.bytes,
        timings: outputs.iter().flat_map(|o| o.timings.clone()).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form_with(files: &[(&str, &str, &str)]) -> Form {
        let mut form = Form::default();
        for (field, name, body) in files {
            form.add_file(field, Document::new(*name, body.as_bytes().to_vec()));
        }
        form
    }

    #[test]
    fn a_file_in_the_field_is_read_whatever_its_name() {
        let mut form = form_with(&[
            ("data", "invoice.json", r#"{"a":1}"#),
            ("ctx", "more.json", r#"{"b":2}"#),
            ("template", "invoice.docx", "PK"),
        ]);
        assert_eq!(form.json("data").unwrap()["a"], 1);
        assert_eq!(form.json("ctx").unwrap()["b"], 2);
        assert_eq!(form.files.len(), 1, "only the template is left");
        assert_eq!(form.files[0].name, "invoice.docx");
    }

    #[test]
    fn a_file_named_for_the_field_still_counts_in_any_field() {
        let mut form = form_with(&[
            ("files", "data.json", r#"{"a":1}"#),
            ("files", "t.docx", "PK"),
        ]);
        assert_eq!(form.json("data").unwrap()["a"], 1);
        assert_eq!(form.files.len(), 1);
        assert_eq!(form.files[0].name, "t.docx");
    }

    #[test]
    fn an_identical_input_survives_the_data_file() {
        let body = r#"{"a":1}"#;
        let mut form = form_with(&[("files", "a.json", body), ("data", "a.json", body)]);
        form.json("data").unwrap();
        assert_eq!(form.files.len(), 1);
    }

    #[test]
    fn data_that_is_not_json_or_sent_twice_is_refused() {
        let mut form = form_with(&[("data", "invoice.docx", "PK")]);
        let err = form.json("data").unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(
            err.body.message.contains("invoice.docx"),
            "{}",
            err.body.message
        );

        let mut form = form_with(&[("data", "a.json", "{}"), ("data", "b.json", "{}")]);
        assert!(
            form.json("data")
                .unwrap_err()
                .body
                .message
                .contains("more than once")
        );

        let mut form = form_with(&[("data", "a.json", "{}")]);
        form.fields.insert("data".into(), "{}".into());
        assert!(
            form.json("data")
                .unwrap_err()
                .body
                .message
                .contains("more than once")
        );
    }

    #[test]
    fn no_data_is_an_empty_object() {
        let mut form = form_with(&[("files", "report.docx", "PK")]);
        assert_eq!(form.json("data").unwrap(), serde_json::json!({}));
        assert_eq!(form.files.len(), 1);
    }

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
