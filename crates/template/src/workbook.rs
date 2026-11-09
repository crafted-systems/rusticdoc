//! Whole sheets: repeating a sheet once per item, and naming sheets from
//! data, in XLSX and ODS.
//!
//! A sheet loop is a `{% sheet for s in sites %}` tag in any cell of the
//! sheet, and the sheet's name may hold `{{ }}` expressions, such as
//! `{{ s.name }}`. The loop goes in a cell because a sheet name is capped at
//! 31 characters and cannot hold `[ ] : / \ ? *`, which leaves no room for a
//! loop and a name together.
//!
//! At compile time the loop tag is hoisted around the sheet: around its part
//! in XLSX, around its `table:table` in ODS. The sheet's name is compiled as
//! text the template prints at the start of each copy, between two
//! characters XML forbids, which the value formatter strips from data. After
//! rendering, each copy's name is read back, made valid and unique, and the
//! package is put together: in XLSX each copy becomes its own part with its
//! own relationships, drawings, charts and tables.

use std::collections::{HashMap, HashSet};

use regex::Regex;

use crate::package::{self, Entry, MAX_ENTRIES, MAX_TOTAL_BYTES};
use crate::program::{find_tags, Block, PartTree, Tag, TagCtx, TagKind};
use crate::sheet::Layout;
use crate::xml::{self, elem_at_mut, node_at_mut, Elem, Node, Piece, TagId, Text};
use crate::{Diagnostic, Format, Location, Severity, TemplateError};

/// Opens the name a sheet copy printed for itself.
const TITLE_OPEN: char = '\u{3}';
/// Closes it.
const TITLE_CLOSE: char = '\u{4}';

/// Excel's longest sheet name, in UTF-16 units. LibreOffice allows more but
/// the same rule keeps an ODS openable after conversion.
const MAX_NAME: usize = 31;

const WORKBOOK: &str = "xl/workbook.xml";
const WORKBOOK_RELS: &str = "xl/_rels/workbook.xml.rels";
const CONTENT_TYPES: &str = "[Content_Types].xml";
const APP: &str = "docProps/app.xml";

/// `(name, part)` of every sheet in the workbook, in order.
pub fn xlsx_sheets(entries: &[Entry]) -> Vec<(String, String)> {
    let rels = package::relationships(entries, WORKBOOK_RELS, "xl");
    let Some(wb) = package::text_of(entries, WORKBOOK).and_then(|s| xml::parse(&s).ok()) else {
        return Vec::new();
    };
    let Some(sheets) = xml::root(&wb).and_then(|r| r.find("sheets")) else {
        return Vec::new();
    };
    sheets
        .children_named("sheet")
        .filter_map(|s| {
            let (name, rid) = (s.attr("name")?, s.attr("r:id")?);
            let (_, _, part) = rels.iter().find(|(id, _, _)| *id == rid)?;
            Some((name, part.clone()))
        })
        .collect()
}

/// Sheet parts whose name holds a tag: they are compiled even when their
/// cells hold none, so that the name can be printed.
pub fn xlsx_named_parts(entries: &[Entry]) -> HashSet<String> {
    xlsx_sheets(entries)
        .into_iter()
        .filter(|(name, _)| name.contains('{'))
        .map(|(_, part)| part)
        .collect()
}

fn error(message: String, loc: &Location) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        message,
        location: Some(loc.clone()),
    }
}

/// Everything compile needs to turn sheet loops and sheet names into
/// template code.
pub struct Compile<'a> {
    pub format: Format,
    pub entries: &'a [Entry],
    pub parts: &'a mut [PartTree],
    pub tags: &'a mut Vec<Tag>,
    pub ctxs: &'a mut Vec<TagCtx>,
    pub blocks: &'a [Block],
    pub diags: &'a mut Vec<Diagnostic>,
}

impl Compile<'_> {
    /// Hoists sheet loops around their sheets and compiles templated sheet
    /// names. In ODS it also marks the rows of sheets that have row blocks,
    /// for re-basing after rendering.
    pub fn run(mut self) {
        let loops: Vec<TagId> = (0..self.tags.len())
            .filter(|&id| matches!(self.tags[id].kind, TagKind::SheetLoop(_)))
            .collect();
        match self.format {
            Format::Xlsx => self.xlsx(&loops),
            Format::Ods => self.ods(&loops),
            _ => {
                for id in loops {
                    let tag = &self.tags[id];
                    self.diags.push(error(
                        format!(
                            "'{}' repeats a sheet, so it works in XLSX and ODS only",
                            tag.text
                        ),
                        &tag.loc,
                    ));
                }
            }
        }
    }

    fn xlsx(&mut self, loops: &[TagId]) {
        for (name, part_name) in xlsx_sheets(self.entries) {
            let Some(part) = self.parts.iter().position(|p| p.name == part_name) else {
                continue;
            };
            let mine: Vec<TagId> = loops
                .iter()
                .copied()
                .filter(|&id| self.ctxs[id].part == part)
                .collect();
            let Some((head, title)) = self.plan(&mine, &name, WORKBOOK, part) else {
                continue;
            };
            if let Some((open, close)) = head {
                self.parts[part].before.push(open);
                self.parts[part].after.push(close);
            }
            // Printed first, before the XML declaration, and cut off again.
            self.parts[part].nodes.insert(0, title);
        }
    }

    fn ods(&mut self, loops: &[TagId]) {
        let Some(part) = self.parts.iter().position(|p| p.name == "content.xml") else {
            return;
        };
        let blocks = self.parts[part].row_blocks.clone();
        crate::sheet::annotate_ods(&mut self.parts[part].nodes, &blocks);
        let Some(list) = sheet_list_path(&self.parts[part].nodes) else {
            for &id in loops {
                let tag = &self.tags[id];
                self.diags.push(error(
                    format!("'{}' must sit in a cell of the sheet it repeats", tag.text),
                    &tag.loc,
                ));
            }
            return;
        };
        let tables: Vec<(usize, String)> = match xml::elem_at(&self.parts[part].nodes, &list) {
            Some(e) => e
                .children
                .iter()
                .enumerate()
                .filter_map(|(k, n)| match n {
                    Node::Elem(t) if t.name == "table:table" => {
                        Some((k, t.attr("table:name").unwrap_or_default()))
                    }
                    _ => None,
                })
                .collect(),
            None => Vec::new(),
        };
        let mut placed = HashSet::new();
        for (k, name) in tables {
            let mut path = list.clone();
            path.push(k);
            let mine: Vec<TagId> = loops
                .iter()
                .copied()
                .filter(|&id| self.ctxs[id].part == part && self.ctxs[id].text.starts_with(&path))
                .collect();
            placed.extend(mine.iter().copied());
            let Some((head, title)) = self.plan(&mine, &name, "content.xml", part) else {
                continue;
            };
            let Some(table) = elem_at_mut(&mut self.parts[part].nodes, &path) else {
                continue;
            };
            if let Some((open, close)) = head {
                table.before.push(Piece::Tag(open));
                table.after.push(Piece::Tag(close));
            }
            table.make_open();
            table.children.insert(0, title);
        }
        for &id in loops.iter().filter(|id| !placed.contains(id)) {
            let tag = &self.tags[id];
            self.diags.push(error(
                format!("'{}' must sit in a cell of the sheet it repeats", tag.text),
                &tag.loc,
            ));
        }
    }

    /// Checks one sheet's loops and name. Returns the loop's opening and
    /// closing tags, when there is a loop, and the text node that prints the
    /// copy's name; `None` when the sheet needs neither.
    #[allow(clippy::type_complexity)]
    fn plan(
        &mut self,
        loops: &[TagId],
        name: &str,
        name_part: &str,
        part: usize,
    ) -> Option<(Option<(TagId, TagId)>, Node)> {
        let name_loc = Location {
            part: name_part.to_string(),
            place: format!("sheet \"{name}\" name"),
            snippet: name.to_string(),
        };
        let mut ok = true;
        if let [first, second, ..] = loops {
            let (a, b) = (&self.tags[*first], &self.tags[*second]);
            self.diags.push(error(
                format!(
                    "'{}' ({}) and '{}' ({}) both repeat sheet \"{name}\"; a sheet takes one sheet loop",
                    a.text, a.loc.place, b.text, b.loc.place
                ),
                &b.loc,
            ));
            ok = false;
        }
        let head = loops.first().copied();
        if let Some(id) = head {
            let tag = &self.tags[id];
            if tag.sheet_loop().is_none() {
                self.diags.push(error(
                    format!(
                        "'{}' is not a sheet loop; write it as {{% sheet for item in list %}}",
                        tag.text
                    ),
                    &tag.loc,
                ));
                ok = false;
            }
            // The loop is hoisted around the whole sheet, so a block around
            // the tag would close inside the loop it opened outside of.
            let holder = self.blocks.iter().find(|b| {
                b.open < id && id < b.close && self.ctxs[b.open].part == self.ctxs[id].part
            });
            if let Some(b) = holder {
                let open = &self.tags[b.open];
                self.diags.push(error(
                    format!(
                        "'{}' sits inside '{}' ({}); a sheet loop repeats the whole sheet, so no other block can hold it",
                        tag.text, open.text, open.loc.place
                    ),
                    &tag.loc,
                ));
                ok = false;
            }
        }

        let spans = find_tags(name);
        let mut outside = String::new();
        let mut last = 0;
        for &(s, e) in &spans {
            outside.push_str(&name[last..s]);
            last = e;
        }
        outside.push_str(&name[last..]);
        if outside.contains("{{") || outside.contains("{%") || outside.contains("{#") {
            self.diags.push(error(
                format!("sheet name \"{name}\" holds an unterminated tag"),
                &name_loc,
            ));
            ok = false;
        }
        let mut pieces = vec![Piece::Lit(TITLE_OPEN.to_string())];
        let mut last = 0;
        for &(s, e) in &spans {
            if s > last {
                pieces.push(Piece::Lit(xml::escape_text(&name[last..s])));
            }
            let tag = Tag::parse(
                &name[s..e],
                Location {
                    snippet: name[s..e].to_string(),
                    ..name_loc.clone()
                },
            );
            if !matches!(tag.kind, TagKind::Expr(_)) {
                self.diags.push(error(
                    format!(
                        "sheet name \"{name}\" holds '{}'; a sheet name can hold only {{{{ }}}} expressions, so put the sheet loop in a cell as {{% sheet for item in list %}}",
                        tag.text
                    ),
                    &name_loc,
                ));
                ok = false;
            } else {
                pieces.push(Piece::Tag(self.push(tag, part)));
            }
            last = e;
        }
        if last < name.len() {
            pieces.push(Piece::Lit(xml::escape_text(&name[last..])));
        }
        pieces.push(Piece::Lit(TITLE_CLOSE.to_string()));
        if !ok || (head.is_none() && spans.is_empty()) {
            return None;
        }

        let head = head.map(|open| {
            let ctx = self.ctxs[open].clone();
            if let Some(Node::Text(t)) = node_at_mut(&mut self.parts[ctx.part].nodes, &ctx.text) {
                if let Some(p) = &mut t.pieces {
                    p.retain(|p| *p != Piece::Tag(open));
                }
            }
            let close = self.push(
                Tag {
                    text: "{% endfor %}".to_string(),
                    kind: TagKind::SheetEnd,
                    loc: self.tags[open].loc.clone(),
                },
                part,
            );
            (open, close)
        });
        let title = Node::Text(Text {
            raw: String::new(),
            pieces: Some(pieces),
        });
        Some((head, title))
    }

    /// Adds a tag the document does not hold as text, such as one from a
    /// sheet name, to the part it belongs with.
    fn push(&mut self, tag: Tag, part: usize) -> TagId {
        self.tags.push(tag);
        self.ctxs.push(TagCtx {
            part,
            text: Vec::new(),
            para: Vec::new(),
            row: None,
            table: None,
        });
        self.tags.len() - 1
    }
}

/// The path of `office:spreadsheet`.
fn sheet_list_path(nodes: &[Node]) -> Option<Vec<usize>> {
    fn walk(nodes: &[Node], path: &mut Vec<usize>) -> bool {
        for (i, n) in nodes.iter().enumerate() {
            let Node::Elem(e) = n else { continue };
            path.push(i);
            if e.name == "office:spreadsheet" || walk(&e.children, path) {
                return true;
            }
            path.pop();
        }
        false
    }
    let mut path = Vec::new();
    walk(nodes, &mut path).then_some(path)
}

// ---------------------------------------------------------------------------
// Names.

/// A name Excel and LibreOffice accept: the forbidden characters
/// `: \ / ? * [ ]` become `_`, control characters go, no apostrophe at
/// either end, at most 31 UTF-16 units, and never empty.
pub fn valid_name(raw: &str) -> String {
    let replaced: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| {
            if matches!(c, ':' | '\\' | '/' | '?' | '*' | '[' | ']') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let name = truncate(trim_name(&replaced), MAX_NAME);
    let name = trim_name(&name);
    if name.is_empty() {
        "Sheet".to_string()
    } else if name.eq_ignore_ascii_case("history") {
        // Excel keeps this name for its change tracking.
        format!("{name}_")
    } else {
        name.to_string()
    }
}

fn trim_name(s: &str) -> &str {
    s.trim().trim_matches('\'').trim()
}

/// At most `max` UTF-16 units of `s`, cut between characters.
fn truncate(s: &str, max: usize) -> String {
    let mut units = 0;
    s.chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= max
        })
        .collect()
}

/// `name`, or `name (2)`, `name (3)` and so on, whichever no sheet has yet.
/// Sheet names compare without regard to case.
pub fn unique_name(name: &str, taken: &mut HashSet<String>) -> String {
    let mut candidate = name.to_string();
    let mut n = 2;
    while taken.contains(&candidate.to_lowercase()) {
        let suffix = format!(" ({n})");
        let base = truncate(name, MAX_NAME - suffix.encode_utf16().count());
        candidate = format!("{}{suffix}", base.trim_end());
        n += 1;
    }
    taken.insert(candidate.to_lowercase());
    candidate
}

/// A sheet name as a formula writes it, quoted.
fn quoted(name: &str) -> String {
    format!("'{}'", name.replace('\'', "''"))
}

/// Sheet renames as `(old name, new name)`.
type Renames = [(String, String)];

/// Replaces references to each old sheet name in unescaped text, such as a
/// formula, with references to its new name. `sep` follows the name: `!` in
/// OOXML, `.` in ODF.
fn rename_text(text: &str, renames: &Renames, sep: char) -> String {
    let mut out = text.to_string();
    for (old, new) in renames {
        let from = format!("{}{sep}", quoted(old));
        if out.contains(&from) {
            out = out.replace(&from, &format!("{}{sep}", quoted(new)));
        }
    }
    out
}

/// Whether raw XML may name one of the old sheets, in any of the ways XML
/// can write it. A cheap test before parsing.
fn mentions(xml: &str, renames: &Renames) -> bool {
    renames.iter().any(|(old, _)| {
        let inner = old.replace('\'', "''");
        [xml::escape_text(&inner), xml::escape_attr(&inner)]
            .iter()
            .any(|form| xml.contains(form.as_str()) || xml.contains(&form.replace('\'', "&apos;")))
    })
}

/// Renames sheet references in parsed XML: in attribute values always, and
/// in text when `text` is set. Values are read unescaped and written back
/// escaped for where they go, so a name holding `"`, `<`, `&` or `'` cannot
/// break the markup. Returns whether anything changed.
fn rename_nodes(nodes: &mut [Node], renames: &Renames, sep: char, text: bool) -> bool {
    let mut changed = false;
    for n in nodes {
        match n {
            Node::Text(t) if text && t.pieces.is_none() => {
                let plain = xml::unescape(&t.raw).into_owned();
                let moved = rename_text(&plain, renames, sep);
                if moved != plain {
                    t.raw = xml::escape_text(&moved);
                    changed = true;
                }
            }
            Node::Elem(e) => {
                for (name, raw) in e.attrs() {
                    let value = xml::unescape(&raw).into_owned();
                    let moved = rename_text(&value, renames, sep);
                    if moved != value {
                        e.set_attr(&name, &moved);
                        changed = true;
                    }
                }
                changed |= rename_nodes(&mut e.children, renames, sep, text);
            }
            _ => {}
        }
    }
    changed
}

/// Renames sheet references in a whole OOXML part, text and attributes.
fn rename_xml(xml: &str, renames: &Renames) -> String {
    if renames.is_empty() || !mentions(xml, renames) {
        return xml.to_string();
    }
    let Ok(mut nodes) = xml::parse(xml) else {
        return xml.to_string();
    };
    if rename_nodes(&mut nodes, renames, '!', true) {
        xml::to_xml(&nodes)
    } else {
        xml.to_string()
    }
}

fn one(old: &str, new: &str) -> Vec<(String, String)> {
    if old == new {
        Vec::new()
    } else {
        vec![(old.to_string(), new.to_string())]
    }
}

/// Splits the name a copy printed off the front of its content.
pub fn split_title(content: &str) -> (Option<String>, &str) {
    if let Some(rest) = content.strip_prefix(TITLE_OPEN) {
        if let Some(end) = rest.find(TITLE_CLOSE) {
            let title = xml::unescape(&rest[..end]).into_owned();
            return (Some(title), &rest[end + TITLE_CLOSE.len_utf8()..]);
        }
    }
    (None, content)
}

// ---------------------------------------------------------------------------
// XLSX after rendering.

/// A rendered sheet on its way into the package.
pub struct SheetOut {
    pub part: String,
    /// The name it printed, cut off its content with [`split_title`].
    pub title: Option<String>,
    pub content: String,
    /// Where its rows went.
    pub layout: Layout,
}

/// One sheet of the finished workbook.
struct Final<'a> {
    /// The `<sheet>` element it is made from.
    elem: Elem,
    /// Its index in the template's sheet list.
    old_index: usize,
    old_name: String,
    /// The template's part for it.
    template_part: String,
    part: String,
    /// What was rendered for it, for a sheet that was compiled.
    out: Option<&'a SheetOut>,
    /// 0 for the sheet that keeps the template's part, 1 and up for copies.
    copy: usize,
    name: String,
}

impl Final<'_> {
    fn titled(&self) -> bool {
        self.out.is_some_and(|o| o.title.is_some())
    }
}

/// Puts rendered sheets into the package. A sheet that came out once keeps
/// its part; further copies get parts, relationships, content types and
/// owned drawings, charts and tables of their own; a sheet that came out
/// zero times is removed. Names are made valid and unique, and the sheet
/// list, defined names, the active sheet and `docProps/app.xml` follow.
/// Defined names, such as print areas, that point at moved rows move too.
/// Returns the names of the sheet parts that hold rendered content.
pub fn finish_xlsx(
    entries: &mut Vec<Entry>,
    outputs: &[SheetOut],
    compiled: &[&str],
) -> Result<Vec<String>, TemplateError> {
    let mut seen = HashSet::new();
    let plain = outputs
        .iter()
        .all(|o| o.title.is_none() && seen.insert(o.part.as_str()))
        && compiled.iter().all(|c| seen.contains(c));
    if plain {
        for o in outputs {
            package::set(entries, &o.part, o.content.clone().into_bytes());
        }
        if outputs.iter().any(|o| !o.layout.is_identity()) {
            let layouts: HashMap<String, &Layout> = xlsx_sheets(entries)
                .into_iter()
                .filter_map(|(name, part)| {
                    Some((name, &outputs.iter().find(|o| o.part == part)?.layout))
                })
                .collect();
            if let Some(mut wb) =
                package::text_of(entries, WORKBOOK).and_then(|s| xml::parse(&s).ok())
            {
                if rebase_names(&mut wb, &layouts) {
                    package::set(entries, WORKBOOK, xml::to_xml(&wb).into_bytes());
                }
            }
        }
        return Ok(outputs.iter().map(|o| o.part.clone()).collect());
    }

    let wb_src = package::text_of(entries, WORKBOOK)
        .ok_or_else(|| TemplateError::new("missing xl/workbook.xml"))?;
    let mut wb = xml::parse(&wb_src).map_err(|e| TemplateError::new(format!("{WORKBOOK}: {e}")))?;
    let rels = package::relationships(entries, WORKBOOK_RELS, "xl");
    let rel_of = |e: &Elem| {
        e.attr("r:id")
            .and_then(|rid| rels.iter().find(|(id, _, _)| *id == rid).cloned())
    };
    let template_sheets: Vec<Elem> = xml::root(&wb)
        .and_then(|r| r.find("sheets"))
        .map(|s| s.children_named("sheet").cloned().collect())
        .unwrap_or_default();

    let mut by_part: HashMap<&str, Vec<&SheetOut>> = HashMap::new();
    for out in outputs {
        by_part.entry(out.part.as_str()).or_default().push(out);
    }
    let mut finals: Vec<Final> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    let mut listed = HashSet::new();
    for (old_index, elem) in template_sheets.iter().enumerate() {
        let old_name = elem.attr("name").unwrap_or_default();
        let part = rel_of(elem).map(|(_, _, t)| t).unwrap_or_default();
        listed.insert(part.clone());
        let copies: Vec<Option<&SheetOut>> = if compiled.contains(&part.as_str()) {
            by_part
                .get(part.as_str())
                .map(|v| v.iter().map(|o| Some(*o)).collect())
                .unwrap_or_default()
        } else {
            vec![None]
        };
        if copies.is_empty() {
            removed.push(part);
            continue;
        }
        for (copy, out) in copies.into_iter().enumerate() {
            finals.push(Final {
                elem: elem.clone(),
                old_index,
                name: old_name.clone(),
                old_name: old_name.clone(),
                template_part: part.clone(),
                part: part.clone(),
                out,
                copy,
            });
        }
    }
    if finals.is_empty() {
        return Err(TemplateError::new(
            "the sheet loops left the workbook with no sheets, and a workbook needs one; \
             keep a sheet without a loop or make sure the list is not empty",
        ));
    }

    // Names: sheets that keep theirs first, then the printed ones in order.
    let mut taken: HashSet<String> = finals
        .iter()
        .filter(|f| !f.titled())
        .map(|f| f.name.to_lowercase())
        .collect();
    for f in finals.iter_mut().filter(|f| f.titled()) {
        let title = f.out.and_then(|o| o.title.as_deref()).unwrap_or("");
        f.name = unique_name(&valid_name(title), &mut taken);
    }
    // Each templated sheet once, with the name of its first copy: what a
    // reference to it from elsewhere now points at.
    let mut first_name: Vec<(String, String)> = Vec::new();
    for f in finals.iter().filter(|f| f.titled()) {
        if !first_name.iter().any(|(old, _)| *old == f.old_name) {
            first_name.push((f.old_name.clone(), f.name.clone()));
        }
    }
    first_name.retain(|(old, new)| old != new);

    let mut pkg = Copier::new(entries);
    // Refuse a loop whose copies cannot fit before building any of them.
    pkg.check_room(finals.iter().filter(|f| f.copy > 0).map(|f| {
        (
            f.template_part.as_str(),
            f.out.map_or(0, |o| o.content.len()),
        )
    }))?;
    for f in finals.iter_mut().filter(|f| f.copy > 0) {
        f.part = pkg.fresh(&f.template_part);
    }
    // Content, and each copy's own relationships and owned parts. Copies
    // come first: they copy the template's charts before the first copy's
    // charts are pointed at its new name.
    let order = finals
        .iter()
        .filter(|f| f.copy > 0)
        .chain(finals.iter().filter(|f| f.copy == 0));
    for f in order {
        let Some(out) = f.out else { continue };
        let mut renames = one(&f.old_name, &f.name);
        renames.extend(
            first_name
                .iter()
                .filter(|(old, _)| *old != f.old_name)
                .cloned(),
        );
        let mut xml = rename_xml(&out.content, &renames);
        if f.copy > 0 {
            let tables = pkg.copy_rels(&f.template_part, &f.part, &f.old_name, &f.name)?;
            xml = copy_sheet_fixes(&xml, &tables);
            pkg.content_type_like(&f.template_part, &f.part);
        } else {
            pkg.rename_owned(&f.part, &f.old_name, &f.name)?;
        }
        pkg.put(&f.part, xml.into_bytes())?;
    }
    // Static sheets that point at a renamed sheet point at its first copy.
    for f in finals.iter().filter(|f| f.out.is_none()) {
        if let Some(src) = pkg.text(&f.part) {
            let xml = rename_xml(&src, &first_name);
            if xml != src {
                pkg.put(&f.part, xml.into_bytes())?;
            }
        }
    }
    for part in &removed {
        pkg.remove_sheet(part);
    }

    // The workbook: sheet list, defined names and views.
    let mut next_sheet_id = template_sheets
        .iter()
        .filter_map(|e| e.attr("sheetId")?.parse::<u32>().ok())
        .max()
        .unwrap_or(0)
        + 1;
    let mut next_rid = max_rid(&rels) + 1;
    let mut sheet_elems = Vec::new();
    for f in &finals {
        let mut e = f.elem.clone();
        if f.titled() {
            e.set_attr("name", &f.name);
        }
        if f.copy > 0 {
            e.set_attr("sheetId", &next_sheet_id.to_string());
            next_sheet_id += 1;
            let rid = format!("rId{next_rid}");
            next_rid += 1;
            let ty = rel_of(&f.elem).map(|(_, ty, _)| ty).unwrap_or_default();
            let target = f.part.strip_prefix("xl/").unwrap_or(&f.part).to_string();
            pkg.new_rels.push((rid.clone(), ty, target));
            e.set_attr("r:id", &rid);
        }
        sheet_elems.push(Node::Elem(e));
    }
    let mut new_index: Vec<Vec<usize>> = vec![Vec::new(); template_sheets.len()];
    for (i, f) in finals.iter().enumerate() {
        new_index[f.old_index].push(i);
    }
    let layouts: HashMap<String, &Layout> = finals
        .iter()
        .filter_map(|f| Some((f.name.clone(), &f.out?.layout)))
        .collect();
    if let Some(root) = xml::root_mut(&mut wb) {
        if let Some(sheets) = root.find_mut("sheets") {
            sheets.make_open();
            sheets.children = sheet_elems;
        }
        if let Some(names) = root.find_mut("definedNames") {
            let mut out = Vec::new();
            for n in names.children.drain(..) {
                let Node::Elem(d) = n else {
                    out.push(n);
                    continue;
                };
                let text = d.text_content();
                match d.attr("localSheetId").and_then(|v| v.parse::<usize>().ok()) {
                    // A name scoped to a sheet goes with each copy, naming it.
                    Some(old) => {
                        for &i in new_index.get(old).into_iter().flatten() {
                            let mut c = d.clone();
                            c.set_attr("localSheetId", &i.to_string());
                            let f = &finals[i];
                            set_text(&mut c, &rename_text(&text, &one(&f.old_name, &f.name), '!'));
                            out.push(Node::Elem(c));
                        }
                    }
                    None => {
                        let mut c = d.clone();
                        let t = rename_text(&text, &first_name, '!');
                        if t != text {
                            set_text(&mut c, &t);
                        }
                        out.push(Node::Elem(c));
                    }
                }
            }
            names.children = out;
        }
        rebase_names(&mut root.children, &layouts);
        if let Some(views) = root.find_mut("bookViews") {
            for view in views.children.iter_mut() {
                let Node::Elem(v) = view else { continue };
                for attr in ["activeTab", "firstSheet"] {
                    if let Some(old) = v.attr(attr).and_then(|a| a.parse::<usize>().ok()) {
                        let now = new_index
                            .get(old)
                            .and_then(|v| v.first())
                            .copied()
                            .unwrap_or(0);
                        v.set_attr(attr, &now.to_string());
                    }
                }
            }
        }
    }
    pkg.put(WORKBOOK, xml::to_xml(&wb).into_bytes())?;

    let worksheet_names: Vec<String> = finals
        .iter()
        .filter(|f| f.part.starts_with("xl/worksheets/"))
        .map(|f| f.name.clone())
        .collect();
    let template_worksheets = template_sheets
        .iter()
        .filter(|e| rel_of(e).is_some_and(|(_, _, t)| t.starts_with("xl/worksheets/")))
        .count();
    pkg.update_app(template_worksheets, &worksheet_names, &first_name)?;

    let mut names: Vec<String> = finals
        .iter()
        .filter(|f| f.out.is_some())
        .map(|f| f.part.clone())
        .collect();
    // Parts the workbook does not list are written as they came out.
    for o in outputs.iter().filter(|o| !listed.contains(&o.part)) {
        pkg.put(&o.part, o.content.clone().into_bytes())?;
        names.push(o.part.clone());
    }
    pkg.finish()?;
    Ok(names)
}

/// Moves the references in defined names, such as print areas, to rows
/// that loops moved, by the layout of the sheet each reference names.
/// Returns whether anything changed.
fn rebase_names(nodes: &mut [Node], layouts: &HashMap<String, &Layout>) -> bool {
    let mut changed = false;
    for n in nodes.iter_mut() {
        let Node::Elem(e) = n else { continue };
        if e.name != "definedName" {
            changed |= rebase_names(&mut e.children, layouts);
            continue;
        }
        let text = e.text_content();
        let moved = crate::sheet::rebase_named(&text, |name| layouts.get(name).copied());
        if moved != text {
            set_text(e, &moved);
            changed = true;
        }
    }
    changed
}

fn set_text(e: &mut Elem, text: &str) {
    e.make_open();
    e.children = vec![Node::Text(Text {
        raw: xml::escape_text(text),
        pieces: None,
    })];
}

fn max_rid(rels: &[(String, String, String)]) -> u32 {
    rels.iter()
        .filter_map(|(id, _, _)| id.strip_prefix("rId")?.parse::<u32>().ok())
        .max()
        .unwrap_or(0)
}

/// What only a copy of a sheet needs: it is not the selected tab, it has no
/// code name of its own yet, and its formulas name its own tables.
fn copy_sheet_fixes(xml: &str, tables: &[(String, String)]) -> String {
    let Ok(mut nodes) = xml::parse(xml) else {
        return xml.to_string();
    };
    if let Some(root) = xml::root_mut(&mut nodes) {
        if let Some(pr) = root.child_mut("sheetPr") {
            pr.remove_attr("codeName");
        }
        if let Some(views) = root.child_mut("sheetViews") {
            for v in views.children.iter_mut() {
                if let Node::Elem(v) = v {
                    v.remove_attr("tabSelected");
                }
            }
        }
        if !tables.is_empty() {
            if let Some(data) = root.find_mut("sheetData") {
                let renames: Vec<(Regex, String)> = tables
                    .iter()
                    .filter_map(|(old, new)| {
                        let re = Regex::new(&format!(r"\b{}\[", regex::escape(old))).ok()?;
                        Some((re, format!("{new}[")))
                    })
                    .collect();
                rename_tables(&mut data.children, &renames);
            }
        }
    }
    xml::to_xml(&nodes)
}

/// Points structured references such as `Table1[Amount]` at the copy's
/// tables.
fn rename_tables(nodes: &mut [Node], renames: &[(Regex, String)]) {
    for n in nodes {
        let Node::Elem(e) = n else { continue };
        if e.name == "f" {
            let text = e.text_content();
            let mut moved = text.clone();
            for (re, new) in renames {
                moved = re.replace_all(&moved, new.as_str()).into_owned();
            }
            if moved != text {
                set_text(e, &moved);
            }
        } else {
            rename_tables(&mut e.children, renames);
        }
    }
}

/// Relationship types whose targets are shared by every copy rather than
/// copied: they are not owned by one sheet.
const SHARED_TYPES: [&str; 3] = ["/pivotCacheDefinition", "/externalLink", "/customXml"];

/// Copies parts and keeps the package's indexes in step. Parts are found
/// through a name index, the content types and the workbook's relationships
/// are edited in memory and written once by [`Copier::finish`], and every
/// write counts against the package limits, so a loop over many items stays
/// linear and stops as soon as its copies pass a limit.
struct Copier<'a> {
    entries: &'a mut Vec<Entry>,
    /// Each entry's position in `entries`.
    index: HashMap<String, usize>,
    /// Bytes the entries hold.
    total: u64,
    /// Every part name in use or handed out.
    names: HashSet<String>,
    /// The next number to try for each `dir/base.ext` pattern.
    counters: HashMap<String, u32>,
    types: Vec<Node>,
    /// Override content types by part name, for copies to take theirs.
    overrides: HashMap<String, String>,
    types_changed: bool,
    /// Workbook relationships to add: `(id, type, target)`.
    new_rels: Vec<(String, String, String)>,
    /// Sheet parts whose workbook relationship goes.
    dead_sheets: Vec<String>,
    next_table_id: u32,
    table_names: HashSet<String>,
    next_vml_block: u32,
}

fn too_many_parts(n: usize) -> TemplateError {
    TemplateError::new(format!(
        "the sheet loops make {n} parts; a package holds at most {MAX_ENTRIES}"
    ))
}

fn too_many_bytes() -> TemplateError {
    TemplateError::new(format!(
        "the sheet loops make the package pass its limit of {MAX_TOTAL_BYTES} bytes"
    ))
}

impl<'a> Copier<'a> {
    fn new(entries: &'a mut Vec<Entry>) -> Self {
        let mut next_table_id = 1;
        let mut table_names = HashSet::new();
        let mut next_vml_block = 1;
        let idmap = Regex::new(r#"<o:idmap[^>]*\bdata="([0-9, ]+)""#).expect("valid");
        for e in entries.iter() {
            if e.name.starts_with("xl/tables/") {
                let table = std::str::from_utf8(&e.data)
                    .ok()
                    .and_then(|s| xml::parse(s).ok());
                if let Some(root) = table.as_deref().and_then(xml::root) {
                    if let Some(id) = root.attr("id").and_then(|v| v.parse::<u32>().ok()) {
                        next_table_id = next_table_id.max(id + 1);
                    }
                    for a in ["name", "displayName"] {
                        if let Some(n) = root.attr(a) {
                            table_names.insert(n.to_lowercase());
                        }
                    }
                }
            }
            if e.name.ends_with(".vml") {
                let text = String::from_utf8_lossy(&e.data);
                for c in idmap.captures_iter(&text) {
                    for n in c[1].split(',').filter_map(|n| n.trim().parse::<u32>().ok()) {
                        next_vml_block = next_vml_block.max(n + 1);
                    }
                }
            }
        }
        let types = package::text_of(entries, CONTENT_TYPES)
            .and_then(|s| xml::parse(&s).ok())
            .unwrap_or_default();
        let overrides = xml::root(&types)
            .map(|r| {
                r.children_named("Override")
                    .filter_map(|o| {
                        let part = o.attr("PartName")?;
                        Some((
                            part.trim_start_matches('/').to_string(),
                            o.attr("ContentType")?,
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut copier = Copier {
            index: HashMap::new(),
            total: 0,
            names: entries.iter().map(|e| e.name.clone()).collect(),
            entries,
            counters: HashMap::new(),
            types,
            overrides,
            types_changed: false,
            new_rels: Vec::new(),
            dead_sheets: Vec::new(),
            next_table_id,
            table_names,
            next_vml_block,
        };
        copier.reindex();
        copier
    }

    fn reindex(&mut self) {
        self.index = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.name.clone(), i))
            .collect();
        self.total = self.entries.iter().map(|e| e.data.len() as u64).sum();
    }

    fn get(&self, name: &str) -> Option<&Entry> {
        self.index.get(name).map(|&i| &self.entries[i])
    }

    fn text(&self, name: &str) -> Option<String> {
        self.get(name)
            .and_then(|e| String::from_utf8(e.data.clone()).ok())
    }

    /// `(type, resolved target)` of each relationship in `part`'s rels.
    fn relationships(&self, part: &str) -> Vec<(String, String)> {
        let base = part.rsplit_once('/').map_or("", |(d, _)| d);
        let Some(nodes) = self
            .text(&package::rels_name(part))
            .and_then(|s| xml::parse(&s).ok())
        else {
            return Vec::new();
        };
        let Some(root) = xml::root(&nodes) else {
            return Vec::new();
        };
        root.children_named("Relationship")
            .filter(|r| r.attr("TargetMode").as_deref() != Some("External"))
            .filter_map(|r| Some((r.attr("Type")?, package::resolve(base, &r.attr("Target")?))))
            .collect()
    }

    /// Writes a part, adding it when it is new, and stops with an error once
    /// the package passes a limit.
    fn put(&mut self, name: &str, data: Vec<u8>) -> Result<(), TemplateError> {
        let len = data.len() as u64;
        match self.index.get(name) {
            Some(&i) => {
                self.total = self.total - self.entries[i].data.len() as u64 + len;
                self.entries[i].data = data;
            }
            None => {
                self.names.insert(name.to_string());
                self.index.insert(name.to_string(), self.entries.len());
                self.entries.push(Entry {
                    name: name.to_string(),
                    data,
                    stored: false,
                });
                self.total += len;
            }
        }
        if self.entries.len() > MAX_ENTRIES {
            return Err(too_many_parts(self.entries.len()));
        }
        if self.total > MAX_TOTAL_BYTES {
            return Err(too_many_bytes());
        }
        Ok(())
    }

    /// The XML parts a sheet owns and would copy, with their relationship
    /// parts: what each copy adds, as `(parts, bytes)`.
    fn owned(&self, part: &str) -> (usize, u64) {
        let mut owned = vec![part.to_string()];
        let mut i = 0;
        while i < owned.len() {
            for (ty, target) in self.relationships(&owned[i]) {
                let xml_part = target.ends_with(".xml") || target.ends_with(".vml");
                if xml_part
                    && !SHARED_TYPES.iter().any(|s| ty.ends_with(s))
                    && self.index.contains_key(&target)
                    && !owned.contains(&target)
                {
                    owned.push(target);
                }
            }
            i += 1;
        }
        let mut parts = 0;
        let mut bytes = 0;
        // The sheet itself is counted by the caller, from what it rendered.
        for (k, p) in owned.iter().enumerate() {
            if k > 0 {
                if let Some(e) = self.get(p) {
                    parts += 1;
                    bytes += e.data.len() as u64;
                }
            }
            if let Some(e) = self.get(&package::rels_name(p)) {
                parts += 1;
                bytes += e.data.len() as u64;
            }
        }
        (parts, bytes)
    }

    /// Refuses copies that cannot fit before any is built: each copy takes
    /// its rendered sheet and a copy of every part its template sheet owns.
    /// `copies` gives each copy's template part and rendered size.
    fn check_room<'s>(
        &self,
        copies: impl Iterator<Item = (&'s str, usize)>,
    ) -> Result<(), TemplateError> {
        let mut seen: HashMap<&str, (usize, u64)> = HashMap::new();
        let mut parts = self.entries.len();
        let mut bytes = self.total;
        for (template, size) in copies {
            let (p, b) = *seen.entry(template).or_insert_with(|| self.owned(template));
            parts += 1 + p;
            bytes = bytes.saturating_add(size as u64 + b);
            if parts > MAX_ENTRIES {
                return Err(TemplateError::new(format!(
                    "the sheet loops would make {parts} parts or more; a package holds at most {MAX_ENTRIES}"
                )));
            }
            if bytes > MAX_TOTAL_BYTES {
                return Err(TemplateError::new(format!(
                    "the sheet loops would make the package pass its limit of {MAX_TOTAL_BYTES} bytes"
                )));
            }
        }
        Ok(())
    }

    /// A part name like `like` (`xl/charts/chart1.xml`) that is free.
    fn fresh(&mut self, like: &str) -> String {
        let (dir, file) = like.rsplit_once('/').unwrap_or(("", like));
        let (stem, ext) = file.rsplit_once('.').unwrap_or((file, ""));
        let base = stem.trim_end_matches(|c: char| c.is_ascii_digit());
        let prefix = if dir.is_empty() {
            base.to_string()
        } else {
            format!("{dir}/{base}")
        };
        let n = self.counters.entry(format!("{prefix}.{ext}")).or_insert(1);
        loop {
            let candidate = format!("{prefix}{n}.{ext}");
            *n += 1;
            if self.names.insert(candidate.clone()) {
                return candidate;
            }
        }
    }

    /// Gives the copy `new` of sheet `old` its own relationships: XML parts
    /// the sheet owns (drawings, charts, tables, comments) are copied with
    /// their own relationships and pointed at the copy's name, and pictures
    /// and other binary parts are shared. Returns the tables it renamed.
    fn copy_rels(
        &mut self,
        old: &str,
        new: &str,
        old_name: &str,
        new_name: &str,
    ) -> Result<Vec<(String, String)>, TemplateError> {
        let mut tables = Vec::new();
        let mut done = HashMap::new();
        let renames = one(old_name, new_name);
        self.copy_rels_of(old, new, &renames, &mut done, &mut tables)?;
        Ok(tables)
    }

    fn copy_rels_of(
        &mut self,
        old: &str,
        new: &str,
        renames: &Renames,
        done: &mut HashMap<String, String>,
        tables: &mut Vec<(String, String)>,
    ) -> Result<(), TemplateError> {
        let Some(src) = self.text(&package::rels_name(old)) else {
            return Ok(());
        };
        let Ok(mut nodes) = xml::parse(&src) else {
            return Ok(());
        };
        let base = old.rsplit_once('/').map_or("", |(d, _)| d).to_string();
        let mut copies = Vec::new();
        if let Some(root) = xml::root_mut(&mut nodes) {
            for n in root.children.iter_mut() {
                let Node::Elem(r) = n else { continue };
                let (Some(target), Some(ty)) = (r.attr("Target"), r.attr("Type")) else {
                    continue;
                };
                if r.attr("TargetMode").as_deref() == Some("External")
                    || SHARED_TYPES.iter().any(|s| ty.ends_with(s))
                {
                    continue;
                }
                let part = package::resolve(&base, &target);
                if !(part.ends_with(".xml") || part.ends_with(".vml"))
                    || !self.index.contains_key(&part)
                {
                    continue;
                }
                let copy = match done.get(&part) {
                    Some(c) => c.clone(),
                    None => {
                        let c = self.fresh(&part);
                        done.insert(part.clone(), c.clone());
                        copies.push((part.clone(), c.clone()));
                        c
                    }
                };
                let file = copy.rsplit_once('/').map_or(copy.as_str(), |(_, f)| f);
                let new_target = match target.rsplit_once('/') {
                    Some((dir, _)) => format!("{dir}/{file}"),
                    None => file.to_string(),
                };
                r.set_attr("Target", &new_target);
            }
        }
        self.put(&package::rels_name(new), xml::to_xml(&nodes).into_bytes())?;
        for (part, copy) in copies {
            let Some(data) = self.get(&part).map(|e| e.data.clone()) else {
                continue;
            };
            let text = String::from_utf8_lossy(&data).into_owned();
            let text = rename_xml(&text, renames);
            let text = if part.starts_with("xl/tables/") {
                self.fix_table(&text, tables)
            } else if part.ends_with(".vml") {
                self.fix_vml(&text)
            } else {
                text
            };
            self.put(&copy, text.into_bytes())?;
            self.content_type_like(&part, &copy);
            self.copy_rels_of(&part, &copy, renames, done, tables)?;
        }
        Ok(())
    }

    /// A copied table needs an id and a name no other table has.
    fn fix_table(&mut self, xml: &str, renamed: &mut Vec<(String, String)>) -> String {
        let Ok(mut nodes) = xml::parse(xml) else {
            return xml.to_string();
        };
        if let Some(root) = xml::root_mut(&mut nodes) {
            root.set_attr("id", &self.next_table_id.to_string());
            self.next_table_id += 1;
            let old = root
                .attr("displayName")
                .or_else(|| root.attr("name"))
                .unwrap_or_else(|| "Table".to_string());
            let mut n = 2;
            let mut new = format!("{old}_{n}");
            while self.table_names.contains(&new.to_lowercase()) {
                n += 1;
                new = format!("{old}_{n}");
            }
            self.table_names.insert(new.to_lowercase());
            root.set_attr("name", &new);
            root.set_attr("displayName", &new);
            renamed.push((old, new));
        }
        xml::to_xml(&nodes)
    }

    /// A copied comment drawing needs its own block of shape ids.
    fn fix_vml(&mut self, xml: &str) -> String {
        let idmap = Regex::new(r#"(<o:idmap[^>]*\bdata=")([0-9]+)"#).expect("valid");
        let Some(old) = idmap.captures(xml).and_then(|c| c[2].parse::<i64>().ok()) else {
            return xml.to_string();
        };
        let new = self.next_vml_block as i64;
        self.next_vml_block += 1;
        let xml = idmap.replace(xml, |c: &regex::Captures| format!("{}{new}", &c[1]));
        let spid = Regex::new(r"_x0000_s([0-9]+)").expect("valid");
        spid.replace_all(&xml, |c: &regex::Captures| {
            let id = c[1].parse::<i64>().unwrap_or(0);
            format!("_x0000_s{}", id - old * 1024 + new * 1024)
        })
        .into_owned()
    }

    /// Gives `copy` the content-type override `part` has, if any.
    fn content_type_like(&mut self, part: &str, copy: &str) {
        let Some(ct) = self.overrides.get(part).cloned() else {
            return;
        };
        if let Some(root) = xml::root_mut(&mut self.types) {
            root.children.push(Node::Elem(Elem::new(
                "Override",
                &[("PartName", &format!("/{copy}")), ("ContentType", &ct)],
                true,
            )));
            self.overrides.insert(copy.to_string(), ct);
            self.types_changed = true;
        }
    }

    /// Points the parts the first copy owns, such as its charts, at the
    /// sheet's new name.
    fn rename_owned(
        &mut self,
        part: &str,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), TemplateError> {
        let renames = one(old_name, new_name);
        if renames.is_empty() {
            return Ok(());
        }
        let mut seen = HashSet::new();
        let mut todo = vec![part.to_string()];
        while let Some(p) = todo.pop() {
            if !seen.insert(p.clone()) {
                continue;
            }
            for (ty, target) in self.relationships(&p) {
                if SHARED_TYPES.iter().any(|s| ty.ends_with(s)) || !target.ends_with(".xml") {
                    continue;
                }
                if let Some(text) = self.text(&target) {
                    let renamed = rename_xml(&text, &renames);
                    if renamed != text {
                        self.put(&target, renamed.into_bytes())?;
                    }
                }
                todo.push(target);
            }
        }
        Ok(())
    }

    /// Removes a sheet that came out zero times, with the parts only it owns.
    fn remove_sheet(&mut self, part: &str) {
        let mut owned = vec![part.to_string()];
        let mut i = 0;
        while i < owned.len() {
            for (ty, target) in self.relationships(&owned[i]) {
                let xml_part = target.ends_with(".xml") || target.ends_with(".vml");
                if xml_part
                    && !SHARED_TYPES.iter().any(|s| ty.ends_with(s))
                    && !owned.contains(&target)
                {
                    owned.push(target);
                }
            }
            i += 1;
        }
        let mut dead: HashSet<String> = HashSet::new();
        for p in &owned {
            dead.insert(p.clone());
            dead.insert(package::rels_name(p));
            if self.overrides.remove(p).is_some() {
                let wanted = format!("/{p}");
                if let Some(root) = xml::root_mut(&mut self.types) {
                    root.children.retain(|n| {
                        !matches!(n, Node::Elem(e) if e.name == "Override"
                            && e.attr("PartName").as_deref() == Some(wanted.as_str()))
                    });
                }
                self.types_changed = true;
            }
        }
        self.entries.retain(|e| !dead.contains(&e.name));
        self.reindex();
        self.dead_sheets.push(part.to_string());
    }

    /// Lists the new worksheet names in `docProps/app.xml`, when its first
    /// heading counts the template's worksheets as Excel writes it. A file
    /// laid out otherwise is left as it is: Excel rebuilds the list on save.
    fn update_app(
        &mut self,
        template_worksheets: usize,
        names: &[String],
        first_name: &[(String, String)],
    ) -> Result<(), TemplateError> {
        let Some(src) = self.text(APP) else {
            return Ok(());
        };
        let Ok(mut nodes) = xml::parse(&src) else {
            return Ok(());
        };
        let Some(root) = xml::root_mut(&mut nodes) else {
            return Ok(());
        };
        let count = root
            .find("HeadingPairs")
            .and_then(|h| h.find("vt:vector"))
            .and_then(|v| v.children_named("vt:variant").nth(1))
            .and_then(|v| v.find("vt:i4"))
            .and_then(|i| i.text_content().trim().parse::<usize>().ok());
        if count != Some(template_worksheets) {
            return Ok(());
        }
        let Some(titles) = root
            .find_mut("TitlesOfParts")
            .and_then(|t| t.find_mut("vt:vector"))
        else {
            return Ok(());
        };
        let rest: Vec<Node> = titles
            .children
            .iter()
            .filter(|n| matches!(n, Node::Elem(e) if e.name == "vt:lpstr"))
            .skip(template_worksheets)
            .cloned()
            .map(|n| match n {
                Node::Elem(mut e) => {
                    let t = rename_text(&e.text_content(), first_name, '!');
                    set_text(&mut e, &t);
                    Node::Elem(e)
                }
                other => other,
            })
            .collect();
        let mut children: Vec<Node> = names
            .iter()
            .map(|n| {
                let mut e = Elem::new("vt:lpstr", &[], false);
                set_text(&mut e, n);
                Node::Elem(e)
            })
            .collect();
        let size = children.len() + rest.len();
        children.extend(rest);
        titles.make_open();
        titles.children = children;
        titles.set_attr("size", &size.to_string());
        let count = root
            .find_mut("HeadingPairs")
            .and_then(|h| h.find_mut("vt:vector"))
            .and_then(|v| {
                v.children
                    .iter_mut()
                    .filter_map(|n| match n {
                        Node::Elem(e) if e.name == "vt:variant" => Some(e),
                        _ => None,
                    })
                    .nth(1)
            })
            .and_then(|v| v.find_mut("vt:i4"));
        if let Some(i4) = count {
            set_text(i4, &names.len().to_string());
        }
        self.put(APP, xml::to_xml(&nodes).into_bytes())
    }

    /// Writes the content types and workbook relationships.
    fn finish(mut self) -> Result<(), TemplateError> {
        if self.types_changed {
            let types = xml::to_xml(&self.types).into_bytes();
            self.put(CONTENT_TYPES, types)?;
        }
        if !self.new_rels.is_empty() || !self.dead_sheets.is_empty() {
            if let Some(mut nodes) = self.text(WORKBOOK_RELS).and_then(|s| xml::parse(&s).ok()) {
                if let Some(root) = xml::root_mut(&mut nodes) {
                    let dead = &self.dead_sheets;
                    root.children.retain(|n| match n {
                        Node::Elem(e) => !e
                            .attr("Target")
                            .is_some_and(|t| dead.contains(&package::resolve("xl", &t))),
                        _ => true,
                    });
                    for (id, ty, target) in &self.new_rels {
                        root.children.push(Node::Elem(Elem::new(
                            "Relationship",
                            &[("Id", id), ("Type", ty), ("Target", target)],
                            true,
                        )));
                    }
                }
                self.put(WORKBOOK_RELS, xml::to_xml(&nodes).into_bytes())?;
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ODS after rendering.

/// Names the sheets of a rendered `content.xml`, re-bases the formulas of
/// sheets with row blocks, and points references at renamed sheets.
pub fn finish_ods(content: &str, blocks: &[crate::program::RowBlock]) -> Result<String, String> {
    if !content.contains(TITLE_OPEN) && !crate::sheet::ods_marked(content) {
        return Ok(content.to_string());
    }
    let mut nodes = xml::parse(content)?;
    let Some(list_path) = sheet_list_path(&nodes) else {
        return Ok(content.to_string());
    };
    let Some(list) = elem_at_mut(&mut nodes, &list_path) else {
        return Ok(content.to_string());
    };
    // (child index, template name, printed name)
    let mut sheets: Vec<(usize, String, Option<String>)> = Vec::new();
    for (k, n) in list.children.iter_mut().enumerate() {
        let Node::Elem(table) = n else { continue };
        if table.name != "table:table" {
            continue;
        }
        crate::sheet::rebase_ods(table, blocks);
        let mut title = None;
        if let Some(Node::Text(t)) = table.children.first_mut() {
            let (printed, rest) = split_title(&t.raw);
            if printed.is_some() {
                title = printed;
                t.raw = rest.to_string();
            }
        }
        if matches!(table.children.first(), Some(Node::Text(t)) if t.raw.is_empty()) {
            table.children.remove(0);
        }
        sheets.push((k, table.attr("table:name").unwrap_or_default(), title));
    }
    let mut taken: HashSet<String> = sheets
        .iter()
        .filter(|(_, _, t)| t.is_none())
        .map(|(_, n, _)| n.to_lowercase())
        .collect();
    let mut first_name: HashMap<String, String> = HashMap::new();
    for (k, old, title) in &sheets {
        let Some(title) = title else { continue };
        let name = unique_name(&valid_name(title), &mut taken);
        if let Some(Node::Elem(table)) = list.children.get_mut(*k) {
            table.set_attr("table:name", &name);
            rename_nodes(
                std::slice::from_mut(&mut list.children[*k]),
                &one(old, &name),
                '.',
                false,
            );
        }
        first_name.entry(old.clone()).or_insert(name);
    }
    // References elsewhere, in other sheets' formulas and named ranges,
    // point at a renamed sheet's first copy. They sit in attributes.
    let elsewhere: Vec<(String, String)> = first_name
        .into_iter()
        .filter(|(old, first)| old != first)
        .collect();
    rename_nodes(&mut nodes, &elsewhere, '.', false);
    Ok(xml::to_xml(&nodes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_valid() {
        assert_eq!(
            valid_name("Paris: north/south [1]?*"),
            "Paris_ north_south _1___"
        );
        assert_eq!(valid_name("  'quoted'  "), "quoted");
        assert_eq!(valid_name(""), "Sheet");
        assert_eq!(valid_name("History"), "History_");
        let long = "A very long site name that does not fit";
        assert_eq!(valid_name(long), "A very long site name that does");
        assert_eq!(valid_name(long).encode_utf16().count(), 31);
        // Characters outside the BMP count twice, as Excel counts them.
        let emoji = "😀".repeat(20);
        assert_eq!(valid_name(&emoji).encode_utf16().count(), 30);
    }

    #[test]
    fn names_are_made_unique_without_regard_to_case() {
        let mut taken = HashSet::new();
        taken.insert("summary".to_string());
        assert_eq!(unique_name("Summary", &mut taken), "Summary (2)");
        assert_eq!(unique_name("SUMMARY", &mut taken), "SUMMARY (3)");
        assert_eq!(unique_name("Paris", &mut taken), "Paris");
        let long = "x".repeat(31);
        let mut taken = HashSet::new();
        assert_eq!(unique_name(&long, &mut taken), long);
        let second = unique_name(&long, &mut taken);
        assert_eq!(second, format!("{} (2)", "x".repeat(27)));
        assert_eq!(second.len(), 31);
    }

    #[test]
    fn references_follow_a_renamed_sheet() {
        let xml = "<r><f>'{{ s.name }}'!A1+'O''Brien {{ x }}'!B2</f><v a=\"&apos;{{ s.name }}&apos;!A1\" b=\"x\"/></r>";
        let out = rename_xml(xml, &one("O'Brien {{ x }}", "D'Arcy"));
        assert!(out.contains("'D''Arcy'!B2"), "{out}");
        // A name that holds markup characters stays inside its text node and
        // its attribute value.
        let out = rename_xml(xml, &one("{{ s.name }}", "A\"b<c&d'e"));
        assert!(out.contains("<f>'A\"b&lt;c&amp;d''e'!A1"), "{out}");
        assert!(
            out.contains("<v a=\"'A&quot;b&lt;c&amp;d''e'!A1\" b=\"x\"/>"),
            "{out}"
        );
        let nodes = xml::parse(&out).unwrap();
        let v = xml::root(&nodes).unwrap().child("v").unwrap();
        assert_eq!(v.attr("a").as_deref(), Some("'A\"b<c&d''e'!A1"));
        assert_eq!(v.attr("b").as_deref(), Some("x"));
        // Nothing to rename leaves the bytes alone.
        assert_eq!(rename_xml(xml, &one("Other", "New")), xml);
    }

    #[test]
    fn a_title_is_cut_off_the_front() {
        let (t, rest) = split_title("\u{3}R&amp;D\u{4}<?xml?>");
        assert_eq!(t.as_deref(), Some("R&D"));
        assert_eq!(rest, "<?xml?>");
        assert_eq!(split_title("<a/>"), (None, "<a/>"));
    }
}
