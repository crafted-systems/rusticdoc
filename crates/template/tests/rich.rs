//! The filters that produce content other than text, format by format.

mod common;

use common::*;
use regex::Regex;
use rusticdoc_template::{
    compile, environment, rich_environment, validate, Compiled, Format, Severity,
};
use serde_json::{json, Value};

const IMAGE_REL: &str = "relationships/image\"";
const HYPERLINK_REL: &str = "relationships/hyperlink\"";
const CHART_REL: &str = "relationships/chart\"";
const DRAWING_REL: &str = "relationships/drawing\"";
const CHART_CT: &str = "application/vnd.openxmlformats-officedocument.drawingml.chart+xml";

fn render(format: Format, bytes: &[u8], data: Value) -> Vec<u8> {
    compile(format, bytes)
        .expect("compiles")
        .render(&data)
        .expect("renders")
}

fn render_err(format: Format, bytes: &[u8], data: Value) -> String {
    let err = compile(format, bytes)
        .expect("compiles")
        .render(&data)
        .expect_err("must fail");
    assert!(
        err.location.is_some(),
        "the error should say where it happened: {err}"
    );
    err.to_string()
}

fn logo() -> Vec<u8> {
    png(200, 100)
}

fn logo_data() -> Value {
    json!({ "logo": b64(&logo()) })
}

/// The target of the relationship with this id in a rels part.
fn rel_target(rels: &str, id: &str) -> String {
    let re = Regex::new("<Relationship [^>]*>").unwrap();
    let attr = |tag: &str, name: &str| {
        Regex::new(&format!("{name}=\"([^\"]*)\""))
            .unwrap()
            .captures(tag)
            .map(|c| c[1].to_string())
    };
    let target = re
        .find_iter(rels)
        .map(|m| m.as_str())
        .find(|tag| attr(tag, "Id").as_deref() == Some(id))
        .and_then(|tag| attr(tag, "Target"));
    target.unwrap_or_else(|| panic!("no relationship {id} in {rels}"))
}

fn capture(re: &str, text: &str) -> String {
    Regex::new(re)
        .unwrap()
        .captures(text)
        .unwrap_or_else(|| panic!("{re} not found in {text}"))[1]
        .to_string()
}

// --- image ----------------------------------------------------------------

#[test]
fn docx_image_is_inline_with_its_relationship_and_media_part() {
    let doc = docx(&(p("Before {{ logo | image(width=120) }} after") + &p("Other")));
    let out = render(Format::Docx, &doc, logo_data());
    let xml = part(&out, "word/document.xml");
    // 120 pt wide, the 2:1 aspect kept.
    assert!(
        xml.contains("<wp:extent cx=\"1524000\" cy=\"762000\"/>"),
        "{xml}"
    );
    assert!(xml.contains("<w:drawing><wp:inline "), "{xml}");
    assert!(xml.contains("xmlns:wp="), "namespace declared: {xml}");
    let rid = capture("r:embed=\"(rId[0-9]+)\"", &xml);
    let rels = part(&out, "word/_rels/document.xml.rels");
    assert!(rels.contains(IMAGE_REL));
    assert_eq!(rel_target(&rels, &rid), "media/rich1.png");
    assert_eq!(bytes_of(&out, "word/media/rich1.png").unwrap(), logo());
    let types = part(&out, "[Content_Types].xml");
    assert_eq!(types.matches("Extension=\"png\"").count(), 1, "{types}");
    assert_eq!(docx_text(&out), ["Before  after", "Other"]);
    // The text around the tag kept its run properties in both halves.
    assert_eq!(xml.matches("<w:rsid w:val=\"000\"/>").count(), 4, "{xml}");
    assert_unchanged(
        &doc,
        &out,
        &["word/header1.xml", "word/media/image1.png", "_rels/.rels"],
    );
}

#[test]
fn image_values_may_be_data_uris_or_objects() {
    let doc = docx(&p(
        "{{ a | image }}{{ b | image(height=30) }}{{ c | image }}",
    ));
    let jpg = jpeg(40, 20);
    let out = render(
        Format::Docx,
        &doc,
        json!({
            "a": data_uri("image/png", &png(800, 400)),
            "b": data_uri("image/jpeg", &jpg),
            "c": {"data": b64(&png(10, 10)), "mime": "image/png", "width": 50, "height": 70},
        }),
    );
    let xml = part(&out, "word/document.xml");
    // 800 px is 600 pt at 96 dpi: capped at the 450 pt text width.
    assert!(xml.contains("cx=\"5715000\" cy=\"2857500\""), "{xml}");
    assert!(xml.contains("cx=\"762000\" cy=\"381000\""), "{xml}");
    assert!(xml.contains("cx=\"635000\" cy=\"889000\""), "{xml}");
    assert_eq!(bytes_of(&out, "word/media/rich2.jpeg").unwrap(), jpg);
    let types = part(&out, "[Content_Types].xml");
    assert!(
        types.contains("Extension=\"jpeg\" ContentType=\"image/jpeg\""),
        "{types}"
    );
}

#[test]
fn bad_images_are_template_errors() {
    let doc = docx(&p("{{ v | image }}"));
    let cases = [
        (json!("http://example.com/logo.png"), "URLs are not fetched"),
        (json!("not base64!"), "base64"),
        (json!(b64(b"GIF89a12345")), "only PNG and JPEG"),
        (json!(b64(b"\x89PNG\r\n\x1a\nthis is no png")), "dimensions"),
        (
            json!({"data": b64(&png(2, 2)), "mime": "image/jpeg"}),
            "image/jpeg",
        ),
        (json!({"nodata": 1}), "data must be a string"),
        (json!(12), "expected a data URI"),
        (json!(data_uri("image/jpeg", &png(2, 2))), "image/jpeg"),
    ];
    for (value, expected) in cases {
        let err = render_err(Format::Docx, &doc, json!({ "v": value }));
        assert!(err.contains(expected), "{expected:?} not in {err}");
    }
    let doc = docx(&p("{{ v | image(width=-1) }}"));
    let err = render_err(Format::Docx, &doc, json!({"v": b64(&png(2, 2))}));
    assert!(err.contains("width must be above 0"), "{err}");
    let doc = docx(&p("{{ v | image(depth=1) }}"));
    let err = render_err(Format::Docx, &doc, json!({"v": b64(&png(2, 2))}));
    assert!(err.contains("depth"), "{err}");
}

#[test]
fn images_are_capped_per_size_and_per_render() {
    let doc = docx(&p("{{ v | image }}"));
    let mut big = png(2, 2);
    big.resize(20 * 1024 * 1024 + 1, 0);
    let err = render_err(Format::Docx, &doc, json!({"v": b64(&big)}));
    assert!(err.contains("larger than"), "{err}");

    let looped = docx(&p("{% for i in v %}{{ i | image }}{% endfor %}"));
    let one = b64(&png(2, 2));
    let at_cap: Vec<&String> = std::iter::repeat_n(&one, 200).collect();
    let out = render(Format::Docx, &looped, json!({ "v": at_cap }));
    assert_eq!(
        part(&out, "word/document.xml")
            .matches("<w:drawing>")
            .count(),
        200
    );
    let over: Vec<&String> = std::iter::repeat_n(&one, 201).collect();
    let err = render_err(Format::Docx, &looped, json!({ "v": over }));
    assert!(err.contains("at most 200 images"), "{err}");
}

#[test]
fn images_in_a_table_row_loop_share_one_media_part() {
    let doc = docx(&tbl(&[
        &["Item", "Logo"],
        &[
            "{% for i in items %}{{ i.name }}",
            "{{ i.logo | image(width=20) }}{% endfor %}",
        ],
    ]));
    let logo = b64(&logo());
    let out = render(
        Format::Docx,
        &doc,
        json!({"items": [
            {"name": "A", "logo": logo},
            {"name": "B", "logo": logo},
            {"name": "C", "logo": logo},
        ]}),
    );
    let xml = part(&out, "word/document.xml");
    assert_eq!(xml.matches("<w:tr>").count(), 4);
    assert_eq!(xml.matches("<w:drawing>").count(), 3);
    let ids: Vec<String> = Regex::new("<wp:docPr id=\"([0-9]+)\"")
        .unwrap()
        .captures_iter(&xml)
        .map(|c| c[1].to_string())
        .collect();
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 3, "drawing ids must be unique: {ids:?}");
    let rids: Vec<String> = Regex::new("r:embed=\"(rId[0-9]+)\"")
        .unwrap()
        .captures_iter(&xml)
        .map(|c| c[1].to_string())
        .collect();
    assert_eq!(rids.len(), 3);
    assert!(rids.iter().all(|r| r == &rids[0]), "{rids:?}");
    let media: Vec<String> = entries(&out)
        .into_iter()
        .map(|(n, _)| n)
        .filter(|n| n.contains("rich"))
        .collect();
    assert_eq!(media, ["word/media/rich1.png"]);
    assert_eq!(docx_text(&out)[2..6], ["A", "", "B", ""]);
}

#[test]
fn an_image_in_a_header_uses_the_headers_own_relationships() {
    let doc = docx_with(&p("Body"), "{{ logo | image }}");
    let out = render(Format::Docx, &doc, logo_data());
    let header = part(&out, "word/header1.xml");
    let rid = capture("r:embed=\"(rId[0-9]+)\"", &header);
    let rels = part(&out, "word/_rels/header1.xml.rels");
    assert_eq!(rel_target(&rels, &rid), "media/rich1.png");
    assert_unchanged(
        &doc,
        &out,
        &["word/_rels/document.xml.rels", "word/document.xml"],
    );
}

#[test]
fn pptx_image_is_a_picture_at_the_shape_of_its_tag() {
    let shape_at = "<p:sp><p:nvSpPr><p:cNvPr id=\"7\" name=\"Text\"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x=\"1000\" y=\"2000\"/><a:ext cx=\"10\" cy=\"10\"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:p><a:r><a:rPr lang=\"en-US\"/><a:t>A {{ logo | image(width=100) }} B</a:t></a:r></a:p></p:txBody></p:sp>";
    let deck = pptx(&[shape_at, &shape(&["Plain"])]);
    let out = render(Format::Pptx, &deck, logo_data());
    let xml = part(&out, "ppt/slides/slide1.xml");
    assert!(
        xml.contains("<a:off x=\"1000\" y=\"2000\"/><a:ext cx=\"1270000\" cy=\"635000\"/>"),
        "{xml}"
    );
    assert_eq!(slide_text(&out, "ppt/slides/slide1.xml"), ["A  B"]);
    // The picture joins the tree after the shape, with an id of its own.
    let sp_end = xml.find("</p:sp>").unwrap();
    let pic = xml.find("<p:pic>").unwrap();
    assert!(
        sp_end < pic && pic < xml.find("</p:spTree>").unwrap(),
        "{xml}"
    );
    assert!(
        xml.contains("<p:cNvPr id=\"8\" name=\"Picture 8\"/>"),
        "{xml}"
    );
    let rid = capture("r:embed=\"(rId[0-9]+)\"", &xml);
    let rels = part(&out, "ppt/slides/_rels/slide1.xml.rels");
    assert_eq!(rel_target(&rels, &rid), "../media/rich1.png");
    assert_eq!(bytes_of(&out, "ppt/media/rich1.png").unwrap(), logo());
    assert_unchanged(
        &deck,
        &out,
        &["ppt/slides/slide2.xml", "ppt/slides/_rels/slide2.xml.rels"],
    );
}

#[test]
fn pptx_slide_loop_gives_every_copy_its_own_relationship() {
    let deck = pptx(&[
        &shape(&["{% for i in items %}{{ i | image }}{% endfor %}"]),
        "",
    ]);
    let deck2 = pptx(&[
        &shape(&["{% for i in items %}{{ i | image }}"]),
        &shape(&["{% endfor %}"]),
    ]);
    let one = b64(&png(10, 10));
    let two = b64(&png(20, 20));
    let out = render(Format::Pptx, &deck, json!({"items": [one, two]}));
    assert_eq!(
        part(&out, "ppt/slides/slide1.xml")
            .matches("<p:pic>")
            .count(),
        2
    );
    let out = render(Format::Pptx, &deck2, json!({"items": [one, two]}));
    let order = slide_order(&out);
    assert_eq!(order.len(), 4);
    let pictures: Vec<usize> = order
        .iter()
        .map(|s| part(&out, s).matches("<p:pic>").count())
        .collect();
    assert_eq!(pictures.iter().sum::<usize>(), 2, "{pictures:?}");
    for slide in &order {
        let xml = part(&out, slide);
        if xml.contains("<p:pic>") {
            let rid = capture("r:embed=\"(rId[0-9]+)\"", &xml);
            let rels = part(
                &out,
                &slide
                    .replace("slides/", "slides/_rels/")
                    .replace(".xml", ".xml.rels"),
            );
            assert!(
                rel_target(&rels, &rid).starts_with("../media/rich"),
                "{rels}"
            );
        }
    }
}

#[test]
fn xlsx_image_is_anchored_at_the_cell_after_row_numbering() {
    let book = xlsx(
        &[
            row(1, &[cell("A1", "inlineStr", "Head")]),
            row(
                2,
                &[cell("B2", "inlineStr", "{% for i in items %}{{ i.name }}")],
            ),
            row(3, &[cell("B3", "inlineStr", "{% endfor %}")]),
            row(
                5,
                &[cell("C5", "inlineStr", "{{ logo | image(width=60) }}")],
            ),
        ],
        &[],
    );
    let mut data = logo_data();
    data["items"] = json!([{"name": "x"}, {"name": "y"}]);
    let out = render(Format::Xlsx, &book, data);
    let sheet = part(&out, "xl/worksheets/sheet1.xml");
    // Two rows of two copies push row 5 to row 7.
    assert!(sheet.contains("<c r=\"C7\"/>"), "the cell empties: {sheet}");
    let rid = capture("<drawing r:id=\"(rId[0-9]+)\"/>", &sheet);
    assert!(
        sheet.find("</sheetData>").unwrap() < sheet.find("<drawing ").unwrap(),
        "{sheet}"
    );
    let rels = part(&out, "xl/worksheets/_rels/sheet1.xml.rels");
    assert!(rels.contains(DRAWING_REL), "{rels}");
    assert_eq!(rel_target(&rels, &rid), "../drawings/drawing1.xml");
    let drawing = part(&out, "xl/drawings/drawing1.xml");
    assert!(
        drawing.contains(
            "<xdr:from><xdr:col>2</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>6</xdr:row>"
        ),
        "{drawing}"
    );
    assert!(
        drawing.contains("<xdr:ext cx=\"762000\" cy=\"381000\"/>"),
        "{drawing}"
    );
    let image_rid = capture("r:embed=\"(rId[0-9]+)\"", &drawing);
    let drawing_rels = part(&out, "xl/drawings/_rels/drawing1.xml.rels");
    assert_eq!(rel_target(&drawing_rels, &image_rid), "../media/rich1.png");
    let types = part(&out, "[Content_Types].xml");
    assert!(types.contains("/xl/drawings/drawing1.xml"), "{types}");
    assert_eq!(bytes_of(&out, "xl/media/rich1.png").unwrap(), logo());
    assert_unchanged(
        &book,
        &out,
        &["xl/sharedStrings.xml", "xl/_rels/workbook.xml.rels"],
    );
}

#[test]
fn xlsx_image_goes_into_a_drawing_the_sheet_already_has() {
    let book = xlsx(
        &[row(1, &[cell("A1", "inlineStr", "{{ logo | image }}")])],
        &[],
    );
    let once = render(Format::Xlsx, &book, logo_data());
    // Feed a sheet that already has a drawing back in, with a new tag.
    let mut files = entries(&once);
    for (name, data) in &mut files {
        if name == "xl/worksheets/sheet1.xml" {
            let text = String::from_utf8(data.clone()).unwrap().replace(
                "<c r=\"A1\"/>",
                "<c r=\"A1\" t=\"inlineStr\"><is><t>{{ logo | image }}</t></is></c>",
            );
            *data = text.into_bytes();
        }
    }
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();
    let out = render(Format::Xlsx, &zip(&refs), logo_data());
    assert!(bytes_of(&out, "xl/drawings/drawing2.xml").is_none());
    let drawing = part(&out, "xl/drawings/drawing1.xml");
    assert_eq!(
        drawing.matches("<xdr:oneCellAnchor>").count(),
        2,
        "{drawing}"
    );
    let ids = Regex::new("<xdr:cNvPr id=\"([0-9]+)\"").unwrap();
    let found: Vec<String> = ids
        .captures_iter(&drawing)
        .map(|c| c[1].to_string())
        .collect();
    assert_eq!(found, ["1", "2"]);
    let sheet = part(&out, "xl/worksheets/sheet1.xml");
    assert_eq!(sheet.matches("<drawing ").count(), 1, "{sheet}");
    assert!(sheet.contains("xmlns:r="), "{sheet}");
}

#[test]
fn odf_images_are_frames_listed_in_the_manifest() {
    let doc = odt("<text:p>See {{ logo | image(width=120) }} here</text:p>");
    let out = render(Format::Odt, &doc, logo_data());
    let xml = part(&out, "content.xml");
    assert!(
        xml.contains("text:anchor-type=\"as-char\" svg:width=\"120.00pt\" svg:height=\"60.00pt\""),
        "{xml}"
    );
    assert!(
        xml.contains("<draw:image xlink:href=\"Pictures/rich1.png\""),
        "{xml}"
    );
    assert!(xml.contains("xmlns:xlink="), "{xml}");
    assert_eq!(bytes_of(&out, "Pictures/rich1.png").unwrap(), logo());
    let manifest = part(&out, "META-INF/manifest.xml");
    assert!(
        manifest.contains(
            "manifest:full-path=\"Pictures/rich1.png\" manifest:media-type=\"image/png\""
        ),
        "{manifest}"
    );
    assert_eq!(entries(&out)[0].0, "mimetype");
    assert_unchanged(&doc, &out, &["mimetype", "styles.xml"]);

    let sheet = "<table:table table:name=\"S\"><table:table-column/><table:table-row><table:table-cell office:value-type=\"string\"><text:p>{{ logo | image }}</text:p></table:table-cell></table:table-row></table:table>";
    let out = render(Format::Ods, &ods(sheet), logo_data());
    let xml = part(&out, "content.xml");
    assert!(
        xml.contains("<table:table-cell office:value-type=\"string\"><draw:frame "),
        "the frame sits on the cell: {xml}"
    );
    assert!(part(&out, "META-INF/manifest.xml").contains("Pictures/rich1.png"));

    let slide = "<draw:page draw:name=\"p1\"><draw:frame svg:x=\"3cm\" svg:y=\"4cm\"><draw:text-box><text:p>Hi {{ logo | image }}</text:p></draw:text-box></draw:frame></draw:page>";
    let out = render(Format::Odp, &odp(slide), logo_data());
    let xml = part(&out, "content.xml");
    assert!(
        xml.contains("</draw:frame><draw:frame draw:name=\"Rich2\" svg:x=\"3cm\" svg:y=\"4cm\""),
        "the frame sits on the page at the shape's place: {xml}"
    );
    assert_eq!(odf_paragraphs(&xml), ["Hi "]);
}

// --- barcode --------------------------------------------------------------

#[test]
fn barcodes_become_real_pngs_in_every_format() {
    for (kind, value) in [
        ("qr", "https://example.com/a?b=1"),
        ("code128", "SKU-42"),
        ("code39", "SKU42"),
        ("ean13", "5901234123457"),
        ("ean8", "96385074"),
    ] {
        let doc = docx(&p(&format!("{{{{ v | barcode(\"{kind}\", size=100) }}}}")));
        let out = render(Format::Docx, &doc, json!({"v": value}));
        let png = bytes_of(&out, "word/media/rich1.png").unwrap();
        assert!(png.starts_with(b"\x89PNG"), "{kind}");
        let xml = part(&out, "word/document.xml");
        assert!(xml.contains("<wp:extent cx=\"1270000\" "), "{kind}: {xml}");
    }
    let doc = pptx(&[&shape(&["{{ v | barcode(\"qr\") }}"])]);
    let out = render(Format::Pptx, &doc, json!({"v": "x"}));
    assert!(part(&out, "ppt/slides/slide1.xml").contains("<p:pic>"));
    let doc = xlsx(
        &[row(
            1,
            &[cell("A1", "inlineStr", "{{ v | barcode(\"qr\") }}")],
        )],
        &[],
    );
    let out = render(Format::Xlsx, &doc, json!({"v": "x"}));
    assert!(bytes_of(&out, "xl/media/rich1.png").is_some());
    let doc = odt("<text:p>{{ v | barcode(\"ean8\") }}</text:p>");
    let out = render(Format::Odt, &doc, json!({"v": "96385074"}));
    assert!(bytes_of(&out, "Pictures/rich1.png").is_some());
}

#[test]
fn barcode_values_are_checked() {
    let doc = docx(&p("{{ v | barcode(k) }}"));
    for (k, v, expected) in [
        ("ean13", "5901234123458", "not a valid ean13"),
        ("ean13", "123", "not a valid ean13"),
        ("ean8", "abcdefgh", "not a valid ean8"),
        ("code39", "lower", "not a valid code39"),
        ("code128", "caf\u{e9}", "printable ASCII"),
        ("pdf417", "x", "unknown symbology"),
    ] {
        let err = render_err(Format::Docx, &doc, json!({"k": k, "v": v}));
        assert!(err.contains(expected), "{expected:?} not in {err}");
    }
    let long = "x".repeat(4097);
    let err = render_err(Format::Docx, &doc, json!({"k": "qr", "v": long}));
    assert!(err.contains("longer than 4096"), "{err}");
    let doc = docx(&p("{{ v | barcode }}"));
    let err = render_err(Format::Docx, &doc, json!({"v": "x"}));
    assert!(err.contains("symbology"), "{err}");
}

// --- link -----------------------------------------------------------------

#[test]
fn links_in_every_format() {
    let url = "https://example.com/?a=1&b=2";
    let data = json!({"u": url, "label": "Go <here>"});
    let ampersand = "https://example.com/?a=1&amp;b=2";

    let doc = docx(&p("See {{ u | link(label) }}."));
    let out = render(Format::Docx, &doc, data.clone());
    let xml = part(&out, "word/document.xml");
    let rid = capture("<w:hyperlink r:id=\"(rId[0-9]+)\"", &xml);
    assert!(xml.contains("Go &lt;here&gt;"), "{xml}");
    let rels = part(&out, "word/_rels/document.xml.rels");
    assert_eq!(rel_target(&rels, &rid), ampersand);
    assert!(rels.contains(HYPERLINK_REL) && rels.contains("TargetMode=\"External\""));
    assert_eq!(docx_text(&out), ["See Go <here>."]);

    let deck = pptx(&[&shape(&["See {{ u | link(label) }}."])]);
    let out = render(Format::Pptx, &deck, data.clone());
    let xml = part(&out, "ppt/slides/slide1.xml");
    let rid = capture("<a:hlinkClick r:id=\"(rId[0-9]+)\"/>", &xml);
    assert!(
        xml.contains("<a:rPr lang=\"en-US\"><a:hlinkClick"),
        "on the run's properties: {xml}"
    );
    let rels = part(&out, "ppt/slides/_rels/slide1.xml.rels");
    assert_eq!(rel_target(&rels, &rid), ampersand);
    assert_eq!(
        slide_text(&out, "ppt/slides/slide1.xml"),
        ["See Go <here>."]
    );

    let book = xlsx(
        &[row(2, &[cell("B2", "inlineStr", "{{ u | link(label) }}")])],
        &[],
    );
    let out = render(Format::Xlsx, &book, data.clone());
    let sheet = part(&out, "xl/worksheets/sheet1.xml");
    let rid = capture("<hyperlink ref=\"B2\" r:id=\"(rId[0-9]+)\"/>", &sheet);
    assert!(sheet.contains("Go &lt;here&gt;"), "{sheet}");
    assert!(sheet.find("</sheetData>").unwrap() < sheet.find("<hyperlinks>").unwrap());
    let rels = part(&out, "xl/worksheets/_rels/sheet1.xml.rels");
    assert_eq!(rel_target(&rels, &rid), ampersand);

    for (format, doc) in [
        (Format::Odt, odt("<text:p>See {{ u | link(label) }}.</text:p>")),
        (
            Format::Ods,
            ods("<table:table><table:table-row><table:table-cell><text:p>See {{ u | link(label) }}.</text:p></table:table-cell></table:table-row></table:table>"),
        ),
        (
            Format::Odp,
            odp("<draw:page><draw:frame><draw:text-box><text:p>See {{ u | link(label) }}.</text:p></draw:text-box></draw:frame></draw:page>"),
        ),
    ] {
        let out = render(format, &doc, data.clone());
        let xml = part(&out, "content.xml");
        assert!(
            xml.contains(&format!(
                "See <text:a xlink:type=\"simple\" xlink:href=\"{ampersand}\">Go &lt;here&gt;</text:a>."
            )),
            "{xml}"
        );
    }
}

#[test]
fn a_link_defaults_to_showing_its_address() {
    let doc = docx(&p("{{ u | link }}"));
    let out = render(Format::Docx, &doc, json!({"u": "mailto:a@b.example"}));
    assert_eq!(docx_text(&out), ["a@b.example"]);
    let doc = docx(&p("{{ u | link }}"));
    let out = render(Format::Docx, &doc, json!({"u": "http://a.example"}));
    assert_eq!(docx_text(&out), ["http://a.example"]);
}

#[test]
fn only_web_and_mail_links_are_allowed() {
    let templates = [
        (Format::Docx, docx(&p("{{ u | link(\"x\") }}"))),
        (Format::Pptx, pptx(&[&shape(&["{{ u | link(\"x\") }}"])])),
        (
            Format::Xlsx,
            xlsx(
                &[row(1, &[cell("A1", "inlineStr", "{{ u | link(\"x\") }}")])],
                &[],
            ),
        ),
        (Format::Odt, odt("<text:p>{{ u | link(\"x\") }}</text:p>")),
    ];
    for (format, doc) in &templates {
        for bad in [
            "javascript:alert(1)",
            "JAVASCRIPT:alert(1)",
            " javascript:alert(1)",
            "java\tscript:alert(1)",
            "file:///etc/passwd",
            "data:text/html,<script>",
            "ftp://example.com",
            "/relative/path",
            "http://a.example/\"><evil",
            "",
        ] {
            let err = render_err(*format, doc, json!({"u": bad}));
            assert!(err.contains("link:"), "{bad:?} {err}");
        }
    }
}

// --- color ----------------------------------------------------------------

#[test]
fn docx_color_keeps_the_runs_properties() {
    let doc = docx(&p("A {{ s | color(\"#ff0000\", background=\"#fe0\") }} B"));
    let out = render(Format::Docx, &doc, json!({"s": "x < y"}));
    let xml = part(&out, "word/document.xml");
    assert!(
        xml.contains("<w:rPr><w:rsid w:val=\"000\"/><w:color w:val=\"FF0000\"/><w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"FFEE00\"/></w:rPr><w:t xml:space=\"preserve\">x &lt; y</w:t>"),
        "{xml}"
    );
    assert_eq!(docx_text(&out), ["A x < y B"]);
    assert_unchanged(
        &doc,
        &out,
        &["word/_rels/document.xml.rels", "[Content_Types].xml"],
    );
}

#[test]
fn docx_color_goes_in_schema_order_and_keeps_line_breaks() {
    let body = "<w:p><w:r><w:rPr><w:b/><w:sz w:val=\"20\"/></w:rPr><w:t>{{ s | color(c) }}</w:t></w:r></w:p>";
    let out = render(
        Format::Docx,
        &docx(body),
        json!({"s": "1\n2", "c": "#010203"}),
    );
    let xml = part(&out, "word/document.xml");
    assert!(
        xml.contains("<w:rPr><w:b/><w:color w:val=\"010203\"/><w:sz w:val=\"20\"/></w:rPr>"),
        "{xml}"
    );
    assert!(
        xml.contains(">1</w:t><w:br/><w:t xml:space=\"preserve\">2</w:t>"),
        "{xml}"
    );
}

#[test]
fn pptx_color_is_a_fill_on_the_run() {
    let deck = pptx(&[&shape(&[
        "{{ s | color(\"#00ff00\", background=\"#123456\") }}",
    ])]);
    let out = render(Format::Pptx, &deck, json!({"s": "ok"}));
    let xml = part(&out, "ppt/slides/slide1.xml");
    assert!(
        xml.contains("<a:r><a:rPr lang=\"en-US\"><a:solidFill><a:srgbClr val=\"00FF00\"/></a:solidFill><a:highlight><a:srgbClr val=\"123456\"/></a:highlight></a:rPr><a:t>ok</a:t></a:r>"),
        "{xml}"
    );
}

#[test]
fn odf_text_color_is_an_automatic_style_with_a_span() {
    for (format, doc) in [
        (Format::Odt, odt("<text:p>A {{ s | color(\"#ff0000\", background=\"#ffee00\") }} {{ s | color(\"#F00\", background=\"#fe0\") }}</text:p>")),
        (Format::Odp, odp("<draw:page><draw:frame><draw:text-box><text:p>A {{ s | color(\"#ff0000\", background=\"#ffee00\") }} {{ s | color(\"#F00\", background=\"#fe0\") }}</text:p></draw:text-box></draw:frame></draw:page>")),
    ] {
        let out = render(format, &doc, json!({"s": "x"}));
        let xml = part(&out, "content.xml");
        assert!(
            xml.contains("<office:automatic-styles><style:style style:name=\"RichT1\" style:family=\"text\"><style:text-properties fo:color=\"#ff0000\" fo:background-color=\"#ffee00\"/></style:style></office:automatic-styles><office:body>"),
            "one style for the same colours, before the body: {xml}"
        );
        assert_eq!(xml.matches("<text:span text:style-name=\"RichT1\">x</text:span>").count(), 2, "{xml}");
    }
}

#[test]
fn odt_color_does_not_reuse_a_style_name_in_the_template() {
    let doc = odt_styled(
        "<style:style style:name=\"RichT1\" style:family=\"text\"/>",
        "<text:p>{{ s | color(\"#111\") }}</text:p>",
    );
    let out = render(Format::Odt, &doc, json!({"s": "x"}));
    let xml = part(&out, "content.xml");
    assert!(
        xml.contains("<text:span text:style-name=\"RichT2\">x"),
        "{xml}"
    );
}

#[test]
fn xlsx_color_adds_a_font_a_fill_and_a_cell_format() {
    let book = xlsx_styled(&[row(
        1,
        &[
            "<c r=\"A1\" s=\"1\" t=\"inlineStr\"><is><t>{{ s | color(\"#ff0000\", background=\"#ffee00\") }}</t></is></c>".to_string(),
            "<c r=\"B1\" s=\"1\" t=\"inlineStr\"><is><t>{{ s | color(\"#ff0000\", background=\"#ffee00\") }}</t></is></c>".to_string(),
            "<c r=\"C1\" t=\"inlineStr\"><is><t>{{ s | color(\"#0000ff\") }}</t></is></c>".to_string(),
        ],
    )]);
    let out = render(Format::Xlsx, &book, json!({"s": "x"}));
    let sheet = part(&out, "xl/worksheets/sheet1.xml");
    let styles = part(&out, "xl/styles.xml");
    assert!(
        sheet.contains("<c r=\"A1\" s=\"2\" t=\"inlineStr\">"),
        "{sheet}"
    );
    assert!(
        sheet.contains("<c r=\"B1\" s=\"2\" t=\"inlineStr\">"),
        "the same format is reused: {sheet}"
    );
    assert!(
        sheet.contains("<c r=\"C1\" t=\"inlineStr\" s=\"3\">"),
        "{sheet}"
    );
    assert!(styles.contains("<fonts count=\"3\">"), "{styles}");
    assert!(
        styles.contains(
            "<font><b/><sz val=\"14\"/><color rgb=\"FFFF0000\"/><name val=\"Arial\"/></font>"
        ),
        "a copy of the cell's font: {styles}"
    );
    assert!(styles.contains("<fills count=\"3\">"), "{styles}");
    assert!(styles.contains("<fgColor rgb=\"FFFFEE00\"/>"), "{styles}");
    assert!(styles.contains("<cellXfs count=\"4\">"), "{styles}");
    // The number format of the cell's old format stays.
    assert!(
        styles.contains("<xf numFmtId=\"4\" fontId=\"1\" fillId=\"2\" borderId=\"0\" xfId=\"0\" applyNumberFormat=\"1\" applyFont=\"1\" applyFill=\"1\"/>"),
        "{styles}"
    );
    assert_unchanged(
        &book,
        &out,
        &["xl/_rels/workbook.xml.rels", "[Content_Types].xml"],
    );
}

#[test]
fn xlsx_color_creates_the_styles_part_when_absent() {
    let book = xlsx(
        &[row(
            1,
            &[cell("A1", "inlineStr", "{{ s | color(\"#ff0000\") }}")],
        )],
        &[],
    );
    let out = render(Format::Xlsx, &book, json!({"s": "x"}));
    let styles = part(&out, "xl/styles.xml");
    assert!(styles.contains("<styleSheet "), "{styles}");
    assert!(
        styles.find("<fonts").unwrap() < styles.find("<fills").unwrap(),
        "{styles}"
    );
    assert!(
        styles.find("<fills").unwrap() < styles.find("<borders").unwrap(),
        "{styles}"
    );
    assert!(
        styles.find("<borders").unwrap() < styles.find("<cellStyleXfs").unwrap(),
        "{styles}"
    );
    assert!(
        styles.find("<cellStyleXfs").unwrap() < styles.find("<cellXfs").unwrap(),
        "{styles}"
    );
    assert!(styles.contains("<cellXfs count=\"2\">"), "{styles}");
    let sheet = part(&out, "xl/worksheets/sheet1.xml");
    assert!(sheet.contains("s=\"1\""), "{sheet}");
    let rels = part(&out, "xl/_rels/workbook.xml.rels");
    assert!(
        rels.contains("relationships/styles\" Target=\"styles.xml\""),
        "{rels}"
    );
    let types = part(&out, "[Content_Types].xml");
    assert!(types.contains("/xl/styles.xml"), "{types}");
}

#[test]
fn ods_color_is_an_automatic_cell_style() {
    let cell = |style: &str| {
        format!("<table:table><table:table-row><table:table-cell{style}><text:p>{{{{ s | color(\"#ff0000\", background=\"#ffee00\") }}}}</text:p></table:table-cell></table:table-row></table:table>")
    };
    let out = render(Format::Ods, &ods(&cell("")), json!({"s": "x"}));
    let xml = part(&out, "content.xml");
    assert!(
        xml.contains("<table:table-cell table:style-name=\"RichC1\"><text:p>x</text:p>"),
        "{xml}"
    );
    assert!(
        xml.contains("<style:style style:name=\"RichC1\" style:family=\"table-cell\"><style:table-cell-properties fo:background-color=\"#ffee00\"/><style:text-properties fo:color=\"#ff0000\"/></style:style>"),
        "{xml}"
    );
    // An automatic base style is copied; automatic styles have no children.
    let doc = ods_styled(
        "<style:style style:name=\"ce1\" style:family=\"table-cell\" style:parent-style-name=\"Default\" style:data-style-name=\"N1\"><style:text-properties fo:font-weight=\"bold\"/></style:style>",
        &cell(" table:style-name=\"ce1\""),
    );
    let out = render(Format::Ods, &doc, json!({"s": "x"}));
    let xml = part(&out, "content.xml");
    assert!(
        xml.contains("<style:style style:name=\"RichC1\" style:family=\"table-cell\" style:parent-style-name=\"Default\" style:data-style-name=\"N1\"><style:table-cell-properties fo:background-color=\"#ffee00\"/><style:text-properties fo:font-weight=\"bold\" fo:color=\"#ff0000\"/></style:style>"),
        "{xml}"
    );
    // A named base style is inherited from.
    let out = render(
        Format::Ods,
        &ods(&cell(" table:style-name=\"Named\"")),
        json!({"s": "x"}),
    );
    let xml = part(&out, "content.xml");
    assert!(xml.contains("style:parent-style-name=\"Named\""), "{xml}");
}

#[test]
fn colours_are_validated_strictly() {
    let doc = docx(&p("{{ s | color(c) }}"));
    for bad in [
        "red",
        "#ff00",
        "#ggg",
        "ff0000",
        "#ff0000\"/><w:evil a=\"",
        "\"><x",
        "#ff0000 ",
        "",
    ] {
        let err = render_err(Format::Docx, &doc, json!({"s": "x", "c": bad}));
        assert!(err.contains("#rgb or #rrggbb"), "{bad:?} {err}");
    }
    let doc = docx(&p("{{ s | color(background=c) }}"));
    let err = render_err(Format::Docx, &doc, json!({"s": "x", "c": "x\"y"}));
    assert!(err.contains("#rgb or #rrggbb"), "{err}");
    let doc = docx(&p("{{ s | color }}"));
    let err = render_err(Format::Docx, &doc, json!({"s": "x"}));
    assert!(err.contains("give a colour"), "{err}");
    let doc = docx(&p("{{ s | color(\"#000\", size=1) }}"));
    let err = render_err(Format::Docx, &doc, json!({"s": "x"}));
    assert!(err.contains("size"), "{err}");
}

// --- html -----------------------------------------------------------------

const BODY: &str = "<h1>Title</h1><p>One <b>bold</b> <i>it</i> <u>un</u> <s>st</s> H<sub>2</sub>O x<sup>2</sup></p>\
<script>alert('x')</script><style>p{color:red}</style>\
<div onclick=\"evil()\">kept <span style=\"color:#f00;background:url(evil)\">red</span> <span style=\"color:red\">plain</span></div>\
<ul><li>first<ul><li>nested</li></ul></li><li>second</li></ul><ol><li>uno</li><li>dos</li></ol>\
<p>a<br>b <a href=\"https://example.com/?a=1&amp;b=2\">site</a></p>\
<table><tr><th>H</th><td>V</td></tr><tr><td>only</td></tr></table>";

#[test]
fn docx_html_replaces_the_paragraph_with_native_ones() {
    let body = format!(
        "{}<w:p><w:pPr><w:jc w:val=\"center\"/><w:sectPr><w:pgSz w:w=\"1\"/></w:sectPr></w:pPr><w:r><w:rPr><w:i/></w:rPr><w:t>{{{{ body | html }}}}</w:t></w:r></w:p>",
        p("Before")
    );
    let out = render(Format::Docx, &docx(&body), json!({"body": BODY}));
    let xml = part(&out, "word/document.xml");
    let text = docx_text(&out);
    assert_eq!(text[0], "Before");
    assert!(text.contains(&"Title".to_string()), "{text:?}");
    assert!(
        text.contains(&"One bold it un st H2O x2".to_string()),
        "{text:?}"
    );
    assert!(text.contains(&"kept red plain".to_string()), "{text:?}");
    assert!(text.contains(&"\u{2022}first".to_string()), "{text:?}");
    assert!(text.contains(&"\u{2022}nested".to_string()), "{text:?}");
    assert!(text.contains(&"1.uno".to_string()), "{text:?}");
    assert!(text.contains(&"2.dos".to_string()), "{text:?}");
    assert!(!xml.contains("alert"), "{xml}");
    assert!(!xml.contains("evil"), "{xml}");
    assert!(!xml.contains("color:red"), "{xml}");
    // Native formatting on top of the template's run properties.
    assert!(
        xml.contains("<w:rPr><w:b/><w:i/></w:rPr><w:t xml:space=\"preserve\">bold"),
        "{xml}"
    );
    assert!(xml.contains("<w:vertAlign w:val=\"subscript\"/>"), "{xml}");
    assert!(
        xml.contains("<w:vertAlign w:val=\"superscript\"/>"),
        "{xml}"
    );
    assert!(xml.contains("<w:strike/>"), "{xml}");
    assert!(xml.contains("<w:u w:val=\"single\"/>"), "{xml}");
    assert!(xml.contains("<w:color w:val=\"FF0000\"/>"), "{xml}");
    // Headings by bold and size, with the template's paragraph properties.
    assert!(
        xml.contains("<w:p><w:pPr><w:jc w:val=\"center\"/><w:outlineLvl w:val=\"0\"/></w:pPr><w:r><w:rPr><w:b/><w:i/><w:sz w:val=\"48\"/></w:rPr>"),
        "{xml}"
    );
    // Lists by indent and a literal marker; no numbering part.
    assert!(
        xml.contains("<w:ind w:left=\"360\" w:hanging=\"360\"/>"),
        "{xml}"
    );
    assert!(
        xml.contains("<w:ind w:left=\"720\" w:hanging=\"360\"/>"),
        "{xml}"
    );
    assert!(bytes_of(&out, "word/numbering.xml").is_none());
    // A real link and a line break.
    let rid = capture("<w:hyperlink r:id=\"(rId[0-9]+)\"", &xml);
    assert_eq!(
        rel_target(&part(&out, "word/_rels/document.xml.rels"), &rid),
        "https://example.com/?a=1&amp;b=2"
    );
    assert!(xml.contains("<w:r><w:br/></w:r>"), "{xml}");
    // A table, padded to its widest row.
    assert_eq!(xml.matches("<w:tbl>").count(), 1, "{xml}");
    assert_eq!(xml.matches("<w:tc>").count(), 4, "{xml}");
    assert_eq!(xml.matches("<w:gridCol ").count(), 2, "{xml}");
    assert!(
        xml.contains("<w:tcW w:w=\"4500\" w:type=\"dxa\"/>"),
        "{xml}"
    );
    // The section break stays once, on the last paragraph.
    assert_eq!(xml.matches("<w:pgSz w:w=\"1\"/>").count(), 1, "{xml}");
    let last_para = xml.rfind("<w:p>").unwrap();
    assert!(
        xml[last_para..].contains("<w:sectPr><w:pgSz w:w=\"1\"/>"),
        "{xml}"
    );
}

#[test]
fn docx_html_keeps_the_text_around_the_tag() {
    let doc = docx(&p("Intro {{ b | html }} outro"));
    let out = render(Format::Docx, &doc, json!({"b": "<p>one</p><p>two</p>"}));
    assert_eq!(docx_text(&out), ["Intro ", "one", "two", " outro"]);
    // Nothing to show still leaves a paragraph, which a cell requires.
    let doc = docx(&tbl(&[&["{{ b | html }}"]]));
    let out = render(Format::Docx, &doc, json!({"b": "<script>x</script>"}));
    let xml = part(&out, "word/document.xml");
    assert!(xml.contains("</w:tcPr><w:p>"), "{xml}");
    assert!(xml.contains("</w:p></w:tc>"), "{xml}");
}

#[test]
fn html_in_a_loop_gives_each_iteration_its_own_paragraphs() {
    let doc = docx(&(p("{% for b in bodies %}") + &p("{{ b | html }}") + &p("{% endfor %}")));
    let out = render(
        Format::Docx,
        &doc,
        json!({"bodies": ["<p>a</p><p>b</p>", "<p>c</p>"]}),
    );
    let text: Vec<String> = docx_text(&out)
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect();
    assert_eq!(text, ["a", "b", "c"]);
}

#[test]
fn odt_html_is_native_structure() {
    let doc = odt_styled(
        "",
        "<text:p text:style-name=\"Standard\">{{ body | html }}</text:p><text:p>End</text:p>",
    );
    let out = render(Format::Odt, &doc, json!({"body": BODY}));
    let xml = part(&out, "content.xml");
    assert!(
        xml.contains("<text:h text:style-name=\"Standard\" text:outline-level=\"1\">"),
        "{xml}"
    );
    assert!(
        xml.contains("<text:list text:style-name=\"RichBullet"),
        "{xml}"
    );
    assert!(
        xml.contains("<text:list text:style-name=\"RichNumber"),
        "{xml}"
    );
    assert!(
        xml.contains("<text:list-style style:name=\"RichBullet"),
        "{xml}"
    );
    assert!(
        xml.contains(
            "<text:list-item><text:p text:style-name=\"Standard\">first</text:p><text:list "
        ),
        "{xml}"
    );
    assert!(xml.contains("<text:line-break/>"), "{xml}");
    assert!(
        xml.contains("xlink:href=\"https://example.com/?a=1&amp;b=2\""),
        "{xml}"
    );
    assert!(xml.contains("fo:color=\"#ff0000\""), "{xml}");
    assert!(xml.contains("style:text-position=\"sub 58%\""), "{xml}");
    assert!(
        xml.contains("<table:table><table:table-column table:number-columns-repeated=\"2\"/>"),
        "{xml}"
    );
    assert!(!xml.contains("alert"), "{xml}");
    assert!(!xml.contains("evil"), "{xml}");
    let paragraphs = odf_paragraphs(&xml);
    assert!(
        paragraphs.contains(&"One bold it un st H2O x2".to_string()),
        "{paragraphs:?}"
    );
    assert_eq!(paragraphs.last().unwrap(), "End");
    // A table cannot sit in a list item.
    let out = render(
        Format::Odt,
        &odt("<text:p>{{ b | html }}</text:p>"),
        json!({"b": "<ul><li><table><tr><td>a</td><td>b</td></tr></table></li></ul>"}),
    );
    let xml = part(&out, "content.xml");
    assert!(!xml.contains("<table:table"), "{xml}");
    assert!(xml.contains(">a\tb</text:p>"), "{xml}");
}

#[test]
fn html_is_plain_text_where_there_is_no_paragraph_structure() {
    let body = "<p>one<br>two</p><script>x</script><ul><li>three</li></ul>";
    let data = json!({"b": body});
    let book = xlsx(&[row(1, &[cell("A1", "inlineStr", "{{ b | html }}")])], &[]);
    let out = render(Format::Xlsx, &book, data.clone());
    let sheet = part(&out, "xl/worksheets/sheet1.xml");
    assert!(sheet.contains("one\ntwo\n\u{2022} three"), "{sheet}");
    assert!(part(&out, "xl/styles.xml").contains("<alignment wrapText=\"1\"/>"));

    let deck = pptx(&[&shape(&["{{ b | html }}"])]);
    let out = render(Format::Pptx, &deck, data.clone());
    let xml = part(&out, "ppt/slides/slide1.xml");
    assert!(
        xml.contains("<a:t>one</a:t></a:r><a:br/><a:r><a:rPr lang=\"en-US\"/><a:t>two</a:t>"),
        "{xml}"
    );

    let out = render(
        Format::Odp,
        &odp("<draw:page><draw:frame><draw:text-box><text:p>{{ b | html }}</text:p></draw:text-box></draw:frame></draw:page>"),
        data.clone(),
    );
    let xml = part(&out, "content.xml");
    assert!(
        xml.contains("one<text:line-break/>two<text:line-break/>"),
        "{xml}"
    );
    assert!(!xml.contains("<text:list"), "{xml}");

    let out = render(
        Format::Ods,
        &ods("<table:table><table:table-row><table:table-cell><text:p>{{ b | html }}</text:p></table:table-cell></table:table-row></table:table>"),
        data,
    );
    assert!(part(&out, "content.xml").contains("one<text:line-break/>two"));
}

#[test]
fn html_limits_and_bad_links_are_template_errors() {
    let doc = docx(&p("{{ b | html }}"));
    let cases = [
        (
            json!("<a href=\"javascript:alert(1)\">x</a>"),
            "not allowed",
        ),
        (json!("<a href=\"file:///etc/passwd\">x</a>"), "not allowed"),
        (json!("<b>".repeat(65)), "deeper than 64"),
        (json!("x".repeat(1024 * 1024 + 1)), "longer than"),
    ];
    for (value, expected) in cases {
        let err = render_err(Format::Docx, &doc, json!({ "b": value }));
        assert!(err.contains(expected), "{expected:?} not in {err}");
    }
}

#[test]
fn a_huge_html_table_is_refused_in_every_format_that_expands_it() {
    // Small input: one wide row, then thousands of rows padded to its width.
    let huge = format!(
        "<table><tr>{}</tr>{}</table>",
        "<td>".repeat(250),
        "<tr><td>".repeat(9_000)
    );
    assert!(huge.len() < 100_000);
    for (format, doc) in [
        (Format::Docx, docx(&p("{{ b | html }}"))),
        (Format::Odt, odt("<text:p>{{ b | html }}</text:p>")),
        (Format::Pptx, pptx(&[&shape(&["{{ b | html }}"])])),
    ] {
        let err = render_err(format, &doc, json!({ "b": huge }));
        assert!(err.contains("at most 50000"), "{err}");
    }
    let err = render_err(
        Format::Docx,
        &docx(&p("{{ b | html }}")),
        json!({ "b": format!("<table>{}</table>", "<tr><td>".repeat(10_001)) }),
    );
    assert!(err.contains("at most 10000 are allowed"), "{err}");
    // Values that pass one at a time add up over a render.
    let doc = docx(&p("{% for b in bodies %}{{ b | html }}{% endfor %}"));
    let table = format!(
        "<table>{}</table>",
        format!("<tr>{}</tr>", "<td>".repeat(100)).repeat(400)
    );
    let err = render_err(Format::Docx, &doc, json!({ "bodies": vec![table; 3] }));
    assert!(err.contains("cells in all"), "{err}");
}

#[test]
fn generated_html_markup_counts_against_the_package_limit() {
    // Every generated paragraph repeats the template paragraph's properties,
    // so 1 MB of short paragraphs would come to about 375 MB.
    let filler = "x".repeat(3000);
    let body = format!(
        "<w:p><w:pPr><w:pStyle w:val=\"{filler}\"/></w:pPr><w:r><w:t>{{{{ b | html }}}}</w:t></w:r></w:p>"
    );
    let html = "<p>x</p>".repeat(1024 * 1024 / 8);
    let result = compile(Format::Docx, &docx(&body))
        .unwrap()
        .render(&json!({ "b": html }));
    let err = match result {
        Ok(_) => panic!("must not render"),
        Err(e) => e,
    };
    assert!(err.message.contains("package limit"), "{err}");
}

#[test]
fn an_oversized_barcode_size_stays_within_the_pixel_limit() {
    let doc = docx(&p("{{ v | barcode(\"code128\", size=14400) }}"));
    let out = render(Format::Docx, &doc, json!({"v": "x".repeat(360)}));
    let png = bytes_of(&out, "word/media/rich1.png").unwrap();
    let side = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as u64;
    assert!(
        side(16) * side(20) <= 4_000_000,
        "{} x {}",
        side(16),
        side(20)
    );
    let xml = part(&out, "word/document.xml");
    assert!(
        xml.contains("<wp:extent cx=\"182880000\" "),
        "the printed size holds: {xml}"
    );
}

#[test]
fn many_values_in_one_run_stay_fast() {
    // An inline loop puts every marker in one text node; handling each must
    // not copy all of it. Quadratic work here takes minutes, not moments.
    let url = "https://a.example";
    let data = json!({ "n": 20_000, "u": url, "b": "<p>x</p>", "t": "y".repeat(100) });
    let started = std::time::Instant::now();
    let loop_of = |tag: &str| format!("{{% for i in range(n) %}}{{{{ t }}}}{tag}{{% endfor %}}");
    let tag = loop_of("{{ u | link(\"x\") }}{{ b | html }}");
    render(Format::Docx, &docx(&p(&tag)), data.clone());
    render(
        Format::Odt,
        &odt(&format!("<text:p>{tag}</text:p>")),
        data.clone(),
    );
    render(
        Format::Pptx,
        &pptx(&[&shape(&[&loop_of("{{ u | link(\"x\") }}")])]),
        data,
    );
    assert!(started.elapsed().as_secs() < 60, "{:?}", started.elapsed());
}

#[test]
fn a_long_value_printed_in_a_loop_hits_the_print_budget() {
    // Each print places the whole text; printing 100 KiB 3,000 times must not
    // reach 300 MB of markup.
    let doc = docx(&p(
        "{% set c = t | color(\"#ff0000\") %}{% for i in range(3000) %}{{ c }}{% endfor %}",
    ));
    let err = render_err(Format::Docx, &doc, json!({ "t": "y".repeat(100 * 1024) }));
    assert!(err.contains("package limit"), "{err}");
}

#[test]
fn one_chart_printed_many_times_counts_each_placement() {
    let doc = docx(&p(
        "{% set c = rows | chart(type=\"bar\", x=\"month\", y=\"sales\") %}{% for i in range(201) %}{{ c }}{% endfor %}",
    ));
    let err = compile(Format::Docx, &doc)
        .expect("compiles")
        .render(&sales())
        .expect_err("must fail");
    assert!(err.message.contains("200 charts"), "{err}");
}

// --- chart ----------------------------------------------------------------

fn sales() -> Value {
    json!({"rows": [
        {"month": "Jan & <Feb>", "sales": 10, "costs": 4.5},
        {"month": "Mar", "sales": 20.25, "costs": null},
    ]})
}

const CHART_TAG: &str =
    "{{ rows | chart(type=\"bar\", x=\"month\", y=[\"sales\",\"costs\"], title=\"Rev & <Co>\") }}";

fn assert_chart_values(chart: &str) {
    assert!(chart.contains("<c:chartSpace "), "{chart}");
    assert!(chart.contains("<c:barDir val=\"bar\"/>"), "{chart}");
    assert!(chart.contains("<a:t>Rev &amp; &lt;Co&gt;</a:t>"), "{chart}");
    assert!(
        chart.contains("<c:pt idx=\"0\"><c:v>Jan &amp; &lt;Feb&gt;</c:v></c:pt>"),
        "{chart}"
    );
    assert!(
        chart.contains("<c:strCache><c:ptCount val=\"2\"/>"),
        "{chart}"
    );
    assert!(chart.contains("<c:numCache><c:formatCode>General</c:formatCode><c:ptCount val=\"2\"/><c:pt idx=\"0\"><c:v>10</c:v></c:pt><c:pt idx=\"1\"><c:v>20.25</c:v></c:pt></c:numCache>"), "{chart}");
    // A null is a gap, not a zero.
    assert!(
        chart.contains("<c:ptCount val=\"2\"/><c:pt idx=\"0\"><c:v>4.5</c:v></c:pt></c:numCache>"),
        "{chart}"
    );
    assert!(chart.contains("<c:v>costs</c:v>"), "{chart}");
    assert!(
        !chart.contains("externalData"),
        "no embedded workbook: {chart}"
    );
}

#[test]
fn docx_chart_is_a_native_part() {
    let doc = docx(&p(&format!("Before {CHART_TAG}")));
    let out = render(Format::Docx, &doc, sales());
    assert_chart_values(&part(&out, "word/charts/chart1.xml"));
    let xml = part(&out, "word/document.xml");
    let rid = capture("<c:chart r:id=\"(rId[0-9]+)\"/>", &xml);
    assert!(
        xml.contains("xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\""),
        "{xml}"
    );
    assert!(
        xml.contains(
            "<a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/chart\">"
        ),
        "{xml}"
    );
    let rels = part(&out, "word/_rels/document.xml.rels");
    assert!(rels.contains(CHART_REL), "{rels}");
    assert_eq!(rel_target(&rels, &rid), "charts/chart1.xml");
    let types = part(&out, "[Content_Types].xml");
    assert!(
        types.contains(&format!(
            "PartName=\"/word/charts/chart1.xml\" ContentType=\"{CHART_CT}\""
        )),
        "{types}"
    );
    assert_eq!(docx_text(&out), ["Before "]);
    assert_unchanged(&doc, &out, &["word/header1.xml"]);
}

#[test]
fn pptx_chart_is_a_frame_at_the_shape_of_its_tag() {
    let tag = shape(&[CHART_TAG]).replace(
        "<p:spPr/>",
        "<p:spPr><a:xfrm><a:off x=\"5\" y=\"6\"/><a:ext cx=\"1\" cy=\"1\"/></a:xfrm></p:spPr>",
    );
    let deck = pptx(&[&tag]);
    let out = render(Format::Pptx, &deck, sales());
    assert_chart_values(&part(&out, "ppt/charts/chart1.xml"));
    let xml = part(&out, "ppt/slides/slide1.xml");
    assert!(
        xml.contains(
            "<p:xfrm><a:off x=\"5\" y=\"6\"/><a:ext cx=\"5715000\" cy=\"3429000\"/></p:xfrm>"
        ),
        "{xml}"
    );
    let rid = capture("<c:chart r:id=\"(rId[0-9]+)\"/>", &xml);
    let rels = part(&out, "ppt/slides/_rels/slide1.xml.rels");
    assert_eq!(rel_target(&rels, &rid), "../charts/chart1.xml");
    assert!(part(&out, "[Content_Types].xml").contains("/ppt/charts/chart1.xml"));
}

#[test]
fn xlsx_chart_reads_a_hidden_sheet_holding_the_same_values() {
    let book = xlsx(&[row(3, &[cell("B3", "inlineStr", CHART_TAG)])], &[]);
    let out = render(Format::Xlsx, &book, sales());
    let chart = part(&out, "xl/charts/chart1.xml");
    assert_chart_values(&chart);
    assert!(
        chart.contains("<c:f>'RichChartData'!$A$2:$A$3</c:f>"),
        "{chart}"
    );
    assert!(
        chart.contains("<c:f>'RichChartData'!$B$2:$B$3</c:f>"),
        "{chart}"
    );
    assert!(chart.contains("<c:f>'RichChartData'!$C$1</c:f>"), "{chart}");
    let drawing = part(&out, "xl/drawings/drawing1.xml");
    assert!(drawing.contains("<xdr:col>1</xdr:col>"), "{drawing}");
    assert!(drawing.contains("<xdr:row>2</xdr:row>"), "{drawing}");
    assert!(drawing.contains("<xdr:graphicFrame "), "{drawing}");
    let rid = capture("<c:chart r:id=\"(rId[0-9]+)\"/>", &drawing);
    let rels = part(&out, "xl/drawings/_rels/drawing1.xml.rels");
    assert_eq!(rel_target(&rels, &rid), "../charts/chart1.xml");
    let types = part(&out, "[Content_Types].xml");
    assert!(types.contains("/xl/charts/chart1.xml"), "{types}");
    assert!(types.contains("/xl/worksheets/richdata1.xml"), "{types}");
    let workbook = part(&out, "xl/workbook.xml");
    assert!(
        workbook.contains("<sheet name=\"RichChartData\" sheetId=\"2\" state=\"hidden\" r:id="),
        "{workbook}"
    );
    let data = part(&out, "xl/worksheets/richdata1.xml");
    assert!(data.contains("Jan &amp; &lt;Feb&gt;"), "{data}");
    assert!(data.contains("<c r=\"B3\"><v>20.25</v></c>"), "{data}");
    assert!(!data.contains("<f>"), "{data}");
    assert!(
        !data.contains("<c r=\"C3\">"),
        "a null writes no cell: {data}"
    );
    assert_unchanged(&book, &out, &["xl/sharedStrings.xml"]);
}

#[test]
fn chart_kinds() {
    for (kind, expected) in [
        ("column", "<c:barDir val=\"col\"/>"),
        ("bar", "<c:barDir val=\"bar\"/>"),
        ("line", "<c:lineChart>"),
        ("pie", "<c:pieChart>"),
    ] {
        let doc = docx(&p(&format!(
            "{{{{ rows | chart(type=\"{kind}\", x=\"month\", y=\"sales\") }}}}"
        )));
        let out = render(Format::Docx, &doc, sales());
        let chart = part(&out, "word/charts/chart1.xml");
        assert!(chart.contains(expected), "{kind}: {chart}");
        assert_eq!(chart.contains("<c:catAx>"), kind != "pie", "{chart}");
    }
}

#[test]
fn charts_refuse_bad_values() {
    let tag = |args: &str| docx(&p(&format!("{{{{ rows | chart({args}) }}}}")));
    let good = "type=\"bar\", x=\"m\", y=\"v\"";
    let rows = |rows: Value| json!({ "rows": rows });
    let points: Vec<Value> = (0..1001).map(|i| json!({"m": "a", "v": i})).collect();
    let cases = [
        (good, rows(json!(points)), "at most 1000"),
        (good, rows(json!([])), "no rows"),
        (good, rows(json!("nope")), "list of objects"),
        (good, rows(json!([1])), "not an object"),
        (good, rows(json!([{"m": "a", "v": "12"}])), "not a number"),
        (good, rows(json!([{"m": "a", "v": true}])), "not a number"),
        (good, rows(json!([{"m": "a"}])), "no row has a value"),
        (good, rows(json!([{"v": 1}])), "has no \"m\""),
        (
            "type=\"radar\", x=\"m\", y=\"v\"",
            rows(json!([])),
            "type must be",
        ),
        ("x=\"m\", y=\"v\"", rows(json!([])), "type= is required"),
        ("type=\"bar\", y=\"v\"", rows(json!([])), "x= is required"),
        ("type=\"bar\", x=\"m\"", rows(json!([])), "y= is required"),
        (
            "type=\"pie\", x=\"m\", y=[\"a\", \"b\"]",
            rows(json!([])),
            "one y key",
        ),
        (
            "type=\"bar\", x=\"m\", y=\"v\", depth=1",
            rows(json!([])),
            "depth",
        ),
    ];
    for (args, data, expected) in cases {
        let err = render_err(Format::Docx, &tag(args), data);
        assert!(err.contains(expected), "{expected:?} not in {err}");
    }
    // A non-finite number can come from a template expression.
    let doc = docx(&p(
        "{{ [{\"m\": \"a\", \"v\": 1e308 * 10}] | chart(type=\"bar\", x=\"m\", y=\"v\") }}",
    ));
    let err = render_err(Format::Docx, &doc, json!({}));
    assert!(err.contains("finite"), "{err}");
    // Long labels repeated once per series pass the generated-size budget.
    let label = "x".repeat(400 * 1024);
    let wide = docx(&p(&format!(
        "{{{{ rows | chart(type=\"bar\", x=\"m\", y=[{}]) }}}}",
        (0..50)
            .map(|i| format!("\"s{i}\""))
            .collect::<Vec<_>>()
            .join(",")
    )));
    let row: serde_json::Map<String, Value> = std::iter::once(("m".to_string(), json!(label)))
        .chain((0..50).map(|i| (format!("s{i}"), json!(1))))
        .collect();
    let err = render_err(Format::Docx, &wide, json!({"rows": [row]}));
    assert!(err.contains("repeated"), "{err}");

    // 1,000 points is the limit, not an error.
    let points: Vec<Value> = (0..1000).map(|i| json!({"m": "a", "v": i})).collect();
    let out = render(Format::Docx, &tag(good), rows(json!(points)));
    assert!(bytes_of(&out, "word/charts/chart1.xml").is_some());
}

fn odf_chart_docs(tag: &str) -> [(Format, Vec<u8>); 3] {
    let tag = escape(tag);
    [
        (Format::Odt, odt(&format!("<text:p>Before {tag} after</text:p>"))),
        (
            Format::Ods,
            ods(&format!("<table:table><table:table-row><table:table-cell><text:p>{tag}</text:p></table:table-cell></table:table-row></table:table>")),
        ),
        (
            Format::Odp,
            odp(&format!("<draw:page><draw:frame svg:x=\"2cm\" svg:y=\"3cm\"><draw:text-box><text:p>{tag}</text:p></draw:text-box></draw:frame></draw:page>")),
        ),
    ]
}

#[test]
fn odf_charts_are_embedded_chart_objects() {
    for (format, doc) in odf_chart_docs(CHART_TAG) {
        let out = render(format, &doc, sales());
        let content = part(&out, "content.xml");
        assert!(
            content.contains(
                "svg:width=\"450.00pt\" svg:height=\"270.00pt\"><draw:object xlink:href=\"./Object 1\""
            ),
            "{format:?}: {content}"
        );
        assert!(!content.contains('\u{1}'), "{content}");
        match format {
            Format::Odt => assert!(
                content.contains("text:anchor-type=\"as-char\""),
                "{content}"
            ),
            Format::Ods => assert!(
                content.contains("<table:table-cell><draw:frame"),
                "the frame goes on the cell: {content}"
            ),
            _ => assert!(
                content.contains("svg:x=\"2cm\" svg:y=\"3cm\" svg:width=\"450.00pt\""),
                "the frame sits where the tag's shape is: {content}"
            ),
        }
        let manifest = part(&out, "META-INF/manifest.xml");
        for (path, mime) in [
            ("Object 1/", "application/vnd.oasis.opendocument.chart"),
            ("Object 1/content.xml", "text/xml"),
            ("Object 1/styles.xml", "text/xml"),
        ] {
            assert!(
                manifest.contains(&format!(
                    "manifest:full-path=\"{path}\" manifest:media-type=\"{mime}\""
                )),
                "{manifest}"
            );
        }
        assert!(part(&out, "Object 1/styles.xml").contains("<office:document-styles"));
        let chart = part(&out, "Object 1/content.xml");
        // Horizontal bars, the title and names escaped, two coloured series.
        assert!(chart.contains("chart:class=\"chart:bar\""), "{chart}");
        assert!(chart.contains("chart:vertical=\"true\""), "{chart}");
        assert!(
            chart.contains("<text:p>Rev &amp; &lt;Co&gt;</text:p></chart:title>"),
            "{chart}"
        );
        assert!(
            chart.contains("<text:p>Jan &amp; &lt;Feb&gt;</text:p>"),
            "{chart}"
        );
        assert!(
            chart.contains("table:cell-range-address=\"local-table.$A$1:.$C$3\""),
            "{chart}"
        );
        assert!(chart.contains("chart:values-cell-range-address=\"local-table.$C$2:.$C$3\" chart:label-cell-address=\"local-table.$C$1\""), "{chart}");
        assert!(
            chart.contains("office:value-type=\"float\" office:value=\"20.25\""),
            "{chart}"
        );
        // The missing cost is a gap.
        assert!(
            chart.contains("<table:table-cell/></table:table-row>"),
            "{chart}"
        );
        assert!(chart.contains("draw:fill-color=\"#4472c4\""), "{chart}");
        assert!(chart.contains("draw:fill-color=\"#ed7d31\""), "{chart}");
        assert!(
            !chart.contains("xlink:href=\"..") && !chart.contains("cell-range-address=\"Sheet"),
            "the chart reads only its own table: {chart}"
        );
    }
}

#[test]
fn odf_chart_kinds() {
    for (kind, class, vertical) in [
        ("column", "chart:bar", "false"),
        ("line", "chart:line", "false"),
        ("pie", "chart:circle", "false"),
    ] {
        let tag = format!("{{{{ rows | chart(type=\"{kind}\", x=\"month\", y=\"sales\") }}}}");
        let out = render(
            Format::Odt,
            &odt(&format!("<text:p>{tag}</text:p>")),
            sales(),
        );
        let chart = part(&out, "Object 1/content.xml");
        assert!(
            chart.contains(&format!("<chart:chart svg:width=\"450.00pt\" svg:height=\"270.00pt\" chart:class=\"{class}\"")),
            "{kind}: {chart}"
        );
        assert!(
            chart.contains(&format!("chart:vertical=\"{vertical}\"")),
            "{chart}"
        );
        assert_eq!(chart.contains("<chart:grid"), kind != "pie", "{chart}");
        assert!(
            !chart.contains("<chart:title"),
            "no title was given: {chart}"
        );
        if kind == "pie" {
            // Each slice has a colour of its own.
            assert!(
                chart.contains("<chart:data-point chart:style-name=\"chS0\"/><chart:data-point chart:style-name=\"chS1\"/>"),
                "{chart}"
            );
        }
        if kind == "line" {
            assert!(chart.contains("chart:symbol-type=\"automatic\""), "{chart}");
        }
    }
}

#[test]
fn odf_charts_take_sizes_and_never_overwrite_an_object() {
    let tag = "{{ rows | chart(type=\"line\", x=\"month\", y=\"sales\", width=200, height=100) }} {{ rows | chart(type=\"pie\", x=\"month\", y=\"sales\") }}";
    let doc = odt(&format!("<text:p>{tag}</text:p>"));
    // The template already embeds an object under the first free name.
    let mut files = entries(&doc);
    files.push(("Object 1/content.xml".to_string(), b"<x/>".to_vec()));
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();
    let out = render(Format::Odt, &zip(&refs), sales());
    assert_eq!(bytes_of(&out, "Object 1/content.xml").unwrap(), b"<x/>");
    let content = part(&out, "content.xml");
    assert!(
        content.contains(
            "svg:width=\"200.00pt\" svg:height=\"100.00pt\"><draw:object xlink:href=\"./Object 2\""
        ),
        "{content}"
    );
    assert!(content.contains("xlink:href=\"./Object 3\""), "{content}");
    assert!(part(&out, "Object 2/content.xml").contains("chart:line"));
    assert!(part(&out, "Object 3/content.xml").contains("chart:circle"));
}

#[test]
fn odf_charts_count_against_the_chart_limit() {
    let doc = odt("<text:p>{% set c = rows | chart(type=\"bar\", x=\"month\", y=\"sales\") %}{% for i in range(201) %}{{ c }}{% endfor %}</text:p>");
    let err = compile(Format::Odt, &doc)
        .unwrap()
        .render(&sales())
        .unwrap_err();
    assert!(err.message.contains("200 charts"), "{err}");
}

// --- shift ----------------------------------------------------------------

/// A text box at `(x, y)` EMU holding one paragraph per entry.
fn card(id: u32, x: i64, y: i64, paras: &[&str]) -> String {
    let mut s = format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"Card {id}\"/><p:cNvSpPr txBox=\"1\"/><p:nvPr/>\
         </p:nvSpPr><p:spPr><a:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"2000000\" cy=\"500000\"/>\
         </a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr/>\
         <a:lstStyle/>"
    );
    for para in paras {
        s.push_str(&format!(
            "<a:p><a:r><a:rPr lang=\"en-US\"/><a:t>{}</a:t></a:r></a:p>",
            escape(para)
        ));
    }
    s.push_str("</p:txBody></p:sp>");
    s
}

/// `(x, y)` of every shape offset on a slide, in order.
fn offsets(slide: &str) -> Vec<(i64, i64)> {
    Regex::new("<a:off x=\"(-?[0-9]+)\" y=\"(-?[0-9]+)\"/>")
        .unwrap()
        .captures_iter(slide)
        .map(|c| (c[1].parse().unwrap(), c[2].parse().unwrap()))
        .collect()
}

fn cards_data() -> Value {
    json!({"cards": [
        {"name": "Ada", "role": "Engineer"},
        {"name": "Grace", "role": "Admiral"},
        {"name": "Linus", "role": "Kernel"},
    ]})
}

#[test]
fn shift_repeats_the_shapes_of_a_loop_and_moves_each_copy() {
    // A name box and a role box beside it, one card per item, 2 cm apart.
    let deck = pptx(&[&(card(
        5,
        100,
        200,
        &["{% for c in cards | shift(y=\"2cm\") %}{{ c.name }}"],
    ) + &card(6, 3000000, 200, &["{{ c.role }}{% endfor %}"])
        + &card(7, 0, 6000000, &["Footer"]))]);
    let out = render(Format::Pptx, &deck, cards_data());
    assert_eq!(slide_order(&out).len(), 1, "the slide is not repeated");
    let slide = part(&out, "ppt/slides/slide1.xml");
    assert!(!slide.contains("rusticdoc"), "{slide}");
    assert_eq!(
        slide_text(&out, "ppt/slides/slide1.xml"),
        ["Ada", "Engineer", "Grace", "Admiral", "Linus", "Kernel", "Footer"]
    );
    assert_eq!(
        offsets(&slide),
        [
            (100, 200),
            (3000000, 200),
            (100, 720200),
            (3000000, 720200),
            (100, 1440200),
            (3000000, 1440200),
            (0, 6000000),
        ]
    );
    // Every shape keeps an id of its own.
    let ids: Vec<String> = Regex::new("<p:cNvPr id=\"([0-9]+)\"")
        .unwrap()
        .captures_iter(&slide)
        .map(|c| c[1].to_string())
        .collect();
    let unique: std::collections::HashSet<&String> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len(), "{ids:?}");
}

#[test]
fn shift_in_one_text_box_repeats_the_box_across_the_slide() {
    let deck = pptx(&[&card(
        5,
        0,
        0,
        &["{% for c in cards | shift(x=\"3in\") %}{{ c.name }}{% endfor %}"],
    )]);
    let out = render(Format::Pptx, &deck, cards_data());
    let slide = part(&out, "ppt/slides/slide1.xml");
    assert_eq!(
        offsets(&slide),
        [(0, 0), (2743200, 0), (5486400, 0)],
        "{slide}"
    );
    assert_eq!(
        slide_text(&out, "ppt/slides/slide1.xml"),
        ["Ada", "Grace", "Linus"]
    );
    // An empty list leaves no shape, and an outer slide loop still works.
    let out = render(Format::Pptx, &deck, json!({"cards": []}));
    assert!(offsets(&part(&out, "ppt/slides/slide1.xml")).is_empty());
}

#[test]
fn shifted_loops_nest_into_a_grid() {
    let deck = pptx(&[&card(
        5,
        0,
        0,
        &["{% for row in rows | shift(y=\"1cm\") %}{% for v in row | shift(x=\"2cm\") %}{{ v }}{% endfor %}{% endfor %}"],
    )]);
    let out = render(
        Format::Pptx,
        &deck,
        json!({"rows": [["a", "b"], ["c", "d"]]}),
    );
    let slide = part(&out, "ppt/slides/slide1.xml");
    assert_eq!(
        offsets(&slide),
        [(0, 0), (720000, 0), (0, 360000), (720000, 360000)]
    );
    assert_eq!(
        slide_text(&out, "ppt/slides/slide1.xml"),
        ["a", "b", "c", "d"]
    );
}

#[test]
fn a_picture_in_a_shifted_copy_sits_at_the_moved_shape() {
    let deck = pptx(&[&card(
        5,
        1000,
        2000,
        &["{% for c in cards | shift(y=\"1in\") %}{{ logo | image(width=10, height=10) }}{% endfor %}"],
    )]);
    let mut data = cards_data();
    data["logo"] = json!(b64(&logo()));
    let out = render(Format::Pptx, &deck, data);
    let slide = part(&out, "ppt/slides/slide1.xml");
    let pics: Vec<(i64, i64)> = Regex::new("(?s)<p:pic>.*?<a:off x=\"([0-9]+)\" y=\"([0-9]+)\"/>")
        .unwrap()
        .captures_iter(&slide)
        .map(|c| (c[1].parse().unwrap(), c[2].parse().unwrap()))
        .collect();
    assert_eq!(pics, [(1000, 2000), (1000, 916400), (1000, 1830800)]);
}

#[test]
fn a_placeholder_without_a_position_takes_its_layouts_before_moving() {
    let title = "<p:sp><p:nvSpPr><p:cNvPr id=\"2\" name=\"Title 1\"/><p:cNvSpPr/><p:nvPr><p:ph type=\"title\"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{% for c in cards | shift(y=\"1cm\") %}{{ c.name }}{% endfor %}</a:t></a:r></a:p></p:txBody></p:sp>";
    let deck = pptx(&[title]);
    let layout = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<p:sldLayout xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id=\"2\" name=\"Title 1\"/><p:cNvSpPr/><p:nvPr><p:ph type=\"title\"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x=\"500\" y=\"600\"/><a:ext cx=\"7000\" cy=\"800\"/></a:xfrm></p:spPr></p:sp></p:spTree></p:cSld></p:sldLayout>";
    let mut files = entries(&deck);
    files.push((
        "ppt/slideLayouts/slideLayout1.xml".to_string(),
        layout.as_bytes().to_vec(),
    ));
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();
    let out = render(Format::Pptx, &zip(&refs), cards_data());
    let slide = part(&out, "ppt/slides/slide1.xml");
    // The first copy keeps inheriting; the others get the layout's box, moved.
    assert_eq!(offsets(&slide), [(500, 360600), (500, 720600)], "{slide}");
    assert!(slide.contains("<a:ext cx=\"7000\" cy=\"800\"/>"), "{slide}");
}

#[test]
fn shift_mistakes_are_located_errors() {
    let compile_err = |deck: Vec<u8>| {
        let err = compile(Format::Pptx, &deck).expect_err("must not compile");
        assert!(err.location.is_some(), "{err}");
        err.to_string()
    };
    let one = |text: &str| pptx(&[&card(5, 0, 0, &[text])]);
    let err = compile_err(one(
        "{% for c in cards | shift(y=\"2\") %}{{ c }}{% endfor %}",
    ));
    assert!(
        err.contains("no unit") && err.contains("slide1.xml"),
        "{err}"
    );
    let err = compile_err(one(
        "{% for c in cards | shift(y=gap) %}{{ c }}{% endfor %}",
    ));
    assert!(err.contains("cannot come from data"), "{err}");
    let err = compile_err(one(
        "{% for c in cards | shift(y=\"1cm\") %}{{ c }}{% else %}none{% endfor %}",
    ));
    assert!(
        err.contains("cannot sit in a loop that moves shapes"),
        "{err}"
    );
    let err = compile_err(pptx(&[
        &card(5, 0, 0, &["{% for c in cards | shift(y=\"1cm\") %}"]),
        &card(5, 0, 0, &["{% endfor %}"]),
    ]));
    assert!(err.contains("different slides"), "{err}");
    // Other formats refuse the filter when it runs.
    let doc = docx(&p(
        "{% for c in cards | shift(y=\"1cm\") %}{{ c.name }}{% endfor %}",
    ));
    let err = render_err(Format::Docx, &doc, cards_data());
    assert!(err.contains("does not work in DOCX"), "{err}");
    let doc = odp("<draw:page><draw:frame><draw:text-box><text:p>{% for c in cards | shift(y=\"1cm\") %}{{ c.name }}{% endfor %}</text:p></draw:text-box></draw:frame></draw:page>");
    let err = render_err(Format::Odp, &doc, cards_data());
    assert!(err.contains("does not work in ODP"), "{err}");
    let err = environment()
        .render_str("{% for c in [] | shift(y=\"1cm\") %}{% endfor %}", ())
        .unwrap_err();
    assert!(err.to_string().contains("Office template"), "{err}");
}

#[test]
fn validate_checks_shift_like_a_render() {
    let deck = pptx(&[&card(
        5,
        0,
        0,
        &["{% for c in cards | shift(y=\"1cm\") %}{{ c.name }}{% endfor %}"],
    )]);
    assert!(validate(Format::Pptx, &deck, &cards_data()).is_empty());
    let deck = pptx(&[&card(
        5,
        0,
        0,
        &["{% for c in cards | shift(down=\"1cm\") %}{{ c.name }}{% endfor %}"],
    )]);
    let diags = validate(Format::Pptx, &deck, &cards_data());
    assert!(diags[0].message.contains("not down="), "{diags:?}");
    assert_eq!(
        diags[0].location.as_ref().unwrap().snippet,
        "{% for c in cards | shift(down=\"1cm\") %}"
    );
}

// --- pagebreak ------------------------------------------------------------

#[test]
fn docx_pagebreak() {
    let doc = docx(&(p("{% for i in items %}{{ i }}{{ \"\" | pagebreak }}{% endfor %}")));
    let out = render(Format::Docx, &doc, json!({"items": ["a", "b"]}));
    let xml = part(&out, "word/document.xml");
    assert_eq!(xml.matches("<w:br w:type=\"page\"/>").count(), 2, "{xml}");
    assert_eq!(docx_text(&out), ["ab"]);
}

#[test]
fn odt_pagebreak_ends_or_splits_a_paragraph() {
    // Alone in a paragraph: the next content starts the new page.
    let doc = odt_styled(
        "<style:style style:name=\"P1\" style:family=\"paragraph\" style:parent-style-name=\"Standard\"/>",
        "<text:p text:style-name=\"P1\">{{ \"\" | pagebreak }}</text:p><text:p>next</text:p>",
    );
    let out = render(Format::Odt, &doc, json!({}));
    let xml = part(&out, "content.xml");
    assert!(
        xml.contains("<style:style style:name=\"RichP1\" style:family=\"paragraph\" style:parent-style-name=\"Standard\"><style:paragraph-properties fo:break-after=\"page\"/></style:style>"),
        "{xml}"
    );
    assert!(
        xml.contains("<text:p text:style-name=\"RichP1\"></text:p><text:p>next</text:p>"),
        "{xml}"
    );

    // In mid text: the text after it moves to a new paragraph on a new page.
    let doc = odt("<text:p>one <text:span text:style-name=\"B\">two{{ \"\" | pagebreak }}three</text:span> four</text:p>");
    let out = render(Format::Odt, &doc, json!({}));
    let xml = part(&out, "content.xml");
    assert!(xml.contains("fo:break-before=\"page\""), "{xml}");
    assert!(
        xml.contains("<text:p>one <text:span text:style-name=\"B\">two</text:span></text:p><text:p text:style-name=\"RichP1\"><text:span text:style-name=\"B\">three</text:span> four</text:p>"),
        "{xml}"
    );
}

#[test]
fn page_breaks_exist_only_in_documents() {
    let tag = "{{ \"\" | pagebreak }}";
    for (format, doc) in [
        (Format::Pptx, pptx(&[&shape(&[tag])])),
        (Format::Xlsx, xlsx(&[row(1, &[cell("A1", "inlineStr", tag)])], &[])),
        (
            Format::Ods,
            ods(&format!("<table:table><table:table-row><table:table-cell><text:p>{tag}</text:p></table:table-cell></table:table-row></table:table>")),
        ),
    ] {
        let err = render_err(format, &doc, json!({}));
        assert!(err.contains("page breaks exist only"), "{err}");
    }
}

// --- markers --------------------------------------------------------------

/// What a marker for table entry 0 looks like; a test detail on purpose.
const FORGED: &str = "\u{1}\u{10}\u{2}";

#[test]
fn data_cannot_forge_a_marker() {
    let forged = format!("a{FORGED}b");
    for (tag, expected) in [
        ("{{ evil }}", "ab"),
        ("{{ evil | safe }}", "ab"),
        ("{{ evil | upper }}", "AB"),
        ("{% autoescape false %}{{ evil }}{% endautoescape %}", "ab"),
        ("{{ [evil] | join(\"\") }}", "ab"),
        ("{% set x = evil %}{{ x }}", "ab"),
    ] {
        // A real image is registered first, so entry 0 exists to be stolen.
        let doc = docx(&p(&format!("{{{{ logo | image }}}}|{tag}")));
        let mut data = logo_data();
        data["evil"] = json!(forged);
        let out = render(Format::Docx, &doc, data);
        let xml = part(&out, "word/document.xml");
        assert_eq!(xml.matches("<w:drawing>").count(), 1, "{tag}: {xml}");
        assert_eq!(docx_text(&out), [format!("|{expected}")], "{tag}");
        assert!(!xml.contains('\u{1}'), "{tag}: {xml}");
    }
}

#[test]
fn data_passed_to_rich_filters_cannot_forge_a_marker() {
    let doc = docx(&p(
        "{{ logo | image }}{{ \"http://a.example\" | link(evil) }}{{ evil | color(\"#000\") }}{{ html | html }}",
    ));
    let mut data = logo_data();
    data["evil"] = json!(format!("x{FORGED}y"));
    data["html"] = json!(format!("<p>z{FORGED}w&#1;&#16;&#2;</p>"));
    let out = render(Format::Docx, &doc, data);
    let xml = part(&out, "word/document.xml");
    assert_eq!(xml.matches("<w:drawing>").count(), 1, "{xml}");
    for bad in ['\u{1}', '\u{2}', '\u{10}'] {
        assert!(!xml.contains(bad), "{xml}");
    }
    assert!(xml.contains(">xy<"), "{xml}");
    assert!(xml.contains(">zw<"), "{xml}");
}

#[test]
fn a_rich_value_cannot_go_through_a_string_path() {
    for tag in [
        "{{ logo | image ~ \"x\" }}",
        "{{ logo | image | upper }}",
        "{{ logo | image | string }}",
        "{% macro m(v) %}{{ v | image }}{% endmacro %}{{ m(logo) }}",
        "{% set x %}{{ logo | image }}{% endset %}{{ x }}",
        "{% filter upper %}{{ logo | image }}{% endfilter %}",
    ] {
        let doc = docx(&p(tag));
        let result = compile(Format::Docx, &doc).unwrap().render(&logo_data());
        let err = match result {
            Ok(_) => panic!("{tag} must not render"),
            Err(e) => e,
        };
        assert!(err.message.contains("printed directly"), "{tag}: {err}");
    }
    // Set to a variable and printed later, or not at all, is fine.
    let doc = docx(&p("{% set x = logo | image %}{{ x }}"));
    let out = render(Format::Docx, &doc, logo_data());
    assert!(part(&out, "word/document.xml").contains("<w:drawing>"));
    let doc = docx(&p("{% set x = logo | image %}done"));
    let out = render(Format::Docx, &doc, logo_data());
    assert_eq!(docx_text(&out), ["done"]);
    assert!(bytes_of(&out, "word/media/rich1.png").is_none());
}

#[test]
fn text_environments_refuse_rich_filters_clearly() {
    let env = environment();
    for tag in [
        "{{ x | image }}",
        "{{ x | barcode(\"qr\") }}",
        "{{ x | link(\"t\") }}",
        "{{ x | color(\"#000\") }}",
        "{{ x | html }}",
        "{{ x | chart(type=\"bar\", x=\"a\", y=\"b\") }}",
        "{{ x | pagebreak }}",
    ] {
        let err = env
            .render_str(tag, minijinja::context! { x => "http://a.example" })
            .unwrap_err();
        assert!(
            err.to_string().contains("needs an Office template"),
            "{tag}: {err}"
        );
    }
}

#[test]
fn the_rich_environment_checks_and_prints_nothing() {
    let env = rich_environment(Format::Docx);
    let out = env
        .render_str(
            "a{{ x | link(\"t\") }}b{{ x | color(\"#000\") }}c{{ x | html }}d",
            minijinja::context! { x => "http://a.example" },
        )
        .unwrap();
    assert_eq!(out, "abcd");
    assert!(env
        .render_str(
            "{{ x | link }}",
            minijinja::context! { x => "javascript:alert(1)" },
        )
        .is_err());
    let ods_env = rich_environment(Format::Ods);
    let err = ods_env
        .render_str("{{ x | pagebreak }}", minijinja::context! { x => "" })
        .unwrap_err();
    assert!(err.to_string().contains("not ODS"), "{err}");
    let odt_env = rich_environment(Format::Odt);
    let err = odt_env
        .render_str(
            "{{ x | chart(type=\"bar\", x=\"a\", y=\"b\") }}",
            minijinja::context! { x => "" },
        )
        .unwrap_err();
    assert!(err.to_string().contains("list of objects"), "{err}");
}

// --- concurrency, validation, what stays the same -------------------------

#[test]
fn concurrent_renders_of_one_template_keep_their_own_values() {
    let doc = docx(&p(
        "{{ logo | image }}{{ url | link(name) }}{{ name | color(c) }}",
    ));
    let compiled: Compiled = compile(Format::Docx, &doc).unwrap();
    let job = |n: u32| {
        let image = png(n + 10, n + 10);
        let data = json!({
            "logo": b64(&image),
            "url": format!("https://example.com/{n}"),
            "name": format!("name{n}"),
            "c": format!("#00000{}", n % 10),
        });
        (0..25).for_each(|_| {
            let out = compiled.render(&data).unwrap();
            assert_eq!(bytes_of(&out, "word/media/rich1.png").unwrap(), image);
            let rels = part(&out, "word/_rels/document.xml.rels");
            assert!(
                rels.contains(&format!("https://example.com/{n}\"")),
                "{rels}"
            );
            let xml = part(&out, "word/document.xml");
            assert_eq!(xml.matches(&format!(">name{n}<")).count(), 2, "{xml}");
            assert!(xml.contains(&format!("w:val=\"00000{}\"", n % 10)), "{xml}");
        });
    };
    std::thread::scope(|s| {
        s.spawn(|| job(1));
        s.spawn(|| job(2));
        s.spawn(|| job(3));
    });
}

#[test]
fn validate_accepts_rich_filters_and_reports_bad_arguments() {
    let doc = docx(
        &(p("{{ logo | image(width=120) }}")
            + &p("{{ u | link(\"t\") }}")
            + &p("{{ s | color(\"#ff0000\") }}")
            + &p("{{ b | html }}")
            + &p("{{ rows | chart(type=\"pie\", x=\"month\", y=\"sales\") }}")
            + &p("{{ \"\" | pagebreak }}")
            + &p("{{ sku | barcode(\"qr\") }}")),
    );
    let mut data = sales();
    data["logo"] = json!(b64(&logo()));
    data["u"] = json!("https://a.example");
    data["s"] = json!("x");
    data["b"] = json!("<p>x</p>");
    data["sku"] = json!("x");
    let diags = validate(Format::Docx, &doc, &data);
    assert!(diags.is_empty(), "{diags:?}");

    // Bad arguments are found even when the sample data is missing.
    let bad = [
        (
            "{{ logo | image(width=\"wide\") }}",
            "width must be a number",
        ),
        ("{{ logo | image(nope=1) }}", "nope"),
        ("{{ s | color(\"red\") }}", "#rgb or #rrggbb"),
        ("{{ s | barcode(\"qr\", size=-1) }}", "size must be above 0"),
        (
            "{{ s | barcode(\"code128\", size=\"big\") }}",
            "size must be a number",
        ),
        (
            "{{ rows | chart(type=\"radar\", x=\"a\", y=\"b\") }}",
            "type must be",
        ),
    ];
    for (tag, expected) in bad {
        let doc = docx(&p(tag));
        let diags = validate(Format::Docx, &doc, &json!({}));
        assert!(
            diags.iter().any(|d| d.severity == Severity::Error
                && d.message.contains(expected)
                && d.location.is_some()),
            "{tag}: {diags:?}"
        );
    }
    // A bad value in the sample data is reported, placed at its tag.
    let doc = docx(&p("{{ u | link }}"));
    let diags = validate(Format::Docx, &doc, &json!({"u": "javascript:x"}));
    assert!(diags[0].message.contains("not allowed"), "{diags:?}");
    assert_eq!(
        diags[0].location.as_ref().unwrap().snippet,
        "{{ u | link }}"
    );
    // A missing value is reported like any undefined value.
    let doc = docx(&p("{{ gone | image }}"));
    let diags = validate(Format::Docx, &doc, &json!({}));
    assert!(
        diags.iter().any(|d| d.message.contains("undefined")),
        "{diags:?}"
    );
    // A format that cannot hold the content.
    let doc = ods("<table:table><table:table-row><table:table-cell><text:p>{{ x | pagebreak }}</text:p></table:table-cell></table:table-row></table:table>");
    let diags = validate(Format::Ods, &doc, &json!({"x": 1}));
    assert!(
        diags[0].message.contains("only in DOCX and ODT, not ODS"),
        "{diags:?}"
    );
}

#[test]
fn a_template_without_rich_values_is_written_as_before() {
    let doc = docx(&(p("Hi {{ name }}") + &tbl(&[&["{{ name }}"]])));
    let out = render(Format::Docx, &doc, json!({"name": "A"}));
    for name in [
        "[Content_Types].xml",
        "word/_rels/document.xml.rels",
        "word/header1.xml",
    ] {
        assert_unchanged(&doc, &out, &[name]);
    }
    // A filter that ran but was never printed adds nothing either.
    let doc = docx(&p("{% set x = logo | image %}{{ name }}"));
    let mut data = logo_data();
    data["name"] = json!("A");
    let out = render(Format::Docx, &doc, data);
    assert_unchanged(
        &doc,
        &out,
        &["[Content_Types].xml", "word/_rels/document.xml.rels"],
    );
}

#[test]
fn generated_names_never_come_from_data_and_never_collide() {
    // The template already holds a part with the first generated name.
    let doc = docx(&p("{{ logo | image }}"));
    let mut files = entries(&doc);
    files.push(("word/media/rich1.png".to_string(), vec![1, 2, 3]));
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();
    let doc = zip(&refs);
    let data =
        json!({"logo": {"data": b64(&logo()), "mime": "image/png", "name": "../../evil.png"}});
    let out = render(Format::Docx, &doc, data);
    assert_eq!(bytes_of(&out, "word/media/rich1.png").unwrap(), [1, 2, 3]);
    assert_eq!(bytes_of(&out, "word/media/rich2.png").unwrap(), logo());
    assert!(entries(&out).iter().all(|(n, _)| !n.contains("evil")));
}

#[test]
fn every_rendered_xml_part_stays_well_formed() {
    let doc = docx(
        &(p("{{ logo | image }} {{ u | link(s) }} {{ s | color(\"#123\") }}")
            + &p("{{ b | html }}")
            + &p("{{ rows | chart(type=\"line\", x=\"month\", y=\"sales\", title=s) }}")),
    );
    let mut data = sales();
    data["logo"] = json!(b64(&logo()));
    data["u"] = json!("https://a.example/?q=\u{e9}&r='1'");
    data["s"] = json!("<&>\"' \u{1f600}");
    data["b"] =
        json!("<p>&lt;&amp;&gt; \"q\" <a href=\"http://a.example/?x='1'&amp;y=2\">l</a></p>");
    let out = render(Format::Docx, &doc, data);
    for (name, bytes) in entries(&out) {
        if name.ends_with(".xml") || name.ends_with(".rels") {
            let mut reader = quick_xml_reader(&bytes);
            loop {
                match reader.read_event() {
                    Ok(quick_xml::events::Event::Eof) => break,
                    Ok(_) => {}
                    Err(e) => panic!("{name}: {e}"),
                }
            }
        }
    }
}

fn quick_xml_reader(bytes: &[u8]) -> quick_xml::Reader<&[u8]> {
    let mut reader = quick_xml::Reader::from_reader(bytes);
    reader.config_mut().check_end_names = true;
    reader
}

// --- LibreOffice ----------------------------------------------------------

fn report_data(chart: bool) -> Value {
    let logo = b64(&png(200, 100));
    json!({
        "customer": "Ada & Co",
        "logo": logo,
        "sku": "SKU-42",
        "url": "https://example.com/?a=1&b=2",
        "status": "Overdue",
        "body": "<h2>Heading two</h2><p>Para <b>bold</b> text</p><script>alert(1)</script><ul><li>Item one</li><li>Item two</li></ul><ol><li>Step one</li></ol><table><tr><th>Cell head</th><td>Cell value</td></tr></table>",
        "items": [{"name": "Lamp", "logo": logo}, {"name": "Plug", "logo": logo}],
        "chart": chart,
        "rows": [
            {"month": "January", "sales": 10, "costs": 4},
            {"month": "February", "sales": 20, "costs": 8},
        ],
    })
}

struct Case {
    file: &'static str,
    template: &'static [u8],
    format: Format,
    chart: bool,
    /// Text that must show in the PDF, beyond what every format shows.
    text: &'static [&'static str],
    /// The fewest images: logo, barcodes and the images of the rows.
    images: usize,
    pages: Option<u32>,
}

/// Renders LibreOffice-made templates of every format, converts the result to
/// PDF with LibreOffice and looks at what it drew. Skips without the images.
#[test]
fn libreoffice_opens_rich_output_and_shows_the_content() {
    if !libreoffice_available() {
        eprintln!("skipping: docker images not available");
        return;
    }
    let all: &[&str] = &[
        "Report for Ada",
        "Click here",
        "Overdue",
        "Heading two",
        "Para bold text",
        "Item one",
        "Item two",
        "Step one",
        "Cell head",
    ];
    let cases = [
        Case {
            file: "out.docx",
            template: include_bytes!("fixtures/rich.docx"),
            format: Format::Docx,
            chart: true,
            text: &[
                "Lamp",
                "Plug",
                "Second page",
                "Revenue",
                "January",
                "February",
                "sales",
                "costs",
            ],
            images: 5,
            pages: Some(2),
        },
        Case {
            file: "out.odt",
            template: include_bytes!("fixtures/rich.odt"),
            format: Format::Odt,
            chart: true,
            text: &[
                "Lamp",
                "Plug",
                "Second page",
                "Cell value",
                "Revenue",
                "January",
                "February",
                "sales",
                "costs",
            ],
            images: 5,
            pages: Some(2),
        },
        Case {
            file: "out.xlsx",
            template: include_bytes!("fixtures/rich.xlsx"),
            format: Format::Xlsx,
            chart: true,
            text: &[
                "Lamp", "Plug", "Keep", "Revenue", "January", "February", "sales", "costs",
            ],
            images: 3,
            pages: None,
        },
        Case {
            file: "out.ods",
            template: include_bytes!("fixtures/rich.ods"),
            format: Format::Ods,
            chart: true,
            text: &[
                "Lamp", "Plug", "Keep", "Below", "Revenue", "January", "February", "sales", "costs",
            ],
            images: 3,
            pages: None,
        },
        Case {
            file: "out.pptx",
            template: include_bytes!("fixtures/rich.pptx"),
            format: Format::Pptx,
            chart: true,
            text: &["Revenue", "January", "February", "sales", "costs"],
            images: 2,
            pages: Some(1),
        },
        Case {
            file: "out.odp",
            template: include_bytes!("fixtures/rich.odp"),
            format: Format::Odp,
            chart: true,
            text: &["Revenue", "January", "February", "sales", "costs"],
            images: 2,
            pages: Some(1),
        },
    ];
    let keep = std::env::var("RICH_KEEP").ok();
    let temp = tempfile::tempdir().unwrap();
    let dir = keep.as_deref().unwrap_or(temp.path().to_str().unwrap());
    for case in &cases {
        let out = render(case.format, case.template, report_data(case.chart));
        std::fs::write(format!("{dir}/{}", case.file), &out).unwrap();
        assert!(
            soffice(dir, "pdf", case.file),
            "{}: conversion failed",
            case.file
        );
        let pdf = case.file.replace('.', "_") + ".pdf";
        std::fs::rename(format!("{dir}/out.pdf"), format!("{dir}/{pdf}")).unwrap();
        let text = pdf_text(dir, &pdf);
        for expected in all.iter().chain(case.text) {
            assert!(
                text.contains(expected),
                "{}: no {expected:?} in {text}",
                case.file
            );
        }
        assert!(!text.contains("alert"), "{}: {text}", case.file);
        assert!(!text.contains("{{"), "{}: {text}", case.file);
        let images = pdf_images(dir, &pdf);
        assert!(images.len() >= case.images, "{}: {images:?}", case.file);
        if let Some(pages) = case.pages {
            assert_eq!(pdf_pages(dir, &pdf), pages, "{}", case.file);
        }
        let links = pdf_links(dir, &pdf);
        assert!(
            links.iter().any(|l| l.contains("example.com")),
            "{}: {links:?}",
            case.file
        );
    }
}
