//! `drop()`: removing the element that holds the tag, format by format.
//!
//! The `drop.*` fixtures were written by LibreOffice from the flat sources
//! next to them (see `fixtures/gen.sh`). Each holds a tag per element, on a
//! condition the data sets.

mod common;

use common::*;
use regex::Regex;
use rusticdoc_template::{compile, environment, validate, Format, Severity};
use serde_json::{json, Value};

const DOCX: &[u8] = include_bytes!("fixtures/drop.docx");
const ODT: &[u8] = include_bytes!("fixtures/drop.odt");
const XLSX: &[u8] = include_bytes!("fixtures/drop.xlsx");
const ODS: &[u8] = include_bytes!("fixtures/drop.ods");
const PPTX: &[u8] = include_bytes!("fixtures/drop.pptx");
const ODP: &[u8] = include_bytes!("fixtures/drop.odp");

fn items() -> Value {
    json!([
        {"name": "Beans", "qty": 2, "discount": 1, "amount": 10, "hidden": false},
        {"name": "Hidden item", "qty": 5, "discount": 4, "amount": 50, "hidden": true},
        {"name": "Mugs", "qty": 3, "discount": 2, "amount": 30, "hidden": false}
    ])
}

/// Everything stays, but for the row of the hidden item.
fn keep_all() -> Value {
    json!({
        "note": true, "extra": true, "discount": true, "notes": true, "logo": true,
        "chart": true, "stamp": true, "two": true, "appendix": true, "details": true,
        "items": items(), "total": 40
    })
}

/// Every element with a tag goes.
fn drop_all() -> Value {
    json!({
        "note": false, "extra": false, "discount": false, "notes": false, "logo": false,
        "chart": false, "stamp": false, "two": false, "appendix": false, "details": false,
        "items": items(), "total": 40
    })
}

fn render(format: Format, bytes: &[u8], data: &Value) -> Vec<u8> {
    compile(format, bytes)
        .expect("compiles")
        .render(data)
        .expect("renders")
}

fn names(bytes: &[u8]) -> Vec<String> {
    entries(bytes).into_iter().map(|(n, _)| n).collect()
}

fn has(bytes: &[u8], name: &str) -> bool {
    names(bytes).iter().any(|n| n == name)
}

/// Visible text of an XML part, one string.
fn text_of(xml: &str) -> String {
    let t = Regex::new(r"<[^>]+>").unwrap().replace_all(xml, " ");
    unescape(&t)
}

fn count(re: &str, text: &str) -> usize {
    Regex::new(re).unwrap().find_iter(text).count()
}

/// Rewrites one part of a package.
fn patch(bytes: &[u8], name: &str, from: &str, to: &str) -> Vec<u8> {
    let mut out = entries(bytes);
    let mut hit = false;
    for (n, data) in &mut out {
        if n == name {
            let text = String::from_utf8(data.clone()).unwrap();
            assert!(text.contains(from), "{from} not in {name}");
            *data = text.replacen(from, to, 1).into_bytes();
            hit = true;
        }
    }
    assert!(hit, "no part {name}");
    let refs: Vec<(&str, &[u8])> = out
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();
    zip(&refs)
}

/// Every XML part of the package parses, and no marker or alt-text
/// delimiter is left in it.
fn assert_clean(bytes: &[u8]) {
    for (name, data) in entries(bytes) {
        if !(name.ends_with(".xml") || name.ends_with(".rels")) {
            continue;
        }
        let text = String::from_utf8(data).unwrap();
        assert!(
            !text.chars().any(|c| ('\u{1}'..='\u{8}').contains(&c)),
            "{name} keeps a private character"
        );
        let mut reader = quick_xml::Reader::from_str(&text);
        loop {
            match reader.read_event() {
                Ok(quick_xml::events::Event::Eof) => break,
                Ok(_) => {}
                Err(e) => panic!("{name} is not well formed: {e}"),
            }
        }
    }
    // Every relationship to a part names a part that exists, and every
    // content type override names one too.
    let all = names(bytes);
    let relationship = Regex::new(r#"<Relationship [^>]*>"#).unwrap();
    let target_attr = Regex::new(r#"Target="([^"]*)""#).unwrap();
    for name in all.iter().filter(|n| n.ends_with(".rels")) {
        let rels = part(bytes, name);
        let source = name.replace("_rels/", "");
        let source = source.trim_end_matches(".rels");
        let base = source.rsplit_once('/').map_or("", |(d, _)| d);
        for c in relationship.find_iter(&rels) {
            let tag = c.as_str();
            if tag.contains("External") {
                continue;
            }
            let target = target_attr.captures(tag).unwrap()[1].to_string();
            let mut segs: Vec<&str> = if target.starts_with('/') {
                vec![]
            } else {
                base.split('/').filter(|s| !s.is_empty()).collect()
            };
            for s in target.trim_start_matches('/').split('/') {
                if s == ".." {
                    segs.pop();
                } else {
                    segs.push(s);
                }
            }
            let resolved = segs.join("/");
            assert!(
                all.contains(&resolved),
                "{name} points at missing {resolved}"
            );
        }
    }
    if all.iter().any(|n| n == "[Content_Types].xml") {
        let types = part(bytes, "[Content_Types].xml");
        for c in Regex::new(r#"PartName="/([^"]*)""#)
            .unwrap()
            .captures_iter(&types)
        {
            assert!(
                all.contains(&c[1].to_string()),
                "override for missing {}",
                &c[1]
            );
        }
    }
    if all.iter().any(|n| n == "META-INF/manifest.xml") {
        let manifest = part(bytes, "META-INF/manifest.xml");
        for c in Regex::new(r#"manifest:full-path="([^"]*)""#)
            .unwrap()
            .captures_iter(&manifest)
        {
            let p = &c[1];
            if p == "/" || p.ends_with('/') {
                continue;
            }
            assert!(all.iter().any(|n| n == p), "manifest lists missing {p}");
        }
    }
}

// --- DOCX -------------------------------------------------------------------

#[test]
fn docx_keeps_what_the_conditions_keep() {
    let out = render(Format::Docx, DOCX, &keep_all());
    assert_clean(&out);
    let doc = part(&out, "word/document.xml");
    let text = text_of(&doc);
    for w in [
        "Note paragraph",
        "Extra one",
        "Extra two",
        "Discount",
        "Notes table",
        "Beans",
        "Mugs",
        "Stamp",
    ] {
        assert!(text.contains(w), "{w} missing");
    }
    assert!(!text.contains("Hidden item"), "the hidden item's row goes");
    assert!(has(&out, "word/media/image1.png"));
    assert!(has(&out, "word/charts/chart1.xml"));
    // The alt text keeps its own text and loses the tag.
    assert!(doc.contains("title=\"Company logo\""), "{doc}");
    assert!(!doc.contains("descr="), "the description held only a tag");
}

#[test]
fn docx_drops_every_element() {
    let out = render(Format::Docx, DOCX, &drop_all());
    assert_clean(&out);
    let doc = part(&out, "word/document.xml");
    let text = text_of(&doc);
    for w in [
        "Note paragraph",
        "Extra one",
        "Extra two",
        "Discount",
        "Notes table",
        "Kept only with notes",
        "Hidden item",
        "Stamp",
    ] {
        assert!(!text.contains(w), "{w} should be gone: {text}");
    }
    for w in [
        "Intro paragraph",
        "Between tables",
        "Beans",
        "Mugs",
        "Logo",
        "here",
        "End of document",
    ] {
        assert!(text.contains(w), "{w} missing");
    }
    // The column: the grid and every row lose one.
    assert_eq!(count("<w:gridCol ", &doc), 3, "{doc}");
    assert_eq!(count("<w:tbl>", &doc), 1);
    let rows: Vec<&str> = Regex::new(r"<w:tr[ >]")
        .unwrap()
        .split(&doc)
        .skip(1)
        .collect();
    assert_eq!(rows.len(), 4, "header, two items, total");
    for r in &rows {
        assert_eq!(count("<w:tc>", r), 3, "{r}");
    }
    // The picture and the chart go with their parts and relationships.
    assert!(!doc.contains("<w:drawing>"), "{doc}");
    assert!(!doc.contains("mc:AlternateContent"));
    assert!(!has(&out, "word/media/image1.png"));
    assert!(!has(&out, "word/charts/chart1.xml"));
    let rels = part(&out, "word/_rels/document.xml.rels");
    assert!(
        !rels.contains("image1.png") && !rels.contains("chart1.xml"),
        "{rels}"
    );
    let types = part(&out, "[Content_Types].xml");
    assert!(!types.contains("chart1.xml"), "{types}");
}

// --- ODT --------------------------------------------------------------------

#[test]
fn odt_drops_every_element() {
    let out = render(Format::Odt, ODT, &drop_all());
    assert_clean(&out);
    let content = part(&out, "content.xml");
    let text = text_of(&content);
    for w in [
        "Note paragraph",
        "Extra one",
        "Extra two",
        "Discount",
        "Notes table",
        "Hidden item",
        "Stamp",
    ] {
        assert!(!text.contains(w), "{w} should be gone");
    }
    for w in [
        "Intro paragraph",
        "Between tables",
        "Beans",
        "Mugs",
        "End of document",
    ] {
        assert!(text.contains(w), "{w} missing");
    }
    assert!(!content.contains("<draw:image"));
    assert!(!content.contains("<draw:object"));
    assert!(!content.contains("<draw:custom-shape"));
    assert_eq!(count("<table:table ", &content), 1);
    // LibreOffice wrote the four columns as A twice, A, D: one A goes.
    assert!(
        content.contains("table:style-name=\"Items.A\" table:number-columns-repeated=\"2\"/><table:table-column table:style-name=\"Items.D\"/>"),
        "{content}"
    );
    assert!(!names(&out)
        .iter()
        .any(|n| n.starts_with("Pictures/") || n.starts_with("Object 1")));
    let manifest = part(&out, "META-INF/manifest.xml");
    assert!(
        !manifest.contains("Pictures/") && !manifest.contains("Object 1"),
        "{manifest}"
    );
}

#[test]
fn odt_keeps_what_the_conditions_keep() {
    let out = render(Format::Odt, ODT, &keep_all());
    assert_clean(&out);
    let content = part(&out, "content.xml");
    assert!(content.contains("<draw:image") && content.contains("<draw:object"));
    assert!(content.contains("<svg:title>Company logo</svg:title>"));
    assert!(names(&out).iter().any(|n| n.starts_with("Pictures/")));
}

// --- XLSX -------------------------------------------------------------------

fn formulas(sheet: &str) -> Vec<(String, String)> {
    Regex::new(r#"<c r="([A-Z]+[0-9]+)"[^>]*><f[^>]*>([^<]*)</f>"#)
        .unwrap()
        .captures_iter(sheet)
        .map(|c| (c[1].to_string(), unescape(&c[2])))
        .collect()
}

#[test]
fn xlsx_drops_rows_and_columns_and_moves_formulas() {
    let out = render(Format::Xlsx, XLSX, &drop_all());
    assert_clean(&out);
    let sheet = part(&out, "xl/worksheets/sheet1.xml");
    let cells = cells_resolved(&out);
    let text: Vec<&str> = cells.iter().map(|(_, v)| v.as_str()).collect();
    assert!(
        !text
            .iter()
            .any(|t| t.contains("Discount") || t.contains("Hidden item")),
        "{cells:?}"
    );
    let at = |r: &str| cells.iter().find(|(c, _)| c == r).map(|(_, v)| v.clone());
    assert_eq!(at("C2").as_deref(), Some("Amount"), "{cells:?}");
    assert_eq!(at("A3").as_deref(), Some("Beans"));
    assert_eq!(at("A4").as_deref(), Some("Mugs"));
    assert_eq!(at("A5").as_deref(), Some("Total"));
    let f = formulas(&sheet);
    assert!(f.contains(&("B5".into(), "SUM(B2:B4)".into())), "{f:?}");
    assert!(f.contains(&("C5".into(), "SUM(C2:C4)".into())), "{f:?}");
    assert!(f.contains(&("C6".into(), "C5*2".into())), "{f:?}");
    assert!(
        !sheet.contains("r=\"D"),
        "nothing is left in column D: {sheet}"
    );
    // Another sheet's reference and the print area move too.
    let static_sheet = sheet_part(&out, "Static");
    assert!(static_sheet.contains("Summary!C2"), "{static_sheet}");
    let wb = part(&out, "xl/workbook.xml");
    assert!(wb.contains("Summary!$A$1:$C$6"), "{wb}");
    assert!(wb.contains("fullCalcOnLoad=\"1\""));
}

#[test]
fn xlsx_drops_pictures_charts_shapes_and_sheets() {
    let out = render(Format::Xlsx, XLSX, &drop_all());
    let wb = part(&out, "xl/workbook.xml");
    assert!(!wb.contains("name=\"Details\""), "{wb}");
    assert!(wb.contains("name=\"Summary\"") && wb.contains("name=\"Static\""));
    assert!(!names(&out)
        .iter()
        .filter(|n| n.starts_with("xl/worksheets/sheet"))
        .any(|n| part(&out, n).contains("Details")));
    let drawing = part(&out, "xl/drawings/drawing1.xml");
    assert_eq!(count("Anchor>", &drawing), 0, "{drawing}");
    assert!(!has(&out, "xl/media/image1.png"));
    assert!(
        !names(&out).iter().any(|n| n.starts_with("xl/charts/")),
        "{:?}",
        names(&out)
    );
    let types = part(&out, "[Content_Types].xml");
    assert!(!types.contains("chart1.xml"), "{types}");

    let kept = render(Format::Xlsx, XLSX, &keep_all());
    assert_clean(&kept);
    let drawing = part(&kept, "xl/drawings/drawing1.xml");
    assert_eq!(count("<xdr:twoCellAnchor", &drawing), 3, "{drawing}");
    assert!(
        !drawing.contains("descr="),
        "the alt text held only tags: {drawing}"
    );
    assert!(part(&kept, "xl/workbook.xml").contains("name=\"Details\""));
}

#[test]
fn xlsx_refuses_a_formula_into_a_removed_column() {
    let bytes = patch(
        XLSX,
        "xl/worksheets/sheet1.xml",
        "<f aca=\"false\">D4*2</f>",
        "<f aca=\"false\">C4*2</f>",
    );
    let err = compile(Format::Xlsx, &bytes)
        .unwrap()
        .render(&drop_all())
        .unwrap_err();
    assert!(err.message.contains("removes cell C"), "{err}");
    assert!(err.message.contains("Summary"), "{err}");
}

#[test]
fn xlsx_refuses_dropping_a_sheet_others_point_at() {
    let bytes = patch(XLSX, "xl/worksheets/sheet3.xml", "Summary!D2", "Details!B1");
    let err = compile(Format::Xlsx, &bytes)
        .unwrap()
        .render(&drop_all())
        .unwrap_err();
    assert!(err.message.contains("removes sheet \"Details\""), "{err}");
}

// --- ODS --------------------------------------------------------------------

#[test]
fn ods_drops_rows_columns_objects_and_sheets() {
    let out = render(Format::Ods, ODS, &drop_all());
    assert_clean(&out);
    let content = part(&out, "content.xml");
    let text = text_of(&content);
    for w in ["Discount", "Hidden item", "Details"] {
        assert!(!text.contains(w), "{w} should be gone");
    }
    for w in ["Beans", "Mugs", "Total", "Amount"] {
        assert!(text.contains(w), "{w} missing");
    }
    assert!(content.contains("of:=SUM([.B2:.B4])"), "{content}");
    assert!(content.contains("of:=SUM([.C2:.C4])"), "{content}");
    assert!(content.contains("of:=[.C5]*2"), "{content}");
    assert!(content.contains("[$Summary.C2]"), "{content}");
    assert!(
        !content.contains("<draw:image")
            && !content.contains("<draw:object")
            && !content.contains("draw:custom-shape")
    );
    assert!(!names(&out)
        .iter()
        .any(|n| n.starts_with("Pictures/") || n.starts_with("Object 1")));
}

// --- PPTX -------------------------------------------------------------------

#[test]
fn pptx_drops_every_element() {
    let out = render(Format::Pptx, PPTX, &drop_all());
    assert_clean(&out);
    assert_eq!(slide_order(&out).len(), 2, "the appendix slide goes");
    let slide = part(&out, &slide_order(&out)[0]);
    let text = text_of(&slide);
    for w in [
        "Point two",
        "Discount",
        "Hidden item",
        "Notes table",
        "Stamp",
    ] {
        assert!(!text.contains(w), "{w} should be gone");
    }
    for w in ["Point one", "Point three", "Beans", "Mugs", "Amount"] {
        assert!(text.contains(w), "{w} missing");
    }
    assert!(!slide.contains("<p:pic>"));
    assert_eq!(
        count("<p:graphicFrame>", &slide),
        1,
        "only the items table: {slide}"
    );
    assert_eq!(count("<a:gridCol ", &slide), 3);
    assert!(!has(&out, "ppt/media/image1.png"));
    assert!(!names(&out).iter().any(|n| n.starts_with("ppt/charts/")));
    let all = names(&out);
    assert!(!all.iter().any(|n| n.contains("slide2.xml")
        && n.starts_with("ppt/slides/")
        && part(&out, n).contains("Appendix")));
}

#[test]
fn pptx_keeps_what_the_conditions_keep() {
    let out = render(Format::Pptx, PPTX, &keep_all());
    assert_clean(&out);
    assert_eq!(slide_order(&out).len(), 3);
    let slide = part(&out, &slide_order(&out)[0]);
    assert!(slide.contains("<p:pic>"));
    assert_eq!(count("<p:graphicFrame>", &slide), 3);
}

// --- ODP --------------------------------------------------------------------

#[test]
fn odp_drops_every_element() {
    let out = render(Format::Odp, ODP, &drop_all());
    assert_clean(&out);
    let content = part(&out, "content.xml");
    assert_eq!(count("<draw:page ", &content), 2);
    let text = text_of(&content);
    for w in [
        "Point two",
        "Discount",
        "Hidden item",
        "Notes table",
        "Stamp",
        "Appendix",
    ] {
        assert!(!text.contains(w), "{w} should be gone");
    }
    for w in ["Point one", "Point three", "Beans", "Mugs", "Thanks"] {
        assert!(text.contains(w), "{w} missing");
    }
    // A table frame holds a preview picture, which goes with the table.
    assert!(!content.contains("draw:name=\"Logo\"") && !content.contains("<draw:object"));
    assert!(
        !names(&out)
            .iter()
            .any(|n| (n.starts_with("Pictures/") && n.ends_with(".png"))
                || n.starts_with("Object 1")
                || n.contains("TablePreview2")),
        "{:?}",
        names(&out)
    );
    assert!(names(&out).iter().any(|n| n.contains("TablePreview1")));
}

/// The part of the sheet with this name.
fn sheet_part(bytes: &[u8], name: &str) -> String {
    let wb = part(bytes, "xl/workbook.xml");
    let tag = Regex::new(&format!(r#"<sheet [^>]*name="{name}"[^>]*>"#))
        .unwrap()
        .find(&wb)
        .unwrap_or_else(|| panic!("no sheet {name}"))
        .as_str()
        .to_string();
    let rid = Regex::new(r#"r:id="([^"]*)""#)
        .unwrap()
        .captures(&tag)
        .unwrap()[1]
        .to_string();
    let rels = part(bytes, "xl/_rels/workbook.xml.rels");
    let rel = Regex::new(&format!(r#"<Relationship [^>]*Id="{rid}"[^>]*>"#))
        .unwrap()
        .find(&rels)
        .unwrap()
        .as_str()
        .to_string();
    let target = Regex::new(r#"Target="([^"]*)""#)
        .unwrap()
        .captures(&rel)
        .unwrap()[1]
        .to_string();
    part(bytes, &format!("xl/{}", target.trim_start_matches("/xl/")))
}

// --- Mistakes ----------------------------------------------------------------

fn compile_err(format: Format, bytes: &[u8]) -> rusticdoc_template::TemplateError {
    compile(format, bytes).expect_err("must be refused")
}

#[test]
fn misplaced_drops_are_located_errors_naming_the_tag() {
    let cases: Vec<(Format, Vec<u8>, &str)> = vec![
        (Format::Docx, docx(&p("{{ drop(\"slide\") }}")), "a DOCX has no slides"),
        (Format::Docx, docx(&p("{{ drop(\"row\") }}")), "must sit in a table cell"),
        (Format::Docx, docx(&p("{{ drop(\"picture\") }}")), "alt text"),
        (Format::Docx, docx(&p("{{ drop(\"cell\") }}")), "is not something drop removes"),
        (Format::Pptx, pptx(&[&shape(&["{{ drop(\"sheet\") }}"])]), "a PPTX has no sheets"),
        (Format::Odt, odt("<text:p>{{ drop(\"column\") }}</text:p>"), "must sit in a table cell"),
        (
            Format::Ods,
            ods("<table:table table:name=\"S\"><table:table-row><table:table-cell><text:p>{{ drop(\"paragraph\") }}</text:p></table:table-cell></table:table-row></table:table>"),
            "a sheet has no paragraphs",
        ),
    ];
    for (format, bytes, want) in cases {
        let err = compile_err(format, &bytes);
        assert!(err.message.contains(want), "{format:?}: {err}");
        assert!(err.message.contains("drop("), "the tag is named: {err}");
        let loc = err.location.expect("located");
        assert!(
            !loc.part.is_empty() && loc.snippet.contains("drop("),
            "{loc:?}"
        );
    }
}

#[test]
fn validate_reports_a_misplaced_drop() {
    let diags = validate(Format::Docx, &docx(&p("{{ drop(\"row\") }}")), &json!({}));
    assert!(
        diags
            .iter()
            .any(|d| d.severity == Severity::Error && d.message.contains("table cell")),
        "{diags:?}"
    );
}

#[test]
fn a_name_from_data_is_checked_when_placed() {
    let bytes = docx(&p("Text{{ drop(what) }}"));
    let t = compile(Format::Docx, &bytes).unwrap();
    let err = t.render(&json!({"what": "slide"})).unwrap_err();
    assert!(err.message.contains("a DOCX has no slides"), "{err}");
    let err = t.render(&json!({"what": "row"})).unwrap_err();
    assert!(err.message.contains("must sit in a table cell"), "{err}");
    let out = t.render(&json!({"what": "paragraph"})).unwrap();
    assert!(!part(&out, "word/document.xml").contains("Text"));
}

#[test]
fn bad_arguments_are_refused() {
    let t = |tag: &str| {
        compile(Format::Docx, &docx(&tbl(&[&[tag]])))
            .unwrap()
            .render(&json!({}))
            .unwrap_err()
            .message
    };
    assert!(t("{{ drop(1) }}").contains("name what to remove"));
    assert!(t("{{ drop(\"table\", 2) }}").contains("takes no count"));
    assert!(t("{{ drop(\"paragraph\", 0) }}").contains("whole number"));
}

#[test]
fn a_text_template_refuses_drop() {
    let env = environment();
    let err = env
        .render_str("{{ drop(\"row\") }}", minijinja::context! {})
        .unwrap_err();
    assert!(
        err.to_string().contains("needs an Office template"),
        "{err}"
    );
}

#[test]
fn a_count_removes_the_paragraphs_after_the_tag() {
    let body = [
        p("Keep"),
        p("One{{ drop(\"paragraph\", 3) }}"),
        p("Two"),
        p("Three"),
        p("Last"),
    ]
    .concat();
    let out = render(Format::Docx, &docx(&body), &json!({}));
    assert_eq!(docx_text(&out), vec!["Keep", "Last"]);
    let short = [p("One{{ drop(\"paragraph\", 3) }}"), p("Two")].concat();
    let err = compile(Format::Docx, &docx(&short))
        .unwrap()
        .render(&json!({}))
        .unwrap_err();
    assert!(
        err.message
            .contains("needs 3 paragraphs in a row, but only 2"),
        "{err}"
    );
}

#[test]
fn a_paragraph_dropped_from_a_cell_leaves_the_cell_one_paragraph() {
    let body = tbl(&[&["Only{{ drop(\"paragraph\") }}", "B"]]);
    let out = render(Format::Docx, &docx(&body), &json!({}));
    let doc = part(&out, "word/document.xml");
    let first = doc.split("<w:tc>").nth(1).unwrap();
    assert!(
        first.contains("<w:p/>") && !first.contains("Only"),
        "{first}"
    );
}

#[test]
fn rows_with_a_count_and_every_row_of_a_table() {
    let body = tbl(&[
        &["H1", "H2"],
        &["a{{ drop(\"row\", 2) }}", "b"],
        &["c", "d"],
        &["e", "f"],
    ]);
    let out = render(Format::Docx, &docx(&body), &json!({}));
    let text = docx_text(&out).join("|");
    assert_eq!(text, "H1|H2|e|f");
    // A table left with no rows goes.
    let body = [tbl(&[&["x{{ drop(\"row\") }}", "y"]]), p("After")].concat();
    let out = render(Format::Docx, &docx(&body), &json!({}));
    let doc = part(&out, "word/document.xml");
    assert!(!doc.contains("<w:tbl>"), "{doc}");
}

#[test]
fn a_row_holding_part_of_a_merge_down_is_refused() {
    let merged = "<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w=\"2000\"/></w:tblGrid>\
        <w:tr><w:tc><w:tcPr><w:vMerge w:val=\"restart\"/></w:tcPr><w:p><w:r><w:t>Tall{{ drop(\"row\") }}</w:t></w:r></w:p></w:tc></w:tr>\
        <w:tr><w:tc><w:tcPr><w:vMerge/></w:tcPr><w:p/></w:tc></w:tr></w:tbl><w:p/>";
    let err = compile(Format::Docx, &docx(merged))
        .unwrap()
        .render(&json!({}))
        .unwrap_err();
    assert!(err.message.contains("merged down"), "{err}");
}

#[test]
fn a_column_across_merged_cells_is_refused() {
    let merged = "<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w=\"2000\"/><w:gridCol w:w=\"2000\"/></w:tblGrid>\
        <w:tr><w:tc><w:tcPr><w:gridSpan w:val=\"2\"/></w:tcPr><w:p><w:r><w:t>Wide</w:t></w:r></w:p></w:tc></w:tr>\
        <w:tr><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>{{ drop(\"column\") }}</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p/>";
    let err = compile(Format::Docx, &docx(merged))
        .unwrap()
        .render(&json!({}))
        .unwrap_err();
    assert!(err.message.contains("merged across columns"), "{err}");

    let table = ptbl(&[&["a", "b"], &["c", "{{ drop(\"column\") }}"]])
        .replacen("<a:tc>", "<a:tc gridSpan=\"2\">", 1)
        .replacen("<a:tc>", "<a:tc hMerge=\"1\">", 1);
    let err = compile(Format::Pptx, &pptx(&[&table]))
        .unwrap()
        .render(&json!({}))
        .unwrap_err();
    assert!(err.message.contains("merged across columns"), "{err}");
}

#[test]
fn a_pptx_column_narrows_the_frame() {
    let table = ptbl(&[&["a", "b{{ drop(\"column\") }}", "c"], &["1", "2", "3"]]).replace(
        "<a:ext cx=\"1\" cy=\"1\"/>",
        "<a:ext cx=\"6000000\" cy=\"741680\"/>",
    );
    let out = render(Format::Pptx, &pptx(&[&table]), &json!({}));
    let slide = part(&out, "ppt/slides/slide1.xml");
    assert_eq!(count("<a:gridCol ", &slide), 2);
    assert_eq!(count("<a:tc>", &slide), 4);
    assert!(slide.contains("cx=\"4000000\""), "{slide}");
    let text = text_of(&slide);
    assert!(text.contains(" c ") && !text.contains(" 2 "), "{text}");
}

#[test]
fn alt_text_can_hold_other_expressions() {
    let bytes = patch(
        PPTX,
        "ppt/slides/slide1.xml",
        "descr=\"{{ drop(&quot;picture&quot;) if not logo }}\"",
        "descr=\"Logo of {{ company }} &amp; co{{ drop(&quot;picture&quot;) if not logo }}\"",
    );
    let mut data = keep_all();
    data["company"] = json!("Acme <Ltd> \"Q\"");
    let out = render(Format::Pptx, &bytes, &data);
    assert_clean(&out);
    let slide = part(&out, &slide_order(&out)[0]);
    assert!(
        slide.contains("descr=\"Logo of Acme &lt;Ltd> &quot;Q&quot; &amp; co\""),
        "{}",
        capture_tag(&slide, "<p:cNvPr id=\"13\"")
    );
    // In a DOCX a newline in a value stays one.
    let bytes = patch(
        DOCX,
        "word/document.xml",
        "title=\"Company logo\"",
        "title=\"{{ caption }}\"",
    );
    let mut data = keep_all();
    data["caption"] = json!("Line one\nLine two");
    let out = render(Format::Docx, &bytes, &data);
    assert_clean(&out);
    let doc = part(&out, "word/document.xml");
    assert!(doc.contains("title=\"Line one&#10;Line two\""), "{doc}");
}

#[test]
fn a_block_in_alt_text_must_close_there() {
    let bytes = patch(
        PPTX,
        "ppt/slides/slide1.xml",
        "descr=\"{{ drop(&quot;picture&quot;) if not logo }}\"",
        "descr=\"{% if logo %}x\"",
    );
    let err = compile_err(Format::Pptx, &bytes);
    assert!(
        err.message.contains("open and close in the same alt text"),
        "{err}"
    );
}

#[test]
fn a_picture_drop_in_a_chart_is_refused() {
    let bytes = patch(
        PPTX,
        "ppt/slides/slide1.xml",
        "{% if not chart %}{{ drop(&quot;chart&quot;) }}{% endif %}",
        "{{ drop(&quot;picture&quot;) }}",
    );
    let err = compile_err(Format::Pptx, &bytes);
    assert!(
        err.message.contains("sits in a chart, not a picture"),
        "{err}"
    );
}

fn capture_tag(text: &str, start: &str) -> String {
    let at = text.find(start).unwrap_or(0);
    text[at..at + text[at..].find('>').unwrap_or(0) + 1].to_string()
}

// --- LibreOffice ------------------------------------------------------------

type Case = (
    Format,
    &'static [u8],
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
);

/// Converts every drop-all output with LibreOffice and checks what it drew,
/// then the same template with nothing dropped.
#[test]
fn libreoffice_opens_every_output_without_the_dropped_elements() {
    if !libreoffice_available() {
        eprintln!("skipping: docker images not available");
        return;
    }
    let keep = std::env::var("DROP_KEEP").ok();
    let temp = tempfile::tempdir().unwrap();
    let dir = keep.as_deref().unwrap_or(temp.path().to_str().unwrap());
    let doc_in: &[&str] = &[
        "Intro paragraph",
        "Between tables",
        "Beans",
        "Mugs",
        "Amount",
        "End of document",
    ];
    let doc_out: &[&str] = &[
        "Note paragraph",
        "Extra one",
        "Discount",
        "Notes table",
        "Hidden item",
        "Stamp",
        "Sales",
    ];
    let sheet_in: &[&str] = &["Report", "Beans", "Mugs", "Amount", "Total"];
    let sheet_out: &[&str] = &["Discount", "Hidden item", "Details", "Stamp", "Sales"];
    let deck_in: &[&str] = &[
        "Point one",
        "Point three",
        "Beans",
        "Mugs",
        "Amount",
        "Thanks",
    ];
    let deck_out: &[&str] = &[
        "Point two",
        "Discount",
        "Notes table",
        "Hidden item",
        "Stamp",
        "Appendix",
        "Sales",
    ];
    let cases: [Case; 6] = [
        (Format::Docx, DOCX, "docx", doc_in, doc_out),
        (Format::Odt, ODT, "odt", doc_in, doc_out),
        (Format::Xlsx, XLSX, "xlsx", sheet_in, sheet_out),
        (Format::Ods, ODS, "ods", sheet_in, sheet_out),
        (Format::Pptx, PPTX, "pptx", deck_in, deck_out),
        (Format::Odp, ODP, "odp", deck_in, deck_out),
    ];
    for (format, template, ext, present, absent) in cases {
        let file = format!("dropped.{ext}");
        std::fs::write(
            format!("{dir}/{file}"),
            render(format, template, &drop_all()),
        )
        .unwrap();
        assert!(soffice(dir, "pdf", &file), "{file}: conversion failed");
        let pdf = format!("dropped_{ext}.pdf");
        std::fs::rename(format!("{dir}/dropped.pdf"), format!("{dir}/{pdf}")).unwrap();
        let text = pdf_text(dir, &pdf);
        for w in present {
            assert!(text.contains(w), "{file}: no {w:?} in {text}");
        }
        for w in absent {
            assert!(!text.contains(w), "{file}: {w:?} should be gone: {text}");
        }
        // ODS cells stay text, which SUM skips.
        if format == Format::Xlsx {
            // The totals come from the moved formulas: 10 + 30, then twice.
            assert!(text.contains("40") && text.contains("80"), "{file}: {text}");
        }
        let file = format!("kept.{ext}");
        std::fs::write(
            format!("{dir}/{file}"),
            render(format, template, &keep_all()),
        )
        .unwrap();
        assert!(soffice(dir, "pdf", &file), "{file}: conversion failed");
        let pdf = format!("kept_{ext}.pdf");
        std::fs::rename(format!("{dir}/kept.pdf"), format!("{dir}/{pdf}")).unwrap();
        let text = pdf_text(dir, &pdf);
        // The print area of the sheets leaves their drawings off the page.
        let shown: &[&str] = match format {
            Format::Xlsx | Format::Ods => &["Discount", "Hidden"],
            _ => &["Discount", "Sales", "Stamp"],
        };
        for w in shown.iter().filter(|w| **w != "Hidden") {
            assert!(text.contains(w), "{file}: no {w:?} in {text}");
        }
        assert!(!text.contains("Hidden item"), "{file}: the hidden row goes");
    }
}

// --- Sheet loops ------------------------------------------------------------

const SHEETS_XLSX: &[u8] = include_bytes!("fixtures/sheets.xlsx");
const SHEETS_ODS: &[u8] = include_bytes!("fixtures/sheets.ods");

fn sites(skip_lyon: bool) -> Value {
    let group = |n: &str| json!({"name": n, "items": [{"name": "Mug", "qty": 1, "price": 2}]});
    json!({"sites": [
        {"name": "Paris", "city": "Paris", "skip": false, "groups": [group("Coffee")]},
        {"name": "Lyon", "city": "Lyon", "skip": skip_lyon, "groups": [group("Tea")]}
    ]})
}

#[test]
fn a_drop_in_a_sheet_loop_removes_that_copy_only() {
    let xlsx = patch(
        SHEETS_XLSX,
        "xl/sharedStrings.xml",
        "{% sheet for s in sites %}Site {{ s.name }}",
        "{% sheet for s in sites %}Site {{ s.name }}{{ drop(&quot;sheet&quot;) if s.skip }}",
    );
    let out = render(Format::Xlsx, &xlsx, &sites(true));
    assert_clean(&out);
    let wb = part(&out, "xl/workbook.xml");
    assert!(
        wb.contains("name=\"Paris\"") && !wb.contains("name=\"Lyon\""),
        "{wb}"
    );
    let out = render(Format::Xlsx, &xlsx, &sites(false));
    assert!(part(&out, "xl/workbook.xml").contains("name=\"Lyon\""));

    let ods = patch(
        SHEETS_ODS,
        "content.xml",
        "Site {{ s.name }}",
        "Site {{ s.name }}{{ drop(&quot;sheet&quot;) if s.skip }}",
    );
    let out = render(Format::Ods, &ods, &sites(true));
    assert_clean(&out);
    let content = part(&out, "content.xml");
    assert!(content.contains("table:name=\"Paris\"") && !content.contains("table:name=\"Lyon\""));
}

#[test]
fn dropping_every_sheet_is_refused() {
    let one = xlsx(
        &[row(1, &[cell("A1", "inlineStr", "{{ drop(\"sheet\") }}")])],
        &[],
    );
    let err = compile(Format::Xlsx, &one)
        .unwrap()
        .render(&json!({}))
        .unwrap_err();
    assert!(err.message.contains("removes every sheet"), "{err}");
}
