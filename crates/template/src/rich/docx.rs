//! DOCX: runs are split around markers and block content replaces paragraphs.

use std::fmt::Write as _;
use std::sync::Arc;

use crate::rich::html::{self, Block, Inline};
use crate::rich::media::{self, Fit};
use crate::rich::pkg::{Pkg, CHART_CT, OFFICE_RELS, REL_CHART, REL_HYPERLINK};
use crate::rich::placeholder;
use crate::rich::{
    blank, emu, ensure_ns, esc, has_mark, insert_ordered, max_id, misplaced, segments, serialize,
    shell, text_node, Picture, Rich, Scope, Seg,
};
use crate::xml::{self, Elem, Node};
use crate::TemplateError;

const NAMESPACES: &[(&str, &str)] = &[
    ("r", OFFICE_RELS),
    (
        "wp",
        "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
    ),
    ("a", "http://schemas.openxmlformats.org/drawingml/2006/main"),
    (
        "pic",
        "http://schemas.openxmlformats.org/drawingml/2006/picture",
    ),
    (
        "c",
        "http://schemas.openxmlformats.org/drawingml/2006/chart",
    ),
];

const RPR_ORDER: &str =
    "w:rStyle w:rFonts w:b w:bCs w:i w:iCs w:caps w:smallCaps w:strike w:dstrike \
     w:outline w:shadow w:emboss w:imprint w:noProof w:snapToGrid w:vanish w:webHidden \
     w:color w:spacing w:w w:kern w:position w:sz w:szCs w:highlight w:u w:effect w:bdr \
     w:shd w:fitText w:vertAlign w:rtl w:cs w:em w:lang w:eastAsianLayout w:specVanish \
     w:oMath";

const TCPR_ORDER: &str =
    "w:cnfStyle w:tcW w:gridSpan w:hMerge w:vMerge w:tcBorders w:shd w:noWrap w:tcMar \
     w:textDirection w:tcFitText w:vAlign w:hideMark w:headers w:cellIns w:cellDel \
     w:cellMerge w:tcPrChange";

const PPR_ORDER: &str =
    "w:pStyle w:keepNext w:keepLines w:pageBreakBefore w:framePr w:widowControl w:numPr \
     w:suppressLineNumbers w:pBdr w:shd w:tabs w:suppressAutoHyphens w:kinsoku \
     w:wordWrap w:overflowPunct w:topLinePunct w:autoSpaceDE w:autoSpaceDN w:bidi \
     w:adjustRightInd w:snapToGrid w:spacing w:ind w:contextualSpacing w:mirrorIndents \
     w:suppressOverlap w:jc w:textDirection w:textAlignment w:textboxTightWrap \
     w:outlineLvl w:divId w:cnfStyle w:rPr w:sectPr w:pPrChange";

/// What the markup around one table cell comes to, roughly.
const CELL_MARKUP: usize = 80;
/// Half-points per heading level.
const HEADING_SIZES: [u32; 6] = [48, 36, 28, 24, 22, 20];
/// Indent of one list level, in twentieths of a point.
const LIST_INDENT: u32 = 360;
const LINK_COLOR: &str = "0563C1";

struct Docx<'a, 'b> {
    pkg: &'a mut Pkg<'b>,
    items: &'a [Arc<Rich>],
    part: &'a str,
    next_id: u64,
    /// Colours waiting for the paragraph, cell or row around their tag.
    effects: Vec<Effect>,
}

/// A colour for the structure around a tag.
struct Effect {
    scope: Scope,
    color: Option<String>,
    background: Option<String>,
}

pub fn rewrite(
    pkg: &mut Pkg,
    items: &[Arc<Rich>],
    part: &str,
    src: &str,
    nodes: &mut Vec<Node>,
) -> Result<(), TemplateError> {
    let mut ctx = Docx {
        pkg,
        items,
        part,
        next_id: max_id(src, "wp:docPr", "id") + 1,
        effects: Vec::new(),
    };
    if let Some(root) = xml::root_mut(nodes) {
        ensure_ns(root, NAMESPACES);
    }
    ctx.walk(nodes)?;
    match ctx.effects.first() {
        Some(effect) => Err(ctx.fail(&format!(
            "scope=\"{}\" needs the tag inside a table {}",
            effect.scope.name(),
            effect.scope.name()
        ))),
        None => Ok(()),
    }
}

/// Sets the colour on every run under `e` the template wrote.
fn color_runs(e: &mut Elem, color: &str) {
    for node in e.children.iter_mut() {
        let Node::Elem(child) = node else { continue };
        if child.name == "w:r" {
            set_run_color(child, color);
        } else if child.name == "w:pPr" {
            // The paragraph mark's own properties.
            if let Some(rpr) = child.child_mut("w:rPr") {
                insert_ordered(rpr, prop("w:color", &[("w:val", color)]), RPR_ORDER);
            }
        } else if child.name != "w:drawing" {
            color_runs(child, color);
        }
    }
}

fn set_run_color(run: &mut Elem, color: &str) {
    run.make_open();
    if run.child("w:rPr").is_none() {
        run.children
            .insert(0, Node::Elem(Elem::new("w:rPr", &[], false)));
    }
    if let Some(rpr) = run.child_mut("w:rPr") {
        insert_ordered(rpr, prop("w:color", &[("w:val", color)]), RPR_ORDER);
    }
}

fn shading(fill: &str) -> Elem {
    prop(
        "w:shd",
        &[("w:val", "clear"), ("w:color", "auto"), ("w:fill", fill)],
    )
}

/// Colours a table cell: its shading, and the text it holds.
fn color_cell(tc: &mut Elem, effect: &Effect) {
    if let Some(fill) = &effect.background {
        tc.make_open();
        if tc.child("w:tcPr").is_none() {
            tc.children
                .insert(0, Node::Elem(Elem::new("w:tcPr", &[], false)));
        }
        if let Some(tcpr) = tc.child_mut("w:tcPr") {
            insert_ordered(tcpr, shading(fill), TCPR_ORDER);
        }
    }
    if let Some(color) = &effect.color {
        color_runs(tc, color);
    }
}

/// Colours a paragraph: its shading, and its runs.
fn color_paragraph(p: &mut Elem, effect: &Effect) {
    if let Some(fill) = &effect.background {
        p.make_open();
        if p.child("w:pPr").is_none() {
            p.children
                .insert(0, Node::Elem(Elem::new("w:pPr", &[], false)));
        }
        if let Some(ppr) = p.child_mut("w:pPr") {
            insert_ordered(ppr, shading(fill), PPR_ORDER);
        }
    }
    if let Some(color) = &effect.color {
        color_runs(p, color);
    }
}

fn prop(name: &str, attrs: &[(&str, &str)]) -> Elem {
    Elem::new(name, attrs, true)
}

/// A copy of `base` with these properties set, in schema order.
fn rpr_with(base: Option<&Elem>, props: Vec<Elem>) -> String {
    let mut rpr = base
        .cloned()
        .unwrap_or_else(|| Elem::new("w:rPr", &[], false));
    for p in props {
        insert_ordered(&mut rpr, p, RPR_ORDER);
    }
    if rpr.children.is_empty() {
        return String::new();
    }
    serialize(&rpr)
}

/// `w:t` elements for text, with a `w:br` for each newline.
fn text_runs(text: &str) -> String {
    let parts: Vec<String> = text
        .split('\n')
        .map(|line| format!("<w:t xml:space=\"preserve\">{}</w:t>", esc(line)))
        .collect();
    parts.join("<w:br/>")
}

/// The base properties for a block's generated runs: the template run's.
fn inline_props(style: &html::Style) -> Vec<Elem> {
    let mut props = Vec::new();
    if style.bold {
        props.push(prop("w:b", &[]));
    }
    if style.italic {
        props.push(prop("w:i", &[]));
    }
    if style.strike {
        props.push(prop("w:strike", &[]));
    }
    if style.underline {
        props.push(prop("w:u", &[("w:val", "single")]));
    }
    if style.sub {
        props.push(prop("w:vertAlign", &[("w:val", "subscript")]));
    }
    if style.sup {
        props.push(prop("w:vertAlign", &[("w:val", "superscript")]));
    }
    if let Some(color) = &style.color {
        props.push(prop("w:color", &[("w:val", color)]));
    }
    props
}

fn link_props(base: Option<&Elem>) -> Vec<Elem> {
    let mut props = vec![prop("w:u", &[("w:val", "single")])];
    if !base.is_some_and(|b| b.child("w:color").is_some()) {
        props.push(prop("w:color", &[("w:val", LINK_COLOR)]));
    }
    props
}

/// A paragraph's properties for generated copies: a section break must stay
/// on the last paragraph only, or the document grows sections.
fn without_section(ppr: Option<&Elem>) -> Option<Elem> {
    let mut ppr = ppr.cloned()?;
    ppr.children
        .retain(|n| !matches!(n, Node::Elem(e) if e.name == "w:sectPr"));
    Some(ppr)
}

impl Docx<'_, '_> {
    fn fail(&self, message: &str) -> TemplateError {
        misplaced(self.part, "paragraph", message)
    }

    fn walk(&mut self, children: &mut Vec<Node>) -> Result<(), TemplateError> {
        let mut i = 0;
        while i < children.len() {
            let Node::Elem(e) = &mut children[i] else {
                i += 1;
                continue;
            };
            let before = self.effects.len();
            // Text boxes nest paragraphs inside paragraphs: inner ones first.
            self.walk(&mut e.children)?;
            if e.name == "w:p" && has_mark(&e.children) {
                let Node::Elem(p) = std::mem::replace(&mut children[i], Node::Raw(String::new()))
                else {
                    continue;
                };
                let mark = self.effects.len();
                let mut out = self.paragraph(p)?;
                let mine: Vec<Effect> = self.take_effects(mark, Scope::Paragraph);
                for effect in &mine {
                    for node in out.iter_mut() {
                        if let Node::Elem(p) = node {
                            color_paragraph(p, effect);
                        }
                    }
                }
                let count = out.len();
                children.splice(i..=i, out);
                i += count;
                continue;
            }
            let scope = match e.name.as_str() {
                "w:tc" => Some(Scope::Cell),
                "w:tr" => Some(Scope::Row),
                _ => None,
            };
            if let Some(scope) = scope {
                for effect in self.take_effects(before, scope) {
                    if scope == Scope::Cell {
                        color_cell(e, &effect);
                    } else {
                        for cell in e.children.iter_mut() {
                            if let Node::Elem(tc) = cell {
                                if tc.name == "w:tc" {
                                    color_cell(tc, &effect);
                                }
                            }
                        }
                    }
                }
            }
            i += 1;
        }
        Ok(())
    }

    /// The effects of `scope` recorded since `mark`, removed from the list.
    fn take_effects(&mut self, mark: usize, scope: Scope) -> Vec<Effect> {
        let mut taken = Vec::new();
        let mut i = mark;
        while i < self.effects.len() {
            if self.effects[i].scope == scope {
                taken.push(self.effects.remove(i));
            } else {
                i += 1;
            }
        }
        taken
    }

    fn paragraph(&mut self, p: Elem) -> Result<Vec<Node>, TemplateError> {
        let ppr = p.child("w:pPr").cloned();
        let copy_ppr = without_section(ppr.as_ref());
        let mut out: Vec<Node> = Vec::new();
        let mut cur: Vec<Node> = Vec::new();
        let empty = shell(&p);
        let close = |cur: &mut Vec<Node>, ppr: Option<&Elem>, out: &mut Vec<Node>| {
            let mut para = empty.clone();
            para.make_open();
            para.children.extend(ppr.cloned().map(Node::Elem));
            para.children.append(cur);
            out.push(Node::Elem(para));
        };
        for child in p.children {
            let Node::Elem(mut e) = child else {
                cur.push(child);
                continue;
            };
            if e.name == "w:pPr" {
                continue;
            }
            if !has_mark(&e.children) {
                cur.push(Node::Elem(e));
                continue;
            }
            if e.name != "w:r" {
                self.nested(&mut e)?;
                cur.push(Node::Elem(e));
                continue;
            }
            let rpr = e.child("w:rPr").cloned();
            for piece in self.split_run(&e)? {
                match piece {
                    Piece::Run(run) => cur.push(run),
                    Piece::Mark(id) => {
                        let item = self.items[id].clone();
                        if let Rich::Html(blocks) = &*item {
                            if !blank(&cur) {
                                close(&mut cur, copy_ppr.as_ref(), &mut out);
                            }
                            cur.clear();
                            let mut xml = String::new();
                            self.blocks(blocks, copy_ppr.as_ref(), rpr.as_ref(), 0, &mut xml)?;
                            if !xml.is_empty() {
                                out.push(Node::Raw(xml));
                            }
                        } else {
                            cur.push(self.inline(&item, rpr.as_ref(), false)?);
                        }
                    }
                }
            }
        }
        let has_section = ppr
            .as_ref()
            .is_some_and(|ppr| ppr.child("w:sectPr").is_some());
        if !blank(&cur) || out.is_empty() || has_section {
            close(&mut cur, ppr.as_ref(), &mut out);
        }
        Ok(out)
    }

    /// Runs inside a hyperlink, insertion or content control.
    fn nested(&mut self, parent: &mut Elem) -> Result<(), TemplateError> {
        let in_link = parent.name == "w:hyperlink";
        let mut out = Vec::with_capacity(parent.children.len());
        for child in std::mem::take(&mut parent.children) {
            match child {
                Node::Elem(run) if run.name == "w:r" && has_mark(&run.children) => {
                    let rpr = run.child("w:rPr").cloned();
                    for piece in self.split_run(&run)? {
                        match piece {
                            Piece::Run(node) => out.push(node),
                            Piece::Mark(id) => {
                                let item = self.items[id].clone();
                                if matches!(&*item, Rich::Html(_)) {
                                    return Err(self.fail(
                                        "html must sit directly in a paragraph, not in a \
                                         hyperlink or content control",
                                    ));
                                }
                                out.push(self.inline(&item, rpr.as_ref(), in_link)?);
                            }
                        }
                    }
                }
                Node::Elem(mut e) if has_mark(&e.children) => {
                    self.nested(&mut e)?;
                    out.push(Node::Elem(e));
                }
                other => out.push(other),
            }
        }
        parent.children = out;
        Ok(())
    }

    /// Cuts a run at its markers; the text around them stays in copies of the
    /// run, with its properties.
    fn split_run(&self, run: &Elem) -> Result<Vec<Piece>, TemplateError> {
        let piece = |children: Vec<Node>| {
            let mut copy = shell(run);
            copy.children
                .extend(run.child("w:rPr").cloned().map(Node::Elem));
            copy.children.extend(children);
            Piece::Run(Node::Elem(copy))
        };
        let mut out = Vec::new();
        let mut pending: Vec<Node> = Vec::new();
        for child in &run.children {
            match child {
                Node::Elem(e) if e.name == "w:rPr" => {}
                Node::Elem(e) if e.name == "w:t" && has_mark(&e.children) => {
                    for inner in &e.children {
                        let Node::Text(text) = inner else {
                            continue;
                        };
                        let segs = segments(&text.raw).map_err(|m| self.fail(&m))?;
                        for seg in segs {
                            match seg {
                                Seg::Text(s) => {
                                    let mut t = shell(e);
                                    t.set_attr("xml:space", "preserve");
                                    t.children = vec![text_node(s)];
                                    pending.push(Node::Elem(t));
                                }
                                Seg::Mark(id) => {
                                    if !pending.is_empty() {
                                        out.push(piece(std::mem::take(&mut pending)));
                                    }
                                    out.push(Piece::Mark(id));
                                }
                            }
                        }
                    }
                }
                other => pending.push(other.clone()),
            }
        }
        if !pending.is_empty() {
            out.push(piece(pending));
        }
        Ok(out)
    }

    fn inline(
        &mut self,
        item: &Rich,
        rpr: Option<&Elem>,
        in_link: bool,
    ) -> Result<Node, TemplateError> {
        let base = rpr_with(rpr, Vec::new());
        let markup = match item {
            Rich::Picture(picture) => format!(
                "<w:r>{base}<w:drawing>{}</w:drawing></w:r>",
                self.picture(picture)?
            ),
            Rich::Chart(chart) => {
                let part =
                    self.pkg
                        .new_part("word/charts", "chart", chart.xml("Sheet1", 1), CHART_CT)?;
                let rid = self.pkg.rel_to(self.part, REL_CHART, &part)?;
                format!(
                    "<w:r>{base}<w:drawing>{}</w:drawing></w:r>",
                    self.chart_frame(&rid, chart.width_pt, chart.height_pt)
                )
            }
            Rich::Link { url, text } => {
                if in_link {
                    return Err(self.fail("link cannot sit inside an existing hyperlink"));
                }
                let rid = self.pkg.rel(self.part, REL_HYPERLINK, url, true)?;
                let props = rpr_with(rpr, link_props(rpr));
                format!(
                    "<w:hyperlink r:id=\"{rid}\" w:history=\"1\"><w:r>{props}{}</w:r></w:hyperlink>",
                    text_runs(text)
                )
            }
            Rich::Color {
                text,
                color,
                background,
                scope,
            } => {
                let mut props = Vec::new();
                if let Some(color) = color {
                    props.push(prop("w:color", &[("w:val", color)]));
                }
                if *scope == Scope::Text {
                    if let Some(fill) = background {
                        props.push(shading(fill));
                    }
                } else {
                    self.effects.push(Effect {
                        scope: *scope,
                        color: color.clone(),
                        background: background.clone(),
                    });
                }
                format!("<w:r>{}{}</w:r>", rpr_with(rpr, props), text_runs(text))
            }
            Rich::PageBreak => "<w:r><w:br w:type=\"page\"/></w:r>".to_string(),
            Rich::Drop(_) => return Err(self.fail("drop() cannot sit here")),
            Rich::Html(_) => {
                return Err(self.fail("html must sit directly in a paragraph"));
            }
        };
        // Charged as written: escaping and line breaks make this larger
        // than the value it came from.
        self.pkg.charge(markup.len())?;
        Ok(Node::Raw(markup))
    }

    fn doc_pr(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn picture(&mut self, picture: &Picture) -> Result<String, TemplateError> {
        let blip = placeholder::blip(self.pkg, self.part, "word/media", &picture.media)?;
        let crop = framed_crop(picture);
        let (cx, cy) = (emu(picture.width_pt), emu(picture.height_pt));
        let id = self.doc_pr();
        Ok(format!(
            "<wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">\
             <wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:effectExtent l=\"0\" t=\"0\" r=\"0\" b=\"0\"/>\
             <wp:docPr id=\"{id}\" name=\"Picture {id}\"/><wp:cNvGraphicFramePr>\
             <a:graphicFrameLocks noChangeAspect=\"1\"/></wp:cNvGraphicFramePr><a:graphic>\
             <a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
             <pic:pic><pic:nvPicPr><pic:cNvPr id=\"{id}\" name=\"Picture {id}\"/><pic:cNvPicPr/>\
             </pic:nvPicPr><pic:blipFill>{blip}{crop}<a:stretch><a:fillRect/>\
             </a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/>\
             <a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/>\
             </a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline>"
        ))
    }

    fn chart_frame(&mut self, rid: &str, width_pt: f64, height_pt: f64) -> String {
        let (cx, cy) = (emu(width_pt), emu(height_pt));
        let id = self.doc_pr();
        format!(
            "<wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">\
             <wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:effectExtent l=\"0\" t=\"0\" r=\"0\" b=\"0\"/>\
             <wp:docPr id=\"{id}\" name=\"Chart {id}\"/><wp:cNvGraphicFramePr/><a:graphic>\
             <a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/chart\">\
             <c:chart r:id=\"{rid}\"/></a:graphicData></a:graphic></wp:inline>"
        )
    }

    // --- HTML ---------------------------------------------------------------

    fn blocks(
        &mut self,
        blocks: &[Block],
        ppr: Option<&Elem>,
        rpr: Option<&Elem>,
        level: u32,
        out: &mut String,
    ) -> Result<(), TemplateError> {
        for block in blocks {
            match block {
                Block::Para(runs) => self.para(runs, ppr, rpr, None, level, None, out)?,
                Block::Heading(n, runs) => self.para(runs, ppr, rpr, None, level, Some(*n), out)?,
                Block::List { ordered, items } => {
                    for (i, item) in items.iter().enumerate() {
                        let marker = html::list_marker(*ordered, i);
                        self.list_item(item, &marker, ppr, rpr, level + 1, out)?;
                    }
                }
                Block::Table(rows) => self.table(rows, ppr, rpr, out)?,
            }
        }
        Ok(())
    }

    fn list_item(
        &mut self,
        item: &[Block],
        marker: &str,
        ppr: Option<&Elem>,
        rpr: Option<&Elem>,
        level: u32,
        out: &mut String,
    ) -> Result<(), TemplateError> {
        let mut bullet = Some(marker);
        for block in item {
            match block {
                Block::Para(runs) => {
                    self.para(runs, ppr, rpr, bullet.take(), level, None, out)?;
                }
                Block::Heading(n, runs) => {
                    self.para(runs, ppr, rpr, bullet.take(), level, Some(*n), out)?;
                }
                other => {
                    if let Some(marker) = bullet.take() {
                        self.para(&[], ppr, rpr, Some(marker), level, None, out)?;
                    }
                    self.blocks(std::slice::from_ref(other), ppr, rpr, level, out)?;
                }
            }
        }
        if let Some(marker) = bullet {
            self.para(&[], ppr, rpr, Some(marker), level, None, out)?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn para(
        &mut self,
        runs: &[Inline],
        ppr: Option<&Elem>,
        rpr: Option<&Elem>,
        bullet: Option<&str>,
        level: u32,
        heading: Option<u8>,
        out: &mut String,
    ) -> Result<(), TemplateError> {
        let start = out.len();
        let mut props = ppr
            .cloned()
            .unwrap_or_else(|| Elem::new("w:pPr", &[], false));
        if level > 0 {
            let left = (level * LIST_INDENT).to_string();
            let hanging = LIST_INDENT.to_string();
            let mut attrs = vec![("w:left", left.as_str())];
            if bullet.is_some() {
                attrs.push(("w:hanging", hanging.as_str()));
            }
            insert_ordered(&mut props, prop("w:ind", &attrs), PPR_ORDER);
        }
        let mut heading_props = Vec::new();
        if let Some(n) = heading {
            let lvl = (n - 1).to_string();
            insert_ordered(
                &mut props,
                prop("w:outlineLvl", &[("w:val", &lvl)]),
                PPR_ORDER,
            );
            let size = HEADING_SIZES[usize::from(n - 1).min(5)].to_string();
            heading_props.push(prop("w:b", &[]));
            heading_props.push(prop("w:sz", &[("w:val", &size)]));
        }
        out.push_str("<w:p>");
        if !props.children.is_empty() {
            out.push_str(&serialize(&props));
        }
        if let Some(marker) = bullet {
            let base = rpr_with(rpr, heading_props.clone());
            let _ = write!(
                out,
                "<w:r>{base}<w:t xml:space=\"preserve\">{}</w:t><w:tab/></w:r>",
                esc(marker)
            );
        }
        for run in runs {
            match run {
                Inline::Break => out.push_str("<w:r><w:br/></w:r>"),
                Inline::Text(text, style) => {
                    let mut props = heading_props.clone();
                    props.extend(inline_props(style));
                    let body = format!("<w:t xml:space=\"preserve\">{}</w:t>", esc(text));
                    if let Some(url) = &style.href {
                        let rid = self.pkg.rel(self.part, REL_HYPERLINK, url, true)?;
                        props.extend(link_props(rpr));
                        let _ = write!(
                            out,
                            "<w:hyperlink r:id=\"{rid}\" w:history=\"1\"><w:r>{}{body}</w:r>\
                             </w:hyperlink>",
                            rpr_with(rpr, props)
                        );
                    } else {
                        let _ = write!(out, "<w:r>{}{body}</w:r>", rpr_with(rpr, props));
                    }
                }
            }
        }
        out.push_str("</w:p>");
        self.pkg.charge(out.len() - start)
    }

    fn table(
        &mut self,
        rows: &[Vec<html::Cell>],
        ppr: Option<&Elem>,
        rpr: Option<&Elem>,
        out: &mut String,
    ) -> Result<(), TemplateError> {
        let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
        if columns == 0 {
            return Ok(());
        }
        let width = 9000 / columns;
        out.push_str(
            "<w:tbl><w:tblPr><w:tblW w:w=\"5000\" w:type=\"pct\"/><w:tblBorders>\
             <w:top w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
             <w:left w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
             <w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
             <w:right w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
             <w:insideH w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
             <w:insideV w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
             </w:tblBorders></w:tblPr><w:tblGrid>",
        );
        for _ in 0..columns {
            let _ = write!(out, "<w:gridCol w:w=\"{width}\"/>");
        }
        out.push_str("</w:tblGrid>");
        for row in rows {
            out.push_str("<w:tr>");
            for i in 0..columns {
                let _ = write!(
                    out,
                    "<w:tc><w:tcPr><w:tcW w:w=\"{width}\" w:type=\"dxa\"/></w:tcPr>"
                );
                self.pkg.charge(CELL_MARKUP)?;
                let cell = row.get(i);
                let mut inner = String::new();
                if let Some(cell) = cell {
                    let bold;
                    let cell_rpr = if cell.header {
                        let mut base = rpr
                            .cloned()
                            .unwrap_or_else(|| Elem::new("w:rPr", &[], false));
                        insert_ordered(&mut base, prop("w:b", &[]), RPR_ORDER);
                        bold = base;
                        Some(&bold)
                    } else {
                        rpr
                    };
                    self.blocks(&cell.blocks, ppr, cell_rpr, 0, &mut inner)?;
                }
                // A cell must end in a paragraph.
                if inner.is_empty() || inner.ends_with("</w:tbl>") {
                    inner.push_str("<w:p/>");
                }
                out.push_str(&inner);
                out.push_str("</w:tc>");
            }
            out.push_str("</w:tr>");
        }
        out.push_str("</w:tbl><w:p/>");
        Ok(())
    }
}

enum Piece {
    Run(Node),
    Mark(usize),
}

/// The `a:srcRect` for a new picture given both sides and a fit.
pub fn framed_crop(picture: &Picture) -> String {
    match picture.fit {
        Some(fit @ (Fit::Contain | Fit::Cover)) if picture.sized => placeholder::src_rect(
            media::place(
                (picture.width_pt, picture.height_pt),
                picture.media.aspect(),
                fit,
            )
            .crop,
        ),
        _ => String::new(),
    }
}
