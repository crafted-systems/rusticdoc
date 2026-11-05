//! rusticdoc-template: Office documents as templates.
//!
//! A template is an ordinary DOCX, ODT, XLSX, ODS, PPTX or ODP file whose
//! text holds `{{ expression }}` and `{% statement %}` tags. The engine
//! compiles the document once into a program of literal XML spans and
//! expressions, then renders it against JSON data. Loops repeat the
//! structure their tags sit in: a table row, a run of paragraphs, a slide,
//! a sheet row or a sheet.
//!
//! This file holds the public interface; the implementation lands in the
//! submodules.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Formats the engine understands. Each is a ZIP of XML parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Docx,
    Odt,
    Xlsx,
    Ods,
    Pptx,
    Odp,
}

impl Format {
    pub fn from_extension(ext: &str) -> Option<Self> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "docx" => Format::Docx,
            "odt" => Format::Odt,
            "xlsx" => Format::Xlsx,
            "ods" => Format::Ods,
            "pptx" => Format::Pptx,
            "odp" => Format::Odp,
            _ => return None,
        })
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Docx => "docx",
            Format::Odt => "odt",
            Format::Xlsx => "xlsx",
            Format::Ods => "ods",
            Format::Pptx => "pptx",
            Format::Odp => "odp",
        }
    }
}

/// Where in the document something sits, for error messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    /// The ZIP part, such as `word/document.xml` or `xl/worksheets/sheet1.xml`.
    pub part: String,
    /// Plain words: `paragraph 12`, `sheet "Sales" row 4`, `slide 3`.
    pub place: String,
    /// The first words of the affected text, or the tag itself.
    pub snippet: String,
}

#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
#[error("{message}{}", location.as_ref().map(|l| format!(" ({} {}: {})", l.part, l.place, l.snippet)).unwrap_or_default())]
pub struct TemplateError {
    pub message: String,
    pub location: Option<Location>,
}

impl TemplateError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            location: None,
        }
    }

    pub fn at(mut self, location: Location) -> Self {
        self.location = Some(location);
        self
    }
}

pub type Result<T> = std::result::Result<T, TemplateError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// One finding from validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub location: Option<Location>,
}

/// Template-level settings, read from the document's front matter (a
/// `{% settings ... %}` tag or custom document properties).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub lang: Option<String>,
    pub timezone: Option<String>,
    pub currency: Option<String>,
    /// Translation table used by the `t` filter.
    pub translations: BTreeMap<String, String>,
}

/// A template parsed once, renderable many times.
#[derive(Debug)]
pub struct Compiled {
    format: Format,
    hash: String,
    settings: Settings,
    tags: Vec<String>,
    source: Vec<u8>,
}

impl Compiled {
    pub fn format(&self) -> Format {
        self.format
    }

    /// Hex SHA-256 of the template bytes.
    pub fn hash(&self) -> &str {
        &self.hash
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Every tag found, in document order, for validation and listing.
    pub fn tags(&self) -> &[String] {
        &self.tags
    }

    /// Fills the template. `data` is the root the expressions see; the
    /// caller puts request context under `ctx` before calling.
    pub fn render(&self, data: &serde_json::Value) -> Result<Vec<u8>> {
        let _ = (data, &self.source);
        Err(TemplateError::new(format!(
            "templating inside .{} files is not implemented yet",
            self.format.extension()
        )))
    }
}

/// Parses a template. Fails on a malformed container or an unparseable tag.
pub fn compile(format: Format, bytes: &[u8]) -> Result<Compiled> {
    use sha2::Digest;
    Ok(Compiled {
        format,
        hash: hex::encode(sha2::Sha256::digest(bytes)),
        settings: Settings::default(),
        tags: Vec::new(),
        source: bytes.to_vec(),
    })
}

/// Compiles and test-renders against sample data, reporting every problem
/// found rather than stopping at the first.
pub fn validate(format: Format, bytes: &[u8], data: &serde_json::Value) -> Vec<Diagnostic> {
    match compile(format, bytes) {
        Err(e) => vec![Diagnostic {
            severity: Severity::Error,
            message: e.message,
            location: e.location,
        }],
        Ok(compiled) => match compiled.render(data) {
            Ok(_) => Vec::new(),
            Err(e) => vec![Diagnostic {
                severity: Severity::Error,
                message: e.message,
                location: e.location,
            }],
        },
    }
}

/// The expression environment shared by every format: strict about
/// undefined names, with a step budget and the document filters.
pub fn environment() -> minijinja::Environment<'static> {
    let mut env = minijinja::Environment::new();
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
    env.set_fuel(Some(5_000_000));
    env.add_filter("currency", filters::currency);
    env
}

pub mod filters {
    /// `{{ 1234.5 | currency("EUR") }}` gives `EUR 1,234.50`.
    pub fn currency(value: f64, code: Option<String>) -> String {
        let negative = value < 0.0;
        let cents = (value.abs() * 100.0).round() as u64;
        let whole = cents / 100;
        let frac = cents % 100;
        let digits = whole.to_string();
        let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
        for (i, ch) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) {
                grouped.push(',');
            }
            grouped.push(ch);
        }
        let sign = if negative { "-" } else { "" };
        match code {
            Some(code) if !code.is_empty() => format!("{sign}{code} {grouped}.{frac:02}"),
            _ => format!("{sign}{grouped}.{frac:02}"),
        }
    }
}
