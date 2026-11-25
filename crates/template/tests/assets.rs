//! `asset(name)`: images a template names instead of carrying.

mod common;

use common::*;
use rusticdoc_template::assets::{copy_name, AssetDirs};
use rusticdoc_template::{
    compile, rich_environment, validate_with, Format, RenderOptions, Severity,
};
use serde_json::{json, Value};

struct Held {
    _dir: tempfile::TempDir,
    options: RenderOptions,
}

/// `brand/logo.png` mounted, `stored.png` uploaded and `page.css` mounted.
fn held() -> Held {
    let dir = tempfile::tempdir().unwrap();
    let (mounted, stored) = (dir.path().join("mounted"), dir.path().join("stored"));
    std::fs::create_dir_all(mounted.join("brand")).unwrap();
    std::fs::create_dir_all(&stored).unwrap();
    std::fs::write(mounted.join("brand/logo.png"), png(40, 20)).unwrap();
    std::fs::write(mounted.join("page.css"), "p {}").unwrap();
    std::fs::write(stored.join(copy_name("stored.png")), png(30, 30)).unwrap();
    Held {
        options: RenderOptions {
            assets: AssetDirs {
                mounted: Some(mounted),
                stored: Some(stored),
            },
            ..RenderOptions::default()
        },
        _dir: dir,
    }
}

fn render(format: Format, bytes: &[u8], data: Value) -> Result<Vec<u8>, String> {
    let held = held();
    compile(format, bytes)
        .expect("compiles")
        .render_with(&data, &held.options)
        .map_err(|e| e.message)
}

#[test]
fn every_office_format_takes_an_asset() {
    const TAG: &str = r#"{{ asset("brand/logo.png") | image(width=60) }}"#;
    let para = |tag: &str| format!("<text:p>{tag}</text:p>");
    for (format, bytes) in [
        (Format::Docx, docx(&p(TAG))),
        (
            Format::Xlsx,
            xlsx(&[row(1, &[cell("A1", "inlineStr", TAG)])], &[]),
        ),
        (Format::Pptx, pptx(&[&shape(&[TAG])])),
        (Format::Odt, odt(&para(TAG))),
    ] {
        let out = render(format, &bytes, json!({})).unwrap_or_else(|e| panic!("{format:?}: {e}"));
        let logo = png(40, 20);
        assert!(
            entries(&out).iter().any(|(_, data)| *data == logo),
            "{format:?}"
        );
    }
}

#[test]
fn a_name_can_be_worked_out_and_a_stored_asset_found() {
    let tag = r#"{{ asset(brand ~ ".png") | image }}"#;
    let out = render(Format::Docx, &docx(&p(tag)), json!({"brand": "stored"})).unwrap();
    assert_eq!(bytes_of(&out, "word/media/rich1.png"), Some(png(30, 30)));
}

#[test]
fn a_barcode_takes_an_asset_as_its_logo() {
    let tag = r#"{{ "https://example.com" | barcode("qr", logo=asset("brand/logo.png")) }}"#;
    let out = render(Format::Docx, &docx(&p(tag)), json!({})).unwrap();
    let svg = String::from_utf8(bytes_of(&out, "word/media/rich2.svg").unwrap()).unwrap();
    assert!(svg.contains("<image"), "{svg}");
}

#[test]
fn what_is_not_an_asset_is_an_error_that_says_so() {
    for (tag, why) in [
        (r#"{{ asset("gone.png") | image }}"#, "there is no asset"),
        (r#"{{ asset("../logo.png") | image }}"#, "not an asset name"),
        (r#"{{ asset("/etc/passwd") | image }}"#, "not an asset name"),
        (r#"{{ asset("page.css") | image }}"#, "only PNG, JPEG"),
        (r#"{{ asset(7) | image }}"#, "must be a string"),
        (r#"{{ asset(nothing) | image }}"#, "undefined"),
        // Printing the name would be a forgotten filter.
        (r#"{{ asset("brand/logo.png") }}"#, "image filter"),
    ] {
        let e = render(Format::Docx, &docx(&p(tag)), json!({})).unwrap_err();
        assert!(e.contains(why), "{tag}: {e}");
    }
    // A name in the data is not one: only `asset` reads files.
    let e = render(
        Format::Docx,
        &docx(&p("{{ logo | image }}")),
        json!({"logo": "brand/logo.png"}),
    );
    assert!(e.unwrap_err().contains("base64"));
    // With nowhere to look.
    let bytes = docx(&p(r#"{{ asset("brand/logo.png") | image }}"#));
    let e = compile(Format::Docx, &bytes).unwrap().render(&json!({}));
    assert!(e.unwrap_err().message.contains("there is no asset"));
    rich_environment(Format::Docx)
        .render_str(r#"{{ asset("a.png") }}"#, ())
        .unwrap_err();
}

#[test]
fn validation_finds_assets_and_reports_missing_ones() {
    let held = held();
    let errors = |tag: &str| -> Vec<String> {
        validate_with(Format::Docx, &docx(&p(tag)), &json!({}), &held.options)
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| d.message)
            .collect()
    };
    assert!(errors(r#"{{ asset("brand/logo.png") | image }}"#).is_empty());
    let found = errors(r#"{{ asset("gone.png") | image }}"#);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("gone.png"));
}
