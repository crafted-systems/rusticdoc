//! The template stage.
//!
//! Text formats are rendered whole with `minijinja`, so `{{ customer.name }}`,
//! `{% for %}` and filters all work in `.txt`, `.md`, `.html`, `.csv`, `.xml`,
//! `.json` and `.svg` templates today. Office formats are recognised and
//! refused with a clear message until the DOCX engine lands.

use minijinja::{Environment, UndefinedBehavior};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::job::Document;

/// Formats rendered as one text file.
pub const TEXT_FORMATS: &[&str] = &["txt", "md", "html", "htm", "csv", "xml", "json", "svg"];

/// Formats that are ZIPs of XML and get their own engines.
pub const OFFICE_FORMATS: &[&str] = &["docx", "xlsx", "pptx", "odt", "ods", "odp"];

/// Hex SHA-256 of a template, the key for compiled-template caches.
pub fn template_hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub fn is_text_template(ext: &str) -> bool {
    TEXT_FORMATS.contains(&ext)
}

pub fn is_office_format(ext: &str) -> bool {
    OFFICE_FORMATS.contains(&ext)
}

/// Fills a template with `data` as the root and `ctx` under `ctx`.
pub fn render(
    doc: &Document,
    data: &serde_json::Value,
    ctx: &serde_json::Value,
) -> Result<Document> {
    let ext = doc.extension();
    if is_office_format(&ext) {
        return Err(Error::Unsupported(format!(
            "templating inside .{ext} files arrives in phase 1; send the file without 'data' to convert it as it is"
        )));
    }
    if !is_text_template(&ext) {
        return Err(Error::Unsupported(format!(
            "'.{ext}' is not a template format"
        )));
    }
    let source = std::str::from_utf8(&doc.bytes)
        .map_err(|_| Error::Template(format!("{} is not valid UTF-8", doc.name)))?;

    let mut root = match data {
        serde_json::Value::Object(map) => map.clone(),
        serde_json::Value::Null => serde_json::Map::new(),
        _ => return Err(Error::InvalidRequest("'data' must be a JSON object".into())),
    };
    root.entry("ctx").or_insert_with(|| ctx.clone());

    let env = environment();
    let rendered = env
        .render_str(
            source,
            minijinja::Value::from(minijinja::value::Serde(&root)),
        )
        .map_err(|e| Error::Template(describe(&e)))?;

    Ok(Document::new(doc.name.clone(), rendered.into_bytes()))
}

/// The expression environment: strict about undefined names, with the
/// document filters added.
pub fn environment() -> Environment<'static> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    // A budget of evaluation steps, so a template cannot loop for ever.
    env.set_fuel(Some(5_000_000));
    env.add_filter("currency", currency);
    env
}

/// `{{ 1234.5 | currency("EUR") }}` gives `EUR 1,234.50`.
fn currency(value: f64, code: Option<String>) -> String {
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

fn describe(e: &minijinja::Error) -> String {
    let mut msg = e.to_string();
    if let Some(line) = e.line() {
        msg.push_str(&format!(" (line {line})"));
    }
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        msg.push_str(&format!(": {s}"));
        source = s.source();
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn txt(s: &str) -> Document {
        Document::new("letter.txt", s.as_bytes().to_vec())
    }

    #[test]
    fn substitutes_loops_and_filters() {
        let doc = txt(
            "Dear {{ customer.name }},\n{% for i in items %}- {{ i.name }}: {{ i.price | currency('EUR') }}\n{% endfor %}Total {{ items | sum(attribute='price') | currency('EUR') }}\nFrom {{ ctx.company }}",
        );
        let data = json!({"customer": {"name": "Ada"}, "items": [{"name": "Lamp", "price": 1234.5}, {"name": "Plug", "price": 0.5}]});
        let out = render(&doc, &data, &json!({"company": "Crafted"})).unwrap();
        let text = String::from_utf8(out.bytes.to_vec()).unwrap();
        assert_eq!(
            text,
            "Dear Ada,\n- Lamp: EUR 1,234.50\n- Plug: EUR 0.50\nTotal EUR 1,235.00\nFrom Crafted"
        );
    }

    #[test]
    fn undefined_names_are_errors_with_a_line() {
        let doc = txt("line one\n{{ nobody.home }}");
        let err = render(&doc, &json!({}), &json!({})).unwrap_err();
        assert!(matches!(err, Error::Template(_)));
        assert!(err.to_string().contains("line 2"), "{err}");
    }

    #[test]
    fn office_formats_are_refused_clearly() {
        let doc = Document::new("report.docx", vec![0x50, 0x4b]);
        let err = render(&doc, &json!({"a": 1}), &json!({})).unwrap_err();
        assert!(matches!(err, Error::Unsupported(_)));
        assert!(err.to_string().contains("phase 1"));
    }

    #[test]
    fn currency_formats() {
        assert_eq!(currency(0.0, None), "0.00");
        assert_eq!(
            currency(-1234567.891, Some("USD".into())),
            "-USD 1,234,567.89"
        );
        assert_eq!(currency(999.999, Some("".into())), "1,000.00");
    }

    #[test]
    fn hash_is_stable() {
        assert_eq!(template_hash(b"abc").len(), 64);
        assert_eq!(template_hash(b"abc"), template_hash(b"abc"));
        assert_ne!(template_hash(b"abc"), template_hash(b"abd"));
    }
}
