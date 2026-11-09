//! From XML parts to one minijinja template.
//!
//! Per paragraph the text of the runs is joined so that a tag Word split
//! across runs becomes whole again; tags are then cut out of their text
//! nodes as pieces, blocks are matched, and the block tags are hoisted onto
//! the row, paragraph, slide or part they should repeat. The parts are
//! finally written out as a single template source in which every tag
//! starts on its own line, so an error line maps back to a tag.

use std::sync::LazyLock;

use regex::Regex;

use crate::formats::Dialect;
use crate::xml::{self, elem_at, elem_at_mut, node_at_mut, Elem, Node, Path, Piece, TagId, Text};
use crate::{Diagnostic, Format, Location, Settings, Severity};

static TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)\{\{.*?\}\}|\{%.*?%\}|\{#.*?#\}").unwrap());
static SETTING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"([\w.\-]+)\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap());
static FOR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)^(.+?)\s+in\s+(.+?)(\s+if\s+.+?)?(\s+recursive)?\s*$").unwrap()
});
static SET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)^([^=]+?)\s*=\s*(.+)$").unwrap());

/// Separates parts in the rendered output; it cannot occur in XML.
pub const MARK: char = '\u{0}';

#[derive(Debug, Clone)]
pub struct Tag {
    /// The normalised tag text, such as `{{ customer.name }}`.
    pub text: String,
    pub kind: TagKind,
    pub loc: Location,
}

#[derive(Debug, Clone)]
pub enum TagKind {
    Expr(String),
    Stmt {
        name: String,
        rest: String,
    },
    Comment,
    Settings,
    /// `{% sheet for s in sites %}`: repeats the sheet it sits in, once per
    /// item. Holds what follows `sheet`, which should start with `for`.
    SheetLoop(String),
    /// The end of a sheet loop. Authors never type it: the engine adds one
    /// after the sheet a [`TagKind::SheetLoop`] repeats.
    SheetEnd,
}

const OPENERS: &[&str] = &[
    "for",
    "if",
    "with",
    "macro",
    "raw",
    "filter",
    "block",
    "call",
    "autoescape",
];

impl Tag {
    pub fn parse(raw: &str, loc: Location) -> Tag {
        let text: String = raw
            .chars()
            .map(|c| match c {
                '\u{2018}' | '\u{2019}' | '\u{201A}' => '\'',
                '\u{201C}' | '\u{201D}' | '\u{201E}' => '"',
                '\u{00A0}' | '\u{2007}' | '\u{202F}' => ' ',
                c => c,
            })
            .collect();
        let inner = text[2..text.len() - 2].trim().trim_matches('-').trim();
        let kind = if text.starts_with("{{") {
            TagKind::Expr(inner.to_string())
        } else if text.starts_with("{#") {
            TagKind::Comment
        } else {
            let (name, rest) = match inner.find(char::is_whitespace) {
                Some(p) => (&inner[..p], inner[p..].trim()),
                None => (inner, ""),
            };
            if name == "settings" {
                TagKind::Settings
            } else if name == "sheet" {
                TagKind::SheetLoop(rest.to_string())
            } else {
                TagKind::Stmt {
                    name: name.to_string(),
                    rest: rest.to_string(),
                }
            }
        };
        Tag { text, kind, loc }
    }

    fn stmt(&self) -> Option<(&str, &str)> {
        match &self.kind {
            TagKind::Stmt { name, rest } => Some((name, rest)),
            _ => None,
        }
    }

    /// The block this tag opens, if any.
    pub fn opener(&self) -> Option<&str> {
        let (name, rest) = self.stmt()?;
        if OPENERS.contains(&name) || (name == "set" && !rest.contains('=')) {
            Some(name)
        } else {
            None
        }
    }

    /// The block this tag closes, if any.
    pub fn closer(&self) -> Option<&str> {
        self.stmt()?.0.strip_prefix("end")
    }

    pub fn is_middle(&self) -> bool {
        matches!(self.stmt(), Some(("else" | "elif", _)))
    }

    pub fn is_for(&self) -> bool {
        matches!(self.stmt(), Some(("for", _)))
    }

    /// The loop head of a sheet loop, `s in sites` for
    /// `{% sheet for s in sites %}`, when it is written that way.
    pub fn sheet_loop(&self) -> Option<&str> {
        match &self.kind {
            TagKind::SheetLoop(rest) => rest
                .strip_prefix("for")
                .filter(|r| r.starts_with(char::is_whitespace))
                .map(str::trim)
                .filter(|r| FOR_RE.is_match(r)),
            _ => None,
        }
    }

    /// A tag the engine made up, which the tag listing leaves out.
    pub fn is_synthetic(&self) -> bool {
        matches!(self.kind, TagKind::SheetEnd)
    }
}

/// Where a tag sits, as paths into its part's tree.
#[derive(Debug, Clone)]
pub struct TagCtx {
    pub part: usize,
    pub text: Path,
    pub para: Path,
    pub row: Option<Path>,
    pub table: Option<Path>,
}

/// A block of rows in a sheet whose copies must be renumbered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowBlock {
    pub start: u32,
    pub end: u32,
    pub is_loop: bool,
    /// Which sheet the rows belong to when one part holds several: the
    /// sheet's child index under `office:spreadsheet` in ODS, 0 in XLSX.
    pub sheet: usize,
}

/// One ZIP part with its tree and the tags hoisted to its boundaries.
#[derive(Debug, Clone)]
pub struct PartTree {
    pub name: String,
    pub nodes: Vec<Node>,
    pub before: Vec<TagId>,
    pub after: Vec<TagId>,
    pub row_blocks: Vec<RowBlock>,
}

/// Byte spans of the tags in a text.
pub fn find_tags(text: &str) -> Vec<(usize, usize)> {
    TAG_RE
        .find_iter(text)
        .map(|m| (m.start(), m.end()))
        .collect()
}

pub fn diag(severity: Severity, message: String, loc: Option<Location>) -> Diagnostic {
    Diagnostic {
        severity,
        message,
        location: loc,
    }
}

// ---------------------------------------------------------------------------
// Phase A: joining split runs.

#[derive(Debug)]
struct Frag {
    path: Path,
    chain: Vec<String>,
    text: String,
    space: bool,
    dirty: bool,
}

fn find_paragraphs(nodes: &[Node], d: &Dialect, prefix: &Path, out: &mut Vec<Path>) {
    for (i, n) in nodes.iter().enumerate() {
        if let Node::Elem(e) = n {
            let mut path = prefix.clone();
            path.push(i);
            if d.paragraphs.contains(&e.name.as_str()) {
                out.push(path.clone());
            }
            find_paragraphs(&e.children, d, &path, out);
        }
    }
}

fn collect_fragments(
    nodes: &[Node],
    prefix: &Path,
    chain: &mut Vec<String>,
    d: &Dialect,
    out: &mut Vec<Frag>,
) {
    for (i, n) in nodes.iter().enumerate() {
        let mut path = prefix.clone();
        path.push(i);
        match n {
            Node::Text(t) => {
                let ok = match d.text_leaves {
                    None => true,
                    Some(leaves) => chain.last().is_some_and(|n| leaves.contains(&n.as_str())),
                };
                if ok {
                    out.push(Frag {
                        path,
                        chain: chain.clone(),
                        text: xml::unescape(&t.raw).into_owned(),
                        space: false,
                        dirty: false,
                    });
                }
            }
            Node::Elem(e) => {
                if d.paragraphs.contains(&e.name.as_str()) {
                    continue;
                }
                if let Some((_, s)) = d.space_like.iter().find(|(n, _)| *n == e.name) {
                    let count = e
                        .attr("text:c")
                        .and_then(|c| c.parse::<usize>().ok())
                        .unwrap_or(1);
                    out.push(Frag {
                        path,
                        chain: chain.clone(),
                        text: s.repeat(count),
                        space: true,
                        dirty: false,
                    });
                    continue;
                }
                chain.push(e.name.clone());
                collect_fragments(&e.children, &path, chain, d, out);
                chain.pop();
            }
            Node::Raw(_) => {}
        }
    }
}

fn concat(frags: &[Frag]) -> (String, Vec<usize>) {
    let mut full = String::new();
    let mut starts = Vec::with_capacity(frags.len());
    for f in frags {
        starts.push(full.len());
        full.push_str(&f.text);
    }
    (full, starts)
}

fn frag_index(starts: &[usize], offset: usize) -> usize {
    match starts.binary_search(&offset) {
        Ok(i) => {
            // Several empty fragments may share an offset; take the last.
            let mut i = i;
            while i + 1 < starts.len() && starts[i + 1] == offset {
                i += 1;
            }
            i
        }
        Err(i) => i - 1,
    }
}

/// Moves the text of fragments `i..=j` (up to `end` bytes into `j`) into
/// fragment `i`.
fn merge(frags: &mut [Frag], i: usize, j: usize, end: usize) {
    let mut joined = String::new();
    for f in frags.iter_mut().take(j).skip(i + 1) {
        joined.push_str(&f.text);
        f.text.clear();
        f.dirty = true;
    }
    let tail = frags[j].text.split_off(end);
    joined.push_str(&frags[j].text);
    frags[j].text = tail;
    frags[j].dirty = true;
    frags[i].text.push_str(&joined);
    frags[i].dirty = true;
}

/// Joins runs so every tag, and every block whose tags sit in runs of
/// different shape, lies in one text node.
fn join_runs(frags: &mut [Frag]) {
    loop {
        let (full, starts) = concat(frags);
        let split = find_tags(&full).into_iter().find_map(|(s, e)| {
            let (i, j) = (frag_index(&starts, s), frag_index(&starts, e - 1));
            (i != j).then_some((i, j, e - starts[j]))
        });
        match split {
            Some((i, j, end)) => merge(frags, i, j, end),
            None => break,
        }
    }
    loop {
        let (full, starts) = concat(frags);
        let spans = find_tags(&full);
        let tags: Vec<Tag> = spans
            .iter()
            .map(|&(s, e)| Tag::parse(&full[s..e], Location::default()))
            .collect();
        let mut stack: Vec<(usize, Vec<usize>)> = Vec::new();
        let mut split = None;
        for (k, t) in tags.iter().enumerate() {
            if t.opener().is_some() {
                stack.push((k, Vec::new()));
            } else if t.is_middle() {
                if let Some(top) = stack.last_mut() {
                    top.1.push(k);
                }
            } else if let Some(name) = t.closer() {
                if let Some(pos) = stack
                    .iter()
                    .rposition(|(o, _)| tags[*o].opener() == Some(name))
                {
                    let (o, mids) = stack.remove(pos);
                    stack.truncate(pos);
                    let fo = frag_index(&starts, spans[o].0);
                    let fc = frag_index(&starts, spans[k].1 - 1);
                    let same = std::iter::once(fc)
                        .chain(mids.iter().map(|&m| frag_index(&starts, spans[m].0)))
                        .all(|f| frags[f].chain == frags[fo].chain);
                    if !same {
                        split = Some((fo, fc, spans[k].1 - starts[fc]));
                        break;
                    }
                }
            }
        }
        match split {
            Some((i, j, end)) => merge(frags, i, j, end),
            None => break,
        }
    }
}

fn write_back(nodes: &mut [Node], frags: &[Frag], d: &Dialect) {
    for f in frags.iter().filter(|f| f.dirty) {
        let Some(node) = node_at_mut(nodes, &f.path) else {
            continue;
        };
        if f.space {
            *node = Node::Text(Text {
                raw: String::new(),
                pieces: None,
            });
        } else if let Node::Text(t) = node {
            t.raw = xml::escape_text(&f.text);
        }
        if d.preserve_space && !f.space {
            if let Some(parent) = elem_at_mut(nodes, &f.path[..f.path.len() - 1]) {
                parent.set_attr("xml:space", "preserve");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Phase B: cutting tags out of text nodes.

struct Walker<'a> {
    d: &'a Dialect,
    part: usize,
    part_name: &'a str,
    prefix: &'a str,
    tags: &'a mut Vec<Tag>,
    ctxs: &'a mut Vec<TagCtx>,
    settings: &'a mut Settings,
    diags: &'a mut Vec<Diagnostic>,
    anc: Vec<(String, Path)>,
    paragraphs: usize,
    tables: usize,
    slides: usize,
    table_rows: Vec<usize>,
    cell: Option<String>,
    preserve: Vec<Path>,
}

impl Walker<'_> {
    fn walk(&mut self, nodes: &mut [Node], prefix: &Path) {
        for (i, node) in nodes.iter_mut().enumerate() {
            let mut path = prefix.clone();
            path.push(i);
            match node {
                Node::Elem(e) => {
                    let name = e.name.clone();
                    let is_table = self.d.tables.contains(&name.as_str());
                    if self.d.paragraphs.contains(&name.as_str()) {
                        self.paragraphs += 1;
                    }
                    if is_table {
                        self.tables += 1;
                        self.table_rows.push(0);
                    }
                    if self.d.rows.contains(&name.as_str()) {
                        if let Some(r) = self.table_rows.last_mut() {
                            *r += 1;
                        }
                    }
                    if self.d.slide == Some(name.as_str()) {
                        self.slides += 1;
                    }
                    if self.d.sheet && name == "c" {
                        self.cell = e.attr("r");
                    }
                    self.anc.push((name, path.clone()));
                    self.walk(&mut e.children, &path);
                    self.anc.pop();
                    if is_table {
                        self.table_rows.pop();
                    }
                }
                Node::Text(t) => self.text(t, path),
                Node::Raw(_) => {}
            }
        }
    }

    fn nearest(&self, names: &[&str]) -> Option<Path> {
        self.anc
            .iter()
            .rev()
            .find(|(n, _)| names.contains(&n.as_str()))
            .map(|(_, p)| p.clone())
    }

    fn place(&self) -> String {
        let mut s = self.prefix.to_string();
        if self.d.sheet {
            let cell = self.cell.clone().unwrap_or_default();
            let row: String = cell.chars().filter(|c| c.is_ascii_digit()).collect();
            s.push_str(&format!("row {row} (cell {cell})"));
            return s;
        }
        if self.d.slide.is_some() {
            s.push_str(&format!("slide {} ", self.slides));
        }
        if let Some(rows) = self.table_rows.last() {
            s.push_str(&format!("table {} row {} ", self.tables, rows));
        }
        s.push_str(&format!("paragraph {}", self.paragraphs));
        s
    }

    fn text(&mut self, t: &mut Text, path: Path) {
        let Some(para) = self.nearest(self.d.paragraphs) else {
            return;
        };
        if let Some(leaves) = self.d.text_leaves {
            if !self
                .anc
                .last()
                .is_some_and(|(n, _)| leaves.contains(&n.as_str()))
            {
                return;
            }
        }
        let text = xml::unescape(&t.raw).into_owned();
        if !text.contains('{') {
            return;
        }
        let spans = find_tags(&text);
        let outside = {
            let mut rest = String::new();
            let mut last = 0;
            for &(s, e) in &spans {
                rest.push_str(&text[last..s]);
                last = e;
            }
            rest.push_str(&text[last..]);
            rest
        };
        if outside.contains("{{") || outside.contains("{%") {
            self.diags.push(diag(
                Severity::Warning,
                "text looks like an unterminated tag".into(),
                Some(Location {
                    part: self.part_name.to_string(),
                    place: self.place(),
                    snippet: snippet(&outside),
                }),
            ));
        }
        if spans.is_empty() {
            return;
        }
        let row = self.nearest(self.d.rows);
        let table = self.nearest(self.d.tables);
        let place = self.place();
        let mut pieces = Vec::new();
        let mut last = 0;
        for (s, e) in spans {
            if s > last {
                pieces.push(Piece::Lit(xml::escape_text(&text[last..s])));
            }
            let loc = Location {
                part: self.part_name.to_string(),
                place: place.clone(),
                snippet: snippet(&text[s..e]),
            };
            let tag = Tag::parse(&text[s..e], loc);
            if matches!(tag.kind, TagKind::Settings) {
                apply_settings(&tag.text, self.settings);
            }
            let id = self.tags.len();
            self.tags.push(tag);
            self.ctxs.push(TagCtx {
                part: self.part,
                text: path.clone(),
                para: para.clone(),
                row: row.clone(),
                table: table.clone(),
            });
            pieces.push(Piece::Tag(id));
            last = e;
        }
        if last < text.len() {
            pieces.push(Piece::Lit(xml::escape_text(&text[last..])));
        }
        t.pieces = Some(pieces);
        if self.d.preserve_space {
            self.preserve.push(path[..path.len() - 1].to_vec());
        }
    }
}

fn snippet(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() > 60 {
        format!("{}…", s.chars().take(59).collect::<String>())
    } else {
        s.to_string()
    }
}

pub(crate) fn apply_settings(text: &str, settings: &mut Settings) {
    for cap in SETTING_RE.captures_iter(text) {
        let value = cap
            .get(2)
            .or_else(|| cap.get(3))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        apply_setting(settings, &cap[1], value);
    }
}

/// Applies one `key=value` setting, from the tag or from a document
/// property. Unknown keys are ignored, as the tag has always done.
pub fn apply_setting(settings: &mut Settings, key: &str, value: String) {
    match key {
        "lang" => settings.lang = Some(value),
        "timezone" | "tz" => settings.timezone = Some(value),
        "currency" => settings.currency = Some(value),
        k => {
            if let Some(t) = k.strip_prefix("t.") {
                settings.translations.insert(t.to_string(), value);
            }
        }
    }
}

/// Runs both phases over one part.
#[allow(clippy::too_many_arguments)]
pub fn extract_part(
    part: usize,
    tree: &mut PartTree,
    d: &Dialect,
    prefix: &str,
    tags: &mut Vec<Tag>,
    ctxs: &mut Vec<TagCtx>,
    settings: &mut Settings,
    diags: &mut Vec<Diagnostic>,
) {
    let mut paras = Vec::new();
    find_paragraphs(&tree.nodes, d, &Vec::new(), &mut paras);
    for para in &paras {
        let Some(elem) = elem_at(&tree.nodes, para) else {
            continue;
        };
        if !elem.text_content().contains('{') {
            continue;
        }
        let mut frags = Vec::new();
        collect_fragments(&elem.children, para, &mut Vec::new(), d, &mut frags);
        join_runs(&mut frags);
        write_back(&mut tree.nodes, &frags, d);
    }
    let part_name = tree.name.clone();
    let mut w = Walker {
        d,
        part,
        part_name: &part_name,
        prefix,
        tags,
        ctxs,
        settings,
        diags,
        anc: Vec::new(),
        paragraphs: 0,
        tables: 0,
        slides: 0,
        table_rows: Vec::new(),
        cell: None,
        preserve: Vec::new(),
    };
    w.walk(&mut tree.nodes, &Vec::new());
    let preserve = std::mem::take(&mut w.preserve);
    for p in preserve {
        if let Some(e) = elem_at_mut(&mut tree.nodes, &p) {
            e.set_attr("xml:space", "preserve");
        }
    }
}

// ---------------------------------------------------------------------------
// Blocks and their placement.

#[derive(Debug, Clone)]
pub struct Block {
    pub open: TagId,
    pub middles: Vec<TagId>,
    pub close: TagId,
}

/// Pairs openers with closers in document order, reporting what does not
/// match and carrying on.
pub fn match_blocks(tags: &[Tag], diags: &mut Vec<Diagnostic>) -> Vec<Block> {
    let mut stack: Vec<Block> = Vec::new();
    let mut blocks = Vec::new();
    let unclosed = |b: &Block, diags: &mut Vec<Diagnostic>| {
        let t = &tags[b.open];
        diags.push(diag(
            Severity::Error,
            format!(
                "'{}' is never closed; add '{{% end{} %}}'",
                t.text,
                t.opener().unwrap_or("")
            ),
            Some(t.loc.clone()),
        ));
    };
    for (id, tag) in tags.iter().enumerate() {
        if tag.opener().is_some() {
            stack.push(Block {
                open: id,
                middles: Vec::new(),
                close: id,
            });
        } else if tag.is_middle() {
            match stack.last_mut() {
                Some(b) if matches!(tags[b.open].opener(), Some("if" | "for")) => {
                    b.middles.push(id)
                }
                _ => diags.push(diag(
                    Severity::Error,
                    format!("'{}' has no open 'if' block", tag.text),
                    Some(tag.loc.clone()),
                )),
            }
        } else if let Some(name) = tag.closer() {
            match stack
                .iter()
                .rposition(|b| tags[b.open].opener() == Some(name))
            {
                None => diags.push(diag(
                    Severity::Error,
                    format!("'{}' has no matching '{{% {name} %}}'", tag.text),
                    Some(tag.loc.clone()),
                )),
                Some(pos) => {
                    for b in stack.drain(pos + 1..) {
                        unclosed(&b, diags);
                    }
                    let mut b = stack.pop().expect("block at pos");
                    b.close = id;
                    blocks.push(b);
                }
            }
        }
    }
    for b in &stack {
        unclosed(b, diags);
    }
    blocks.sort_by_key(|b| b.open);
    blocks
}

#[derive(Debug, Clone)]
enum Target {
    Node {
        part: usize,
        path: Path,
        before: bool,
    },
    Part {
        part: usize,
        before: bool,
    },
    /// Like `Node`, with a comment beside the tag that marks where each copy
    /// of a shifted loop starts or ends (see `rich::shift`).
    Shape {
        part: usize,
        path: Path,
        before: bool,
        marker: String,
    },
}

/// A PPTX loop with `| shift(…)` repeats the shapes on the slide that hold
/// its tags, a group counting as one shape, and marks each copy so that it
/// can be moved after rendering.
fn plan_shift(
    parts: &[PartTree],
    tags: &[Tag],
    ctxs: &[TagCtx],
    b: &Block,
    step: crate::rich::shift::Step,
) -> Result<Vec<(TagId, Target)>, String> {
    let (o, c) = (&ctxs[b.open], &ctxs[b.close]);
    let (open, close) = (&tags[b.open].text, &tags[b.close].text);
    if o.part != c.part {
        return Err(format!(
            "'{open}' and '{close}' sit on different slides; shift moves shapes on one slide"
        ));
    }
    if let Some(&m) = b.middles.first() {
        return Err(format!(
            "'{}' cannot sit in a loop that moves shapes with shift",
            tags[m].text
        ));
    }
    let nodes = &parts[o.part].nodes;
    let shape = |para: &Path| -> Option<Path> {
        (1..para.len())
            .find(|&d| elem_at(nodes, &para[..d]).is_some_and(|e| e.name == "p:spTree"))
            .map(|d| para[..=d].to_vec())
    };
    let (Some(unit_o), Some(unit_c)) = (shape(&o.para), shape(&c.para)) else {
        return Err(format!(
            "'{open}' moves shapes with shift, so it and '{close}' must sit in shapes on the slide"
        ));
    };
    let (begin, end) = crate::rich::shift::markers(b.open, step);
    Ok(vec![
        (
            b.open,
            Target::Shape {
                part: o.part,
                path: unit_o,
                before: true,
                marker: begin,
            },
        ),
        (
            b.close,
            Target::Shape {
                part: o.part,
                path: unit_c,
                before: false,
                marker: end,
            },
        ),
    ])
}

fn siblings(a: &[usize], b: &[usize]) -> bool {
    a.len() == b.len() && a[..a.len() - 1] == b[..b.len() - 1]
}

fn common_prefix(a: &[usize], b: &[usize]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UnitMode {
    Rows,
    Lca(usize),
}

fn plan_block(
    format: Format,
    parts: &mut [PartTree],
    tags: &[Tag],
    ctxs: &[TagCtx],
    b: &Block,
) -> Result<Vec<(TagId, Target)>, Diagnostic> {
    let o = &ctxs[b.open];
    let c = &ctxs[b.close];
    let describe = |id: TagId| {
        let t = &tags[id];
        format!("'{}' ({} {})", t.text, t.loc.part, t.loc.place)
    };
    let err = |msg: String| diag(Severity::Error, msg, Some(tags[b.open].loc.clone()));

    if format == Format::Pptx && tags[b.open].is_for() {
        let step = tags[b.open]
            .stmt()
            .and_then(|(_, rest)| crate::rich::shift::of_for(rest));
        if let Some(step) = step {
            return plan_shift(parts, tags, ctxs, b, step.map_err(err)?).map_err(err);
        }
    }

    if o.part != c.part {
        if format != Format::Pptx {
            return Err(err(format!(
                "{} and {} sit in different parts; a block cannot span parts",
                describe(b.open),
                describe(b.close)
            )));
        }
        let mut h = vec![(
            b.open,
            Target::Part {
                part: o.part,
                before: true,
            },
        )];
        for &m in &b.middles {
            let mp = ctxs[m].part;
            if mp <= o.part || mp > c.part {
                return Err(err(format!(
                    "{} must sit on a slide after {} and no later than {}",
                    describe(m),
                    describe(b.open),
                    describe(b.close)
                )));
            }
            h.push((
                m,
                Target::Part {
                    part: mp,
                    before: true,
                },
            ));
        }
        h.push((
            b.close,
            Target::Part {
                part: c.part,
                before: false,
            },
        ));
        return Ok(h);
    }

    // On one slide, a block whose tags sit in different shapes repeats the
    // slide, as PowerPoint authors mean when they open a loop in the title
    // and close it in the body. Repeating the shapes would print each copy
    // over the last, since a copied shape keeps its position.
    if format == Format::Pptx && o.para != c.para {
        let lcp = common_prefix(&o.para, &c.para);
        let across_shapes = elem_at(&parts[o.part].nodes, &o.para[..lcp]).is_some_and(|e| {
            matches!(e.name.as_str(), "p:spTree" | "p:grpSp" | "p:cSld" | "p:sld")
        });
        if across_shapes {
            if let Some(&m) = b.middles.first() {
                return Err(err(format!(
                    "{} cannot divide a slide that {} and {} repeat; put it on a slide of its own",
                    describe(m),
                    describe(b.open),
                    describe(b.close)
                )));
            }
            return Ok(vec![
                (
                    b.open,
                    Target::Part {
                        part: o.part,
                        before: true,
                    },
                ),
                (
                    b.close,
                    Target::Part {
                        part: c.part,
                        before: false,
                    },
                ),
            ]);
        }
    }

    if o.para == c.para {
        for &m in &b.middles {
            if ctxs[m].para != o.para {
                return Err(err(format!(
                    "{} must sit in the same paragraph as {} and {}",
                    describe(m),
                    describe(b.open),
                    describe(b.close)
                )));
            }
        }
        return Ok(Vec::new());
    }

    let part = o.part;
    let (unit_o, unit_c, mode) = match (&o.row, &c.row) {
        (Some(ro), Some(rc)) if o.table == c.table && (ro == rc || siblings(ro, rc)) => {
            (ro.clone(), rc.clone(), UnitMode::Rows)
        }
        _ => {
            let lcp = common_prefix(&o.para, &c.para);
            if lcp == o.para.len() {
                (o.para.clone(), o.para.clone(), UnitMode::Lca(lcp - 1))
            } else {
                (
                    o.para[..=lcp].to_vec(),
                    c.para[..=lcp].to_vec(),
                    UnitMode::Lca(lcp),
                )
            }
        }
    };
    let word = if mode == UnitMode::Rows {
        "row"
    } else {
        "paragraph"
    };
    let mut h = vec![(
        b.open,
        Target::Node {
            part,
            path: unit_o.clone(),
            before: true,
        },
    )];
    for &m in &b.middles {
        let mc = &ctxs[m];
        let unit_m = match mode {
            UnitMode::Rows => mc
                .row
                .clone()
                .filter(|r| mc.table == o.table && siblings(r, &unit_o)),
            UnitMode::Lca(depth) => (mc.para.len() > depth && mc.para[..depth] == unit_o[..depth])
                .then(|| mc.para[..=depth].to_vec()),
        };
        let Some(unit_m) = unit_m else {
            return Err(err(format!(
                "{} must sit in its own {word} between {} and {}",
                describe(m),
                describe(b.open),
                describe(b.close)
            )));
        };
        if unit_m == unit_o || unit_m == unit_c || unit_m < unit_o || unit_m > unit_c {
            return Err(err(format!(
                "{} needs its own {word} between {} and {}",
                describe(m),
                describe(b.open),
                describe(b.close)
            )));
        }
        h.push((
            m,
            Target::Node {
                part,
                path: unit_m,
                before: true,
            },
        ));
    }
    h.push((
        b.close,
        Target::Node {
            part,
            path: unit_c.clone(),
            before: false,
        },
    ));
    if format == Format::Xlsx {
        if mode != UnitMode::Rows {
            return Err(err(format!(
                "{} and {} must sit in cells of the same sheet",
                describe(b.open),
                describe(b.close)
            )));
        }
        let row_of = |p: &Path| {
            elem_at(&parts[part].nodes, p)
                .and_then(|e| e.attr("r"))
                .and_then(|r| r.parse::<u32>().ok())
        };
        if let (Some(start), Some(end)) = (row_of(&unit_o), row_of(&unit_c)) {
            parts[part].row_blocks.push(RowBlock {
                start,
                end,
                is_loop: tags[b.open].is_for(),
                sheet: 0,
            });
        }
    }
    // ODS rows carry no numbers, so a block is the span of positions its rows
    // take in their sheet. Rows of a table nested in a cell are not sheet
    // rows and get no block: their copies need no renumbering.
    if format == Format::Ods && mode == UnitMode::Rows {
        let nodes = &parts[part].nodes;
        if let Some((sheet, table)) = crate::sheet::ods_sheet_of(nodes, &unit_o) {
            let first = crate::sheet::ods_row_span(nodes, &table, &unit_o);
            let last = crate::sheet::ods_row_span(nodes, &table, &unit_c);
            if let (Some((start, _)), Some((last_start, count))) = (first, last) {
                parts[part].row_blocks.push(RowBlock {
                    start,
                    end: last_start + count - 1,
                    is_loop: tags[b.open].is_for(),
                    sheet,
                });
            }
        }
    }
    Ok(h)
}

/// Hoists every block's tags onto the structure they repeat.
pub fn place_blocks(
    format: Format,
    parts: &mut [PartTree],
    tags: &[Tag],
    ctxs: &[TagCtx],
    blocks: &[Block],
    diags: &mut Vec<Diagnostic>,
) {
    let mut hoists = Vec::new();
    for b in blocks {
        match plan_block(format, parts, tags, ctxs, b) {
            Ok(h) => hoists.extend(h),
            Err(d) => diags.push(d),
        }
    }
    hoists.sort_by_key(|(id, _)| *id);
    for (id, target) in hoists {
        let ctx = &ctxs[id];
        if let Some(Node::Text(t)) = node_at_mut(&mut parts[ctx.part].nodes, &ctx.text) {
            if let Some(p) = &mut t.pieces {
                p.retain(|p| *p != Piece::Tag(id));
            }
        }
        match target {
            Target::Node { part, path, before } => {
                if let Some(e) = elem_at_mut(&mut parts[part].nodes, &path) {
                    if before {
                        e.before.push(Piece::Tag(id));
                    } else {
                        e.after.push(Piece::Tag(id));
                    }
                }
            }
            Target::Part { part, before } => {
                if before {
                    parts[part].before.push(id);
                } else {
                    parts[part].after.push(id);
                }
            }
            // The marker sits inside the loop, so every copy carries one.
            Target::Shape {
                part,
                path,
                before,
                marker,
            } => {
                if let Some(e) = elem_at_mut(&mut parts[part].nodes, &path) {
                    if before {
                        e.before.push(Piece::Tag(id));
                        e.before.push(Piece::Lit(marker));
                    } else {
                        e.after.push(Piece::Lit(marker));
                        e.after.push(Piece::Tag(id));
                    }
                }
            }
        }
    }
    // Blocks nest, so a sheet keeps all of them: renumbering walks them as a
    // tree. Two blocks over the same rows, such as two loops opened in one
    // row, repeat those rows as one block would.
    for part in parts.iter_mut() {
        let blocks = &mut part.row_blocks;
        blocks.sort_by_key(|b| (b.sheet, b.start, std::cmp::Reverse(b.end), !b.is_loop));
        blocks.dedup_by(|a, b| a.sheet == b.sheet && a.start == b.start && a.end == b.end);
    }
}

/// Turns sheet cells whose whole content is one expression into typed
/// cells, and cells that only held block tags into empty cells.
pub fn type_cells(tree: &mut PartTree, tags: &[Tag]) {
    let Some(root) = xml::root_mut(&mut tree.nodes) else {
        return;
    };
    let Some(data) = root.find_mut("sheetData") else {
        return;
    };
    for row in data.children.iter_mut() {
        let Node::Elem(row) = row else { continue };
        for cell in row.children.iter_mut() {
            let Node::Elem(c) = cell else { continue };
            if c.name != "c" || !c.before.is_empty() || !c.after.is_empty() {
                continue;
            }
            let Some(is) = c.child("is") else { continue };
            let Some(t) = is.child("t") else { continue };
            let Some(pieces) = t.pieces() else { continue };
            match pieces.as_slice() {
                [] => {
                    c.remove_attr("t");
                    c.children.clear();
                    c.self_closing = true;
                    c.open = format!("{}/>", c.open.trim_end_matches('>'));
                }
                [Piece::Tag(id)] if matches!(tags[*id].kind, TagKind::Expr(_)) => {
                    let mut e = c.clone();
                    e.remove_attr("t");
                    let open = e.open.trim_end_matches('>').to_string();
                    *cell = Node::Text(Text {
                        raw: String::new(),
                        pieces: Some(vec![Piece::TypedCell { expr: *id, open }]),
                    });
                }
                _ => {}
            }
        }
    }
}

impl Elem {
    /// The pieces of this element's single text child, when it has been cut.
    fn pieces(&self) -> Option<&Vec<Piece>> {
        match self.children.as_slice() {
            [Node::Text(t)] => t.pieces.as_ref(),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Serialisation.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Render,
    /// Every expression is routed through a probe filter that records
    /// undefined values instead of failing.
    Probe,
}

/// The generated template and the line each tag starts on.
#[derive(Debug, Clone)]
pub struct Source {
    pub text: String,
    pub lines: Vec<(usize, TagId)>,
}

impl Source {
    /// The tag a template line belongs to.
    pub fn tag_at_line(&self, line: usize) -> Option<TagId> {
        let i = self.lines.partition_point(|(l, _)| *l <= line);
        i.checked_sub(1).map(|i| self.lines[i].1)
    }
}

struct Writer<'a> {
    out: String,
    line: usize,
    lines: Vec<(usize, TagId)>,
    mode: Mode,
    tags: &'a [Tag],
}

impl Writer<'_> {
    fn lit(&mut self, s: &str) {
        let mut rest = s;
        while let Some(pos) = rest.find('{') {
            self.out.push_str(&rest[..pos]);
            let next = rest[pos + 1..].chars().next();
            if matches!(next, Some('{' | '%' | '#') | None) {
                self.out.push_str("{{ '{' }}");
            } else {
                self.out.push('{');
            }
            rest = &rest[pos + 1..];
        }
        self.out.push_str(rest);
        self.line += s.matches('\n').count();
    }

    fn mark(&mut self, id: TagId) {
        self.out.push_str("{#\n#}");
        self.line += 1;
        self.lines.push((self.line, id));
    }

    fn tag(&mut self, id: TagId) {
        let tag = &self.tags[id];
        let src = match (&tag.kind, self.mode) {
            (TagKind::Comment | TagKind::Settings, _) => return,
            (TagKind::Expr(inner), Mode::Render) => format!("{{{{ {inner} }}}}"),
            (TagKind::Expr(inner), Mode::Probe) => {
                format!("{{{{ ({inner}) | __probe({id}) }}}}")
            }
            (TagKind::Stmt { name, rest }, Mode::Render) => format!("{{% {name} {rest} %}}"),
            (TagKind::Stmt { name, rest }, Mode::Probe) => probe_stmt(name, rest, id),
            (TagKind::SheetLoop(_), mode) => {
                // A malformed head was reported at compile time; this only has
                // to keep the template parseable.
                let head = tag.sheet_loop().unwrap_or("__ in []");
                match mode {
                    Mode::Render => format!("{{% for {head} %}}"),
                    Mode::Probe => probe_stmt("for", head, id),
                }
            }
            (TagKind::SheetEnd, _) => "{% endfor %}".to_string(),
        };
        self.mark(id);
        self.out.push_str(&src);
        self.line += src.matches('\n').count();
    }

    fn typed_cell(&mut self, id: TagId, open: &str) {
        let TagKind::Expr(inner) = &self.tags[id].kind else {
            return;
        };
        self.mark(id);
        let probe = match self.mode {
            Mode::Render => String::new(),
            Mode::Probe => format!(" | __probe({id})"),
        };
        self.out
            .push_str(&format!("{{% set __v = ({inner}){probe} %}}"));
        self.line += inner.matches('\n').count();
        self.lit(open);
        self.out.push_str(concat!(
            "{% if __v is boolean %} t=\"b\"><v>{{ 1 if __v else 0 }}</v></c>",
            "{% elif __v is number %}><v>{{ __v }}</v></c>",
            "{% elif __v is __blank %}/>",
            "{% else %} t=\"inlineStr\"><is><t xml:space=\"preserve\">{{ __v }}</t></is></c>",
            "{% endif %}"
        ));
    }

    fn piece(&mut self, p: &Piece) {
        match p {
            Piece::Lit(s) => self.lit(s),
            Piece::Tag(id) => self.tag(*id),
            Piece::TypedCell { expr, open } => self.typed_cell(*expr, open),
        }
    }

    fn nodes(&mut self, nodes: &[Node]) {
        for n in nodes {
            match n {
                Node::Elem(e) => {
                    for p in &e.before {
                        self.piece(p);
                    }
                    self.lit(&e.open);
                    if !e.self_closing {
                        self.nodes(&e.children);
                        self.lit(&format!("</{}>", e.name));
                    }
                    for p in &e.after {
                        self.piece(p);
                    }
                }
                Node::Text(t) => match &t.pieces {
                    None => self.lit(&t.raw),
                    Some(ps) => {
                        for p in ps {
                            self.piece(p);
                        }
                    }
                },
                Node::Raw(r) => self.lit(r),
            }
        }
    }
}

fn probe_stmt(name: &str, rest: &str, id: TagId) -> String {
    match name {
        "for" => match FOR_RE.captures(rest) {
            Some(c) => format!(
                "{{% for {} in ({}) | __probe_iter({id}){}{} %}}",
                &c[1],
                &c[2],
                c.get(3).map(|m| m.as_str()).unwrap_or(""),
                c.get(4).map(|m| m.as_str()).unwrap_or("")
            ),
            None => format!("{{% for {rest} %}}"),
        },
        "if" | "elif" => format!("{{% {name} ({rest}) | __probe({id}) %}}"),
        "set" => match SET_RE.captures(rest) {
            Some(c) => format!("{{% set {} = ({}) | __probe({id}) %}}", &c[1], &c[2]),
            None => format!("{{% set {rest} %}}"),
        },
        _ => format!("{{% {name} {rest} %}}"),
    }
}

/// Writes every part into one template, each part behind a marker.
pub fn emit(parts: &[PartTree], tags: &[Tag], mode: Mode) -> Source {
    let mut w = Writer {
        out: String::new(),
        line: 1,
        lines: Vec::new(),
        mode,
        tags,
    };
    for part in parts {
        for &id in &part.before {
            w.tag(id);
        }
        w.out.push(MARK);
        w.out.push_str(&part.name);
        w.out.push(MARK);
        w.nodes(&part.nodes);
        for &id in &part.after {
            w.tag(id);
        }
    }
    Source {
        text: w.out,
        lines: w.lines,
    }
}

/// Splits a rendered template into `(part name, content)` in output order.
pub fn split_output(rendered: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut it = rendered.split(MARK);
    it.next();
    while let (Some(name), Some(content)) = (it.next(), it.next()) {
        out.push((name.to_string(), content.to_string()));
    }
    out
}

/// A tag's own (statement-level) source, for checking it alone.
pub fn standalone_source(tag: &Tag) -> Option<String> {
    match &tag.kind {
        TagKind::Expr(inner) => Some(format!("{{{{ {inner} }}}}")),
        TagKind::Stmt { name, rest } => Some(match name.as_str() {
            "for" => format!("{{% for {rest} %}}{{% endfor %}}"),
            "if" | "elif" => format!("{{% if {rest} %}}{{% endif %}}"),
            "else" | "endif" | "endfor" | "endset" | "endwith" | "endmacro" | "endblock"
            | "endfilter" | "endcall" | "endraw" | "endautoescape" => return None,
            _ => format!("{{% {name} {rest} %}}"),
        }),
        TagKind::SheetLoop(_) => Some(format!("{{% for {} %}}{{% endfor %}}", tag.sheet_loop()?)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_tags() {
        let t = Tag::parse("{{ “x” | default(‘y’) }}", Location::default());
        assert!(matches!(&t.kind, TagKind::Expr(e) if e == "\"x\" | default('y')"));
        let t = Tag::parse("{%- for i in items -%}", Location::default());
        assert_eq!(t.opener(), Some("for"));
        assert!(Tag::parse("{% endfor %}", Location::default()).closer() == Some("for"));
        assert!(Tag::parse("{% set x %}", Location::default())
            .opener()
            .is_some());
        assert!(Tag::parse("{% set x = 1 %}", Location::default())
            .opener()
            .is_none());
    }

    #[test]
    fn escapes_literal_braces() {
        let mut w = Writer {
            out: String::new(),
            line: 1,
            lines: vec![],
            mode: Mode::Render,
            tags: &[],
        };
        w.lit("a{b {{c {%d {#e f{");
        assert_eq!(w.out, "a{b {{ '{' }}{c {{ '{' }}%d {{ '{' }}#e f{{ '{' }}");
    }

    #[test]
    fn maps_lines_to_tags() {
        let s = Source {
            text: String::new(),
            lines: vec![(2, 0), (5, 1), (9, 2)],
        };
        assert_eq!(s.tag_at_line(1), None);
        assert_eq!(s.tag_at_line(2), Some(0));
        assert_eq!(s.tag_at_line(7), Some(1));
        assert_eq!(s.tag_at_line(40), Some(2));
    }

    #[test]
    fn probes_for_statements() {
        assert_eq!(
            probe_stmt("for", "i in items if i.ok", 3),
            "{% for i in (items) | __probe_iter(3) if i.ok %}"
        );
        assert_eq!(
            probe_stmt("set", "x = a.b", 4),
            "{% set x = (a.b) | __probe(4) %}"
        );
    }
}
