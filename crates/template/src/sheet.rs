//! Spreadsheet specifics: shared strings, and renumbering the rows a loop
//! copied so that cell references and formulas keep pointing at the right
//! cells, in XLSX and in ODS.

use std::collections::HashMap;
use std::ops::Range;

use crate::program::{ColBlock, RowBlock};
use crate::xml::{self, elem_at, elem_at_mut, Elem, Node, Path, Text};

/// The text of each `si` in `sharedStrings.xml`, rich runs joined.
pub fn shared_strings(src: &str) -> Vec<String> {
    let Ok(nodes) = xml::parse(src) else {
        return Vec::new();
    };
    match xml::root(&nodes) {
        Some(sst) => sst
            .children_named("si")
            .map(|si| si.text_content())
            .collect(),
        None => Vec::new(),
    }
}

/// Rewrites every shared-string cell whose string holds a tag as an inline
/// string, so the sheet part owns the text the engine edits.
pub fn inline_shared_strings(nodes: &mut [Node], shared: &[String]) {
    let Some(root) = xml::root_mut(nodes) else {
        return;
    };
    let Some(data) = root.find_mut("sheetData") else {
        return;
    };
    for row in data.children.iter_mut() {
        let Node::Elem(row) = row else { continue };
        for cell in row.children.iter_mut() {
            let Node::Elem(c) = cell else { continue };
            if c.name != "c" || c.attr("t").as_deref() != Some("s") {
                continue;
            }
            let Some(idx) = c
                .child("v")
                .map(|v| v.text_content())
                .and_then(|v| v.trim().parse::<usize>().ok())
            else {
                continue;
            };
            let Some(text) = shared.get(idx) else {
                continue;
            };
            if crate::program::find_tags(text).is_empty() {
                continue;
            }
            let mut t = Elem::new("t", &[("xml:space", "preserve")], false);
            t.children.push(Node::Text(Text {
                raw: xml::escape_text(text),
                pieces: None,
            }));
            let mut is = Elem::new("is", &[], false);
            is.children.push(Node::Elem(t));
            c.make_open();
            c.children = vec![Node::Elem(is)];
            c.set_attr("t", "inlineStr");
        }
    }
}

pub fn col_to_num(col: &str) -> u32 {
    col.bytes().fold(0, |n, b| n * 26 + (b - b'A' + 1) as u32)
}

pub fn num_to_col(mut n: u32) -> String {
    let mut s = Vec::new();
    while n > 0 {
        n -= 1;
        s.push(b'A' + (n % 26) as u8);
        n /= 26;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

/// Splits `B12` into `("B", 12)`.
pub fn split_ref(r: &str) -> Option<(String, u32)> {
    let p = r.find(|c: char| c.is_ascii_digit())?;
    Some((r[..p].to_string(), r[p..].parse().ok()?))
}

/// Where a reference stands in a formula.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefPos {
    Single,
    RangeStart,
    RangeEnd,
}

/// Rewrites the A1 references in a formula. The callback gets the column
/// number, the row, whether each is absolute and the reference's position,
/// and returns the new pair. Sheet-qualified references and string
/// literals are left alone.
///
/// A range the callback turns around, which happens when every row it
/// covered was removed, becomes the one row its end landed on: the row
/// before the removed ones. Left reversed it would reach down to the row
/// after them, often the formula's own, and Excel would report a circular
/// reference.
pub fn rewrite_refs(
    formula: &str,
    mut f: impl FnMut(u32, bool, u32, bool, RefPos) -> (u32, u32),
) -> String {
    let b = formula.as_bytes();
    let mut out = String::with_capacity(formula.len());
    let mut i = 0;
    let mut in_string = false;
    // Where the last range start's row sits in `out`, and the row.
    let mut start: Option<(std::ops::Range<usize>, u32)> = None;
    while i < b.len() {
        let c = b[i];
        if c == b'"' {
            in_string = !in_string;
        }
        let prev_ok =
            i == 0 || !(b[i - 1].is_ascii_alphanumeric() || matches!(b[i - 1], b'_' | b'.' | b'!'));
        if !in_string && prev_ok && (c == b'$' || c.is_ascii_uppercase()) {
            let mut j = i;
            let abs_col = b[j] == b'$';
            if abs_col {
                j += 1;
            }
            let cs = j;
            while j < b.len() && b[j].is_ascii_uppercase() && j - cs < 3 {
                j += 1;
            }
            let col = &formula[cs..j];
            let abs_row = j < b.len() && b[j] == b'$';
            if abs_row {
                j += 1;
            }
            let rs = j;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let next_ok =
                j >= b.len() || !(b[j].is_ascii_alphanumeric() || matches!(b[j], b'(' | b'_'));
            if !col.is_empty() && j > rs && next_ok {
                if let Ok(row) = formula[rs..j].parse::<u32>() {
                    let pos = if j < b.len() && b[j] == b':' {
                        RefPos::RangeStart
                    } else if i > 0 && b[i - 1] == b':' {
                        RefPos::RangeEnd
                    } else {
                        RefPos::Single
                    };
                    let (nc, nr) = f(col_to_num(col), abs_col, row, abs_row, pos);
                    if abs_col {
                        out.push('$');
                    }
                    out.push_str(&num_to_col(nc));
                    if abs_row {
                        out.push('$');
                    }
                    let at = out.len();
                    out.push_str(&nr.to_string());
                    match pos {
                        RefPos::RangeStart => start = Some((at..out.len(), nr)),
                        RefPos::RangeEnd => {
                            if let Some((span, first)) = start.take() {
                                if nr < first {
                                    out.replace_range(span, &nr.to_string());
                                }
                            }
                        }
                        RefPos::Single => {}
                    }
                    i = j;
                    continue;
                }
            }
        }
        // Copy whole characters: a sheet name or a string may hold any.
        let ch = formula[i..].chars().next().unwrap_or('\u{fffd}');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// One pass through rows: the whole sheet, or one iteration of a block.
#[derive(Debug)]
struct Iteration {
    parent: Option<usize>,
    /// Added to a template row number at the start of this pass.
    offset: i64,
    /// The blocks directly inside, in row order, each with its iterations.
    children: Vec<Instance>,
}

/// One block as it came out inside one pass of its parent.
#[derive(Debug)]
struct Instance {
    block: usize,
    iterations: Vec<usize>,
    /// Rows the block added, or removed when negative, against the template.
    expansion: i64,
}

/// Where the rendered rows of a sheet go and how references move with them.
///
/// Blocks form a tree: a row block inside another repeats once per
/// iteration of the outer one. The rendered rows still carry their template
/// row numbers, so each block's run of rows is cut into iterations where a
/// row number goes back up, and the tree of passes that results says where
/// every row lands and where every template row went.
pub struct Layout {
    blocks: Vec<RowBlock>,
    /// The blocks directly inside each block; the last entry is the sheet's.
    kids: Vec<Vec<usize>>,
    iterations: Vec<Iteration>,
    /// Per rendered row: its new number and the innermost pass it is in.
    rows: Vec<(i64, usize)>,
}

impl Layout {
    /// `blocks` belong to one sheet, sorted by start and then by reverse end,
    /// as `place_blocks` leaves them. `rows` holds the template row number of
    /// every rendered row, in order.
    pub fn new(blocks: &[RowBlock], rows: &[u32]) -> Self {
        let mut kids = vec![Vec::new(); blocks.len() + 1];
        let mut open: Vec<usize> = Vec::new();
        for (i, b) in blocks.iter().enumerate() {
            while open
                .last()
                .is_some_and(|&o| !(blocks[o].start <= b.start && b.end <= blocks[o].end))
            {
                open.pop();
            }
            kids[open.last().copied().unwrap_or(blocks.len())].push(i);
            open.push(i);
        }
        let mut layout = Layout {
            blocks: blocks.to_vec(),
            kids,
            iterations: Vec::new(),
            rows: vec![(0, 0); rows.len()],
        };
        layout.pass(rows, None, 0..rows.len(), 0, None);
        layout
    }

    fn len(&self, block: usize) -> i64 {
        (self.blocks[block].end - self.blocks[block].start + 1) as i64
    }

    fn holds(&self, block: usize, row: u32) -> bool {
        self.blocks[block].start <= row && row <= self.blocks[block].end
    }

    /// Lays out the rows of one pass and returns its id and the offset in
    /// force after it.
    fn pass(
        &mut self,
        t: &[u32],
        block: Option<usize>,
        range: Range<usize>,
        mut offset: i64,
        parent: Option<usize>,
    ) -> (usize, i64) {
        let id = self.iterations.len();
        self.iterations.push(Iteration {
            parent,
            offset,
            children: Vec::new(),
        });
        let kids = self.kids[block.unwrap_or(self.blocks.len())].clone();
        // A block none of whose rows came out was repeated zero times.
        let skip = |layout: &mut Self, block: usize, offset: &mut i64| {
            let len = layout.len(block);
            layout.iterations[id].children.push(Instance {
                block,
                iterations: Vec::new(),
                expansion: -len,
            });
            *offset -= len;
        };
        let mut k = 0;
        let mut i = range.start;
        while i < range.end {
            let r = t[i];
            while k < kids.len() && self.blocks[kids[k]].end < r {
                skip(self, kids[k], &mut offset);
                k += 1;
            }
            if k < kids.len() && self.blocks[kids[k]].start <= r {
                let c = kids[k];
                let mut j = i;
                while j < range.end && self.holds(c, t[j]) {
                    j += 1;
                }
                let len = self.len(c);
                let mut at = offset;
                let mut iterations = Vec::new();
                for run in self.split(t, c, i..j) {
                    let (it, end) = self.pass(t, Some(c), run, at, Some(id));
                    iterations.push(it);
                    at = end + len;
                }
                let after = at - len;
                self.iterations[id].children.push(Instance {
                    block: c,
                    iterations,
                    expansion: after - offset,
                });
                offset = after;
                k += 1;
                i = j;
            } else {
                self.rows[i] = (r as i64 + offset, id);
                i += 1;
            }
        }
        while k < kids.len() {
            skip(self, kids[k], &mut offset);
            k += 1;
        }
        (id, offset)
    }

    /// Cuts a block's run of rows into iterations: a new one starts where a
    /// row number does not go forward. Rows of a block nested inside count
    /// as that block's first row, so its repeats do not start iterations.
    fn split(&self, t: &[u32], block: usize, range: Range<usize>) -> Vec<Range<usize>> {
        let key = |r: u32| {
            self.kids[block]
                .iter()
                .find(|&&g| self.holds(g, r))
                .map_or((r, false), |&g| (self.blocks[g].start, true))
        };
        let mut out = Vec::new();
        let mut start = range.start;
        let mut last: Option<(u32, bool)> = None;
        for i in range.clone() {
            let k = key(t[i]);
            if let Some(l) = last {
                if k.0 < l.0 || (k.0 == l.0 && !k.1) {
                    out.push(start..i);
                    start = i;
                }
            }
            last = Some(k);
        }
        if !range.is_empty() {
            out.push(start..range.end);
        }
        out
    }

    /// Whether no row moved, so nothing needs rewriting.
    pub fn is_identity(&self) -> bool {
        self.blocks.is_empty()
    }

    /// The new number of rendered row `i`.
    pub fn row(&self, i: usize) -> u32 {
        self.rows[i].0.max(1) as u32
    }

    /// The offset at template row `row` within a pass: what the blocks
    /// before it added. A range that starts on a block's last row stays
    /// before the block's growth; anything else that reaches the last row
    /// moves past it.
    fn offset_at(&self, it: usize, row: u32, pos: RefPos) -> i64 {
        let pass = &self.iterations[it];
        pass.offset
            + pass
                .children
                .iter()
                .filter(|c| {
                    let end = self.blocks[c.block].end;
                    end < row || (pos != RefPos::RangeStart && end <= row)
                })
                .map(|c| c.expansion)
                .sum::<i64>()
    }

    /// Where a reference to template row `row` lands, seen from rendered row
    /// `from`, or from the sheet as a whole. A reference into a block the
    /// formula sits in stays in the formula's own iteration. From outside a
    /// block, the start of a range goes to the first iteration and its end to
    /// the last, so `SUM(B4:B4)` grows over every copy; a single reference
    /// goes to the last iteration when it names the block's last row, and to
    /// the first otherwise.
    pub fn shift(&self, from: Option<usize>, row: u32, pos: RefPos) -> u32 {
        let mut chain = Vec::new();
        let mut at = from.map(|i| self.rows[i].1);
        while let Some(it) = at {
            chain.push(it);
            at = self.iterations[it].parent;
        }
        self.resolve(0, &chain, row, pos)
    }

    fn resolve(&self, mut it: usize, chain: &[usize], row: u32, pos: RefPos) -> u32 {
        loop {
            let found = self.iterations[it]
                .children
                .iter()
                .find(|c| self.holds(c.block, row) && !c.iterations.is_empty());
            let Some(c) = found else {
                return (row as i64 + self.offset_at(it, row, pos)).max(1) as u32;
            };
            it = match c.iterations.iter().find(|j| chain.contains(j)) {
                Some(&j) => j,
                None if pos == RefPos::RangeEnd
                    || (pos == RefPos::Single && row == self.blocks[c.block].end) =>
                {
                    c.iterations[c.iterations.len() - 1]
                }
                None => c.iterations[0],
            };
        }
    }

    /// Where a range of rows that moves as one, such as a merged cell, lands:
    /// once per iteration of the innermost block holding all of it, and not
    /// at all when that block came out empty.
    pub fn copies(&self, first: u32, last: u32) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        self.copies_in(0, first, last, &mut out);
        out
    }

    fn copies_in(&self, it: usize, first: u32, last: u32, out: &mut Vec<(u32, u32)>) {
        let inside = self.iterations[it]
            .children
            .iter()
            .find(|c| self.holds(c.block, first) && self.holds(c.block, last));
        match inside {
            Some(c) => {
                for &j in &c.iterations {
                    self.copies_in(j, first, last, out);
                }
            }
            None => out.push((
                self.resolve(it, &[], first, RefPos::RangeStart),
                self.resolve(it, &[], last, RefPos::RangeEnd),
            )),
        }
    }
}

/// The last column a sheet can hold, `XFD`.
const MAX_COL: i64 = 16_384;

/// One column loop in a rendered row: the template columns it repeats, how
/// many copies the row holds, and how far the copies of the loops to its
/// left have already pushed it.
struct Spun {
    start: u32,
    end: u32,
    copies: i64,
    before: i64,
}

impl Spun {
    fn len(&self) -> i64 {
        (self.end - self.start) as i64 + 1
    }

    fn holds(&self, col: u32) -> bool {
        self.start <= col && col <= self.end
    }
}

/// Where the cells of one rendered row land once its column loops have
/// repeated their cells to the right.
struct Spread {
    blocks: Vec<Spun>,
}

impl Spread {
    /// `cells` holds the template column of every cell in the rendered row,
    /// so the cell that opens a loop appears once per copy.
    fn new(row: u32, cols: &[ColBlock], cells: &[u32]) -> Option<Spread> {
        let mut mine: Vec<&ColBlock> = cols.iter().filter(|b| b.row == row).collect();
        mine.sort_by_key(|b| b.start);
        let mut blocks = Vec::new();
        let mut before = 0;
        for b in mine {
            let spun = Spun {
                start: b.start,
                end: b.end,
                copies: cells.iter().filter(|&&c| c == b.start).count() as i64,
                before,
            };
            // No copies at all pulls the cells to the right back in.
            before += (spun.copies - 1) * spun.len();
            blocks.push(spun);
        }
        (!blocks.is_empty()).then_some(Spread { blocks })
    }

    fn block_of(&self, col: u32) -> Option<usize> {
        self.blocks.iter().position(|b| b.holds(col))
    }

    /// How far the loops that end left of `col` push it.
    fn outside(&self, col: u32) -> i64 {
        self.blocks
            .iter()
            .filter(|b| b.end < col)
            .map(|b| (b.copies - 1) * b.len())
            .sum()
    }

    /// The column of a cell, given the loop and copy it sits in.
    fn cell(&self, col: u32, copy: Option<(usize, i64)>) -> i64 {
        let at = match copy {
            Some((i, k)) => col as i64 + self.blocks[i].before + k * self.blocks[i].len(),
            None => col as i64 + self.outside(col),
        };
        at.max(1)
    }

    /// The column a reference to `col` means, read from a formula in the
    /// given loop and copy. A formula inside a loop points at its own copy.
    /// From outside, the start of a range lands on the first copy and its
    /// end on the last one, so a sum over the loop's cells widens with it.
    fn refer(&self, col: u32, pos: RefPos, from: Option<(usize, i64)>) -> i64 {
        let Some(i) = self.block_of(col) else {
            return self.cell(col, None).min(MAX_COL);
        };
        let b = &self.blocks[i];
        let first = col as i64 + b.before;
        let at = match from {
            Some((j, k)) if j == i => first + k * b.len(),
            // The loop made no copies: point at the column before it.
            _ if b.copies == 0 => b.start as i64 - 1 + b.before,
            _ if matches!(pos, RefPos::RangeEnd) => first + (b.copies - 1) * b.len(),
            _ => first,
        };
        at.clamp(1, MAX_COL)
    }
}

/// Renumbers the rows of a rendered sheet, re-bases formulas and merged
/// ranges, and drops cached formula values so they are recomputed. Cells a
/// column loop repeated are moved to the right of one another. Returns the
/// layout too, for references from outside the sheet such as its print
/// area.
pub fn rebase(
    src: &str,
    blocks: &[RowBlock],
    cols: &[ColBlock],
) -> Result<(String, Layout), String> {
    let mut nodes = xml::parse(src)?;
    let root = xml::root_mut(&mut nodes).ok_or("sheet has no root element")?;
    let Some(data) = root.find_mut("sheetData") else {
        return Ok((src.to_string(), Layout::new(&[], &[])));
    };

    // Pass 1: the template row of every rendered row, and the masters of
    // shared formulas.
    let mut template_rows = Vec::new();
    let mut shared: HashMap<String, (u32, u32, String)> = HashMap::new();
    for row in data.children.iter() {
        let Node::Elem(row) = row else { continue };
        template_rows.push(row.attr("r").and_then(|r| r.parse().ok()).unwrap_or(0));
        for c in row.children_named("c") {
            let Some(f) = c.child("f") else { continue };
            if f.attr("t").as_deref() != Some("shared") {
                continue;
            }
            let cell = c.attr("r").and_then(|r| split_ref(&r));
            if let (Some(si), Some((col, r0))) = (f.attr("si"), cell) {
                let text = f.text_content();
                if !text.is_empty() {
                    shared.entry(si).or_insert((col_to_num(&col), r0, text));
                }
            }
        }
    }
    let layout = Layout::new(blocks, &template_rows);

    // Pass 2: rewrite rows, cells and formulas.
    let mut max_row = 0u32;
    let mut widest = 0i64;
    let mut ri = 0;
    for row in data.children.iter_mut() {
        let Node::Elem(row) = row else { continue };
        let here = ri;
        ri += 1;
        let r = template_rows[here];
        let new_r = layout.row(here);
        max_row = max_row.max(new_r);
        row.set_attr("r", &new_r.to_string());
        let spread = if cols.iter().any(|b| b.row == r) {
            let cells: Vec<u32> = row
                .children_named("c")
                .filter_map(|c| c.attr("r").and_then(|r| split_ref(&r)))
                .map(|(col, _)| col_to_num(&col))
                .collect();
            Spread::new(r, cols, &cells)
        } else {
            None
        };
        if spread.is_some() {
            // The row's cells no longer span the columns it claimed.
            row.remove_attr("spans");
        }
        let mut seen = vec![0i64; spread.as_ref().map_or(0, |s| s.blocks.len())];
        for cell in row.children.iter_mut() {
            let Node::Elem(c) = cell else { continue };
            if c.name != "c" {
                continue;
            }
            let Some((col, _)) = c.attr("r").and_then(|r| split_ref(&r)) else {
                continue;
            };
            let colnum = col_to_num(&col);
            // The loop and the copy this cell sits in, if any.
            let mut copy = None;
            match &spread {
                Some(s) => {
                    if let Some(i) = s.block_of(colnum) {
                        if s.blocks[i].start == colnum {
                            seen[i] += 1;
                        }
                        copy = Some((i, (seen[i] - 1).max(0)));
                    }
                    let at = s.cell(colnum, copy);
                    if at > MAX_COL {
                        return Err(format!(
                            "row {new_r}: column loops fill more than 16,384 columns"
                        ));
                    }
                    widest = widest.max(at);
                    c.set_attr("r", &format!("{}{new_r}", num_to_col(at as u32)));
                }
                None => c.set_attr("r", &format!("{col}{new_r}")),
            }
            if c.child("f").is_none() {
                continue;
            }
            // Without a cached value the cell type must not claim one.
            c.children
                .retain(|n| !matches!(n, Node::Elem(e) if e.name == "v"));
            c.remove_attr("t");
            let Some(f) = c.child_mut("f") else { continue };
            let mut text = f.text_content();
            if f.attr("t").as_deref() == Some("shared") {
                if text.is_empty() {
                    if let Some((mc, mr, mtext)) = f.attr("si").and_then(|si| shared.get(&si)) {
                        let (dc, dr) = (colnum as i64 - *mc as i64, r as i64 - *mr as i64);
                        text = rewrite_refs(mtext, |c, ac, r, ar, _| {
                            (
                                if ac { c } else { (c as i64 + dc).max(1) as u32 },
                                if ar { r } else { (r as i64 + dr).max(1) as u32 },
                            )
                        });
                    }
                }
                f.remove_attr("t");
                f.remove_attr("ref");
                f.remove_attr("si");
            }
            // A reference to this row follows the row's column loops. One to
            // another row does too, as it would when a cell is filled to the
            // right, unless its column is fixed as in `$H$1`.
            let text = rewrite_refs(&text, |c, fixed, to, _, pos| {
                let c = match &spread {
                    Some(s) if to == r || !fixed => s.refer(c, pos, copy) as u32,
                    _ => c,
                };
                (c, layout.shift(Some(here), to, pos))
            });
            f.make_open();
            f.children = vec![Node::Text(Text {
                raw: xml::escape_text(&text),
                pieces: None,
            })];
        }
    }

    if let Some(dim) = root.find_mut("dimension") {
        if let Some((a, b)) = dim.attr("ref").and_then(|r| {
            r.split_once(':')
                .map(|(a, b)| (a.to_string(), b.to_string()))
        }) {
            if let Some((col, _)) = split_ref(&b) {
                let col = if widest > col_to_num(&col) as i64 {
                    num_to_col(widest as u32)
                } else {
                    col
                };
                dim.set_attr("ref", &format!("{a}:{col}{max_row}"));
            }
        }
    }
    if let Some(merges) = root.find_mut("mergeCells") {
        let mut out = Vec::new();
        for m in merges.children_named("mergeCell") {
            let Some(r) = m.attr("ref") else { continue };
            let Some((a, b)) = r.split_once(':') else {
                continue;
            };
            let (Some((ca, ra)), Some((cb, rb))) = (split_ref(a), split_ref(b)) else {
                continue;
            };
            for (na, nb) in layout.copies(ra, rb) {
                out.push(format!("{ca}{na}:{cb}{nb}"));
            }
        }
        merges.children = out
            .iter()
            .map(|r| Node::Elem(Elem::new("mergeCell", &[("ref", r)], true)))
            .collect();
        merges.set_attr("count", &out.len().to_string());
    }
    Ok((xml::to_xml(&nodes), layout))
}

/// Re-bases the sheet references in a defined name such as a print area
/// (`'Sales'!$A$1:$E$6`), as seen from outside the sheet's rows. `layout`
/// gives the layout of the sheet a reference names, if it has one.
pub fn rebase_named<'a>(text: &str, layout: impl Fn(&str) -> Option<&'a Layout>) -> String {
    static QUALIFIED: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"('(?:[^']|'')+'|[A-Za-z0-9_.]+)!(\$?[A-Z]{1,3}\$?[0-9]+(?::\$?[A-Z]{1,3}\$?[0-9]+)?)",
        )
        .expect("valid")
    });
    QUALIFIED
        .replace_all(text, |c: &regex::Captures| {
            let name = c[1]
                .strip_prefix('\'')
                .and_then(|n| n.strip_suffix('\''))
                .map(|n| n.replace("''", "'"))
                .unwrap_or_else(|| c[1].to_string());
            let Some(layout) = layout(&name).filter(|l| !l.is_identity()) else {
                return c[0].to_string();
            };
            let moved = rewrite_refs(&c[2], |col, _, r, _, pos| (col, layout.shift(None, r, pos)));
            format!("{}!{moved}", &c[1])
        })
        .into_owned()
}

/// Asks Excel and LibreOffice to recompute every formula on load.
pub fn force_recalc(src: &str) -> String {
    let Ok(mut nodes) = xml::parse(src) else {
        return src.to_string();
    };
    let Some(root) = xml::root_mut(&mut nodes) else {
        return src.to_string();
    };
    if let Some(calc) = root.child_mut("calcPr") {
        calc.set_attr("fullCalcOnLoad", "1");
    } else {
        let after = root.children.iter().rposition(
            |n| matches!(n, Node::Elem(e) if e.name == "definedNames" || e.name == "sheets"),
        );
        let calc = Node::Elem(Elem::new("calcPr", &[("fullCalcOnLoad", "1")], true));
        match after {
            Some(i) => root.children.insert(i + 1, calc),
            None => root.children.push(calc),
        }
    }
    xml::to_xml(&nodes)
}

// ---------------------------------------------------------------------------
// ODS. A sheet is a `table:table` under `office:spreadsheet`, and its rows
// carry no numbers: a row's number is its position, with a row that stands
// for several (`table:number-rows-repeated`) taking several positions.

/// Marks a sheet whose rows were numbered for re-basing, with its child
/// index. Removed again after rendering.
const ODS_SHEET_ATTR: &str = "rusticdoc-sheet";
/// Marks a row with its first position in the template.
const ODS_ROW_ATTR: &str = "rusticdoc-row";

/// The attributes that hold a formula cell's cached result.
const ODS_CACHED: [&str; 8] = [
    "office:value-type",
    "office:value",
    "office:date-value",
    "office:time-value",
    "office:boolean-value",
    "office:string-value",
    "office:currency",
    "calcext:value-type",
];

/// Elements that group rows without being rows.
const ODS_ROW_GROUPS: [&str; 3] = [
    "table:table-header-rows",
    "table:table-rows",
    "table:table-row-group",
];

/// The sheet holding the node at `path`: its child index under
/// `office:spreadsheet`, and its path.
pub fn ods_sheet_of(nodes: &[Node], path: &[usize]) -> Option<(usize, Path)> {
    for k in 2..=path.len() {
        let e = elem_at(nodes, &path[..k])?;
        if e.name == "table:table" {
            let parent = elem_at(nodes, &path[..k - 1])?;
            return (parent.name == "office:spreadsheet")
                .then(|| (path[k - 1], path[..k].to_vec()));
        }
    }
    None
}

/// Every row of a sheet in order, as a path from the sheet and the number
/// of positions it takes. Rows of a table nested in a cell are not listed.
fn ods_rows(table: &Elem) -> Vec<(Path, u32)> {
    fn walk(e: &Elem, prefix: &mut Path, out: &mut Vec<(Path, u32)>) {
        for (i, n) in e.children.iter().enumerate() {
            let Node::Elem(c) = n else { continue };
            prefix.push(i);
            if c.name == "table:table-row" {
                let count = c
                    .attr("table:number-rows-repeated")
                    .and_then(|v| v.parse::<u32>().ok())
                    .unwrap_or(1)
                    .max(1);
                out.push((prefix.clone(), count));
            } else if ODS_ROW_GROUPS.contains(&c.name.as_str()) {
                walk(c, prefix, out);
            }
            prefix.pop();
        }
    }
    let mut out = Vec::new();
    walk(table, &mut Vec::new(), &mut out);
    out
}

/// The first position (from 1) of the row at `row` in the sheet at `table`,
/// and how many positions it takes.
pub fn ods_row_span(nodes: &[Node], table: &[usize], row: &[usize]) -> Option<(u32, u32)> {
    let rel = row.strip_prefix(table)?;
    let mut at = 1u32;
    for (path, count) in ods_rows(elem_at(nodes, table)?) {
        if path == rel {
            return Some((at, count));
        }
        at = at.saturating_add(count);
    }
    None
}

/// Writes each row's template position on the rows of the sheets that have
/// row blocks, so that re-basing can tell copies and iterations apart after
/// rendering. [`rebase_ods`] removes the marks.
pub fn annotate_ods(nodes: &mut [Node], blocks: &[RowBlock]) {
    let Some(sheet_list) = xml::root_mut(nodes).and_then(|r| r.find_mut("office:spreadsheet"))
    else {
        return;
    };
    let mut sheets: Vec<usize> = blocks.iter().map(|b| b.sheet).collect();
    sheets.dedup();
    for k in sheets {
        let Some(Node::Elem(table)) = sheet_list.children.get_mut(k) else {
            continue;
        };
        table.set_attr(ODS_SHEET_ATTR, &k.to_string());
        let mut at = 1u32;
        for (path, count) in ods_rows(table) {
            if let Some(row) = elem_at_mut(&mut table.children, &path) {
                row.set_attr(ODS_ROW_ATTR, &at.to_string());
            }
            at = at.saturating_add(count);
        }
    }
}

/// Whether rendered `content.xml` holds sheets marked by [`annotate_ods`].
pub fn ods_marked(content: &str) -> bool {
    content.contains(ODS_SHEET_ATTR)
}

/// Re-bases the formulas of a rendered sheet marked by [`annotate_ods`] and
/// removes the marks. `blocks` are every row block of the part; the sheet
/// takes those of its own template sheet, so each copy a sheet loop made is
/// re-based on its own.
pub fn rebase_ods(table: &mut Elem, blocks: &[RowBlock]) {
    let Some(k) = table
        .attr(ODS_SHEET_ATTR)
        .and_then(|v| v.parse::<usize>().ok())
    else {
        return;
    };
    table.remove_attr(ODS_SHEET_ATTR);
    let mine: Vec<RowBlock> = blocks.iter().copied().filter(|b| b.sheet == k).collect();
    let rows = ods_rows(table);
    let template: Vec<u32> = rows
        .iter()
        .map(|(path, _)| {
            elem_at(&table.children, path)
                .and_then(|r| r.attr(ODS_ROW_ATTR))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0)
        })
        .collect();
    let layout = Layout::new(&mine, &template);
    for (i, (path, _)) in rows.iter().enumerate() {
        let Some(row) = elem_at_mut(&mut table.children, path) else {
            continue;
        };
        row.remove_attr(ODS_ROW_ATTR);
        for cell in row.children.iter_mut() {
            let Node::Elem(c) = cell else { continue };
            let Some(formula) = c.attr("table:formula") else {
                continue;
            };
            let moved = rewrite_odf_refs(&formula, |col, _, r, _, pos| {
                (col, layout.shift(Some(i), r, pos))
            });
            if moved != formula {
                c.set_attr("table:formula", &moved);
            }
            // The cached result is the template's; without one LibreOffice
            // computes the cell on load, as XLSX cells lose theirs.
            for attr in ODS_CACHED {
                c.remove_attr(attr);
            }
            c.children
                .retain(|n| !matches!(n, Node::Elem(e) if e.name == "text:p"));
        }
    }
    // The print ranges name the sheet's own cells, so they grow with its
    // loops as a range in a formula outside them does.
    if let Some(ranges) = table.attr("table:print-ranges") {
        let moved = rewrite_odf_ranges(&ranges, |col, _, r, _, pos| {
            (col, layout.shift(None, r, pos))
        });
        if moved != ranges {
            table.set_attr("table:print-ranges", &moved);
        }
    }
}

/// Rewrites a space-separated list of ODF cell range addresses such as
/// `'Sales'.A1:'Sales'.E6`, whatever sheet each names.
fn rewrite_odf_ranges(
    list: &str,
    mut f: impl FnMut(u32, bool, u32, bool, RefPos) -> (u32, u32),
) -> String {
    let mut out = Vec::new();
    for range in split_outside_quotes(list, ' ') {
        let ends: Vec<(String, &str)> = split_outside_quotes(range, ':')
            .into_iter()
            .map(|end| {
                let dot = split_outside_quotes(end, '.');
                match dot.as_slice() {
                    [sheet @ .., cell] if !sheet.is_empty() => {
                        (format!("{}.", sheet.join(".")), *cell)
                    }
                    _ => (String::new(), end),
                }
            })
            .collect();
        match move_ends(&ends, &mut f) {
            Some(moved) => out.push(moved.join(":")),
            None => out.push(range.to_string()),
        }
    }
    out.join(" ")
}

/// Moves the ends of a cell or a range, each a prefix such as a sheet and a
/// cell such as `$B$2`, and keeps a range the right way round as
/// [`rewrite_refs`] does. `None` when an end is not a cell.
fn move_ends(
    ends: &[(String, &str)],
    f: &mut impl FnMut(u32, bool, u32, bool, RefPos) -> (u32, u32),
) -> Option<Vec<String>> {
    let positions: &[RefPos] = match ends.len() {
        1 => &[RefPos::Single],
        2 => &[RefPos::RangeStart, RefPos::RangeEnd],
        _ => return None,
    };
    let mut moved: Vec<(String, u32)> = Vec::new();
    for ((prefix, cell), pos) in ends.iter().zip(positions) {
        let (col, abs_col, row, abs_row) = parse_cell(cell)?;
        let (nc, nr) = f(col_to_num(col), abs_col, row, abs_row, *pos);
        moved.push((
            format!(
                "{prefix}{}{}{}",
                if abs_col { "$" } else { "" },
                num_to_col(nc),
                if abs_row { "$" } else { "" },
            ),
            nr,
        ));
    }
    if let [(_, first), (_, last)] = moved.as_mut_slice() {
        if *last < *first {
            *first = *last;
        }
    }
    Some(moved.into_iter().map(|(p, r)| format!("{p}{r}")).collect())
}

/// Splits on `sep` where it is not inside single quotes.
fn split_outside_quotes(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut quoted = false;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        if c == '\'' {
            quoted = !quoted;
        } else if c == sep && !quoted {
            out.push(&s[start..i]);
            start = i + c.len_utf8();
        }
    }
    out.push(&s[start..]);
    out
}

/// Rewrites the references of an OpenFormula expression such as
/// `of:=SUM([.B2:.B4])`, like [`rewrite_refs`] does for A1 formulas. Only
/// references to the formula's own sheet (`[.B2]`) move; a reference that
/// names a sheet (`[$Other.B2]`) and string literals are left alone.
pub fn rewrite_odf_refs(
    formula: &str,
    mut f: impl FnMut(u32, bool, u32, bool, RefPos) -> (u32, u32),
) -> String {
    let b = formula.as_bytes();
    let mut out = String::with_capacity(formula.len());
    let mut i = 0;
    let mut last = 0;
    let mut in_string = false;
    while i < b.len() {
        match b[i] {
            b'"' => in_string = !in_string,
            b'[' if !in_string => {
                let mut j = i + 1;
                let mut quoted = false;
                while j < b.len() && (quoted || b[j] != b']') {
                    if b[j] == b'\'' {
                        quoted = !quoted;
                    }
                    j += 1;
                }
                if j < b.len() {
                    if let Some(inner) = rewrite_odf_ref(&formula[i + 1..j], &mut f) {
                        out.push_str(&formula[last..=i]);
                        out.push_str(&inner);
                        last = j;
                    }
                    i = j;
                }
            }
            _ => {}
        }
        i += 1;
    }
    out.push_str(&formula[last..]);
    out
}

/// One bracketed reference without its brackets, rewritten when it is a
/// cell or a range of cells on the formula's own sheet.
fn rewrite_odf_ref(
    inner: &str,
    f: &mut impl FnMut(u32, bool, u32, bool, RefPos) -> (u32, u32),
) -> Option<String> {
    let ends = inner
        .split(':')
        .map(|end| Some((".".to_string(), end.strip_prefix('.')?)))
        .collect::<Option<Vec<_>>>()?;
    Some(move_ends(&ends, f)?.join(":"))
}

/// Splits `$B$12` into `("B", true, 12, true)`; anything else is refused.
fn parse_cell(cell: &str) -> Option<(&str, bool, u32, bool)> {
    let (abs_col, rest) = match cell.strip_prefix('$') {
        Some(r) => (true, r),
        None => (false, cell),
    };
    let letters = rest.bytes().take_while(u8::is_ascii_uppercase).count();
    if letters == 0 || letters > 3 {
        return None;
    }
    let (col, rest) = rest.split_at(letters);
    let (abs_row, digits) = match rest.strip_prefix('$') {
        Some(r) => (true, r),
        None => (false, rest),
    };
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((col, abs_col, digits.parse().ok()?, abs_row))
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    #[test]
    fn rewrites_references_only() {
        let out = rewrite_refs(
            "SUM(A4:$B$5)+LOG10(C2)+Sheet2!D3+\"A1\"",
            |c, _, r, _, _| (c, r + 10),
        );
        assert_eq!(out, "SUM(A14:$B$15)+LOG10(C12)+Sheet2!D3+\"A1\"");
        assert_eq!(num_to_col(28), "AB");
        assert_eq!(col_to_num("AB"), 28);
        // Characters outside ASCII are copied whole.
        let out = rewrite_refs("'Zürich'!A1&\"€\"&B2", |c, _, r, _, _| (c, r + 1));
        assert_eq!(out, "'Zürich'!A1&\"€\"&B3");
    }

    #[test]
    fn a_range_turned_around_becomes_its_end_row() {
        // Rows 4 and 5 were removed: the start moves down past them, the end
        // up before them.
        let removed = |_c: u32, _: bool, r: u32, _: bool, pos: RefPos| match pos {
            RefPos::RangeStart => (1, r),
            _ => (1, r - 2),
        };
        assert_eq!(rewrite_refs("SUM(B4:B5)", removed), "SUM(A3:A3)");
        assert_eq!(
            rewrite_odf_refs("of:=SUM([.B4:.B5])", removed),
            "of:=SUM([.A3:.A3])"
        );
    }

    fn sheet(rows: &str) -> String {
        format!(
            "<worksheet><dimension ref=\"A1:C7\"/><sheetData>{rows}</sheetData><mergeCells count=\"1\"><mergeCell ref=\"A4:B4\"/></mergeCells></worksheet>"
        )
    }

    #[test]
    fn renumbers_copied_rows_and_formulas() {
        let rows = concat!(
            "<row r=\"1\"><c r=\"A1\"><v>1</v></c></row>",
            "<row r=\"4\"><c r=\"A4\"><v>10</v></c><c r=\"B4\"><f>A4*2</f><v>20</v></c></row>",
            "<row r=\"4\"><c r=\"A4\"><v>11</v></c><c r=\"B4\"><f>A4*2</f><v>22</v></c></row>",
            "<row r=\"4\"><c r=\"A4\"><v>12</v></c><c r=\"B4\"><f>A4*2</f><v>24</v></c></row>",
            "<row r=\"6\"><c r=\"B6\"><f>SUM(B4:B4)+A1</f><v>0</v></c></row>",
            "<row r=\"7\"><c r=\"B7\"><f>B6</f></c></row>"
        );
        let blocks = [RowBlock {
            start: 4,
            end: 4,
            is_loop: true,
            sheet: 0,
        }];
        let (out, _) = rebase(&sheet(rows), &blocks, &[]).unwrap();
        for expected in [
            "<row r=\"5\"><c r=\"A5\"><v>11</v></c><c r=\"B5\"><f>A5*2</f></c></row>",
            "<row r=\"6\"><c r=\"A6\"><v>12</v></c><c r=\"B6\"><f>A6*2</f></c></row>",
            "<row r=\"8\"><c r=\"B8\"><f>SUM(B4:B6)+A1</f></c></row>",
            "<row r=\"9\"><c r=\"B9\"><f>B8</f></c></row>",
            "<dimension ref=\"A1:C9\"/>",
            "<mergeCell ref=\"A4:B4\"/><mergeCell ref=\"A5:B5\"/><mergeCell ref=\"A6:B6\"/>",
        ] {
            assert!(out.contains(expected), "missing {expected} in {out}");
        }
    }

    #[test]
    fn removes_rows_of_an_empty_loop() {
        let rows = concat!(
            "<row r=\"1\"><c r=\"A1\"><v>1</v></c></row>",
            "<row r=\"6\"><c r=\"B6\"><f>SUM(B4:B5)</f></c></row>"
        );
        let blocks = [RowBlock {
            start: 4,
            end: 5,
            is_loop: true,
            sheet: 0,
        }];
        let (out, _) = rebase(&sheet(rows), &blocks, &[]).unwrap();
        assert!(
            out.contains("<row r=\"4\"><c r=\"B4\"><f>SUM(B3:B3)</f></c></row>"),
            "{out}"
        );
        assert!(out.contains("<mergeCells count=\"0\">"), "{out}");
    }

    #[test]
    fn unshares_shared_formulas() {
        let rows = concat!(
            "<row r=\"2\"><c r=\"C2\"><f t=\"shared\" ref=\"C2:C3\" si=\"0\">A2+$B$1</f><v>1</v></c></row>",
            "<row r=\"3\"><c r=\"C3\"><f t=\"shared\" si=\"0\"/><v>2</v></c></row>"
        );
        let (out, _) = rebase(&sheet(rows), &[], &[]).unwrap();
        assert!(out.contains("<c r=\"C2\"><f>A2+$B$1</f></c>"), "{out}");
        assert!(out.contains("<c r=\"C3\"><f>A3+$B$1</f></c>"), "{out}");
    }

    #[test]
    fn adds_calc_pr() {
        let out = force_recalc("<workbook><sheets/><extLst/></workbook>");
        assert_eq!(
            out,
            "<workbook><sheets/><calcPr fullCalcOnLoad=\"1\"/><extLst/></workbook>"
        );
        let out = force_recalc("<workbook><calcPr calcId=\"1\"/></workbook>");
        assert_eq!(
            out,
            "<workbook><calcPr calcId=\"1\" fullCalcOnLoad=\"1\"/></workbook>"
        );
    }

    fn block(start: u32, end: u32) -> RowBlock {
        RowBlock {
            start,
            end,
            is_loop: true,
            sheet: 0,
        }
    }

    /// Rows 2 to 4 repeat per group: a header, an item row repeated per item
    /// (row 3) and a subtotal. Groups get two, zero and one items.
    #[test]
    fn renumbers_a_loop_nested_in_a_loop() {
        let item = "<row r=\"3\"><c r=\"B3\"><f>A3*2</f></c></row>";
        let head = "<row r=\"2\"><c r=\"A2\"><v>0</v></c></row>";
        let sub = "<row r=\"4\"><c r=\"B4\"><f>SUM(B2:B3)</f></c></row>";
        let rows = [
            "<row r=\"1\"><c r=\"A1\"><v>1</v></c></row>",
            head,
            item,
            item,
            sub,
            head,
            sub,
            head,
            item,
            sub,
            "<row r=\"6\"><c r=\"B6\"><f>SUBTOTAL(9,B2:B4)+B4</f></c></row>",
        ]
        .concat();
        let src = format!(
            "<worksheet><dimension ref=\"A1:B6\"/><sheetData>{rows}</sheetData><mergeCells count=\"2\"><mergeCell ref=\"A2:B2\"/><mergeCell ref=\"C3:D3\"/></mergeCells></worksheet>"
        );
        let (out, _) = rebase(&src, &[block(2, 4), block(3, 3)], &[]).unwrap();
        let numbers: Vec<&str> = Regex::new("<row r=\"([0-9]+)\"")
            .unwrap()
            .captures_iter(&out)
            .map(|c| c.get(1).unwrap().as_str())
            .collect();
        assert_eq!(
            numbers,
            ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "12"]
        );
        let formulas: Vec<&str> = Regex::new("<f>([^<]*)</f>")
            .unwrap()
            .captures_iter(&out)
            .map(|c| c.get(1).unwrap().as_str())
            .collect();
        assert_eq!(
            formulas,
            [
                "A3*2",
                "A4*2",
                // Each subtotal sums its own group's rows; an empty group's
                // range shrinks to its header.
                "SUM(B2:B4)",
                "SUM(B6:B6)",
                "A9*2",
                "SUM(B8:B9)",
                // From outside, a range runs from the first group to the last.
                "SUBTOTAL(9,B2:B10)+B10",
            ]
        );
        assert!(out.contains("<dimension ref=\"A1:B12\"/>"), "{out}");
        assert!(
            out.contains("<mergeCells count=\"6\"><mergeCell ref=\"A2:B2\"/><mergeCell ref=\"A6:B6\"/><mergeCell ref=\"A8:B8\"/><mergeCell ref=\"C3:D3\"/><mergeCell ref=\"C4:D4\"/><mergeCell ref=\"C9:D9\"/></mergeCells>"),
            "{out}"
        );
    }

    #[test]
    fn rewrites_odf_references_on_the_formulas_own_sheet() {
        let out = rewrite_odf_refs(
            "of:=SUM([.B2:.B4])+[$Other.B2]+['It''s'.C1]+[.$C$3]*\"[.B2]\"&[.A:.A]",
            |c, _, r, _, _| (c, r + 10),
        );
        assert_eq!(
            out,
            "of:=SUM([.B12:.B14])+[$Other.B2]+['It''s'.C1]+[.$C$13]*\"[.B2]\"&[.A:.A]"
        );
        let mut seen = Vec::new();
        rewrite_odf_refs("of:=[.A1]+SUM([.B2:.C3])", |c, _, r, _, pos| {
            seen.push(pos);
            (c, r)
        });
        assert_eq!(seen, [RefPos::Single, RefPos::RangeStart, RefPos::RangeEnd]);
    }

    #[test]
    fn rewrites_odf_range_lists() {
        let out = rewrite_odf_ranges("'My sheet'.A1:'My sheet'.$E$6 Other.B2", |c, _, r, _, _| {
            (c, r * 2)
        });
        assert_eq!(out, "'My sheet'.A2:'My sheet'.$E$12 Other.B4");
    }

    #[test]
    fn defined_names_follow_their_sheets_rows() {
        // Row 3 came out three times.
        let layout = Layout::new(&[block(3, 3)], &[1, 2, 3, 3, 3, 4, 5, 6]);
        let lookup = |name: &str| (name == "My 'Sales'").then_some(&layout);
        assert_eq!(
            rebase_named("'My ''Sales'''!$A$1:$E$6,Other!$A$1:$A$6", lookup),
            "'My ''Sales'''!$A$1:$E$8,Other!$A$1:$A$6"
        );
        assert_eq!(rebase_named("Other!$A$6", lookup), "Other!$A$6");
    }

    #[test]
    fn counts_ods_rows_through_groups_and_repeats() {
        let src = "<office:document-content><office:body><office:spreadsheet><table:table-calculation-settings/><table:table table:name=\"S\"><table:table-header-rows><table:table-row/></table:table-header-rows><table:table-row table:number-rows-repeated=\"3\"/><table:table-row-group><table:table-row><table:table-cell><table:table><table:table-row/></table:table></table:table-cell></table:table-row></table:table-row-group><table:table-row/></table:table></office:spreadsheet></office:body></office:document-content>";
        let mut nodes = xml::parse(src).unwrap();
        // document-content > body > spreadsheet > table > row group > row.
        let row_in_group = vec![0, 0, 0, 1, 2, 0];
        let (sheet, table) = ods_sheet_of(&nodes, &row_in_group).unwrap();
        assert_eq!((sheet, table.as_slice()), (1, &[0, 0, 0, 1][..]));
        assert_eq!(ods_row_span(&nodes, &table, &row_in_group), Some((5, 1)));
        assert_eq!(ods_row_span(&nodes, &table, &[0, 0, 0, 1, 1]), Some((2, 3)));
        assert_eq!(ods_row_span(&nodes, &table, &[0, 0, 0, 1, 3]), Some((6, 1)));
        // The row of a table in a cell is not a sheet row.
        assert_eq!(
            ods_row_span(&nodes, &table, &[0, 0, 0, 1, 2, 0, 0, 0, 0]),
            None
        );
        annotate_ods(
            &mut nodes,
            &[RowBlock {
                sheet: 1,
                ..block(5, 5)
            }],
        );
        let out = xml::to_xml(&nodes);
        assert!(
            out.contains("<table:table table:name=\"S\" rusticdoc-sheet=\"1\">"),
            "{out}"
        );
        assert!(
            out.contains("<table:table-row table:number-rows-repeated=\"3\" rusticdoc-row=\"2\"/>"),
            "{out}"
        );
        assert!(
            out.contains("<table:table><table:table-row/></table:table>"),
            "the nested row is not marked: {out}"
        );
    }

    fn cols(row: u32, start: u32, end: u32) -> ColBlock {
        ColBlock { row, start, end }
    }

    #[test]
    fn a_column_loop_moves_its_copies_to_the_right() {
        // Row 6 repeats B and C three times, and D sums over them.
        let copy = |v: u32| {
            format!("<c r=\"B6\"><v>{v}</v></c><c r=\"C6\"><f>B6*B$2+$A6+$H$1</f><v>0</v></c>")
        };
        let rows = format!(
            "<row r=\"2\"><c r=\"B2\"><v>7</v></c></row>\
             <row r=\"6\" spans=\"1:4\"><c r=\"A6\"><v>1</v></c>{}{}{}\
             <c r=\"D6\"><f>SUM(B6:C6)+B6</f></c></row>\
             <row r=\"7\"><c r=\"D7\"><f>D6+B6</f></c></row>",
            copy(10),
            copy(11),
            copy(12)
        );
        let (out, _) = rebase(&sheet(&rows), &[], &[cols(6, 2, 3)]).unwrap();
        for expected in [
            "<dimension ref=\"A1:H7\"/>",
            "<row r=\"6\"><c r=\"A6\"><v>1</v></c>",
            // A copy points at its own cells, in its own row and in others.
            "<c r=\"B6\"><v>10</v></c><c r=\"C6\"><f>B6*B$2+$A6+$H$1</f></c>",
            "<c r=\"D6\"><v>11</v></c><c r=\"E6\"><f>D6*D$2+$A6+$H$1</f></c>",
            "<c r=\"F6\"><v>12</v></c><c r=\"G6\"><f>F6*F$2+$A6+$H$1</f></c>",
            // From outside, a range widens and a single cell is the first copy.
            "<c r=\"H6\"><f>SUM(B6:G6)+B6</f></c>",
            // A row without a column loop is left alone.
            "<row r=\"7\"><c r=\"D7\"><f>D6+B6</f></c></row>",
        ] {
            assert!(out.contains(expected), "missing {expected}\n{out}");
        }
    }

    #[test]
    fn a_column_loop_with_no_items_pulls_the_row_in() {
        let rows = "<row r=\"6\"><c r=\"A6\"><v>1</v></c>\
                    <c r=\"D6\"><f>SUM(B6:C6)</f></c><c r=\"E6\"><f>D6</f></c></row>";
        let (out, _) = rebase(&sheet(rows), &[], &[cols(6, 2, 3)]).unwrap();
        assert!(
            out.contains(
                "<c r=\"A6\"><v>1</v></c><c r=\"B6\"><f>SUM(A6:A6)</f></c><c r=\"C6\"><f>B6</f></c>"
            ),
            "{out}"
        );
    }

    #[test]
    fn two_column_loops_in_a_row_push_each_other() {
        let rows = "<row r=\"3\"><c r=\"A3\"/><c r=\"B3\"/><c r=\"B3\"/><c r=\"C3\"/>\
                    <c r=\"D3\"/><c r=\"D3\"/><c r=\"D3\"/>\
                    <c r=\"E3\"><f>SUM(B3:B3)+SUM(D3:D3)+C3</f></c></row>\
                    <row r=\"4\"><c r=\"A4\"/><c r=\"B4\"/><c r=\"C4\"><f>B4</f></c></row>";
        // The blocks arrive in any order.
        let blocks = [cols(3, 4, 4), cols(4, 9, 9), cols(3, 2, 2)];
        let (out, _) = rebase(&sheet(rows), &[], &blocks).unwrap();
        for expected in [
            "<row r=\"3\"><c r=\"A3\"/><c r=\"B3\"/><c r=\"C3\"/><c r=\"D3\"/>\
             <c r=\"E3\"/><c r=\"F3\"/><c r=\"G3\"/>\
             <c r=\"H3\"><f>SUM(B3:C3)+SUM(E3:G3)+D3</f></c></row>",
            "<row r=\"4\"><c r=\"A4\"/><c r=\"B4\"/><c r=\"C4\"><f>B4</f></c></row>",
            "<dimension ref=\"A1:H4\"/>",
        ] {
            let expected = expected.replace("             ", "");
            assert!(out.contains(&expected), "missing {expected}\n{out}");
        }
    }

    #[test]
    fn a_column_loop_cannot_run_past_the_last_column() {
        let row = format!("<row r=\"1\">{}</row>", "<c r=\"A1\"/>".repeat(16_385));
        let err = rebase(&sheet(&row), &[], &[cols(1, 1, 1)])
            .err()
            .expect("too many columns is an error");
        assert!(err.contains("more than 16,384 columns"), "{err}");
        let row = format!("<row r=\"1\">{}</row>", "<c r=\"A1\"/>".repeat(16_384));
        assert!(rebase(&sheet(&row), &[], &[cols(1, 1, 1)]).is_ok());
    }
}
