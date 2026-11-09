//! ODT, ODS and ODP. Text is mixed content, so markers are replaced in place.
//! ODT block content replaces paragraphs; sheets and presentations have no
//! paragraph structure to speak of, so html becomes plain text there, and
//! pictures go on the cell or the page.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use crate::rich::html::{self, Block, Inline, Style};
use crate::rich::media::{self, Fit, Media};
use crate::rich::pkg::Pkg;
use crate::rich::placeholder;
use crate::rich::{
    blank, ensure_ns, esc, esc_attr, has_mark, insert_ordered, marker, misplaced, segments, shell,
    text_node, Picture, Rich, Scope, Seg, MARK_OPEN,
};
use crate::xml::{self, Elem, Node};
use crate::{Format, TemplateError};

const NAMESPACES: &[(&str, &str)] = &[
    ("office", "urn:oasis:names:tc:opendocument:xmlns:office:1.0"),
    ("text", "urn:oasis:names:tc:opendocument:xmlns:text:1.0"),
    ("table", "urn:oasis:names:tc:opendocument:xmlns:table:1.0"),
    ("draw", "urn:oasis:names:tc:opendocument:xmlns:drawing:1.0"),
    ("style", "urn:oasis:names:tc:opendocument:xmlns:style:1.0"),
    (
        "fo",
        "urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0",
    ),
    (
        "svg",
        "urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0",
    ),
    ("xlink", "http://www.w3.org/1999/xlink"),
];

const STYLE_CHILD_ORDER: &str = "style:graphic-properties style:table-cell-properties \
                                 style:paragraph-properties style:text-properties";
/// What the markup around one table cell comes to, roughly.
const CELL_MARKUP: usize = 100;

/// Point sizes per heading level.
const HEADING_SIZES: [u32; 6] = [24, 18, 14, 12, 11, 10];

/// The automatic styles of one part: those it had, and those added to it.
struct StyleBook {
    names: HashSet<String>,
    existing: HashMap<String, Elem>,
    added: Vec<Node>,
    made: HashMap<String, String>,
    next: u32,
}

impl StyleBook {
    fn read(root: &Elem) -> StyleBook {
        let mut existing = HashMap::new();
        if let Some(styles) = root.child("office:automatic-styles") {
            for style in styles.children_named("style:style") {
                if let Some(name) = style.attr("style:name") {
                    existing.insert(name, style.clone());
                }
            }
        }
        let mut names: HashSet<String> = existing.keys().cloned().collect();
        collect_style_names(&root.children, &mut names);
        StyleBook {
            names,
            existing,
            added: Vec::new(),
            made: HashMap::new(),
            next: 1,
        }
    }

    fn fresh(&mut self, prefix: &str) -> String {
        loop {
            let name = format!("{prefix}{}", self.next);
            self.next += 1;
            if self.names.insert(name.clone()) {
                return name;
            }
        }
    }

    /// A style of `family` with these properties, based on `base`: a copy when
    /// `base` is an automatic style (which nothing may inherit from), otherwise
    /// a child of it.
    fn derived(
        &mut self,
        prefix: &str,
        family: &str,
        base: Option<&str>,
        props: &[(&str, Vec<(&str, String)>)],
    ) -> String {
        let key = format!("{family}|{base:?}|{props:?}");
        if let Some(found) = self.made.get(&key) {
            return found.clone();
        }
        let name = self.fresh(prefix);
        let mut style = match base.and_then(|b| self.existing.get(b)) {
            Some(found) => {
                let mut copy = found.clone();
                copy.set_attr("style:name", &name);
                copy
            }
            None => {
                let mut attrs = vec![("style:name", name.as_str()), ("style:family", family)];
                if let Some(base) = base {
                    attrs.push(("style:parent-style-name", base));
                }
                Elem::new("style:style", &attrs, false)
            }
        };
        style.make_open();
        for (element, attrs) in props {
            let mut child = style
                .child(element)
                .cloned()
                .unwrap_or_else(|| Elem::new(element, &[], true));
            for (k, v) in attrs {
                child.set_attr(k, v);
            }
            insert_ordered(&mut style, child, STYLE_CHILD_ORDER);
        }
        self.existing.insert(name.clone(), style.clone());
        self.added.push(Node::Elem(style));
        self.made.insert(key, name.clone());
        name
    }

    fn text_style(&mut self, attrs: Vec<(&str, String)>) -> String {
        self.derived("RichT", "text", None, &[("style:text-properties", attrs)])
    }

    fn list_style(&mut self, ordered: bool) -> String {
        let key = format!("list|{ordered}");
        if let Some(found) = self.made.get(&key) {
            return found.clone();
        }
        let name = self.fresh(if ordered { "RichNumber" } else { "RichBullet" });
        let mut xml = format!("<text:list-style style:name=\"{name}\">");
        for level in 1..=10 {
            let margin = f64::from(level) * 0.5;
            let props = format!(
                "<style:list-level-properties \
                 text:list-level-position-and-space-mode=\"label-alignment\">\
                 <style:list-level-label-alignment text:label-followed-by=\"listtab\" \
                 text:list-tab-stop-position=\"{margin}in\" fo:text-indent=\"-0.25in\" \
                 fo:margin-left=\"{margin}in\"/></style:list-level-properties>"
            );
            if ordered {
                let _ = write!(
                    xml,
                    "<text:list-level-style-number text:level=\"{level}\" style:num-format=\"1\" \
                     style:num-suffix=\".\">{props}</text:list-level-style-number>"
                );
            } else {
                let _ = write!(
                    xml,
                    "<text:list-level-style-bullet text:level=\"{level}\" \
                     text:bullet-char=\"\u{2022}\">{props}</text:list-level-style-bullet>"
                );
            }
        }
        xml.push_str("</text:list-style>");
        self.added.push(Node::Raw(xml));
        self.made.insert(key, name.clone());
        name
    }

    fn write_to(self, root: &mut Elem) {
        if self.added.is_empty() {
            return;
        }
        if root.child("office:automatic-styles").is_none() {
            let at = root
                .children
                .iter()
                .position(|n| {
                    matches!(n, Node::Elem(e) if matches!(
                        e.name.as_str(),
                        "office:master-styles" | "office:body"
                    ))
                })
                .unwrap_or(root.children.len());
            root.children.insert(
                at,
                Node::Elem(Elem::new("office:automatic-styles", &[], false)),
            );
        }
        if let Some(styles) = root.child_mut("office:automatic-styles") {
            styles.make_open();
            styles.children.extend(self.added);
        }
    }
}

fn collect_style_names(nodes: &[Node], names: &mut HashSet<String>) {
    for n in nodes {
        if let Node::Elem(e) = n {
            if let Some(name) = e.attr("style:name") {
                names.insert(name);
            }
            collect_style_names(&e.children, names);
        }
    }
}

struct Odf<'a, 'b> {
    pkg: &'a mut Pkg<'b>,
    format: Format,
    items: &'a [Arc<Rich>],
    part: &'a str,
    book: StyleBook,
    frames: u64,
    /// Pictures and colours of the cell being walked (ODS).
    cell_frames: Vec<Node>,
    cell_color: Option<(Option<String>, Option<String>)>,
    /// Pictures of the page being walked (ODP), and where its shape sits.
    page_frames: Vec<Node>,
    origin: (String, String),
    /// Colours waiting for the paragraph, cell, row or shape around them.
    effects: Vec<(Scope, Option<String>, Option<String>)>,
}

/// A length such as `2.5cm` in points.
fn length_pt(value: &str) -> Option<f64> {
    let value = value.trim();
    let at = value
        .find(|c: char| c.is_ascii_alphabetic() || c == '%')
        .unwrap_or(value.len());
    let n: f64 = value[..at].trim().parse().ok()?;
    let per = match &value[at..] {
        "pt" => 1.0,
        "in" => 72.0,
        "cm" => 72.0 / 2.54,
        "mm" => 72.0 / 25.4,
        "pc" => 12.0,
        "px" => 0.75,
        _ => return None,
    };
    Some(n * per).filter(|n| n.is_finite() && *n > 0.0)
}

/// `fo:clip` for a crop of media, in the image's own size as office suites
/// read it.
fn clip(media: &Media, crop: [f64; 4]) -> String {
    let (w, h) = (media.physical_pt.0 / 72.0, media.physical_pt.1 / 72.0);
    // No "-0.0000in" for a crop that rounds to nothing.
    let n = |v: f64| if v.abs() < 0.00005 { 0.0 } else { v };
    format!(
        "rect({:.4}in, {:.4}in, {:.4}in, {:.4}in)",
        n(crop[1] * h),
        n(crop[2] * w),
        n(crop[3] * h),
        n(crop[0] * w)
    )
}

pub fn rewrite(
    pkg: &mut Pkg,
    format: Format,
    items: &[Arc<Rich>],
    part: &str,
    src: &str,
    nodes: &mut [Node],
) -> Result<(), TemplateError> {
    let Some(root) = xml::root_mut(nodes) else {
        return Ok(());
    };
    ensure_ns(root, NAMESPACES);
    let book = StyleBook::read(root);
    let mut ctx = Odf {
        pkg,
        format,
        items,
        part,
        book,
        frames: src.matches("<draw:frame").count() as u64 + 1,
        cell_frames: Vec::new(),
        cell_color: None,
        page_frames: Vec::new(),
        origin: ("0pt".to_string(), "0pt".to_string()),
        effects: Vec::new(),
    };
    ctx.placeholders(&mut root.children)?;
    ctx.walk(&mut root.children, "")?;
    if let Some((scope, ..)) = ctx.effects.first() {
        let what = match scope {
            Scope::Shape => "a shape".to_string(),
            other => format!("a table {}", other.name()),
        };
        return Err(ctx.fail(&format!(
            "scope=\"{}\" needs the tag inside {what}",
            scope.name()
        )));
    }
    let book = std::mem::replace(
        &mut ctx.book,
        StyleBook {
            names: HashSet::new(),
            existing: HashMap::new(),
            added: Vec::new(),
            made: HashMap::new(),
            next: 1,
        },
    );
    book.write_to(root);
    Ok(())
}

fn text_markup(text: &str) -> String {
    esc(text).replace('\n', "<text:line-break/>")
}

impl Odf<'_, '_> {
    fn fail(&self, message: &str) -> TemplateError {
        misplaced(self.part, "paragraph", message)
    }

    /// Frames whose title or description holds an image tag show its image.
    fn placeholders(&mut self, nodes: &mut [Node]) -> Result<(), TemplateError> {
        for node in nodes.iter_mut() {
            let Node::Elem(e) = node else { continue };
            if !has_mark(&e.children) {
                continue;
            }
            if e.name == "draw:frame" {
                self.placeholder(e)?;
            }
            self.placeholders(&mut e.children)?;
        }
        Ok(())
    }

    fn placeholder(&mut self, frame: &mut Elem) -> Result<(), TemplateError> {
        let place = match frame.attr("draw:name") {
            Some(name) => format!("picture {name:?}"),
            None => "picture".to_string(),
        };
        let mut ids = Vec::new();
        for node in frame.children.iter_mut() {
            if let Node::Elem(e) = node {
                if e.name == "svg:title" || e.name == "svg:desc" {
                    ids.extend(placeholder::take_markers(e, self.part, &place)?);
                }
            }
        }
        if ids.is_empty() {
            return Ok(());
        }
        let picture = placeholder::picture(self.items, &ids, self.part, &place)?;
        let first = frame
            .children
            .iter()
            .position(|n| matches!(n, Node::Elem(e) if e.name == "draw:image"))
            .ok_or_else(|| {
                misplaced(
                    self.part,
                    &place,
                    "only a picture can be a placeholder; this frame holds no image",
                )
            })?;
        let size = frame
            .attr("svg:width")
            .and_then(|w| length_pt(&w))
            .zip(frame.attr("svg:height").and_then(|h| length_pt(&h)))
            .ok_or_else(|| {
                misplaced(
                    self.part,
                    &place,
                    "the placeholder picture has no size in points",
                )
            })?;
        let fit = picture.fit.unwrap_or(Fit::Contain);
        let placed = media::place(size, picture.media.aspect(), fit);
        let images = self.images(&picture.media)?;
        frame
            .children
            .retain(|n| !matches!(n, Node::Elem(e) if e.name == "draw:image"));
        frame.children.splice(
            first.min(frame.children.len())..first.min(frame.children.len()),
            images,
        );
        let base = frame.attr("draw:style-name");
        let style = self.book.derived(
            "RichG",
            "graphic",
            base.as_deref(),
            &[(
                "style:graphic-properties",
                vec![("fo:clip", clip(&picture.media, placed.crop))],
            )],
        );
        frame.set_attr("draw:style-name", &style);
        if let Some(height) = placed.height {
            frame.set_attr("svg:height", &format!("{height:.2}pt"));
        }
        Ok(())
    }

    /// The `draw:image` elements for media: an SVG comes with its PNG.
    fn images(&mut self, media: &Media) -> Result<Vec<Node>, TemplateError> {
        let mut out = Vec::new();
        for m in std::iter::once(media).chain(media.fallback.as_deref()) {
            let path = self.pkg.media_part(m, "Pictures")?;
            out.push(Node::Raw(format!(
                "<draw:image xlink:href=\"{path}\" xlink:type=\"simple\" xlink:show=\"embed\" \
                 xlink:actuate=\"onLoad\" draw:mime-type=\"{}\"/>",
                m.mime
            )));
        }
        Ok(out)
    }

    fn take_effects(
        &mut self,
        mark: usize,
        scope: Scope,
    ) -> Vec<(Scope, Option<String>, Option<String>)> {
        let mut taken = Vec::new();
        let mut i = mark;
        while i < self.effects.len() {
            if self.effects[i].0 == scope {
                taken.push(self.effects.remove(i));
            } else {
                i += 1;
            }
        }
        taken
    }

    /// Gives every paragraph under `e` a text colour.
    fn color_paragraphs(&mut self, e: &mut Elem, color: &str) {
        if matches!(e.name.as_str(), "text:p" | "text:h") {
            let base = e.attr("text:style-name");
            let style = self.book.derived(
                "RichP",
                "paragraph",
                base.as_deref(),
                &[(
                    "style:text-properties",
                    vec![("fo:color", format!("#{}", color.to_lowercase()))],
                )],
            );
            e.set_attr("text:style-name", &style);
            return;
        }
        for node in e.children.iter_mut() {
            if let Node::Elem(child) = node {
                self.color_paragraphs(child, color);
            }
        }
    }

    fn color_cell(&mut self, cell: &mut Elem, color: Option<&str>, background: Option<&str>) {
        let mut props = Vec::new();
        if let Some(bg) = background {
            let bg = format!("#{}", bg.to_lowercase());
            props.push(if self.format == Format::Odp {
                (
                    "style:graphic-properties",
                    vec![("draw:fill", "solid".to_string()), ("draw:fill-color", bg)],
                )
            } else {
                (
                    "style:table-cell-properties",
                    vec![("fo:background-color", bg)],
                )
            });
        }
        if let (Some(color), Format::Ods) = (color, self.format) {
            props.push((
                "style:text-properties",
                vec![("fo:color", format!("#{}", color.to_lowercase()))],
            ));
        }
        if !props.is_empty() {
            let base = cell.attr("table:style-name");
            let name = self
                .book
                .derived("RichC", "table-cell", base.as_deref(), &props);
            cell.set_attr("table:style-name", &name);
        }
        if let (Some(color), false) = (color, self.format == Format::Ods) {
            self.color_paragraphs(cell, color);
        }
    }

    fn color_shape(&mut self, shape: &mut Elem, color: Option<&str>, background: Option<&str>) {
        let mut props = Vec::new();
        if let Some(bg) = background {
            props.push((
                "style:graphic-properties",
                vec![
                    ("draw:fill", "solid".to_string()),
                    ("draw:fill-color", format!("#{}", bg.to_lowercase())),
                ],
            ));
        }
        if !props.is_empty() {
            let (attr, family) = if shape.attr("presentation:style-name").is_some() {
                ("presentation:style-name", "presentation")
            } else {
                ("draw:style-name", "graphic")
            };
            let base = shape.attr(attr);
            let name = self.book.derived("RichS", family, base.as_deref(), &props);
            shape.set_attr(attr, &name);
        }
        if let Some(color) = color {
            self.color_paragraphs(shape, color);
        }
    }

    fn is_block(&self, id: usize) -> bool {
        self.format == Format::Odt && matches!(&*self.items[id], Rich::Html(_) | Rich::PageBreak)
    }

    fn walk(&mut self, children: &mut Vec<Node>, parent: &str) -> Result<(), TemplateError> {
        let mut i = 0;
        while i < children.len() {
            let Node::Elem(e) = &mut children[i] else {
                i += 1;
                continue;
            };
            let name = e.name.clone();
            if self.format == Format::Odp && parent == "draw:page" {
                self.origin = (
                    e.attr("svg:x").unwrap_or_else(|| "0pt".to_string()),
                    e.attr("svg:y").unwrap_or_else(|| "0pt".to_string()),
                );
            }
            let before = self.effects.len();
            // Frames nest paragraphs inside paragraphs: inner ones first.
            self.walk(&mut e.children, &name)?;
            if matches!(name.as_str(), "text:p" | "text:h") && has_mark(&e.children) {
                let Node::Elem(p) = std::mem::replace(&mut children[i], Node::Raw(String::new()))
                else {
                    continue;
                };
                let mark = self.effects.len();
                let mut out = self.paragraph(p, parent)?;
                for (_, color, background) in self.take_effects(mark, Scope::Paragraph) {
                    let mut props = Vec::new();
                    if let Some(bg) = background {
                        props.push((
                            "style:paragraph-properties",
                            vec![("fo:background-color", format!("#{}", bg.to_lowercase()))],
                        ));
                    }
                    if let Some(color) = color {
                        props.push((
                            "style:text-properties",
                            vec![("fo:color", format!("#{}", color.to_lowercase()))],
                        ));
                    }
                    for node in out.iter_mut() {
                        if let Node::Elem(p) = node {
                            let base = p.attr("text:style-name");
                            let style =
                                self.book
                                    .derived("RichP", "paragraph", base.as_deref(), &props);
                            p.set_attr("text:style-name", &style);
                        }
                    }
                }
                let count = out.len();
                children.splice(i..=i, out);
                i += count;
                continue;
            }
            match name.as_str() {
                "table:table-cell" => {
                    for (_, color, background) in self.take_effects(before, Scope::Cell) {
                        self.color_cell(e, color.as_deref(), background.as_deref());
                    }
                }
                "table:table-row" => {
                    for (_, color, background) in self.take_effects(before, Scope::Row) {
                        for cell in e.children.iter_mut() {
                            if let Node::Elem(c) = cell {
                                if c.name == "table:table-cell" {
                                    self.color_cell(c, color.as_deref(), background.as_deref());
                                }
                            }
                        }
                    }
                }
                "draw:frame" | "draw:custom-shape" | "draw:rect" | "draw:ellipse"
                | "draw:polygon" | "draw:path" => {
                    for (_, color, background) in self.take_effects(before, Scope::Shape) {
                        self.color_shape(e, color.as_deref(), background.as_deref());
                    }
                }
                _ => {}
            }
            if name == "table:table-cell" && self.format == Format::Ods {
                self.finish_cell(e);
            }
            if name == "draw:page" && !self.page_frames.is_empty() {
                let frames = std::mem::take(&mut self.page_frames);
                let at = e
                    .children
                    .iter()
                    .position(|n| {
                        matches!(n, Node::Elem(c) if matches!(
                            c.name.as_str(),
                            "presentation:notes" | "office:forms"
                        ))
                    })
                    .unwrap_or(e.children.len());
                e.children.splice(at..at, frames);
            }
            i += 1;
        }
        Ok(())
    }

    fn finish_cell(&mut self, cell: &mut Elem) {
        if let Some((color, background)) = self.cell_color.take() {
            let mut props = Vec::new();
            if let Some(background) = background {
                props.push((
                    "style:table-cell-properties",
                    vec![(
                        "fo:background-color",
                        format!("#{}", background.to_lowercase()),
                    )],
                ));
            }
            if let Some(color) = color {
                props.push((
                    "style:text-properties",
                    vec![("fo:color", format!("#{}", color.to_lowercase()))],
                ));
            }
            let base = cell.attr("table:style-name");
            let name = self
                .book
                .derived("RichC", "table-cell", base.as_deref(), &props);
            cell.set_attr("table:style-name", &name);
        }
        if !self.cell_frames.is_empty() {
            let frames = std::mem::take(&mut self.cell_frames);
            cell.make_open();
            cell.children.splice(0..0, frames);
        }
    }

    fn paragraph(&mut self, p: Elem, parent: &str) -> Result<Vec<Node>, TemplateError> {
        let empty = shell(&p);
        let base = p.attr("text:style-name");
        let with = |nodes: Vec<Node>| {
            let mut para = empty.clone();
            para.children = nodes;
            para
        };
        let mut pieces = self.cut(p.children)?.into_iter();
        let mut cur = match pieces.next() {
            Some(Cut::Nodes(nodes)) => with(nodes),
            _ => with(Vec::new()),
        };
        let mut out = Vec::new();
        while let Some(piece) = pieces.next() {
            let Cut::Mark(id) = piece else { continue };
            let next = match pieces.next() {
                Some(Cut::Nodes(nodes)) => nodes,
                _ => Vec::new(),
            };
            let item = self.items[id].clone();
            match &*item {
                Rich::Html(blocks) => {
                    if !blank(&cur.children) {
                        out.push(self.inline_paragraph(cur)?);
                    }
                    let mut xml = String::new();
                    let tables = parent != "text:list-item";
                    self.blocks(blocks, base.as_deref(), tables, &mut xml)?;
                    if !xml.is_empty() {
                        out.push(Node::Raw(xml));
                    }
                    cur = with(next);
                }
                Rich::PageBreak if blank(&next) => {
                    // Nothing follows: the break goes after this paragraph.
                    let own = cur.attr("text:style-name");
                    let style = self.break_style("fo:break-after", own.as_deref());
                    cur.set_attr("text:style-name", &style);
                    out.push(self.inline_paragraph(cur)?);
                    cur = with(next);
                }
                Rich::PageBreak => {
                    if !blank(&cur.children) {
                        out.push(self.inline_paragraph(cur)?);
                    }
                    let style = self.break_style("fo:break-before", base.as_deref());
                    cur = with(next);
                    cur.set_attr("text:style-name", &style);
                }
                _ => return Err(self.fail("this value is not block content")),
            }
        }
        if !blank(&cur.children) || out.is_empty() {
            out.push(self.inline_paragraph(cur)?);
        }
        Ok(out)
    }

    fn break_style(&mut self, side: &str, base: Option<&str>) -> String {
        self.book.derived(
            "RichP",
            "paragraph",
            base,
            &[(
                "style:paragraph-properties",
                vec![(side, "page".to_string())],
            )],
        )
    }

    /// Cuts a paragraph's content at its block markers, in one pass, repeating
    /// the elements that hold one on both sides. Other markers stay in the text.
    fn cut(&self, nodes: Vec<Node>) -> Result<Vec<Cut>, TemplateError> {
        let mut out = Vec::new();
        let mut cur = Vec::new();
        for node in nodes {
            match node {
                Node::Text(t) if t.raw.contains(MARK_OPEN) => {
                    for seg in segments(&t.raw).map_err(|m| self.fail(&m))? {
                        match seg {
                            Seg::Text(s) => cur.push(text_node(s)),
                            Seg::Mark(id) if self.is_block(id) => {
                                out.push(Cut::Nodes(std::mem::take(&mut cur)));
                                out.push(Cut::Mark(id));
                            }
                            Seg::Mark(id) => cur.push(text_node(marker(id))),
                        }
                    }
                }
                Node::Elem(mut e) if has_mark(&e.children) => {
                    let empty = shell(&e);
                    for piece in self.cut(std::mem::take(&mut e.children))? {
                        match piece {
                            Cut::Nodes(children) => {
                                let mut part = empty.clone();
                                part.children = children;
                                cur.push(Node::Elem(part));
                            }
                            Cut::Mark(id) => {
                                out.push(Cut::Nodes(std::mem::take(&mut cur)));
                                out.push(Cut::Mark(id));
                            }
                        }
                    }
                }
                other => cur.push(other),
            }
        }
        out.push(Cut::Nodes(cur));
        Ok(out)
    }

    fn inline_paragraph(&mut self, mut p: Elem) -> Result<Node, TemplateError> {
        p.children = self.inline(std::mem::take(&mut p.children), false)?;
        Ok(Node::Elem(p))
    }

    fn inline(&mut self, nodes: Vec<Node>, in_link: bool) -> Result<Vec<Node>, TemplateError> {
        let mut out = Vec::with_capacity(nodes.len());
        for node in nodes {
            match node {
                Node::Text(t) if t.raw.contains(MARK_OPEN) => {
                    for seg in segments(&t.raw).map_err(|m| self.fail(&m))? {
                        match seg {
                            Seg::Text(s) => out.push(text_node(s)),
                            Seg::Mark(id) => out.extend(self.item(id, in_link)?),
                        }
                    }
                }
                Node::Elem(mut e) if has_mark(&e.children) => {
                    let inner = in_link || e.name == "text:a";
                    e.children = self.inline(std::mem::take(&mut e.children), inner)?;
                    out.push(Node::Elem(e));
                }
                other => out.push(other),
            }
        }
        Ok(out)
    }

    fn item(&mut self, id: usize, in_link: bool) -> Result<Vec<Node>, TemplateError> {
        let item = self.items[id].clone();
        match &*item {
            Rich::Picture(picture) => {
                let frame = self.frame(picture)?;
                match self.format {
                    Format::Ods => {
                        self.cell_frames.push(frame);
                        Ok(Vec::new())
                    }
                    Format::Odp => {
                        self.page_frames.push(frame);
                        Ok(Vec::new())
                    }
                    _ => Ok(vec![frame]),
                }
            }
            Rich::Link { url, text } => {
                if in_link {
                    return Err(self.fail("link cannot sit inside an existing hyperlink"));
                }
                Ok(vec![Node::Raw(format!(
                    "<text:a xlink:type=\"simple\" xlink:href=\"{}\">{}</text:a>",
                    esc_attr(url),
                    text_markup(text)
                ))])
            }
            Rich::Color {
                text,
                color,
                background,
                scope,
            } => {
                if self.format == Format::Ods && *scope != Scope::Row {
                    self.cell_color = Some((color.clone(), background.clone()));
                    return Ok(vec![Node::Raw(text_markup(text))]);
                }
                if *scope != Scope::Text {
                    self.effects
                        .push((*scope, color.clone(), background.clone()));
                    return Ok(vec![Node::Raw(text_markup(text))]);
                }
                let mut attrs = Vec::new();
                if let Some(color) = color {
                    attrs.push(("fo:color", format!("#{}", color.to_lowercase())));
                }
                if let Some(background) = background {
                    attrs.push((
                        "fo:background-color",
                        format!("#{}", background.to_lowercase()),
                    ));
                }
                let style = self.book.text_style(attrs);
                Ok(vec![Node::Raw(format!(
                    "<text:span text:style-name=\"{style}\">{}</text:span>",
                    text_markup(text)
                ))])
            }
            Rich::Html(blocks) => {
                let lines: Vec<String> = html::plain_lines(blocks)
                    .iter()
                    .map(|line| esc(line))
                    .collect();
                let markup = lines.join("<text:line-break/>");
                self.pkg.charge(markup.len())?;
                Ok(vec![Node::Raw(markup)])
            }
            Rich::Chart(_) | Rich::PageBreak => Err(self.fail("this value cannot be placed here")),
        }
    }

    fn frame(&mut self, picture: &Picture) -> Result<Node, TemplateError> {
        let images: String = self
            .images(&picture.media)?
            .into_iter()
            .map(|n| match n {
                Node::Raw(s) => s,
                _ => String::new(),
            })
            .collect();
        let style = match picture.fit {
            Some(fit @ (Fit::Contain | Fit::Cover)) if picture.sized => {
                let crop = media::place(
                    (picture.width_pt, picture.height_pt),
                    picture.media.aspect(),
                    fit,
                )
                .crop;
                let name = self.book.derived(
                    "RichG",
                    "graphic",
                    None,
                    &[(
                        "style:graphic-properties",
                        vec![("fo:clip", clip(&picture.media, crop))],
                    )],
                );
                format!(" draw:style-name=\"{name}\"")
            }
            _ => String::new(),
        };
        let n = self.frames;
        self.frames += 1;
        let (w, h) = (picture.width_pt, picture.height_pt);
        let place = match self.format {
            Format::Odt => "text:anchor-type=\"as-char\"".to_string(),
            Format::Ods => "svg:x=\"0pt\" svg:y=\"0pt\"".to_string(),
            _ => format!(
                "svg:x=\"{}\" svg:y=\"{}\"",
                esc_attr(&self.origin.0),
                esc_attr(&self.origin.1)
            ),
        };
        Ok(Node::Raw(format!(
            "<draw:frame draw:name=\"Rich{n}\"{style} {place} svg:width=\"{w:.2}pt\" \
             svg:height=\"{h:.2}pt\">{images}</draw:frame>"
        )))
    }

    // --- HTML ---------------------------------------------------------------

    fn blocks(
        &mut self,
        blocks: &[Block],
        para_style: Option<&str>,
        tables: bool,
        out: &mut String,
    ) -> Result<(), TemplateError> {
        let style_attr = para_style
            .map(|s| format!(" text:style-name=\"{}\"", esc_attr(s)))
            .unwrap_or_default();
        for block in blocks {
            let start = out.len();
            match block {
                Block::Para(runs) => {
                    let body = self.runs(runs, None);
                    let _ = write!(out, "<text:p{style_attr}>{body}</text:p>");
                }
                Block::Heading(n, runs) => {
                    let size = HEADING_SIZES[usize::from(n - 1).min(5)];
                    let body = self.runs(runs, Some(size));
                    let _ = write!(
                        out,
                        "<text:h{style_attr} text:outline-level=\"{n}\">{body}</text:h>"
                    );
                }
                Block::List { ordered, items } => {
                    let style = self.book.list_style(*ordered);
                    let _ = write!(out, "<text:list text:style-name=\"{style}\">");
                    for item in items {
                        out.push_str("<text:list-item>");
                        // A list item holds paragraphs and lists, no tables.
                        let mut inner = String::new();
                        self.blocks(item, para_style, false, &mut inner)?;
                        if inner.is_empty() {
                            let _ = write!(inner, "<text:p{style_attr}/>");
                        }
                        out.push_str(&inner);
                        out.push_str("</text:list-item>");
                    }
                    out.push_str("</text:list>");
                }
                Block::Table(rows) if tables => self.table(rows, para_style, out)?,
                Block::Table(rows) => {
                    for line in html::plain_lines(&[Block::Table(rows.clone())]) {
                        let _ = write!(out, "<text:p{style_attr}>{}</text:p>", esc(&line));
                    }
                    self.pkg.charge(out.len() - start)?;
                }
            }
            // What lists and tables hold was counted when it was written.
            if matches!(block, Block::Para(_) | Block::Heading(..)) {
                self.pkg.charge(out.len() - start)?;
            }
        }
        Ok(())
    }

    fn table(
        &mut self,
        rows: &[Vec<html::Cell>],
        para_style: Option<&str>,
        out: &mut String,
    ) -> Result<(), TemplateError> {
        let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
        if columns == 0 {
            return Ok(());
        }
        let border = self.book.derived(
            "RichC",
            "table-cell",
            None,
            &[(
                "style:table-cell-properties",
                vec![
                    ("fo:border", "0.5pt solid #000000".to_string()),
                    ("fo:padding", "2pt".to_string()),
                ],
            )],
        );
        let _ = write!(
            out,
            "<table:table><table:table-column table:number-columns-repeated=\"{columns}\"/>"
        );
        for row in rows {
            out.push_str("<table:table-row>");
            for i in 0..columns {
                let _ = write!(
                    out,
                    "<table:table-cell table:style-name=\"{border}\" office:value-type=\"string\">"
                );
                self.pkg.charge(CELL_MARKUP)?;
                let mut inner = String::new();
                if let Some(cell) = row.get(i) {
                    if cell.header {
                        let bold = emboldened(&cell.blocks);
                        self.blocks(&bold, para_style, true, &mut inner)?;
                    } else {
                        self.blocks(&cell.blocks, para_style, true, &mut inner)?;
                    }
                }
                if inner.is_empty() {
                    inner.push_str("<text:p/>");
                }
                out.push_str(&inner);
                out.push_str("</table:table-cell>");
            }
            out.push_str("</table:table-row>");
        }
        out.push_str("</table:table>");
        Ok(())
    }

    fn runs(&mut self, runs: &[Inline], heading_size: Option<u32>) -> String {
        let mut out = String::new();
        for run in runs {
            match run {
                Inline::Break => out.push_str("<text:line-break/>"),
                Inline::Text(text, style) => {
                    let mut body = esc(text);
                    let attrs = style_attrs(style, heading_size);
                    if !attrs.is_empty() {
                        let name = self.book.text_style(attrs);
                        body = format!("<text:span text:style-name=\"{name}\">{body}</text:span>");
                    }
                    if let Some(url) = &style.href {
                        body = format!(
                            "<text:a xlink:type=\"simple\" xlink:href=\"{}\">{body}</text:a>",
                            esc_attr(url)
                        );
                    }
                    out.push_str(&body);
                }
            }
        }
        out
    }
}

fn emboldened(blocks: &[Block]) -> Vec<Block> {
    let bold_runs = |runs: &[Inline]| -> Vec<Inline> {
        runs.iter()
            .map(|r| match r {
                Inline::Text(t, s) => {
                    let mut s = s.clone();
                    s.bold = true;
                    Inline::Text(t.clone(), s)
                }
                Inline::Break => Inline::Break,
            })
            .collect()
    };
    blocks
        .iter()
        .map(|b| match b {
            Block::Para(runs) => Block::Para(bold_runs(runs)),
            other => other.clone(),
        })
        .collect()
}

fn style_attrs(style: &Style, heading_size: Option<u32>) -> Vec<(&'static str, String)> {
    let mut attrs = Vec::new();
    if style.bold || heading_size.is_some() {
        attrs.push(("fo:font-weight", "bold".to_string()));
    }
    if let Some(size) = heading_size {
        attrs.push(("fo:font-size", format!("{size}pt")));
    }
    if style.italic {
        attrs.push(("fo:font-style", "italic".to_string()));
    }
    if style.underline {
        attrs.push(("style:text-underline-style", "solid".to_string()));
        attrs.push(("style:text-underline-width", "auto".to_string()));
        attrs.push(("style:text-underline-color", "font-color".to_string()));
    }
    if style.strike {
        attrs.push(("style:text-line-through-style", "solid".to_string()));
        attrs.push(("style:text-line-through-type", "single".to_string()));
    }
    if style.sub {
        attrs.push(("style:text-position", "sub 58%".to_string()));
    }
    if style.sup {
        attrs.push(("style:text-position", "super 58%".to_string()));
    }
    if let Some(color) = &style.color {
        attrs.push(("fo:color", format!("#{}", color.to_lowercase())));
    }
    attrs
}

/// A piece of a paragraph cut at its block markers.
enum Cut {
    Nodes(Vec<Node>),
    Mark(usize),
}
