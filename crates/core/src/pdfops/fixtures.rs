//! Small PDFs built in code, for the engine tests and the container checks.
//! They use the base-14 Helvetica without embedding it, so they are plain
//! PDFs; [`pdfa_like`] carries PDF/A-style metadata (Info plus XMP with
//! `pdfaid`) without being a valid PDF/A file.

use std::path::Path;

use ::lopdf::content::{Content, Operation};
use ::lopdf::{Document as Pdf, Object, ObjectId, Stream, dictionary};

use super::Bookmark;

/// A 1x1 opaque red PNG.
pub const PNG_DOT: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
    0x44, 0xAE, 0x42, 0x60, 0x82,
];

/// A document with one US Letter page per text, each showing its text in
/// Helvetica. Returns the document, its page ids and the font id.
fn base(texts: &[&str]) -> (Pdf, Vec<ObjectId>, ObjectId) {
    let mut pdf = Pdf::with_version("1.7");
    let font_id = pdf.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1",
        "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding",
    });
    let pages_id = pdf.new_object_id();
    let mut kids = Vec::new();
    for text in texts {
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 24.into()]),
                Operation::new("Td", vec![72.into(), 700.into()]),
                Operation::new("Tj", vec![Object::string_literal(*text)]),
                Operation::new("ET", vec![]),
            ],
        };
        let content_id = pdf.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let page_id = pdf.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Contents" => content_id,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
        });
        kids.push(page_id);
    }
    pdf.objects.insert(
        pages_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => kids.iter().map(|id| Object::Reference(*id)).collect::<Vec<_>>(),
            "Count" => kids.len() as i64,
        }
        .into(),
    );
    let catalog_id = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog_id);
    (pdf, kids, font_id)
}

/// A deadline far enough away for fixture building.
pub fn far() -> super::Deadline {
    super::Deadline::new(std::time::Duration::from_secs(60))
}

fn bytes(mut pdf: Pdf) -> Vec<u8> {
    let mut out = Vec::new();
    pdf.save_to(&mut out).expect("fixture saves");
    out
}

/// `count` pages reading `Page 1`, `Page 2`, ...
pub fn pages(count: u32) -> Vec<u8> {
    let texts: Vec<String> = (1..=count).map(|n| format!("Page {n}")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    bytes(base(&refs).0)
}

/// The outline of [`outlined`]: two chapters, the first with a section.
pub fn outline() -> Vec<Bookmark> {
    vec![
        Bookmark {
            title: "Chapter 1".into(),
            page: 1,
            children: vec![Bookmark {
                title: "Section 1.1".into(),
                page: 2,
                children: vec![],
            }],
        },
        Bookmark {
            title: "Chapter 2".into(),
            page: 3,
            children: vec![],
        },
    ]
}

/// Three pages with [`outline`].
pub fn outlined() -> Vec<u8> {
    let (mut pdf, _, _) = base(&["Chapter 1", "Section 1.1", "Chapter 2"]);
    super::lopdf::write_outline(&mut pdf, &outline(), far()).expect("outline writes");
    bytes(pdf)
}

/// One page with an AcroForm: a text field `name` holding `Ada` and a
/// checked checkbox `agree`, both with appearance streams.
pub fn form() -> Vec<u8> {
    let (mut pdf, pages, font_id) = base(&["Form"]);
    let page_id = pages[0];
    let text_ap = pdf.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), 228.into(), 30.into()],
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
        },
        b"/Tx BMC BT /F1 12 Tf 2 10 Td (Ada) Tj ET EMC".to_vec(),
    ));
    let text_field = pdf.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx",
        "T" => Object::string_literal("name"), "V" => Object::string_literal("Ada"),
        "Rect" => vec![72.into(), 600.into(), 300.into(), 630.into()],
        "F" => 4, "P" => page_id,
        "AP" => dictionary! { "N" => text_ap },
    });
    let on = pdf.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), 20.into(), 20.into()],
        },
        b"0 0 1 rg 2 2 16 16 re f".to_vec(),
    ));
    let off = pdf.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), 20.into(), 20.into()],
        },
        b"0 G 1 w 0.5 0.5 19 19 re S".to_vec(),
    ));
    let checkbox = pdf.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Btn",
        "T" => Object::string_literal("agree"), "V" => "Yes", "AS" => "Yes",
        "Rect" => vec![72.into(), 550.into(), 92.into(), 570.into()],
        "F" => 4, "P" => page_id,
        "AP" => dictionary! { "N" => dictionary! { "Yes" => on, "Off" => off } },
    });
    pdf.get_object_mut(page_id)
        .and_then(Object::as_dict_mut)
        .unwrap()
        .set(
            "Annots",
            vec![Object::Reference(text_field), Object::Reference(checkbox)],
        );
    pdf.catalog_mut().unwrap().set(
        "AcroForm",
        dictionary! {
            "Fields" => vec![Object::Reference(text_field), Object::Reference(checkbox)],
            "DA" => Object::string_literal("/F1 12 Tf 0 g"),
            "DR" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
        },
    );
    bytes(pdf)
}

/// One page with an Info dictionary and an XMP stream claiming PDF/A-2b,
/// the two in step.
pub fn pdfa_like() -> Vec<u8> {
    let (mut pdf, _, _) = base(&["PDF/A fixture"]);
    let info = pdf.add_object(dictionary! {
        "Title" => Object::string_literal("Fixture"),
        "Author" => Object::string_literal("rusticdoc"),
        "Producer" => Object::string_literal("lopdf"),
        "CreationDate" => Object::string_literal("D:20240102030405Z"),
        "ModDate" => Object::string_literal("D:20240102030405Z"),
    });
    pdf.trailer.set("Info", info);
    let xml = concat!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n",
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n",
        "<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n",
        "<rdf:Description rdf:about=\"\" xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\">",
        "<pdfaid:part>2</pdfaid:part><pdfaid:conformance>B</pdfaid:conformance></rdf:Description>\n",
        "<rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">",
        "<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Fixture</rdf:li></rdf:Alt></dc:title>",
        "<dc:creator><rdf:Seq><rdf:li>rusticdoc</rdf:li></rdf:Seq></dc:creator></rdf:Description>\n",
        "<rdf:Description rdf:about=\"\" xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\">",
        "<pdf:Producer>lopdf</pdf:Producer><xmp:CreateDate>2024-01-02T03:04:05Z</xmp:CreateDate>",
        "<xmp:ModifyDate>2024-01-02T03:04:05Z</xmp:ModifyDate></rdf:Description>\n",
        "</rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>",
    );
    super::lopdf::set_xmp_packet(&mut pdf, xml.to_string()).expect("xmp writes");
    bytes(pdf)
}

/// Writes every fixture into `dir`: `three-pages.pdf`, `outlined.pdf`,
/// `form.pdf`, `pdfa-like.pdf` and `dot.png`.
pub fn write_all(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("three-pages.pdf"), pages(3))?;
    std::fs::write(dir.join("outlined.pdf"), outlined())?;
    std::fs::write(dir.join("form.pdf"), form())?;
    std::fs::write(dir.join("pdfa-like.pdf"), pdfa_like())?;
    std::fs::write(dir.join("dot.png"), PNG_DOT)?;
    Ok(())
}
