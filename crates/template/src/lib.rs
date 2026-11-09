//! rusticdoc-template: Office documents as templates.
//!
//! A template is an ordinary DOCX, ODT, XLSX, ODS, PPTX or ODP file whose
//! text holds `{{ expression }}` and `{% statement %}` tags. The engine
//! compiles the document once into a program of literal XML spans and
//! expressions, then renders it against JSON data. Loops repeat the
//! structure their tags sit in: a table row, a run of paragraphs, a slide,
//! a sheet row or a sheet.
//!
//! See `README.md` for the tag syntax and the loop placement rules.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use minijinja::value::Value;
use serde::{Deserialize, Serialize};

pub mod filters;
mod formats;
pub mod limit;
mod package;
mod program;
mod properties;
mod rich;
mod sheet;
mod workbook;
mod xml;

use program::{Mode, PartTree, Source, Tag, TagCtx};

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
    /// The format for a file extension such as `docx`, in any case.
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

    /// The lower-case file extension.
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

    /// The markup a newline inside a rendered value becomes.
    fn line_break(self) -> &'static str {
        match self {
            Format::Docx => "</w:t><w:br/><w:t xml:space=\"preserve\">",
            Format::Odt => "<text:line-break/>",
            _ => "",
        }
    }

    /// The environment a compiled template of this format renders with.
    fn environment(self, settings: &Settings) -> minijinja::Environment<'static> {
        filters::office_environment(settings, self.line_break(), rich::Mode::Render(self))
    }
}

/// Where in the document something sits, for error messages.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    /// The ZIP part, such as `word/document.xml` or `xl/worksheets/sheet1.xml`.
    pub part: String,
    /// Plain words: `paragraph 12`, `sheet "Sales" row 4`, `slide 3`.
    pub place: String,
    /// The first words of the affected text, or the tag itself.
    pub snippet: String,
}

/// A compile or render failure, with where it happened when known.
#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
#[error("{message}{}", location.as_ref().map(|l| format!(" ({} {}: {})", l.part, l.place, l.snippet)).unwrap_or_default())]
pub struct TemplateError {
    pub message: String,
    pub location: Option<Location>,
}

impl TemplateError {
    /// An error without a location.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            location: None,
        }
    }

    /// Attaches a location.
    pub fn at(mut self, location: Location) -> Self {
        self.location = Some(location);
        self
    }
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, TemplateError>;

/// How serious a validation finding is; warnings do not fail a render.
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

/// Template-level settings, read from the document's `{% settings ... %}`
/// tag (`lang`, `timezone`, `currency` and `t.<key>="…"` translations), and
/// from custom document properties of the same names. The tag wins where
/// both set the same thing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub lang: Option<String>,
    pub timezone: Option<String>,
    pub currency: Option<String>,
    /// Translation table used by the `t` filter.
    pub translations: BTreeMap<String, String>,
}

/// Choices a caller makes per render.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderOptions {
    /// Print an undefined value, or a missing attribute of one, as nothing
    /// instead of failing the render. `date`, `currency`, `round` and `abs`
    /// print nothing for it too, and a sheet cell holding only it is left
    /// empty. Off by default: a misspelt name is an error.
    pub undefined_as_empty: bool,
}

/// A template parsed once, renderable many times.
pub struct Compiled {
    format: Format,
    hash: String,
    settings: Settings,
    tags: Vec<String>,
    entries: Vec<package::Entry>,
    parts: Vec<PartTree>,
    table: Vec<Tag>,
    source: Source,
    env: minijinja::Environment<'static>,
    /// Most bytes one render may write: what the package could hold.
    output_limit: usize,
}

impl std::fmt::Debug for Compiled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Compiled")
            .field("format", &self.format)
            .field("hash", &self.hash)
            .field("settings", &self.settings)
            .field("tags", &self.tags)
            .field(
                "parts",
                &self.parts.iter().map(|p| &p.name).collect::<Vec<_>>(),
            )
            .finish()
    }
}

const TEMPLATE_NAME: &str = "document";

impl Compiled {
    /// The format the template was compiled as.
    pub fn format(&self) -> Format {
        self.format
    }

    /// Hex SHA-256 of the template bytes.
    pub fn hash(&self) -> &str {
        &self.hash
    }

    /// The settings read from the document's `{% settings %}` tag and its
    /// custom properties.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Replaces the settings, for callers that supply translations or a
    /// timezone from outside the document. Filters pick the change up on
    /// the next render.
    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = settings;
        let mut env = self.format.environment(&self.settings);
        let _ = env.add_template_owned(TEMPLATE_NAME.to_string(), self.source.text.clone());
        self.env = env;
    }

    /// Every tag found, in document order, for validation and listing.
    pub fn tags(&self) -> &[String] {
        &self.tags
    }

    /// Fills the template. `data` is the root the expressions see; the
    /// caller puts request context under `ctx` before calling.
    pub fn render(&self, data: &serde_json::Value) -> Result<Vec<u8>> {
        let (rendered, table) = self.render_text(&self.env, &self.source, data)?;
        self.assemble(&rendered, table.as_deref())
    }

    /// [`Compiled::render`] with per-render choices.
    pub fn render_with(
        &self,
        data: &serde_json::Value,
        options: &RenderOptions,
    ) -> Result<Vec<u8>> {
        if !options.undefined_as_empty {
            return self.render(data);
        }
        // Cloning shares the parsed template; only the settings differ.
        let mut env = self.env.clone();
        filters::undefined_as_empty(&mut env, &self.settings);
        let (rendered, table) = self.render_text(&env, &self.source, data)?;
        self.assemble(&rendered, table.as_deref())
    }

    fn render_text(
        &self,
        env: &minijinja::Environment<'static>,
        source: &Source,
        data: &serde_json::Value,
    ) -> Result<(String, Option<Arc<rich::Table>>)> {
        let tmpl = env
            .get_template(TEMPLATE_NAME)
            .map_err(|e| self.map_error(&e, source))?;
        // The package could never hold more than this, so a render that passes
        // it stops here rather than growing without bound.
        let (output, captured) = limit::render(
            &tmpl,
            Value::from(minijinja::value::Serde(data)),
            self.output_limit,
        )
        .map_err(|failure| match failure {
            limit::RenderFailure::TooLarge(limit) => TemplateError::new(format!(
                "the rendered document passes the package limit of {limit} bytes"
            )),
            limit::RenderFailure::Template(e) => self.map_error(&e, source),
        })?;
        let table = captured.state().get_extension::<Arc<rich::Table>>();
        Ok((output, table.cloned()))
    }

    fn map_error(&self, e: &minijinja::Error, source: &Source) -> TemplateError {
        let mut err = TemplateError::new(describe(e));
        if let Some(loc) = e
            .line()
            .and_then(|l| source.tag_at_line(l))
            .map(|id| self.table[id].loc.clone())
        {
            err = err.at(loc);
        }
        err
    }

    /// Puts the rendered parts back into the container.
    fn assemble(&self, rendered: &str, table: Option<&rich::Table>) -> Result<Vec<u8>> {
        let outputs = program::split_output(rendered);
        if let Some((name, _)) = outputs
            .iter()
            .find(|(name, _)| !self.parts.iter().any(|p| &p.name == name))
        {
            return Err(TemplateError::new(format!(
                "rendered output names a part that was not compiled: {name:?}"
            )));
        }
        let mut entries = self.entries.clone();
        let blocks_of = |name: &str| {
            self.parts
                .iter()
                .find(|p| p.name == name)
                .map(|p| p.row_blocks.as_slice())
                .unwrap_or(&[])
        };
        let mut names: Vec<String> = outputs.iter().map(|(n, _)| n.clone()).collect();
        match self.format {
            Format::Pptx => package::apply_slides(&mut entries, &outputs)?,
            Format::Xlsx => {
                let mut formulas = false;
                let mut sheets = Vec::with_capacity(outputs.len());
                for (name, content) in &outputs {
                    let (title, content) = workbook::split_title(content);
                    formulas |= content.contains("<f>") || content.contains("<f ");
                    let (fixed, layout) = sheet::rebase(content, blocks_of(name))
                        .map_err(|e| TemplateError::new(format!("{name}: {e}")))?;
                    sheets.push(workbook::SheetOut {
                        part: name.clone(),
                        title,
                        content: fixed,
                        layout,
                    });
                }
                let compiled: Vec<&str> = self.parts.iter().map(|p| p.name.as_str()).collect();
                names = workbook::finish_xlsx(&mut entries, &sheets, &compiled)?;
                if formulas {
                    if let Some(wb) = package::text_of(&entries, "xl/workbook.xml") {
                        package::set(
                            &mut entries,
                            "xl/workbook.xml",
                            sheet::force_recalc(&wb).into_bytes(),
                        );
                    }
                }
                // Excel's calculation chain lists formula cells where they
                // were. Once loops move rows it names cells that hold no
                // formula, and Excel reports unreadable content and offers a
                // repair. It is only a cache, rebuilt when Excel calculates.
                package::remove_part(&mut entries, "xl/calcChain.xml", "xl/workbook.xml");
            }
            Format::Ods => {
                for (name, content) in &outputs {
                    let fixed = workbook::finish_ods(content, blocks_of(name))
                        .map_err(|e| TemplateError::new(format!("{name}: {e}")))?;
                    package::set(&mut entries, name, fixed.into_bytes());
                }
            }
            _ => {
                for (name, content) in &outputs {
                    package::set(&mut entries, name, content.clone().into_bytes());
                }
            }
        }
        if let Some(table) = table {
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            rich::apply(self.format, &mut entries, table, rendered, &names)?;
        }
        package::write(&entries)
    }
}

fn describe(e: &minijinja::Error) -> String {
    match e.detail() {
        Some(d) => format!("{}: {d}", e.kind()),
        None => e.kind().to_string(),
    }
}

fn to_diag(e: TemplateError) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        message: e.message,
        location: e.location,
    }
}

/// Everything compile learns, including what went wrong.
struct Build {
    compiled: Compiled,
    diags: Vec<Diagnostic>,
    /// False when the template could not be parsed by minijinja.
    renderable: bool,
}

fn build(format: Format, bytes: &[u8]) -> Result<Build> {
    use sha2::Digest;
    let hash = hex::encode(sha2::Sha256::digest(bytes));
    let entries = package::read(bytes)?;
    let dialect = formats::dialect(format);
    let mut diags = Vec::new();
    let mut tags: Vec<Tag> = Vec::new();
    let mut ctxs: Vec<TagCtx> = Vec::new();
    // Properties first, so that a `{% settings %}` tag overrides them.
    let mut settings = properties::settings(format, &entries);
    let mut parts: Vec<PartTree> = Vec::new();
    let named = match format {
        Format::Xlsx => workbook::xlsx_named_parts(&entries),
        _ => Default::default(),
    };

    let shared = match format {
        Format::Xlsx => package::text_of(&entries, "xl/sharedStrings.xml")
            .map(|s| sheet::shared_strings(&s))
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    for info in package::text_parts(format, &entries) {
        let Some(src) = package::text_of(&entries, &info.name) else {
            continue;
        };
        let has_shared_tag = format == Format::Xlsx
            && src.contains("t=\"s\"")
            && shared.iter().any(|s| !program::find_tags(s).is_empty());
        // A slide the output does not list is a slide a loop dropped, so
        // slides are compiled even when they hold no tag.
        if !src.contains('{')
            && !has_shared_tag
            && format != Format::Pptx
            && !named.contains(&info.name)
        {
            continue;
        }
        let nodes = xml::parse(&src).map_err(|e| {
            TemplateError::new(format!("{}: malformed XML: {e}", info.name)).at(Location {
                part: info.name.clone(),
                place: String::new(),
                snippet: String::new(),
            })
        })?;
        let mut tree = PartTree {
            name: info.name.clone(),
            nodes,
            before: Vec::new(),
            after: Vec::new(),
            row_blocks: Vec::new(),
        };
        if format == Format::Xlsx {
            sheet::inline_shared_strings(&mut tree.nodes, &shared);
        }
        program::extract_part(
            parts.len(),
            &mut tree,
            &dialect,
            &info.prefix,
            &mut tags,
            &mut ctxs,
            &mut settings,
            &mut diags,
        );
        parts.push(tree);
    }
    let blocks = program::match_blocks(&tags, &mut diags);
    program::place_blocks(format, &mut parts, &tags, &ctxs, &blocks, &mut diags);
    workbook::Compile {
        format,
        entries: &entries,
        parts: &mut parts,
        tags: &mut tags,
        ctxs: &mut ctxs,
        blocks: &blocks,
        diags: &mut diags,
    }
    .run();
    if format == Format::Xlsx {
        for part in &mut parts {
            program::type_cells(part, &tags);
        }
    }
    // Slides without tags still take part in slide loops; other tagless
    // parts are copied through untouched.
    let parts: Vec<PartTree> = parts
        .into_iter()
        .enumerate()
        .filter(|(i, _)| format == Format::Pptx || ctxs.iter().any(|c| c.part == *i))
        .map(|(_, p)| p)
        .collect();

    let source = program::emit(&parts, &tags, Mode::Render);
    let mut env = format.environment(&settings);
    let mut renderable = true;
    if let Err(e) = env.add_template_owned(TEMPLATE_NAME.to_string(), source.text.clone()) {
        renderable = false;
        let mut err = TemplateError::new(format!("cannot parse tag: {}", describe(&e)));
        if let Some(loc) = e
            .line()
            .and_then(|l| source.tag_at_line(l))
            .map(|id| tags[id].loc.clone())
        {
            err = err.at(loc);
        }
        diags.push(to_diag(err));
    }

    let compiled = Compiled {
        format,
        hash,
        settings,
        tags: tags
            .iter()
            .filter(|t| !t.is_synthetic())
            .map(|t| t.text.clone())
            .collect(),
        entries,
        parts,
        table: tags,
        source,
        env,
        output_limit: package::MAX_TOTAL_BYTES as usize,
    };
    Ok(Build {
        compiled,
        diags,
        renderable,
    })
}

/// Parses a template. Fails on a malformed container or an unparseable tag.
pub fn compile(format: Format, bytes: &[u8]) -> Result<Compiled> {
    let b = build(format, bytes)?;
    if let Some(d) = b.diags.iter().find(|d| d.severity == Severity::Error) {
        return Err(TemplateError {
            message: d.message.clone(),
            location: d.location.clone(),
        });
    }
    Ok(b.compiled)
}

/// Compiles and test-renders against sample data, reporting every problem
/// found rather than stopping at the first.
pub fn validate(format: Format, bytes: &[u8], data: &serde_json::Value) -> Vec<Diagnostic> {
    validate_within(format, bytes, data, package::MAX_TOTAL_BYTES as usize)
}

/// What is wrong with a template whatever the data: a file that cannot be read,
/// and tags that cannot parse. A value the data has yet to supply is no fault
/// here, which is the difference from [`validate`], so this suits a template
/// being stored for later.
pub fn check_syntax(format: Format, bytes: &[u8]) -> Vec<Diagnostic> {
    let b = match build(format, bytes) {
        Ok(b) => b,
        Err(e) => return vec![to_diag(e)],
    };
    let mut diags = b.diags;
    syntax_diagnostics(&b.compiled, &mut diags);
    dedup(&mut diags);
    diags
}

/// Parses each tag on its own, so one bad tag does not hide the next.
fn syntax_diagnostics(compiled: &Compiled, diags: &mut Vec<Diagnostic>) {
    let probe_env = environment();
    let mut reported: Vec<Location> = Vec::new();
    for tag in &compiled.table {
        let Some(src) = program::standalone_source(tag) else {
            continue;
        };
        if let Err(e) = probe_env.template_from_str(&src) {
            reported.push(tag.loc.clone());
            diags.push(Diagnostic {
                severity: Severity::Error,
                message: format!("cannot parse tag: {}", describe(&e)),
                location: Some(tag.loc.clone()),
            });
        }
    }
    // The whole-template parse error repeats one of these.
    diags.retain(|d| {
        !(d.message.starts_with("cannot parse tag")
            && d.location.as_ref().is_some_and(|l| reported.contains(l))
            && !reported.is_empty()
            && !d.message.contains(": syntax error"))
    });
}

fn validate_within(
    format: Format,
    bytes: &[u8],
    data: &serde_json::Value,
    output_limit: usize,
) -> Vec<Diagnostic> {
    let mut b = match build(format, bytes) {
        Ok(b) => b,
        Err(e) => return vec![to_diag(e)],
    };
    b.compiled.output_limit = output_limit;
    let compiled = &b.compiled;
    let mut diags = b.diags;
    syntax_diagnostics(compiled, &mut diags);
    dedup(&mut diags);
    if !b.renderable || diags.iter().any(|d| d.severity == Severity::Error) {
        return diags;
    }

    // A strict render finds the first runtime problem precisely.
    match compiled.render_text(&compiled.env, &compiled.source, data) {
        // Rich values and sheet copies are placed after rendering, so placing
        // them is part of the test.
        Ok((rendered, table)) => {
            if table.is_some() || matches!(format, Format::Xlsx | Format::Ods) {
                if let Err(e) = compiled.assemble(&rendered, table.as_deref()) {
                    diags.push(to_diag(e));
                }
            }
        }
        Err(e) => diags.push(to_diag(e)),
    }

    // A probing render finds every undefined value and loops over scalars.
    let records: Arc<Mutex<Vec<(usize, Severity, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let source = program::emit(&compiled.parts, &compiled.table, Mode::Probe);
    let mut env = format.environment(&compiled.settings);
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Chainable);
    // A filter that refuses undefined input would end the probe at the first
    // missing value, so these pass it on for `__probe` to record.
    env.add_filter(
        "date",
        |v: Value,
         format: Option<String>,
         timezone: Option<String>|
         -> std::result::Result<Value, minijinja::Error> {
            if v.is_undefined() {
                return Ok(v);
            }
            filters::date(&v, format.as_deref(), timezone.as_deref()).map(Value::from)
        },
    );
    env.add_filter(
        "currency",
        |v: Value, code: Option<String>| -> std::result::Result<Value, minijinja::Error> {
            if v.is_undefined() {
                return Ok(v);
            }
            Ok(Value::from(filters::currency(f64::try_from(v)?, code)))
        },
    );
    env.add_filter(
        "round",
        |v: Value, precision: Option<i32>| -> std::result::Result<Value, minijinja::Error> {
            if v.is_undefined() {
                return Ok(v);
            }
            // The probe's output is thrown away, so the value only has to
            // stay a number.
            let _ = precision;
            Ok(Value::from(f64::try_from(v)?))
        },
    );
    let rec = records.clone();
    env.add_filter("__probe", move |v: Value, id: usize| {
        if v.is_undefined() {
            if let Ok(mut r) = rec.lock() {
                r.push((id, Severity::Error, "value is undefined".to_string()));
            }
        }
        v
    });
    let rec = records.clone();
    env.add_filter("__probe_iter", move |v: Value, id: usize| {
        if v.is_undefined() {
            if let Ok(mut r) = rec.lock() {
                r.push((id, Severity::Error, "loop over an undefined value".into()));
            }
            return Value::from(Vec::<Value>::new());
        }
        if v.try_iter().is_err() || v.as_str().is_some() {
            if let Ok(mut r) = rec.lock() {
                r.push((
                    id,
                    Severity::Warning,
                    format!("loop over a {} rather than an array", v.kind()),
                ));
            }
            return Value::from(Vec::<Value>::new());
        }
        v
    });
    if env
        .add_template_owned(TEMPLATE_NAME.to_string(), source.text.clone())
        .is_ok()
    {
        if let Err(e) = compiled.render_text(&env, &source, data) {
            if !e.message.starts_with("undefined value") {
                diags.push(to_diag(e));
            }
        }
    }
    if let Ok(records) = records.lock() {
        for (id, severity, message) in records.iter() {
            let tag = &compiled.table[*id];
            diags.push(Diagnostic {
                severity: *severity,
                message: format!("{message}: {}", tag.text),
                location: Some(tag.loc.clone()),
            });
        }
    }
    dedup(&mut diags);
    diags
}

fn dedup(diags: &mut Vec<Diagnostic>) {
    let mut seen: Vec<(Severity, Option<Location>)> = Vec::new();
    diags.retain(|d| {
        // One finding per tag and severity.
        let key = (d.severity, d.location.clone());
        if d.location.is_some() && seen.contains(&key) {
            false
        } else {
            seen.push(key);
            true
        }
    });
}

/// The expression environment shared by every format: strict about
/// undefined names, with a step budget and the document filters. This is the
/// environment for plain text templates: the rich filters (`image`, `barcode`,
/// `link`, `color`, `html`, `chart`, `pagebreak`) exist in it only to fail with
/// a clear error, because text has no package to put their content in.
pub fn environment() -> minijinja::Environment<'static> {
    filters::environment_for(&Settings::default(), "")
}

/// The expression environment with working rich filters for this format, for
/// callers that want to check tags and their arguments outside a compiled
/// template: a filter reports the same errors as in a real render, and its
/// result prints as nothing. Only [`Compiled::render`] places rich content,
/// and [`compile`] and [`validate`] already use the real filters themselves.
pub fn rich_environment(format: Format) -> minijinja::Environment<'static> {
    filters::office_environment(
        &Settings::default(),
        format.line_break(),
        rich::Mode::Check(format),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// A one-part DOCX whose text is `tag`.
    fn docx(tag: &str) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        let xml = format!(
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>{tag}</w:t></w:r></w:p></w:body></w:document>"#
        );
        zip.start_file("word/document.xml", options).unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
        zip.finish().unwrap().into_inner()
    }

    const WIDE: &str = r#"{% set x = "a" * 1000 %}{% for i in range(100000) %}{{ x }}{% endfor %}"#;

    #[test]
    fn a_render_stops_at_the_output_limit() {
        let mut compiled = compile(Format::Docx, &docx(WIDE)).unwrap();
        compiled.output_limit = 1 << 20;
        let err = compiled.render(&serde_json::json!({})).unwrap_err();
        assert!(err.message.contains("package limit"), "{err}");
    }

    #[test]
    fn the_package_limit_is_the_default() {
        let compiled = compile(Format::Docx, &docx("x {{ a }}")).unwrap();
        assert_eq!(compiled.output_limit as u64, package::MAX_TOTAL_BYTES);
    }

    /// A one-sheet XLSX whose sheet is named `name` and whose A1 holds `text`.
    fn xlsx(name: &str, text: &str) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        let main = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
        let rel = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
        let parts = [
            (
                "xl/workbook.xml",
                format!(
                    r#"<workbook xmlns="{main}" xmlns:r="{rel}"><sheets><sheet name="{name}" sheetId="1" r:id="rId1"/></sheets></workbook>"#
                ),
            ),
            (
                "xl/_rels/workbook.xml.rels",
                format!(
                    r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{rel}/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#
                ),
            ),
            (
                "xl/worksheets/sheet1.xml",
                format!(
                    r#"<worksheet xmlns="{main}"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>{text}</t></is></c></row></sheetData></worksheet>"#
                ),
            ),
        ];
        for (name, xml) in parts {
            zip.start_file(name, options).unwrap();
            zip.write_all(xml.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn a_sheet_loop_stops_at_the_output_limit() {
        let doc = xlsx(
            "{{ s }}",
            r#"{% sheet for s in range(100000) %}{{ "x" * 1000 }}"#,
        );
        let mut compiled = compile(Format::Xlsx, &doc).unwrap();
        compiled.output_limit = 1 << 20;
        let err = compiled.render(&serde_json::json!({})).unwrap_err();
        assert!(err.message.contains("package limit"), "{err}");
        let lenient = RenderOptions {
            undefined_as_empty: true,
        };
        let err = compiled
            .render_with(&serde_json::json!({}), &lenient)
            .unwrap_err();
        assert!(err.message.contains("package limit"), "{err}");
    }

    #[test]
    fn validation_stops_at_the_output_limit_too() {
        let problems = validate_within(Format::Docx, &docx(WIDE), &serde_json::json!({}), 1 << 20);
        assert!(
            problems.iter().any(|d| d.message.contains("package limit")),
            "{problems:?}"
        );
    }
}
