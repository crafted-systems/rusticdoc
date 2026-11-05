//! Reading a multipart form into documents and fields, and writing a
//! document back as a download.

use std::collections::BTreeMap;

use axum::body::Body;
use axum::extract::Multipart;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use rusticdoc_core::{Document, Output};

use crate::error::{ApiError, ApiResult};

#[derive(Debug, Default)]
pub struct Form {
    /// Every part that came with a file name, in order.
    pub files: Vec<Document>,
    /// Every text part, last value wins.
    pub fields: BTreeMap<String, String>,
}

impl Form {
    pub async fn read(mut multipart: Multipart) -> ApiResult<Self> {
        let mut form = Form::default();
        while let Some(field) = multipart.next_field().await? {
            let name = field.name().unwrap_or("").to_string();
            match field.file_name().map(|f| f.to_string()) {
                Some(file_name) if !file_name.is_empty() => {
                    let bytes = field.bytes().await?;
                    form.files.push(Document::new(sanitize(&file_name), bytes));
                }
                _ => {
                    let text = field.text().await?;
                    form.fields.insert(name, text);
                }
            }
        }
        Ok(form)
    }

    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
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
        matches!(self.field(name).map(str::trim), Some("true" | "1" | "yes"))
    }

    pub fn require_file(&mut self, what: &str) -> ApiResult<Document> {
        if self.files.is_empty() {
            return Err(ApiError::bad_request(format!(
                "no file in the form; send {what}"
            )));
        }
        Ok(self.files.remove(0))
    }
}

/// Keeps the base name only, so a client cannot steer a path.
fn sanitize(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name).trim();
    if base.is_empty() || base == "." || base == ".." {
        "upload".to_string()
    } else {
        base.to_string()
    }
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
    let mut res = Response::new(Body::from(out.bytes));
    *res.status_mut() = StatusCode::OK;
    let headers = res.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(out.content_type),
    );
    if let Ok(v) = HeaderValue::from_str(&format!(
        "attachment; filename=\"{}\"",
        out.name.replace('"', "")
    )) {
        headers.insert(header::CONTENT_DISPOSITION, v);
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
}
