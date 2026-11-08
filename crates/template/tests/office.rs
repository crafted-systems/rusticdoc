//! Templates made by hand in Word, Excel and PowerPoint for Mac 16, as their
//! authors would type them: curly quotes, a tag split by formatting, spelling
//! marks, placeholders that inherit their position, and a loop that opens in
//! one shape and closes in another. Each test pins a fault Office showed.

mod common;

use common::*;
use regex::Regex;
use rusticdoc_template::{compile, Format};
use serde_json::{json, Value};

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

fn data() -> Value {
    json!({
        "seller": {"name": "Mawingu Coffee Co."},
        "number": "INV-2041", "issued": "2026-11-06", "currency": "KES",
        "customer": {"name": "Amani Otieno", "address": "48 Sample Avenue, Mombasa"},
        "items": [
            {"name": "Beans", "qty": 12, "price": 2400},
            {"name": "Filters", "qty": 6, "price": 950}
        ],
        "total": 36300,
        "payment_url": "https://pay.example.com/INV-2041",
        "status": "Overdue",
        "notes": "<p>Thanks</p>",
        "logo": PNG,
        "company": "Mawingu Coffee Co.",
        "title": "Quarterly review",
        "products": [
            {"name": "House blend", "description": "Medium roast", "price": 1800},
            {"name": "Kenya AA", "description": "Bright", "price": 2400}
        ],
        "rows": [{"month": "August", "sales": 90}, {"month": "September", "sales": 120}]
    })
}

fn render(format: Format, fixture: &str) -> Vec<u8> {
    let bytes = std::fs::read(format!(
        "{}/tests/fixtures/office/{fixture}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    compile(format, &bytes)
        .expect("compiles")
        .render(&data())
        .expect("renders")
}

fn names(bytes: &[u8]) -> Vec<String> {
    entries(bytes).into_iter().map(|(n, _)| n).collect()
}

#[test]
fn word_tags_survive_curly_quotes_and_a_split_tag() {
    let out = render(Format::Docx, "word-invoice.docx");
    let xml = part(&out, "word/document.xml");
    let text = Regex::new("<[^>]+>").unwrap().replace_all(&xml, "");
    for want in [
        "Invoice INV-2041, issued 06 November 2026",
        "Billed to Amani Otieno",
        "KES 28,800.00",
        "Total due: KES 36,300.00",
    ] {
        assert!(text.contains(want), "missing {want:?} in {text}");
    }
    assert!(!text.contains("{{") && !text.contains("{%"), "{text}");
}

/// Excel rebuilt rows and repaired the file: its calculation chain still
/// named the formula cells from before the loop moved them.
#[test]
fn excel_loses_its_stale_calculation_chain() {
    let out = render(Format::Xlsx, "excel-report.xlsx");
    assert!(!names(&out).iter().any(|n| n == "xl/calcChain.xml"));
    assert!(!part(&out, "[Content_Types].xml").contains("calcChain"));
    assert!(!part(&out, "xl/_rels/workbook.xml.rels").contains("calcChain"));
    let sheet = part(&out, "xl/worksheets/sheet1.xml");
    assert!(sheet.contains("<f>SUM(B4:B5)</f>"), "{sheet}");
}

/// A loop opened in a slide's title and closed in its body printed every
/// product over the last on one slide. It repeats the slide.
#[test]
fn powerpoint_loop_across_shapes_repeats_the_slide() {
    let out = render(Format::Pptx, "powerpoint-deck.pptx");
    let slides: Vec<String> = names(&out)
        .into_iter()
        .filter(|n| n.starts_with("ppt/slides/slide"))
        .collect();
    assert_eq!(slides.len(), 5, "{slides:?}");
    let pres = part(&out, "ppt/presentation.xml");
    assert_eq!(pres.matches("<p:sldId ").count(), 5, "{pres}");
    let texts: Vec<String> = (1..=5)
        .map(|i| part(&out, &format!("ppt/slides/slide{i}.xml")))
        .collect();
    assert_eq!(
        texts.iter().filter(|t| t.contains("House blend")).count(),
        1
    );
    assert_eq!(texts.iter().filter(|t| t.contains("Kenya AA")).count(), 1);
    assert!(!texts
        .iter()
        .any(|t| t.contains("House blend") && t.contains("Kenya AA")));
}

/// Placeholders without a position of their own sat the chart and the picture
/// at the slide's corner. They take the layout's position, through its master.
#[test]
fn powerpoint_chart_and_picture_take_the_placeholder_position() {
    let out = render(Format::Pptx, "powerpoint-deck.pptx");
    let all: String = (1..=5)
        .map(|i| part(&out, &format!("ppt/slides/slide{i}.xml")))
        .collect();
    let frame = Regex::new(r#"<p:graphicFrame>.*?<a:off x="(\d+)" y="(\d+)"/>"#).unwrap();
    let c = frame.captures(&all).expect("a chart frame");
    assert_ne!((&c[1], &c[2]), ("0", "0"), "chart at the corner");
    let pic = Regex::new(r#"<p:pic>.*?<a:off x="(\d+)" y="(\d+)"/>"#).unwrap();
    let p = pic.captures(&all).expect("a picture");
    assert_ne!((&p[1], &p[2]), ("0", "0"), "picture at the corner");
}
