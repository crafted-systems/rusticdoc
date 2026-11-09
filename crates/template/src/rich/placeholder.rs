//! Placeholder pictures: a picture in the template whose alt text holds an
//! `image` or `barcode` tag shows the rendered image instead of its own. The
//! picture keeps its frame, position, wrapping, border and effects; only the
//! image and its crop change.
//!
//! Compiling puts the tag's marker just before the element that carries the
//! alt text (`wp:docPr`, `p:cNvPr`, `xdr:cNvPr`), or in the text of an ODF
//! frame's `svg:title` or `svg:desc`. This module resolves the DrawingML
//! ones; `odf.rs` resolves frames with its styles.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock};

use regex::Regex;

use crate::rich::media::{self, Fit, Media, Placement};
use crate::rich::pkg::{Pkg, REL_IMAGE};
use crate::rich::{has_mark, max_id, misplaced, segments, Picture, Rich, Seg};
use crate::xml::{self, Elem, Node};
use crate::{Format, TemplateError};

static PLACEHOLDER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\|\s*(image|barcode)\b").unwrap());

/// Whether a tag's expression prints a picture, which alt text may hold.
pub fn is_placeholder_tag(expr: &str) -> bool {
    PLACEHOLDER_RE.is_match(expr)
}

/// The URI of the extension that carries an SVG beside its PNG.
pub const SVG_EXT: &str = "{96DAC541-7B7A-43D3-8B79-37D633B846F1}";
pub const SVG_NS: &str = "http://schemas.microsoft.com/office/drawing/2016/SVG/main";

/// The `a:extLst` markup that points a blip at its SVG.
pub fn svg_ext(rid: &str) -> String {
    format!(
        "<a:extLst><a:ext uri=\"{SVG_EXT}\"><asvg:svgBlip xmlns:asvg=\"{SVG_NS}\" \
         r:embed=\"{rid}\"/></a:ext></a:extLst>"
    )
}

/// `a:srcRect` for a placement, empty when nothing is cut.
pub fn src_rect(crop: [f64; 4]) -> String {
    if crop.iter().all(|c| c.abs() < 1e-9) {
        return String::new();
    }
    let n = |c: f64| (c * 100_000.0).round() as i64;
    format!(
        "<a:srcRect l=\"{}\" t=\"{}\" r=\"{}\" b=\"{}\"/>",
        n(crop[0]),
        n(crop[1]),
        n(crop[2]),
        n(crop[3])
    )
}

/// The `a:blip` for media, with its SVG extension when it has one. The
/// media parts go in `dir` and are related from `part`.
pub fn blip(pkg: &mut Pkg, part: &str, dir: &str, media: &Media) -> Result<String, TemplateError> {
    let shown = media.fallback.as_deref().unwrap_or(media);
    let target = pkg.media_part(shown, dir)?;
    let rid = pkg.rel_to(part, REL_IMAGE, &target)?;
    if media.fallback.is_none() {
        return Ok(format!("<a:blip r:embed=\"{rid}\"/>"));
    }
    let svg = pkg.media_part(media, dir)?;
    let svg_rid = pkg.rel_to(part, REL_IMAGE, &svg)?;
    Ok(format!(
        "<a:blip r:embed=\"{rid}\">{}</a:blip>",
        svg_ext(&svg_rid)
    ))
}

/// The marker ids in the text children of `e`, removing them.
pub fn take_markers(e: &mut Elem, part: &str, place: &str) -> Result<Vec<usize>, TemplateError> {
    let mut ids = Vec::new();
    for node in e.children.iter_mut() {
        let Node::Text(t) = node else { continue };
        if !t.raw.contains(super::MARK_OPEN) {
            continue;
        }
        let mut rest = String::new();
        for seg in segments(&t.raw).map_err(|m| misplaced(part, place, m))? {
            match seg {
                Seg::Text(s) => rest.push_str(s),
                Seg::Mark(id) => ids.push(id),
            }
        }
        t.raw = rest;
    }
    Ok(ids)
}

/// The one picture a placeholder's alt text asked for.
pub fn picture<'a>(
    items: &'a [Arc<Rich>],
    ids: &[usize],
    part: &str,
    place: &str,
) -> Result<&'a Picture, TemplateError> {
    if ids.len() > 1 {
        return Err(misplaced(
            part,
            place,
            "a picture's alt text holds more than one image or barcode tag",
        ));
    }
    let Some(Rich::Picture(p)) = ids.first().map(|id| &*items[*id]) else {
        return Err(misplaced(
            part,
            place,
            "a picture's alt text can hold only an image or barcode tag",
        ));
    };
    if p.sized {
        return Err(misplaced(
            part,
            place,
            "a placeholder picture takes its size from its frame: drop width, height and \
             size, and choose how the image sits with fit=",
        ));
    }
    Ok(p)
}

struct Ctx<'a, 'b> {
    pkg: &'a mut Pkg<'b>,
    format: Format,
    items: &'a [Arc<Rich>],
    part: &'a str,
    dir: &'static str,
    /// `wp:docPr` ids seen, so copies made by a loop get their own.
    ids: HashSet<u64>,
    next_id: u64,
    changed: bool,
}

/// Resolves the placeholders of a DrawingML part. Returns whether anything
/// changed.
pub fn rewrite(
    pkg: &mut Pkg,
    format: Format,
    items: &[Arc<Rich>],
    part: &str,
    src: &str,
    nodes: &mut [Node],
) -> Result<bool, TemplateError> {
    let dir = match format {
        Format::Docx => "word/media",
        Format::Pptx => "ppt/media",
        Format::Xlsx => "xl/media",
        _ => return Ok(false),
    };
    let mut ctx = Ctx {
        pkg,
        format,
        items,
        part,
        dir,
        ids: HashSet::new(),
        next_id: max_id(src, "wp:docPr", "id").max(max_id(src, "pic:cNvPr", "id")) + 1,
        changed: false,
    };
    ctx.walk(nodes)?;
    Ok(ctx.changed)
}

/// The element that names the picture and carries its alt text, as the
/// object's child (DOCX) or the child of its first child (the others).
fn carrier_name(object: &Elem) -> Option<String> {
    let find = |e: &Elem| {
        e.children.iter().find_map(|n| match n {
            Node::Elem(c) if c.name.ends_with(":docPr") || c.name.ends_with(":cNvPr") => {
                c.attr("name")
            }
            _ => None,
        })
    };
    find(object).or_else(|| {
        object.children.iter().find_map(|n| match n {
            Node::Elem(c) if c.name.contains(":nv") => find(c),
            _ => None,
        })
    })
}

impl Ctx<'_, '_> {
    fn walk(&mut self, nodes: &mut [Node]) -> Result<(), TemplateError> {
        for node in nodes.iter_mut() {
            let Node::Elem(e) = node else { continue };
            if !has_mark(&e.children) {
                continue;
            }
            match (self.format, e.name.as_str()) {
                (Format::Docx, "wp:inline" | "wp:anchor") => self.docx(e)?,
                (Format::Pptx, "p:pic" | "p:sp" | "p:graphicFrame" | "p:cxnSp") => {
                    self.object(e)?;
                }
                (
                    Format::Xlsx,
                    "xdr:twoCellAnchor" | "xdr:oneCellAnchor" | "xdr:absoluteAnchor",
                ) => self.anchor(e)?,
                _ => {}
            }
            self.walk(&mut e.children)?;
        }
        Ok(())
    }

    fn place(object: &Elem) -> String {
        match carrier_name(object) {
            Some(name) => format!("picture {name:?}"),
            None => "picture".to_string(),
        }
    }

    /// A Word drawing: the marker sits among `wp:inline`'s own children.
    fn docx(&mut self, drawing: &mut Elem) -> Result<(), TemplateError> {
        let place = Self::place(drawing);
        let ids = take_markers(drawing, self.part, &place)?;
        if ids.is_empty() {
            return Ok(());
        }
        let picture = picture(self.items, &ids, self.part, &place)?;
        let Some(pic) = drawing.find_mut("pic:pic") else {
            return Err(misplaced(
                self.part,
                &place,
                "only a picture can be a placeholder; this drawing is a shape or chart",
            ));
        };
        let new_cy = self.picture(pic, picture, &place)?;
        if let Some(cy) = new_cy {
            if let Some(extent) = drawing.child_mut("wp:extent") {
                extent.set_attr("cy", &cy.to_string());
            }
        }
        // Copies made by a loop share the template's ids; Word wants each
        // drawing's to be its own.
        let id = drawing
            .child("wp:docPr")
            .and_then(|e| e.attr("id"))
            .and_then(|i| i.parse::<u64>().ok());
        if let Some(id) = id {
            if !self.ids.insert(id) {
                let fresh = self.next_id.to_string();
                self.next_id += 1;
                for name in ["wp:docPr", "pic:cNvPr"] {
                    if let Some(e) = drawing.find_mut(name) {
                        e.set_attr("id", &fresh);
                    }
                }
            }
        }
        self.changed = true;
        Ok(())
    }

    /// A sheet's anchor: the picture is its object child.
    fn anchor(&mut self, anchor: &mut Elem) -> Result<(), TemplateError> {
        let mut cy = None;
        for node in anchor.children.iter_mut() {
            let Node::Elem(object) = node else { continue };
            if object.name.starts_with("xdr:") && has_mark(&object.children) {
                cy = self.object(object)?;
            }
        }
        let Some(cy) = cy else { return Ok(()) };
        if anchor.name == "xdr:twoCellAnchor" {
            // The far corner would need the rows' heights; a one-cell
            // anchor takes the size itself.
            let cx = anchor
                .find("a:ext")
                .and_then(|e| e.attr("cx"))
                .unwrap_or_else(|| "0".to_string());
            anchor
                .children
                .retain(|n| !matches!(n, Node::Elem(e) if e.name == "xdr:to"));
            let at = anchor
                .children
                .iter()
                .position(|n| matches!(n, Node::Elem(e) if e.name == "xdr:from"))
                .map_or(0, |i| i + 1);
            anchor.children.insert(
                at,
                Node::Elem(Elem::new(
                    "xdr:ext",
                    &[("cx", &cx), ("cy", &cy.to_string())],
                    true,
                )),
            );
            anchor.remove_attr("editAs");
            let attrs = anchor.open["<xdr:twoCellAnchor".len()..].to_string();
            anchor.name = "xdr:oneCellAnchor".to_string();
            anchor.open = format!("<xdr:oneCellAnchor{attrs}");
        } else if let Some(ext) = anchor.child_mut("xdr:ext") {
            ext.set_attr("cy", &cy.to_string());
        }
        Ok(())
    }

    /// A PowerPoint or sheet object: the marker sits in its `nv…Pr` child.
    /// Returns the new frame height when the fit changed it.
    fn object(&mut self, object: &mut Elem) -> Result<Option<i64>, TemplateError> {
        let place = Self::place(object);
        let mut ids = Vec::new();
        for node in object.children.iter_mut() {
            if let Node::Elem(nv) = node {
                if nv.name.contains(":nv") {
                    ids.extend(take_markers(nv, self.part, &place)?);
                }
            }
        }
        if ids.is_empty() {
            return Ok(None);
        }
        let picture = picture(self.items, &ids, self.part, &place)?;
        if !object.name.ends_with(":pic") {
            return Err(misplaced(
                self.part,
                &place,
                "only a picture can be a placeholder; this is a shape, chart or table",
            ));
        }
        self.changed = true;
        self.picture(object, picture, &place)
    }

    /// Swaps the image of a `pic:pic`, `p:pic` or `xdr:pic` and sets its
    /// crop. Returns the new height in EMU when the fit changed it.
    fn picture(
        &mut self,
        pic: &mut Elem,
        picture: &Picture,
        place: &str,
    ) -> Result<Option<i64>, TemplateError> {
        let prefix = pic.name.split(':').next().unwrap_or("pic").to_string();
        let fill_name = format!("{prefix}:blipFill");
        let sppr_name = format!("{prefix}:spPr");
        let (cx, cy) = pic
            .child(&sppr_name)
            .and_then(|sp| sp.find("a:ext"))
            .and_then(|ext| {
                Some((
                    ext.attr("cx")?.parse::<i64>().ok()?,
                    ext.attr("cy")?.parse::<i64>().ok()?,
                ))
            })
            .filter(|(cx, cy)| *cx > 0 && *cy > 0)
            .ok_or_else(|| misplaced(self.part, place, "the placeholder picture has no size"))?;
        let fit = picture.fit.unwrap_or(Fit::Contain);
        let Placement { crop, height } =
            media::place((cx as f64, cy as f64), picture.media.aspect(), fit);
        let new_blip = blip(self.pkg, self.part, self.dir, &picture.media)?;
        let fill = pic
            .child_mut(&fill_name)
            .ok_or_else(|| misplaced(self.part, place, "the placeholder picture has no image"))?;
        let mut blip_elem = xml::parse(&new_blip)
            .ok()
            .and_then(|mut n| match n.pop() {
                Some(Node::Elem(e)) => Some(e),
                _ => None,
            })
            .ok_or_else(|| misplaced(self.part, place, "cannot build the picture"))?;
        // Effects on the old image (recolour, transparency, brightness) and
        // its extensions other than an SVG one stay.
        if let Some(old) = fill.child("a:blip") {
            let effects: Vec<Node> = old
                .children
                .iter()
                .filter(|n| matches!(n, Node::Elem(e) if e.name != "a:extLst"))
                .cloned()
                .collect();
            let kept_ext: Vec<Node> = old
                .child("a:extLst")
                .map(|list| {
                    list.children
                        .iter()
                        .filter(|n| {
                            matches!(n, Node::Elem(e) if e.name == "a:ext"
                                && e.attr("uri").as_deref() != Some(SVG_EXT))
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            if !effects.is_empty() || !kept_ext.is_empty() {
                blip_elem.make_open();
                blip_elem.children.splice(0..0, effects);
            }
            if !kept_ext.is_empty() {
                match blip_elem.child_mut("a:extLst") {
                    Some(list) => {
                        list.children.splice(0..0, kept_ext);
                    }
                    None => {
                        let mut list = Elem::new("a:extLst", &[], false);
                        list.children = kept_ext;
                        blip_elem.children.push(Node::Elem(list));
                    }
                }
            }
        }
        let rect = src_rect(crop);
        let mut children: Vec<Node> = vec![Node::Elem(blip_elem)];
        if !rect.is_empty() {
            children.push(Node::Raw(rect));
        }
        children.push(Node::Raw("<a:stretch><a:fillRect/></a:stretch>".into()));
        fill.make_open();
        fill.children = children;
        let Some(height) = height else {
            return Ok(None);
        };
        let new_cy = (height.round() as i64).max(1);
        if let Some(ext) = pic
            .child_mut(&sppr_name)
            .and_then(|sp| sp.find_mut("a:ext"))
        {
            ext.set_attr("cy", &new_cy.to_string());
        }
        Ok(Some(new_cy))
    }
}
