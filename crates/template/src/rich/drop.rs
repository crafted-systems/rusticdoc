//! `drop("…")`: a value that removes the element holding it.
//!
//! `{{ drop("column") if not discount }}` prints nothing. Like the other rich
//! values it prints a marker; after rendering, [`apply_part`] finds each
//! marker, works out the element its position names (the paragraph, table
//! row, table column, table, picture, chart, shape, slide or sheet) and
//! removes it, with the relationships, parts and manifest entries only that
//! element used. A drop inside a repeated row removes that copy only, since
//! each copy carries its own marker.
//!
//! Pictures, charts and shapes take the tag in their alt text. ODF keeps alt
//! text in `svg:title` and `svg:desc` elements, which the compiler reads like
//! paragraphs. OOXML keeps it in the `descr` and `title` attributes of
//! `wp:docPr` and `*:cNvPr`, so [`extract_alt`] moves those tags into pieces
//! printed after the element, between characters XML forbids, and
//! [`finish_alt`] puts the rendered text back into the attribute.
//!
//! Slides in a PPTX and sheets in an XLSX are parts of their own: the
//! assembler leaves them out before the slide list or the workbook is
//! rebuilt, as a loop over an empty list does.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, LazyLock};

use minijinja::value::Value;
use minijinja::{Error, State};
use regex::Regex;

use crate::package::{self, Entry};
use crate::program::{find_tags, PartTree, Tag, TagCtx, TagKind};
use crate::rich::pkg::Pkg;
use crate::rich::{fail, misplaced, register, segments, spans, Rich, Seg, Table, MARK_OPEN};
use crate::sheet::{col_to_num, num_to_col, rewrite_refs, split_ref, RefPos};
use crate::xml::{self, elem_at, elem_at_mut, Elem, Node, Path, Piece, TagId};
use crate::{Diagnostic, Format, Location, Severity, TemplateError};

/// Opens a piece of alt text printed after its element; the attribute's name
/// follows, then [`ALT_SEP`], the text, and [`ALT_CLOSE`]. Like the value
/// markers these are characters XML forbids, which the formatter strips from
/// data.
pub const ALT_OPEN: char = '\u{5}';
const ALT_SEP: char = '\u{6}';
const ALT_CLOSE: char = '\u{7}';
const ALT_ATTRS: [&str; 2] = ["descr", "title"];

/// The most paragraphs or rows one drop may remove.
const MAX_COUNT: u32 = 10_000;

/// What a drop removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Element {
    Paragraph,
    Row,
    Column,
    Table,
    Picture,
    Chart,
    Shape,
    Slide,
    Sheet,
}

const ELEMENTS: [Element; 9] = [
    Element::Paragraph,
    Element::Row,
    Element::Column,
    Element::Table,
    Element::Picture,
    Element::Chart,
    Element::Shape,
    Element::Slide,
    Element::Sheet,
];

impl Element {
    fn name(self) -> &'static str {
        match self {
            Element::Paragraph => "paragraph",
            Element::Row => "row",
            Element::Column => "column",
            Element::Table => "table",
            Element::Picture => "picture",
            Element::Chart => "chart",
            Element::Shape => "shape",
            Element::Slide => "slide",
            Element::Sheet => "sheet",
        }
    }

    fn parse(s: &str) -> Option<Element> {
        ELEMENTS.into_iter().find(|e| e.name() == s)
    }

    fn takes_count(self) -> bool {
        matches!(self, Element::Paragraph | Element::Row)
    }

    /// Whether the format has this element at all.
    fn supported(self, format: Format) -> Result<(), String> {
        let upper = format.extension().to_ascii_uppercase();
        match (self, format) {
            (Element::Slide, Format::Pptx | Format::Odp) => Ok(()),
            (Element::Slide, _) => Err(format!(
                "a {upper} has no slides; drop(\"slide\") works in PPTX and ODP"
            )),
            (Element::Sheet, Format::Xlsx | Format::Ods) => Ok(()),
            (Element::Sheet, _) => Err(format!(
                "a {upper} has no sheets; drop(\"sheet\") works in XLSX and ODS"
            )),
            (Element::Paragraph | Element::Table, Format::Xlsx | Format::Ods) => Err(format!(
                "a sheet has no {}s to remove; use row, column or sheet",
                self.name()
            )),
            _ => Ok(()),
        }
    }
}

/// A checked drop, waiting for its place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Drop {
    pub element: Element,
    /// How many paragraphs or rows, from the tag's own.
    pub count: u32,
}

fn element_list() -> String {
    ELEMENTS.map(Element::name).join(", ")
}

/// The `drop(element, n)` function.
pub(super) fn function(
    format: Format,
    state: &mut State,
    element: Option<Value>,
    count: Option<Value>,
) -> Result<Value, Error> {
    let name = element
        .as_ref()
        .and_then(|v| v.as_str())
        .ok_or_else(|| fail("drop", format!("name what to remove: {}", element_list())))?;
    let element = Element::parse(name).ok_or_else(|| {
        fail(
            "drop",
            format!(
                "{name:?} is not something drop removes; use {}",
                element_list()
            ),
        )
    })?;
    element.supported(format).map_err(|m| fail("drop", m))?;
    let count = match count {
        None => 1,
        Some(v) if v.is_none() || v.is_undefined() => 1,
        Some(v) => {
            if !element.takes_count() {
                return Err(fail(
                    "drop",
                    format!(
                        "drop(\"{}\") takes no count; only paragraph and row do",
                        element.name()
                    ),
                ));
            }
            v.as_i64()
                .filter(|n| (1..=MAX_COUNT as i64).contains(n))
                .ok_or_else(|| {
                    fail(
                        "drop",
                        format!("the count must be a whole number from 1 to {MAX_COUNT}"),
                    )
                })? as u32
        }
    };
    register(state, "drop", Rich::Drop(Drop { element, count }))
}

// ---------------------------------------------------------------------------
// Compile time.

fn is_alt_owner(name: &str) -> bool {
    name == "wp:docPr" || name.ends_with(":cNvPr")
}

fn find_owners(nodes: &[Node], prefix: &mut Path, out: &mut Vec<Path>) {
    for (i, n) in nodes.iter().enumerate() {
        let Node::Elem(e) = n else { continue };
        prefix.push(i);
        if is_alt_owner(&e.name) && e.open.contains('{') {
            out.push(prefix.clone());
        }
        find_owners(&e.children, prefix, out);
        prefix.pop();
    }
}

/// Whether the blocks among these tags all close.
fn balanced(tags: &[Tag]) -> bool {
    let mut stack: Vec<&str> = Vec::new();
    for t in tags {
        if let Some(name) = t.opener() {
            stack.push(name);
        } else if let Some(name) = t.closer() {
            if stack.pop() != Some(name) {
                return false;
            }
        }
    }
    stack.is_empty()
}

/// Moves the tags in OOXML alt text (`descr` and `title` of `wp:docPr` and
/// `*:cNvPr`) into pieces printed right after their element. Returns nothing;
/// the ids of the tags it adds go into `alt`.
#[allow(clippy::too_many_arguments)]
pub fn extract_alt(
    format: Format,
    part: usize,
    tree: &mut PartTree,
    prefix: &str,
    tags: &mut Vec<Tag>,
    ctxs: &mut Vec<TagCtx>,
    alt: &mut HashSet<TagId>,
    diags: &mut Vec<Diagnostic>,
) {
    if !matches!(format, Format::Docx | Format::Pptx | Format::Xlsx) {
        return;
    }
    let mut owners = Vec::new();
    find_owners(&tree.nodes, &mut Vec::new(), &mut owners);
    for path in owners {
        let Some(e) = elem_at_mut(&mut tree.nodes, &path) else {
            continue;
        };
        let shape = e.attr("name").unwrap_or_default();
        for attr in ALT_ATTRS {
            let Some(value) = e.attr(attr) else { continue };
            let found = find_tags(&value);
            if found.is_empty() {
                continue;
            }
            let loc = |snippet: &str| Location {
                part: tree.name.clone(),
                place: format!("{prefix}alt text of {shape:?}"),
                snippet: snippet.trim().to_string(),
            };
            let parsed: Vec<Tag> = found
                .iter()
                .map(|&(s, end)| Tag::parse(&value[s..end], loc(&value[s..end])))
                .collect();
            if let Some(t) = parsed
                .iter()
                .find(|t| matches!(t.kind, TagKind::SheetLoop(_)))
            {
                diags.push(diag(
                    format!("'{}' cannot sit in alt text; put it in a cell", t.text),
                    &t.loc,
                ));
                continue;
            }
            if !balanced(&parsed) {
                diags.push(diag(
                    "a block in alt text must open and close in the same alt text".into(),
                    &parsed[0].loc,
                ));
                continue;
            }
            e.remove_attr(attr);
            let (open, close) = if format == Format::Docx {
                // A newline in a value prints `</w:t><w:br/><w:t …>`, which
                // needs a `w:t` around it to stay well formed.
                (
                    format!("<w:t xml:space=\"preserve\">{ALT_OPEN}{attr}{ALT_SEP}"),
                    format!("{ALT_CLOSE}</w:t>"),
                )
            } else {
                (format!("{ALT_OPEN}{attr}{ALT_SEP}"), ALT_CLOSE.to_string())
            };
            e.after.push(Piece::Lit(open));
            let mut last = 0;
            for (&(s, end), tag) in found.iter().zip(parsed) {
                if s > last {
                    e.after.push(Piece::Lit(xml::escape_text(&value[last..s])));
                }
                let id = tags.len();
                tags.push(tag);
                ctxs.push(TagCtx {
                    part,
                    text: path.clone(),
                    para: path.clone(),
                    row: None,
                    table: None,
                });
                alt.insert(id);
                e.after.push(Piece::Tag(id));
                last = end;
            }
            if last < value.len() {
                e.after.push(Piece::Lit(xml::escape_text(&value[last..])));
            }
            e.after.push(Piece::Lit(close));
        }
    }
}

fn diag(message: String, loc: &Location) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        message,
        location: Some(loc.clone()),
    }
}

static DROP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\bdrop\s*\(\s*["']([^"']*)["']"#).expect("valid"));

/// Checks every `drop("…")` whose element is written in the tag against
/// where the tag sits, so that a misplaced one is reported at compile time
/// with the tag named. A name that comes from data is checked when the
/// value is placed.
pub fn check_tags(
    format: Format,
    entries: &[Entry],
    parts: &[PartTree],
    tags: &[Tag],
    ctxs: &[TagCtx],
    alt: &HashSet<TagId>,
    diags: &mut Vec<Diagnostic>,
) {
    // A drawing is compiled once, outside the sheet loop that copies its
    // sheet, so its alt text cannot be read per copy.
    if format == Format::Xlsx {
        for &id in alt {
            let part = &parts[ctxs[id].part].name;
            let looped = sheets_owning(entries, part).iter().any(|sheet| {
                parts
                    .iter()
                    .position(|p| &p.name == sheet)
                    .is_some_and(|i| {
                        tags.iter().enumerate().any(|(t, tag)| {
                            ctxs[t].part == i && matches!(tag.kind, TagKind::SheetLoop(_))
                        })
                    })
            });
            if looped {
                diags.push(diag(
                    format!(
                        "'{}' sits in a drawing on a sheet that a sheet loop repeats, where \
                         alt text is not read",
                        tags[id].text
                    ),
                    &tags[id].loc,
                ));
            }
        }
    }
    for (id, tag) in tags.iter().enumerate() {
        if !tag.text.contains("drop") {
            continue;
        }
        for cap in DROP_RE.captures_iter(&tag.text) {
            let Some(element) = Element::parse(&cap[1]) else {
                diags.push(diag(
                    format!(
                        "'{}': {:?} is not something drop removes; use {}",
                        tag.text,
                        &cap[1],
                        element_list()
                    ),
                    &tag.loc,
                ));
                continue;
            };
            let ctx = &ctxs[id];
            let tree = &parts[ctx.part];
            let in_alt = alt.contains(&id) || is_odf_alt(&tree.nodes, &ctx.para);
            let checked = element
                .supported(format)
                .and_then(|_| placement(format, element, tree, ctx, in_alt));
            if let Err(m) = checked {
                diags.push(diag(format!("'{}': {m}", tag.text), &tag.loc));
            }
        }
    }
}

fn sheets_owning(entries: &[Entry], drawing: &str) -> Vec<String> {
    entries
        .iter()
        .filter(|e| e.name.starts_with("xl/worksheets/_rels/"))
        .filter_map(|e| {
            let sheet = e
                .name
                .replace("_rels/", "")
                .strip_suffix(".rels")?
                .to_string();
            package::relationships(entries, &e.name, "xl/worksheets")
                .iter()
                .any(|(_, _, t)| t == drawing)
                .then_some(sheet)
        })
        .collect()
}

fn is_odf_alt(nodes: &[Node], path: &[usize]) -> bool {
    elem_at(nodes, path).is_some_and(|e| matches!(e.name.as_str(), "svg:title" | "svg:desc"))
}

/// Where a drop of `element` may sit, as the compiler sees the tag.
fn placement(
    format: Format,
    element: Element,
    tree: &PartTree,
    ctx: &TagCtx,
    in_alt: bool,
) -> Result<(), String> {
    let nodes = &tree.nodes;
    let what = element.name();
    let not_alt = || -> Result<(), String> {
        if in_alt {
            Err(format!(
                "sits in alt text, where drop(\"{what}\") has no {what} to remove; \
                 put it in the {what}'s own text"
            ))
        } else {
            Ok(())
        }
    };
    match element {
        Element::Paragraph => not_alt(),
        Element::Row | Element::Column => {
            not_alt()?;
            if ctx.row.is_none() {
                return Err(format!("drop(\"{what}\") must sit in a table cell"));
            }
            Ok(())
        }
        Element::Table => {
            if ctx.table.is_some() && !in_alt {
                return Ok(());
            }
            match object_of(format, nodes, &ctx.para) {
                Some((_, Kind::Table)) if in_alt => Ok(()),
                _ => Err("drop(\"table\") must sit in a cell of the table".into()),
            }
        }
        Element::Picture | Element::Chart | Element::Shape => {
            if format == Format::Xlsx && tree.name.starts_with("xl/worksheets/") {
                return Err(format!(
                    "in a sheet, drop(\"{what}\") goes in the alt text of the {what}"
                ));
            }
            let Some((_, kind)) = object_of(format, nodes, &ctx.para) else {
                return Err(format!(
                    "drop(\"{what}\") must sit in the alt text (title or description) of a {what}{}",
                    if element == Element::Shape { " or in its text" } else { "" }
                ));
            };
            check_kind(element, kind, in_alt)
        }
        Element::Slide => {
            if format == Format::Odp && nearest(nodes, &ctx.para, &["draw:page"]).is_none() {
                return Err("drop(\"slide\") must sit on a slide".into());
            }
            Ok(())
        }
        Element::Sheet => {
            if format == Format::Xlsx && !tree.name.starts_with("xl/worksheets/") {
                return Err("drop(\"sheet\") must sit in a cell of the sheet".into());
            }
            if format == Format::Ods && ods_sheet(nodes, &ctx.para).is_none() {
                return Err("drop(\"sheet\") must sit in the sheet".into());
            }
            Ok(())
        }
    }
}

fn check_kind(element: Element, kind: Kind, in_alt: bool) -> Result<(), String> {
    let want = match element {
        Element::Picture => Kind::Picture,
        Element::Chart => Kind::Chart,
        _ => return Ok(()),
    };
    if kind != want {
        return Err(format!(
            "sits in {}, not a {}",
            kind.describe(),
            element.name()
        ));
    }
    if !in_alt {
        return Err(format!(
            "drop(\"{}\") goes in the alt text of the {}",
            element.name(),
            element.name()
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Finding things in a tree.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Picture,
    Chart,
    Table,
    Shape,
}

impl Kind {
    fn describe(self) -> &'static str {
        match self {
            Kind::Picture => "a picture",
            Kind::Chart => "a chart",
            Kind::Table => "a table",
            Kind::Shape => "a shape",
        }
    }
}

fn name_at(nodes: &[Node], path: &[usize]) -> Option<String> {
    elem_at(nodes, path).map(|e| e.name.clone())
}

/// The deepest element on `path` (itself included) with one of these names.
fn nearest(nodes: &[Node], path: &[usize], names: &[&str]) -> Option<Path> {
    (1..=path.len())
        .rev()
        .find(|&k| elem_at(nodes, &path[..k]).is_some_and(|e| names.contains(&e.name.as_str())))
        .map(|k| path[..k].to_vec())
}

fn graphic_uri(e: &Elem) -> String {
    e.find("a:graphicData")
        .and_then(|g| g.attr("uri"))
        .unwrap_or_default()
}

fn kind_of_graphic(e: &Elem) -> Kind {
    let uri = graphic_uri(e);
    if uri.ends_with("/picture") {
        Kind::Picture
    } else if uri.ends_with("/chart") || uri.contains("chartex") {
        Kind::Chart
    } else if uri.ends_with("/table") {
        Kind::Table
    } else {
        Kind::Shape
    }
}

const PPTX_OBJECTS: [&str; 6] = [
    "p:pic",
    "p:graphicFrame",
    "p:sp",
    "p:cxnSp",
    "p:grpSp",
    "p:contentPart",
];
const XLSX_OBJECTS: [&str; 5] = [
    "xdr:pic",
    "xdr:graphicFrame",
    "xdr:sp",
    "xdr:cxnSp",
    "xdr:grpSp",
];
const ODF_OBJECTS: [&str; 15] = [
    "draw:frame",
    "draw:custom-shape",
    "draw:rect",
    "draw:ellipse",
    "draw:circle",
    "draw:line",
    "draw:polyline",
    "draw:polygon",
    "draw:regular-polygon",
    "draw:path",
    "draw:connector",
    "draw:measure",
    "draw:caption",
    "draw:g",
    "draw:control",
];

/// Steps out of `mc:AlternateContent`, which offers one object in two forms.
fn outside_alternates(nodes: &[Node], mut path: Path) -> Path {
    if path.len() >= 2
        && matches!(
            name_at(nodes, &path[..path.len() - 1]).as_deref(),
            Some("mc:Choice" | "mc:Fallback")
        )
    {
        path.truncate(path.len() - 2);
    }
    path
}

/// The object a path sits in, as the path of what to remove, and its kind.
fn object_of(format: Format, nodes: &[Node], path: &[usize]) -> Option<(Path, Kind)> {
    match format {
        Format::Docx => {
            let at = nearest(nodes, path, &["w:drawing", "w:pict"])?;
            let e = elem_at(nodes, &at)?;
            let kind = if e.name == "w:pict" {
                Kind::Shape
            } else {
                match kind_of_graphic(e) {
                    Kind::Table => Kind::Shape,
                    k => k,
                }
            };
            Some((outside_alternates(nodes, at), kind))
        }
        Format::Pptx => {
            let at = nearest(nodes, path, &PPTX_OBJECTS)?;
            let e = elem_at(nodes, &at)?;
            let kind = match e.name.as_str() {
                "p:pic" => Kind::Picture,
                "p:graphicFrame" => kind_of_graphic(e),
                _ => Kind::Shape,
            };
            Some((outside_alternates(nodes, at), kind))
        }
        Format::Xlsx => {
            let at = nearest(nodes, path, &XLSX_OBJECTS)?;
            let e = elem_at(nodes, &at)?;
            let kind = match e.name.as_str() {
                "xdr:pic" => Kind::Picture,
                "xdr:graphicFrame" => kind_of_graphic(e),
                _ => Kind::Shape,
            };
            let mut target = outside_alternates(nodes, at);
            if target.len() >= 2
                && name_at(nodes, &target[..target.len() - 1])
                    .is_some_and(|n| n.ends_with("Anchor"))
            {
                target.pop();
            }
            Some((target, kind))
        }
        Format::Odt | Format::Ods | Format::Odp => {
            let at = nearest(nodes, path, &ODF_OBJECTS)?;
            let e = elem_at(nodes, &at)?;
            let kind = if e.name != "draw:frame" {
                Kind::Shape
            } else if e.child("draw:object").is_some() {
                Kind::Chart
            } else if e.child("draw:image").is_some() {
                Kind::Picture
            } else if e.child("table:table").is_some() {
                Kind::Table
            } else {
                Kind::Shape
            };
            Some((at, kind))
        }
    }
}

/// The path of the ODS sheet (`table:table` under `office:spreadsheet`)
/// holding `path`.
fn ods_sheet(nodes: &[Node], path: &[usize]) -> Option<Path> {
    (2..=path.len()).find_map(|k| {
        let e = elem_at(nodes, &path[..k])?;
        (e.name == "table:table"
            && name_at(nodes, &path[..k - 1]).as_deref() == Some("office:spreadsheet"))
        .then(|| path[..k].to_vec())
    })
}

fn children_mut<'a>(nodes: &'a mut Vec<Node>, parent: &[usize]) -> Option<&'a mut Vec<Node>> {
    if parent.is_empty() {
        return Some(nodes);
    }
    elem_at_mut(nodes, parent).map(|e| &mut e.children)
}

fn attr_u32(e: &Elem, name: &str) -> Option<u32> {
    e.attr(name).and_then(|v| v.parse().ok())
}

// ---------------------------------------------------------------------------
// Placing: whole slides and sheets.

/// Whether rendered content holds a drop of this element.
pub fn dropped(table: Option<&Table>, content: &str, element: Element) -> bool {
    let Some(table) = table else { return false };
    if !content.contains(MARK_OPEN) {
        return false;
    }
    let items = table.items();
    spans(content).is_ok_and(|found| {
        found.iter().any(|&(_, _, id)| {
            matches!(items.get(id).map(|r| &**r), Some(Rich::Drop(d)) if d.element == element)
        })
    })
}

/// Every part `part` reaches through relationships, itself left out.
pub fn owned_parts(entries: &[Entry], part: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut todo = vec![part.to_string()];
    while let Some(p) = todo.pop() {
        let base = p.rsplit_once('/').map_or("", |(d, _)| d).to_string();
        for (_, ty, target) in package::relationships(entries, &package::rels_name(&p), &base) {
            if ty.ends_with("/hyperlink") || package::get(entries, &target).is_none() {
                continue;
            }
            if target != part && !out.contains(&target) {
                out.push(target.clone());
                todo.push(target);
            }
        }
    }
    out
}

/// Leaves out the slides that hold `drop("slide")`, and lists the parts
/// they may leave unused, when any went.
#[allow(clippy::type_complexity)]
pub fn drop_slides(
    entries: &[Entry],
    outputs: &[(String, String)],
    table: Option<&Table>,
) -> (Vec<(String, String)>, Option<Vec<String>>) {
    let (kept, gone): (Vec<_>, Vec<_>) = outputs
        .iter()
        .cloned()
        .partition(|(_, c)| !dropped(table, c, Element::Slide));
    if gone.is_empty() {
        return (kept, None);
    }
    let mut orphans = Vec::new();
    for (name, _) in &gone {
        if !kept.iter().any(|(n, _)| n == name) {
            orphans.extend(owned_parts(entries, name));
        }
    }
    (kept, Some(orphans))
}

/// Leaves out the sheets that hold `drop("sheet")`, with the drawings only
/// they own, and lists the parts they may leave unused. `compiled` names the
/// parts the template compiled.
#[allow(clippy::type_complexity)]
pub fn drop_sheets(
    entries: &[Entry],
    compiled: &[&str],
    outputs: Vec<(String, String)>,
    table: Option<&Table>,
) -> Result<(Vec<(String, String)>, Vec<String>), TemplateError> {
    let is_dropped = |c: &str| dropped(table, c, Element::Sheet);
    if !outputs.iter().any(|(_, c)| is_dropped(c)) {
        return Ok((outputs, Vec::new()));
    }
    let (kept, gone): (Vec<_>, Vec<_>) = outputs.into_iter().partition(|(_, c)| !is_dropped(c));
    let sheets = crate::workbook::xlsx_sheets(entries);
    let survives = |part: &str| !compiled.contains(&part) || kept.iter().any(|(n, _)| n == part);
    if !sheets.iter().any(|(_, part)| survives(part)) {
        return Err(TemplateError::new(
            "drop(\"sheet\") removes every sheet, and a workbook needs one",
        ));
    }
    let kept_parts: Vec<String> = sheets
        .iter()
        .filter(|(_, p)| survives(p))
        .map(|(_, p)| p.clone())
        .collect();
    let mut orphans = Vec::new();
    for (part, content) in &gone {
        if survives(part) {
            continue;
        }
        // A copy named from data cannot be named by another sheet.
        if !content.contains(crate::workbook::TITLE_MARK) {
            if let Some((name, _)) = sheets.iter().find(|(_, p)| p == part) {
                check_sheet_refs(entries, name, &kept_parts)?;
            }
        }
        orphans.extend(owned_parts(entries, part));
    }
    let kept = kept
        .into_iter()
        .filter(|(name, _)| !orphans.contains(name))
        .collect();
    Ok((kept, orphans))
}

/// After dropped slides left the presentation: refuses an empty one, removes
/// what only they used, and the section and custom show entries that named
/// them.
pub fn after_slides(entries: &mut Vec<Entry>, gone: &[String]) -> Result<(), TemplateError> {
    if package::slide_order(entries).is_empty() {
        return Err(TemplateError::new(
            "drop(\"slide\") removes every slide, and a presentation needs one",
        ));
    }
    gc(entries, gone);
    let Some(src) = package::text_of(entries, "ppt/presentation.xml") else {
        return Ok(());
    };
    let Ok(mut nodes) = xml::parse(&src) else {
        return Ok(());
    };
    let Some(root) = xml::root_mut(&mut nodes) else {
        return Ok(());
    };
    let ids: HashSet<String> = root
        .find("p:sldIdLst")
        .map(|l| {
            l.children_named("p:sldId")
                .filter_map(|e| e.attr("id"))
                .collect()
        })
        .unwrap_or_default();
    let rids: HashSet<String> = root
        .find("p:sldIdLst")
        .map(|l| {
            l.children_named("p:sldId")
                .filter_map(|e| e.attr("r:id"))
                .collect()
        })
        .unwrap_or_default();
    fn prune(e: &mut Elem, ids: &HashSet<String>, rids: &HashSet<String>) {
        e.children.retain(|n| match n {
            Node::Elem(c) if c.name == "p14:sldId" => {
                c.attr("id").is_some_and(|i| ids.contains(&i))
            }
            Node::Elem(c) if c.name == "p:sld" => c.attr("r:id").is_some_and(|i| rids.contains(&i)),
            _ => true,
        });
        for n in e.children.iter_mut() {
            if let Node::Elem(c) = n {
                prune(c, ids, rids);
            }
        }
    }
    prune(root, &ids, &rids);
    let out = xml::to_xml(&nodes);
    if out != src {
        package::set(entries, "ppt/presentation.xml", out.into_bytes());
    }
    Ok(())
}

/// Refuses dropping a sheet that formulas or names elsewhere still point at.
fn check_sheet_refs(
    entries: &[Entry],
    name: &str,
    kept_parts: &[String],
) -> Result<(), TemplateError> {
    let quoted = format!("'{}'!", name.replace('\'', "''"));
    let plain = format!("{name}!");
    let names = |text: &str| {
        text.contains(&quoted)
            || text.match_indices(&plain).any(|(at, _)| {
                at == 0
                    || !text[..at]
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
            })
    };
    let refuse = |place: &str| {
        Err(TemplateError::new(format!(
            "drop(\"sheet\") removes sheet {name:?}, but {place} points at it; remove that \
             reference or keep the sheet"
        )))
    };
    for part in kept_parts {
        let Some(src) = package::text_of(entries, part) else {
            continue;
        };
        if !src.contains(name) {
            continue;
        }
        for f in F_RE.captures_iter(&src) {
            if names(&xml::unescape(&f[1])) {
                return refuse(&format!("a formula in {part}"));
            }
        }
    }
    if let Some(wb) = package::text_of(entries, "xl/workbook.xml") {
        for d in DEFINED_RE.captures_iter(&wb) {
            if !d[1].contains("localSheetId") && names(&xml::unescape(&d[2])) {
                return refuse("a defined name");
            }
        }
    }
    Ok(())
}

static F_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<f(?:\s[^>]*)?>([^<]*)</f>").expect("valid"));
static DEFINED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<definedName([^>]*)>([^<]*)</definedName>").expect("valid"));

// ---------------------------------------------------------------------------
// Placing: everything else, part by part.

/// What removals leave behind to clean up once every part is written.
#[derive(Default)]
pub struct Leftovers {
    /// `(part, relationship id)` that a removed element used.
    rels: Vec<(String, String)>,
    /// ODF package paths a removed element linked to.
    hrefs: Vec<String>,
}

/// A drop found in a part: where its marker sits.
struct Found {
    path: Path,
    drop: Drop,
    in_alt: bool,
}

struct Walk<'a> {
    items: &'a [Arc<Rich>],
    part: &'a str,
    found: Vec<Found>,
    in_alt: bool,
    names: Vec<String>,
}

impl Walk<'_> {
    fn nodes(&mut self, nodes: &mut [Node], prefix: &mut Path) -> Result<(), TemplateError> {
        for (i, n) in nodes.iter_mut().enumerate() {
            prefix.push(i);
            match n {
                Node::Elem(e) => {
                    self.names.push(e.name.clone());
                    self.nodes(&mut e.children, prefix)?;
                    self.names.pop();
                }
                Node::Text(t) => {
                    if t.raw.contains(MARK_OPEN)
                        || t.raw.contains(ALT_OPEN)
                        || t.raw.contains(ALT_CLOSE)
                    {
                        self.text(&mut t.raw, prefix)?;
                    }
                }
                Node::Raw(_) => {}
            }
            prefix.pop();
        }
        Ok(())
    }

    fn text(&mut self, raw: &mut String, path: &Path) -> Result<(), TemplateError> {
        let segs = segments(raw).map_err(|m| misplaced(self.part, "", m))?;
        let odf_alt = self
            .names
            .iter()
            .any(|n| n == "svg:title" || n == "svg:desc");
        let mut out = String::new();
        let mut changed = false;
        for seg in segs {
            match seg {
                Seg::Text(s) => {
                    if let Some(i) = s.rfind([ALT_OPEN, ALT_CLOSE]) {
                        self.in_alt = s[i..].starts_with(ALT_OPEN);
                    }
                    out.push_str(s);
                }
                Seg::Mark(id) => {
                    let in_alt = self.in_alt || odf_alt;
                    match self.items.get(id).map(|r| &**r) {
                        Some(Rich::Drop(drop)) => {
                            self.found.push(Found {
                                path: path.clone(),
                                drop: *drop,
                                in_alt,
                            });
                            changed = true;
                        }
                        _ if in_alt => {
                            return Err(misplaced(
                                self.part,
                                "",
                                "alt text can hold drop() but no other rich value",
                            ))
                        }
                        _ => out.push_str(&super::marker(id)),
                    }
                }
            }
        }
        if changed {
            *raw = out;
        }
        Ok(())
    }
}

/// Removes what the drops in one rendered part name.
pub fn apply_part(
    format: Format,
    pkg: &mut Pkg,
    items: &[Arc<Rich>],
    part: &str,
    nodes: &mut Vec<Node>,
    left: &mut Leftovers,
) -> Result<(), TemplateError> {
    if !items.iter().any(|r| matches!(**r, Rich::Drop(_))) {
        return Ok(());
    }
    let mut walk = Walk {
        items,
        part,
        found: Vec::new(),
        in_alt: false,
        names: Vec::new(),
    };
    walk.nodes(nodes, &mut Vec::new())?;
    let found = walk.found;
    if found.is_empty() {
        return Ok(());
    }
    let fail = |m: String| misplaced(part, "", m);
    if format == Format::Xlsx && part.starts_with("xl/worksheets/") {
        return crate::rich::drop::xlsx::apply(pkg, part, nodes, &found, left);
    }
    let mut ops: Vec<Op> = Vec::new();
    let mut ods_edits: HashMap<Path, (BTreeSet<u32>, BTreeSet<u32>)> = HashMap::new();
    for f in &found {
        let what = f.drop.element.name();
        if f.in_alt
            && matches!(
                f.drop.element,
                Element::Paragraph | Element::Row | Element::Column
            )
        {
            return Err(fail(format!(
                "drop(\"{what}\") sits in alt text, where it has no {what} to remove"
            )));
        }
        // Rows and columns of an ODS sheet move formulas, as in XLSX.
        if format == Format::Ods && matches!(f.drop.element, Element::Row | Element::Column) {
            let sheet = ods_sheet(nodes, &f.path)
                .ok_or_else(|| fail(format!("drop(\"{what}\") must sit in a cell")))?;
            let (row, col) = ods::position(nodes, &sheet, &f.path)
                .ok_or_else(|| fail(format!("drop(\"{what}\") must sit in a cell")))?;
            let edit = ods_edits.entry(sheet).or_default();
            if f.drop.element == Element::Row {
                edit.0.extend(row..row + f.drop.count);
            } else {
                edit.1.insert(col);
            }
            continue;
        }
        ops.push(resolve(format, nodes, f).map_err(fail)?);
    }
    if !ods_edits.is_empty() {
        ods::apply(nodes, ods_edits).map_err(fail)?;
    }
    run_ops(format, pkg, part, nodes, ops, left).map_err(fail)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Op {
    Remove {
        path: Path,
        count: u32,
        element: Element,
    },
    Column {
        table: Path,
        col: u32,
    },
}

impl Op {
    fn anchor(&self) -> &Path {
        match self {
            Op::Remove { path, .. } => path,
            Op::Column { table, .. } => table,
        }
    }
}

const PARAGRAPHS: [&str; 4] = ["w:p", "a:p", "text:p", "text:h"];
const ROWS: [&str; 3] = ["w:tr", "a:tr", "table:table-row"];
const TABLES: [&str; 3] = ["w:tbl", "a:tbl", "table:table"];
const CELLS: [&str; 4] = [
    "w:tc",
    "a:tc",
    "table:table-cell",
    "table:covered-table-cell",
];

/// The removal one drop asks for.
fn resolve(format: Format, nodes: &[Node], f: &Found) -> Result<Op, String> {
    let what = f.drop.element.name();
    let remove = |path: Path| Op::Remove {
        path,
        count: f.drop.count,
        element: f.drop.element,
    };
    match f.drop.element {
        Element::Paragraph => nearest(nodes, &f.path, &PARAGRAPHS)
            .map(remove)
            .ok_or_else(|| "drop(\"paragraph\") must sit in a paragraph".into()),
        Element::Row => nearest(nodes, &f.path, &ROWS)
            .map(remove)
            .ok_or_else(|| "drop(\"row\") must sit in a table cell".into()),
        Element::Column => {
            let cell = nearest(nodes, &f.path, &CELLS)
                .ok_or_else(|| "drop(\"column\") must sit in a table cell".to_string())?;
            let table = nearest(nodes, &cell, &TABLES)
                .ok_or_else(|| "drop(\"column\") must sit in a table cell".to_string())?;
            let col = tables::column_of(nodes, &cell)?;
            Ok(Op::Column { table, col })
        }
        Element::Table => {
            if !f.in_alt {
                if let Some(t) = nearest(nodes, &f.path, &TABLES) {
                    return Ok(remove(table_holder(format, nodes, t)));
                }
            }
            match object_of(format, nodes, &f.path) {
                Some((path, Kind::Table)) if f.in_alt => Ok(remove(path)),
                _ => Err("drop(\"table\") must sit in a cell of the table".into()),
            }
        }
        Element::Picture | Element::Chart | Element::Shape => {
            let (path, kind) = object_of(format, nodes, &f.path)
                .ok_or_else(|| format!("drop(\"{what}\") must sit in the alt text of a {what}"))?;
            check_kind(f.drop.element, kind, f.in_alt)?;
            Ok(remove(path))
        }
        Element::Slide => nearest(nodes, &f.path, &["draw:page"])
            .map(remove)
            .ok_or_else(|| "drop(\"slide\") must sit on a slide".into()),
        Element::Sheet => ods_sheet(nodes, &f.path)
            .map(remove)
            .ok_or_else(|| "drop(\"sheet\") must sit in a sheet".into()),
    }
}

/// What holds a table on a page: its graphic frame in PPTX, its frame in
/// ODP, the table itself elsewhere.
fn table_holder(format: Format, nodes: &[Node], table: Path) -> Path {
    match format {
        Format::Pptx => nearest(nodes, &table, &["p:graphicFrame"])
            .map(|p| outside_alternates(nodes, p))
            .unwrap_or(table),
        Format::Odp | Format::Odt => {
            if table.len() >= 2
                && name_at(nodes, &table[..table.len() - 1]).as_deref() == Some("draw:frame")
            {
                table[..table.len() - 1].to_vec()
            } else {
                table
            }
        }
        _ => table,
    }
}

/// Applies the removals deepest and last first, so that a path never moves
/// before its own removal runs.
fn run_ops(
    format: Format,
    pkg: &mut Pkg,
    part: &str,
    nodes: &mut Vec<Node>,
    mut ops: Vec<Op>,
    left: &mut Leftovers,
) -> Result<(), String> {
    ops.sort_by(|a, b| {
        let rank = |o: &Op| match o {
            Op::Column { col, .. } => (1, *col),
            Op::Remove { .. } => (0, 0),
        };
        (b.anchor(), rank(b)).cmp(&(a.anchor(), rank(a)))
    });
    ops.dedup();
    let mut removed: Vec<Path> = Vec::new();
    for op in ops {
        if removed.iter().any(|r| op.anchor().starts_with(r)) {
            continue;
        }
        match op {
            Op::Remove {
                path,
                count,
                element,
            } => {
                remove(format, pkg, part, nodes, &path, count, element, left)?;
                removed.push(path);
            }
            Op::Column { table, col } => {
                if tables::remove_column(format, nodes, &table, col, part, left)? {
                    let holder = table_holder(format, nodes, table);
                    remove(format, pkg, part, nodes, &holder, 1, Element::Table, left)?;
                    removed.push(holder);
                }
            }
        }
    }
    Ok(())
}

/// Records the relationships and links a subtree uses, before it goes.
fn collect_refs(node: &Node, part: &str, left: &mut Leftovers) {
    let Node::Elem(e) = node else { return };
    for (k, v) in e.attrs() {
        if k.starts_with("r:") && !v.is_empty() {
            left.rels
                .push((part.to_string(), xml::unescape(&v).into_owned()));
        } else if k == "xlink:href" {
            let v = xml::unescape(&v).into_owned();
            if !v.contains("://") && !v.starts_with('#') && !v.starts_with("mailto:") {
                left.hrefs.push(v.trim_start_matches("./").to_string());
            }
        }
    }
    for c in &e.children {
        collect_refs(c, part, left);
    }
}

/// Elements a run of paragraphs or rows cannot be counted across.
const BARRIERS: [&str; 7] = [
    "w:tbl",
    "w:sectPr",
    "w:sdt",
    "table:table",
    "text:list",
    "text:section",
    "a:tbl",
];

#[allow(clippy::too_many_arguments)]
fn remove(
    format: Format,
    pkg: &mut Pkg,
    part: &str,
    nodes: &mut Vec<Node>,
    path: &[usize],
    count: u32,
    element: Element,
    left: &mut Leftovers,
) -> Result<(), String> {
    let (&idx, parent) = path.split_last().ok_or("nothing to remove")?;
    let parent = parent.to_vec();
    let siblings: &[&str] = match element {
        Element::Paragraph => &PARAGRAPHS,
        Element::Row => &ROWS,
        _ => &[],
    };
    if element == Element::Row {
        tables::check_row_merges(nodes, &parent, idx, count)?;
    }
    if element == Element::Slide {
        let pages = elem_at(nodes, &parent)
            .map(|e| e.children_named("draw:page").count())
            .unwrap_or(0);
        if pages <= 1 {
            return Err("drop(\"slide\") removes every slide, and a presentation needs one".into());
        }
    }
    if element == Element::Sheet {
        let sheets = elem_at(nodes, &parent)
            .map(|e| e.children_named("table:table").count())
            .unwrap_or(0);
        if sheets <= 1 {
            return Err("drop(\"sheet\") removes every sheet, and a workbook needs one".into());
        }
    }
    let kids = children_mut(nodes, &parent).ok_or("the element went missing")?;
    let mut take = vec![idx];
    let mut i = idx + 1;
    while take.len() < count as usize {
        let Some(n) = kids.get(i) else {
            return Err(format!(
                "drop(\"{}\", {count}) needs {count} {}s in a row, but only {} are there",
                element.name(),
                element.name(),
                take.len()
            ));
        };
        if let Node::Elem(e) = n {
            if siblings.contains(&e.name.as_str()) {
                take.push(i);
            } else if BARRIERS.contains(&e.name.as_str()) {
                let what = match e.name.as_str() {
                    "w:sectPr" => "the end of the section",
                    "w:sdt" => "a content control",
                    "text:list" => "a list",
                    "text:section" => "a section",
                    _ => "a table",
                };
                return Err(format!(
                    "drop(\"{}\", {count}) needs {count} {}s in a row, but only {} come before {what}",
                    element.name(),
                    element.name(),
                    take.len()
                ));
            }
        }
        i += 1;
    }
    for &t in take.iter().rev() {
        // A Word paragraph that ends a section keeps its properties, or the
        // section's page setup would go with it.
        if let Node::Elem(e) = &mut kids[t] {
            if e.name == "w:p"
                && e.child("w:pPr")
                    .is_some_and(|p| p.child("w:sectPr").is_some())
            {
                for c in &e.children {
                    if !matches!(c, Node::Elem(p) if p.name == "w:pPr") {
                        collect_refs(c, part, left);
                    }
                }
                e.children
                    .retain(|c| matches!(c, Node::Elem(p) if p.name == "w:pPr"));
                continue;
            }
        }
        let gone = kids.remove(t);
        collect_refs(&gone, part, left);
    }
    fixup(format, pkg, part, nodes, &parent, left)
}

/// Keeps a container valid once something left it.
fn fixup(
    format: Format,
    pkg: &mut Pkg,
    part: &str,
    nodes: &mut Vec<Node>,
    parent: &[usize],
    left: &mut Leftovers,
) -> Result<(), String> {
    let Some(e) = elem_at_mut(nodes, parent) else {
        return Ok(());
    };
    let has = |e: &Elem, names: &[&str]| {
        e.children
            .iter()
            .any(|n| matches!(n, Node::Elem(c) if names.contains(&c.name.as_str())))
    };
    match e.name.as_str() {
        "w:tc" | "w:txbxContent" | "w:hdr" | "w:ftr" | "w:footnote" | "w:endnote"
        | "w:sdtContent" => {
            let last_is_table = e
                .children
                .iter()
                .rev()
                .find_map(|n| match n {
                    Node::Elem(c) => Some(c.name == "w:tbl"),
                    _ => None,
                })
                .unwrap_or(false);
            if !has(e, &["w:p", "w:tbl", "w:sdt"]) || (e.name == "w:tc" && last_is_table) {
                e.make_open();
                e.children.push(Node::Raw("<w:p/>".into()));
            }
        }
        "a:txBody" | "p:txBody" | "xdr:txBody" => {
            if !has(e, &["a:p"]) {
                e.make_open();
                e.children.push(Node::Raw("<a:p/>".into()));
            }
        }
        "w:tbl" | "a:tbl" if !has(e, &["w:tr", "a:tr"]) => {
            let table = parent.to_vec();
            let holder = table_holder(format, nodes, table);
            return remove(format, pkg, part, nodes, &holder, 1, Element::Table, left);
        }
        "table:table" if e.find("table:table-row").is_none() => {
            let sheet = parent.len() >= 2
                && name_at(nodes, &parent[..parent.len() - 1]).as_deref()
                    == Some("office:spreadsheet");
            if !sheet {
                let holder = table_holder(format, nodes, parent.to_vec());
                return remove(format, pkg, part, nodes, &holder, 1, Element::Table, left);
            }
        }
        "table:table-header-rows" | "table:table-rows" | "table:table-row-group"
            if !has(e, &["table:table-row", "table:table-row-group"]) =>
        {
            return remove(format, pkg, part, nodes, parent, 1, Element::Table, left);
        }
        "text:list-item" | "text:list-header" if !has(e, &["text:p", "text:h", "text:list"]) => {
            return remove(format, pkg, part, nodes, parent, 1, Element::Table, left);
        }
        "text:list" if !has(e, &["text:list-item", "text:list-header"]) => {
            return remove(format, pkg, part, nodes, parent, 1, Element::Table, left);
        }
        _ => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// After every part: what only removed elements used.

/// Removes relationships that no element uses any more, then the parts
/// nothing points at, and ODF files nothing links to.
pub fn finish(entries: &mut Vec<Entry>, left: Leftovers) {
    let mut candidates: Vec<String> = Vec::new();
    let mut by_part: HashMap<String, HashSet<String>> = HashMap::new();
    for (part, id) in left.rels {
        by_part.entry(part).or_default().insert(id);
    }
    for (part, ids) in by_part {
        let Some(src) = package::text_of(entries, &part) else {
            continue;
        };
        let rels_name = package::rels_name(&part);
        let Some(rels) = package::text_of(entries, &rels_name) else {
            continue;
        };
        let Ok(mut nodes) = xml::parse(&rels) else {
            continue;
        };
        let base = part.rsplit_once('/').map_or("", |(d, _)| d).to_string();
        let mut changed = false;
        if let Some(root) = xml::root_mut(&mut nodes) {
            root.children.retain(|n| {
                let Node::Elem(r) = n else { return true };
                let Some(id) = r.attr("Id") else { return true };
                let used = src.contains(&format!("\"{id}\""));
                if ids.contains(&id) && !used {
                    if r.attr("TargetMode").as_deref() != Some("External") {
                        if let Some(t) = r.attr("Target") {
                            candidates.push(package::resolve(&base, &t));
                        }
                    }
                    changed = true;
                    return false;
                }
                true
            });
        }
        if changed {
            package::set(entries, &rels_name, xml::to_xml(&nodes).into_bytes());
        }
    }
    gc(entries, &candidates);
    odf_gc(entries, &left.hrefs);
}

/// Removes each candidate part that no relationship targets any more, with
/// its relationships, its content-type override and, in turn, the parts only
/// it pointed at.
pub fn gc(entries: &mut Vec<Entry>, candidates: &[String]) {
    let mut todo: Vec<String> = candidates.to_vec();
    let mut dead: HashSet<String> = HashSet::new();
    while !todo.is_empty() {
        let targeted: HashSet<String> = entries
            .iter()
            .filter(|e| e.name.ends_with(".rels") && !dead.contains(&e.name))
            .flat_map(|e| {
                let source = e.name.replace("_rels/", "");
                let source = source.strip_suffix(".rels").unwrap_or(&source);
                let base = source.rsplit_once('/').map_or("", |(d, _)| d).to_string();
                package::relationships(entries, &e.name, &base)
                    .into_iter()
                    .map(|(_, _, t)| t)
            })
            .collect();
        let mut next = Vec::new();
        for part in todo.drain(..) {
            if dead.contains(&part)
                || targeted.contains(&part)
                || package::get(entries, &part).is_none()
                || part.ends_with(".rels")
            {
                continue;
            }
            dead.insert(part.clone());
            let base = part.rsplit_once('/').map_or("", |(d, _)| d).to_string();
            let rels = package::rels_name(&part);
            for (_, _, t) in package::relationships(entries, &rels, &base) {
                next.push(t);
            }
            dead.insert(rels);
        }
        if dead.is_empty() {
            return;
        }
        todo = next;
        if todo.is_empty() {
            break;
        }
        // Re-read what is still targeted, without the parts found dead.
        let names: Vec<String> = dead.iter().cloned().collect();
        for n in &names {
            entries.retain(|e| &e.name != n);
        }
    }
    if dead.is_empty() {
        return;
    }
    entries.retain(|e| !dead.contains(&e.name));
    if let Some(src) = package::text_of(entries, "[Content_Types].xml") {
        if let Ok(mut nodes) = xml::parse(&src) {
            if let Some(root) = xml::root_mut(&mut nodes) {
                root.children.retain(|n| match n {
                    Node::Elem(e) if e.name == "Override" => !e
                        .attr("PartName")
                        .is_some_and(|p| dead.contains(p.trim_start_matches('/'))),
                    _ => true,
                });
            }
            package::set(
                entries,
                "[Content_Types].xml",
                xml::to_xml(&nodes).into_bytes(),
            );
        }
    }
}

/// Removes ODF files and object directories no XML part links to any more,
/// with their manifest entries.
fn odf_gc(entries: &mut Vec<Entry>, hrefs: &[String]) {
    if hrefs.is_empty() {
        return;
    }
    let linked = |entries: &[Entry], path: &str| {
        entries
            .iter()
            .filter(|e| e.name.ends_with(".xml") && e.name != "META-INF/manifest.xml")
            .filter(|e| !e.name.starts_with(&format!("{path}/")))
            .any(|e| {
                let text = String::from_utf8_lossy(&e.data);
                text.contains(&format!("\"{path}\"")) || text.contains(&format!("\"./{path}\""))
            })
    };
    let mut dead: Vec<String> = Vec::new();
    for href in hrefs {
        let path = href.trim_end_matches('/').to_string();
        if path.is_empty() || dead.contains(&path) || linked(entries, &path) {
            continue;
        }
        // An object's preview picture goes with it.
        if !path.contains('/') {
            let preview = format!("ObjectReplacements/{path}");
            if !linked(entries, &preview) {
                dead.push(preview);
            }
        }
        dead.push(path);
    }
    if dead.is_empty() {
        return;
    }
    let gone = |name: &str| {
        let name = name.trim_end_matches('/');
        dead.iter()
            .any(|d| name == d || name.starts_with(&format!("{d}/")))
    };
    entries.retain(|e| !gone(&e.name));
    if let Some(src) = package::text_of(entries, "META-INF/manifest.xml") {
        if let Ok(mut nodes) = xml::parse(&src) {
            if let Some(root) = xml::root_mut(&mut nodes) {
                root.children.retain(|n| match n {
                    Node::Elem(e) => !e.attr("manifest:full-path").is_some_and(|p| gone(&p)),
                    _ => true,
                });
            }
            package::set(
                entries,
                "META-INF/manifest.xml",
                xml::to_xml(&nodes).into_bytes(),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Alt text back into its attribute.

/// Puts the alt text printed after each `wp:docPr` and `*:cNvPr` back into
/// its attribute.
pub fn finish_alt(format: Format, entries: &mut [Entry]) -> Result<(), TemplateError> {
    if !matches!(format, Format::Docx | Format::Pptx | Format::Xlsx) {
        return Ok(());
    }
    for e in entries.iter_mut() {
        if !e.name.ends_with(".xml") || !e.data.contains(&(ALT_OPEN as u8)) {
            continue;
        }
        let Ok(src) = String::from_utf8(e.data.clone()) else {
            continue;
        };
        let mut nodes = xml::parse(&src)
            .map_err(|m| misplaced(&e.name, "", format!("rendered XML is malformed: {m}")))?;
        restore_alt(&mut nodes);
        e.data = xml::to_xml(&nodes).into_bytes();
    }
    Ok(())
}

fn node_text(n: &Node) -> String {
    match n {
        Node::Text(t) => xml::unescape(&t.raw).into_owned(),
        Node::Elem(e) if e.name == "w:br" || e.name == "a:br" => "\n".into(),
        Node::Elem(e) => e.text_content(),
        Node::Raw(_) => String::new(),
    }
}

fn restore_alt(nodes: &mut Vec<Node>) {
    let starts = |n: &Node| match n {
        Node::Text(t) => t.raw.contains(ALT_OPEN),
        Node::Elem(e) if e.name == "w:t" => {
            matches!(e.children.first(), Some(Node::Text(t)) if t.raw.contains(ALT_OPEN))
        }
        _ => false,
    };
    let mut i = 0;
    while i < nodes.len() {
        if starts(&nodes[i]) {
            let mut j = i;
            let mut text = String::new();
            while j < nodes.len() {
                text.push_str(&node_text(&nodes[j]));
                if text.contains(ALT_CLOSE) {
                    break;
                }
                j += 1;
            }
            let end = j.min(nodes.len() - 1);
            nodes.drain(i..=end);
            let body = text
                .split_once(ALT_OPEN)
                .map_or("", |(_, b)| b)
                .split(ALT_CLOSE)
                .next()
                .unwrap_or("");
            let (attr, value) = body.split_once(ALT_SEP).unwrap_or(("", body));
            let value: String = value
                .chars()
                .filter(|c| !crate::filters::xml_illegal(*c) || *c == '\n')
                .collect();
            if ALT_ATTRS.contains(&attr) {
                let owner = nodes[..i].iter_mut().rev().find_map(|n| match n {
                    Node::Elem(e) => Some(e),
                    _ => None,
                });
                if let Some(owner) = owner {
                    if value.trim().is_empty() {
                        owner.remove_attr(attr);
                    } else {
                        // An attribute keeps a newline only as a reference.
                        owner.set_attr(attr, &value.replace('\n', "\u{e}"));
                        owner.open = owner.open.replace('\u{e}', "&#10;");
                    }
                }
            }
            continue;
        }
        if let Node::Elem(e) = &mut nodes[i] {
            restore_alt(&mut e.children);
        }
        i += 1;
    }
}

// ---------------------------------------------------------------------------
// Tables in documents and slides: columns and merged cells.

mod tables {
    use super::*;

    fn span_of(e: &Elem) -> u32 {
        match e.name.as_str() {
            "w:tc" => e
                .child("w:tcPr")
                .and_then(|p| p.child("w:gridSpan"))
                .and_then(|g| attr_u32(g, "w:val"))
                .unwrap_or(1)
                .max(1),
            "table:table-cell" | "table:covered-table-cell" => {
                attr_u32(e, "table:number-columns-repeated")
                    .unwrap_or(1)
                    .max(1)
            }
            _ => 1,
        }
    }

    fn grid_before(row: &Elem) -> u32 {
        row.child("w:trPr")
            .and_then(|p| p.child("w:gridBefore"))
            .and_then(|g| attr_u32(g, "w:val"))
            .unwrap_or(0)
    }

    /// The grid column a cell starts at, from 0.
    pub(super) fn column_of(nodes: &[Node], cell: &[usize]) -> Result<u32, String> {
        let (&idx, row_path) = cell.split_last().ok_or("no cell")?;
        let row = elem_at(nodes, row_path).ok_or("no row")?;
        if !ROWS.contains(&row.name.as_str()) {
            return Err("drop(\"column\") must sit in a cell directly in a table row".into());
        }
        let mut col = if row.name == "w:tr" {
            grid_before(row)
        } else {
            0
        };
        for n in &row.children[..idx] {
            if let Node::Elem(e) = n {
                if CELLS.contains(&e.name.as_str()) {
                    col += span_of(e);
                } else if e.name == "w:sdt" {
                    return Err("drop(\"column\") cannot count cells in a content control".into());
                }
            }
        }
        Ok(col)
    }

    /// The rows of a table, through ODF's row groups.
    pub(super) fn rows(table: &Elem, base: &Path, out: &mut Vec<Path>) {
        for (i, n) in table.children.iter().enumerate() {
            let Node::Elem(e) = n else { continue };
            let mut p = base.clone();
            p.push(i);
            if ROWS.contains(&e.name.as_str()) {
                out.push(p);
            } else if matches!(
                e.name.as_str(),
                "table:table-header-rows" | "table:table-rows" | "table:table-row-group"
            ) {
                rows(e, &p, out);
            }
        }
    }

    fn columns(table: &Elem, base: &Path, out: &mut Vec<Path>) {
        for (i, n) in table.children.iter().enumerate() {
            let Node::Elem(e) = n else { continue };
            let mut p = base.clone();
            p.push(i);
            if e.name == "table:table-column" {
                out.push(p);
            } else if matches!(
                e.name.as_str(),
                "table:table-header-columns" | "table:table-columns" | "table:table-column-group"
            ) {
                columns(e, &p, out);
            }
        }
    }

    /// Where grid column `col` falls among `children`: the child's index and
    /// whether it covers more than that column.
    fn locate(
        children: &[Node],
        names: &[&str],
        mut at: u32,
        col: u32,
        span: impl Fn(&Elem) -> u32,
    ) -> Option<(usize, u32)> {
        for (i, n) in children.iter().enumerate() {
            let Node::Elem(e) = n else { continue };
            if !names.contains(&e.name.as_str()) {
                continue;
            }
            let s = span(e);
            if col < at + s {
                return Some((i, s));
            }
            at += s;
        }
        None
    }

    fn merged(col: u32) -> String {
        format!(
            "drop(\"column\") cannot remove column {}: a cell merged across columns spans it",
            col + 1
        )
    }

    /// Removes grid column `col` from a table. Returns true when no column
    /// is left, so the table itself should go.
    pub(super) fn remove_column(
        format: Format,
        nodes: &mut [Node],
        table: &Path,
        col: u32,
        part: &str,
        left: &mut Leftovers,
    ) -> Result<bool, String> {
        let t = elem_at(nodes, table).ok_or("the table went missing")?;
        let mut row_paths = Vec::new();
        rows(t, &Vec::new(), &mut row_paths);
        // Check every row first, so that a refusal changes nothing.
        let mut plan: Vec<(Path, Option<(usize, bool)>)> = Vec::new();
        for rp in &row_paths {
            let row = elem_at(&t.children, rp).ok_or("a row went missing")?;
            let entry = match row.name.as_str() {
                "w:tr" => {
                    let gb = grid_before(row);
                    if col < gb {
                        (rp.clone(), None)
                    } else {
                        match locate(&row.children, &["w:tc"], gb, col, span_of) {
                            Some((_, s)) if s > 1 => return Err(merged(col)),
                            Some((i, _)) => (rp.clone(), Some((i, false))),
                            None => continue,
                        }
                    }
                }
                "a:tr" => {
                    let found = locate(&row.children, &["a:tc"], 0, col, |_| 1);
                    let Some((i, _)) = found else { continue };
                    let cell = elem_at(&row.children, &[i]).ok_or("no cell")?;
                    let spans = attr_u32(cell, "gridSpan").unwrap_or(1) > 1
                        || cell.attr("hMerge").is_some_and(|v| v == "1" || v == "true");
                    if spans {
                        return Err(merged(col));
                    }
                    (rp.clone(), Some((i, false)))
                }
                _ => {
                    let names = ["table:table-cell", "table:covered-table-cell"];
                    let Some((i, s)) = locate(&row.children, &names, 0, col, span_of) else {
                        continue;
                    };
                    let cell = elem_at(&row.children, &[i]).ok_or("no cell")?;
                    if cell.name == "table:covered-table-cell"
                        || attr_u32(cell, "table:number-columns-spanned").unwrap_or(1) > 1
                    {
                        return Err(merged(col));
                    }
                    (rp.clone(), Some((i, s > 1)))
                }
            };
            plan.push(entry);
        }
        let t = elem_at_mut(nodes, table).ok_or("the table went missing")?;
        for (rp, cell) in plan {
            let row = elem_at_mut(&mut t.children, &rp).ok_or("a row went missing")?;
            match cell {
                None => {
                    // The column falls in Word's cells before the row starts.
                    if let Some(g) = row
                        .child_mut("w:trPr")
                        .and_then(|p| p.child_mut("w:gridBefore"))
                    {
                        let n = attr_u32(g, "w:val").unwrap_or(1).saturating_sub(1);
                        g.set_attr("w:val", &n.to_string());
                    }
                }
                Some((i, true)) => {
                    if let Some(Node::Elem(c)) = row.children.get_mut(i) {
                        let n = attr_u32(c, "table:number-columns-repeated").unwrap_or(2) - 1;
                        if n > 1 {
                            c.set_attr("table:number-columns-repeated", &n.to_string());
                        } else {
                            c.remove_attr("table:number-columns-repeated");
                        }
                    }
                }
                Some((i, false)) => {
                    let gone = row.children.remove(i);
                    collect_refs(&gone, part, left);
                }
            }
        }
        // The grid, and the width the table took. With no column left the
        // caller removes the table.
        let mut width_emu: Option<i64> = None;
        let mut width_twips: Option<i64> = None;
        let mut odf_width: Option<String> = None;
        let remaining;
        if let Some(grid) = t.child_mut("w:tblGrid") {
            let cols: Vec<usize> = grid
                .children
                .iter()
                .enumerate()
                .filter(|(_, n)| matches!(n, Node::Elem(e) if e.name == "w:gridCol"))
                .map(|(i, _)| i)
                .collect();
            if let Some(&i) = cols.get(col as usize) {
                if let Node::Elem(g) = grid.children.remove(i) {
                    width_twips = g.attr("w:w").and_then(|w| w.parse().ok());
                }
            }
            remaining = cols.len().saturating_sub(1);
        } else if let Some(grid) = t.child_mut("a:tblGrid") {
            let cols: Vec<usize> = grid
                .children
                .iter()
                .enumerate()
                .filter(|(_, n)| matches!(n, Node::Elem(e) if e.name == "a:gridCol"))
                .map(|(i, _)| i)
                .collect();
            if let Some(&i) = cols.get(col as usize) {
                if let Node::Elem(g) = grid.children.remove(i) {
                    width_emu = g.attr("w").and_then(|w| w.parse().ok());
                }
            }
            remaining = cols.len().saturating_sub(1);
        } else {
            let mut col_paths = Vec::new();
            columns(t, &Vec::new(), &mut col_paths);
            let mut at = 0;
            let mut total = 0;
            let mut hit: Option<(Path, u32)> = None;
            for p in &col_paths {
                let c = elem_at(&t.children, p).ok_or("no column")?;
                let n = attr_u32(c, "table:number-columns-repeated")
                    .unwrap_or(1)
                    .max(1);
                if hit.is_none() && col < at + n {
                    hit = Some((p.clone(), n));
                    odf_width = c.attr("table:style-name");
                }
                at += n;
                total += n;
            }
            if let Some((p, n)) = hit {
                if n > 1 {
                    if let Some(c) = elem_at_mut(&mut t.children, &p) {
                        let left = n - 1;
                        if left > 1 {
                            c.set_attr("table:number-columns-repeated", &left.to_string());
                        } else {
                            c.remove_attr("table:number-columns-repeated");
                        }
                    }
                } else if let Some((last, parent)) = p.split_last() {
                    let kids = if parent.is_empty() {
                        Some(&mut t.children)
                    } else {
                        elem_at_mut(&mut t.children, parent).map(|e| &mut e.children)
                    };
                    if let Some(kids) = kids {
                        kids.remove(*last);
                    }
                }
            }
            remaining = total.saturating_sub(1) as usize;
        }
        if remaining == 0 {
            return Ok(true);
        }
        if let Some(w) = width_twips {
            if let Some(tw) = t.child_mut("w:tblPr").and_then(|p| p.child_mut("w:tblW")) {
                if tw.attr("w:type").as_deref() == Some("dxa") {
                    if let Some(total) = tw.attr("w:w").and_then(|v| v.parse::<i64>().ok()) {
                        tw.set_attr("w:w", &(total - w).max(0).to_string());
                    }
                }
            }
        }
        if let Some(w) = width_emu {
            if format == Format::Pptx {
                if let Some(frame) = nearest(nodes, table, &["p:graphicFrame"]) {
                    if let Some(ext) = elem_at_mut(nodes, &frame)
                        .and_then(|f| f.child_mut("p:xfrm"))
                        .and_then(|x| x.child_mut("a:ext"))
                    {
                        if let Some(cx) = ext.attr("cx").and_then(|v| v.parse::<i64>().ok()) {
                            ext.set_attr("cx", &(cx - w).max(1).to_string());
                        }
                    }
                }
            }
        }
        if let (Some(style), Format::Odp) = (odf_width, format) {
            shrink_odp_frame(nodes, table, &style);
        }
        Ok(false)
    }

    /// Narrows the frame of an ODP table by the width of a removed column.
    fn shrink_odp_frame(nodes: &mut [Node], table: &Path, style: &str) {
        let width = xml::root(nodes)
            .and_then(|r| r.child("office:automatic-styles"))
            .and_then(|s| {
                s.children_named("style:style")
                    .find(|st| st.attr("style:name").as_deref() == Some(style))
                    .and_then(|st| st.child("style:table-column-properties"))
                    .and_then(|p| p.attr("style:column-width"))
            })
            .and_then(|w| length_cm(&w));
        let Some(width) = width else { return };
        if table.len() < 2 {
            return;
        }
        if let Some(frame) = elem_at_mut(nodes, &table[..table.len() - 1]) {
            if frame.name == "draw:frame" {
                if let Some(total) = frame.attr("svg:width").and_then(|w| length_cm(&w)) {
                    frame.set_attr("svg:width", &format!("{:.3}cm", (total - width).max(0.01)));
                }
            }
        }
    }

    fn length_cm(s: &str) -> Option<f64> {
        let s = s.trim();
        let split = s.find(|c: char| c.is_ascii_alphabetic())?;
        let n: f64 = s[..split].parse().ok()?;
        Some(match &s[split..] {
            "cm" => n,
            "mm" => n / 10.0,
            "in" => n * 2.54,
            "pt" => n * 2.54 / 72.0,
            _ => return None,
        })
    }

    /// Refuses removing rows that hold part of a cell merged down across
    /// rows that stay.
    pub(super) fn check_row_merges(
        nodes: &[Node],
        parent: &[usize],
        idx: usize,
        count: u32,
    ) -> Result<(), String> {
        let table_path = match nearest(nodes, parent, &TABLES) {
            Some(t) => t,
            None => return Ok(()),
        };
        let Some(table) = elem_at(nodes, &table_path) else {
            return Ok(());
        };
        let mut row_paths = Vec::new();
        rows(table, &Vec::new(), &mut row_paths);
        let rel_parent: Vec<usize> = parent[table_path.len()..].to_vec();
        let first = row_paths
            .iter()
            .position(|p| p[..p.len() - 1] == rel_parent[..] && p[p.len() - 1] == idx);
        let Some(first) = first else { return Ok(()) };
        let removed: std::ops::Range<usize> = first..first + count as usize;
        // Vertical merges as (column, first row, last row).
        let mut groups: Vec<(u32, usize, usize)> = Vec::new();
        let mut open: HashMap<u32, usize> = HashMap::new();
        for (r, rp) in row_paths.iter().enumerate() {
            let Some(row) = elem_at(&table.children, rp) else {
                continue;
            };
            let mut col = if row.name == "w:tr" {
                grid_before(row)
            } else {
                0
            };
            for n in &row.children {
                let Node::Elem(c) = n else { continue };
                if !CELLS.contains(&c.name.as_str()) {
                    continue;
                }
                let span = span_of(c);
                match c.name.as_str() {
                    "w:tc" => {
                        let vm = c.child("w:tcPr").and_then(|p| p.child("w:vMerge"));
                        match vm {
                            Some(v) if v.attr("w:val").as_deref() == Some("restart") => {
                                if let Some(start) = open.insert(col, r) {
                                    groups.push((col, start, r - 1));
                                }
                            }
                            Some(_) => {}
                            None => {
                                if let Some(start) = open.remove(&col) {
                                    groups.push((col, start, r - 1));
                                }
                            }
                        }
                    }
                    "a:tc" => {
                        if let Some(n) = attr_u32(c, "rowSpan").filter(|n| *n > 1) {
                            groups.push((col, r, r + n as usize - 1));
                        }
                    }
                    _ => {
                        if let Some(n) = attr_u32(c, "table:number-rows-spanned").filter(|n| *n > 1)
                        {
                            groups.push((col, r, r + n as usize - 1));
                        }
                    }
                }
                col += span;
            }
        }
        for (col, start) in open {
            groups.push((col, start, row_paths.len() - 1));
        }
        for (_, a, b) in groups {
            let touches = a < removed.end && b >= removed.start;
            let inside = a >= removed.start && b < removed.end;
            if a < b && touches && !inside {
                return Err(
                    "drop(\"row\") cannot remove a row that holds part of a cell merged down \
                     across rows that stay"
                        .into(),
                );
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// References in spreadsheets.

/// Moves cell references past removed rows and columns. A reference to a
/// removed cell is an error; a range loses the removed rows and columns,
/// and a range of only removed rows becomes the row before them, as a loop
/// over an empty list leaves it.
struct Remap<'a> {
    rows: &'a BTreeSet<u32>,
    cols: &'a BTreeSet<u32>,
    start: Option<(u32, u32)>,
    error: Option<String>,
}

impl<'a> Remap<'a> {
    fn new(rows: &'a BTreeSet<u32>, cols: &'a BTreeSet<u32>) -> Self {
        Remap {
            rows,
            cols,
            start: None,
            error: None,
        }
    }

    fn before(set: &BTreeSet<u32>, v: u32) -> u32 {
        set.range(..v).count() as u32
    }

    fn up_to(set: &BTreeSet<u32>, v: u32) -> u32 {
        set.range(..=v).count() as u32
    }

    fn fail(&mut self, col: u32, row: u32) {
        if self.error.is_none() {
            self.error = Some(format!("{}{row}", num_to_col(col)));
        }
    }

    /// For [`rewrite_refs`]: `(column, row)` of one end, from 1.
    fn map(&mut self, col: u32, row: u32, pos: RefPos) -> (u32, u32) {
        match pos {
            RefPos::Single => {
                if self.rows.contains(&row) || self.cols.contains(&col) {
                    self.fail(col, row);
                    return (col, row);
                }
                (
                    col - Self::before(self.cols, col),
                    row - Self::before(self.rows, row),
                )
            }
            RefPos::RangeStart => {
                self.start = Some((col, row));
                (
                    col - Self::before(self.cols, col),
                    row - Self::before(self.rows, row),
                )
            }
            RefPos::RangeEnd => {
                let (sc, sr) = self.start.take().unwrap_or((col, row));
                let (lo, hi) = (sc.min(col), sc.max(col));
                if !self.cols.is_empty() && (lo..=hi).all(|c| self.cols.contains(&c)) {
                    self.fail(sc, sr);
                    return (col, row);
                }
                let (rlo, rhi) = (sr.min(row), sr.max(row));
                let nr = row - Self::up_to(self.rows, row);
                if (rlo..=rhi).all(|r| self.rows.contains(&r)) && nr == 0 {
                    self.fail(sc, sr);
                    return (col, row);
                }
                (col - Self::up_to(self.cols, col), nr)
            }
        }
    }

    /// The new position of a 0-based row or column index of a drawing
    /// anchor: it moves up past every removed one before it.
    fn index(set: &BTreeSet<u32>, idx: u32) -> u32 {
        idx - Self::before(set, idx + 1).min(idx)
    }
}

static QUALIFIED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"('(?:[^']|'')+'|[A-Za-z0-9_.]+)!(\$?[A-Z]{1,3}\$?[0-9]+(?::\$?[A-Z]{1,3}\$?[0-9]+)?)",
    )
    .expect("valid")
});
static WHOLE_COLS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^A-Za-z0-9_$])\$?[A-Z]{1,3}:\$?[A-Z]{1,3}(?:[^A-Za-z0-9_(]|$)")
        .expect("valid")
});
static WHOLE_ROWS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^A-Za-z0-9_$.])\$?[0-9]+:\$?[0-9]+(?:[^A-Za-z0-9_]|$)").expect("valid")
});

fn sheet_name_of(raw: &str) -> String {
    raw.strip_prefix('\'')
        .and_then(|n| n.strip_suffix('\''))
        .map(|n| n.replace("''", "'"))
        .unwrap_or_else(|| raw.to_string())
}

/// Rewrites an A1 formula. `own` moves the references without a sheet too.
/// Returns the formula, or the cell a reference into the removed cells names.
fn rewrite_formula(
    text: &str,
    sheet: &str,
    own: bool,
    rows: &BTreeSet<u32>,
    cols: &BTreeSet<u32>,
) -> Result<String, String> {
    let names_sheet = text.contains(sheet);
    if !own && !names_sheet {
        return Ok(text.to_string());
    }
    let whole = if own || names_sheet {
        (!cols.is_empty() && WHOLE_COLS.is_match(text))
            || (!rows.is_empty() && WHOLE_ROWS.is_match(text))
    } else {
        false
    };
    if whole {
        return Err("a whole-column or whole-row reference".into());
    }
    let mut remap = Remap::new(rows, cols);
    let mut out = if own {
        rewrite_refs(text, |c, _, r, _, pos| remap.map(c, r, pos))
    } else {
        text.to_string()
    };
    if names_sheet {
        out = QUALIFIED
            .replace_all(&out, |c: &regex::Captures| {
                if sheet_name_of(&c[1]) != sheet {
                    return c[0].to_string();
                }
                let moved = rewrite_refs(&c[2], |col, _, r, _, pos| remap.map(col, r, pos));
                format!("{}!{moved}", &c[1])
            })
            .into_owned();
    }
    match remap.error {
        Some(cell) => Err(cell),
        None => Ok(out),
    }
}

/// A space-separated list of A1 ranges, such as a `sqref`, with the parts
/// that only covered removed cells left out.
fn rewrite_sqref(list: &str, rows: &BTreeSet<u32>, cols: &BTreeSet<u32>) -> String {
    list.split_whitespace()
        .filter_map(|r| {
            let range = if r.contains(':') {
                r.to_string()
            } else {
                format!("{r}:{r}")
            };
            let mut remap = Remap::new(rows, cols);
            let out = rewrite_refs(&range, |c, _, row, _, pos| remap.map(c, row, pos));
            let (a, b) = out.split_once(':')?;
            if remap.error.is_some() {
                return None;
            }
            let (_, ra) = split_ref(a.trim_start_matches('$'))?;
            let (_, rb) = split_ref(b.trim_start_matches('$'))?;
            // A range of only removed rows came back as the row before.
            let all_gone = {
                let (Some((_, r0)), Some((_, r1))) = (
                    split_ref(&range[..range.find(':')?]),
                    split_ref(&range[range.find(':')? + 1..]),
                ) else {
                    return None;
                };
                (r0..=r1).all(|x| rows.contains(&x))
            };
            if all_gone || ra == 0 || rb == 0 {
                return None;
            }
            Some(if a == b { a.to_string() } else { out })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn ref_error(what: &str, sheet: &str, place: &str, cell: String) -> String {
    if cell.contains("whole-") {
        format!(
            "{what} in sheet {sheet:?} cannot move {place}: it uses {cell}; write the range with rows, such as A1:A100"
        )
    } else {
        format!(
            "{what} in sheet {sheet:?} removes cell {cell}, which {place} points at; remove that reference or keep the cells"
        )
    }
}

mod xlsx {
    use super::*;
    use crate::rich::pkg::REL_DRAWING;

    /// Removes the rows and columns the drops in one worksheet name, and
    /// moves every reference to the cells after them.
    pub(super) fn apply(
        pkg: &mut Pkg,
        part: &str,
        nodes: &mut [Node],
        found: &[Found],
        left: &mut Leftovers,
    ) -> Result<(), TemplateError> {
        let fail = |m: String| misplaced(part, "", m);
        let mut rows = BTreeSet::new();
        let mut cols = BTreeSet::new();
        for f in found {
            let cell = nearest(nodes, &f.path, &["c"])
                .and_then(|p| elem_at(nodes, &p))
                .and_then(|c| c.attr("r"))
                .and_then(|r| split_ref(&r));
            let what = f.drop.element.name();
            let Some((col, row)) = cell else {
                return Err(fail(format!("drop(\"{what}\") must sit in a cell")));
            };
            match f.drop.element {
                Element::Row => rows.extend(row..row + f.drop.count),
                Element::Column => {
                    cols.insert(col_to_num(&col));
                }
                Element::Sheet => {}
                Element::Picture | Element::Chart | Element::Shape => {
                    return Err(fail(format!(
                        "in a sheet, drop(\"{what}\") goes in the alt text of the {what}"
                    )))
                }
                _ => {
                    return Err(fail(format!(
                        "a sheet has no {what}s to remove; use row, column or sheet"
                    )))
                }
            }
        }
        if rows.is_empty() && cols.is_empty() {
            return Ok(());
        }
        let sheet = crate::workbook::xlsx_sheets(pkg.entries)
            .into_iter()
            .find(|(_, p)| p == part)
            .map(|(n, _)| n)
            .unwrap_or_default();
        let what = match (rows.is_empty(), cols.is_empty()) {
            (false, true) => "drop(\"row\")",
            (true, false) => "drop(\"column\")",
            _ => "drop(\"row\") and drop(\"column\")",
        };
        let err = |place: &str, cell: String| fail(ref_error(what, &sheet, place, cell));
        let root = xml::root_mut(nodes).ok_or_else(|| fail("the sheet is empty".into()))?;
        let mut changed_formulas = false;

        // Cells, rows and formulas.
        if let Some(data) = root.find_mut("sheetData") {
            data.children.retain(|n| match n {
                Node::Elem(r) if r.name == "row" => {
                    !attr_u32(r, "r").is_some_and(|x| rows.contains(&x))
                }
                _ => true,
            });
            for n in data.children.iter_mut() {
                let Node::Elem(row) = n else { continue };
                if let Some(r) = attr_u32(row, "r") {
                    row.set_attr("r", &(r - Remap::before(&rows, r)).to_string());
                }
                if !cols.is_empty() {
                    row.remove_attr("spans");
                    row.children.retain(|c| match c {
                        Node::Elem(c) if c.name == "c" => !c
                            .attr("r")
                            .and_then(|r| split_ref(&r))
                            .is_some_and(|(col, _)| cols.contains(&col_to_num(&col))),
                        _ => true,
                    });
                }
                for c in row.children.iter_mut() {
                    let Node::Elem(c) = c else { continue };
                    if c.name != "c" {
                        continue;
                    }
                    let Some((col, r)) = c.attr("r").and_then(|r| split_ref(&r)) else {
                        continue;
                    };
                    let n = col_to_num(&col);
                    let at = format!(
                        "{}{}",
                        num_to_col(n - Remap::before(&cols, n)),
                        r - Remap::before(&rows, r)
                    );
                    c.set_attr("r", &at);
                    let Some(f) = c.child_mut("f") else { continue };
                    let text = f.text_content();
                    let moved = rewrite_formula(&text, &sheet, true, &rows, &cols)
                        .map_err(|cell| err(&format!("the formula in {col}{r} ({text})"), cell))?;
                    if moved != text {
                        f.make_open();
                        f.children = vec![Node::Text(xml::Text {
                            raw: xml::escape_text(&moved),
                            pieces: None,
                        })];
                        changed_formulas = true;
                    }
                    if let Some(r) = f.attr("ref") {
                        let moved = rewrite_sqref(&r, &rows, &cols);
                        if moved.is_empty() {
                            f.remove_attr("ref");
                        } else {
                            f.set_attr("ref", &moved);
                        }
                    }
                }
            }
        }

        // Column widths.
        if !cols.is_empty() {
            if let Some(list) = root.child_mut("cols") {
                for &c in cols.iter().rev() {
                    list.children.retain_mut(|n| {
                        let Node::Elem(e) = n else { return true };
                        let (Some(min), Some(max)) = (attr_u32(e, "min"), attr_u32(e, "max"))
                        else {
                            return true;
                        };
                        if min <= c && c <= max {
                            if min == max {
                                return false;
                            }
                            e.set_attr("max", &(max - 1).to_string());
                        } else if min > c {
                            e.set_attr("min", &(min - 1).to_string());
                            e.set_attr("max", &(max - 1).to_string());
                        }
                        true
                    });
                }
                if list.children.iter().all(|n| !matches!(n, Node::Elem(_))) {
                    root.children
                        .retain(|n| !matches!(n, Node::Elem(e) if e.name == "cols"));
                }
            }
        }

        // Merged cells: refused across a removed column, shrunk over rows.
        if let Some(merges) = root.child_mut("mergeCells") {
            let mut kept = Vec::new();
            for m in merges.children_named("mergeCell") {
                let Some(r) = m.attr("ref") else { continue };
                let Some((a, b)) = r.split_once(':') else {
                    continue;
                };
                let (Some((ca, ra)), Some((cb, rb))) = (split_ref(a), split_ref(b)) else {
                    continue;
                };
                let (ca, cb) = (col_to_num(&ca), col_to_num(&cb));
                if cols.iter().any(|&c| ca <= c && c <= cb) && ca != cb {
                    return Err(fail(format!(
                        "drop(\"column\") cannot remove a column of sheet {sheet:?} that the \
                         merged cells {r} span; unmerge them or keep the column"
                    )));
                }
                if ca == cb && cols.contains(&ca) {
                    continue;
                }
                if (ra..=rb).all(|x| rows.contains(&x)) {
                    continue;
                }
                // Not every row of it goes, so its end stays at or after
                // its start.
                let na = ra - Remap::before(&rows, ra);
                let nb = rb - Remap::up_to(&rows, rb);
                let ca2 = ca - Remap::before(&cols, ca);
                let cb2 = cb - Remap::before(&cols, cb);
                if na == nb && ca2 == cb2 {
                    continue;
                }
                kept.push(format!("{}{na}:{}{nb}", num_to_col(ca2), num_to_col(cb2)));
            }
            merges.children = kept
                .iter()
                .map(|r| Node::Elem(Elem::new("mergeCell", &[("ref", r)], true)))
                .collect();
            merges.set_attr("count", &kept.len().to_string());
            if kept.is_empty() {
                root.children
                    .retain(|n| !matches!(n, Node::Elem(e) if e.name == "mergeCells"));
            }
        }

        if let Some(dim) = root.child_mut("dimension") {
            if let Some(r) = dim.attr("ref") {
                let moved = rewrite_sqref(&r, &rows, &cols);
                dim.set_attr("ref", if moved.is_empty() { "A1" } else { &moved });
            }
        }
        if let Some(f) = root.child_mut("autoFilter") {
            if let Some(r) = f.attr("ref") {
                let moved = rewrite_sqref(&r, &rows, &cols);
                if moved.is_empty() {
                    root.children
                        .retain(|n| !matches!(n, Node::Elem(e) if e.name == "autoFilter"));
                } else {
                    f.set_attr("ref", &moved);
                }
            }
        }
        // The selection is only where the cursor was.
        if let Some(views) = root.child_mut("sheetViews") {
            for v in views.children.iter_mut() {
                if let Node::Elem(v) = v {
                    v.children
                        .retain(|n| !matches!(n, Node::Elem(e) if e.name == "selection"));
                }
            }
        }
        // Conditional formats and validations: their ranges move, and those
        // left with no cells go.
        let mut ranged_error: Option<TemplateError> = None;
        root.children.retain_mut(|n| {
            let Node::Elem(e) = n else { return true };
            if e.name != "conditionalFormatting" {
                return true;
            }
            let Some(sq) = e.attr("sqref") else {
                return true;
            };
            let moved = rewrite_sqref(&sq, &rows, &cols);
            if moved.is_empty() {
                return false;
            }
            e.set_attr("sqref", &moved);
            for rule in e.children.iter_mut() {
                let Node::Elem(rule) = rule else { continue };
                for f in rule.children.iter_mut() {
                    let Node::Elem(f) = f else { continue };
                    if f.name != "formula" {
                        continue;
                    }
                    let text = f.text_content();
                    match rewrite_formula(&text, &sheet, true, &rows, &cols) {
                        Ok(m) => {
                            f.children = vec![Node::Text(xml::Text {
                                raw: xml::escape_text(&m),
                                pieces: None,
                            })]
                        }
                        Err(cell) => {
                            ranged_error.get_or_insert_with(|| {
                                err(&format!("the conditional format on {sq}"), cell)
                            });
                        }
                    }
                }
            }
            true
        });
        if let Some(e) = ranged_error {
            return Err(e);
        }
        if let Some(list) = root.child_mut("dataValidations") {
            list.children.retain_mut(|n| {
                let Node::Elem(v) = n else { return true };
                let Some(sq) = v.attr("sqref") else {
                    return true;
                };
                let moved = rewrite_sqref(&sq, &rows, &cols);
                if moved.is_empty() {
                    return false;
                }
                v.set_attr("sqref", &moved);
                for f in v.children.iter_mut() {
                    let Node::Elem(f) = f else { continue };
                    if f.name != "formula1" && f.name != "formula2" {
                        continue;
                    }
                    let text = f.text_content();
                    match rewrite_formula(&text, &sheet, true, &rows, &cols) {
                        Ok(m) => {
                            f.children = vec![Node::Text(xml::Text {
                                raw: xml::escape_text(&m),
                                pieces: None,
                            })]
                        }
                        Err(cell) => {
                            ranged_error.get_or_insert_with(|| {
                                err(&format!("the data validation on {sq}"), cell)
                            });
                        }
                    }
                }
                true
            });
            if let Some(e) = ranged_error.take() {
                return Err(e);
            }
            let count = list
                .children
                .iter()
                .filter(|n| matches!(n, Node::Elem(_)))
                .count();
            list.set_attr("count", &count.to_string());
        }
        if let Some(list) = root.child_mut("hyperlinks") {
            list.children.retain_mut(|n| {
                let Node::Elem(h) = n else { return true };
                let Some(r) = h.attr("ref") else { return true };
                let moved = rewrite_sqref(&r, &rows, &cols);
                if moved.is_empty() {
                    if let Some(id) = h.attr("r:id") {
                        left.rels.push((part.to_string(), id));
                    }
                    return false;
                }
                h.set_attr("ref", &moved);
                true
            });
        }
        for brk in ["rowBreaks", "colBreaks"] {
            let set = if brk == "rowBreaks" { &rows } else { &cols };
            if set.is_empty() {
                continue;
            }
            if let Some(list) = root.child_mut(brk) {
                list.children.retain_mut(|n| {
                    let Node::Elem(b) = n else { return true };
                    let Some(id) = attr_u32(b, "id") else {
                        return true;
                    };
                    if set.contains(&id) {
                        return false;
                    }
                    b.set_attr("id", &(id - Remap::before(set, id)).to_string());
                    true
                });
            }
        }
        root.children
            .retain(|n| !matches!(n, Node::Elem(e) if e.name == "ignoredErrors"));

        // Tables in the sheet, the drawing's anchors, other sheets, names and
        // charts.
        let dir = part.rsplit_once('/').map_or("", |(d, _)| d).to_string();
        let rels = package::relationships(pkg.entries, &package::rels_name(part), &dir);
        for (_, ty, target) in &rels {
            if ty.ends_with("/table") {
                fix_table(pkg.entries, target, &rows, &cols, &sheet).map_err(fail)?;
            } else if ty == REL_DRAWING {
                fix_anchors(pkg.entries, target, &rows, &cols);
            }
        }
        for (name, other) in crate::workbook::xlsx_sheets(pkg.entries) {
            if other == part {
                continue;
            }
            let Some(src) = package::text_of(pkg.entries, &other) else {
                continue;
            };
            if !src.contains(&sheet) {
                continue;
            }
            let mut tree = xml::parse(&src).map_err(|m| misplaced(&other, "", m))?;
            let mut changed = false;
            let mut problem = None;
            visit(&mut tree, &mut |e: &mut Elem| {
                if e.name != "f" || problem.is_some() {
                    return;
                }
                let text = e.text_content();
                match rewrite_formula(&text, &sheet, false, &rows, &cols) {
                    Ok(m) if m != text => {
                        e.children = vec![Node::Text(xml::Text {
                            raw: xml::escape_text(&m),
                            pieces: None,
                        })];
                        changed = true;
                    }
                    Ok(_) => {}
                    Err(cell) => {
                        problem = Some(err(&format!("a formula in sheet {name:?} ({text})"), cell))
                    }
                }
            });
            if let Some(p) = problem {
                return Err(p);
            }
            if changed {
                changed_formulas = true;
                package::set(pkg.entries, &other, xml::to_xml(&tree).into_bytes());
            }
        }
        let mut charts: Vec<String> = pkg
            .entries
            .iter()
            .filter(|e| e.name.starts_with("xl/charts/") && e.name.ends_with(".xml"))
            .map(|e| e.name.clone())
            .collect();
        charts.push("xl/workbook.xml".into());
        for name in charts {
            let Some(src) = package::text_of(pkg.entries, &name) else {
                continue;
            };
            if !src.contains(&sheet) {
                continue;
            }
            let mut tree = xml::parse(&src).map_err(|m| misplaced(&name, "", m))?;
            let mut changed = false;
            let mut problem = None;
            visit(&mut tree, &mut |e: &mut Elem| {
                if !(e.name == "c:f" || e.name == "definedName") || problem.is_some() {
                    return;
                }
                let text = e.text_content();
                match rewrite_formula(&text, &sheet, false, &rows, &cols) {
                    Ok(m) if m != text => {
                        e.children = vec![Node::Text(xml::Text {
                            raw: xml::escape_text(&m),
                            pieces: None,
                        })];
                        changed = true;
                    }
                    Ok(_) => {}
                    Err(cell) => {
                        let place = if e.name == "c:f" {
                            format!("a chart's data ({text})")
                        } else {
                            format!("the name {:?}", e.attr("name").unwrap_or_default())
                        };
                        problem = Some(err(&place, cell));
                    }
                }
            });
            if let Some(p) = problem {
                return Err(p);
            }
            if changed {
                package::set(pkg.entries, &name, xml::to_xml(&tree).into_bytes());
            }
        }
        if changed_formulas {
            if let Some(wb) = package::text_of(pkg.entries, "xl/workbook.xml") {
                package::set(
                    pkg.entries,
                    "xl/workbook.xml",
                    crate::sheet::force_recalc(&wb).into_bytes(),
                );
            }
        }
        Ok(())
    }

    fn visit(nodes: &mut [Node], f: &mut impl FnMut(&mut Elem)) {
        for n in nodes {
            if let Node::Elem(e) = n {
                f(e);
                visit(&mut e.children, f);
            }
        }
    }

    /// Moves a table over its sheet's removed rows and columns. A removed
    /// column inside it goes from its column list too.
    fn fix_table(
        entries: &mut Vec<Entry>,
        part: &str,
        rows: &BTreeSet<u32>,
        cols: &BTreeSet<u32>,
        sheet: &str,
    ) -> Result<(), String> {
        let Some(src) = package::text_of(entries, part) else {
            return Ok(());
        };
        let mut tree = xml::parse(&src)?;
        let Some(root) = xml::root_mut(&mut tree) else {
            return Ok(());
        };
        let name = root.attr("displayName").unwrap_or_default();
        let Some(r) = root.attr("ref") else {
            return Ok(());
        };
        let Some((a, b)) = r.split_once(':') else {
            return Ok(());
        };
        let (Some((ca, ra)), Some((cb, rb))) = (split_ref(a), split_ref(b)) else {
            return Ok(());
        };
        let (ca, cb) = (col_to_num(&ca), col_to_num(&cb));
        let inside: Vec<u32> = cols
            .iter()
            .copied()
            .filter(|c| ca <= *c && *c <= cb)
            .collect();
        if rows.contains(&ra) && (ra..=rb).any(|x| !rows.contains(&x)) {
            return Err(format!(
                "drop(\"row\") cannot remove the header row of table {name:?} in sheet {sheet:?}"
            ));
        }
        if (ra..=rb).all(|x| rows.contains(&x)) {
            return Err(format!(
                "drop(\"row\") cannot remove every row of table {name:?} in sheet {sheet:?}"
            ));
        }
        if inside.len() as u32 == cb - ca + 1 {
            return Err(format!(
                "drop(\"column\") cannot remove every column of table {name:?} in sheet {sheet:?}"
            ));
        }
        let nb = rb - Remap::up_to(rows, rb);
        let na = ra - Remap::before(rows, ra);
        if nb <= na && root.attr("headerRowCount").as_deref() != Some("0") {
            return Err(format!(
                "drop(\"row\") would leave table {name:?} in sheet {sheet:?} with no data row"
            ));
        }
        let moved = format!(
            "{}{na}:{}{nb}",
            num_to_col(ca - Remap::before(cols, ca)),
            num_to_col(cb - Remap::up_to(cols, cb))
        );
        root.set_attr("ref", &moved);
        if let Some(f) = root.child_mut("autoFilter") {
            f.set_attr("ref", &moved);
            f.children.clear();
            f.self_closing = true;
            f.open = format!("{}/>", f.open.trim_end_matches("/>").trim_end_matches('>'));
        }
        if let Some(list) = root.child_mut("tableColumns") {
            for &c in inside.iter().rev() {
                let idx = (c - ca) as usize;
                let at = list
                    .children
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| matches!(n, Node::Elem(e) if e.name == "tableColumn"))
                    .nth(idx)
                    .map(|(i, _)| i);
                if let Some(i) = at {
                    list.children.remove(i);
                }
            }
            let count = list
                .children
                .iter()
                .filter(|n| matches!(n, Node::Elem(_)))
                .count();
            list.set_attr("count", &count.to_string());
            for (i, n) in list
                .children
                .iter_mut()
                .filter_map(|n| match n {
                    Node::Elem(e) => Some(e),
                    _ => None,
                })
                .enumerate()
            {
                n.set_attr("id", &(i + 1).to_string());
            }
        }
        package::set(entries, part, xml::to_xml(&tree).into_bytes());
        Ok(())
    }

    /// Moves the anchors of a drawing past removed rows and columns.
    fn fix_anchors(
        entries: &mut Vec<Entry>,
        part: &str,
        rows: &BTreeSet<u32>,
        cols: &BTreeSet<u32>,
    ) {
        let Some(src) = package::text_of(entries, part) else {
            return;
        };
        let Ok(mut tree) = xml::parse(&src) else {
            return;
        };
        visit(&mut tree, &mut |e: &mut Elem| {
            let set = match e.name.as_str() {
                "xdr:col" => cols,
                "xdr:row" => rows,
                _ => return,
            };
            if let Ok(idx) = e.text_content().trim().parse::<u32>() {
                e.children = vec![Node::Text(xml::Text {
                    raw: Remap::index(set, idx).to_string(),
                    pieces: None,
                })];
            }
        });
        package::set(entries, part, xml::to_xml(&tree).into_bytes());
    }
}

mod ods {
    use super::*;

    /// `(row, column)` from 1 of the cell holding `path` in a sheet.
    pub(super) fn position(nodes: &[Node], sheet: &Path, path: &[usize]) -> Option<(u32, u32)> {
        let cell = nearest(
            nodes,
            path,
            &["table:table-cell", "table:covered-table-cell"],
        )?;
        let table = elem_at(nodes, sheet)?;
        let mut row_paths = Vec::new();
        tables::rows(table, &Vec::new(), &mut row_paths);
        let mut at = 1u32;
        for rp in row_paths {
            let full: Path = sheet.iter().chain(rp.iter()).copied().collect();
            let row = elem_at(nodes, &full)?;
            let n = attr_u32(row, "table:number-rows-repeated")
                .unwrap_or(1)
                .max(1);
            if cell.starts_with(&full) && cell.len() == full.len() + 1 {
                let idx = cell[full.len()];
                let mut col = 1u32;
                for c in &row.children[..idx] {
                    if let Node::Elem(c) = c {
                        if c.name.starts_with("table:") && c.name.ends_with("table-cell") {
                            col += attr_u32(c, "table:number-columns-repeated")
                                .unwrap_or(1)
                                .max(1);
                        }
                    }
                }
                return Some((at, col));
            }
            at += n;
        }
        None
    }

    fn name_of(table: &Elem) -> String {
        table.attr("table:name").unwrap_or_default()
    }

    /// Removes rows and columns from ODS sheets, and moves the references
    /// in formulas and ranges past them.
    pub(super) fn apply(
        nodes: &mut [Node],
        edits: HashMap<Path, (BTreeSet<u32>, BTreeSet<u32>)>,
    ) -> Result<(), String> {
        for (sheet_path, (rows, cols)) in edits {
            let table = elem_at(nodes, &sheet_path).ok_or("the sheet went missing")?;
            let sheet = name_of(table);
            let what = match (rows.is_empty(), cols.is_empty()) {
                (false, true) => "drop(\"row\")",
                (true, false) => "drop(\"column\")",
                _ => "drop(\"row\") and drop(\"column\")",
            };
            // Refuse merges first.
            let mut row_paths = Vec::new();
            tables::rows(table, &Vec::new(), &mut row_paths);
            let mut at = 1u32;
            for rp in &row_paths {
                let row = elem_at(&table.children, rp).ok_or("no row")?;
                let n = attr_u32(row, "table:number-rows-repeated")
                    .unwrap_or(1)
                    .max(1);
                let mut col = 1u32;
                for c in &row.children {
                    let Node::Elem(c) = c else { continue };
                    if !(c.name == "table:table-cell" || c.name == "table:covered-table-cell") {
                        continue;
                    }
                    let rep = attr_u32(c, "table:number-columns-repeated")
                        .unwrap_or(1)
                        .max(1);
                    let span_r = attr_u32(c, "table:number-rows-spanned").unwrap_or(1);
                    let span_c = attr_u32(c, "table:number-columns-spanned").unwrap_or(1);
                    if span_r > 1 {
                        let cover = at..at + span_r;
                        if cover.clone().any(|r| rows.contains(&r))
                            && !cover.clone().all(|r| rows.contains(&r))
                        {
                            return Err(format!(
                                "{what} cannot remove a row of sheet {sheet:?} that holds part of a cell merged down across rows that stay"
                            ));
                        }
                    }
                    if span_c > 1 {
                        let cover = col..col + span_c;
                        if cover.clone().any(|x| cols.contains(&x)) && !rows.contains(&at) {
                            return Err(format!(
                                "drop(\"column\") cannot remove a column of sheet {sheet:?} that a cell merged across columns spans"
                            ));
                        }
                    }
                    col += rep;
                }
                at += n;
            }
            let table = elem_at_mut(nodes, &sheet_path).ok_or("the sheet went missing")?;
            // Rows, from the last.
            let mut at = 1u32;
            let mut gone: Vec<Path> = Vec::new();
            for rp in &row_paths {
                let row = elem_at(&table.children, rp).ok_or("no row")?;
                let n = attr_u32(row, "table:number-rows-repeated")
                    .unwrap_or(1)
                    .max(1);
                if (at..at + n).any(|r| rows.contains(&r)) {
                    gone.push(rp.clone());
                }
                at += n;
            }
            for rp in gone.iter().rev() {
                let (last, parent) = rp.split_last().ok_or("no row")?;
                let kids = if parent.is_empty() {
                    &mut table.children
                } else {
                    &mut elem_at_mut(&mut table.children, parent)
                        .ok_or("no group")?
                        .children
                };
                kids.remove(*last);
            }
            // Columns, from the last.
            for &c in cols.iter().rev() {
                remove_col(table, c);
            }
            // Formulas in this sheet.
            let mut problem: Option<String> = None;
            fix_formulas(table, &sheet, true, &rows, &cols, &mut problem);
            if let Some(cell) = problem {
                return Err(ref_error(what, &sheet, "a formula", cell));
            }
            if let Some(ranges) = table.attr("table:print-ranges") {
                let moved = odf_ranges(&ranges, &sheet, &rows, &cols);
                table.set_attr("table:print-ranges", &moved);
            }
            // Other sheets and named ranges.
            if let Some(list) = sheet_path
                .split_last()
                .and_then(|(_, p)| elem_at_mut(nodes, p))
            {
                let mut problem: Option<String> = None;
                for n in list.children.iter_mut() {
                    let Node::Elem(e) = n else { continue };
                    if e.name == "table:table" && name_of(e) != sheet {
                        fix_formulas(e, &sheet, false, &rows, &cols, &mut problem);
                    } else if e.name == "table:named-expressions" {
                        for r in e.children.iter_mut() {
                            let Node::Elem(r) = r else { continue };
                            for attr in ["table:cell-range-address", "table:base-cell-address"] {
                                if let Some(v) = r.attr(attr) {
                                    r.set_attr(attr, &odf_ranges(&v, &sheet, &rows, &cols));
                                }
                            }
                        }
                    }
                }
                if let Some(cell) = problem {
                    return Err(ref_error(what, &sheet, "a formula on another sheet", cell));
                }
            }
        }
        Ok(())
    }

    fn remove_col(table: &mut Elem, col: u32) {
        fn shrink(children: &mut Vec<Node>, names: &[&str], repeat: &str, col: u32) {
            let mut at = 1u32;
            let mut hit = None;
            for (i, n) in children.iter().enumerate() {
                let Node::Elem(e) = n else { continue };
                if !names.contains(&e.name.as_str()) {
                    continue;
                }
                let r = attr_u32(e, repeat).unwrap_or(1).max(1);
                if col < at + r {
                    hit = Some((i, r));
                    break;
                }
                at += r;
            }
            if let Some((i, r)) = hit {
                if r > 1 {
                    if let Node::Elem(e) = &mut children[i] {
                        if r - 1 > 1 {
                            e.set_attr(repeat, &(r - 1).to_string());
                        } else {
                            e.remove_attr(repeat);
                        }
                    }
                } else {
                    children.remove(i);
                }
            }
        }
        fn walk(e: &mut Elem, col: u32) {
            match e.name.as_str() {
                "table:table-row" => shrink(
                    &mut e.children,
                    &["table:table-cell", "table:covered-table-cell"],
                    "table:number-columns-repeated",
                    col,
                ),
                "table:table"
                | "table:table-columns"
                | "table:table-header-columns"
                | "table:table-column-group" => {
                    shrink(
                        &mut e.children,
                        &["table:table-column"],
                        "table:number-columns-repeated",
                        col,
                    );
                    for n in e.children.iter_mut() {
                        if let Node::Elem(c) = n {
                            if c.name != "table:table" {
                                walk(c, col);
                            }
                        }
                    }
                }
                "table:table-rows" | "table:table-header-rows" | "table:table-row-group" => {
                    for n in e.children.iter_mut() {
                        if let Node::Elem(c) = n {
                            walk(c, col);
                        }
                    }
                }
                _ => {}
            }
        }
        walk(table, col);
    }

    fn fix_formulas(
        e: &mut Elem,
        sheet: &str,
        own: bool,
        rows: &BTreeSet<u32>,
        cols: &BTreeSet<u32>,
        problem: &mut Option<String>,
    ) {
        for n in e.children.iter_mut() {
            let Node::Elem(c) = n else { continue };
            if let Some(f) = c.attr("table:formula") {
                match odf_formula(&f, sheet, own, rows, cols) {
                    Ok(m) if m != f => {
                        c.set_attr("table:formula", &m);
                        for attr in [
                            "office:value",
                            "office:date-value",
                            "office:time-value",
                            "office:boolean-value",
                            "office:string-value",
                        ] {
                            c.remove_attr(attr);
                        }
                        c.children
                            .retain(|n| !matches!(n, Node::Elem(p) if p.name == "text:p"));
                    }
                    Ok(_) => {}
                    Err(cell) => {
                        problem.get_or_insert(cell);
                    }
                }
            }
            fix_formulas(c, sheet, own, rows, cols, problem);
        }
    }

    /// One end of an ODF reference: `$'Sheet'.$B$2`, `.B2` or `Sheet.B2`.
    fn split_end(end: &str) -> Option<(Option<String>, &str)> {
        let mut quoted = false;
        let mut dot = None;
        for (i, c) in end.char_indices() {
            match c {
                '\'' => quoted = !quoted,
                '.' if !quoted => dot = Some(i),
                _ => {}
            }
        }
        let dot = dot?;
        let sheet = end[..dot].trim_start_matches('$');
        let sheet = (!sheet.is_empty()).then(|| sheet_name_of(sheet));
        Some((sheet, &end[dot + 1..]))
    }

    /// Rewrites the cells of a reference list on one sheet as A1 text,
    /// keeping everything around them.
    fn move_ends(ends: &[&str], sheet: &str, own: bool, remap: &mut Remap) -> Option<Vec<String>> {
        let positions: &[RefPos] = match ends.len() {
            1 => &[RefPos::Single],
            2 => &[RefPos::RangeStart, RefPos::RangeEnd],
            _ => return None,
        };
        let mut out = Vec::new();
        let mut targeted = false;
        for (k, end) in ends.iter().enumerate() {
            let (name, cell) = split_end(end)?;
            let here = match &name {
                Some(n) => n == sheet,
                None => own || (k == 1 && targeted),
            };
            if k == 0 {
                targeted = here;
            }
            if !here {
                return None;
            }
            let prefix = &end[..end.len() - cell.len()];
            let moved = rewrite_refs(cell, |c, _, r, _, _| remap.map(c, r, positions[k]));
            out.push(format!("{prefix}{moved}"));
        }
        Some(out)
    }

    fn odf_formula(
        formula: &str,
        sheet: &str,
        own: bool,
        rows: &BTreeSet<u32>,
        cols: &BTreeSet<u32>,
    ) -> Result<String, String> {
        if !own && !formula.contains(sheet) {
            return Ok(formula.to_string());
        }
        let mut remap = Remap::new(rows, cols);
        let mut out = String::new();
        let mut rest = formula;
        while let Some(open) = rest.find('[') {
            let Some(close) = rest[open..].find(']') else {
                break;
            };
            out.push_str(&rest[..=open]);
            let inner = &rest[open + 1..open + close];
            let ends: Vec<&str> = inner.split(':').collect();
            match move_ends(&ends, sheet, own, &mut remap) {
                Some(m) => out.push_str(&m.join(":")),
                None => out.push_str(inner),
            }
            rest = &rest[open + close..];
        }
        out.push_str(rest);
        match remap.error {
            Some(cell) => Err(cell),
            None => Ok(out),
        }
    }

    fn odf_ranges(list: &str, sheet: &str, rows: &BTreeSet<u32>, cols: &BTreeSet<u32>) -> String {
        list.split(' ')
            .map(|range| {
                let mut remap = Remap::new(rows, cols);
                let ends: Vec<&str> = range.split(':').collect();
                match move_ends(&ends, sheet, false, &mut remap) {
                    Some(m) if remap.error.is_none() => m.join(":"),
                    _ => range.to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(v: &[u32]) -> BTreeSet<u32> {
        v.iter().copied().collect()
    }

    #[test]
    fn formulas_move_past_removed_columns_and_rows() {
        let none = set(&[]);
        let b = set(&[2]);
        assert_eq!(
            rewrite_formula("SUM(A3:D3)*C1", "S", true, &none, &b).unwrap(),
            "SUM(A3:C3)*B1"
        );
        assert_eq!(
            rewrite_formula("B3*2", "S", true, &none, &b).unwrap_err(),
            "B3"
        );
        assert_eq!(
            rewrite_formula("SUM(B3:B5)", "S", true, &none, &b).unwrap_err(),
            "B3"
        );
        let r4 = set(&[4]);
        assert_eq!(
            rewrite_formula("SUM(D3:D5)+D6", "S", true, &r4, &none).unwrap(),
            "SUM(D3:D4)+D5"
        );
        // A range of only removed rows becomes the row before, as an empty
        // loop leaves it.
        assert_eq!(
            rewrite_formula("SUM(D4:D4)", "S", true, &r4, &none).unwrap(),
            "SUM(D3:D3)"
        );
        assert_eq!(
            rewrite_formula("'My S'!C2+Other!C2", "My S", false, &none, &b).unwrap(),
            "'My S'!B2+Other!C2"
        );
        assert!(rewrite_formula("SUM(C:C)", "S", true, &none, &b).is_err());
    }

    #[test]
    fn sqref_drops_ranges_that_only_covered_removed_cells() {
        let b = set(&[2]);
        let none = set(&[]);
        assert_eq!(rewrite_sqref("A1:D4 B2 C5", &none, &b), "A1:C4 B5");
        assert_eq!(rewrite_sqref("A3:C3", &set(&[3]), &none), "");
    }

    #[test]
    fn alt_text_goes_back_into_its_attribute() {
        let src = format!(
            "<a><p:cNvPr id=\"2\" name=\"x\"/>{ALT_OPEN}descr{ALT_SEP}Logo &amp; more{ALT_CLOSE}<b/></a>"
        );
        let mut nodes = xml::parse(&src).unwrap();
        restore_alt(&mut nodes);
        assert_eq!(
            xml::to_xml(&nodes),
            "<a><p:cNvPr id=\"2\" name=\"x\" descr=\"Logo &amp; more\"/><b/></a>"
        );
        let src = format!(
            "<a><wp:docPr id=\"2\" name=\"x\"/><w:t>{ALT_OPEN}title{ALT_SEP}a</w:t><w:br/><w:t>b{ALT_CLOSE}</w:t></a>"
        );
        let mut nodes = xml::parse(&src).unwrap();
        restore_alt(&mut nodes);
        assert_eq!(
            xml::to_xml(&nodes),
            "<a><wp:docPr id=\"2\" name=\"x\" title=\"a&#10;b\"/></a>"
        );
    }
}
