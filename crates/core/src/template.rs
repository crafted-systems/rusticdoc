//! The template stage.
//!
//! Text formats are rendered whole with `minijinja`, so `{{ customer.name }}`,
//! `{% for %}` and filters all work in `.txt`, `.md`, `.html`, `.csv`, `.xml`,
//! `.json` and `.svg` templates today. Office formats are recognised and
//! refused with a clear message until the DOCX engine lands.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

use minijinja::{AutoEscape, Environment, UndefinedBehavior};
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
///
/// `limit` is the most a text template may write; the render stops there.
/// Office formats stop at their package limit instead.
///
/// `options.undefined_as_empty` prints undefined values as nothing rather
/// than failing, in every format.
pub fn render(
    doc: &Document,
    data: &serde_json::Value,
    ctx: &serde_json::Value,
    limit: usize,
    options: &rusticdoc_template::RenderOptions,
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
            .render_with(&serde_json::Value::Object(root), options)
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
    let mut env = environment();
    if options.undefined_as_empty {
        env.set_undefined_behavior(UndefinedBehavior::Chainable);
    }
    let escape = auto_escape(&ext);
    let writes_json = escape == AutoEscape::Json;
    env.set_auto_escape_callback(move |_| escape.clone());
    let rendered = render_str(
        &env,
        source,
        minijinja::Value::from(minijinja::value::Serde(&root)),
        limit,
    )?;
    if writes_json && let Err(e) = serde_json::from_str::<serde::de::IgnoredAny>(&rendered) {
        return Err(Error::Template(format!(
            "{} does not render to valid JSON: {e}. Values are written as JSON, so remove any quotes around {{{{ name }}}}",
            doc.name
        )));
    }
    Ok(Document::new(doc.name.clone(), rendered.into_bytes()))
}

/// How values are escaped in a text template of this format, so data cannot add
/// markup or keys of its own. `{{ value | safe }}` writes a value as it is.
fn auto_escape(ext: &str) -> AutoEscape {
    match ext {
        "html" | "htm" | "xml" | "svg" => AutoEscape::Html,
        "json" => AutoEscape::Json,
        _ => AutoEscape::None,
    }
}

/// Renders a template string, stopping at `limit` bytes of output.
pub fn render_str(
    env: &Environment<'_>,
    source: &str,
    ctx: minijinja::Value,
    limit: usize,
) -> Result<String> {
    use rusticdoc_template::limit::{self, RenderFailure};
    let template = env
        .template_from_str(source)
        .map_err(|e| Error::Template(describe(&e)))?;
    match limit::render(&template, ctx, limit) {
        Ok((output, _)) => Ok(output),
        Err(RenderFailure::TooLarge(limit)) => Err(too_large(limit)),
        Err(RenderFailure::Template(e)) => Err(Error::Template(describe(&e))),
    }
}

/// The refusal for output past the limit.
pub fn too_large(limit: usize) -> Error {
    Error::InvalidRequest(format!("rendered template is over the {limit} byte limit"))
}

/// Every problem a template has against sample data, without rendering a
/// file for the caller: office formats report each tag; text formats
/// report the first error.
pub fn validate(
    doc: &Document,
    data: &serde_json::Value,
    ctx: &serde_json::Value,
    limit: usize,
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
    match render(doc, data, ctx, limit, &Default::default()) {
        Ok(_) => Vec::new(),
        Err(e) => vec![Diagnostic {
            severity: Severity::Error,
            message: e.to_string(),
            location: None,
        }],
    }
}

/// What is wrong with a template whatever data it later gets. Formats that hold
/// no tags have nothing to check.
pub fn check_syntax(doc: &Document) -> Vec<rusticdoc_template::Diagnostic> {
    use rusticdoc_template::{Diagnostic, Severity};
    let ext = doc.extension();
    if let Some(format) = rusticdoc_template::Format::from_extension(&ext) {
        return rusticdoc_template::check_syntax(format, &doc.bytes);
    }
    if is_text_template(&ext) {
        let source = String::from_utf8_lossy(&doc.bytes);
        let env = environment();
        if let Err(e) = env.template_from_str(&source) {
            return vec![Diagnostic {
                severity: Severity::Error,
                message: e.to_string(),
                location: None,
            }];
        }
    }
    Vec::new()
}

/// Every tag a template holds, in document order: `{{ customer.name }}`,
/// `{% for item in items %}`. Formats that hold no tags have none. A template
/// with a syntax error fails, as it would at its first render.
pub fn tags(doc: &Document) -> Result<Vec<String>> {
    let ext = doc.extension();
    if let Some(format) = rusticdoc_template::Format::from_extension(&ext) {
        return Ok(compiled(format, &doc.bytes)?.tags().to_vec());
    }
    if is_text_template(&ext) {
        let source = String::from_utf8_lossy(&doc.bytes);
        environment()
            .template_from_str(&source)
            .map_err(|e| Error::Template(e.to_string()))?;
        return Ok(scan_tags(&source));
    }
    Ok(Vec::new())
}

/// The tags of a text template, as written, less comments and what a
/// `{% raw %}` block holds.
fn scan_tags(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = source;
    let mut raw = false;
    while let Some(at) = rest.find('{') {
        let after = &rest[at..];
        let close = match after.as_bytes().get(1) {
            Some(b'{') => "}}",
            Some(b'%') => "%}",
            Some(b'#') => "#}",
            _ => {
                rest = &after[1..];
                continue;
            }
        };
        let Some(end) = after[2..].find(close) else {
            break;
        };
        let tag = &after[..end + 4];
        rest = &after[end + 4..];
        let inner = tag[2..tag.len() - 2].trim().trim_matches('-').trim();
        if raw {
            raw = !(close == "%}" && inner == "endraw");
            continue;
        }
        match close {
            "#}" => {}
            "%}" if inner == "raw" => raw = true,
            _ => found.push(tag.to_string()),
        }
    }
    found
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

static CACHE_HITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static CACHE_MISSES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Compiled-template cache lookups since start: hits, then misses. A low hit
/// rate means templates are parsed again on most renders, which is where the
/// cache's time saving goes.
pub fn cache_stats() -> (u64, u64) {
    use std::sync::atomic::Ordering::Relaxed;
    (CACHE_HITS.load(Relaxed), CACHE_MISSES.load(Relaxed))
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
        CACHE_HITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return Ok(hit);
    }
    CACHE_MISSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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

/// Evaluation steps one render may take.
pub const TEMPLATE_FUEL: u64 = 5_000_000;

/// The expression environment: strict about undefined names, with the
/// document filters added.
pub fn environment() -> Environment<'static> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    // A budget of evaluation steps, so a template cannot loop for ever.
    env.set_fuel(Some(TEMPLATE_FUEL));
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

    const LIMIT: usize = crate::limits::DEFAULT_MAX_RENDER_BYTES;

    fn txt(s: &str) -> Document {
        Document::new("letter.txt", s.as_bytes().to_vec())
    }

    #[test]
    fn substitutes_loops_and_filters() {
        let doc = txt(
            "Dear {{ customer.name }},\n{% for i in items %}- {{ i.name }}: {{ i.price | currency('EUR') }}\n{% endfor %}Total {{ items | sum(attribute='price') | currency('EUR') }}\nFrom {{ ctx.company }}",
        );
        let data = json!({"customer": {"name": "Ada"}, "items": [{"name": "Lamp", "price": 1234.5}, {"name": "Plug", "price": 0.5}]});
        let out = render(
            &doc,
            &data,
            &json!({"company": "Crafted"}),
            LIMIT,
            &Default::default(),
        )
        .unwrap();
        let text = String::from_utf8(out.bytes.to_vec()).unwrap();
        assert_eq!(
            text,
            "Dear Ada,\n- Lamp: EUR 1,234.50\n- Plug: EUR 0.50\nTotal EUR 1,235.00\nFrom Crafted"
        );
    }

    fn filled(name: &str, source: &str, data: serde_json::Value) -> Result<String> {
        let doc = Document::new(name, source.as_bytes().to_vec());
        let out = render(&doc, &data, &json!({}), LIMIT, &Default::default())?;
        Ok(String::from_utf8(out.bytes.to_vec()).unwrap())
    }

    #[test]
    fn markup_formats_escape_values() {
        let evil = json!({"x": "<script>alert('1')</script>&\""});
        for name in ["a.html", "a.htm", "a.xml", "a.svg"] {
            let out = filled(name, "<p title=\"{{ x }}\">{{ x }}</p>", evil.clone()).unwrap();
            assert!(!out.contains("<script"), "{name}: {out}");
            assert!(out.contains("&lt;script&gt;"), "{name}: {out}");
            assert!(out.contains("&amp;&quot;\">"), "{name}: {out}");
        }
    }

    #[test]
    fn markup_can_be_let_through_on_purpose_and_is_not_escaped_twice() {
        let data = json!({"x": "<b>&"});
        assert_eq!(
            filled("a.html", "{{ x | safe }}", data.clone()).unwrap(),
            "<b>&"
        );
        assert_eq!(
            filled("a.html", "{{ x | escape }}", data.clone()).unwrap(),
            "&lt;b&gt;&amp;"
        );
        assert_eq!(
            filled(
                "a.html",
                "{% autoescape false %}{{ x }}{% endautoescape %}",
                data
            )
            .unwrap(),
            "<b>&"
        );
    }

    #[test]
    fn plain_formats_write_values_as_they_are() {
        for name in ["a.txt", "a.md", "a.csv"] {
            let out = filled(name, "{{ x }}", json!({"x": "<b> & \"q\""})).unwrap();
            assert_eq!(out, "<b> & \"q\"", "{name}");
        }
    }

    #[test]
    fn json_templates_write_values_as_json() {
        let source = "{\"name\":{{ name }},\"n\":{{ n }},\"ok\":{{ ok }},\"list\":{{ list }},\"also\":{{ name | tojson }}}";
        let evil = "a\",\"admin\":true,\"b\":\"";
        let data = json!({"name": evil, "n": 1.5, "ok": true, "list": [1, "two"]});
        let out = filled("a.json", source, data).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["name"], evil);
        assert_eq!(parsed["also"], evil);
        assert_eq!(parsed["n"], 1.5);
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["list"], json!([1, "two"]));
        assert!(parsed.get("admin").is_none(), "{out}");
    }

    #[test]
    fn a_quoted_value_in_a_json_template_is_a_clear_error() {
        let err = filled(
            "a.json",
            "{\"name\":\"{{ name }}\"}",
            json!({"name": "Ada"}),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Template(_)), "{err}");
        assert!(err.to_string().contains("remove any quotes"), "{err}");
        assert!(err.to_string().contains("{{ name }}"), "{err}");
    }

    #[test]
    fn undefined_names_are_errors_with_a_line() {
        let doc = txt("line one\n{{ nobody.home }}");
        let err = render(&doc, &json!({}), &json!({}), LIMIT, &Default::default()).unwrap_err();
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
        let out = render(
            &doc,
            &json!({"customer": {"name": "Ada"}}),
            &json!({}),
            LIMIT,
            &Default::default(),
        )
        .unwrap();
        let xml = docx_text(&out.bytes);
        assert!(strip_tags(&xml).contains("Hi Ada!"), "{xml}");
        // A second render of the same bytes hits the cache and still works.
        let again = render(
            &doc,
            &json!({"customer": {"name": "Bob"}}),
            &json!({}),
            LIMIT,
            &Default::default(),
        )
        .unwrap();
        assert!(strip_tags(&docx_text(&again.bytes)).contains("Hi Bob!"));
    }

    #[test]
    fn docx_validation_lists_every_problem() {
        let body = r#"<w:p><w:r><w:t>{{ missing }} and {{ also }}</w:t></w:r></w:p>"#;
        let doc = Document::new("letter.docx", tiny_docx(body));
        let problems = validate(&doc, &json!({}), &json!({}), LIMIT);
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

    /// A short template that prints a long value in a loop: 100 MB with fuel to
    /// spare, so only the writer can stop it.
    const WIDE: &str = r#"{% set x = "a" * 1000 %}{% for i in range(100000) %}{{ x }}{% endfor %}"#;

    #[test]
    fn text_output_stops_at_the_limit() {
        let err = render(
            &txt(WIDE),
            &json!({}),
            &json!({}),
            1 << 20,
            &Default::default(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidRequest(_)), "{err}");
        assert!(err.to_string().contains("byte limit"), "{err}");
        let problems = validate(&txt(WIDE), &json!({}), &json!({}), 1 << 20);
        assert!(problems[0].message.contains("byte limit"), "{problems:?}");
    }

    #[test]
    fn hash_is_stable() {
        assert_eq!(template_hash(b"abc").len(), 64);
        assert_eq!(template_hash(b"abc"), template_hash(b"abc"));
        assert_ne!(template_hash(b"abc"), template_hash(b"abd"));
    }
}
