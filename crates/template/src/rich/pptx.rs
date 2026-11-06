//! PPTX: runs are split around markers; pictures and charts become shapes on
//! the slide at the offset of the shape that held the tag.

use std::sync::Arc;

use crate::rich::html;
use crate::rich::pkg::{Pkg, CHART_CT, OFFICE_RELS, REL_CHART, REL_HYPERLINK, REL_IMAGE};
use crate::rich::{
    emu, ensure_ns, esc, has_mark, insert_ordered, max_id, misplaced, segments, serialize, Rich,
    Seg,
};
use crate::xml::{self, Elem, Node};
use crate::TemplateError;

const NAMESPACES: &[(&str, &str)] = &[
    ("r", OFFICE_RELS),
    ("a", "http://schemas.openxmlformats.org/drawingml/2006/main"),
    (
        "p",
        "http://schemas.openxmlformats.org/presentationml/2006/main",
    ),
    (
        "c",
        "http://schemas.openxmlformats.org/drawingml/2006/chart",
    ),
];

const RPR_ORDER: &str =
    "a:ln a:noFill a:solidFill a:gradFill a:blipFill a:pattFill a:grpFill a:effectLst \
     a:effectDag a:highlight a:uLnTx a:uLn a:uFillTx a:uFill a:latin a:ea a:cs a:sym \
     a:hlinkClick a:hlinkMouseOver a:rtl a:extLst";

const FILLS: &[&str] = &[
    "a:noFill",
    "a:solidFill",
    "a:gradFill",
    "a:blipFill",
    "a:pattFill",
    "a:grpFill",
];

struct Pptx<'a, 'b> {
    pkg: &'a mut Pkg<'b>,
    items: &'a [Arc<Rich>],
    part: &'a str,
    next_id: u64,
}

pub fn rewrite(
    pkg: &mut Pkg,
    items: &[Arc<Rich>],
    part: &str,
    src: &str,
    nodes: &mut [Node],
) -> Result<(), TemplateError> {
    let mut ctx = Pptx {
        pkg,
        items,
        part,
        next_id: max_id(src, "p:cNvPr", "id") + 1,
    };
    let Some(root) = xml::root_mut(nodes) else {
        return Ok(());
    };
    ensure_ns(root, NAMESPACES);
    let Some(tree) = root.find_mut("p:spTree") else {
        return Ok(());
    };
    ctx.container(tree)
}

fn fill(tag: &str, rgb: &str) -> Elem {
    let mut elem = Elem::new(tag, &[], false);
    elem.children
        .push(Node::Elem(Elem::new("a:srgbClr", &[("val", rgb)], true)));
    elem
}

/// A copy of the run's properties with these set, always as `a:rPr`.
fn rpr_with(base: Option<&Elem>, props: Vec<Elem>) -> Elem {
    let mut rpr = base
        .cloned()
        .unwrap_or_else(|| Elem::new("a:rPr", &[], true));
    for p in props {
        if FILLS.contains(&p.name.as_str()) {
            rpr.children
                .retain(|n| !matches!(n, Node::Elem(e) if FILLS.contains(&e.name.as_str())));
        }
        insert_ordered(&mut rpr, p, RPR_ORDER);
    }
    rpr
}

fn run(rpr: &Elem, raw_text: &str) -> Node {
    Node::Raw(format!(
        "<a:r>{}<a:t>{raw_text}</a:t></a:r>",
        serialize(rpr)
    ))
}

impl Pptx<'_, '_> {
    fn fail(&self, message: &str) -> TemplateError {
        misplaced(self.part, "slide", message)
    }

    /// Goes through a shape tree or group. New shapes join the same container
    /// as the shape that held the tag, so its offset means the same thing.
    fn container(&mut self, tree: &mut Elem) -> Result<(), TemplateError> {
        let mut added = Vec::new();
        for child in tree.children.iter_mut() {
            let Node::Elem(e) = child else { continue };
            match e.name.as_str() {
                "p:grpSp" | "mc:AlternateContent" | "mc:Choice" | "mc:Fallback" => {
                    self.container(e)?
                }
                _ if has_mark(&e.children) => {
                    let off = e.find("a:off");
                    let at = |name: &str| {
                        off.and_then(|o| o.attr(name))
                            .and_then(|v| v.parse::<i64>().ok())
                            .unwrap_or(0)
                    };
                    let (x, mut y) = (at("x"), at("y"));
                    self.paragraphs(&mut e.children, x, &mut y, &mut added)?;
                }
                _ => {}
            }
        }
        if !added.is_empty() {
            let at = tree
                .children
                .iter()
                .position(|n| matches!(n, Node::Elem(e) if e.name == "p:extLst"))
                .unwrap_or(tree.children.len());
            tree.children.splice(at..at, added);
        }
        Ok(())
    }

    fn paragraphs(
        &mut self,
        nodes: &mut [Node],
        x: i64,
        y: &mut i64,
        added: &mut Vec<Node>,
    ) -> Result<(), TemplateError> {
        for node in nodes {
            let Node::Elem(e) = node else { continue };
            if !has_mark(&e.children) {
                continue;
            }
            if e.name != "a:p" {
                self.paragraphs(&mut e.children, x, y, added)?;
                continue;
            }
            let mut out = Vec::with_capacity(e.children.len());
            for child in std::mem::take(&mut e.children) {
                match child {
                    Node::Elem(r) if r.name == "a:r" && has_mark(&r.children) => {
                        self.split_run(&r, x, y, added, &mut out)?;
                    }
                    other => out.push(other),
                }
            }
            e.children = out;
        }
        Ok(())
    }

    fn split_run(
        &mut self,
        r: &Elem,
        x: i64,
        y: &mut i64,
        added: &mut Vec<Node>,
        out: &mut Vec<Node>,
    ) -> Result<(), TemplateError> {
        let base = r.child("a:rPr");
        let plain = rpr_with(base, Vec::new());
        let raw = r.child("a:t").map(|t| xml::to_xml(&t.children));
        let raw = raw.unwrap_or_default();
        for seg in segments(&raw).map_err(|m| self.fail(&m))? {
            let id = match seg {
                Seg::Text(s) => {
                    out.push(run(&plain, s));
                    continue;
                }
                Seg::Mark(id) => id,
            };
            let item = self.items[id].clone();
            match &*item {
                Rich::Picture(picture) => {
                    let part = self.pkg.media_part(&picture.media, "ppt/media")?;
                    let rid = self.pkg.rel_to(self.part, REL_IMAGE, &part)?;
                    let (cx, cy) = (emu(picture.width_pt), emu(picture.height_pt));
                    added.push(Node::Raw(self.pic_xml(&rid, x, *y, cx, cy)));
                    *y += cy;
                }
                Rich::Chart(chart) => {
                    let part = self.pkg.new_part(
                        "ppt/charts",
                        "chart",
                        chart.xml("Sheet1", 1),
                        CHART_CT,
                    )?;
                    let rid = self.pkg.rel_to(self.part, REL_CHART, &part)?;
                    let (cx, cy) = (emu(chart.width_pt), emu(chart.height_pt));
                    added.push(Node::Raw(self.chart_xml(&rid, x, *y, cx, cy)));
                    *y += cy;
                }
                Rich::Link { url, text } => {
                    let rid = self.pkg.rel(self.part, REL_HYPERLINK, url, true)?;
                    let link = Elem::new("a:hlinkClick", &[("r:id", &rid)], true);
                    out.push(run(&rpr_with(base, vec![link]), &esc(text)));
                }
                Rich::Color {
                    text,
                    color,
                    background,
                } => {
                    let mut props = Vec::new();
                    if let Some(color) = color {
                        props.push(fill("a:solidFill", color));
                    }
                    if let Some(background) = background {
                        props.push(fill("a:highlight", background));
                    }
                    out.push(run(&rpr_with(base, props), &esc(text)));
                }
                Rich::Html(blocks) => {
                    for (i, line) in html::plain_lines(blocks).iter().enumerate() {
                        if i > 0 {
                            out.push(Node::Raw("<a:br/>".to_string()));
                        }
                        let line = run(&plain, &esc(line));
                        if let Node::Raw(markup) = &line {
                            self.pkg.charge(markup.len())?;
                        }
                        out.push(line);
                    }
                }
                Rich::PageBreak => return Err(self.fail("a slide has no page breaks")),
            }
        }
        Ok(())
    }

    fn shape_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn pic_xml(&mut self, rid: &str, x: i64, y: i64, cx: i64, cy: i64) -> String {
        let id = self.shape_id();
        format!(
            "<p:pic><p:nvPicPr><p:cNvPr id=\"{id}\" name=\"Picture {id}\"/><p:cNvPicPr>\
             <a:picLocks noChangeAspect=\"1\"/></p:cNvPicPr><p:nvPr/></p:nvPicPr><p:blipFill>\
             <a:blip r:embed=\"{rid}\"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr>\
             <a:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
             <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr></p:pic>"
        )
    }

    fn chart_xml(&mut self, rid: &str, x: i64, y: i64, cx: i64, cy: i64) -> String {
        let id = self.shape_id();
        format!(
            "<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id=\"{id}\" name=\"Chart {id}\"/>\
             <p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x=\"{x}\" \
             y=\"{y}\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></p:xfrm><a:graphic><a:graphicData \
             uri=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"><c:chart \
             r:id=\"{rid}\"/></a:graphicData></a:graphic></p:graphicFrame>"
        )
    }
}
