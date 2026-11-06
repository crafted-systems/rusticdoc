//! Filters that produce content other than text.
//!
//! A rich filter writes no XML. It checks its value, stores the result in a
//! table that lives as long as one render, and prints a private marker. The
//! marker is built only from characters the value formatter strips from
//! everything else, so data can never produce one. After rendering, `apply`
//! finds the markers in each part and rewrites the enclosing run, paragraph or
//! cell, adding media, chart parts, relationships and content types.

mod chart;
mod docx;
mod html;
mod media;
mod odf;
mod pkg;
mod pptx;
mod xlsx;

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use minijinja::value::{Kwargs, Object, ObjectRepr, Rest, Value};
use minijinja::{Environment, Error, ErrorKind, State, UndefinedBehavior};

use crate::filters::xml_illegal;
use crate::package::{self, Entry, MAX_TOTAL_BYTES};
use crate::xml::{self, Elem, Node, Text};
use crate::{Format, Location, TemplateError};

pub use chart::Chart;
pub use media::Media;

/// Opens a marker; the digits of the table index follow, then [`MARK_CLOSE`].
const MARK_OPEN: char = '\u{1}';
const MARK_CLOSE: char = '\u{2}';
/// Marker digit 0; digits 1 to 9 follow it. Like the delimiters these are
/// characters XML forbids, so a marker that loses its way prints as nothing.
const MARK_DIGIT: u32 = 0x10;

/// The most rich values one render may register.
const MAX_VALUES: usize = 100_000;
/// The most charts one render may place.
const MAX_CHARTS: usize = 200;

const NAMES: [&str; 7] = [
    "image",
    "barcode",
    "link",
    "color",
    "html",
    "chart",
    "pagebreak",
];

/// How the rich filters behave in an environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// No package: the filters refuse with a clear error.
    Text,
    /// The filters check everything and print nothing.
    Check(Format),
    /// The filters print markers that `apply` resolves.
    Render(Format),
}

#[derive(Debug)]
pub struct Picture {
    pub media: Arc<Media>,
    pub width_pt: f64,
    pub height_pt: f64,
}

/// A checked value waiting for its place in the document.
#[derive(Debug)]
pub enum Rich {
    Picture(Picture),
    Link {
        url: String,
        text: String,
    },
    Color {
        text: String,
        /// `RRGGBB`.
        color: Option<String>,
        background: Option<String>,
    },
    Html(Vec<html::Block>),
    Chart(Chart),
    PageBreak,
}

/// The values one render registered. It lives in the render's `State`, so
/// concurrent renders of one compiled template never share it.
#[derive(Debug, Default)]
pub struct Table {
    inner: Mutex<Inner>,
    /// How many times a rich value was printed or turned into a string.
    consumed: AtomicUsize,
    /// Bytes of retained text behind every print: printing one long value
    /// in a loop places it each time.
    printed_bytes: AtomicU64,
}

#[derive(Debug, Default)]
struct Inner {
    items: Vec<Arc<Rich>>,
    media: HashMap<[u8; 32], Arc<Media>>,
    pictures: usize,
    charts: usize,
    html_cells: usize,
    media_bytes: u64,
    /// Text the table holds on to, for values repeated in a loop.
    retained: u64,
}

impl Table {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn push(&self, rich: Rich) -> Result<usize, String> {
        let mut inner = self.lock();
        if inner.items.len() >= MAX_VALUES {
            return Err(format!(
                "at most {MAX_VALUES} rich values are allowed in one render"
            ));
        }
        let retained = match &rich {
            Rich::Link { url, text } => url.len() + text.len(),
            Rich::Color { text, .. } => text.len(),
            Rich::Chart(chart) => chart.text_bytes(),
            _ => 0,
        };
        inner.retained += retained as u64;
        if inner.retained > MAX_TOTAL_BYTES {
            return Err(too_much_text());
        }
        inner.items.push(Arc::new(rich));
        Ok(inner.items.len() - 1)
    }

    /// Identical bytes share one `Media`; distinct bytes count against the
    /// package limit straight away, before any more of them are decoded.
    fn intern(&self, media: Media) -> Result<Arc<Media>, String> {
        let mut inner = self.lock();
        if inner.pictures >= media::MAX_IMAGES {
            return Err(format!(
                "at most {} images are allowed in one render",
                media::MAX_IMAGES
            ));
        }
        inner.pictures += 1;
        if let Some(found) = inner.media.get(&media.hash) {
            return Ok(found.clone());
        }
        inner.media_bytes += media.bytes.len() as u64;
        if inner.media_bytes > MAX_TOTAL_BYTES {
            return Err(format!(
                "the images exceed the package limit of {MAX_TOTAL_BYTES} bytes"
            ));
        }
        let media = Arc::new(media);
        inner.media.insert(media.hash, media.clone());
        Ok(media)
    }

    /// Counts one html value: the bytes it keeps and its table cells.
    fn count_html(&self, bytes: usize, cells: usize) -> Result<(), String> {
        let mut inner = self.lock();
        inner.retained += bytes as u64;
        inner.html_cells += cells;
        if inner.retained > MAX_TOTAL_BYTES {
            return Err(too_much_text());
        }
        if inner.html_cells > html::MAX_DOCUMENT_CELLS {
            return Err(format!(
                "the HTML tables hold more than {} cells in all",
                html::MAX_DOCUMENT_CELLS
            ));
        }
        Ok(())
    }

    fn count_chart(&self) -> Result<(), String> {
        let mut inner = self.lock();
        inner.charts += 1;
        if inner.charts > MAX_CHARTS {
            return Err(format!(
                "at most {MAX_CHARTS} charts are allowed in one render"
            ));
        }
        Ok(())
    }

    /// The text one print of a value puts in the document.
    fn print_cost(&self, id: usize) -> u64 {
        match self.lock().items.get(id).map(|r| &**r) {
            Some(Rich::Link { url, text }) => (url.len() + text.len()) as u64,
            Some(Rich::Color { text, .. }) => text.len() as u64,
            Some(Rich::Chart(chart)) => chart.text_bytes() as u64,
            _ => 0,
        }
    }

    fn items(&self) -> Vec<Arc<Rich>> {
        self.lock().items.clone()
    }
}

fn too_much_text() -> String {
    format!("the rich values hold more than the package limit of {MAX_TOTAL_BYTES} bytes")
}

/// What a rich filter returns. Only this type makes the formatter print a
/// marker; a string can never do so.
pub struct Token {
    id: usize,
    table: Arc<Table>,
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<rich value {}>", self.id)
    }
}

impl Object for Token {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }

    /// Reached when the value is used as a string (`~`, `upper`, `join`, …).
    /// The content cannot survive that; `apply` turns the count into an error.
    fn render(self: &Arc<Self>, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.table.consumed.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

/// Prints a rich value.
pub fn write_token(out: &mut minijinja::Output, token: &Token, mode: Mode) -> Result<(), Error> {
    if matches!(mode, Mode::Render(_)) {
        // A value kept with `{% set %}` and printed in a loop counts per
        // print, because each print places a picture or a chart.
        if token.table.consumed.fetch_add(1, Ordering::Relaxed) >= MAX_PLACEMENTS {
            return Err(Error::new(
                minijinja::ErrorKind::InvalidOperation,
                format!("rich values were printed more than {MAX_PLACEMENTS} times"),
            ));
        }
        let cost = token.table.print_cost(token.id) + PLACEMENT_OVERHEAD;
        if token.table.printed_bytes.fetch_add(cost, Ordering::Relaxed) + cost > MAX_TOTAL_BYTES {
            return Err(Error::new(
                minijinja::ErrorKind::InvalidOperation,
                too_much_text(),
            ));
        }
        out.write_str(&marker(token.id)).map_err(Error::from)?;
    }
    Ok(())
}

/// Most rich values one render may print.
const MAX_PLACEMENTS: usize = 100_000;
/// The markup around a placement, charged per print beside its text.
const PLACEMENT_OVERHEAD: u64 = 512;

fn marker(id: usize) -> String {
    let mut s = String::new();
    s.push(MARK_OPEN);
    for digit in id.to_string().bytes() {
        s.extend(char::from_u32(MARK_DIGIT + u32::from(digit - b'0')));
    }
    s.push(MARK_CLOSE);
    s
}

/// The value with XML-illegal characters removed from a string, for output
/// that does not go through the XML formatter.
pub fn scrubbed(value: &Value) -> Value {
    match value.as_str() {
        Some(s) if s.chars().any(xml_illegal) => {
            let clean: String = s.chars().filter(|c| !xml_illegal(*c)).collect();
            if value.is_safe() {
                Value::from_safe_string(clean)
            } else {
                Value::from(clean)
            }
        }
        _ => value.clone(),
    }
}

/// `(start, end, table index)` of every marker in `raw`, in order.
fn spans(raw: &str) -> Result<Vec<(usize, usize, usize)>, String> {
    let mut out = Vec::new();
    for (start, _) in raw.match_indices(MARK_OPEN) {
        let mut id = 0usize;
        let mut end = None;
        for (i, c) in raw[start + 1..].char_indices() {
            if c == MARK_CLOSE {
                end = Some(start + 1 + i + 1);
                break;
            }
            let digit = u32::from(c)
                .checked_sub(MARK_DIGIT)
                .filter(|d| *d < 10)
                .ok_or("a marker holds a character that is not a digit")?;
            id = id
                .checked_mul(10)
                .and_then(|n| n.checked_add(digit as usize))
                .ok_or("a marker holds an index that is too large")?;
        }
        out.push((start, end.ok_or("a marker is not closed")?, id));
    }
    Ok(out)
}

/// A piece of a text node: raw escaped text, or a marker.
enum Seg<'a> {
    Text(&'a str),
    Mark(usize),
}

fn segments(raw: &str) -> Result<Vec<Seg<'_>>, String> {
    let mut out = Vec::new();
    let mut last = 0;
    for (start, end, id) in spans(raw)? {
        if start > last {
            out.push(Seg::Text(&raw[last..start]));
        }
        out.push(Seg::Mark(id));
        last = end;
    }
    if last < raw.len() {
        out.push(Seg::Text(&raw[last..]));
    }
    Ok(out)
}

fn has_mark(nodes: &[Node]) -> bool {
    nodes.iter().any(|n| match n {
        Node::Text(t) => t.raw.contains(MARK_OPEN),
        Node::Elem(e) => has_mark(&e.children),
        Node::Raw(_) => false,
    })
}

/// Whether nothing would show if these nodes stood alone.
fn blank(nodes: &[Node]) -> bool {
    nodes.iter().all(|n| match n {
        Node::Text(t) => t.raw.trim().is_empty(),
        Node::Raw(s) => s.is_empty(),
        Node::Elem(e) => {
            matches!(e.name.as_str(), "w:pPr" | "w:rPr" | "w:proofErr")
                || (!e.self_closing && blank(&e.children))
        }
    })
}

/// The element without its content, so that cutting a run or paragraph into
/// pieces does not copy all of it for every piece.
fn shell(e: &Elem) -> Elem {
    Elem {
        name: e.name.clone(),
        open: e.open.clone(),
        self_closing: e.self_closing,
        children: Vec::new(),
        before: Vec::new(),
        after: Vec::new(),
    }
}

fn serialize(elem: &Elem) -> String {
    let mut out = String::new();
    xml::write_xml(std::slice::from_ref(&Node::Elem(elem.clone())), &mut out);
    out
}

fn text_node(raw: impl Into<String>) -> Node {
    Node::Text(Text {
        raw: raw.into(),
        pieces: None,
    })
}

/// Escapes data for text content, dropping what XML cannot hold at all.
fn esc(s: &str) -> String {
    let clean: String = s.chars().filter(|c| !xml_illegal(*c)).collect();
    xml::escape_text(&clean)
}

/// Escapes data for a double-quoted attribute.
fn esc_attr(s: &str) -> String {
    let clean: String = s.chars().filter(|c| !xml_illegal(*c)).collect();
    xml::escape_attr(&clean)
}

/// Points to English Metric Units.
fn emu(pt: f64) -> i64 {
    ((pt * 12700.0).round() as i64).max(1)
}

/// Declares the namespaces generated markup uses on the part's root.
fn ensure_ns(root: &mut Elem, namespaces: &[(&str, &str)]) {
    for (prefix, uri) in namespaces {
        let key = format!("xmlns:{prefix}");
        if root.attr(&key).is_none() {
            root.set_attr(&key, uri);
        }
    }
}

/// Puts `new` where a schema's element order wants it, replacing a child of
/// the same name. `order` is a space-separated list of names; children it does
/// not list are left where they are.
fn insert_ordered(parent: &mut Elem, new: Elem, order: &str) {
    parent.make_open();
    parent
        .children
        .retain(|n| !matches!(n, Node::Elem(e) if e.name == new.name));
    let rank = |name: &str| order.split(' ').position(|o| o == name);
    let at = rank(&new.name).and_then(|mine| {
        parent.children.iter().position(
            |n| matches!(n, Node::Elem(e) if rank(&e.name).is_some_and(|theirs| theirs > mine)),
        )
    });
    match at {
        Some(i) => parent.children.insert(i, Node::Elem(new)),
        None => parent.children.push(Node::Elem(new)),
    }
}

/// The highest `attr="N"` on `element` start tags in `src`.
fn max_id(src: &str, element: &str, attr: &str) -> u64 {
    let open = format!("<{element} ");
    let needle = format!(" {attr}=\"");
    src.match_indices(&open)
        .filter_map(|(at, _)| {
            let tag = &src[at..at + src[at..].find('>')?];
            let value = &tag[tag.find(&needle)? + needle.len()..];
            value[..value.find('"')?].parse::<u64>().ok()
        })
        .max()
        .unwrap_or(0)
}

/// An error for a rich value that cannot go where its tag sits.
fn misplaced(part: &str, place: &str, message: impl Into<String>) -> TemplateError {
    TemplateError::new(message).at(Location {
        part: part.to_string(),
        place: place.to_string(),
        snippet: String::new(),
    })
}

/// Resolves the markers in the rendered parts. `outputs` names the parts the
/// render wrote; everything else in the package stays byte-identical.
pub fn apply(
    format: Format,
    entries: &mut Vec<Entry>,
    table: &Table,
    rendered: &str,
    outputs: &[&str],
) -> Result<(), TemplateError> {
    let consumed = table.consumed.load(Ordering::Relaxed);
    let printed = rendered.matches(MARK_OPEN).count();
    if consumed != printed {
        return Err(TemplateError::new(
            "a rich filter's result must be printed directly: it cannot go through another \
             filter, `~`, a macro or a `{% set %}` block",
        ));
    }
    if printed == 0 {
        return Ok(());
    }
    let items = table.items();
    let names: Vec<String> = entries
        .iter()
        .filter(|e| {
            crate::formats::is_text_part(format, &e.name)
                && (format == Format::Pptx || outputs.contains(&e.name.as_str()))
                && e.data.contains(&(MARK_OPEN as u8))
        })
        .map(|e| e.name.clone())
        .collect();
    let mut pkg = pkg::Pkg::new(entries);
    let mut book = xlsx::Book::default();
    for name in names {
        let Some(src) = package::text_of(pkg.entries, &name) else {
            continue;
        };
        let mut nodes = xml::parse(&src)
            .map_err(|e| misplaced(&name, "", format!("rendered XML is malformed: {e}")))?;
        for (_, _, id) in spans(&src).map_err(|e| misplaced(&name, "", e))? {
            if id >= items.len() {
                return Err(misplaced(&name, "", "a marker names an unknown value"));
            }
        }
        match format {
            Format::Docx => docx::rewrite(&mut pkg, &items, &name, &src, &mut nodes)?,
            Format::Pptx => pptx::rewrite(&mut pkg, &items, &name, &src, &mut nodes)?,
            Format::Xlsx => xlsx::rewrite(&mut pkg, &mut book, &items, &name, &mut nodes)?,
            Format::Odt | Format::Ods | Format::Odp => {
                odf::rewrite(&mut pkg, format, &items, &name, &src, &mut nodes)?
            }
        }
        let out = xml::to_xml(&nodes);
        if out.contains(MARK_OPEN) {
            return Err(misplaced(
                &name,
                "",
                "a rich value sits where it cannot be placed",
            ));
        }
        package::set(pkg.entries, &name, out.into_bytes());
    }
    book.finish(&mut pkg)?;
    pkg.finish()
}

// ---------------------------------------------------------------------------
// The filters.

fn fail(filter: &str, message: impl fmt::Display) -> Error {
    Error::new(ErrorKind::InvalidOperation, format!("{filter}: {message}"))
}

/// What an undefined value means: an error when rendering for real; in the
/// lenient probing render it stays undefined, for the probe to report.
fn undefined(state: &State, filter: &str) -> Result<Value, Error> {
    match state.undefined_behavior() {
        UndefinedBehavior::Strict => Err(Error::new(
            ErrorKind::UndefinedError,
            format!("{filter}: the value is undefined"),
        )),
        _ => Ok(Value::UNDEFINED),
    }
}

fn shared_table(state: &mut State) -> Arc<Table> {
    state
        .get_or_insert_extension_with(|| Arc::new(Table::default()))
        .clone()
}

fn register(state: &mut State, filter: &str, rich: Rich) -> Result<Value, Error> {
    let table = shared_table(state);
    let id = table.push(rich).map_err(|e| fail(filter, e))?;
    Ok(Value::from_object(Token { id, table }))
}

fn register_picture(
    state: &mut State,
    filter: &str,
    media: Media,
    size: (f64, f64),
) -> Result<Value, Error> {
    let media = shared_table(state)
        .intern(media)
        .map_err(|e| fail(filter, e))?;
    register(
        state,
        filter,
        Rich::Picture(Picture {
            media,
            width_pt: size.0,
            height_pt: size.1,
        }),
    )
}

/// A length argument in points: positive, finite and of a sane size.
fn points(filter: &str, name: &str, value: &Value) -> Result<f64, Error> {
    let n = value
        .is_number()
        .then(|| f64::try_from(value.clone()).ok())
        .flatten()
        .ok_or_else(|| fail(filter, format!("{name} must be a number of points")))?;
    if !n.is_finite() || n <= 0.0 || n > media::MAX_SIDE_PT {
        return Err(fail(
            filter,
            format!(
                "{name} must be above 0 and at most {} points",
                media::MAX_SIDE_PT
            ),
        ));
    }
    Ok(n)
}

fn points_kw(filter: &str, kwargs: &Kwargs, name: &str) -> Result<Option<f64>, Error> {
    let value: Option<Value> = kwargs.get(name).map_err(|e| fail(filter, e))?;
    match value {
        Some(v) if !v.is_none() => points(filter, name, &v).map(Some),
        _ => Ok(None),
    }
}

fn no_more_kwargs(filter: &str, kwargs: &Kwargs) -> Result<(), Error> {
    kwargs.assert_all_used().map_err(|e| fail(filter, e))
}

fn display(value: &Value) -> String {
    if value.is_none() || value.is_undefined() {
        String::new()
    } else {
        value.to_string()
    }
}

fn image(state: &mut State, value: Value, kwargs: Kwargs) -> Result<Value, Error> {
    let width = points_kw("image", &kwargs, "width")?;
    let height = points_kw("image", &kwargs, "height")?;
    no_more_kwargs("image", &kwargs)?;
    if value.is_undefined() {
        return undefined(state, "image");
    }
    let input = media::decode(&value).map_err(|e| fail("image", e))?;
    let found = media::inspect(input.bytes, input.mime.as_deref()).map_err(|e| fail("image", e))?;
    let width = width.or(input.width);
    let height = height.or(input.height);
    let size = media::fit(found.px, width, height).map_err(|e| fail("image", e))?;
    register_picture(state, "image", found, size)
}

fn barcode(
    state: &mut State,
    value: Value,
    kind: Option<Value>,
    kwargs: Kwargs,
) -> Result<Value, Error> {
    let size = points_kw("barcode", &kwargs, "size")?;
    no_more_kwargs("barcode", &kwargs)?;
    let kind = kind.as_ref().and_then(|k| k.as_str()).ok_or_else(|| {
        fail(
            "barcode",
            "name the symbology: qr, code128, code39, ean13 or ean8",
        )
    })?;
    if value.is_undefined() {
        return undefined(state, "barcode");
    }
    let (found, size) =
        media::barcode(kind, &display(&value), size).map_err(|e| fail("barcode", e))?;
    register_picture(state, "barcode", found, size)
}

fn link(state: &mut State, value: Value, text: Option<Value>) -> Result<Value, Error> {
    if value.is_undefined() {
        return undefined(state, "link");
    }
    let url = html::check_url(&display(&value)).map_err(|e| fail("link", e))?;
    let text = match &text {
        Some(t) if !t.is_none() && !t.is_undefined() => display(t),
        _ => url.strip_prefix("mailto:").unwrap_or(&url).to_string(),
    };
    register(state, "link", Rich::Link { url, text })
}

fn color(
    state: &mut State,
    value: Value,
    color: Option<Value>,
    kwargs: Kwargs,
) -> Result<Value, Error> {
    let background: Option<Value> = kwargs.get("background").map_err(|e| fail("color", e))?;
    no_more_kwargs("color", &kwargs)?;
    let hex = |v: Option<Value>| -> Result<Option<String>, Error> {
        match v {
            Some(v) if !v.is_none() => {
                if v.is_undefined() {
                    return Err(fail("color", "the colour is undefined"));
                }
                html::check_hex(&display(&v))
                    .map(Some)
                    .ok_or_else(|| fail("color", "a colour must be written #rgb or #rrggbb"))
            }
            _ => Ok(None),
        }
    };
    let color = hex(color)?;
    let background = hex(background)?;
    if color.is_none() && background.is_none() {
        return Err(fail("color", "give a colour or background="));
    }
    if value.is_undefined() {
        return undefined(state, "color");
    }
    let rich = Rich::Color {
        text: display(&value),
        color,
        background,
    };
    register(state, "color", rich)
}

fn html_filter(state: &mut State, value: Value) -> Result<Value, Error> {
    if value.is_undefined() {
        return undefined(state, "html");
    }
    let input = display(&value);
    let blocks = html::parse(&input).map_err(|e| fail("html", e))?;
    let cells = html::table_cells(&blocks).map_err(|e| fail("html", e))?;
    shared_table(state)
        .count_html(input.len(), cells)
        .map_err(|e| fail("html", e))?;
    register(state, "html", Rich::Html(blocks))
}

fn chart_filter(
    format: Format,
    state: &mut State,
    value: Value,
    kwargs: Kwargs,
) -> Result<Value, Error> {
    let options = chart::Options::read(&kwargs)?;
    no_more_kwargs("chart", &kwargs)?;
    if !matches!(format, Format::Docx | Format::Pptx | Format::Xlsx) {
        return Err(fail(
            "chart",
            format!(
                "charts are not supported in {} templates yet",
                format.extension().to_ascii_uppercase()
            ),
        ));
    }
    if value.is_undefined() {
        return undefined(state, "chart");
    }
    shared_table(state)
        .count_chart()
        .map_err(|e| fail("chart", e))?;
    let chart = Chart::build(&value, options).map_err(|e| fail("chart", e))?;
    register(state, "chart", Rich::Chart(chart))
}

fn pagebreak(format: Format, state: &mut State, _value: Value) -> Result<Value, Error> {
    if !matches!(format, Format::Docx | Format::Odt) {
        return Err(fail(
            "pagebreak",
            format!(
                "page breaks exist only in DOCX and ODT, not {}",
                format.extension().to_ascii_uppercase()
            ),
        ));
    }
    register(state, "pagebreak", Rich::PageBreak)
}

/// Adds the rich filters. Without a format they exist only to say why they
/// cannot run: there is no package to hold their content.
pub fn add_filters(env: &mut Environment<'static>, mode: Mode) {
    let format = match mode {
        Mode::Text => {
            for name in NAMES {
                env.add_filter(
                    name,
                    move |_: Rest<Value>, _: Kwargs| -> Result<Value, Error> {
                        Err(fail(
                            name,
                            "this filter needs an Office template (DOCX, ODT, XLSX, ODS, PPTX \
                             or ODP) and is not available in a text template",
                        ))
                    },
                );
            }
            return;
        }
        Mode::Check(format) | Mode::Render(format) => format,
    };
    env.add_filter("image", image);
    env.add_filter("barcode", barcode);
    env.add_filter("link", link);
    env.add_filter("color", color);
    env.add_filter("html", html_filter);
    env.add_filter(
        "chart",
        move |state: &mut State, value: Value, kwargs: Kwargs| {
            chart_filter(format, state, value, kwargs)
        },
    );
    env.add_filter("pagebreak", move |state: &mut State, value: Value| {
        pagebreak(format, state, value)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_round_trip_and_vanish_when_stripped() {
        let m = marker(1205);
        assert!(m.chars().all(xml_illegal), "every character is stripped");
        let raw = format!("a{m}b{}", marker(0));
        let segs = segments(&raw).unwrap();
        assert!(matches!(segs[0], Seg::Text("a")));
        assert!(matches!(segs[1], Seg::Mark(1205)));
        assert!(matches!(segs[2], Seg::Text("b")));
        assert!(matches!(segs[3], Seg::Mark(0)));
        assert!(segments("x\u{1}y\u{2}").is_err());
        assert!(segments("x\u{1}\u{10}").is_err());
    }

    #[test]
    fn inserts_in_schema_order_and_replaces() {
        let order = "a b c";
        let mut p = xml::parse("<p><a/><c/></p>").unwrap();
        let root = xml::root_mut(&mut p).unwrap();
        insert_ordered(root, Elem::new("b", &[], true), order);
        assert_eq!(xml::to_xml(&p), "<p><a/><b/><c/></p>");
        let root = xml::root_mut(&mut p).unwrap();
        insert_ordered(root, Elem::new("a", &[("x", "1")], true), order);
        assert_eq!(xml::to_xml(&p), "<p><a x=\"1\"/><b/><c/></p>");
    }

    #[test]
    fn finds_the_highest_id() {
        let src =
            "<p:cNvPr id=\"2\" name=\"a\"/><p:cNvPr name=\"b\" id=\"11\"></p:cNvPr><x id=\"99\"/>";
        assert_eq!(max_id(src, "p:cNvPr", "id"), 11);
        assert_eq!(max_id("", "p:cNvPr", "id"), 0);
    }
}
