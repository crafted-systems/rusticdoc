//! Unit tests of the queue and its helpers, then fills in every script
//! the service is asked for, checked through the PDF that comes out.

use super::*;

#[test]
fn only_text_beyond_winansi_is_shaped() {
    assert!(!needs_shaping("Café € “quoted” — ok?"));
    assert!(!needs_shaping("tab\tand\nnewline\u{1}"));
    assert!(needs_shaping("Привет"));
    assert!(needs_shaping("Invoice فاتورة"));
    assert!(needs_shaping("東京"));
}

#[test]
fn fonts_follow_the_field_and_the_language() {
    assert_eq!(FontChoice::from_names("Helv", None), FontChoice::SANS);
    let times = FontChoice::from_names("TiRo", Some("Times-Roman"));
    assert_eq!(times.family, Family::Serif);
    let mono = FontChoice::from_names("F1", Some("Courier-BoldOblique"));
    assert_eq!(
        mono,
        FontChoice {
            family: Family::Mono,
            bold: true,
            italic: true
        }
    );
    assert_eq!(
        FontChoice::from_names("F2", Some("NotoSans-Regular")).family,
        Family::Sans
    );
    assert_eq!(language("こんにちは世界"), Some("ja"));
    assert_eq!(language("안녕 世界"), Some("ko"));
    assert_eq!(language("你好"), Some("zh-CN"));
    assert_eq!(language("مرحبا"), None);
    assert!(font_stack(FontChoice::SANS, Some("ja")).contains("Noto Sans CJK JP"));
}

#[test]
fn values_are_cleaned_and_capped() {
    assert_eq!(clean("a\r\nb\rc\u{0}d\te", true), "a\nb\ncd e");
    assert_eq!(clean("a\r\nb\rc\u{0}d\te", false), "a b cd e");
    let long = "ж".repeat(MAX_VALUE_CHARS + 10);
    assert_eq!(clean(&long, true).chars().count(), MAX_VALUE_CHARS);
}

#[test]
fn colours_come_from_the_appearance_string() {
    assert_eq!(da_colour("0 g"), [0.0; 3]);
    assert_eq!(da_colour("0.5 g"), [0.5; 3]);
    assert_eq!(da_colour("0 0 1 rg"), [0.0, 0.0, 1.0]);
    assert_eq!(da_colour("0 0 0 1 k"), [0.0; 3]);
    assert_eq!(da_colour("1 0 0 0 k"), [0.0, 1.0, 1.0]);
    assert_eq!(da_colour("bad rg"), [0.0; 3]);
}

fn sample(text: &str) -> TextBox {
    TextBox {
        text: text.into(),
        width: 100.0,
        height: 20.0,
        size: 10.0,
        align: Align::Left,
        multiline: false,
        font: FontChoice::SANS,
        colour: [0.0; 3],
    }
}

#[test]
fn a_form_may_queue_so_much() {
    let mut pending = Pending::default();
    for i in 0..MAX_BOXES {
        pending
            .page(
                (i as u32 + 1, 0),
                Rect {
                    x1: 0.0,
                    y1: 0.0,
                    x2: 1.0,
                    y2: 1.0,
                },
                sample("ж"),
            )
            .unwrap();
    }
    let err = pending
        .page(
            (1, 0),
            Rect {
                x1: 0.0,
                y1: 0.0,
                x2: 1.0,
                y2: 1.0,
            },
            sample("ж"),
        )
        .unwrap_err();
    assert!(err.to_string().contains("boxes"), "{err}");

    let mut pending = Pending::default();
    let long = "ж".repeat(MAX_VALUE_CHARS);
    let err = (0..)
        .map(|i| pending.widget((i + 1, 0), String::new(), sample(&long)))
        .find_map(Result::err)
        .unwrap();
    assert!(err.to_string().contains("characters"), "{err}");

    // Replacing a widget's value gives its characters back.
    let mut pending = Pending::default();
    for _ in 0..1_000 {
        pending
            .widget((1, 0), String::new(), sample(&long))
            .unwrap();
    }
    assert_eq!(pending.chars, MAX_VALUE_CHARS);
    pending.forget((1, 0));
    assert!(pending.is_empty());
    assert_eq!(pending.chars, 0);
}

// Fills, checked through the PDF that comes out.

use sha2::Digest as _;

use super::super::{FillLimits, FillOptions, fill_within};

/// A one-page form: `single`, an auto-size centred line; `multi`, a
/// multiline field at 11 points in blue; and a FreeText tag `{{ note }}`
/// with no widget under it, whose value is drawn on the page.
fn form() -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let helv = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1",
        "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding",
    });
    let rect = |x1: i64, y1: i64, x2: i64, y2: i64| -> Object {
        vec![x1.into(), y1.into(), x2.into(), y2.into()].into()
    };
    let single = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx",
        "T" => Object::string_literal("single"),
        "Rect" => rect(72, 700, 372, 724), "F" => 4, "Q" => 1,
        "DA" => Object::string_literal("/Helv 0 Tf 0 g"),
        "MK" => dictionary! { "BC" => vec![0.into()] },
    });
    let multi = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx",
        "T" => Object::string_literal("multi"), "Ff" => 4096,
        "Rect" => rect(72, 560, 372, 680), "F" => 4,
        "DA" => Object::string_literal("/Helv 11 Tf 0 0 1 rg"),
        "MK" => dictionary! { "BC" => vec![0.into()] },
    });
    let note = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "FreeText",
        "Rect" => rect(72, 400, 372, 440),
        "Contents" => Object::string_literal("{{ note }}"),
        "DA" => Object::string_literal("/Helv 12 Tf 0 g"),
    });
    let content = doc.add_object(Stream::new(
        dictionary! {},
        b"BT /Helv 10 Tf 72 740 Td (Form) Tj ET".to_vec(),
    ));
    let page = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id,
        "MediaBox" => rect(0, 0, 612, 792), "Contents" => content,
        "Resources" => dictionary! { "Font" => dictionary! { "Helv" => helv } },
        "Annots" => vec![single.into(), multi.into(), note.into()],
    });
    doc.objects.insert(
        pages_id,
        dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }.into(),
    );
    let catalog = doc.add_object(dictionary! {
        "Type" => "Catalog", "Pages" => pages_id,
        "AcroForm" => dictionary! {
            "Fields" => vec![single.into(), multi.into()],
            "DR" => dictionary! { "Font" => dictionary! { "Helv" => helv } },
            "DA" => Object::string_literal("/Helv 0 Tf 0 g"),
        },
    });
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

fn data(value: &str) -> serde_json::Value {
    serde_json::json!({ "single": value, "multi": format!("{value}\n{value}"), "note": value })
}

fn fill(pdf: &[u8], data: &serde_json::Value, flatten: bool) -> Vec<u8> {
    fill_within(
        pdf,
        data,
        &crate::template::environment(),
        &FillOptions {
            flatten,
            strip_tag_annotations: true,
        },
        FillLimits::default(),
    )
    .unwrap()
}

/// With `RUSTICDOC_WRITE_FIXTURES=<dir>`, keeps a file there for a look.
fn keep(name: &str, bytes: &[u8]) {
    if let Some(dir) = std::env::var_os("RUSTICDOC_WRITE_FIXTURES") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(std::path::Path::new(&dir).join(name), bytes).unwrap();
    }
}

/// An embedded font: its name, the bytes of its font program as stored,
/// and every string its ToUnicode map gives.
struct Embedded {
    name: String,
    program: usize,
    unicode: Vec<String>,
}

fn embedded(pdf: &[u8]) -> Vec<Embedded> {
    let doc = Document::load_mem(pdf).unwrap();
    let dict = |object: &Object| -> Option<Dictionary> {
        match object {
            Object::Reference(id) => doc.get_dictionary(*id).ok().cloned(),
            Object::Dictionary(d) => Some(d.clone()),
            _ => None,
        }
    };
    let stream = |object: &Object| -> Option<Stream> {
        doc.get_object(object.as_reference().ok()?)
            .ok()?
            .as_stream()
            .ok()
            .cloned()
    };
    let mut out = Vec::new();
    for object in doc.objects.values() {
        let Ok(font) = object.as_dict() else {
            continue;
        };
        if font.get(b"Subtype").and_then(Object::as_name).ok() != Some(b"Type0".as_slice()) {
            continue;
        }
        let name =
            String::from_utf8_lossy(font.get(b"BaseFont").unwrap().as_name().unwrap()).into_owned();
        let descendant = font.get(b"DescendantFonts").unwrap().as_array().unwrap()[0].clone();
        let descriptor = dict(dict(&descendant).unwrap().get(b"FontDescriptor").unwrap()).unwrap();
        let program = [b"FontFile2".as_slice(), b"FontFile3", b"FontFile"]
            .iter()
            .find_map(|key| stream(descriptor.get(key).ok()?))
            .map_or(0, |s| s.content.len());
        let cmap = stream(font.get(b"ToUnicode").unwrap()).unwrap();
        let cmap = String::from_utf8_lossy(&cmap.decompressed_content().unwrap()).into_owned();
        out.push(Embedded {
            name,
            program,
            unicode: to_unicode_targets(&cmap),
        });
    }
    out
}

/// The strings a ToUnicode CMap maps glyphs to, from its `bfchar` and
/// `bfrange` sections.
fn to_unicode_targets(cmap: &str) -> Vec<String> {
    let decode = |hex: &str| -> String {
        let units: Vec<u16> = (0..hex.len() / 4)
            .map(|i| u16::from_str_radix(&hex[i * 4..i * 4 + 4], 16).unwrap())
            .collect();
        String::from_utf16_lossy(&units)
    };
    let mut out = Vec::new();
    let mut section = "";
    for line in cmap.lines() {
        let line = line.trim();
        if line.ends_with("beginbfchar") {
            section = "char";
            continue;
        }
        if line.ends_with("beginbfrange") {
            section = "range";
            continue;
        }
        if line.starts_with("end") {
            section = "";
            continue;
        }
        let hexes: Vec<&str> = line
            .split(['<', '>', '[', ']', ' '])
            .filter(|t| !t.is_empty())
            .collect();
        match (section, hexes.as_slice()) {
            ("char", [_, target]) => out.push(decode(target)),
            ("range", [lo, hi, first]) if !line.contains('[') => {
                let count =
                    u32::from_str_radix(hi, 16).unwrap() - u32::from_str_radix(lo, 16).unwrap();
                let start = decode(first);
                let base = start.chars().last().unwrap() as u32;
                let prefix: String = start.chars().take(start.chars().count() - 1).collect();
                for i in 0..=count {
                    out.push(format!("{prefix}{}", char::from_u32(base + i).unwrap()));
                }
            }
            ("range", [_, _, targets @ ..]) => out.extend(targets.iter().map(|t| decode(t))),
            _ => {}
        }
    }
    out
}

/// True when a face on this host has every glyph of `value`.
fn drawable(value: &str) -> bool {
    let mut shaper = Shaper::new();
    let mut layout = shaper.shape(value, FontChoice::SANS, 12.0);
    layout.break_all_lines(None);
    layout.lines().all(|line| {
        line.items().all(|item| match item {
            parley::PositionedLayoutItem::GlyphRun(run) => run.glyphs().all(|g| g.id != 0),
            _ => true,
        })
    })
}

/// `pdftotext` from Poppler, when it is installed.
fn pdftotext() -> Option<std::path::PathBuf> {
    let mut dirs: Vec<std::path::PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.push("/opt/homebrew/bin".into());
    dirs.into_iter()
        .map(|d| d.join("pdftotext"))
        .find(|p| p.is_file())
}

/// The text Poppler extracts, without the bidi controls it adds around
/// right-to-left runs and with runs of white space as one space.
fn extract(pdf: &[u8]) -> Option<String> {
    let bin = pdftotext()?;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("in.pdf");
    std::fs::write(&path, pdf).unwrap();
    let out = std::process::Command::new(bin)
        .args(["-enc", "UTF-8"])
        .arg(&path)
        .arg("-")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(normalise(&String::from_utf8(out.stdout).unwrap()))
}

fn normalise(text: &str) -> String {
    text.chars()
        .filter(|c| !matches!(*c as u32, 0x200E | 0x200F | 0x202A..=0x202E | 0x2066..=0x2069))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Fills the form with `value` everywhere and checks what came out: each
/// face is embedded once and subset, the ToUnicode maps cover the value, the
/// form no longer asks viewers to redraw it, and Poppler extracts the value
/// from the editable form and from the flattened one.
fn check_script(script: &str, value: &str) {
    if !drawable(value) {
        eprintln!("skipping {script}: no font on this host has every glyph of {value:?}");
        return;
    }
    let form = form();
    let filled = fill(&form, &data(value), false);
    let flat = fill(&form, &data(value), true);
    keep(&format!("{script}.pdf"), &filled);
    keep(&format!("{script}-flat.pdf"), &flat);

    let doc = Document::load_mem(&filled).unwrap();
    let acroform = super::super::acroform(&doc).unwrap();
    assert!(
        acroform.get(b"NeedAppearances").is_err(),
        "{script}: viewers must keep the drawn appearances"
    );

    let fonts = embedded(&filled);
    assert!(!fonts.is_empty(), "{script}: no font embedded");
    let mut names: Vec<&str> = fonts.iter().map(|f| f.name.as_str()).collect();
    names.sort();
    names.dedup();
    assert_eq!(
        names.len(),
        fonts.len(),
        "{script}: a face embedded twice: {names:?}"
    );
    for font in &fonts {
        assert!(
            font.name.len() > 7 && font.name.as_bytes()[6] == b'+',
            "{script}: {} is not a subset",
            font.name
        );
        assert!(
            font.program > 0 && font.program < 64 * 1024,
            "{script}: {} carries {} bytes of font",
            font.name,
            font.program
        );
    }
    let mapped: HashSet<char> = fonts
        .iter()
        .flat_map(|f| f.unicode.iter())
        .flat_map(|s| s.chars())
        .collect();
    for c in value.chars().filter(|c| !c.is_whitespace()) {
        assert!(mapped.contains(&c), "{script}: ToUnicode lacks {c:?}");
    }
    assert!(
        filled.len() < form.len() + 128 * 1024,
        "{script}: {} bytes",
        filled.len()
    );
    eprintln!(
        "{script}: form {} bytes, filled {} bytes, flattened {} bytes, fonts {:?}",
        form.len(),
        filled.len(),
        flat.len(),
        fonts
            .iter()
            .map(|f| (&f.name, f.program))
            .collect::<Vec<_>>()
    );

    match (extract(&filled), extract(&flat)) {
        (Some(filled), Some(flat)) => {
            let want = normalise(value);
            for text in [&filled, &flat] {
                // The single line, both lines of the multiline field, and
                // the note.
                assert_eq!(
                    text.matches(&want).count(),
                    4,
                    "{script}: Poppler extracts {text:?}, not {want:?} four times"
                );
            }
        }
        _ => eprintln!("{script}: pdftotext is not installed, so only ToUnicode was checked"),
    }
}

#[test]
fn arabic_joins_and_runs_right_to_left() {
    check_script("arabic", "مرحبا بالعالم");
}

#[test]
fn hebrew_runs_right_to_left() {
    check_script("hebrew", "שלום עולם");
}

#[test]
fn chinese_draws() {
    check_script("chinese", "你好，世界");
}

#[test]
fn japanese_draws() {
    check_script("japanese", "こんにちは世界");
}

#[test]
fn korean_draws() {
    check_script("korean", "안녕하세요 세계");
}

#[test]
fn cyrillic_draws() {
    check_script("cyrillic", "Привет, мир");
}

#[test]
fn latin_and_arabic_mix() {
    check_script("mixed", "Invoice فاتورة");
}

/// The lam-alef glyph maps to lam then alef. Poppler swaps the two when it
/// turns a right-to-left line around, so this checks the map itself.
#[test]
fn a_ligature_copies_as_every_letter_it_stands_for() {
    let value = "لا سلام";
    if !drawable(value) {
        eprintln!("skipping: no Arabic font on this host");
        return;
    }
    let filled = fill(&form(), &data(value), false);
    let targets: Vec<String> = embedded(&filled)
        .into_iter()
        .flat_map(|f| f.unicode)
        .collect();
    assert!(
        targets.iter().any(|t| t == "لا"),
        "the lam-alef glyph should map to both letters: {targets:?}"
    );
}

/// SHA-256 of the form filled with Latin text, editable and flattened, as
/// the Helvetica appearances wrote it before text outside Latin was shaped.
const LATIN_DIGESTS: [&str; 2] = [
    "3931fc6dfb39c98d8deead2373771e5bd17a120e972009e3d35f9caa90e55320",
    "d0638882a398a93cb7a77e3c8419d7d515bea4fc17c5c3a4826f8d08b0e1fbe4",
];

#[test]
fn latin_text_keeps_its_helvetica_appearances() {
    let form = form();
    let latin = data("Amani Otieno, Café “Œuvre” € 12");
    keep("latin-form.pdf", &form);
    keep("latin-data.json", latin.to_string().as_bytes());
    for (flatten, digest) in [false, true].into_iter().zip(LATIN_DIGESTS) {
        let out = fill(&form, &latin, flatten);
        keep(&format!("latin-{flatten}.pdf"), &out);
        assert!(embedded(&out).is_empty());
        assert_eq!(
            hex::encode(sha2::Sha256::digest(&out)),
            digest,
            "flatten: {flatten}"
        );
    }
    let doc = Document::load_mem(&fill(&form, &latin, false)).unwrap();
    let acroform = super::super::acroform(&doc).unwrap();
    assert_eq!(
        acroform.get(b"NeedAppearances").unwrap(),
        &Object::Boolean(true)
    );
}

#[test]
fn many_boxes_share_one_subset_of_each_face() {
    let value = "مرحبا 你好";
    if !drawable(value) {
        eprintln!("skipping: no Arabic or CJK font on this host");
        return;
    }
    let mut doc = Document::load_mem(&form()).unwrap();
    let page = *doc.get_pages().get(&1).unwrap();
    let mut pending = Pending::default();
    for i in 0..300 {
        let y = (i % 70) as f32 * 11.0;
        pending
            .page(
                page,
                Rect {
                    x1: 10.0,
                    y1: y,
                    x2: 200.0,
                    y2: y + 10.0,
                },
                sample(&format!("{value} {i}")),
            )
            .unwrap();
    }
    let started = std::time::Instant::now();
    pending
        .draw(&mut doc, Deadline::new(std::time::Duration::from_secs(30)))
        .unwrap();
    let elapsed = started.elapsed();
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    let fonts = embedded(&out);
    let mut names: Vec<&str> = fonts.iter().map(|f| f.name.as_str()).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), fonts.len(), "{names:?}");
    assert!(fonts.len() <= 4, "{names:?}");
    eprintln!(
        "300 boxes in {elapsed:?}, {} bytes, fonts {names:?}",
        out.len()
    );
}
