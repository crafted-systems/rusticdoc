//! XLSX: markers resolve at cell level. Colours become new cell formats,
//! links become `hyperlinks` entries, and pictures and charts are anchored at
//! the cell in a drawing part.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use crate::package;
use crate::rich::pkg::{
    relative, Pkg, CHART_CT, OFFICE_RELS, REL_CHART, REL_DRAWING, REL_HYPERLINK, REL_IMAGE,
    REL_STYLES, REL_WORKSHEET,
};
use crate::rich::{
    emu, ensure_ns, esc, has_mark, html, insert_ordered, max_id, misplaced, segments, text_node,
    Rich, Seg,
};
use crate::sheet::{col_to_num, split_ref};
use crate::xml::{self, Elem, Node};
use crate::TemplateError;

const WORKSHEET_ORDER: &[&str] = &[
    "sheetPr",
    "dimension",
    "sheetViews",
    "sheetFormatPr",
    "cols",
    "sheetData",
    "sheetCalcPr",
    "sheetProtection",
    "protectedRanges",
    "scenarios",
    "autoFilter",
    "sortState",
    "dataConsolidate",
    "customSheetViews",
    "mergeCells",
    "phoneticPr",
    "conditionalFormatting",
    "dataValidations",
    "hyperlinks",
    "printOptions",
    "pageMargins",
    "pageSetup",
    "headerFooter",
    "rowBreaks",
    "colBreaks",
    "customProperties",
    "cellWatches",
    "ignoredErrors",
    "smartTags",
    "drawing",
    "drawingHF",
    "picture",
    "oleObjects",
    "controls",
    "webPublishItems",
    "tableParts",
    "extLst",
];

const STYLES_ORDER: &[&str] = &[
    "numFmts",
    "fonts",
    "fills",
    "borders",
    "cellStyleXfs",
    "cellXfs",
    "cellStyles",
    "dxfs",
    "tableStyles",
    "colors",
    "extLst",
];

const FONT_ORDER: &[&str] = &[
    "b",
    "i",
    "strike",
    "condense",
    "extend",
    "outline",
    "shadow",
    "u",
    "vertAlign",
    "sz",
    "color",
    "name",
    "family",
    "charset",
    "scheme",
];

const STYLES_CT: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml";
const SHEET_CT: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml";
const DRAWING_CT: &str = "application/vnd.openxmlformats-officedocument.drawing+xml";
const XDR_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
const A_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const C_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
const MAIN_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";

/// What a cell asks of `xl/styles.xml`.
#[derive(Clone, PartialEq, Eq, Hash, Default)]
struct CellFormat {
    color: Option<String>,
    background: Option<String>,
    wrap: bool,
}

impl CellFormat {
    fn is_empty(&self) -> bool {
        self.color.is_none() && self.background.is_none() && !self.wrap
    }
}

struct Styles {
    name: String,
    nodes: Vec<Node>,
    variants: HashMap<(u32, CellFormat), u32>,
}

/// State for a whole workbook: its styles and the charts' hidden data sheet.
#[derive(Default)]
pub struct Book {
    styles: Option<Styles>,
    chart_rows: String,
    next_row: u32,
    data_sheet: Option<String>,
}

fn section<'a>(root: &'a mut Elem, name: &str, default: &str) -> &'a mut Elem {
    if root.child(name).is_none() {
        let nodes = xml::parse(default).unwrap_or_default();
        if let Some(Node::Elem(elem)) = nodes.into_iter().next() {
            insert_ordered(root, elem, STYLES_ORDER);
        }
    }
    root.child_mut(name).expect("the section was just added")
}

/// Appends to a counted list and returns the new element's index.
fn append(list: &mut Elem, item: Elem) -> u32 {
    list.make_open();
    list.children.push(Node::Elem(item));
    let count = list
        .children
        .iter()
        .filter(|n| matches!(n, Node::Elem(_)))
        .count() as u32;
    list.set_attr("count", &count.to_string());
    count - 1
}

fn nth<'a>(list: &'a Elem, tag: &'a str, n: u32) -> Option<&'a Elem> {
    list.children_named(tag).nth(n as usize)
}

impl Styles {
    fn load(pkg: &mut Pkg) -> Result<Styles, TemplateError> {
        let found = package::relationships(pkg.entries, "xl/_rels/workbook.xml.rels", "xl")
            .into_iter()
            .find(|(_, ty, target)| ty == REL_STYLES && package::get(pkg.entries, target).is_some())
            .map(|(_, _, target)| target);
        let (name, src) = match found {
            Some(name) => {
                let src = package::text_of(pkg.entries, &name)
                    .ok_or_else(|| TemplateError::new(format!("{name} is not text")))?;
                (name, src)
            }
            None => {
                let name = "xl/styles.xml".to_string();
                pkg.override_type(&name, STYLES_CT)?;
                pkg.rel_to("xl/workbook.xml", REL_STYLES, &name)?;
                let src = format!("{DECL}<styleSheet xmlns=\"{MAIN_NS}\"/>");
                (name, src)
            }
        };
        let nodes = xml::parse(&src)
            .map_err(|e| TemplateError::new(format!("{name}: malformed XML: {e}")))?;
        Ok(Styles {
            name,
            nodes,
            variants: HashMap::new(),
        })
    }

    /// The format index of `base` with this asked of it, created once.
    fn variant(&mut self, base: u32, format: &CellFormat) -> u32 {
        let key = (base, format.clone());
        if let Some(found) = self.variants.get(&key) {
            return *found;
        }
        let Some(root) = xml::root_mut(&mut self.nodes) else {
            return base;
        };
        root.make_open();
        // The workbook may have had no styles at all.
        let fonts = section(
            root,
            "fonts",
            "<fonts count=\"1\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font></fonts>",
        );
        let _ = fonts;
        let _ = section(
            root,
            "fills",
            "<fills count=\"2\"><fill><patternFill patternType=\"none\"/></fill><fill>\
             <patternFill patternType=\"gray125\"/></fill></fills>",
        );
        let _ = section(
            root,
            "borders",
            "<borders count=\"1\"><border><left/><right/><top/><bottom/><diagonal/></border>\
             </borders>",
        );
        let _ = section(
            root,
            "cellStyleXfs",
            "<cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" \
             borderId=\"0\"/></cellStyleXfs>",
        );
        let xfs = section(
            root,
            "cellXfs",
            "<cellXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" \
             xfId=\"0\"/></cellXfs>",
        );
        let mut xf = nth(xfs, "xf", base).cloned().unwrap_or_else(|| {
            Elem::new(
                "xf",
                &[
                    ("numFmtId", "0"),
                    ("fontId", "0"),
                    ("fillId", "0"),
                    ("borderId", "0"),
                    ("xfId", "0"),
                ],
                true,
            )
        });
        if let Some(color) = &format.color {
            let fonts = section(root, "fonts", "<fonts count=\"0\"/>");
            let base_font = xf.attr("fontId").and_then(|f| f.parse().ok()).unwrap_or(0);
            let mut font = nth(fonts, "font", base_font)
                .cloned()
                .unwrap_or_else(|| Elem::new("font", &[], false));
            let rgb = format!("FF{color}");
            insert_ordered(
                &mut font,
                Elem::new("color", &[("rgb", &rgb)], true),
                FONT_ORDER,
            );
            let id = append(fonts, font);
            xf.set_attr("fontId", &id.to_string());
            xf.set_attr("applyFont", "1");
        }
        if let Some(background) = &format.background {
            let fills = section(root, "fills", "<fills count=\"0\"/>");
            let src = format!(
                "<fill><patternFill patternType=\"solid\"><fgColor rgb=\"FF{background}\"/>\
                 <bgColor indexed=\"64\"/></patternFill></fill>"
            );
            let fill = xml::parse(&src).ok().and_then(|mut n| match n.pop() {
                Some(Node::Elem(e)) => Some(e),
                _ => None,
            });
            if let Some(fill) = fill {
                let id = append(fills, fill);
                xf.set_attr("fillId", &id.to_string());
                xf.set_attr("applyFill", "1");
            }
        }
        if format.wrap {
            xf.make_open();
            match xf.child_mut("alignment") {
                Some(alignment) => alignment.set_attr("wrapText", "1"),
                None => xf.children.insert(
                    0,
                    Node::Elem(Elem::new("alignment", &[("wrapText", "1")], true)),
                ),
            }
            xf.set_attr("applyAlignment", "1");
        }
        let xfs = section(root, "cellXfs", "<cellXfs count=\"0\"/>");
        let id = append(xfs, xf);
        self.variants.insert(key, id);
        id
    }
}

impl Book {
    fn format_cell(
        &mut self,
        pkg: &mut Pkg,
        base: u32,
        format: &CellFormat,
    ) -> Result<u32, TemplateError> {
        if self.styles.is_none() {
            self.styles = Some(Styles::load(pkg)?);
        }
        Ok(self
            .styles
            .as_mut()
            .map_or(base, |styles| styles.variant(base, format)))
    }

    /// Writes the chart's values to the hidden data sheet and returns the
    /// chart part's XML, whose references point at them. Excel reads a chart
    /// from its cells rather than its caches, so in a workbook they must exist.
    fn chart_xml(&mut self, pkg: &Pkg, chart: &crate::rich::Chart) -> String {
        if self.data_sheet.is_none() {
            let workbook = package::text_of(pkg.entries, "xl/workbook.xml").unwrap_or_default();
            let mut name = "RichChartData".to_string();
            let mut n = 1;
            while workbook.contains(&format!("name=\"{name}\"")) {
                n += 1;
                name = format!("RichChartData{n}");
            }
            self.data_sheet = Some(name);
            self.next_row = 1;
        }
        let first = self.next_row;
        self.chart_rows.push_str(&chart.sheet_rows(first));
        self.next_row += chart.row_count();
        let sheet = self.data_sheet.clone().unwrap_or_default();
        chart.xml(&format!("'{sheet}'"), first)
    }

    /// Adds the hidden data sheet, and writes the styles, once all sheets are
    /// done.
    pub fn finish(self, pkg: &mut Pkg) -> Result<(), TemplateError> {
        if let Some(styles) = self.styles {
            package::set(
                pkg.entries,
                &styles.name,
                xml::to_xml(&styles.nodes).into_bytes(),
            );
        }
        let Some(name) = self.data_sheet else {
            return Ok(());
        };
        let data = format!(
            "{DECL}<worksheet xmlns=\"{MAIN_NS}\"><sheetData>{}</sheetData></worksheet>",
            self.chart_rows
        );
        let part = pkg.new_part("xl/worksheets", "richdata", data, SHEET_CT)?;
        let rid = pkg.rel_to("xl/workbook.xml", REL_WORKSHEET, &part)?;
        let src = package::text_of(pkg.entries, "xl/workbook.xml")
            .ok_or_else(|| TemplateError::new("the package has no xl/workbook.xml"))?;
        let mut nodes = xml::parse(&src)
            .map_err(|e| TemplateError::new(format!("xl/workbook.xml: malformed XML: {e}")))?;
        let Some(root) = xml::root_mut(&mut nodes) else {
            return Ok(());
        };
        ensure_ns(root, &[("r", OFFICE_RELS)]);
        let sheets = root
            .find_mut("sheets")
            .ok_or_else(|| TemplateError::new("xl/workbook.xml has no sheets"))?;
        let next_id = sheets
            .children_named("sheet")
            .filter_map(|s| s.attr("sheetId")?.parse::<u32>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        let elem = Elem::new(
            "sheet",
            &[
                ("name", name.as_str()),
                ("sheetId", next_id.to_string().as_str()),
                ("state", "hidden"),
                ("r:id", rid.as_str()),
            ],
            true,
        );
        sheets.make_open();
        sheets.children.push(Node::Elem(elem));
        package::set(
            pkg.entries,
            "xl/workbook.xml",
            xml::to_xml(&nodes).into_bytes(),
        );
        Ok(())
    }
}

/// A picture or chart waiting for its place in the sheet's drawing.
enum Anchored<'a> {
    Picture(&'a crate::rich::Picture),
    Chart(&'a crate::rich::Chart),
}

struct Placement {
    item: Arc<Rich>,
    col: u32,
    row: u32,
    y_offset: i64,
}

pub fn rewrite(
    pkg: &mut Pkg,
    book: &mut Book,
    items: &[Arc<Rich>],
    part: &str,
    nodes: &mut [Node],
) -> Result<(), TemplateError> {
    let Some(root) = xml::root_mut(nodes) else {
        return Ok(());
    };
    ensure_ns(root, &[("r", OFFICE_RELS)]);
    let mut placements: Vec<Placement> = Vec::new();
    let mut links: Vec<(String, String)> = Vec::new();
    let Some(data) = root.find_mut("sheetData") else {
        return Ok(());
    };
    for row in data.children.iter_mut() {
        let Node::Elem(row) = row else { continue };
        for cell in row.children.iter_mut() {
            let Node::Elem(c) = cell else { continue };
            if c.name == "c" && has_mark(&c.children) {
                resolve_cell(pkg, book, items, part, c, &mut placements, &mut links)?;
            }
        }
    }
    if !links.is_empty() {
        if root.child("hyperlinks").is_none() {
            insert_ordered(root, Elem::new("hyperlinks", &[], false), WORKSHEET_ORDER);
        }
        if let Some(list) = root.child_mut("hyperlinks") {
            list.make_open();
            for (at, rid) in &links {
                list.children.push(Node::Elem(Elem::new(
                    "hyperlink",
                    &[("ref", at), ("r:id", rid)],
                    true,
                )));
            }
        }
    }
    if !placements.is_empty() {
        place_all(pkg, book, part, root, &placements)?;
    }
    Ok(())
}

/// Replaces the markers in one cell's text.
fn resolve_cell(
    pkg: &mut Pkg,
    book: &mut Book,
    items: &[Arc<Rich>],
    part: &str,
    c: &mut Elem,
    placements: &mut Vec<Placement>,
    links: &mut Vec<(String, String)>,
) -> Result<(), TemplateError> {
    let at = c.attr("r").unwrap_or_default();
    let (col, row) = split_ref(&at)
        .map(|(col, row)| (col_to_num(&col).saturating_sub(1), row.saturating_sub(1)))
        .ok_or_else(|| misplaced(part, &format!("cell {at}"), "the cell has no reference"))?;
    let mut format = CellFormat::default();
    let mut y_offset = 0;
    let mut empty = true;
    let mut todo = vec![&mut c.children];
    // Every text node of the cell: inline strings and rich runs alike.
    while let Some(children) = todo.pop() {
        for node in children.iter_mut() {
            match node {
                Node::Elem(e) => todo.push(&mut e.children),
                Node::Text(t) if t.raw.contains(crate::rich::MARK_OPEN) => {
                    let mut out = String::new();
                    let segs =
                        segments(&t.raw).map_err(|m| misplaced(part, &format!("cell {at}"), m))?;
                    for seg in segs {
                        let id = match seg {
                            Seg::Text(s) => {
                                out.push_str(s);
                                continue;
                            }
                            Seg::Mark(id) => id,
                        };
                        let item = items[id].clone();
                        match &*item {
                            Rich::Picture(p) => {
                                let height = p.height_pt;
                                placements.push(Placement {
                                    item: item.clone(),
                                    col,
                                    row,
                                    y_offset,
                                });
                                y_offset += emu(height);
                            }
                            Rich::Chart(chart) => {
                                let height = chart.height_pt;
                                placements.push(Placement {
                                    item: item.clone(),
                                    col,
                                    row,
                                    y_offset,
                                });
                                y_offset += emu(height);
                            }
                            Rich::Link { url, text } => {
                                let rid = pkg.rel(part, REL_HYPERLINK, url, true)?;
                                links.push((at.clone(), rid));
                                out.push_str(&esc(text));
                            }
                            Rich::Color {
                                text,
                                color,
                                background,
                            } => {
                                format.color.clone_from(color);
                                format.background.clone_from(background);
                                out.push_str(&esc(text));
                            }
                            Rich::Html(blocks) => {
                                out.push_str(&esc(&html::plain_lines(blocks).join("\n")));
                                format.wrap = true;
                            }
                            Rich::PageBreak => {
                                return Err(misplaced(
                                    part,
                                    &format!("cell {at}"),
                                    "a sheet has no page breaks",
                                ))
                            }
                        }
                    }
                    if !out.is_empty() {
                        empty = false;
                    }
                    *node = text_node(out);
                }
                Node::Text(t) if !t.raw.is_empty() => empty = false,
                _ => {}
            }
        }
    }
    if empty {
        c.remove_attr("t");
        c.children.clear();
        c.self_closing = true;
        c.open = format!("{}/>", c.open.trim_end_matches('>'));
    }
    if !format.is_empty() {
        let base = c.attr("s").and_then(|s| s.parse().ok()).unwrap_or(0);
        let variant = book.format_cell(pkg, base, &format)?;
        c.set_attr("s", &variant.to_string());
    }
    Ok(())
}

/// Adds the anchors to the sheet's drawing, creating it when absent.
fn place_all(
    pkg: &mut Pkg,
    book: &mut Book,
    part: &str,
    root: &mut Elem,
    placements: &[Placement],
) -> Result<(), TemplateError> {
    let existing = root.child("drawing").and_then(|d| d.attr("r:id"));
    let dir = part.rsplit_once('/').map_or("", |(d, _)| d);
    let drawing = existing.and_then(|rid| {
        package::relationships(pkg.entries, &package::rels_name(part), dir)
            .into_iter()
            .find(|(id, ty, _)| *id == rid && ty == REL_DRAWING)
            .map(|(_, _, target)| target)
    });
    let drawing = match drawing {
        Some(drawing) => drawing,
        None => {
            let src = format!(
                "{DECL}<xdr:wsDr xmlns:xdr=\"{XDR_NS}\" xmlns:a=\"{A_NS}\" \
                 xmlns:r=\"{OFFICE_RELS}\"/>"
            );
            let drawing = pkg.new_part("xl/drawings", "drawing", src, DRAWING_CT)?;
            let rid = pkg.rel(part, REL_DRAWING, &relative(part, &drawing), false)?;
            insert_ordered(
                root,
                Elem::new("drawing", &[("r:id", &rid)], true),
                WORKSHEET_ORDER,
            );
            drawing
        }
    };
    let src = package::text_of(pkg.entries, &drawing)
        .ok_or_else(|| misplaced(part, "", format!("{drawing} is missing")))?;
    let mut next_id = max_id(&src, "xdr:cNvPr", "id") + 1;
    let mut nodes = xml::parse(&src).map_err(|e| misplaced(&drawing, "", e))?;
    let Some(dr) = xml::root_mut(&mut nodes) else {
        return Err(misplaced(&drawing, "", "the drawing has no root element"));
    };
    ensure_ns(dr, &[("a", A_NS), ("r", OFFICE_RELS), ("c", C_NS)]);
    dr.make_open();
    for placement in placements {
        let anchored = match &*placement.item {
            Rich::Picture(p) => Anchored::Picture(p),
            Rich::Chart(c) => Anchored::Chart(c),
            _ => continue,
        };
        let id = next_id;
        next_id += 1;
        let (body, cx, cy) = match anchored {
            Anchored::Picture(p) => {
                let media = pkg.media_part(&p.media, "xl/media")?;
                let rid = pkg.rel_to(&drawing, REL_IMAGE, &media)?;
                let (cx, cy) = (emu(p.width_pt), emu(p.height_pt));
                (pic_xml(id, &rid, cx, cy), cx, cy)
            }
            Anchored::Chart(chart) => {
                let xml = book.chart_xml(pkg, chart);
                let chart_part = pkg.new_part("xl/charts", "chart", xml, CHART_CT)?;
                let rid = pkg.rel_to(&drawing, REL_CHART, &chart_part)?;
                let (cx, cy) = (emu(chart.width_pt), emu(chart.height_pt));
                (chart_frame_xml(id, &rid), cx, cy)
            }
        };
        let mut anchor = String::new();
        let _ = write!(
            anchor,
            "<xdr:oneCellAnchor><xdr:from><xdr:col>{}</xdr:col><xdr:colOff>0</xdr:colOff>\
             <xdr:row>{}</xdr:row><xdr:rowOff>{}</xdr:rowOff></xdr:from>\
             <xdr:ext cx=\"{cx}\" cy=\"{cy}\"/>{body}<xdr:clientData/></xdr:oneCellAnchor>",
            placement.col, placement.row, placement.y_offset
        );
        dr.children.push(Node::Raw(anchor));
    }
    package::set(pkg.entries, &drawing, xml::to_xml(&nodes).into_bytes());
    Ok(())
}

fn pic_xml(id: u64, rid: &str, cx: i64, cy: i64) -> String {
    format!(
        "<xdr:pic><xdr:nvPicPr><xdr:cNvPr id=\"{id}\" name=\"Picture {id}\"/><xdr:cNvPicPr>\
         <a:picLocks noChangeAspect=\"1\"/></xdr:cNvPicPr></xdr:nvPicPr><xdr:blipFill>\
         <a:blip r:embed=\"{rid}\"/><a:stretch><a:fillRect/></a:stretch></xdr:blipFill><xdr:spPr>\
         <a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
         <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></xdr:spPr></xdr:pic>"
    )
}

fn chart_frame_xml(id: u64, rid: &str) -> String {
    format!(
        "<xdr:graphicFrame macro=\"\"><xdr:nvGraphicFramePr><xdr:cNvPr id=\"{id}\" \
         name=\"Chart {id}\"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm>\
         <a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/></xdr:xfrm><a:graphic>\
         <a:graphicData uri=\"{C_NS}\"><c:chart r:id=\"{rid}\"/></a:graphicData></a:graphic>\
         </xdr:graphicFrame>"
    )
}
