//! QR styling, logos, labels, GS1 and Swiss QR codes, colour checks, the
//! `qrdata` payloads and alt text. Every drawing is read back with rxing:
//! a style that renders but no longer scans is a failure.

mod common;

use common::*;
use rusticdoc_template::{compile, Format};
use serde_json::{json, Value};

fn render(format: Format, bytes: &[u8], data: Value) -> Vec<u8> {
    compile(format, bytes)
        .expect("compiles")
        .render(&data)
        .expect("renders")
}

fn render_err(format: Format, bytes: &[u8], data: Value) -> String {
    compile(format, bytes)
        .expect("compiles")
        .render(&data)
        .expect_err("must fail")
        .message
}

/// A barcode tag rendered into a DOCX: its PNG and its SVG. With QR_KEEP
/// naming a folder, both are written there to look at or read with another
/// decoder.
fn draw(tag: &str, data: Value) -> (Vec<u8>, String) {
    let out = render(Format::Docx, &docx(&p(tag)), data);
    let png = bytes_of(&out, "word/media/rich1.png").expect("a PNG");
    let svg = bytes_of(&out, "word/media/rich2.svg").expect("an SVG");
    if let Ok(dir) = std::env::var("QR_KEEP") {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let name: String = tag
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .take(90)
            .collect();
        std::fs::write(format!("{dir}/{n:04}-{name}.png"), &png).unwrap();
        std::fs::write(format!("{dir}/{n:04}-{name}.svg"), &svg).unwrap();
    }
    (png, String::from_utf8(svg).unwrap())
}

/// Reads the one barcode in a PNG: its text and symbology identifier.
fn read(png: &[u8]) -> (String, String) {
    let img = image::load_from_memory(png).unwrap().to_luma8();
    let (w, h) = img.dimensions();
    // Light-on-dark codes too, as phone cameras read them.
    let mut hints = rxing::DecodeHints {
        TryHarder: Some(true),
        AlsoInverted: Some(true),
        ..rxing::DecodeHints::default()
    };
    let found = rxing::helpers::detect_in_luma_with_hints(img.into_raw(), w, h, None, &mut hints)
        .unwrap_or_else(|e| panic!("no barcode found: {e}"));
    let id = found
        .getRXingResultMetadata()
        .get(&rxing::RXingResultMetadataType::SYMBOLOGY_IDENTIFIER)
        .map(|m| match m {
            rxing::RXingResultMetadataValue::SymbologyIdentifier(s) => s.clone(),
            other => format!("{other:?}"),
        })
        .unwrap_or_default();
    (found.getText().to_string(), id)
}

fn scans(tag: &str, data: Value, expected: &str) -> (Vec<u8>, String) {
    let (png, svg) = draw(tag, data);
    assert_eq!(read(&png).0, expected, "{tag}");
    (png, svg)
}

const URL: &str = "https://example.com/orders/2026-0042?ref=print";

#[test]
fn every_style_scans() {
    for style in ["square", "dots", "rounded"] {
        for eye in ["square", "rounded", "circle"] {
            for centre in ["square", "rounded", "circle"] {
                for ec in ["L", "H"] {
                    let tag = format!(
                        "{{{{ v | barcode(\"qr\", style=\"{style}\", eye=\"{eye}\", \
                         eye_center=\"{centre}\", ec=\"{ec}\") }}}}"
                    );
                    scans(&tag, json!({ "v": URL }), URL);
                }
            }
        }
    }
    for scale in [0.7, 0.85, 1.0] {
        for style in ["square", "dots", "rounded"] {
            let tag =
                format!("{{{{ v | barcode(\"qr\", style=\"{style}\", module_scale={scale}) }}}}");
            scans(&tag, json!({ "v": URL }), URL);
        }
    }
}

#[test]
fn the_default_style_is_unchanged_and_crisp() {
    let (png, svg) = scans(
        "{{ v | barcode(\"qr\", size=100) }}",
        json!({ "v": "a" }),
        "a",
    );
    // Whole pixels per module, and only black and white.
    let img = image::load_from_memory(&png).unwrap();
    assert_eq!(img.width() % 29, 0);
    assert!(img
        .to_luma8()
        .pixels()
        .all(|p| p.0[0] == 0 || p.0[0] == 255));
    assert!(svg.contains("shape-rendering=\"crispEdges\""), "{svg}");
    assert!(svg.contains("width=\"100pt\" height=\"100pt\""), "{svg}");
}

#[test]
fn colours_and_gradients_scan() {
    let cases = [
        "color=\"#2a7a6f\"",
        "color=\"#c2185b\", style=\"rounded\", eye=\"rounded\"",
        "color=\"#123a6b\", eye_color=\"#c00\", eye_center_color=\"#000\"",
        "color=\"#123a6b\", gradient=\"linear\", gradient_color=\"#6a1b9a\"",
        "color=\"#000\", gradient=\"linear\", gradient_color=\"#1565c0\", gradient_angle=45",
        "color=\"#004d40\", gradient=\"radial\", gradient_color=\"#1b5e20\", style=\"dots\"",
        "invert=true",
        "invert=true, color=\"#ffeb3b\", background=\"#1a237e\"",
        "background=\"transparent\", style=\"dots\"",
    ];
    for options in cases {
        let tag = format!("{{{{ v | barcode(\"qr\", {options}) }}}}");
        let (png, svg) = draw(&tag, json!({ "v": URL }));
        // A transparent code needs a background to be read on.
        let png = if options.contains("transparent") {
            let img = image::load_from_memory(&png).unwrap().to_rgba8();
            let mut flat =
                image::RgbaImage::from_pixel(img.width(), img.height(), image::Rgba([255; 4]));
            image::imageops::overlay(&mut flat, &img, 0, 0);
            let mut out = Vec::new();
            flat.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
                .unwrap();
            out
        } else {
            png
        };
        assert_eq!(read(&png).0, URL, "{options}");
        if options.contains("gradient=") {
            assert!(svg.contains("Gradient id=\"rusticdoc-gradient\""), "{svg}");
        }
    }
}

#[test]
fn colours_a_scanner_cannot_read_are_refused() {
    let cases = [
        ("color=\"#ccc\"", "contrast of 1.6:1"),
        (
            "color=\"#777\", background=\"#555\"",
            "scanners need at least 3:1",
        ),
        ("color=\"#fff\", background=\"#000\"", "pass invert=true"),
        ("eye_color=\"#eee\"", "eye_color #eeeeee"),
        (
            "gradient=\"linear\", gradient_color=\"#ddd\"",
            "the gradient's end",
        ),
        (
            "color=\"#000\", background=\"#fff\", invert=true, eye_color=\"#ff0\"",
            "contrast",
        ),
        ("gradient=\"linear\"", "needs gradient_color"),
        ("gradient_color=\"#123\"", "needs gradient="),
        (
            "gradient=\"radial\", gradient_color=\"#123\", gradient_angle=10",
            "linear gradient",
        ),
        ("style=\"hearts\"", "style is square, dots or rounded"),
        ("eye=\"star\"", "eye is square, rounded or circle"),
        (
            "module_scale=0.5",
            "module_scale must be a number from 0.7 to 1",
        ),
        ("invert=\"yes\"", "invert is true or false"),
    ];
    for (options, expected) in cases {
        let tag = format!("{{{{ v | barcode(\"qr\", {options}) }}}}");
        let err = render_err(Format::Docx, &docx(&p(&tag)), json!({ "v": URL }));
        assert!(err.contains(expected), "{options}: {err}");
    }
    // The check is the same for every symbology.
    let err = render_err(
        Format::Docx,
        &docx(&p("{{ v | barcode(\"code128\", color=\"#ff0\") }}")),
        json!({ "v": "x" }),
    );
    assert!(err.contains("contrast"), "{err}");
    // A transparent background is the page's: nothing to compare with.
    draw(
        "{{ v | barcode(\"code128\", color=\"#fff\", background=\"transparent\") }}",
        json!({ "v": "x" }),
    );
}

/// A 200 by 100 PNG: blue on the left half, orange on the right.
fn logo_png() -> Vec<u8> {
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

const LOGO_SVG: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"40\" height=\"40\">\
    <circle cx=\"20\" cy=\"20\" r=\"18\" fill=\"#c2185b\"/>\
    <text x=\"20\" y=\"26\" font-size=\"16\" text-anchor=\"middle\" fill=\"#fff\">R</text></svg>";

#[test]
fn logos_sit_in_a_hole_and_still_scan() {
    let png_logo = b64(&logo_png());
    let svg_logo = b64(LOGO_SVG.as_bytes());
    for logo in [&png_logo, &svg_logo] {
        for (ec, sizes) in [
            ("M", &[0.1, 0.15][..]),
            ("Q", &[0.15, 0.2]),
            ("H", &[0.2, 0.25]),
        ] {
            for size in sizes {
                for value in ["short", URL, &"x".repeat(300)] {
                    let tag = format!(
                        "{{{{ v | barcode(\"qr\", logo=l, logo_size={size}, ec=\"{ec}\") }}}}"
                    );
                    let (_, svg) = scans(&tag, json!({ "v": value, "l": logo }), value);
                    assert!(
                        svg.contains("<image ") || svg.contains("<svg x="),
                        "the logo is in the SVG"
                    );
                }
            }
        }
    }
    // Excel flattens the curves of an SVG that holds a raster picture until
    // rounded modules stop scanning, so with a PNG logo curves are lines.
    // An SVG logo nests as vectors and keeps them.
    let curves = |svg: &str| {
        svg.split(" d=\"")
            .skip(1)
            .any(|d| d.split('"').next().unwrap_or("").contains('C'))
    };
    let tag = "{{ v | barcode(\"qr\", logo=l, style=\"rounded\", eye=\"circle\") }}";
    let (_, svg) = scans(tag, json!({ "v": URL, "l": png_logo }), URL);
    assert!(!curves(&svg), "a raster logo: lines only");
    let (_, svg) = scans(tag, json!({ "v": URL, "l": svg_logo }), URL);
    assert!(curves(&svg), "an SVG logo keeps curves");

    // Styled, coloured and with a logo, all at once.
    scans(
        "{{ v | barcode(\"qr\", logo=l, style=\"rounded\", eye=\"circle\", \
         eye_center=\"circle\", color=\"#2a7a6f\") }}",
        json!({ "v": URL, "l": png_logo }),
        URL,
    );
}

#[test]
fn a_logo_that_unpacks_to_too_many_pixels_is_refused() {
    use image::ImageEncoder;
    // 20 megapixels of one grey: a few kilobytes of PNG.
    let (w, h) = (5000u32, 4000u32);
    let mut bomb = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bomb)
        .write_image(
            &vec![200u8; (w * h) as usize],
            w,
            h,
            image::ExtendedColorType::L8,
        )
        .unwrap();
    assert!(bomb.len() < 200_000, "{} bytes", bomb.len());
    let err = render_err(
        Format::Docx,
        &docx(&p("{{ v | barcode(\"qr\", logo=l) }}")),
        json!({ "v": "a", "l": b64(&bomb) }),
    );
    assert!(
        err.contains("a logo has at most 16000000 pixels, not 20000000"),
        "{err}"
    );
}

#[test]
fn a_logo_raises_the_version_until_it_fits_or_says_why() {
    // Version 1 has no room in the middle for a logo; the code grows.
    let (plain, _) = draw("{{ v | barcode(\"qr\", size=100) }}", json!({ "v": "a" }));
    let (with, _) = draw(
        "{{ v | barcode(\"qr\", size=100, logo=l) }}",
        json!({ "v": "a", "l": b64(&logo_png()) }),
    );
    let width = |png: &[u8]| image::load_from_memory(png).unwrap().width();
    // Version 1 is 21 modules and version 2 is 25, each with 8 of quiet zone.
    assert_eq!(width(&plain) % 29, 0);
    assert_eq!(width(&with) % 33, 0);
    assert_eq!(read(&with).0, "a");
    let cases = [
        ("logo=l, version=1", "hides too much of this version 1 code"),
        (
            "logo=l, ec=\"L\"",
            "a logo needs error correction M, Q or H",
        ),
        (
            "logo=l, logo_size=0.4",
            "logo_size must be a number from 0.05 to 0.25",
        ),
        ("logo_size=0.2", "logo_size needs a logo"),
        ("logo=\"nope\"", "logo:"),
    ];
    for (options, expected) in cases {
        let tag = format!("{{{{ v | barcode(\"qr\", {options}) }}}}");
        let err = render_err(
            Format::Docx,
            &docx(&p(&tag)),
            json!({ "v": "a", "l": b64(&logo_png()) }),
        );
        assert!(err.contains(expected), "{options}: {err}");
    }
}

#[test]
fn versions_labels_and_print_size() {
    let (png, _) = scans(
        "{{ v | barcode(\"qr\", version=10) }}",
        json!({ "v": "a" }),
        "a",
    );
    let img = image::load_from_memory(&png).unwrap();
    // Version 10 is 57 modules, plus a quiet zone of 4 on each side.
    assert_eq!(img.width() % 65, 0);
    let err = render_err(
        Format::Docx,
        &docx(&p("{{ v | barcode(\"qr\", version=1) }}")),
        json!({ "v": "x".repeat(40) }),
    );
    assert!(err.contains("does not fit a version 1 QR code"), "{err}");

    let (png, svg) = scans(
        "{{ v | barcode(\"qr\", label=\"Scan to pay\", width=120) }}",
        json!({ "v": URL }),
        URL,
    );
    let img = image::load_from_memory(&png).unwrap();
    assert!(img.height() > img.width(), "the label adds height");
    assert!(svg.contains("width=\"120pt\""), "{svg}");
    for (options, expected) in [
        ("label=\"Zahlen \u{2192}\"", "printable ASCII"),
        ("label=\"x\", height=90", "a label sets the height"),
        ("font_size=9", "font_size needs a label"),
        ("text=true", "a qr code takes a label"),
        (
            "size=20",
            "its modules would print 0.17 mm wide, and phones need at least 0.25 mm: make it at least 30 points wide, or the value shorter",
        ),
        ("version=41", "version must be a whole number from 0 to 40"),
        ("version=0", "version must be a whole number from 1 to 40"),
    ] {
        let tag = format!("{{{{ v | barcode(\"qr\", {options}) }}}}");
        let err = render_err(Format::Docx, &docx(&p(&tag)), json!({ "v": URL }));
        assert!(err.contains(expected), "{options}: {err}");
    }
    for (kind, options) in [
        ("code128", "style=\"dots\""),
        ("datamatrix", "version=2"),
        ("aztec", "label=\"x\""),
        ("pdf417", "eye=\"circle\""),
    ] {
        let tag = format!("{{{{ v | barcode(\"{kind}\", {options}) }}}}");
        let err = render_err(Format::Docx, &docx(&p(&tag)), json!({ "v": "ABC" }));
        assert!(err.contains("applies to qr and gs1-qr"), "{kind}: {err}");
    }
}

#[test]
fn non_ascii_text_carries_an_eci_header() {
    let value = "Zürich – 東京 ✓";
    let (png, _) = draw("{{ v | barcode(\"qr\") }}", json!({ "v": value }));
    let (text, id) = read(&png);
    assert_eq!(text, value);
    // ]Q2: a QR code whose data starts with an ECI designator.
    assert_eq!(id, "]Q2");
    let (png, _) = draw("{{ v | barcode(\"qr\") }}", json!({ "v": "plain" }));
    assert_eq!(read(&png).1, "]Q1");
}

#[test]
fn gs1_qr_codes_start_with_fnc1() {
    let (png, _) = draw(
        "{{ v | barcode(\"gs1-qr\") }}",
        json!({ "v": "(01)09506000134352(10)AB-12(17)261231" }),
    );
    let (text, id) = read(&png);
    // ]Q3: FNC1 in first position. A variable field ends with GS.
    assert_eq!(id, "]Q3");
    assert_eq!(text, "010950600013435210AB-12\u{1d}17261231");
    let err = render_err(
        Format::Docx,
        &docx(&p("{{ v | barcode(\"gs1-qr\") }}")),
        json!({ "v": "(01)09506000134353" }),
    );
    assert!(err.contains("wrong check digit"), "{err}");
}

fn swiss_payload() -> Value {
    json!({
        "account": "CH44 3199 9123 0008 8901 2",
        "creditor": {"name": "Robert Schneider AG", "street": "Rue du Lac", "number": "1268",
                     "postcode": "2501", "city": "Biel", "country": "CH"},
        "amount": 1949.75,
        "reference": "210000000003139471430009017",
        "message": "Order of 15 June 2020"
    })
}

#[test]
fn swiss_qr_bills_are_46_mm_with_their_cross() {
    let tag = "{{ bill | qrdata(\"swiss\") | barcode(\"swissqr\") }}";
    let out = render(
        Format::Docx,
        &docx(&p(tag)),
        json!({ "bill": swiss_payload() }),
    );
    let png = bytes_of(&out, "word/media/rich1.png").unwrap();
    let (text, _) = read(&png);
    assert!(text.starts_with("SPC\n0200\n1\nCH4431999123000889012\nS\nRobert Schneider AG"));
    assert!(text.contains("\n1949.75\nCHF\n"));
    let svg = String::from_utf8(bytes_of(&out, "word/media/rich2.svg").unwrap()).unwrap();
    assert!(
        svg.contains("fill=\"#000000\"/><path fill=\"#ffffff\""),
        "the cross: {svg}"
    );
    // The code is 46 mm; its quiet zone takes the width to at least 56 mm.
    let xml = part(&out, "word/document.xml");
    let cx: f64 = xml
        .split("<wp:extent cx=\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .unwrap()
        .parse()
        .unwrap();
    let mm = cx / 36000.0;
    assert!((56.0..58.0).contains(&mm), "{mm} mm");
    for (options, expected) in [
        (", color=\"#123\"", "color cannot change"),
        (", size=200", "size cannot change"),
        (", style=\"dots\"", "style cannot change"),
    ] {
        let tag = format!("{{{{ bill | qrdata(\"swiss\") | barcode(\"swissqr\"{options}) }}}}");
        let err = render_err(
            Format::Docx,
            &docx(&p(&tag)),
            json!({ "bill": swiss_payload() }),
        );
        assert!(err.contains(expected), "{options}: {err}");
    }
    let err = render_err(
        Format::Docx,
        &docx(&p("{{ v | barcode(\"swissqr\") }}")),
        json!({ "v": "hello" }),
    );
    assert!(err.contains("starts with SPC"), "{err}");
}

#[test]
fn qrdata_payloads_scan_back_exactly() {
    let cases = [
        (
            "wifi",
            json!({"ssid": "Office; 2nd floor", "password": "s3cr3t:\"x\""}),
            "WIFI:T:WPA;S:Office\\; 2nd floor;P:s3cr3t\\:\\\"x\\\";;",
        ),
        ("tel", json!("+41 44 668 18 00"), "tel:+41446681800"),
        (
            "email",
            json!({"to": "billing@example.com", "subject": "Invoice 42"}),
            "mailto:billing@example.com?subject=Invoice%2042",
        ),
        (
            "geo",
            json!({"lat": 46.948, "lon": 7.4474}),
            "geo:46.948,7.4474",
        ),
        (
            "epc",
            json!({"name": "Acme GmbH", "iban": "DE89 3704 0044 0532 0130 00", "amount": 99.5,
                   "text": "Invoice 2026-001"}),
            "BCD\n002\n1\nSCT\n\nAcme GmbH\nDE89370400440532013000\nEUR99.50\n\n\nInvoice 2026-001",
        ),
        (
            "gs1",
            json!("(01)09506000134352(10)LOT7"),
            "https://id.gs1.org/01/09506000134352/10/LOT7",
        ),
    ];
    for (kind, value, expected) in cases {
        let tag = format!("{{{{ v | qrdata(\"{kind}\") | barcode(\"qr\") }}}}");
        scans(&tag, json!({ "v": value }), expected);
    }
    // qrdata is plain text, so it works in text templates and prints.
    let out = render(
        Format::Docx,
        &docx(&p("{{ v | qrdata(\"tel\") }}")),
        json!({ "v": "555 0100" }),
    );
    assert!(part(&out, "word/document.xml").contains("tel:5550100"));
    for (tag, expected) in [
        ("{{ v | qrdata(\"fax\") }}", "unknown kind \"fax\""),
        ("{{ v | qrdata }}", "name the kind"),
        ("{{ v | qrdata(\"wifi\") }}", "takes an object"),
        (
            "{{ v | qrdata(\"tel\", domain=\"x\") }}",
            "domain is for gs1",
        ),
    ] {
        let err = render_err(Format::Docx, &docx(&p(tag)), json!({ "v": "1" }));
        assert!(err.contains(expected), "{tag}: {err}");
    }
}

#[test]
fn pictures_carry_alt_text_in_every_format() {
    let data = json!({ "v": URL, "img": b64(&logo_png()) });
    let tags = "{{ v | barcode(\"qr\") }} {{ v | barcode(\"ean13\", alt=\"Product code\") }} \
                {{ img | image(alt=\"Company logo\") }} {{ img | image }}";
    let out = render(
        Format::Docx,
        &docx(&p(tags)),
        json!({ "v": "590123412345", "img": b64(&logo_png()) }),
    );
    let xml = part(&out, "word/document.xml");
    assert!(
        xml.contains("name=\"Picture 1\" descr=\"QR code\""),
        "{xml}"
    );
    assert!(xml.contains("descr=\"Product code\""), "{xml}");
    assert!(xml.contains("descr=\"Company logo\""), "{xml}");
    assert_eq!(
        xml.matches("descr=").count(),
        6,
        "docPr and pic:cNvPr each: {xml}"
    );

    let out = render(
        Format::Pptx,
        &pptx(&[&shape(&[
            "{{ v | barcode(\"qr\", alt=\"Pay <here> & now\") }}",
        ])]),
        data.clone(),
    );
    assert!(part(&out, "ppt/slides/slide1.xml").contains("descr=\"Pay &lt;here> &amp; now\""));

    let doc = xlsx(
        &[row(
            1,
            &[cell("A1", "inlineStr", "{{ v | barcode(\"qr\") }}")],
        )],
        &[],
    );
    let out = render(Format::Xlsx, &doc, data.clone());
    let drawing = part(&out, "xl/drawings/drawing1.xml");
    assert!(drawing.contains("descr=\"QR code\""), "{drawing}");

    let out = render(
        Format::Odt,
        &odt("<text:p>{{ img | image(alt=\"Logo\") }}</text:p>"),
        data.clone(),
    );
    assert!(part(&out, "content.xml").contains("<svg:desc>Logo</svg:desc></draw:frame>"));

    let err = render_err(
        Format::Docx,
        &docx(&p("{{ v | barcode(\"qr\", alt=v * 1000) }}")),
        json!({ "v": "long " }),
    );
    assert!(err.contains("alt holds at most 1000 characters"), "{err}");
}

/// Styled codes go through LibreOffice, which draws the SVG itself, and
/// are read back from the PDF.
#[test]
fn styled_codes_scan_after_libreoffice_draws_them() {
    if !libreoffice_available() {
        eprintln!("skipping: docker images not available");
        return;
    }
    let tags = [
        ("dots", "{{ a | barcode(\"qr\", style=\"dots\", eye=\"circle\", eye_center=\"circle\", size=150) }}"),
        ("rounded", "{{ b | barcode(\"qr\", style=\"rounded\", eye=\"rounded\", eye_center=\"rounded\", color=\"#c2185b\", size=150) }}"),
        ("gradient", "{{ c | barcode(\"qr\", gradient=\"linear\", color=\"#123a6b\", gradient_color=\"#6a1b9a\", size=150) }}"),
        ("logo", "{{ d | barcode(\"qr\", logo=l, ec=\"H\", size=150) }}"),
        ("label", "{{ e | barcode(\"qr\", label=\"Scan me\", size=150) }}"),
        ("swiss", "{{ bill | qrdata(\"swiss\") | barcode(\"swissqr\") }}"),
        // LibreOffice draws nothing for a gradient at exactly 45 degrees.
        ("diagonal", "{{ f | barcode(\"qr\", gradient=\"linear\", color=\"#123a6b\", gradient_color=\"#6a1b9a\", gradient_angle=45, size=150) }}"),
        ("radial", "{{ g | barcode(\"qr\", gradient=\"radial\", color=\"#004d40\", gradient_color=\"#1b5e20\", style=\"dots\", size=150) }}"),
    ];
    let body: String = tags.iter().map(|(_, t)| p(t)).collect();
    let data = json!({
        "a": "https://example.com/a", "b": "https://example.com/b",
        "c": "https://example.com/c", "d": "https://example.com/d",
        "e": "https://example.com/e", "f": "https://example.com/f",
        "g": "https://example.com/g", "l": b64(&logo_png()), "bill": swiss_payload()
    });
    let out = render(Format::Docx, &docx(&body), data);
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().to_str().unwrap();
    std::fs::write(format!("{dir}/qr.docx"), &out).unwrap();
    assert!(soffice(dir, "pdf", "qr.docx"), "conversion failed");
    let codes = pdf_barcodes(dir, "qr.pdf");
    for letter in ["a", "b", "c", "d", "e", "f", "g"] {
        let url = format!("https://example.com/{letter}");
        assert!(codes.contains(&url), "{url} does not scan: {codes:?}");
    }
    assert!(
        codes.iter().any(|c| c.starts_with("SPC\n0200")),
        "{codes:?}"
    );
}

/// Writes a DOCX, PPTX and XLSX holding every QR style, a logo, a label,
/// the Swiss QR-bill code and linear codes, for opening in Word,
/// PowerPoint and Excel by hand: `QR_OFFICE=<folder> cargo test -p
/// rusticdoc-template --test qr office_samples -- --ignored`.
#[test]
#[ignore]
fn office_samples() {
    let dir = std::env::var("QR_OFFICE").expect("QR_OFFICE names a folder");
    let cases: &[(&str, &str)] = &[
        ("square", "{{ u.square | barcode(\"qr\", size=110, alt=\"Square QR\") }}"),
        ("dots", "{{ u.dots | barcode(\"qr\", style=\"dots\", eye=\"circle\", size=110) }}"),
        ("rounded", "{{ u.rounded | barcode(\"qr\", style=\"rounded\", eye=\"rounded\", color=\"#c2185b\", size=110) }}"),
        ("gradient45", "{{ u.gradient45 | barcode(\"qr\", gradient=\"linear\", color=\"#123a6b\", gradient_color=\"#6a1b9a\", gradient_angle=45, size=110) }}"),
        ("radial", "{{ u.radial | barcode(\"qr\", gradient=\"radial\", color=\"#004d40\", gradient_color=\"#1b5e20\", style=\"dots\", size=110) }}"),
        ("logo", "{{ u.logo | barcode(\"qr\", logo=png, style=\"rounded\", color=\"#2a7a6f\", size=110) }}"),
        ("svglogo", "{{ u.svglogo | barcode(\"qr\", logo=svg, eye=\"circle\", size=110) }}"),
        ("label", "{{ u.label | barcode(\"qr\", label=\"Scan to pay\", size=110) }}"),
        ("invert", "{{ u.invert | barcode(\"qr\", invert=true, size=110) }}"),
        ("swiss", "{{ bill | qrdata(\"swiss\") | barcode(\"swissqr\") }}"),
        ("ean13", "{{ \"590123412345\" | barcode(\"ean13\", text=true, width=150) }}"),
        ("code128", "{{ \"INV-2041/abc\" | barcode(\"code128\", text=true, width=170, color=\"#123a6b\") }}"),
        ("datamatrix", "{{ \"Lot 42/7\" | barcode(\"datamatrix\", size=70) }}"),
    ];
    let urls: serde_json::Map<String, Value> = cases
        .iter()
        .map(|(k, _)| (k.to_string(), json!(format!("https://example.com/{k}"))))
        .collect();
    let data = json!({
        "u": urls, "png": b64(&logo_png()), "svg": b64(LOGO_SVG.as_bytes()),
        "bill": swiss_payload()
    });
    let body: String = cases
        .iter()
        .map(|(k, t)| format!("{}{}", p(k), p(t)))
        .collect();
    std::fs::write(
        format!("{dir}/qr-styles.docx"),
        render(Format::Docx, &docx(&body), data.clone()),
    )
    .unwrap();
    // The logo code first, so a slide or a printed sheet doesn't cut it off.
    let short: Vec<String> = [5, 6, 0, 1, 2, 3]
        .iter()
        .map(|&i| cases[i].1.replace("size=110", "size=60"))
        .collect();
    let slide = short.clone();
    let refs: Vec<&str> = slide.iter().map(String::as_str).collect();
    std::fs::write(
        format!("{dir}/qr-styles.pptx"),
        render(Format::Pptx, &pptx(&[&shape(&refs)]), data.clone()),
    )
    .unwrap();
    // Two columns of three, so no page break cuts a code.
    let rows: Vec<String> = (0..3)
        .map(|i| {
            let r = (i * 9 + 1) as u32;
            row(
                r,
                &[
                    cell(&format!("A{r}"), "inlineStr", &short[i]),
                    cell(&format!("E{r}"), "inlineStr", &short[i + 3]),
                ],
            )
        })
        .collect();
    std::fs::write(
        format!("{dir}/qr-styles.xlsx"),
        render(Format::Xlsx, &xlsx(&rows, &[]), data),
    )
    .unwrap();
}
