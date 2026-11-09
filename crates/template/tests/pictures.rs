//! Placeholder pictures, more image types, barcode symbologies and options,
//! and colours with a scope, format by format.

mod common;

use common::*;
use regex::Regex;
use rusticdoc_template::{compile, Format};
use serde_json::{json, Value};

fn render(format: Format, bytes: &[u8], data: Value) -> Vec<u8> {
    compile(format, bytes)
        .expect("compiles")
        .render(&data)
        .expect("renders")
}

fn render_err(format: Format, bytes: &[u8], data: Value) -> rusticdoc_template::TemplateError {
    compile(format, bytes)
        .expect("compiles")
        .render(&data)
        .expect_err("must fail")
}

fn capture(re: &str, text: &str) -> String {
    Regex::new(re)
        .unwrap()
        .captures(text)
        .unwrap_or_else(|| panic!("{re} not found in {text}"))[1]
        .to_string()
}

/// The target of the relationship with this id in a rels part.
fn rel_target(rels: &str, id: &str) -> String {
    let tag = Regex::new("<Relationship [^>]*>").unwrap();
    let found = tag
        .find_iter(rels)
        .map(|m| m.as_str())
        .find(|t| t.contains(&format!("Id=\"{id}\"")))
        .map(|t| capture("Target=\"([^\"]*)\"", t))
        .unwrap_or_else(|| panic!("no relationship {id} in {rels}"));
    found
}

/// A 200 by 100 PNG: blue on the left half, orange on the right.
fn logo() -> Vec<u8> {
    use image::ImageEncoder;
    let mut pixels = Vec::new();
    for _ in 0..100 {
        for x in 0..200 {
            pixels.extend_from_slice(if x < 100 {
                &[30, 80, 200]
            } else {
                &[240, 140, 20]
            });
        }
    }
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(&pixels, 200, 100, image::ExtendedColorType::Rgb8)
        .unwrap();
    out
}

const DRAWING: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"120\" height=\"120\" \
    viewBox=\"0 0 120 120\" onload=\"alert(1)\"><script>alert(2)</script>\
    <rect x=\"10\" y=\"10\" width=\"100\" height=\"100\" rx=\"16\" fill=\"#2a7\"/>\
    <circle cx=\"60\" cy=\"60\" r=\"30\" fill=\"#fff\"/>\
    <image href=\"https://example.com/track.png\" width=\"1\" height=\"1\"/></svg>";

fn data() -> Value {
    let logo = b64(&logo());
    json!({
        "customer": "Ada",
        "logo": logo,
        "drawing": b64(DRAWING.as_bytes()),
        "sku": "590123412345",
        "url": "https://example.com/order/42",
        "items": [{"name": "Lamp", "photo": logo}, {"name": "Plug", "photo": logo}],
    })
}

/// The `<wp:inline>` or `<wp:anchor>` whose `wp:docPr` has this name.
fn docx_drawing<'a>(xml: &'a str, name: &str) -> &'a str {
    let at = xml
        .find(&format!("name=\"{name}\""))
        .unwrap_or_else(|| panic!("no drawing {name} in {xml}"));
    let start = xml[..at]
        .rfind("<wp:inline")
        .max(xml[..at].rfind("<wp:anchor"))
        .unwrap();
    let end = at
        + xml[at..]
            .find("</wp:inline>")
            .or_else(|| xml[at..].find("</wp:anchor>"))
            .unwrap();
    &xml[start..end]
}

fn media_of(out: &[u8], rels_part: &str, element: &str) -> String {
    let rid = capture("r:embed=\"(rId[0-9]+)\"", element);
    rel_target(&part(out, rels_part), &rid)
}

// --- Placeholders ---------------------------------------------------------

/// A picture as Word 365 writes it: anchored, square wrapping, a border, a
/// brightness effect and the local-dpi extension, with the tag typed in the
/// Alt Text pane, which Word copies to the inner picture too.
fn word_picture(descr: &str) -> String {
    format!(
        "<w:p><w:r><w:rPr><w:noProof/></w:rPr><w:drawing><wp:anchor \
         xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" \
         distT=\"0\" distB=\"0\" distL=\"114300\" distR=\"114300\" simplePos=\"0\" \
         relativeHeight=\"251658240\" behindDoc=\"0\" locked=\"0\" layoutInCell=\"1\" \
         allowOverlap=\"1\"><wp:simplePos x=\"0\" y=\"0\"/><wp:positionH relativeFrom=\"column\">\
         <wp:posOffset>3200400</wp:posOffset></wp:positionH><wp:positionV \
         relativeFrom=\"paragraph\"><wp:posOffset>0</wp:posOffset></wp:positionV>\
         <wp:extent cx=\"1524000\" cy=\"1524000\"/><wp:effectExtent l=\"0\" t=\"0\" r=\"0\" \
         b=\"0\"/><wp:wrapSquare wrapText=\"bothSides\"/><wp:docPr id=\"1\" name=\"Picture 1\" \
         descr=\"{descr}\"/><wp:cNvGraphicFramePr><a:graphicFrameLocks \
         xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
         noChangeAspect=\"1\"/></wp:cNvGraphicFramePr><a:graphic \
         xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><a:graphicData \
         uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><pic:pic \
         xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><pic:nvPicPr>\
         <pic:cNvPr id=\"1\" name=\"Picture 1\" descr=\"{descr}\"/><pic:cNvPicPr/></pic:nvPicPr>\
         <pic:blipFill><a:blip r:embed=\"rId2\"><a:lum bright=\"20000\"/><a:extLst><a:ext \
         uri=\"{{28A0092B-C50C-407E-A947-70E740481C1C}}\"><a14:useLocalDpi \
         xmlns:a14=\"http://schemas.microsoft.com/office/drawing/2010/main\" val=\"0\"/>\
         </a:ext></a:extLst></a:blip><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
         <pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"1524000\" cy=\"1524000\"/>\
         </a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom><a:ln w=\"19050\">\
         <a:solidFill><a:srgbClr val=\"1F6F3F\"/></a:solidFill></a:ln></pic:spPr></pic:pic>\
         </a:graphicData></a:graphic></wp:anchor></w:drawing></w:r></w:p>"
    )
}

#[test]
fn a_word_placeholder_keeps_its_frame_and_takes_the_image() {
    let template = docx(&word_picture(
        "Logo {{ logo | image(fit=&quot;cover&quot;) }}",
    ));
    let out = render(Format::Docx, &template, data());
    let xml = part(&out, "word/document.xml");
    assert!(!xml.contains("{{"), "{xml}");
    for kept in [
        "<wp:extent cx=\"1524000\" cy=\"1524000\"/>",
        "<wp:wrapSquare wrapText=\"bothSides\"/>",
        "<wp:posOffset>3200400</wp:posOffset>",
        "<a:lum bright=\"20000\"/>",
        "<a14:useLocalDpi",
        "<a:ln w=\"19050\">",
        "<a:ext cx=\"1524000\" cy=\"1524000\"/>",
        "descr=\"Logo\"",
    ] {
        assert!(xml.contains(kept), "{kept} is gone: {xml}");
    }
    // A 2:1 image covering a square loses a quarter on each side.
    assert!(
        xml.contains("<a:srcRect l=\"25000\" t=\"0\" r=\"25000\" b=\"0\"/>"),
        "{xml}"
    );
    assert_eq!(
        media_of(&out, "word/_rels/document.xml.rels", &xml),
        "media/rich1.png"
    );
    assert_eq!(bytes_of(&out, "word/media/rich1.png").unwrap(), logo());
}

#[test]
fn placeholder_errors_say_which_picture() {
    let sized = docx(&word_picture("{{ logo | image(width=40) }}"));
    let err = render_err(Format::Docx, &sized, data());
    assert!(
        err.message.contains("takes its size from its frame"),
        "{err}"
    );
    let loc = err.location.expect("located");
    assert_eq!(loc.part, "word/document.xml");
    assert!(loc.place.contains("Picture 1"), "{loc:?}");

    let two = docx(&word_picture("{{ logo | image }}{{ logo | image }}"));
    let err = render_err(Format::Docx, &two, data());
    assert!(err.message.contains("more than one"), "{err}");

    // A shape is not a picture.
    let shape = "<p:sp><p:nvSpPr><p:cNvPr id=\"5\" name=\"Box\" descr=\"{{ logo | image }}\"/>\
                 <p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/>\
                 <a:ext cx=\"10\" cy=\"10\"/></a:xfrm></p:spPr></p:sp>";
    let err = render_err(Format::Pptx, &pptx(&[shape]), data());
    assert!(err.message.contains("only a picture"), "{err}");
    assert!(err.location.unwrap().place.contains("Box"));
    // Other tags in alt text are filled in as text, beside a placeholder too.
    let other = docx(&word_picture("For {{ customer }} {{ logo | image }}"));
    let out = render(Format::Docx, &other, data());
    let xml = part(&out, "word/document.xml");
    assert!(xml.contains("descr=\"For Ada\""), "{xml}");
    assert_eq!(
        media_of(&out, "word/_rels/document.xml.rels", &xml),
        "media/rich1.png"
    );
}

#[test]
fn libreoffice_docx_placeholders_fit_their_frames() {
    let out = render(
        Format::Docx,
        include_bytes!("fixtures/placeholder.docx"),
        data(),
    );
    let xml = part(&out, "word/document.xml");
    assert!(!xml.contains("{{"), "{xml}");
    let rels = "word/_rels/document.xml.rels";
    // LibreOffice writes a 120-point frame less its border as 1523365 EMU.
    let contain = docx_drawing(&xml, "Contain");
    assert!(
        contain.contains("<wp:extent cx=\"1523365\" cy=\"1523365\"/>"),
        "{contain}"
    );
    assert!(
        contain.contains("<a:srcRect l=\"0\" t=\"-50000\" r=\"0\" b=\"-50000\"/>"),
        "{contain}"
    );
    assert!(media_of(&out, rels, contain).starts_with("media/rich"));
    let cover = docx_drawing(&xml, "Cover");
    assert!(
        cover.contains("<a:srcRect l=\"25000\" t=\"0\" r=\"25000\" b=\"0\"/>"),
        "{cover}"
    );
    let width = docx_drawing(&xml, "Width");
    assert!(
        width.contains("<wp:extent cx=\"1523365\" cy=\"761760\"/>"),
        "{width}"
    );

    let stretch = docx_drawing(&xml, "Stretch");
    assert!(!stretch.contains("srcRect"), "{stretch}");
    let svg = docx_drawing(&xml, "Wrapped");
    assert!(svg.contains("asvg:svgBlip"), "{svg}");
    let svg_rid = capture("svgBlip [^>]*r:embed=\"(rId[0-9]+)\"", svg);
    let svg_part = rel_target(&part(&out, rels), &svg_rid);
    let stored = part(&out, &format!("word/{svg_part}"));
    assert!(
        stored.contains("<circle") && !stored.contains("script"),
        "{stored}"
    );
    assert!(
        !stored.contains("example.com") && !stored.contains("onload"),
        "{stored}"
    );
    assert!(part(&out, "[Content_Types].xml").contains("Extension=\"svg\""));
    // The table row loop copies the picture; each copy has its own id.
    let ids: Vec<String> = Regex::new("<wp:docPr id=\"([0-9]+)\" name=\"Item\"")
        .unwrap()
        .captures_iter(&xml)
        .map(|c| c[1].to_string())
        .collect();
    assert_eq!(ids.len(), 2, "{xml}");
    assert_ne!(ids[0], ids[1]);
}

#[test]
fn libreoffice_pptx_and_xlsx_placeholders_fit_their_frames() {
    let out = render(
        Format::Pptx,
        include_bytes!("fixtures/placeholder.pptx"),
        data(),
    );
    let slide = part(&out, "ppt/slides/slide1.xml");
    assert!(!slide.contains("{{"), "{slide}");
    assert!(
        slide.contains("<a:srcRect l=\"0\" t=\"-50000\" r=\"0\" b=\"-50000\"/>"),
        "{slide}"
    );
    assert!(
        slide.contains("<a:srcRect l=\"25000\" t=\"0\" r=\"25000\" b=\"0\"/>"),
        "{slide}"
    );
    assert!(slide.contains("asvg:svgBlip"), "{slide}");
    assert!(slide.contains("Deck for Ada"), "{slide}");

    let out = render(
        Format::Xlsx,
        include_bytes!("fixtures/placeholder.xlsx"),
        data(),
    );
    let drawing = part(&out, "xl/drawings/drawing1.xml");
    assert!(!drawing.contains("{{"), "{drawing}");
    // A 2:1 image covering a 2:1 frame needs no crop.
    let rels = part(&out, "xl/drawings/_rels/drawing1.xml.rels");
    let rids: Vec<String> = Regex::new("<a:blip r:embed=\"(rId[0-9]+)\"")
        .unwrap()
        .captures_iter(&drawing)
        .map(|c| c[1].to_string())
        .collect();
    assert_eq!(rids.len(), 2, "{drawing}");
    for rid in rids {
        assert!(
            rel_target(&rels, &rid).starts_with("../media/rich"),
            "{rels}"
        );
    }
    let sheet = part(&out, "xl/worksheets/sheet1.xml");
    assert!(sheet.contains("Report for Ada"), "{sheet}");
}

#[test]
fn libreoffice_odf_placeholders_fit_their_frames() {
    for (format, template) in [
        (Format::Odt, &include_bytes!("fixtures/placeholder.odt")[..]),
        (Format::Ods, &include_bytes!("fixtures/placeholder.ods")[..]),
        (Format::Odp, &include_bytes!("fixtures/placeholder.odp")[..]),
    ] {
        let out = render(format, template, data());
        let xml = part(&out, "content.xml");
        assert!(!xml.contains("{{"), "{format:?}: {xml}");
        assert!(!xml.contains("office:binary-data"), "{format:?}");
        assert!(
            xml.contains("xlink:href=\"Pictures/rich"),
            "{format:?}: {xml}"
        );
        assert!(xml.contains("fo:clip=\"rect("), "{format:?}: {xml}");
        let manifest = part(&out, "META-INF/manifest.xml");
        assert!(manifest.contains("Pictures/rich"), "{format:?}: {manifest}");
    }
    let out = render(
        Format::Odt,
        include_bytes!("fixtures/placeholder.odt"),
        data(),
    );
    let xml = part(&out, "content.xml");
    // The SVG comes first, its PNG after it for readers without SVG.
    let svg = xml.find("draw:mime-type=\"image/svg+xml\"").expect("svg");
    assert!(xml[svg..].contains("draw:mime-type=\"image/png\""), "{xml}");
    // 2:1 in a square frame: a quarter of the height of empty space above and
    // below, in the image's own inches (200 px at 96 dpi is 2.0833 in).
    assert!(
        xml.contains("fo:clip=\"rect(-0.5208in, 0.0000in, -0.5208in, 0.0000in)\""),
        "{xml}"
    );
    assert!(
        xml.contains("svg:height=\"59.98pt\""),
        "the width fit: {xml}"
    );
}

// --- Image types ----------------------------------------------------------

fn gif() -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::gif::GifEncoder::new(&mut out)
        .encode(
            &[200, 30, 30, 255].repeat(40 * 20),
            40,
            20,
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
    out
}

fn webp() -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut out)
        .encode(
            &[30, 30, 200, 255].repeat(40 * 20),
            40,
            20,
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
    out
}

#[test]
fn gif_webp_and_svg_go_in_every_format() {
    let values = json!({
        "g": b64(&gif()),
        "w": data_uri("image/webp", &webp()),
        "s": {"data": b64(DRAWING.as_bytes()), "mime": "image/svg+xml"},
    });
    let tags = "{{ g | image }} {{ w | image }} {{ s | image(width=60) }}";
    let out = render(Format::Docx, &docx(&p(tags)), values.clone());
    let names: Vec<String> = entries(&out).into_iter().map(|(n, _)| n).collect();
    for name in [
        "word/media/rich1.gif",
        "word/media/rich2.png",
        "word/media/rich3.png",
        "word/media/rich4.svg",
    ] {
        assert!(names.iter().any(|n| n == name), "{name}: {names:?}");
    }
    assert_eq!(bytes_of(&out, "word/media/rich1.gif").unwrap(), gif());
    let types = part(&out, "[Content_Types].xml");
    assert!(
        types.contains("Extension=\"gif\" ContentType=\"image/gif\""),
        "{types}"
    );
    assert!(
        types.contains("Extension=\"svg\" ContentType=\"image/svg+xml\""),
        "{types}"
    );
    let xml = part(&out, "word/document.xml");
    assert!(xml.contains("asvg:svgBlip"), "{xml}");

    let out = render(Format::Pptx, &pptx(&[&shape(&[tags])]), values.clone());
    assert!(part(&out, "ppt/slides/slide1.xml").contains("asvg:svgBlip"));
    let out = render(
        Format::Xlsx,
        &xlsx(&[row(1, &[cell("A1", "inlineStr", tags)])], &[]),
        values.clone(),
    );
    assert!(part(&out, "xl/drawings/drawing1.xml").contains("asvg:svgBlip"));
    let out = render(
        Format::Odt,
        &odt(&format!("<text:p>{tags}</text:p>")),
        values,
    );
    let xml = part(&out, "content.xml");
    assert!(
        xml.contains("Pictures/rich4.svg") || xml.contains(".svg\""),
        "{xml}"
    );
    assert!(part(&out, "META-INF/manifest.xml").contains("image/svg+xml"));
}

#[test]
fn unsafe_or_oversized_images_are_refused() {
    let tag = docx(&p("{{ x | image }}"));
    let cases = [
        (
            json!(b64(
                b"<svg xmlns=\"http://www.w3.org/2000/svg\"><!DOCTYPE x>"
            )),
            "cannot be read",
        ),
        (json!(b64(b"<svg>no namespace</svg>")), "svg root element"),
        (json!(b64(b"BM....")), "only PNG, JPEG, GIF, SVG and WebP"),
        (
            json!({"data": b64(&gif()), "mime": "image/png"}),
            "the bytes are image/gif",
        ),
    ];
    for (value, expected) in cases {
        let err = render_err(Format::Docx, &tag, json!({ "x": value }));
        assert!(err.message.contains(expected), "{expected}: {err}");
    }
}

// --- Barcodes -------------------------------------------------------------

/// Decodes the one barcode in a PNG with rxing.
fn decode(png: &[u8]) -> (String, String) {
    let img = image::load_from_memory(png).unwrap().to_luma8();
    let (w, h) = img.dimensions();
    let mut hints = rxing::DecodeHints {
        TryHarder: Some(true),
        ..rxing::DecodeHints::default()
    };
    let found = rxing::helpers::detect_in_luma_with_hints(img.into_raw(), w, h, None, &mut hints)
        .unwrap_or_else(|e| panic!("no barcode found: {e}"));
    (
        format!("{:?}", found.getBarcodeFormat()),
        found.getText().to_string(),
    )
}

fn barcode_png(kind: &str, value: &str, options: &str) -> Vec<u8> {
    let tag = format!("{{{{ v | barcode(\"{kind}\"{options}) }}}}");
    let out = render(Format::Docx, &docx(&p(&tag)), json!({ "v": value }));
    bytes_of(&out, "word/media/rich1.png").expect("a PNG")
}

#[test]
fn every_symbology_decodes_to_its_exact_value() {
    let cases = [
        (
            "qr",
            "https://example.com/?a=1&b=2",
            "QR_CODE",
            "https://example.com/?a=1&b=2",
        ),
        ("datamatrix", "Lot 42/7", "DATA_MATRIX", "Lot 42/7"),
        ("pdf417", "Invoice 2026-001", "PDF_417", "Invoice 2026-001"),
        ("aztec", "Ticket 99", "AZTEC", "Ticket 99"),
        ("code128", "SKU-42 a", "CODE_128", "SKU-42 a"),
        ("code39", "ABC-123", "CODE_39", "ABC-123"),
        ("code93", "Code 93", "CODE_93", "Code 93"),
        ("ean13", "590123412345", "EAN_13", "5901234123457"),
        ("ean8", "9638507", "EAN_8", "96385074"),
        ("upca", "03600029145", "UPC_A", "036000291452"),
        ("upce", "0123456", "UPC_E", "01234565"),
        ("itf", "1234567890", "ITF", "1234567890"),
        ("itf14", "1540014128876", "ITF", "15400141288763"),
        ("codabar", "A40156B", "CODABAR", "40156"),
    ];
    for (kind, value, format, expected) in cases {
        let (found, text) = decode(&barcode_png(kind, value, ""));
        assert_eq!(
            (found.as_str(), text.as_str()),
            (format, expected),
            "{kind}"
        );
        // Options change the drawing, not the value.
        let options = if matches!(kind, "qr" | "datamatrix" | "pdf417" | "aztec") {
            ", color=\"#123\", background=\"#ffe\", quiet_zone=6, rotate=90"
        } else {
            ", text=true, color=\"#123\", background=\"#ffe\", quiet_zone=12, rotate=180, width=260, height=90"
        };
        let (_, text) = decode(&barcode_png(kind, value, options));
        assert_eq!(text, expected, "{kind} with options");
    }
    let (found, text) = decode(&barcode_png("gs1-128", "(01)09501101530003(10)ABC123", ""));
    assert_eq!(found, "CODE_128");
    assert_eq!(text, "010950110153000310ABC123");
    for (kind, ec) in [("qr", "\"H\""), ("aztec", "50"), ("pdf417", "5")] {
        let (_, text) = decode(&barcode_png(kind, "ec test", &format!(", ec={ec}")));
        assert_eq!(text, "ec test", "{kind}");
    }
}

#[test]
fn barcode_options_are_checked_with_clear_errors() {
    let cases = [
        ("barcode(\"qr\", text=true)", "linear codes only"),
        (
            "barcode(\"code128\", rotate=45)",
            "rotate is 0, 90, 180 or 270",
        ),
        (
            "barcode(\"code128\", quiet_zone=-1)",
            "quiet_zone must be a whole number",
        ),
        (
            "barcode(\"code128\", color=\"red\")",
            "color must be written",
        ),
        ("barcode(\"qr\", ec=\"X\")", "ec for QR is L, M, Q or H"),
        ("barcode(\"code39\", ec=1)", "ec applies to qr"),
        ("barcode(\"qr\", width=90, height=40)", "square"),
        ("barcode(\"ean13\", size=90, width=90)", "size or width"),
        ("barcode(\"upce\")", "upce takes 7 digits"),
        ("barcode(\"maxicode\")", "unknown symbology"),
        ("barcode(\"code128\", fit=\"zoom\")", "fit is contain"),
    ];
    for (filter, expected) in cases {
        let tag = format!("{{{{ v | {filter} }}}}");
        let err = render_err(Format::Docx, &docx(&p(&tag)), json!({ "v": "1234" }));
        assert!(err.message.contains(expected), "{filter}: {err}");
    }
}

#[test]
fn a_barcode_fills_a_placeholder() {
    let out = render(
        Format::Docx,
        &docx(&word_picture("{{ sku | barcode(&quot;qr&quot;) }}")),
        json!({"sku": "SKU-7"}),
    );
    let xml = part(&out, "word/document.xml");
    let png = bytes_of(
        &out,
        &format!(
            "word/{}",
            media_of(&out, "word/_rels/document.xml.rels", &xml)
        ),
    )
    .unwrap();
    assert_eq!(decode(&png).1, "SKU-7");
    assert!(
        xml.contains("<wp:extent cx=\"1524000\" cy=\"1524000\"/>"),
        "{xml}"
    );
}

// --- Colour scopes --------------------------------------------------------

#[test]
fn docx_colour_scopes() {
    let body = format!(
        "{}{}",
        p("Para {{ s | color(\"#c00\", background=\"#fe0\", scope=\"paragraph\") }} end"),
        tbl(&[
            &["{{ s | color(background=\"#0c0\", scope=\"cell\") }}", "B"],
            &[
                "{{ s | color(\"#00c\", background=\"#ccc\", scope=\"row\") }}",
                "D"
            ],
        ])
    );
    let out = render(Format::Docx, &docx(&body), json!({"s": "Late"}));
    let xml = part(&out, "word/document.xml");
    let para = &xml[..xml.find("<w:tbl>").unwrap()];
    assert!(
        para.contains("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"FFEE00\"/>"),
        "{para}"
    );
    assert_eq!(
        para.matches("<w:color w:val=\"CC0000\"/>").count(),
        3,
        "every run: {para}"
    );
    let rows: Vec<&str> = xml.split("<w:tr>").skip(1).collect();
    assert_eq!(
        rows[0].matches("w:fill=\"00CC00\"").count(),
        1,
        "{}",
        rows[0]
    );
    assert!(
        rows[0].contains("<w:tcPr><w:tcW w:w=\"3000\" w:type=\"dxa\"/><w:shd"),
        "{}",
        rows[0]
    );
    assert_eq!(
        rows[1].matches("w:fill=\"CCCCCC\"").count(),
        2,
        "{}",
        rows[1]
    );
    assert!(
        rows[1].matches("<w:color w:val=\"0000CC\"/>").count() >= 2,
        "{}",
        rows[1]
    );

    let err = render_err(
        Format::Docx,
        &docx(&p("{{ s | color(background=\"#0c0\", scope=\"cell\") }}")),
        json!({"s": "x"}),
    );
    assert!(err.message.contains("table cell"), "{err}");
    assert!(err.location.is_some());
    let err = render_err(
        Format::Docx,
        &docx(&p("{{ s | color(\"#c00\", scope=\"shape\") }}")),
        json!({"s": "x"}),
    );
    assert!(err.message.contains("only in PPTX and ODP"), "{err}");
}

#[test]
fn sheet_colour_scopes() {
    let book = xlsx_styled(&[
        row(
            1,
            &[
                cell(
                    "A1",
                    "inlineStr",
                    "{{ s | color(\"#c00\", background=\"#fe0\", scope=\"row\") }}",
                ),
                cell("B1", "inlineStr", "Other"),
            ],
        ),
        row(
            2,
            &[cell(
                "A2",
                "inlineStr",
                "{{ s | color(background=\"#0c0\", scope=\"cell\") }}",
            )],
        ),
    ]);
    let out = render(Format::Xlsx, &book, json!({"s": "Late"}));
    let sheet = part(&out, "xl/worksheets/sheet1.xml");
    let row1 = capture("(<row r=\"1\"[^>]*>.*?</row>)", &sheet);
    assert!(row1.contains("customFormat=\"1\""), "{row1}");
    let styles: Vec<String> = Regex::new(" s=\"([0-9]+)\"")
        .unwrap()
        .captures_iter(&row1)
        .map(|c| c[1].to_string())
        .collect();
    assert_eq!(styles.len(), 3, "row and both cells: {row1}");
    let stylesheet = part(&out, "xl/styles.xml");
    assert!(
        stylesheet.contains("FFFFEE00") && stylesheet.contains("FF00CC00"),
        "{stylesheet}"
    );
    let err = render_err(
        Format::Xlsx,
        &xlsx(
            &[row(
                1,
                &[cell(
                    "A1",
                    "inlineStr",
                    "{{ s | color(\"#c00\", scope=\"paragraph\") }}",
                )],
            )],
            &[],
        ),
        json!({"s": "x"}),
    );
    assert!(err.message.contains("XLSX has no paragraphs"), "{err}");

    let ods_body = "<table:table table:name=\"S\"><table:table-row><table:table-cell office:value-type=\"string\"><text:p>{{ s | color(background=\"#0c0\", scope=\"row\") }}</text:p></table:table-cell><table:table-cell office:value-type=\"string\"><text:p>B</text:p></table:table-cell></table:table-row></table:table>";
    let out = render(Format::Ods, &ods(ods_body), json!({"s": "Late"}));
    let xml = part(&out, "content.xml");
    assert_eq!(xml.matches("table:style-name=\"RichC").count(), 2, "{xml}");
    assert!(xml.contains("fo:background-color=\"#00cc00\""), "{xml}");
}

#[test]
fn odt_colour_scopes() {
    let body = "<text:p>{{ s | color(\"#c00\", background=\"#fe0\", scope=\"paragraph\") }}</text:p>\
        <table:table table:name=\"T\"><table:table-column table:number-columns-repeated=\"2\"/>\
        <table:table-row><table:table-cell office:value-type=\"string\"><text:p>{{ s | color(background=\"#0c0\", scope=\"row\") }}</text:p></table:table-cell>\
        <table:table-cell office:value-type=\"string\"><text:p>B</text:p></table:table-cell></table:table-row></table:table>";
    let out = render(Format::Odt, &odt(body), json!({"s": "Late"}));
    let xml = part(&out, "content.xml");
    assert!(
        xml.contains("<style:paragraph-properties fo:background-color=\"#ffee00\"/>"),
        "{xml}"
    );
    assert!(
        xml.contains("<style:text-properties fo:color=\"#cc0000\"/>"),
        "{xml}"
    );
    assert_eq!(xml.matches("table:style-name=\"RichC").count(), 2, "{xml}");
}

#[test]
fn presentation_colour_scopes() {
    let shape_tag =
        shape(&["{{ s | color(\"#fff\", background=\"#c00\", scope=\"shape\") }} and more"]);
    let table = ptbl(&[&["{{ s | color(background=\"#0c0\", scope=\"row\") }}", "B"]]);
    let out = render(
        Format::Pptx,
        &pptx(&[&format!("{shape_tag}{table}")]),
        json!({"s": "Late"}),
    );
    let slide = part(&out, "ppt/slides/slide1.xml");
    let sp = &slide[..slide.find("</p:sp>").unwrap()];
    assert!(
        sp.contains("<p:spPr><a:solidFill><a:srgbClr val=\"CC0000\"/></a:solidFill></p:spPr>"),
        "{sp}"
    );
    assert_eq!(
        sp.matches("<a:srgbClr val=\"FFFFFF\"/>").count(),
        2,
        "every run: {sp}"
    );
    assert_eq!(
        slide
            .matches("<a:tcPr><a:solidFill><a:srgbClr val=\"00CC00\"/></a:solidFill></a:tcPr>")
            .count(),
        2,
        "{slide}"
    );
    let err = render_err(
        Format::Pptx,
        &pptx(&[&shape(&[
            "{{ s | color(background=\"#c00\", scope=\"paragraph\") }}",
        ])]),
        json!({"s": "x"}),
    );
    assert!(
        err.message.contains("paragraphs have no background"),
        "{err}"
    );

    let odp_body = "<draw:page draw:name=\"p\"><draw:frame draw:style-name=\"gr1\" svg:width=\"100pt\" svg:height=\"40pt\" xmlns:svg=\"urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0\"><draw:text-box><text:p>{{ s | color(background=\"#c00\", scope=\"shape\") }}</text:p></draw:text-box></draw:frame></draw:page>";
    let out = render(Format::Odp, &odp(odp_body), json!({"s": "Late"}));
    let xml = part(&out, "content.xml");
    assert!(xml.contains("draw:fill-color=\"#cc0000\""), "{xml}");
    assert!(xml.contains("draw:style-name=\"RichS"), "{xml}");
}

// --- LibreOffice ----------------------------------------------------------

/// Renders a page of a PDF to PNG with pdftoppm, at `dpi`.
fn page_png(dir: &str, pdf: &str, dpi: u32) -> Vec<u8> {
    let mount = format!("{dir}:/w");
    let input = format!("/w/{pdf}");
    let output = format!("/w/{}", pdf.trim_end_matches(".pdf"));
    let dpi = dpi.to_string();
    let args = [
        "run",
        "--rm",
        "--platform",
        "linux/arm64",
        "-v",
        &mount,
        "gotenberg/integration-tools:latest",
        "pdftoppm",
        "-r",
        &dpi,
        "-png",
        "-singlefile",
        &input,
        &output,
    ];
    let (ok, text) = run("docker", &args).unwrap();
    assert!(ok, "pdftoppm failed: {text}");
    std::fs::read(format!("{dir}/{}.png", pdf.trim_end_matches(".pdf"))).unwrap()
}

/// Every barcode rxing finds in a PNG, as text: on the whole page, then in
/// overlapping bands, since its finder can miss a small symbol among many.
fn decode_all(png: &[u8]) -> Vec<String> {
    let img = image::load_from_memory(png).unwrap().to_luma8();
    let (w, h) = img.dimensions();
    let detect = |luma: Vec<u8>, h: u32| -> Vec<String> {
        rxing::helpers::detect_multiple_in_luma(luma, w, h)
            .map(|found| found.iter().map(|r| r.getText().to_string()).collect())
            .unwrap_or_default()
    };
    let mut found = detect(img.as_raw().clone(), h);
    let band = (h / 6).max(1);
    let mut top = 0;
    while top < h {
        let height = (band * 2).min(h - top);
        let rows = &img.as_raw()[(top * w) as usize..((top + height) * w) as usize];
        found.extend(detect(rows.to_vec(), height));
        top += band;
    }
    // rxing looks for an Aztec bullseye from the middle of the image, so it
    // finds one only in a window around it.
    let win = (w / 4).max(64).min(w).min(h);
    let step = (win / 3).max(1);
    let mut y = 0;
    'scan: while y + win <= h {
        let mut x = 0;
        while x + win <= w {
            let mut tile = Vec::with_capacity((win * win) as usize);
            for row in y..y + win {
                let at = (row * w + x) as usize;
                tile.extend_from_slice(&img.as_raw()[at..at + win as usize]);
            }
            if let Ok(aztec) =
                rxing::helpers::detect_in_luma(tile, win, win, Some(rxing::BarcodeFormat::AZTEC))
            {
                found.push(aztec.getText().to_string());
                break 'scan;
            }
            x += step;
        }
        y += step;
    }
    found
}

/// Renders the placeholder fixtures of every format, converts them with
/// LibreOffice, and checks what it drew. With RICH_KEEP set, the PDFs and
/// PNGs stay there to look at.
#[test]
fn libreoffice_draws_placeholders_and_svg() {
    if !libreoffice_available() {
        eprintln!("skipping: docker images not available");
        return;
    }
    let keep = std::env::var("RICH_KEEP").ok();
    let temp = tempfile::tempdir().unwrap();
    let dir = keep.as_deref().unwrap_or(temp.path().to_str().unwrap());
    let cases: [(&str, Format, &[u8]); 6] = [
        (
            "ph.docx",
            Format::Docx,
            include_bytes!("fixtures/placeholder.docx"),
        ),
        (
            "ph.odt",
            Format::Odt,
            include_bytes!("fixtures/placeholder.odt"),
        ),
        (
            "ph.xlsx",
            Format::Xlsx,
            include_bytes!("fixtures/placeholder.xlsx"),
        ),
        (
            "ph.ods",
            Format::Ods,
            include_bytes!("fixtures/placeholder.ods"),
        ),
        (
            "ph.pptx",
            Format::Pptx,
            include_bytes!("fixtures/placeholder.pptx"),
        ),
        (
            "ph.odp",
            Format::Odp,
            include_bytes!("fixtures/placeholder.odp"),
        ),
    ];
    for (file, format, template) in cases {
        let out = render(format, template, data());
        std::fs::write(format!("{dir}/{file}"), &out).unwrap();
        assert!(soffice(dir, "pdf", file), "{file}: conversion failed");
        let pdf = file.replace('.', "_") + ".pdf";
        std::fs::rename(format!("{dir}/ph.pdf"), format!("{dir}/{pdf}")).unwrap();
        let text = pdf_text(dir, &pdf);
        assert!(!text.contains("{{"), "{file}: {text}");
        let png = page_png(dir, &pdf, 200);
        let found = decode_all(&png);
        let expected = match format {
            Format::Docx | Format::Odt => "5901234123457",
            Format::Xlsx | Format::Ods => "https://example.com/order/42",
            _ => "590123412345",
        };
        assert!(
            found.iter().any(|f| f == expected),
            "{file}: decoded {found:?}"
        );
    }
}

/// One of every symbology, as LibreOffice prints them, read back from its PDF
/// at 300 dpi.
#[test]
fn libreoffice_prints_every_symbology_readably() {
    if !libreoffice_available() {
        eprintln!("skipping: docker images not available");
        return;
    }
    let keep = std::env::var("RICH_KEEP").ok();
    let temp = tempfile::tempdir().unwrap();
    let dir = keep.as_deref().unwrap_or(temp.path().to_str().unwrap());
    let kinds = [
        ("qr", "https://example.com/q"),
        ("datamatrix", "DM-77"),
        ("pdf417", "PDF417 text"),
        ("aztec", "AZTEC-5"),
        ("code128", "C128-x"),
        ("gs1-128", "(01)09501101530003(10)LOT7"),
        ("code39", "C39-9"),
        ("code93", "C93-3"),
        ("ean13", "400638133393"),
        ("ean8", "5512345"),
        ("upca", "72527273070"),
        ("upce", "0654321"),
        ("itf", "00123456"),
        ("itf14", "0001234560001"),
        ("codabar", "C1234D"),
    ];
    let mut body = String::new();
    let mut values = serde_json::Map::new();
    for (i, (kind, value)) in kinds.iter().enumerate() {
        let options = if matches!(*kind, "qr" | "datamatrix" | "aztec") {
            ", size=90"
        } else if *kind == "pdf417" {
            ", width=200"
        } else {
            ", text=true, width=220"
        };
        body.push_str(&p(&format!(
            "{kind}: {{{{ v{i} | barcode(\"{kind}\"{options}) }}}}"
        )));
        values.insert(format!("v{i}"), json!(value));
    }
    let out = render(Format::Docx, &docx(&body), Value::Object(values));
    std::fs::write(format!("{dir}/codes.docx"), &out).unwrap();
    assert!(soffice(dir, "pdf", "codes.docx"));
    let pages = pdf_pages(dir, "codes.pdf");
    let mut found = Vec::new();
    for page in 1..=pages {
        let mount = format!("{dir}:/w");
        let (p, out_name) = (page.to_string(), format!("/w/codes-{page}"));
        let args = [
            "run",
            "--rm",
            "--platform",
            "linux/arm64",
            "-v",
            &mount,
            "gotenberg/integration-tools:latest",
            "pdftoppm",
            "-r",
            "300",
            "-png",
            "-singlefile",
            "-f",
            &p,
            "-l",
            &p,
            "/w/codes.pdf",
            &out_name,
        ];
        assert!(run("docker", &args).unwrap().0);
        found.extend(decode_all(
            &std::fs::read(format!("{dir}/codes-{page}.png")).unwrap(),
        ));
    }
    for expected in [
        "https://example.com/q",
        "DM-77",
        "PDF417 text",
        "AZTEC-5",
        "C128-x",
        "010950110153000310LOT7",
        "C39-9",
        "C93-3",
        "4006381333931",
        "55123457",
        "725272730706",
        "06543217",
        "00123456",
        "00012345600012",
        "1234",
    ] {
        assert!(
            found.iter().any(|f| f == expected),
            "{expected} not decoded: {found:?}"
        );
    }
}
