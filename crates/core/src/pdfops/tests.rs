//! Engine tests. The lopdf engine runs everywhere; the process engines run
//! only where their binary exists (see `tests/pdfops-container.sh` for the
//! checks against the real binaries in the container).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ::lopdf::{Document as Pdf, Object, dictionary};

use super::fixtures;
use super::lopdf::{embedded_file_names, read_outline, write_outline, xmp_packet};
use super::*;

const T: Duration = Duration::from_secs(20);

fn doc(name: &str, bytes: Vec<u8>) -> Document {
    Document::new(name, bytes)
}

fn parse(doc: &Document) -> Pdf {
    Pdf::load_mem(&doc.bytes).expect("output parses")
}

fn page_text(pdf: &Pdf, page: u32) -> String {
    let id = pdf.get_pages()[&page];
    String::from_utf8_lossy(&pdf.get_page_content(id)).into_owned()
}

fn text_overlay(text: &str) -> Overlay {
    Overlay {
        kind: OverlayKind::Text {
            text: text.into(),
            font_size: 24.0,
            colour: "#ff0000".into(),
        },
        position: Position::Center,
        opacity: 0.4,
        rotation: 45.0,
        scale: 0.5,
        pages: String::new(),
    }
}

#[tokio::test]
async fn merge_keeps_pages_bookmarks_and_fields() {
    let out = LopdfEngine::default()
        .merge(
            &[
                doc("a.pdf", fixtures::pages(2)),
                doc("b.pdf", fixtures::outlined()),
                doc("c.pdf", fixtures::form()),
            ],
            T,
        )
        .await
        .unwrap();
    assert_eq!(out.name, "a.pdf");
    let pdf = parse(&out);
    assert_eq!(pdf.get_pages().len(), 6);
    assert!(page_text(&pdf, 1).contains("(Page 1)"));
    assert!(page_text(&pdf, 3).contains("(Chapter 1)"));
    assert!(page_text(&pdf, 6).contains("(Form)"));
    let bookmarks = read_outline(&pdf, fixtures::far()).unwrap();
    assert_eq!(bookmarks.len(), 2);
    assert_eq!(bookmarks[0].page, 3);
    assert_eq!(bookmarks[0].children[0].page, 4);
    assert_eq!(bookmarks[1].page, 5);
    let fields = pdf
        .catalog()
        .unwrap()
        .get(b"AcroForm")
        .unwrap()
        .as_dict()
        .unwrap();
    assert_eq!(fields.get(b"Fields").unwrap().as_array().unwrap().len(), 2);
    // Every object id is unique and resolvable.
    for id in pdf.get_pages().values() {
        assert!(pdf.get_dictionary(*id).is_ok());
    }
}

#[tokio::test]
async fn split_by_ranges_unified_and_by_interval() {
    let input = doc("report.pdf", fixtures::pages(5));
    let parts = LopdfEngine::default()
        .split(
            &input,
            &SplitMode::Pages {
                ranges: "1-3,5".into(),
                unify: false,
            },
            T,
        )
        .await
        .unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].name, "report-1-3.pdf");
    assert_eq!(parts[1].name, "report-5.pdf");
    assert_eq!(parse(&parts[0]).get_pages().len(), 3);
    let p5 = parse(&parts[1]);
    assert_eq!(p5.get_pages().len(), 1);
    assert!(page_text(&p5, 1).contains("(Page 5)"));

    let unified = LopdfEngine::default()
        .split(
            &input,
            &SplitMode::Pages {
                ranges: "1-3,5".into(),
                unify: true,
            },
            T,
        )
        .await
        .unwrap();
    assert_eq!(unified.len(), 1);
    assert_eq!(unified[0].name, "report-1-3_5.pdf");
    assert_eq!(parse(&unified[0]).get_pages().len(), 4);

    let spans = LopdfEngine::default()
        .split(&input, &SplitMode::Intervals { span: 2 }, T)
        .await
        .unwrap();
    let names: Vec<&str> = spans.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["report-1-2.pdf", "report-3-4.pdf", "report-5.pdf"]);

    // Bookmarks follow their pages.
    let parts = LopdfEngine::default()
        .split(
            &doc("o.pdf", fixtures::outlined()),
            &SplitMode::Pages {
                ranges: "2-3".into(),
                unify: false,
            },
            T,
        )
        .await
        .unwrap();
    let bookmarks = read_outline(&parse(&parts[0]), fixtures::far()).unwrap();
    assert_eq!(bookmarks.len(), 1);
    assert_eq!(
        (bookmarks[0].title.as_str(), bookmarks[0].page),
        ("Chapter 2", 2)
    );

    let err = LopdfEngine::default()
        .split(
            &input,
            &SplitMode::Pages {
                ranges: "9".into(),
                unify: false,
            },
            T,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidRequest(_)), "{err}");
}

#[tokio::test]
async fn rotate_adds_to_the_existing_rotation() {
    let input = doc("r.pdf", fixtures::pages(3));
    let once = LopdfEngine::default()
        .rotate(&input, Rotation::R90, "1-2", T)
        .await
        .unwrap();
    let twice = LopdfEngine::default()
        .rotate(&once, Rotation::R270, "2", T)
        .await
        .unwrap();
    let pdf = parse(&twice);
    let rotation = |n: u32| {
        pdf.get_dictionary(pdf.get_pages()[&n])
            .unwrap()
            .get(b"Rotate")
            .and_then(Object::as_i64)
            .unwrap_or(0)
    };
    assert_eq!((rotation(1), rotation(2), rotation(3)), (90, 0, 0));
}

#[tokio::test]
async fn metadata_round_trips_through_info_and_xmp() {
    let input = doc("meta.pdf", fixtures::pdfa_like());
    let before = LopdfEngine::default()
        .read_metadata(&input, T)
        .await
        .unwrap();
    assert_eq!(before["Title"], "Fixture");
    assert_eq!(before["xmp"]["dc:creator"], "rusticdoc");
    assert_eq!(before["xmp"]["pdfaid:part"], "2");

    let mut update = Metadata::new();
    update.insert("Title".into(), "Invoice 42 & co".into());
    update.insert("Author".into(), "Zoë".into());
    update.insert("Subject".into(), "Tests".into());
    update.insert("Keywords".into(), "a, b".into());
    update.insert("Creator".into(), "rusticdoc".into());
    update.insert("ModDate".into(), "2025-06-07T08:09:10+02:00".into());
    update.insert("Producer".into(), serde_json::Value::Null);
    update.insert("Department".into(), "Billing".into());
    let out = LopdfEngine::default()
        .write_metadata(&input, &update, T)
        .await
        .unwrap();
    let pdf = parse(&out);
    let after = LopdfEngine::default().read_metadata(&out, T).await.unwrap();
    assert_eq!(after["Title"], "Invoice 42 & co");
    assert_eq!(after["Author"], "Zoë");
    assert_eq!(after["ModDate"], "D:20250607080910+02'00'");
    assert_eq!(after["Department"], "Billing");
    assert!(after.get("Producer").is_none_or(|p| p != "lopdf"));
    let xml = xmp_packet(&pdf).unwrap();
    assert_eq!(
        xmp::get(&xml, "dc:title").as_deref(),
        Some("Invoice 42 & co")
    );
    assert_eq!(xmp::get(&xml, "dc:creator").as_deref(), Some("Zoë"));
    assert_eq!(xmp::get(&xml, "dc:description").as_deref(), Some("Tests"));
    assert_eq!(xmp::get(&xml, "pdf:Keywords").as_deref(), Some("a, b"));
    assert_eq!(
        xmp::get(&xml, "xmp:CreatorTool").as_deref(),
        Some("rusticdoc")
    );
    assert_eq!(
        xmp::get(&xml, "xmp:ModifyDate").as_deref(),
        Some("2025-06-07T08:09:10+02:00")
    );
    assert_eq!(xmp::get(&xml, "pdf:Producer"), None);
    assert_eq!(xmp::get(&xml, "pdfaid:part").as_deref(), Some("2"));
    // The metadata stream stays unfiltered and the packet well formed.
    let id = pdf
        .catalog()
        .unwrap()
        .get(b"Metadata")
        .unwrap()
        .as_reference()
        .unwrap();
    assert!(
        pdf.get_object(id)
            .unwrap()
            .as_stream()
            .unwrap()
            .dict
            .get(b"Filter")
            .is_err()
    );
    assert_eq!(
        xml.matches("<rdf:Description").count(),
        xml.matches("</rdf:Description>").count()
    );

    // A plain PDF gets an Info dictionary and no XMP.
    let plain = LopdfEngine::default()
        .write_metadata(&doc("p.pdf", fixtures::pages(1)), &update, T)
        .await
        .unwrap();
    let pdf = parse(&plain);
    assert!(xmp_packet(&pdf).is_none());
    assert_eq!(
        LopdfEngine::default()
            .read_metadata(&plain, T)
            .await
            .unwrap()["Title"],
        "Invoice 42 & co"
    );
}

#[tokio::test]
async fn bookmarks_round_trip() {
    let input = doc("o.pdf", fixtures::outlined());
    assert_eq!(
        LopdfEngine::default()
            .read_bookmarks(&input, T)
            .await
            .unwrap(),
        fixtures::outline()
    );
    let new = vec![
        Bookmark {
            title: "Début".into(),
            page: 2,
            children: vec![],
        },
        Bookmark {
            title: "Far".into(),
            page: 99,
            children: vec![],
        },
    ];
    let out = LopdfEngine::default()
        .write_bookmarks(&input, &new, T)
        .await
        .unwrap();
    let read = LopdfEngine::default()
        .read_bookmarks(&out, T)
        .await
        .unwrap();
    assert_eq!(read.len(), 2);
    assert_eq!((read[0].title.as_str(), read[0].page), ("Début", 2));
    assert_eq!(read[1].page, 3, "pages past the end are clamped");
    let none = LopdfEngine::default()
        .write_bookmarks(&out, &[], T)
        .await
        .unwrap();
    assert!(
        LopdfEngine::default()
            .read_bookmarks(&none, T)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(parse(&none).catalog().unwrap().get(b"Outlines").is_err());
}

#[tokio::test]
async fn embed_and_facturx_attach_files() {
    let input = doc("inv.pdf", fixtures::pdfa_like());
    let files = vec![
        Attachment {
            name: "notes.txt".into(),
            bytes: b"hello".to_vec(),
            mime: "text/plain".into(),
        },
        Attachment {
            name: "data.json".into(),
            bytes: b"{}".to_vec(),
            mime: "application/json".into(),
        },
    ];
    let out = LopdfEngine::default()
        .embed(&input, &files, T)
        .await
        .unwrap();
    let pdf = parse(&out);
    assert_eq!(embedded_file_names(&pdf), ["data.json", "notes.txt"]);
    assert_eq!(
        pdf.catalog()
            .unwrap()
            .get(b"AF")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let xml = b"<rsm:CrossIndustryInvoice/>".to_vec();
    let out = LopdfEngine::default()
        .facturx(&out, &xml, FacturxProfile::En16931, T)
        .await
        .unwrap();
    let pdf = parse(&out);
    assert_eq!(
        embedded_file_names(&pdf),
        ["data.json", "factur-x.xml", "notes.txt"]
    );
    let af = pdf
        .catalog()
        .unwrap()
        .get(b"AF")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(af.len(), 3);
    let spec = pdf.get_dictionary(af[2].as_reference().unwrap()).unwrap();
    assert_eq!(
        spec.get(b"AFRelationship").unwrap().as_name().unwrap(),
        b"Alternative"
    );
    let packet = xmp_packet(&pdf).unwrap();
    assert_eq!(
        xmp::get(&packet, "fx:ConformanceLevel").as_deref(),
        Some("EN 16931")
    );
    assert_eq!(
        xmp::get(&packet, "fx:DocumentFileName").as_deref(),
        Some("factur-x.xml")
    );
    assert!(packet.contains("pdfaExtension:schemas"));
    assert_eq!(
        xmp::get(&packet, "pdfaid:part").as_deref(),
        Some("2"),
        "PDF/A-2 is left alone"
    );
    assert_eq!(
        xmp::get(&packet, "pdfaid:conformance").as_deref(),
        Some("B")
    );

    // A PDF/A-3 input gets conformance B; a second run replaces the file.
    let three = fixtures::pdfa_like();
    let three = String::from_utf8_lossy(&three).replace("<pdfaid:part>2", "<pdfaid:part>3");
    let out = LopdfEngine::default()
        .facturx(
            &doc("a3.pdf", three.into_bytes()),
            &xml,
            FacturxProfile::Minimum,
            T,
        )
        .await
        .unwrap();
    let out = LopdfEngine::default()
        .facturx(&out, &xml, FacturxProfile::Minimum, T)
        .await
        .unwrap();
    let pdf = parse(&out);
    assert_eq!(embedded_file_names(&pdf), ["factur-x.xml"]);
    let packet = xmp_packet(&pdf).unwrap();
    assert_eq!(xmp::get(&packet, "pdfaid:part").as_deref(), Some("3"));
    assert_eq!(
        xmp::get(&packet, "fx:ConformanceLevel").as_deref(),
        Some("MINIMUM")
    );
    assert_eq!(packet.matches("Factur-X PDFA Extension Schema").count(), 1);
}

#[tokio::test]
async fn flatten_draws_widgets_into_the_page() {
    let input = doc("form.pdf", fixtures::form());
    let out = LopdfEngine::default().flatten(&input, T).await.unwrap();
    let pdf = parse(&out);
    let page = pdf.get_dictionary(pdf.get_pages()[&1]).unwrap();
    assert!(page.get(b"Annots").is_err());
    assert!(pdf.catalog().unwrap().get(b"AcroForm").is_err());
    let content = page_text(&pdf, 1);
    assert_eq!(content.matches(" Do").count(), 2, "{content}");
    assert!(content.starts_with("q\n"), "{content}");
    assert!(content.contains("(Form)"));
    let xobjects = pdf
        .get_page_resources(pdf.get_pages()[&1])
        .unwrap()
        .0
        .unwrap();
    let xobjects = xobjects.get(b"XObject").unwrap().as_dict().unwrap();
    assert_eq!(xobjects.len(), 2);
    for (_, id) in xobjects.iter() {
        let form = pdf
            .get_object(id.as_reference().unwrap())
            .unwrap()
            .as_stream()
            .unwrap();
        assert_eq!(
            form.dict.get(b"Subtype").unwrap().as_name().unwrap(),
            b"Form"
        );
    }
    // The checkbox's "Yes" state was drawn, not "Off".
    let drawn: Vec<Vec<u8>> = xobjects
        .iter()
        .map(|(_, id)| {
            pdf.get_object(id.as_reference().unwrap())
                .unwrap()
                .as_stream()
                .unwrap()
                .content
                .clone()
        })
        .collect();
    assert!(drawn.iter().any(|c| c.starts_with(b"0 0 1 rg")));
    assert!(!drawn.iter().any(|c| c.starts_with(b"0 G")));

    // A widget without an appearance keeps the form.
    let mut raw = fixtures::form();
    let needle = b"/AP";
    let at = raw.windows(needle.len()).position(|w| w == needle).unwrap();
    raw[at..at + 3].copy_from_slice(b"/XP");
    let out = LopdfEngine::default()
        .flatten(&doc("f.pdf", raw), T)
        .await
        .unwrap();
    let pdf = parse(&out);
    assert!(pdf.catalog().unwrap().get(b"AcroForm").is_ok());
    assert_eq!(
        pdf.get_page_annotations(pdf.get_pages()[&1]).unwrap().len(),
        1
    );
}

#[tokio::test]
async fn text_stamps_go_over_and_watermarks_under_the_content() {
    let input = doc("t.pdf", fixtures::pages(2));
    let mut overlay = text_overlay("DRAFT (v1)");
    overlay.pages = "2".into();
    let stamped = LopdfEngine::default()
        .stamp(&input, &overlay, T)
        .await
        .unwrap();
    let pdf = parse(&stamped);
    assert!(!page_text(&pdf, 1).contains("DRAFT"));
    let content = page_text(&pdf, 2);
    let text_at = content.find("(Page 2)").unwrap();
    let stamp_at = content.find("(DRAFT \\(v1\\))").unwrap();
    assert!(stamp_at > text_at, "{content}");
    assert!(content.contains(" gs "));
    let resources = pdf
        .get_page_resources(pdf.get_pages()[&2])
        .unwrap()
        .0
        .unwrap();
    let states = resources.get(b"ExtGState").unwrap().as_dict().unwrap();
    let gs = pdf
        .get_dictionary(states.iter().next().unwrap().1.as_reference().unwrap())
        .unwrap();
    assert_eq!(gs.get(b"ca").unwrap().as_float().unwrap(), 0.4);
    assert!(content.contains("1 0 0 rg"));
    let fonts = pdf.get_page_fonts(pdf.get_pages()[&2]).unwrap();
    assert!(
        fonts
            .values()
            .any(|f| f.get(b"BaseFont").unwrap().as_name().unwrap() == b"Helvetica")
    );

    overlay.pages.clear();
    overlay.position = Position::BottomRight;
    overlay.rotation = 0.0;
    let marked = LopdfEngine::default()
        .watermark(&input, &overlay, T)
        .await
        .unwrap();
    let pdf = parse(&marked);
    let content = page_text(&pdf, 1);
    assert!(
        content.find("(DRAFT").unwrap() < content.find("(Page 1)").unwrap(),
        "{content}"
    );
    assert!(
        content.contains("Q\n"),
        "original content is wrapped in q/Q"
    );

    let mut bad = text_overlay("x");
    bad.pages = "7".into();
    assert!(LopdfEngine::default().stamp(&input, &bad, T).await.is_err());
}

#[tokio::test]
async fn image_and_pdf_overlays() {
    let input = doc("i.pdf", fixtures::pages(2));
    let image = Overlay {
        kind: OverlayKind::Image {
            bytes: fixtures::PNG_DOT.to_vec(),
            name: "dot.png".into(),
        },
        position: Position::TopLeft,
        opacity: 1.0,
        rotation: 0.0,
        scale: 0.25,
        pages: String::new(),
    };
    let out = LopdfEngine::default()
        .stamp(&input, &image, T)
        .await
        .unwrap();
    let pdf = parse(&out);
    assert!(
        page_text(&pdf, 1).contains("153 0 0 153"),
        "{}",
        page_text(&pdf, 1)
    );
    let images = pdf.get_page_images(pdf.get_pages()[&2]).unwrap();
    assert_eq!((images.len(), images[0].width, images[0].height), (1, 1, 1));

    let not_image = Overlay {
        kind: OverlayKind::Image {
            bytes: b"nope".to_vec(),
            name: "x.png".into(),
        },
        ..image.clone()
    };
    assert!(matches!(
        LopdfEngine::default()
            .stamp(&input, &not_image, T)
            .await
            .unwrap_err(),
        Error::InvalidRequest(_)
    ));

    let overlay = Overlay {
        kind: OverlayKind::Pdf {
            bytes: fixtures::outlined(),
            name: "template.pdf".into(),
        },
        ..image
    };
    let out = LopdfEngine::default()
        .watermark(&input, &overlay, T)
        .await
        .unwrap();
    let pdf = parse(&out);
    assert_eq!(pdf.get_pages().len(), 2);
    for n in 1..=2 {
        let content = page_text(&pdf, n);
        assert!(content.contains("/FxX"), "{content}");
        let (_, ids) = pdf.get_page_resources(pdf.get_pages()[&n]).unwrap();
        let resources = pdf
            .get_page_resources(pdf.get_pages()[&n])
            .unwrap()
            .0
            .cloned();
        let resources = resources
            .or_else(|| pdf.get_dictionary(ids[0]).ok().cloned())
            .unwrap();
        let xobjects = resources.get(b"XObject").unwrap().as_dict().unwrap();
        let form = pdf
            .get_object(xobjects.iter().next().unwrap().1.as_reference().unwrap())
            .unwrap();
        let form = form.as_stream().unwrap();
        let content = form
            .decompressed_content()
            .unwrap_or_else(|_| form.content.clone());
        let expected = ["(Chapter 1)", "(Section 1.1)"][n as usize - 1].as_bytes();
        assert!(
            content.windows(expected.len()).any(|w| w == expected),
            "{}",
            String::from_utf8_lossy(&content)
        );
        let fonts = form
            .dict
            .get(b"Resources")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Font")
            .unwrap();
        let font_id = fonts
            .as_dict()
            .unwrap()
            .get(b"F1")
            .unwrap()
            .as_reference()
            .unwrap();
        assert!(
            pdf.get_dictionary(font_id).is_ok(),
            "overlay resources were imported"
        );
    }
    // The overlay's own catalog and page tree were pruned.
    let catalogs = pdf
        .objects
        .values()
        .filter(|o| o.as_dict().is_ok_and(|d| d.has_type(b"Catalog")))
        .count();
    assert_eq!(catalogs, 1);
}

#[tokio::test]
async fn multi_engine_falls_through_unsupported_operations() {
    let input = doc("m.pdf", fixtures::pages(1));
    let none = MultiEngine::default();
    assert!(matches!(
        none.rotate(&input, Rotation::R90, "", T).await.unwrap_err(),
        Error::EngineUnavailable(_)
    ));
    let only_lopdf = MultiEngine::standard(None, None);
    assert_eq!(only_lopdf.names(), ["lopdf"]);
    assert!(matches!(
        only_lopdf.optimize(&input, T).await.unwrap_err(),
        Error::EngineUnavailable(_)
    ));
    assert!(
        only_lopdf
            .rotate(&input, Rotation::R90, "", T)
            .await
            .is_ok()
    );
    let with_processes = MultiEngine::standard(
        Some("/nonexistent/qpdf".into()),
        Some("/nonexistent/pdfcpu".into()),
    );
    assert_eq!(with_processes.names(), ["lopdf", "qpdf", "pdfcpu"]);
    // lopdf serves first; qpdf is reached for encryption and fails to start.
    assert!(
        with_processes
            .rotate(&input, Rotation::R90, "", T)
            .await
            .is_ok()
    );
    let err = with_processes
        .encrypt(&input, "", "owner", T)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Engine { engine: "qpdf", .. }), "{err}");
    let engines: Vec<Arc<dyn PdfEngine>> = vec![Arc::new(LopdfEngine::default())];
    assert!(!MultiEngine::new(engines).is_empty());
    assert!(matches!(
        LopdfEngine::default()
            .rotate(
                &doc("x.pdf", b"%PDF-1.4 junk".to_vec()),
                Rotation::R90,
                "",
                T
            )
            .await
            .unwrap_err(),
        Error::InvalidRequest(_)
    ));
    assert!(matches!(
        LopdfEngine::default()
            .rotate(&doc("x.txt", fixtures::pages(1)), Rotation::R90, "", T)
            .await
            .unwrap_err(),
        Error::InvalidRequest(_)
    ));
}

#[tokio::test]
async fn qpdf_refuses_bad_passwords_before_running() {
    let q = QpdfEngine::new("/nonexistent/qpdf".into());
    let input = doc("a.pdf", fixtures::pages(1));
    for bad in ["a\nb", "a\rb", "a\0b"] {
        let err = q.encrypt(&input, "", bad, T).await.unwrap_err();
        assert!(matches!(err, Error::InvalidRequest(_)), "{bad:?}: {err}");
        let err = q.encrypt(&input, bad, "owner", T).await.unwrap_err();
        assert!(matches!(err, Error::InvalidRequest(_)), "{bad:?}: {err}");
    }
    let err = q
        .encrypt(&doc("a.pdf", b"not a pdf".to_vec()), "", "owner", T)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidRequest(_)), "{err}");
    assert!(matches!(
        q.encrypt(&input, "", "", T).await.unwrap_err(),
        Error::InvalidRequest(_)
    ));
}

/// The qpdf binary of this machine, when there is one.
fn binary(env: &str, candidates: &[&str]) -> Option<std::path::PathBuf> {
    std::env::var(env)
        .ok()
        .map(std::path::PathBuf::from)
        .into_iter()
        .chain(candidates.iter().map(std::path::PathBuf::from))
        .find(|p| Path::new(p).exists())
}

#[tokio::test]
async fn qpdf_engine_against_the_binary() {
    let Some(bin) = binary(
        "RUSTICDOC_QPDF_BIN",
        &[
            "/usr/bin/qpdf",
            "/opt/homebrew/bin/qpdf",
            "/usr/local/bin/qpdf",
        ],
    ) else {
        eprintln!("qpdf is not installed here; skipping (run tests/pdfops-container.sh instead)");
        return;
    };
    let q = QpdfEngine::new(bin).with_linearize(true);
    let input = doc("q.pdf", fixtures::pages(5));
    let merged = q
        .merge(&[input.clone(), doc("b.pdf", fixtures::pages(2))], T)
        .await
        .unwrap();
    assert_eq!(parse(&merged).get_pages().len(), 7);
    let parts = q
        .split(
            &input,
            &SplitMode::Pages {
                ranges: "1-3,5".into(),
                unify: false,
            },
            T,
        )
        .await
        .unwrap();
    assert_eq!(
        parts
            .iter()
            .map(|p| parse(p).get_pages().len())
            .collect::<Vec<_>>(),
        [3, 1]
    );
    let spans = q
        .split(&input, &SplitMode::Intervals { span: 2 }, T)
        .await
        .unwrap();
    assert_eq!(
        spans.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["q-1-2.pdf", "q-3-4.pdf", "q-5.pdf"]
    );
    let rotated = q.rotate(&input, Rotation::R90, "1-2", T).await.unwrap();
    let pdf = parse(&rotated);
    assert_eq!(
        pdf.get_dictionary(pdf.get_pages()[&1])
            .unwrap()
            .get(b"Rotate")
            .unwrap()
            .as_i64()
            .unwrap(),
        90
    );
    let optimized = q.optimize(&input, T).await.unwrap();
    assert!(parse(&optimized).get_pages().len() == 5);
    let flat = q.flatten(&doc("f.pdf", fixtures::form()), T).await.unwrap();
    assert!(parse(&flat).catalog().unwrap().get(b"AcroForm").is_err());
    let encrypted = q.encrypt(&input, "user", "owner", T).await.unwrap();
    assert!(Pdf::load_mem(&encrypted.bytes).unwrap().was_encrypted());
}

#[tokio::test]
async fn pdfcpu_engine_against_the_binary() {
    let Some(bin) = binary(
        "RUSTICDOC_PDFCPU_BIN",
        &[
            "/usr/bin/pdfcpu",
            "/opt/homebrew/bin/pdfcpu",
            "/usr/local/bin/pdfcpu",
        ],
    ) else {
        eprintln!("pdfcpu is not installed here; skipping (run tests/pdfops-container.sh instead)");
        return;
    };
    let p = PdfcpuEngine::new(bin);
    let input = doc("p.pdf", fixtures::pages(5));
    let stamped = p.stamp(&input, &text_overlay("DRAFT"), T).await.unwrap();
    assert_eq!(parse(&stamped).get_pages().len(), 5);
    let marked = p
        .watermark(&input, &text_overlay("DRAFT"), T)
        .await
        .unwrap();
    assert_eq!(parse(&marked).get_pages().len(), 5);
    let rotated = p.rotate(&input, Rotation::R90, "1", T).await.unwrap();
    assert_eq!(parse(&rotated).get_pages().len(), 5);
    assert_eq!(
        parse(&p.optimize(&input, T).await.unwrap())
            .get_pages()
            .len(),
        5
    );
    let spans = p
        .split(&input, &SplitMode::Intervals { span: 2 }, T)
        .await
        .unwrap();
    assert_eq!(
        spans
            .iter()
            .map(|d| parse(d).get_pages().len())
            .collect::<Vec<_>>(),
        [2, 2, 1]
    );
    let merged = p
        .merge(&[input.clone(), doc("b.pdf", fixtures::pages(1))], T)
        .await
        .unwrap();
    assert_eq!(parse(&merged).get_pages().len(), 6);
    let outlined = doc("o.pdf", fixtures::outlined());
    assert_eq!(
        p.read_bookmarks(&outlined, T).await.unwrap(),
        fixtures::outline()
    );
    let written = p
        .write_bookmarks(&input, &fixtures::outline(), T)
        .await
        .unwrap();
    assert_eq!(
        read_outline(&parse(&written), fixtures::far()).unwrap(),
        fixtures::outline()
    );
}

#[test]
fn fixtures_parse() {
    let dir = tempfile::tempdir().unwrap();
    fixtures::write_all(dir.path()).unwrap();
    for name in [
        "three-pages.pdf",
        "outlined.pdf",
        "form.pdf",
        "pdfa-like.pdf",
    ] {
        let pdf = Pdf::load(dir.path().join(name)).unwrap();
        assert!(!pdf.get_pages().is_empty(), "{name}");
    }
    assert_eq!(Rotation::parse("-90").unwrap(), Rotation::R270);
    assert_eq!(
        FacturxProfile::parse("basic-wl").unwrap(),
        FacturxProfile::BasicWl
    );
}

/// The outlined fixture with a `Names/Dests` tree whose only node lists
/// itself as a kid, and whose first outline item uses a named destination.
fn cyclic_name_tree() -> Vec<u8> {
    let mut pdf = Pdf::load_mem(&fixtures::outlined()).unwrap();
    let page3 = pdf.get_pages()[&3];
    let node = pdf.new_object_id();
    pdf.objects.insert(
        node,
        dictionary! {
            "Limits" => vec![Object::string_literal("target"), Object::string_literal("target")],
            "Names" => vec![
                Object::string_literal("target"),
                vec![Object::Reference(page3), "Fit".into()].into(),
            ],
            "Kids" => vec![Object::Reference(node), Object::Reference(node), Object::Reference(node)],
        }
        .into(),
    );
    let root = pdf.add_object(dictionary! {
        "Kids" => vec![Object::Reference(node), Object::Reference(node)],
    });
    pdf.catalog_mut().unwrap().set(
        "Names",
        dictionary! { "Dests" => root, "EmbeddedFiles" => root },
    );
    let outlines = pdf
        .catalog()
        .unwrap()
        .get(b"Outlines")
        .unwrap()
        .as_reference()
        .unwrap();
    let first = pdf
        .get_dictionary(outlines)
        .unwrap()
        .get(b"First")
        .unwrap()
        .as_reference()
        .unwrap();
    let item = pdf
        .get_object_mut(first)
        .and_then(Object::as_dict_mut)
        .unwrap();
    item.set("Dest", Object::string_literal("target"));
    let next = item.get(b"Next").unwrap().as_reference().unwrap();
    pdf.get_object_mut(next)
        .and_then(Object::as_dict_mut)
        .unwrap()
        .set("Dest", Object::string_literal("missing"));
    let mut out = Vec::new();
    pdf.save_to(&mut out).unwrap();
    out
}

#[tokio::test]
async fn cyclic_name_trees_are_walked_once() {
    let input = doc("cyclic.pdf", cyclic_name_tree());
    let started = std::time::Instant::now();
    let bookmarks = LopdfEngine::default()
        .read_bookmarks(&input, T)
        .await
        .unwrap();
    assert_eq!(
        bookmarks[0].page, 3,
        "named destination resolved through /Limits"
    );
    assert_eq!(bookmarks[1].page, 1, "unknown names fall back to page 1");
    let names = embedded_file_names(&parse(&input));
    assert_eq!(names, ["target"], "each node is visited once");
    // Embedding into the cyclic tree rebuilds it flat.
    let files = [Attachment {
        name: "a.txt".into(),
        bytes: b"a".to_vec(),
        mime: "text/plain".into(),
    }];
    let out = LopdfEngine::default()
        .embed(&input, &files, T)
        .await
        .unwrap();
    assert_eq!(embedded_file_names(&parse(&out)), ["a.txt", "target"]);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn an_expired_deadline_stops_every_loop() {
    let expired = Deadline::new(Duration::ZERO);
    let input = doc("d.pdf", fixtures::outlined());
    let timeout = |r: Result<()>| assert!(matches!(r.unwrap_err(), Error::Timeout(_)));
    timeout(lopdf::merge(std::slice::from_ref(&input), expired).map(drop));
    let mode = SplitMode::Intervals { span: 1 };
    timeout(lopdf::split(&input, &mode, expired).map(drop));
    timeout(lopdf::rotate(&input, Rotation::R90, "", expired).map(drop));
    timeout(read_outline(&parse(&input), expired).map(drop));
    timeout(write_outline(
        &mut parse(&input),
        &fixtures::outline(),
        expired,
    ));
    timeout(draw::flatten(
        &mut parse(&doc("f.pdf", fixtures::form())),
        expired,
    ));
    timeout(draw::overlay(
        &mut parse(&input),
        &text_overlay("x"),
        true,
        expired,
    ));
    let err = LopdfEngine::default()
        .rotate(&input, Rotation::R90, "", Duration::ZERO)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Timeout(_)), "{err}");
    assert!(Deadline::new(Duration::from_secs(5)).check().is_ok());
}

#[tokio::test]
async fn request_sizes_are_capped() {
    let input = doc("big.pdf", fixtures::pages(1));
    let small = LopdfEngine::new().with_max_bytes(10);
    let err = small
        .rotate(&input, Rotation::R90, "", T)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidRequest(_)), "{err}");
    let spec = vec!["1"; lopdf::MAX_SPLIT_PARTS + 1].join(",");
    let mode = SplitMode::Pages {
        ranges: spec,
        unify: false,
    };
    let err = LopdfEngine::default()
        .split(&input, &mode, T)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidRequest(_)), "{err}");
    let zero = SplitMode::Intervals { span: 0 };
    let err = LopdfEngine::default()
        .split(&input, &zero, T)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidRequest(_)), "{err}");
    let big_overlay = Overlay {
        kind: OverlayKind::Image {
            bytes: vec![0; 11],
            name: "x.png".into(),
        },
        ..text_overlay("x")
    };
    assert!(matches!(
        small
            .stamp(&doc("s.pdf", vec![]), &big_overlay, T)
            .await
            .unwrap_err(),
        Error::InvalidRequest(_)
    ));
}
