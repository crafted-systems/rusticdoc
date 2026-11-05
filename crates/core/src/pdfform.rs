//! PDF templates: filling form fields, tags in annotations, and laying a
//! rendered template over a base PDF.
//!
//! This file holds the interface; the implementation lands with phase 3.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// One AcroForm field as the API lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldInfo {
    /// Fully qualified name, such as `customer.name`.
    pub name: String,
    /// `text`, `checkbox`, `radio`, `choice`, `signature` or `button`.
    pub kind: String,
    /// 1-based page the first widget sits on.
    pub page: u32,
    /// Current value, when any.
    pub value: Option<String>,
    /// Options for choice fields.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct FillOptions {
    /// Flatten the filled fields into page content so they cannot change.
    pub flatten: bool,
    /// Remove annotations that carried tags after applying them.
    pub strip_tag_annotations: bool,
}

/// Lists the fields of a form.
pub fn fields(pdf: &[u8]) -> Result<Vec<FieldInfo>> {
    let _ = pdf;
    Err(Error::Unsupported(
        "PDF form listing arrives with phase 3".into(),
    ))
}

/// Fills a form from data. A field named `customer.name` takes that path
/// from the data; a field whose default value is a `{{ }}` tag is rendered
/// with the environment. Annotations (notes, free text) whose contents are
/// a tag fill the field under them, or draw the value in their rectangle.
pub fn fill(
    pdf: &[u8],
    data: &serde_json::Value,
    env: &minijinja::Environment<'static>,
    options: &FillOptions,
) -> Result<Vec<u8>> {
    let _ = (pdf, data, env, options);
    Err(Error::Unsupported(
        "PDF form filling arrives with phase 3".into(),
    ))
}

/// Lays every page of `overlay` over the matching page of `base`. Extra
/// overlay pages are appended; extra base pages are kept as they are.
pub fn overlay(base: &[u8], overlay: &[u8]) -> Result<Vec<u8>> {
    let _ = (base, overlay);
    Err(Error::Unsupported(
        "PDF overlay arrives with phase 3".into(),
    ))
}
