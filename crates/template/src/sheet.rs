//! Spreadsheet specifics: shared strings, and renumbering the rows a loop
//! copied so that cell references and formulas keep pointing at the right
//! cells.

use std::collections::HashMap;

use crate::program::RowBlock;
use crate::xml::{self, Elem, Node, Text};

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

fn col_to_num(col: &str) -> u32 {
    col.bytes().fold(0, |n, b| n * 26 + (b - b'A' + 1) as u32)
}

fn num_to_col(mut n: u32) -> String {
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
fn split_ref(r: &str) -> Option<(String, u32)> {
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
pub fn rewrite_refs(
    formula: &str,
    mut f: impl FnMut(u32, bool, u32, bool, RefPos) -> (u32, u32),
) -> String {
    let b = formula.as_bytes();
    let mut out = String::with_capacity(formula.len());
    let mut i = 0;
    let mut in_string = false;
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
                    out.push_str(&nr.to_string());
                    i = j;
                    continue;
                }
            }
        }
        out.push(c as char);
        i += 1;
    }
    out
}

#[derive(Clone, Copy, Default)]
struct BlockState {
    iters: i64,
    last_r: u32,
    base: i64,
    expansion: i64,
}

struct Layout<'a> {
    blocks: &'a [RowBlock],
    states: Vec<BlockState>,
}

impl Layout<'_> {
    fn len(&self, k: usize) -> i64 {
        (self.blocks[k].end - self.blocks[k].start + 1) as i64
    }

    /// Shift for a reference from outside a block: the blocks that end at
    /// or above the row move it. A reference to a block's last row lands on
    /// its last copy; the start of a range stays on the first copy, so
    /// `SUM(B4:B5)` and `SUM(B4:B4)` grow over every copy.
    fn outside(&self, row: u32, pos: RefPos) -> i64 {
        self.blocks
            .iter()
            .zip(&self.states)
            .filter(|(b, _)| {
                if pos == RefPos::RangeStart {
                    b.end < row
                } else {
                    b.end <= row
                }
            })
            .map(|(_, s)| s.expansion)
            .sum()
    }

    fn shift(&self, row: u32, from: Option<(usize, i64)>, pos: RefPos) -> u32 {
        let d = match from {
            Some((k, delta)) if row >= self.blocks[k].start && row <= self.blocks[k].end => delta,
            _ => self.outside(row, pos),
        };
        (row as i64 + d).max(1) as u32
    }
}

/// Renumbers the rows of a rendered sheet, re-bases formulas and merged
/// ranges, and drops cached formula values so they are recomputed.
pub fn rebase(src: &str, blocks: &[RowBlock]) -> Result<String, String> {
    let mut nodes = xml::parse(src)?;
    let root = xml::root_mut(&mut nodes).ok_or("sheet has no root element")?;
    let Some(data) = root.find_mut("sheetData") else {
        return Ok(src.to_string());
    };

    // Pass 1: the shift of every row and the iterations of every block.
    let mut layout = Layout {
        blocks,
        states: vec![BlockState::default(); blocks.len()],
    };
    let mut rows: Vec<(i64, Option<(usize, i64)>)> = Vec::new();
    let mut k = 0;
    let mut base = 0i64;
    let finish = |layout: &mut Layout, k: usize, base: &mut i64| {
        let len = layout.len(k);
        let st = &mut layout.states[k];
        st.base = *base;
        st.expansion = if st.iters > 0 {
            (st.iters - 1) * len
        } else {
            -len
        };
        *base += st.expansion;
    };
    let mut shared: HashMap<String, (u32, u32, String)> = HashMap::new();
    for row in data.children.iter() {
        let Node::Elem(row) = row else { continue };
        let r: u32 = row.attr("r").and_then(|r| r.parse().ok()).unwrap_or(0);
        while k < blocks.len() && r > blocks[k].end {
            finish(&mut layout, k, &mut base);
            k += 1;
        }
        let info = if k < blocks.len() && r >= blocks[k].start {
            let len = layout.len(k);
            let st = &mut layout.states[k];
            if st.iters == 0 || r <= st.last_r {
                st.iters += 1;
            }
            st.last_r = r;
            let delta = base + (st.iters - 1) * len;
            (delta, Some((k, delta)))
        } else {
            (base, None)
        };
        rows.push(info);
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
    while k < blocks.len() {
        finish(&mut layout, k, &mut base);
        k += 1;
    }

    // Pass 2: rewrite rows, cells and formulas.
    let mut max_row = 0u32;
    let mut ri = 0;
    for row in data.children.iter_mut() {
        let Node::Elem(row) = row else { continue };
        let (delta, from) = rows[ri];
        ri += 1;
        let r: u32 = row.attr("r").and_then(|r| r.parse().ok()).unwrap_or(0);
        let new_r = (r as i64 + delta).max(1) as u32;
        max_row = max_row.max(new_r);
        row.set_attr("r", &new_r.to_string());
        for cell in row.children.iter_mut() {
            let Node::Elem(c) = cell else { continue };
            if c.name != "c" {
                continue;
            }
            let Some((col, _)) = c.attr("r").and_then(|r| split_ref(&r)) else {
                continue;
            };
            c.set_attr("r", &format!("{col}{new_r}"));
            if c.child("f").is_none() {
                continue;
            }
            // Without a cached value the cell type must not claim one.
            c.children
                .retain(|n| !matches!(n, Node::Elem(e) if e.name == "v"));
            c.remove_attr("t");
            let colnum = col_to_num(&col);
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
            let text = rewrite_refs(&text, |c, _, r, _, pos| (c, layout.shift(r, from, pos)));
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
            let inside = blocks.iter().position(|bl| ra >= bl.start && rb <= bl.end);
            match inside {
                Some(k) => {
                    let st = layout.states[k];
                    for i in 0..st.iters {
                        let d = st.base + i * layout.len(k);
                        out.push(format!("{ca}{}:{cb}{}", ra as i64 + d, rb as i64 + d));
                    }
                }
                None => out.push(format!(
                    "{ca}{}:{cb}{}",
                    layout.shift(ra, None, RefPos::RangeStart),
                    layout.shift(rb, None, RefPos::RangeEnd)
                )),
            }
        }
        merges.children = out
            .iter()
            .map(|r| Node::Elem(Elem::new("mergeCell", &[("ref", r)], true)))
            .collect();
        merges.set_attr("count", &out.len().to_string());
    }
    Ok(xml::to_xml(&nodes))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_references_only() {
        let out = rewrite_refs(
            "SUM(A4:$B$5)+LOG10(C2)+Sheet2!D3+\"A1\"",
            |c, _, r, _, _| (c, r + 10),
        );
        assert_eq!(out, "SUM(A14:$B$15)+LOG10(C12)+Sheet2!D3+\"A1\"");
        assert_eq!(num_to_col(28), "AB");
        assert_eq!(col_to_num("AB"), 28);
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
        }];
        let out = rebase(&sheet(rows), &blocks).unwrap();
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
        }];
        let out = rebase(&sheet(rows), &blocks).unwrap();
        assert!(
            out.contains("<row r=\"4\"><c r=\"B4\"><f>SUM(B4:B3)</f></c></row>"),
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
        let out = rebase(&sheet(rows), &[]).unwrap();
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
}
