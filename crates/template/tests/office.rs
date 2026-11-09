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

/// The deck's title slide with a card per product below the title: a filled
/// rounded rectangle for the name and a text box for the description, shapes
/// as PowerPoint writes them, with the loop moving each card 3 cm down.
fn deck_with_cards() -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/office/powerpoint-deck.pptx",
        env!("CARGO_MANIFEST_DIR")
    );
    let deck = std::fs::read(path).unwrap();
    let cards = r#"<p:sp><p:nvSpPr><p:cNvPr id="4" name="Rounded Rectangle 3"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="838200" y="4000000"/><a:ext cx="3600000" cy="900000"/></a:xfrm><a:prstGeom prst="roundRect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="4472C4"/></a:solidFill><a:ln><a:noFill/></a:ln></p:spPr><p:txBody><a:bodyPr rtlCol="0" anchor="ctr"/><a:lstStyle/><a:p><a:pPr algn="ctr"/><a:r><a:rPr lang="en-US" sz="2400" dirty="0"><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill></a:rPr><a:t>{% for p in products | shift(y=“3cm”) %}{{ p.name }}</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:cNvPr id="5" name="TextBox 4"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="4700000" y="4000000"/><a:ext cx="5000000" cy="900000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/></p:spPr><p:txBody><a:bodyPr wrap="square" rtlCol="0" anchor="ctr"/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" sz="2400" dirty="0"/><a:t>{{ p.description }}{% endfor %}</a:t></a:r></a:p></p:txBody></p:sp>"#;
    let files: Vec<(String, Vec<u8>)> = entries(&deck)
        .into_iter()
        .map(|(name, data)| {
            if name != "ppt/slides/slide1.xml" {
                return (name, data);
            }
            let xml = String::from_utf8(data).unwrap();
            // Keep the title; the subtitle gives way to the cards.
            let subtitle = xml.find("<p:sp><p:nvSpPr><p:cNvPr id=\"3\"").unwrap();
            let end = xml.find("</p:spTree>").unwrap();
            let xml = format!("{}{cards}{}", &xml[..subtitle], &xml[end..]);
            (name, xml.into_bytes())
        })
        .collect();
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();
    zip(&refs)
}

/// A loop across two shapes with `shift` repeats the shapes on its slide,
/// each card 3 cm below the last, instead of repeating the slide.
#[test]
fn powerpoint_cards_move_down_the_slide_with_shift() {
    let deck = deck_with_cards();
    let out = compile(Format::Pptx, &deck)
        .expect("compiles")
        .render(&data())
        .expect("renders");
    let slides = names(&out)
        .into_iter()
        .filter(|n| n.starts_with("ppt/slides/slide") && n.ends_with(".xml"))
        .count();
    assert_eq!(slides, 5, "slide 1 is not repeated; slide 3 still is");
    let slide = part(&out, "ppt/slides/slide1.xml");
    let offsets: Vec<(String, String)> = Regex::new(r#"<a:off x="(\d+)" y="(\d+)"/>"#)
        .unwrap()
        .captures_iter(&slide)
        .map(|c| (c[1].to_string(), c[2].to_string()))
        .collect();
    let want = [
        ("0", "0"),
        ("838200", "4000000"),
        ("4700000", "4000000"),
        ("838200", "5080000"),
        ("4700000", "5080000"),
    ];
    let want: Vec<(String, String)> = want
        .iter()
        .map(|(x, y)| (x.to_string(), y.to_string()))
        .collect();
    assert_eq!(offsets, want, "{slide}");
    let text = Regex::new("<[^>]+>").unwrap().replace_all(&slide, "|");
    let order: Vec<&str> = [
        "Quarterly review",
        "House blend",
        "Medium roast",
        "Kenya AA",
        "Bright",
    ]
    .iter()
    .map(|w| {
        assert!(text.contains(w), "missing {w} in {text}");
        *w
    })
    .collect();
    let mut at = 0;
    for w in order {
        let found = text[at..].find(w).expect("in order") + at;
        at = found;
    }

    if !libreoffice_available() {
        eprintln!("skipping the LibreOffice check: docker images not available");
        return;
    }
    let keep = std::env::var("RICH_KEEP").ok();
    let temp = tempfile::tempdir().unwrap();
    let dir = keep.as_deref().unwrap_or(temp.path().to_str().unwrap());
    std::fs::write(format!("{dir}/cards.pptx"), &out).unwrap();
    assert!(soffice(dir, "pdf", "cards.pptx"), "conversion failed");
    let text = pdf_text(dir, "cards.pdf");
    for w in [
        "Quarterly review",
        "House blend",
        "Medium roast",
        "Kenya AA",
        "Bright",
    ] {
        assert!(text.contains(w), "no {w:?} in {text}");
    }
}
