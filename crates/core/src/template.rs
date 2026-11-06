//! The template stage.
//!
//! Text formats are rendered whole with `minijinja`, so `{{ customer.name }}`,
//! `{% for %}` and filters all work in `.txt`, `.md`, `.html`, `.csv`, `.xml`,
//! `.json` and `.svg` templates today. Office formats are recognised and
//! refused with a clear message until the DOCX engine lands.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

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

/// Fills a template with `data` as the root and `ctx` under `ctx`. Office
/// formats go through the `rusticdoc-template` engine, compiled once per
/// content hash and cached; text formats are rendered whole.
pub fn render(
    doc: &Document,
    data: &serde_json::Value,
    ctx: &serde_json::Value,
) -> Result<Document> {
    let ext = doc.extension();
    let mut root = match data {
        serde_json::Value::Object(map) => map.clone(),
        serde_json::Value::Null => serde_json::Map::new(),
        _ => return Err(Error::InvalidRequest("'data' must be a JSON object".into())),
    };
    root.entry("ctx").or_insert_with(|| ctx.clone());

    if let Some(format) = rusticdoc_template::Format::from_extension(&ext) {
        let compiled = compiled(format, &doc.bytes)?;
        let bytes = compiled
            .render(&serde_json::Value::Object(root))
            .map_err(|e| Error::Template(e.to_string()))?;
        return Ok(Document::new(doc.name.clone(), bytes));
    }
    if !is_text_template(&ext) {
        return Err(Error::Unsupported(format!(
            "'.{ext}' is not a template format"
        )));
    }
    let source = std::str::from_utf8(&doc.bytes)
        .map_err(|_| Error::Template(format!("{} is not valid UTF-8", doc.name)))?;
    let env = environment();
    let rendered = env
        .render_str(
            source,
            minijinja::Value::from(minijinja::value::Serde(&root)),
        )
        .map_err(|e| Error::Template(describe(&e)))?;
    Ok(Document::new(doc.name.clone(), rendered.into_bytes()))
}

/// Every problem a template has against sample data, without rendering a
/// file for the caller: office formats report each tag; text formats
/// report the first error.
pub fn validate(
    doc: &Document,
    data: &serde_json::Value,
    ctx: &serde_json::Value,
) -> Vec<rusticdoc_template::Diagnostic> {
    use rusticdoc_template::{Diagnostic, Severity};
    let ext = doc.extension();
    if let Some(format) = rusticdoc_template::Format::from_extension(&ext) {
        let mut root = match data {
            serde_json::Value::Object(map) => map.clone(),
            _ => serde_json::Map::new(),
        };
        root.entry("ctx").or_insert_with(|| ctx.clone());
        return rusticdoc_template::validate(format, &doc.bytes, &serde_json::Value::Object(root));
    }
    match render(doc, data, ctx) {
        Ok(_) => Vec::new(),
        Err(e) => vec![Diagnostic {
            severity: Severity::Error,
            message: e.to_string(),
            location: None,
        }],
    }
}

/// Most compiled templates kept in memory, by content hash.
const CACHE_ENTRIES: usize = 64;
/// Largest template kept for reuse by hash.
const RECALL_MAX_BYTES: usize = 16 * 1024 * 1024;

fn recall_cache() -> &'static Mutex<Cache<Document>> {
    static RECALL: OnceLock<Mutex<Cache<Document>>> = OnceLock::new();
    RECALL.get_or_init(|| Mutex::new(Cache::default()))
}

/// Keeps a template so a later request from the same `scope` can send only
/// its hash. A scope is the caller's identity, so one tenant cannot recall
/// another's template by learning its hash.
pub fn remember(scope: &str, doc: &Document) {
    if doc.bytes.len() > RECALL_MAX_BYTES {
        return;
    }
    let key = format!("{scope}/{}", template_hash(&doc.bytes));
    recall_cache()
        .lock()
        .expect("template cache")
        .put(key, Arc::new(doc.clone()));
}

/// A template this `scope` remembered, by the hex SHA-256 of its bytes.
pub fn recall(scope: &str, hash: &str) -> Option<Document> {
    let key = format!("{scope}/{}", hash.trim().to_ascii_lowercase());
    recall_cache()
        .lock()
        .expect("template cache")
        .get(&key)
        .map(|d| d.as_ref().clone())
}

/// The compiled form of a template, from the cache or freshly parsed.
fn compiled(
    format: rusticdoc_template::Format,
    bytes: &[u8],
) -> Result<Arc<rusticdoc_template::Compiled>> {
    static CACHE: OnceLock<Mutex<Cache<rusticdoc_template::Compiled>>> = OnceLock::new();
    let key = format!("{}:{}", format.extension(), template_hash(bytes));
    let cache = CACHE.get_or_init(|| Mutex::new(Cache::default()));
    if let Some(hit) = cache.lock().expect("template cache").get(&key) {
        return Ok(hit);
    }
    let compiled = Arc::new(
        rusticdoc_template::compile(format, bytes).map_err(|e| Error::Template(e.to_string()))?,
    );
    cache
        .lock()
        .expect("template cache")
        .put(key, compiled.clone());
    Ok(compiled)
}

struct Cache<T> {
    map: HashMap<String, Arc<T>>,
    order: VecDeque<String>,
}

impl<T> Default for Cache<T> {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }
}

impl<T> Cache<T> {
    fn get(&mut self, key: &str) -> Option<Arc<T>> {
        let hit = self.map.get(key).cloned()?;
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
        self.order.push_back(key.to_string());
        Some(hit)
    }

    fn put(&mut self, key: String, value: Arc<T>) {
        if self.map.contains_key(&key) {
            if let Some(pos) = self.order.iter().position(|k| *k == key) {
                self.order.remove(pos);
            }
            self.map.remove(&key);
        }
        while self.map.len() >= CACHE_ENTRIES {
            match self.order.pop_front() {
                Some(old) => {
                    self.map.remove(&old);
                }
                None => break,
            }
        }
        self.map.insert(key.clone(), value);
        self.order.push_back(key);
    }
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

    fn tiny_docx(body: &str) -> Vec<u8> {
        use std::io::Write as _;
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zw = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            let parts = [
                ("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#.to_string()),
                ("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.to_string()),
                ("word/document.xml", format!(r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}</w:body></w:document>"#)),
            ];
            for (name, content) in parts {
                zw.start_file(name, opts).unwrap();
                zw.write_all(content.as_bytes()).unwrap();
            }
            zw.finish().unwrap();
        }
        buf.into_inner()
    }

    fn strip_tags(xml: &str) -> String {
        let mut out = String::new();
        let mut inside = false;
        for c in xml.chars() {
            match c {
                '<' => inside = true,
                '>' => inside = false,
                _ if !inside => out.push(c),
                _ => {}
            }
        }
        out
    }

    fn docx_text(bytes: &[u8]) -> String {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
        let mut xml = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("word/document.xml").unwrap(), &mut xml)
            .unwrap();
        xml
    }

    #[test]
    fn docx_templates_render_through_the_office_engine() {
        // The tag is split across two runs, as Word does it.
        let body = r#"<w:p><w:r><w:t xml:space="preserve">Hi {{ cust</w:t></w:r><w:r><w:t>omer.name }}!</w:t></w:r></w:p>"#;
        let doc = Document::new("letter.docx", tiny_docx(body));
        let out = render(&doc, &json!({"customer": {"name": "Ada"}}), &json!({})).unwrap();
        let xml = docx_text(&out.bytes);
        assert!(strip_tags(&xml).contains("Hi Ada!"), "{xml}");
        // A second render of the same bytes hits the cache and still works.
        let again = render(&doc, &json!({"customer": {"name": "Bob"}}), &json!({})).unwrap();
        assert!(strip_tags(&docx_text(&again.bytes)).contains("Hi Bob!"));
    }

    #[test]
    fn docx_validation_lists_every_problem() {
        let body = r#"<w:p><w:r><w:t>{{ missing }} and {{ also }}</w:t></w:r></w:p>"#;
        let doc = Document::new("letter.docx", tiny_docx(body));
        let problems = validate(&doc, &json!({}), &json!({}));
        assert!(problems.len() >= 2, "{problems:?}");
        assert!(problems.iter().all(|d| d.location.is_some()));
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
