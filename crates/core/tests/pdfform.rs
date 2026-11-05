//! PDF form listing, filling, annotation tags, flattening and overlay,
//! against fixtures built with lopdf in code.
//!
//! Set `RUSTICDOC_WRITE_FIXTURES=<dir>` to also write `form.pdf`,
//! `filled.pdf`, `flattened.pdf` and `overlaid.pdf` there for a look with a
//! viewer or `pdftotext`.

use std::collections::BTreeMap;

use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};
use rusticdoc_core::Error;
use rusticdoc_core::pdfform::{FillOptions, fields, fill, overlay};
use serde_json::json;

/// Knobs for the fixture, so one test can move the checkbox out from under
/// the annotation's data path.
struct Fixture {
    checkbox_name: &'static str,
}

impl Default for Fixture {
    fn default() -> Self {
        Fixture {
            checkbox_name: "paid",
        }
    }
}

fn page(doc: &mut Document, pages_id: ObjectId, content: &str, annots: Vec<ObjectId>) -> ObjectId {
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.as_bytes().to_vec()));
    doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        "Contents" => content_id,
        "Annots" => annots.into_iter().map(Object::Reference).collect::<Vec<_>>(),
    })
}

fn state_stream(doc: &mut Document, w: i64, h: i64, body: &str) -> ObjectId {
    doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), w.into(), h.into()],
        },
        body.as_bytes().to_vec(),
    ))
}

/// A two-page form: `customer.name`, `summary` (template default), `paid`
/// checkbox, `method` radio (card, cash), `currency` choice on page 2, a
/// FreeText tag over the checkbox and a free-standing Text tag.
fn build_form(fixture: &Fixture) -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });

    let customer = doc.new_object_id();
    let name = doc.add_object(dictionary! {
        "Type" => "Annot",
        "Subtype" => "Widget",
        "T" => Object::string_literal("name"),
        "Parent" => customer,
        "Rect" => vec![100.into(), 650.into(), 300.into(), 670.into()],
        "F" => 4,
        "DA" => Object::string_literal("/Helv 10 Tf 0 g"),
        "MK" => dictionary! { "BC" => vec![0.into()] },
    });
    doc.objects.insert(
        customer,
        dictionary! {
            "FT" => "Tx",
            "T" => Object::string_literal("customer"),
            "Kids" => vec![Object::Reference(name)],
        }
        .into(),
    );
    let total = doc.add_object(dictionary! {
        "Type" => "Annot",
        "Subtype" => "Widget",
        "FT" => "Tx",
        "T" => Object::string_literal("summary"),
        "DV" => Object::string_literal("Total: {{ total | round(2) }}"),
        "Rect" => vec![100.into(), 600.into(), 300.into(), 620.into()],
        "F" => 4,
        "Q" => 2,
        "DA" => Object::string_literal("/Helv 0 Tf 0 0 1 rg"),
    });
    let yes = state_stream(&mut doc, 20, 20, "0 g 4 4 12 12 re f");
    let off = state_stream(&mut doc, 20, 20, "0 G 0.5 0.5 19 19 re S");
    let paid = doc.add_object(dictionary! {
        "Type" => "Annot",
        "Subtype" => "Widget",
        "FT" => "Btn",
        "T" => Object::string_literal(fixture.checkbox_name),
        "V" => "Off",
        "AS" => "Off",
        "Rect" => vec![100.into(), 550.into(), 120.into(), 570.into()],
        "F" => 4,
        "AP" => dictionary! { "N" => dictionary! { "Yes" => yes, "Off" => off } },
    });
    let method = doc.new_object_id();
    let mut kids = Vec::new();
    for (i, state) in ["card", "cash"].iter().enumerate() {
        let on = state_stream(&mut doc, 20, 20, "0 g 10 10 m 10 10 5 0 360 arc f");
        let off = state_stream(&mut doc, 20, 20, "0 G 0.5 0.5 19 19 re S");
        let x = 100 + 40 * i as i64;
        kids.push(doc.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Widget",
            "Parent" => method,
            "AS" => "Off",
            "Rect" => vec![x.into(), 500.into(), (x + 20).into(), 520.into()],
            "F" => 4,
            "AP" => dictionary! { "N" => dictionary! { *state => on, "Off" => off } },
        }));
    }
    doc.objects.insert(
        method,
        dictionary! {
            "FT" => "Btn",
            "Ff" => 1i64 << 15,
            "T" => Object::string_literal("method"),
            "V" => "Off",
            "Kids" => kids.iter().map(|k| Object::Reference(*k)).collect::<Vec<_>>(),
        }
        .into(),
    );
    let currency = doc.add_object(dictionary! {
        "Type" => "Annot",
        "Subtype" => "Widget",
        "FT" => "Ch",
        "Ff" => 1i64 << 17,
        "T" => Object::string_literal("currency"),
        "Opt" => vec![Object::string_literal("EUR"), Object::string_literal("USD")],
        "Rect" => vec![100.into(), 650.into(), 200.into(), 670.into()],
        "F" => 4,
        "DA" => Object::string_literal("/Helv 10 Tf 0 g"),
    });
    let check_tag = doc.add_object(dictionary! {
        "Type" => "Annot",
        "Subtype" => "FreeText",
        "Rect" => vec![95.into(), 545.into(), 125.into(), 575.into()],
        "Contents" => Object::string_literal("{{ paid | check }}"),
        "DA" => Object::string_literal("/Helv 10 Tf 0 g"),
    });
    let name_tag = doc.add_object(dictionary! {
        "Type" => "Annot",
        "Subtype" => "Text",
        "Rect" => vec![100.into(), 400.into(), 300.into(), 420.into()],
        "Contents" => Object::string_literal("{{ customer.name }}"),
    });

    let page1 = page(
        &mut doc,
        pages_id,
        "BT /Helv 14 Tf 72 720 Td (Invoice) Tj ET",
        vec![name, total, paid, kids[0], kids[1], check_tag, name_tag],
    );
    let page2 = page(
        &mut doc,
        pages_id,
        "BT /Helv 14 Tf 72 720 Td (Terms) Tj ET",
        vec![currency],
    );
    for id in [name, total, paid, kids[0], kids[1], check_tag, name_tag] {
        doc.get_dictionary_mut(id).unwrap().set("P", page1);
    }
    doc.get_dictionary_mut(currency).unwrap().set("P", page2);
    doc.objects.insert(
        pages_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page1), Object::Reference(page2)],
            "Count" => 2,
            "Resources" => dictionary! { "Font" => dictionary! { "Helv" => font_id } },
        }
        .into(),
    );
    let acroform = doc.add_object(dictionary! {
        "Fields" => vec![customer, total, paid, method, currency]
            .into_iter()
            .map(Object::Reference)
            .collect::<Vec<_>>(),
        "DA" => Object::string_literal("/Helv 0 Tf 0 g"),
        "DR" => dictionary! { "Font" => dictionary! { "Helv" => font_id } },
    });
    let catalog = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
        "AcroForm" => acroform,
    });
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

/// A plain PDF with `n` pages, each showing its number and a grey band.
fn build_pages(n: usize, label: &str) -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let mut kids = Vec::new();
    for i in 1..=n {
        let content =
            format!("0.9 g 0 740 612 30 re f 0 g BT /F1 12 Tf 320 750 Td ({label} page {i}) Tj ET");
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
        kids.push(doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Contents" => content_id,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
        }));
    }
    doc.objects.insert(
        pages_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => kids.iter().map(|k| Object::Reference(*k)).collect::<Vec<_>>(),
            "Count" => n as i64,
        }
        .into(),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

fn data() -> serde_json::Value {
    json!({
        "customer": {"name": "Ada Lovelace"},
        "total": 12.345,
        "paid": true,
        "method": "cash",
        "currency": "USD",
        "ctx": {"request_id": "r1"}
    })
}

fn env() -> minijinja::Environment<'static> {
    minijinja::Environment::new()
}

/// Every terminal field dictionary by fully qualified name, read back
/// independently of the module under test.
fn field_dicts(doc: &Document) -> BTreeMap<String, ObjectId> {
    fn walk(doc: &Document, id: ObjectId, parent: &str, out: &mut BTreeMap<String, ObjectId>) {
        let dict = doc.get_dictionary(id).unwrap();
        let name = match dict.get(b"T").ok().and_then(|t| t.as_str().ok()) {
            Some(t) if parent.is_empty() => String::from_utf8_lossy(t).into_owned(),
            Some(t) => format!("{parent}.{}", String::from_utf8_lossy(t)),
            None => parent.to_string(),
        };
        let kids: Vec<ObjectId> = dict
            .get(b"Kids")
            .ok()
            .and_then(|k| k.as_array().ok())
            .map(|k| k.iter().filter_map(|k| k.as_reference().ok()).collect())
            .unwrap_or_default();
        let named_kids = kids
            .iter()
            .any(|k| doc.get_dictionary(*k).unwrap().has(b"T"));
        if named_kids {
            for kid in kids {
                walk(doc, kid, &name, out);
            }
        } else {
            out.insert(name, id);
        }
    }
    let mut out = BTreeMap::new();
    let catalog = doc.catalog().unwrap();
    let form = doc.get_dict_in_dict(catalog, b"AcroForm").unwrap();
    for root in form.get(b"Fields").unwrap().as_array().unwrap() {
        walk(doc, root.as_reference().unwrap(), "", &mut out);
    }
    out
}

fn text_of(doc: &Document, dict: &Dictionary, key: &[u8]) -> String {
    match dict.get(key).map(|o| doc.dereference(o).unwrap().1) {
        Ok(Object::String(bytes, _)) => {
            if bytes.starts_with(&[0xFE, 0xFF]) {
                let units: Vec<u16> = bytes[2..]
                    .chunks(2)
                    .map(|p| u16::from_be_bytes([p[0], p[1]]))
                    .collect();
                String::from_utf16(&units).unwrap()
            } else {
                String::from_utf8(bytes.clone()).unwrap()
            }
        }
        Ok(Object::Name(name)) => String::from_utf8(name.clone()).unwrap(),
        other => panic!("{} is not text: {other:?}", String::from_utf8_lossy(key)),
    }
}

fn appearance_content(doc: &Document, widget: ObjectId) -> String {
    let widget = doc.get_dictionary(widget).unwrap();
    let ap = doc.get_dict_in_dict(widget, b"AP").unwrap();
    let n = ap.get(b"N").unwrap().as_reference().unwrap();
    let stream = doc.get_object(n).unwrap().as_stream().unwrap();
    String::from_utf8(stream.decompressed_content().unwrap()).unwrap()
}

fn widgets_on(doc: &Document, page: ObjectId) -> Vec<Dictionary> {
    doc.get_page_annotations(page)
        .unwrap()
        .into_iter()
        .filter(|a| a.get(b"Subtype").unwrap().as_name().unwrap() == b"Widget")
        .cloned()
        .collect()
}

#[test]
fn lists_fields_with_kinds_pages_and_options() {
    let listed = fields(&build_form(&Fixture::default())).unwrap();
    let by_name: BTreeMap<_, _> = listed.iter().map(|f| (f.name.as_str(), f)).collect();
    assert_eq!(
        listed.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        ["customer.name", "summary", "paid", "method", "currency"]
    );
    assert_eq!(by_name["customer.name"].kind, "text");
    assert_eq!(by_name["customer.name"].page, 1);
    assert_eq!(by_name["summary"].kind, "text");
    assert_eq!(by_name["summary"].value, None);
    assert_eq!(by_name["paid"].kind, "checkbox");
    assert_eq!(by_name["paid"].value.as_deref(), Some("Off"));
    assert_eq!(by_name["method"].kind, "radio");
    assert_eq!(by_name["method"].page, 1);
    assert_eq!(by_name["currency"].kind, "choice");
    assert_eq!(by_name["currency"].page, 2);
    assert_eq!(by_name["currency"].options, ["EUR", "USD"]);
    assert!(fields(&build_pages(1, "plain")).unwrap().is_empty());
}

#[test]
fn fills_every_kind_of_field() {
    let filled = fill(
        &build_form(&Fixture::default()),
        &data(),
        &env(),
        &FillOptions::default(),
    )
    .unwrap();
    let doc = Document::load_mem(&filled).unwrap();
    let ids = field_dicts(&doc);
    let dict = |name: &str| doc.get_dictionary(ids[name]).unwrap();

    assert_eq!(text_of(&doc, dict("customer.name"), b"V"), "Ada Lovelace");
    let ap = appearance_content(&doc, ids["customer.name"]);
    assert!(ap.contains("/Tx BMC"), "{ap}");
    assert!(ap.contains("(Ada Lovelace) Tj"), "{ap}");
    assert!(ap.contains("/Helv 10 Tf"), "{ap}");

    assert_eq!(text_of(&doc, dict("summary"), b"V"), "Total: 12.35");
    let ap = appearance_content(&doc, ids["summary"]);
    assert!(ap.contains("(Total: 12.35) Tj"), "{ap}");
    assert!(ap.contains("0 0 1 rg"), "{ap}");

    assert_eq!(text_of(&doc, dict("paid"), b"V"), "Yes");
    assert_eq!(text_of(&doc, dict("paid"), b"AS"), "Yes");

    assert_eq!(text_of(&doc, dict("method"), b"V"), "cash");
    let kids = dict("method").get(b"Kids").unwrap().as_array().unwrap();
    let states: Vec<String> = kids
        .iter()
        .map(|k| {
            text_of(
                &doc,
                doc.get_dictionary(k.as_reference().unwrap()).unwrap(),
                b"AS",
            )
        })
        .collect();
    assert_eq!(states, ["Off", "cash"]);

    assert_eq!(text_of(&doc, dict("currency"), b"V"), "USD");
    assert!(appearance_content(&doc, ids["currency"]).contains("(USD) Tj"));

    let catalog = doc.catalog().unwrap();
    let form = doc.get_dict_in_dict(catalog, b"AcroForm").unwrap();
    assert!(form.get(b"NeedAppearances").unwrap().as_bool().unwrap());

    let listed = fields(&filled).unwrap();
    let summary = listed.iter().find(|f| f.name == "summary").unwrap();
    assert_eq!(summary.value.as_deref(), Some("Total: 12.35"));
}

#[test]
fn stores_non_latin_text_as_utf16() {
    let data = json!({"customer": {"name": "Ада Лавлейс"}, "total": 1});
    let filled = fill(
        &build_form(&Fixture::default()),
        &data,
        &env(),
        &FillOptions::default(),
    )
    .unwrap();
    let doc = Document::load_mem(&filled).unwrap();
    let ids = field_dicts(&doc);
    let name = doc.get_dictionary(ids["customer.name"]).unwrap();
    let raw = name.get(b"V").unwrap().as_str().unwrap();
    assert_eq!(&raw[..2], &[0xFE, 0xFF]);
    assert_eq!(text_of(&doc, name, b"V"), "Ада Лавлейс");
}

#[test]
fn annotation_over_a_widget_fills_the_widget() {
    // The checkbox is named `chk`, which is not in the data; only the
    // FreeText `{{ paid | check }}` over it can switch it on.
    let pdf = build_form(&Fixture {
        checkbox_name: "chk",
    });
    let filled = fill(&pdf, &data(), &env(), &FillOptions::default()).unwrap();
    let doc = Document::load_mem(&filled).unwrap();
    let ids = field_dicts(&doc);
    let chk = doc.get_dictionary(ids["chk"]).unwrap();
    assert_eq!(text_of(&doc, chk, b"V"), "Yes");
    assert_eq!(text_of(&doc, chk, b"AS"), "Yes");

    let unpaid = fill(
        &pdf,
        &json!({"paid": false, "customer": {"name": "x"}, "total": 1}),
        &env(),
        &FillOptions::default(),
    )
    .unwrap();
    let doc = Document::load_mem(&unpaid).unwrap();
    let ids = field_dicts(&doc);
    assert_eq!(
        text_of(&doc, doc.get_dictionary(ids["chk"]).unwrap(), b"AS"),
        "Off"
    );
}

#[test]
fn free_standing_annotation_draws_into_the_page() {
    let filled = fill(
        &build_form(&Fixture::default()),
        &data(),
        &env(),
        &FillOptions::default(),
    )
    .unwrap();
    let doc = Document::load_mem(&filled).unwrap();
    let pages = doc.get_pages();
    let content = String::from_utf8(doc.get_page_content(pages[&1])).unwrap();
    assert!(content.contains("(Ada Lovelace) Tj"), "{content}");
    assert!(content.contains("/RdHelv"), "{content}");
    assert!(content.contains("1 0 0 1 100 400 cm"), "{content}");
    let fonts = doc.get_page_fonts(pages[&1]).unwrap();
    assert!(fonts.contains_key(b"RdHelv".as_slice()));
    // Without stripping, the tag annotations stay.
    let subtypes: Vec<Vec<u8>> = doc
        .get_page_annotations(pages[&1])
        .unwrap()
        .iter()
        .map(|a| a.get(b"Subtype").unwrap().as_name().unwrap().to_vec())
        .collect();
    assert!(subtypes.contains(&b"Text".to_vec()));
    assert!(subtypes.contains(&b"FreeText".to_vec()));
}

#[test]
fn strip_tag_annotations_removes_them() {
    let filled = fill(
        &build_form(&Fixture::default()),
        &data(),
        &env(),
        &FillOptions {
            strip_tag_annotations: true,
            flatten: false,
        },
    )
    .unwrap();
    let doc = Document::load_mem(&filled).unwrap();
    let pages = doc.get_pages();
    let annots = doc.get_page_annotations(pages[&1]).unwrap();
    assert!(
        annots
            .iter()
            .all(|a| a.get(b"Subtype").unwrap().as_name().unwrap() == b"Widget")
    );
    assert_eq!(annots.len(), 5);
    let content = String::from_utf8(doc.get_page_content(pages[&1])).unwrap();
    assert!(content.contains("(Ada Lovelace) Tj"));
}

#[test]
fn flatten_fixes_values_into_page_content() {
    let flattened = fill(
        &build_form(&Fixture::default()),
        &data(),
        &env(),
        &FillOptions {
            flatten: true,
            strip_tag_annotations: true,
        },
    )
    .unwrap();
    let doc = Document::load_mem(&flattened).unwrap();
    assert!(doc.catalog().unwrap().get(b"AcroForm").is_err());
    let pages = doc.get_pages();
    for page in pages.values() {
        assert!(widgets_on(&doc, *page).is_empty());
        assert!(doc.get_page_annotations(*page).unwrap().is_empty());
    }
    let content = String::from_utf8(doc.get_page_content(pages[&1])).unwrap();
    assert!(content.contains("/RdFm1 Do"), "{content}");
    assert!(
        content.contains("1 0 0 1 100 650 cm /RdFm1 Do"),
        "{content}"
    );
    let xobjects: Vec<String> = ["RdFm1", "RdFm2", "RdFm3", "RdFm4", "RdFm5"]
        .iter()
        .map(|n| {
            let (resources, _) = doc.get_page_resources(pages[&1]).unwrap();
            let xobjects = doc
                .get_dict_in_dict(resources.unwrap(), b"XObject")
                .unwrap();
            let id = xobjects.get(n.as_bytes()).unwrap().as_reference().unwrap();
            let stream = doc.get_object(id).unwrap().as_stream().unwrap();
            String::from_utf8(stream.decompressed_content().unwrap()).unwrap()
        })
        .collect();
    assert!(xobjects[0].contains("(Ada Lovelace) Tj"));
    assert!(xobjects[1].contains("(Total: 12.35) Tj"));
    assert!(
        xobjects[2].contains("4 4 12 12 re f"),
        "checked box: {}",
        xobjects[2]
    );
    assert!(xobjects[3].contains("re S"), "card off: {}", xobjects[3]);
    assert!(xobjects[4].contains("arc f"), "cash on: {}", xobjects[4]);
    assert!(
        String::from_utf8(doc.get_page_content(pages[&2]))
            .unwrap()
            .contains("/RdFm6 Do")
    );
    assert!(fields(&flattened).unwrap().is_empty());
}

#[test]
fn choice_value_outside_options_is_rejected() {
    let mut data = data();
    data["currency"] = json!("GBP");
    let err = fill(
        &build_form(&Fixture::default()),
        &data,
        &env(),
        &FillOptions::default(),
    )
    .unwrap_err();
    assert!(
        matches!(err, Error::InvalidRequest(ref m) if m.contains("GBP") && m.contains("currency")),
        "{err}"
    );

    data["currency"] = json!("USD");
    data["method"] = json!("cheque");
    let err = fill(
        &build_form(&Fixture::default()),
        &data,
        &env(),
        &FillOptions::default(),
    )
    .unwrap_err();
    assert!(
        matches!(err, Error::InvalidRequest(ref m) if m.contains("cheque")),
        "{err}"
    );
}

#[test]
fn template_failures_are_template_errors() {
    let mut data = data();
    data["total"] = json!("not a number");
    let err = fill(
        &build_form(&Fixture::default()),
        &data,
        &env(),
        &FillOptions::default(),
    )
    .unwrap_err();
    assert!(
        matches!(err, Error::Template(ref m) if m.contains("summary")),
        "{err}"
    );
}

#[test]
fn rejects_what_is_not_a_pdf() {
    assert!(matches!(fields(b"hello"), Err(Error::InvalidRequest(_))));
    assert!(matches!(
        overlay(b"hello", &build_pages(1, "o")),
        Err(Error::InvalidRequest(_))
    ));
}

#[test]
fn overlay_one_page_onto_two() {
    let out = overlay(&build_pages(2, "base"), &build_pages(1, "over")).unwrap();
    let doc = Document::load_mem(&out).unwrap();
    let pages = doc.get_pages();
    assert_eq!(pages.len(), 2);
    let p1 = String::from_utf8(doc.get_page_content(pages[&1])).unwrap();
    assert!(p1.contains("(base page 1) Tj"), "{p1}");
    assert!(p1.contains("q /RdOv1 Do Q"), "{p1}");
    let (resources, _) = doc.get_page_resources(pages[&1]).unwrap();
    let xobjects = doc
        .get_dict_in_dict(resources.unwrap(), b"XObject")
        .unwrap();
    let id = xobjects.get(b"RdOv1").unwrap().as_reference().unwrap();
    let stream = doc.get_object(id).unwrap().as_stream().unwrap();
    assert_eq!(
        stream.dict.get(b"Subtype").unwrap().as_name().unwrap(),
        b"Form"
    );
    assert!(stream.dict.has(b"BBox") && stream.dict.has(b"Resources"));
    let body = String::from_utf8(stream.decompressed_content().unwrap()).unwrap();
    assert!(body.contains("(over page 1) Tj"), "{body}");
    let p2 = String::from_utf8(doc.get_page_content(pages[&2])).unwrap();
    assert!(p2.contains("(base page 2) Tj"));
    assert!(!p2.contains("Do"), "{p2}");
}

#[test]
fn overlay_three_pages_onto_two_appends_one() {
    let out = overlay(&build_pages(2, "base"), &build_pages(3, "over")).unwrap();
    let doc = Document::load_mem(&out).unwrap();
    let pages = doc.get_pages();
    assert_eq!(pages.len(), 3);
    for n in 1..=2 {
        let content = String::from_utf8(doc.get_page_content(pages[&n])).unwrap();
        assert!(content.contains(&format!("(base page {n}) Tj")));
        assert!(content.contains(&format!("/RdOv{n} Do")));
    }
    let p3 = String::from_utf8(doc.get_page_content(pages[&3])).unwrap();
    assert!(p3.contains("(over page 3) Tj"), "{p3}");
    let page3 = doc.get_dictionary(pages[&3]).unwrap();
    assert!(page3.has(b"MediaBox") && page3.has(b"Resources"));
    let root = doc
        .catalog()
        .unwrap()
        .get(b"Pages")
        .unwrap()
        .as_reference()
        .unwrap();
    assert_eq!(page3.get(b"Parent").unwrap().as_reference().unwrap(), root);
    assert_eq!(
        doc.get_dictionary(root)
            .unwrap()
            .get(b"Count")
            .unwrap()
            .as_i64()
            .unwrap(),
        3
    );
    // Every object id is unique and every reference resolves.
    let mut doc = doc;
    let dangling: Vec<ObjectId> = doc
        .traverse_objects(|_| {})
        .into_iter()
        .filter(|id| !doc.has_object(*id))
        .collect();
    assert!(dangling.is_empty(), "{dangling:?}");
}

#[test]
fn overlay_a_filled_form_onto_letterhead() {
    let filled = fill(
        &build_form(&Fixture::default()),
        &data(),
        &env(),
        &FillOptions {
            flatten: true,
            strip_tag_annotations: true,
        },
    )
    .unwrap();
    let out = overlay(&build_pages(2, "letterhead"), &filled).unwrap();
    let doc = Document::load_mem(&out).unwrap();
    assert_eq!(doc.get_pages().len(), 2);
}

#[test]
fn writes_fixtures_when_asked() {
    let Ok(dir) = std::env::var("RUSTICDOC_WRITE_FIXTURES") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let form = build_form(&Fixture::default());
    std::fs::write(dir.join("form.pdf"), &form).unwrap();
    let filled = fill(
        &form,
        &data(),
        &env(),
        &FillOptions {
            strip_tag_annotations: true,
            flatten: false,
        },
    )
    .unwrap();
    std::fs::write(dir.join("filled.pdf"), &filled).unwrap();
    let flattened = fill(
        &form,
        &data(),
        &env(),
        &FillOptions {
            strip_tag_annotations: true,
            flatten: true,
        },
    )
    .unwrap();
    std::fs::write(dir.join("flattened.pdf"), &flattened).unwrap();
    let overlaid = overlay(&build_pages(2, "base"), &build_pages(3, "over")).unwrap();
    std::fs::write(dir.join("overlaid.pdf"), &overlaid).unwrap();
}
