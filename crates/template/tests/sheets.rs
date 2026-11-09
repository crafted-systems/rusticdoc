//! Whole sheets and nested row blocks in XLSX and ODS: a sheet repeated
//! once per item and named from the data, row loops inside row loops, and
//! formulas that follow the rows they point at.

mod common;

use common::*;
use regex::Regex;
use rusticdoc_template::{compile, validate, Format, Severity};
use serde_json::{json, Value};

/// `tests/fixtures/sheets.fods` saved by LibreOffice (see `gen.sh`).
const SHEETS_XLSX: &[u8] = include_bytes!("fixtures/sheets.xlsx");
const SHEETS_ODS: &[u8] = include_bytes!("fixtures/sheets.ods");

fn sites() -> Value {
    json!({"sites": [
        {"name": "Paris: Nord/Sud", "city": "Paris", "groups": [
            {"name": "Coffee", "items": [
                {"name": "Beans", "qty": 2, "price": 10},
                {"name": "Filters", "qty": 3, "price": 1}
            ]},
            {"name": "Tea", "items": [{"name": "Green", "qty": 1, "price": 5}]}
        ]},
        {"name": "Lyon", "city": "Lyon", "groups": [
            {"name": "Empty", "items": []},
            {"name": "Cups", "items": [{"name": "Mug", "qty": 4, "price": 2.5}]}
        ]},
        {"name": "summary", "city": "Clash", "groups": [
            {"name": "Misc", "items": [{"name": "Pen", "qty": 1, "price": 1}]}
        ]}
    ]})
}

/// `(name, part)` of the sheets in an XLSX, in order.
fn xlsx_sheets(bytes: &[u8]) -> Vec<(String, String)> {
    let wb = part(bytes, "xl/workbook.xml");
    let rels = part(bytes, "xl/_rels/workbook.xml.rels");
    let sheet = Regex::new(r#"<sheet name="([^"]*)" sheetId="(\d+)"[^>]*r:id="(rId\d+)""#).unwrap();
    sheet
        .captures_iter(&wb)
        .map(|c| {
            let rel = Regex::new(&format!(r#"Id="{}"[^>]*Target="([^"]*)""#, &c[3])).unwrap();
            let target = rel
                .captures(&rels)
                .map(|r| format!("xl/{}", &r[1]))
                .unwrap_or_default();
            (unescape(&c[1]), target)
        })
        .collect()
}

/// Every cell of a sheet part with shared strings resolved.
fn sheet_cells(bytes: &[u8], sheet: &str) -> Vec<(String, String)> {
    let sst = part(bytes, "xl/sharedStrings.xml");
    let si = Regex::new("(?s)<si>(.*?)</si>").unwrap();
    let t = Regex::new("(?s)<t[^>]*>(.*?)</t>").unwrap();
    let shared: Vec<String> = si
        .captures_iter(&sst)
        .map(|c| t.captures_iter(&c[1]).map(|m| unescape(&m[1])).collect())
        .collect();
    let xml = part(bytes, sheet);
    let sre = Regex::new(r#"<c r="([A-Z]+[0-9]+)"[^>]*t="s"[^>]*><v>([0-9]+)</v>"#).unwrap();
    let mut out = cells(&xml);
    for c in sre.captures_iter(&xml) {
        if let Some(cell) = out.iter_mut().find(|(r, _)| r == &c[1]) {
            cell.1 = shared[c[2].parse::<usize>().unwrap()].clone();
        }
    }
    out
}

/// Writes an output where `RUSTICDOC_KEEP` points, to open it by hand.
fn keep(file: &str, bytes: &[u8]) {
    if let Some(dir) = std::env::var_os("RUSTICDOC_KEEP") {
        std::fs::write(std::path::Path::new(&dir).join(file), bytes).unwrap();
    }
}

fn get<'a>(cells: &'a [(String, String)], r: &str) -> &'a str {
    cells
        .iter()
        .find(|(k, _)| k == r)
        .map(|(_, v)| v.as_str())
        .unwrap_or("<none>")
}

#[test]
fn xlsx_sheet_loop_repeats_names_and_rebases_each_copy() {
    let compiled = compile(Format::Xlsx, SHEETS_XLSX).unwrap();
    // The currency comes from a custom document property.
    assert_eq!(compiled.settings().currency.as_deref(), Some("EUR"));
    // The listing holds what the author typed, including the sheet name's
    // tag, and not the loop's made-up end.
    let tags = compiled.tags();
    assert!(
        tags.contains(&"{% sheet for s in sites %}".to_string()),
        "{tags:?}"
    );
    assert!(tags.contains(&"{{ s.name }}".to_string()), "{tags:?}");
    assert_eq!(
        tags.iter().filter(|t| *t == "{% endfor %}").count(),
        2,
        "{tags:?}"
    );
    let out = compiled.render(&sites()).unwrap();
    let sheets = xlsx_sheets(&out);
    let names: Vec<&str> = sheets.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["Summary", "Paris_ Nord_Sud", "Lyon", "summary (2)"]);
    let parts: Vec<&str> = sheets.iter().map(|(_, p)| p.as_str()).collect();
    assert_eq!(
        parts,
        [
            "xl/worksheets/sheet1.xml",
            "xl/worksheets/sheet2.xml",
            "xl/worksheets/sheet3.xml",
            "xl/worksheets/sheet4.xml"
        ]
    );
    let wb = part(&out, "xl/workbook.xml");
    for id in ["1", "2", "3", "4"] {
        assert_eq!(wb.matches(&format!("sheetId=\"{id}\"")).count(), 1, "{wb}");
    }
    let types = part(&out, "[Content_Types].xml");
    for p in &parts {
        assert!(types.contains(&format!("PartName=\"/{p}\"")), "{types}");
    }

    let paris = sheet_cells(&out, "xl/worksheets/sheet2.xml");
    assert_eq!(get(&paris, "A1"), "Site Paris: Nord/Sud");
    assert_eq!(get(&paris, "A3"), "Group Coffee");
    assert_eq!(get(&paris, "A4"), "Beans");
    assert_eq!(get(&paris, "B4"), "2");
    assert_eq!(get(&paris, "D4"), "=B4*C4");
    assert_eq!(get(&paris, "D5"), "=B5*C5");
    assert_eq!(get(&paris, "A6"), "Subtotal Coffee");
    assert_eq!(get(&paris, "D6"), "=SUBTOTAL(9,D3:D5)");
    assert_eq!(get(&paris, "A7"), "Group Tea");
    assert_eq!(get(&paris, "D8"), "=B8*C8");
    assert_eq!(get(&paris, "D9"), "=SUBTOTAL(9,D7:D8)");
    assert_eq!(get(&paris, "A10"), "Total EUR 0.00");
    assert_eq!(get(&paris, "D10"), "=SUBTOTAL(9,D3:D9)");

    let lyon = sheet_cells(&out, "xl/worksheets/sheet3.xml");
    assert_eq!(get(&lyon, "A3"), "Group Empty");
    assert_eq!(get(&lyon, "A4"), "Subtotal Empty");
    assert_eq!(get(&lyon, "D4"), "=SUBTOTAL(9,D3:D3)");
    assert_eq!(get(&lyon, "A6"), "Mug");
    assert_eq!(get(&lyon, "D7"), "=SUBTOTAL(9,D5:D6)");
    assert_eq!(get(&lyon, "D8"), "=SUBTOTAL(9,D3:D7)");

    // Each copy has its own print area, named after it and grown over its
    // rows.
    for (i, name, last) in [
        (1, "Paris_ Nord_Sud", 10),
        (2, "Lyon", 8),
        (3, "summary (2)", 6),
    ] {
        let want = format!("localSheetId=\"{i}\" name=\"_xlnm.Print_Area\" vbProcedure=\"false\">'{name}'!$A$1:$E${last}<");
        assert!(wb.contains(&want), "missing {want} in {wb}");
    }
    // Another sheet's reference to the template sheet follows the first copy.
    let summary = sheet_cells(&out, "xl/worksheets/sheet1.xml");
    assert_eq!(get(&summary, "B1"), "3");
    assert_eq!(get(&summary, "B2"), "='Paris_ Nord_Sud'!D6");
    for p in parts
        .iter()
        .copied()
        .chain(["xl/workbook.xml", "docProps/app.xml"])
    {
        let xml = part(&out, p);
        assert!(!xml.contains("{{") && !xml.contains('\u{3}'), "{p}: {xml}");
    }
}

#[test]
fn ods_sheet_loop_repeats_names_and_rebases_each_copy() {
    let compiled = compile(Format::Ods, SHEETS_ODS).unwrap();
    assert_eq!(compiled.settings().currency.as_deref(), Some("EUR"));
    let out = compiled.render(&sites()).unwrap();
    let xml = part(&out, "content.xml");
    let names: Vec<String> = Regex::new(r#"<table:table table:name="([^"]*)""#)
        .unwrap()
        .captures_iter(&xml)
        .map(|c| unescape(&c[1]))
        .collect();
    assert_eq!(names, ["Summary", "Paris_ Nord_Sud", "Lyon", "summary (2)"]);
    assert!(!xml.contains("rusticdoc-"), "marks are left: {xml}");
    assert!(!xml.contains('\u{3}') && !xml.contains("{{"), "{xml}");
    let formulas: Vec<String> = Regex::new(r#"table:formula="([^"]*)""#)
        .unwrap()
        .captures_iter(&xml)
        .map(|c| unescape(&c[1]).replace("&apos;", "'"))
        .collect();
    assert_eq!(
        formulas,
        [
            "of:=[$'Paris_ Nord_Sud'.D6]",
            // Paris
            "of:=[.B4]*[.C4]",
            "of:=[.B5]*[.C5]",
            "of:=SUBTOTAL(9;[.D3:.D5])",
            "of:=[.B8]*[.C8]",
            "of:=SUBTOTAL(9;[.D7:.D8])",
            "of:=SUBTOTAL(9;[.D3:.D9])",
            // Lyon
            "of:=SUBTOTAL(9;[.D3:.D3])",
            "of:=[.B6]*[.C6]",
            "of:=SUBTOTAL(9;[.D5:.D6])",
            "of:=SUBTOTAL(9;[.D3:.D7])",
            // summary (2)
            "of:=[.B4]*[.C4]",
            "of:=SUBTOTAL(9;[.D3:.D4])",
            "of:=SUBTOTAL(9;[.D3:.D5])",
        ]
    );
    // Each copy prints itself.
    let ranges: Vec<String> = Regex::new(r#"table:print-ranges="([^"]*)""#)
        .unwrap()
        .captures_iter(&xml)
        .map(|c| unescape(&c[1]).replace("&apos;", "'"))
        .collect();
    assert_eq!(
        ranges,
        [
            "'Paris_ Nord_Sud'.A1:'Paris_ Nord_Sud'.E10",
            "'Lyon'.A1:'Lyon'.E8",
            "'summary (2)'.A1:'summary (2)'.E6"
        ]
    );
    let text = odf_paragraphs(&xml);
    for want in [
        "Site Paris: Nord/Sud",
        "Subtotal Coffee",
        "Total EUR 0.00",
        "Mug",
    ] {
        assert!(text.iter().any(|t| t == want), "missing {want} in {text:?}");
    }
}

/// Converts both outputs with LibreOffice and reads the PDFs: the sheets
/// come out in order with their values, which proves the files open and the
/// re-based formulas compute. Skips when the images are not present.
#[test]
fn libreoffice_opens_repeated_sheets() {
    if !libreoffice_available() {
        eprintln!("skipping: docker images not available");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let dir_s = dir.path().to_str().unwrap();
    for (format, bytes, file) in [
        (Format::Xlsx, SHEETS_XLSX, "sites.xlsx"),
        (Format::Ods, SHEETS_ODS, "sites.ods"),
    ] {
        let out = compile(format, bytes).unwrap().render(&sites()).unwrap();
        std::fs::write(dir.path().join(file), &out).unwrap();
        keep(file, &out);
        assert!(soffice(dir_s, "pdf", file), "{file}: conversion failed");
        let pdf = file.replace(".xlsx", ".pdf").replace(".ods", ".pdf");
        let text = pdf_text(dir_s, &pdf);
        let squeezed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let order = [
            // The column is too narrow for the whole title.
            "Site Paris",
            "Subtotal Coffee 23",
            "Subtotal Tea 5",
            "Total EUR 0.00 28",
            "Site Lyon",
            "Subtotal Empty",
            "Subtotal Cups 10",
            "Total EUR 0.00 10",
            "Site summary",
            "Total EUR 0.00 1",
        ];
        let mut at = 0;
        for want in order {
            match squeezed[at..].find(want) {
                Some(p) => at += p + want.len(),
                None => panic!("{file}: missing {want:?} after {at} in {squeezed}"),
            }
        }
        assert!(
            !squeezed.contains("#VALUE") && !squeezed.contains("Err:"),
            "{file}: {squeezed}"
        );
    }
}

// --- A workbook laid out the way Excel writes one ---------------------------

const MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PKG_REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const CT: &str = "application/vnd.openxmlformats-officedocument";
const DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n";

/// One sheet of a [`book`]: its name, its rows, what follows `sheetData`,
/// and its relationships as `(id, type, target)`.
struct Sheet<'a> {
    name: &'a str,
    rows: String,
    tail: &'a str,
    rels: Vec<(&'a str, &'a str, &'a str)>,
}

fn sheet<'a>(name: &'a str, rows: &[String]) -> Sheet<'a> {
    Sheet {
        name,
        rows: rows.join(""),
        tail: "",
        rels: Vec::new(),
    }
}

/// An XLSX with Excel's parts: a calculation chain, a print area per sheet,
/// `docProps/app.xml` listing the sheets, the last sheet active and selected,
/// code names, and custom properties. `extra` adds parts as `(name, content
/// type, data)`.
fn book(sheets: &[Sheet], extra: &[(&str, &str, &[u8])]) -> Vec<u8> {
    let n = sheets.len();
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let mut types = format!(
        "{DECL}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Default Extension=\"png\" ContentType=\"image/png\"/><Default Extension=\"vml\" ContentType=\"{CT}.vmlDrawing\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"{CT}.spreadsheetml.sheet.main+xml\"/><Override PartName=\"/xl/calcChain.xml\" ContentType=\"{CT}.spreadsheetml.calcChain+xml\"/><Override PartName=\"/docProps/app.xml\" ContentType=\"{CT}.extended-properties+xml\"/><Override PartName=\"/docProps/custom.xml\" ContentType=\"{CT}.custom-properties+xml\"/>"
    );
    let mut wb_sheets = String::new();
    let mut wb_rels = format!("{DECL}<Relationships xmlns=\"{PKG_REL}\">");
    let mut names = String::new();
    let mut titles = String::new();
    for (i, s) in sheets.iter().enumerate() {
        let k = i + 1;
        types.push_str(&format!(
            "<Override PartName=\"/xl/worksheets/sheet{k}.xml\" ContentType=\"{CT}.spreadsheetml.worksheet+xml\"/>"
        ));
        wb_sheets.push_str(&format!(
            "<sheet name=\"{}\" sheetId=\"{k}\" r:id=\"rId{k}\"/>",
            escape(s.name)
        ));
        wb_rels.push_str(&format!(
            "<Relationship Id=\"rId{k}\" Type=\"{REL}/worksheet\" Target=\"worksheets/sheet{k}.xml\"/>"
        ));
        let quoted = format!("'{}'", s.name.replace('\'', "''"));
        names.push_str(&format!(
            "<definedName name=\"_xlnm.Print_Area\" localSheetId=\"{i}\">{}!$A$1:$D$4</definedName>",
            escape(&quoted)
        ));
        titles.push_str(&format!("<vt:lpstr>{}</vt:lpstr>", escape(s.name)));
        let selected = if i + 1 == n { " tabSelected=\"1\"" } else { "" };
        let xml = format!(
            "{DECL}<worksheet xmlns=\"{MAIN}\" xmlns:r=\"{REL}\" xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" mc:Ignorable=\"x14ac\" xmlns:x14ac=\"http://schemas.microsoft.com/office/spreadsheetml/2009/9/ac\"><sheetPr codeName=\"Sheet{k}\"/><dimension ref=\"A1:D4\"/><sheetViews><sheetView{selected} workbookViewId=\"0\"/></sheetViews><sheetFormatPr baseColWidth=\"10\" defaultRowHeight=\"16\" x14ac:dyDescent=\"0.2\"/><sheetData>{}</sheetData><pageMargins left=\"0.7\" right=\"0.7\" top=\"0.75\" bottom=\"0.75\" header=\"0.3\" footer=\"0.3\"/>{}</worksheet>",
            s.rows, s.tail
        );
        entries.push((format!("xl/worksheets/sheet{k}.xml"), xml.into_bytes()));
        if !s.rels.is_empty() {
            let mut rels = format!("{DECL}<Relationships xmlns=\"{PKG_REL}\">");
            for (id, ty, target) in &s.rels {
                rels.push_str(&format!(
                    "<Relationship Id=\"{id}\" Type=\"{REL}/{ty}\" Target=\"{target}\"/>"
                ));
            }
            rels.push_str("</Relationships>");
            entries.push((
                format!("xl/worksheets/_rels/sheet{k}.xml.rels"),
                rels.into_bytes(),
            ));
        }
    }
    wb_rels.push_str(&format!(
        "<Relationship Id=\"rId{}\" Type=\"{REL}/calcChain\" Target=\"calcChain.xml\"/></Relationships>",
        n + 1
    ));
    for (name, ct, _) in extra {
        if !ct.is_empty() {
            types.push_str(&format!(
                "<Override PartName=\"/{name}\" ContentType=\"{CT}.{ct}\"/>"
            ));
        }
    }
    types.push_str("</Types>");
    let workbook = format!(
        "{DECL}<workbook xmlns=\"{MAIN}\" xmlns:r=\"{REL}\"><workbookPr defaultThemeVersion=\"202300\"/><bookViews><workbookView xWindow=\"0\" yWindow=\"0\" windowWidth=\"28000\" windowHeight=\"17000\" activeTab=\"{}\"/></bookViews><sheets>{wb_sheets}</sheets><definedNames>{names}</definedNames><calcPr calcId=\"191029\"/></workbook>",
        n - 1
    );
    let app = format!(
        "{DECL}<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\" xmlns:vt=\"http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes\"><Application>Microsoft Macintosh Excel</Application><HeadingPairs><vt:vector size=\"4\" baseType=\"variant\"><vt:variant><vt:lpstr>Worksheets</vt:lpstr></vt:variant><vt:variant><vt:i4>{n}</vt:i4></vt:variant><vt:variant><vt:lpstr>Named Ranges</vt:lpstr></vt:variant><vt:variant><vt:i4>1</vt:i4></vt:variant></vt:vector></HeadingPairs><TitlesOfParts><vt:vector size=\"{}\" baseType=\"lpstr\">{titles}<vt:lpstr>{}!Print_Area</vt:lpstr></vt:vector></TitlesOfParts><AppVersion>16.0300</AppVersion></Properties>",
        n + 1,
        escape(&format!("'{}'", sheets[n - 1].name.replace('\'', "''")))
    );
    let custom = format!(
        "{DECL}<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/custom-properties\" xmlns:vt=\"http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes\"><property fmtid=\"{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}\" pid=\"2\" name=\"lang\"><vt:lpwstr>fr</vt:lpwstr></property><property fmtid=\"{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}\" pid=\"3\" name=\"t.site.fr\"><vt:lpwstr>Site</vt:lpwstr></property></Properties>"
    );
    let root_rels = format!(
        "{DECL}<Relationships xmlns=\"{PKG_REL}\"><Relationship Id=\"rId1\" Type=\"{REL}/officeDocument\" Target=\"xl/workbook.xml\"/><Relationship Id=\"rId2\" Type=\"{REL}/extended-properties\" Target=\"docProps/app.xml\"/><Relationship Id=\"rId3\" Type=\"{REL}/custom-properties\" Target=\"docProps/custom.xml\"/></Relationships>"
    );
    let calc = format!("{DECL}<calcChain xmlns=\"{MAIN}\"><c r=\"D2\" i=\"1\"/></calcChain>");
    let mut all: Vec<(String, Vec<u8>)> = vec![
        ("[Content_Types].xml".into(), types.into_bytes()),
        ("_rels/.rels".into(), root_rels.into_bytes()),
        ("docProps/app.xml".into(), app.into_bytes()),
        ("docProps/custom.xml".into(), custom.into_bytes()),
        ("xl/workbook.xml".into(), workbook.into_bytes()),
        ("xl/_rels/workbook.xml.rels".into(), wb_rels.into_bytes()),
        ("xl/calcChain.xml".into(), calc.into_bytes()),
    ];
    all.extend(entries);
    for (name, _, data) in extra {
        all.push((name.to_string(), data.to_vec()));
    }
    let refs: Vec<(&str, &[u8])> = all
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();
    zip(&refs)
}

fn s(r: &str, v: &str) -> String {
    cell(r, "inlineStr", v)
}

fn n(r: &str, v: &str) -> String {
    cell(r, "n", v)
}

fn f(r: &str, v: &str) -> String {
    cell(r, "f", v)
}

fn names_in(bytes: &[u8]) -> Vec<String> {
    entries(bytes).into_iter().map(|(n, _)| n).collect()
}

fn pair(name: &str, part: &str) -> (String, String) {
    (name.to_string(), part.to_string())
}

#[test]
fn an_excel_workbook_repeats_a_sheet_and_keeps_its_indexes_in_step() {
    let doc = book(
        &[
            sheet(
                "Summary",
                &[row(1, &[s("A1", "{{ sites | length }} sites")])],
            ),
            sheet(
                "{{ s.name }}",
                &[
                    row(
                        1,
                        &[s(
                            "A1",
                            "{% sheet for s in sites %}{{ \"site\" | t }} {{ s.name }}",
                        )],
                    ),
                    row(
                        2,
                        &[
                            s("A2", "{% for i in s.items %}{{ i.name }}"),
                            s("B2", "{{ i.qty }}{% endfor %}"),
                        ],
                    ),
                    row(3, &[s("A3", "Total"), f("B3", "SUM(B2:B2)")]),
                ],
            ),
        ],
        &[],
    );
    let data = json!({"sites": [
        {"name": "Lyon", "items": [{"name": "a", "qty": 1}, {"name": "b", "qty": 2}]},
        {"name": "Nice", "items": []}
    ]});
    let out = compile(Format::Xlsx, &doc).unwrap().render(&data).unwrap();
    keep("book.xlsx", &out);
    assert_eq!(
        xlsx_sheets(&out),
        [
            pair("Summary", "xl/worksheets/sheet1.xml"),
            pair("Lyon", "xl/worksheets/sheet2.xml"),
            pair("Nice", "xl/worksheets/sheet3.xml"),
        ]
    );
    let lyon = cells(&part(&out, "xl/worksheets/sheet2.xml"));
    // `lang` and `t.site.fr` come from custom properties.
    assert_eq!(get(&lyon, "A1"), "Site Lyon");
    assert_eq!(get(&lyon, "B3"), "2");
    assert_eq!(get(&lyon, "B4"), "=SUM(B2:B3)");
    let nice = part(&out, "xl/worksheets/sheet3.xml");
    // The empty loop's range is the row above it, not the formula's own.
    assert_eq!(get(&cells(&nice), "B2"), "=SUM(B1:B1)");
    // Only the template's own copy stays the selected tab, and a copy has no
    // code name of its own.
    assert!(part(&out, "xl/worksheets/sheet2.xml").contains("tabSelected=\"1\""));
    assert!(
        !nice.contains("tabSelected") && !nice.contains("codeName"),
        "{nice}"
    );
    let wb = part(&out, "xl/workbook.xml");
    assert!(wb.contains("activeTab=\"1\""), "{wb}");
    for want in [
        "<definedName name=\"_xlnm.Print_Area\" localSheetId=\"0\">'Summary'!$A$1:$D$4</definedName>",
        "<definedName name=\"_xlnm.Print_Area\" localSheetId=\"1\">'Lyon'!$A$1:$D$5</definedName>",
        "<definedName name=\"_xlnm.Print_Area\" localSheetId=\"2\">'Nice'!$A$1:$D$3</definedName>",
    ] {
        assert!(wb.contains(want), "missing {want} in {wb}");
    }
    assert!(wb.contains("fullCalcOnLoad=\"1\""), "{wb}");
    assert!(!names_in(&out).contains(&"xl/calcChain.xml".to_string()));
    let app = part(&out, "docProps/app.xml");
    assert!(app.contains("<vt:i4>3</vt:i4>"), "{app}");
    assert!(
        app.contains("<vt:vector size=\"4\" baseType=\"lpstr\"><vt:lpstr>Summary</vt:lpstr><vt:lpstr>Lyon</vt:lpstr><vt:lpstr>Nice</vt:lpstr><vt:lpstr>'Lyon'!Print_Area</vt:lpstr></vt:vector>"),
        "{app}"
    );
    let rels = part(&out, "xl/_rels/workbook.xml.rels");
    assert!(rels.contains("Target=\"worksheets/sheet3.xml\""), "{rels}");
    assert!(!rels.contains("calcChain"), "{rels}");
}

#[test]
fn an_empty_list_removes_the_sheet() {
    let doc = book(
        &[
            sheet("Cover", &[row(1, &[s("A1", "Report")])]),
            sheet(
                "{{ s }}",
                &[row(1, &[s("A1", "{% sheet for s in sites %}{{ s }}")])],
            ),
        ],
        &[],
    );
    let out = compile(Format::Xlsx, &doc)
        .unwrap()
        .render(&json!({"sites": []}))
        .unwrap();
    assert_eq!(
        xlsx_sheets(&out),
        [pair("Cover", "xl/worksheets/sheet1.xml")]
    );
    assert!(!names_in(&out).contains(&"xl/worksheets/sheet2.xml".to_string()));
    let wb = part(&out, "xl/workbook.xml");
    assert!(wb.contains("activeTab=\"0\""), "{wb}");
    assert_eq!(wb.matches("<definedName ").count(), 1, "{wb}");
    assert!(!part(&out, "[Content_Types].xml").contains("sheet2"));
    assert!(!part(&out, "xl/_rels/workbook.xml.rels").contains("sheet2"));

    // A workbook needs a sheet.
    let only = book(
        &[sheet(
            "{{ s }}",
            &[row(1, &[s("A1", "{% sheet for s in sites %}{{ s }}")])],
        )],
        &[],
    );
    let err = compile(Format::Xlsx, &only)
        .unwrap()
        .render(&json!({"sites": []}))
        .unwrap_err();
    assert!(err.message.contains("no sheets"), "{err}");
}

#[test]
fn a_sheet_name_can_come_from_data_without_a_loop() {
    // The sheet's cells hold no tag; its name does.
    let doc = book(
        &[sheet("{{ month }}: sales", &[row(1, &[n("A1", "1")])])],
        &[],
    );
    let out = compile(Format::Xlsx, &doc)
        .unwrap()
        .render(&json!({"month": "March [draft]"}))
        .unwrap();
    assert_eq!(xlsx_sheets(&out)[0].0, "March _draft__ sales");
    let wb = part(&out, "xl/workbook.xml");
    assert!(wb.contains("'March _draft__ sales'!$A$1:$D$4"), "{wb}");
}

/// A copy gets its own table, drawing, chart and comments, each with a part,
/// a content type and the ids its kind must keep unique; the picture is
/// shared.
#[test]
fn a_copy_gets_its_own_tables_drawings_charts_and_comments() {
    const C_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
    const A_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
    const XDR: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
    let table = format!("{DECL}<table xmlns=\"{MAIN}\" id=\"1\" name=\"Items\" displayName=\"Items\" ref=\"A1:B2\" totalsRowShown=\"0\"><autoFilter ref=\"A1:B2\"/><tableColumns count=\"2\"><tableColumn id=\"1\" name=\"Item\"/><tableColumn id=\"2\" name=\"Qty\"/></tableColumns><tableStyleInfo name=\"TableStyleMedium2\" showFirstColumn=\"0\" showLastColumn=\"0\" showRowStripes=\"1\" showColumnStripes=\"0\"/></table>");
    let chart = format!("{DECL}<c:chartSpace xmlns:c=\"{C_NS}\" xmlns:a=\"{A_NS}\" xmlns:r=\"{REL}\"><c:chart><c:plotArea><c:layout/><c:barChart><c:barDir val=\"col\"/><c:grouping val=\"clustered\"/><c:ser><c:idx val=\"0\"/><c:order val=\"0\"/><c:cat><c:strRef><c:f>'{{{{ s.name }}}}'!$A$2:$A$2</c:f></c:strRef></c:cat><c:val><c:numRef><c:f>'{{{{ s.name }}}}'!$B$2:$B$2</c:f></c:numRef></c:val></c:ser><c:axId val=\"1\"/><c:axId val=\"2\"/></c:barChart><c:catAx><c:axId val=\"1\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"b\"/><c:crossAx val=\"2\"/></c:catAx><c:valAx><c:axId val=\"2\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"l\"/><c:crossAx val=\"1\"/></c:valAx></c:plotArea></c:chart></c:chartSpace>");
    let drawing = format!("{DECL}<xdr:wsDr xmlns:xdr=\"{XDR}\" xmlns:a=\"{A_NS}\" xmlns:r=\"{REL}\" xmlns:c=\"{C_NS}\"><xdr:twoCellAnchor><xdr:from><xdr:col>3</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>8</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>12</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:graphicFrame macro=\"\"><xdr:nvGraphicFramePr><xdr:cNvPr id=\"2\" name=\"Chart 1\"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/></xdr:xfrm><a:graphic><a:graphicData uri=\"{C_NS}\"><c:chart r:id=\"rId1\"/></a:graphicData></a:graphic></xdr:graphicFrame><xdr:clientData/></xdr:twoCellAnchor></xdr:wsDr>");
    let drawing_rels = format!("{DECL}<Relationships xmlns=\"{PKG_REL}\"><Relationship Id=\"rId1\" Type=\"{REL}/chart\" Target=\"../charts/chart1.xml\"/><Relationship Id=\"rId2\" Type=\"{REL}/image\" Target=\"../media/image1.png\"/></Relationships>");
    let comments = format!("{DECL}<comments xmlns=\"{MAIN}\"><authors><author>A</author></authors><commentList><comment ref=\"A1\" authorId=\"0\"><text><r><t>Note</t></r></text></comment></commentList></comments>");
    let vml = "<xml xmlns:v=\"urn:schemas-microsoft-com:vml\" xmlns:o=\"urn:schemas-microsoft-com:office:office\" xmlns:x=\"urn:schemas-microsoft-com:office:excel\"><o:shapelayout v:ext=\"edit\"><o:idmap v:ext=\"edit\" data=\"1\"/></o:shapelayout><v:shapetype id=\"_x0000_t202\" coordsize=\"21600,21600\" o:spt=\"202\" path=\"m,l,21600r21600,l21600,xe\"><v:stroke joinstyle=\"miter\"/><v:path gradientshapeok=\"t\" o:connecttype=\"rect\"/></v:shapetype><v:shape id=\"_x0000_s1025\" type=\"#_x0000_t202\" style=\"position:absolute;visibility:hidden\" fillcolor=\"#ffffe1\" o:insetmode=\"auto\"><v:fill color2=\"#ffffe1\"/><v:shadow on=\"t\" color=\"black\" obscured=\"t\"/><v:path o:connecttype=\"none\"/><v:textbox style=\"mso-direction-alt:auto\"><div style=\"text-align:left\"/></v:textbox><x:ClientData ObjectType=\"Note\"><x:MoveWithCells/><x:SizeWithCells/><x:Anchor>1, 15, 0, 2, 3, 15, 3, 16</x:Anchor><x:AutoFill>False</x:AutoFill><x:Row>0</x:Row><x:Column>0</x:Column></x:ClientData></v:shape></xml>";
    let png = png(2, 2);
    let mut looped = sheet(
        "{{ s.name }}",
        &[
            row(
                1,
                &[s("A1", "{% sheet for s in sites %}Item"), s("B1", "Qty")],
            ),
            row(2, &[s("A2", "{{ s.name }}"), n("B2", "1")]),
            row(3, &[s("A3", "Total"), f("B3", "SUM(Items[Qty])")]),
        ],
    );
    looped.tail = "<drawing r:id=\"rId2\"/><legacyDrawing r:id=\"rId4\"/><tableParts count=\"1\"><tablePart r:id=\"rId1\"/></tableParts>";
    looped.rels = vec![
        ("rId1", "table", "../tables/table1.xml"),
        ("rId2", "drawing", "../drawings/drawing1.xml"),
        ("rId3", "comments", "../comments1.xml"),
        ("rId4", "vmlDrawing", "../drawings/vmlDrawing1.vml"),
    ];
    let doc = book(
        &[looped],
        &[
            (
                "xl/tables/table1.xml",
                "spreadsheetml.table+xml",
                table.as_bytes(),
            ),
            (
                "xl/charts/chart1.xml",
                "drawingml.chart+xml",
                chart.as_bytes(),
            ),
            (
                "xl/drawings/drawing1.xml",
                "drawing+xml",
                drawing.as_bytes(),
            ),
            (
                "xl/drawings/_rels/drawing1.xml.rels",
                "",
                drawing_rels.as_bytes(),
            ),
            (
                "xl/comments1.xml",
                "spreadsheetml.comments+xml",
                comments.as_bytes(),
            ),
            ("xl/drawings/vmlDrawing1.vml", "", vml.as_bytes()),
            ("xl/media/image1.png", "", &png),
        ],
    );
    let out = compile(Format::Xlsx, &doc)
        .unwrap()
        .render(&json!({"sites": [{"name": "Lyon"}, {"name": "Nice"}]}))
        .unwrap();
    keep("parts.xlsx", &out);
    let names = names_in(&out);
    for p in [
        "xl/worksheets/sheet2.xml",
        "xl/worksheets/_rels/sheet2.xml.rels",
        "xl/tables/table2.xml",
        "xl/drawings/drawing2.xml",
        "xl/drawings/_rels/drawing2.xml.rels",
        "xl/charts/chart2.xml",
        "xl/comments2.xml",
        "xl/drawings/vmlDrawing2.vml",
    ] {
        assert!(names.contains(&p.to_string()), "missing {p} in {names:?}");
    }
    assert!(
        !names.contains(&"xl/media/image2.png".to_string()),
        "the picture is shared"
    );
    let rels = part(&out, "xl/worksheets/_rels/sheet2.xml.rels");
    for t in [
        "../tables/table2.xml",
        "../drawings/drawing2.xml",
        "../comments2.xml",
        "../drawings/vmlDrawing2.vml",
    ] {
        assert!(rels.contains(&format!("Target=\"{t}\"")), "{rels}");
    }
    let drels = part(&out, "xl/drawings/_rels/drawing2.xml.rels");
    assert!(
        drels.contains("../charts/chart2.xml") && drels.contains("../media/image1.png"),
        "{drels}"
    );
    let t2 = part(&out, "xl/tables/table2.xml");
    assert!(
        t2.contains("id=\"2\"") && t2.contains("name=\"Items_2\" displayName=\"Items_2\""),
        "{t2}"
    );
    assert_eq!(
        get(&cells(&part(&out, "xl/worksheets/sheet2.xml")), "B3"),
        "=SUM(Items_2[Qty])"
    );
    assert_eq!(
        get(&cells(&part(&out, "xl/worksheets/sheet1.xml")), "B3"),
        "=SUM(Items[Qty])"
    );
    assert!(part(&out, "xl/charts/chart1.xml").contains("<c:f>'Lyon'!$B$2:$B$2</c:f>"));
    assert!(part(&out, "xl/charts/chart2.xml").contains("<c:f>'Nice'!$B$2:$B$2</c:f>"));
    let vml2 = part(&out, "xl/drawings/vmlDrawing2.vml");
    assert!(
        vml2.contains("data=\"2\"") && vml2.contains("_x0000_s2049"),
        "{vml2}"
    );
    let types = part(&out, "[Content_Types].xml");
    for p in [
        "/xl/worksheets/sheet2.xml",
        "/xl/tables/table2.xml",
        "/xl/drawings/drawing2.xml",
        "/xl/charts/chart2.xml",
        "/xl/comments2.xml",
    ] {
        assert!(types.contains(&format!("PartName=\"{p}\"")), "{p}: {types}");
    }

    if libreoffice_available() {
        let dir = tempfile::tempdir().unwrap();
        let dir_s = dir.path().to_str().unwrap();
        std::fs::write(dir.path().join("parts.xlsx"), &out).unwrap();
        assert!(soffice(dir_s, "pdf", "parts.xlsx"), "conversion failed");
        let text = pdf_text(dir_s, "parts.pdf");
        assert!(text.contains("Lyon") && text.contains("Nice"), "{text}");
    }
}

#[test]
fn misplaced_sheet_loops_are_refused_with_their_place() {
    let err = |format: Format, doc: &[u8]| compile(format, doc).unwrap_err();

    let e = err(Format::Docx, &docx(&p("{% sheet for s in sites %}{{ s }}")));
    assert!(e.message.contains("works in XLSX and ODS only"), "{e}");
    assert_eq!(e.location.unwrap().part, "word/document.xml");

    let two = book(
        &[sheet(
            "{{ s }}",
            &[row(
                1,
                &[
                    s("A1", "{% sheet for s in a %}"),
                    s("B1", "{% sheet for t in b %}"),
                ],
            )],
        )],
        &[],
    );
    let e = err(Format::Xlsx, &two);
    assert!(e.message.contains("a sheet takes one sheet loop"), "{e}");
    assert_eq!(
        e.location.unwrap().place,
        "sheet \"{{ s }}\" row 1 (cell B1)"
    );

    let inside = book(
        &[sheet(
            "{{ s }}",
            &[
                row(1, &[s("A1", "{% for r in rows %}{{ r }}")]),
                row(2, &[s("A2", "{% sheet for s in sites %}{% endfor %}")]),
            ],
        )],
        &[],
    );
    let e = err(Format::Xlsx, &inside);
    assert!(
        e.message.contains("sits inside '{% for r in rows %}'"),
        "{e}"
    );

    let malformed = book(
        &[sheet(
            "{{ s }}",
            &[row(1, &[s("A1", "{% sheet s in sites %}")])],
        )],
        &[],
    );
    let e = err(Format::Xlsx, &malformed);
    assert!(
        e.message
            .contains("write it as {% sheet for item in list %}"),
        "{e}"
    );

    let statement = book(
        &[sheet("{% if x %}A{% endif %}", &[row(1, &[n("A1", "1")])])],
        &[],
    );
    let e = err(Format::Xlsx, &statement);
    assert!(e.message.contains("can hold only {{ }} expressions"), "{e}");
    let loc = e.location.unwrap();
    assert_eq!(loc.part, "xl/workbook.xml");
    assert_eq!(loc.place, "sheet \"{% if x %}A{% endif %}\" name");

    let ods_body = "<table:table table:name=\"S\"><table:table-row><table:table-cell><text:p>{% sheet for s in a %}{% sheet for t in b %}</text:p></table:table-cell></table:table-row></table:table>";
    let e = err(Format::Ods, &ods(ods_body));
    assert!(e.message.contains("a sheet takes one sheet loop"), "{e}");
}

#[test]
fn a_missing_value_in_a_sheet_name_is_reported_at_the_name() {
    let doc = book(&[sheet("{{ nope }}", &[row(1, &[n("A1", "1")])])], &[]);
    let found = validate(Format::Xlsx, &doc, &json!({}));
    let e = found
        .iter()
        .find(|d| d.severity == Severity::Error)
        .unwrap_or_else(|| panic!("{found:?}"));
    let loc = e.location.as_ref().unwrap();
    assert_eq!(
        (loc.part.as_str(), loc.place.as_str()),
        ("xl/workbook.xml", "sheet \"{{ nope }}\" name")
    );
    let err = compile(Format::Xlsx, &doc)
        .unwrap()
        .render(&json!({}))
        .unwrap_err();
    assert_eq!(err.location.unwrap().place, "sheet \"{{ nope }}\" name");
}

#[test]
fn an_ods_sheet_loop_without_a_name_numbers_its_copies() {
    let body = "<table:table table:name=\"Week\"><table:table-row><table:table-cell office:value-type=\"string\"><text:p>{% sheet for w in weeks %}Week {{ w }}</text:p></table:table-cell></table:table-row></table:table>";
    let out = compile(Format::Ods, &ods(body))
        .unwrap()
        .render(&json!({"weeks": [1, 2, 3]}))
        .unwrap();
    let xml = part(&out, "content.xml");
    let names: Vec<String> = Regex::new(r#"table:name="([^"]*)""#)
        .unwrap()
        .captures_iter(&xml)
        .map(|c| c[1].to_string())
        .collect();
    assert_eq!(names, ["Week", "Week (2)", "Week (3)"]);
    assert_eq!(odf_paragraphs(&xml), ["Week 1", "Week 2", "Week 3"]);
}

/// ODS formulas follow the rows a loop copies, as XLSX formulas do.
#[test]
fn ods_formulas_are_rebased_when_rows_repeat() {
    let text = |t: &str| {
        format!("<table:table-cell office:value-type=\"string\"><text:p>{t}</text:p></table:table-cell>")
    };
    let formula = |f: &str| {
        format!("<table:table-cell table:formula=\"{f}\" office:value-type=\"float\" office:value=\"0\"><text:p>0</text:p></table:table-cell>")
    };
    let body = format!(
        "<table:table table:name=\"S\"><table:table-header-rows><table:table-row>{}</table:table-row></table:table-header-rows><table:table-row>{}{}{}</table:table-row><table:table-row table:number-rows-repeated=\"2\"><table:table-cell/></table:table-row><table:table-row>{}{}</table:table-row></table:table>",
        text("Item"),
        text("{% for i in items %}{{ i.name }}"),
        text("{{ i.qty }}{% endfor %}"),
        formula("of:=[.B2]*2+[.$B$1]"),
        text("Total"),
        formula("of:=SUM([.C2:.C2])+[.C2]"),
    );
    let out = compile(Format::Ods, &ods(&body))
        .unwrap()
        .render(&items_data())
        .unwrap();
    let xml = part(&out, "content.xml");
    let formulas: Vec<String> = Regex::new(r#"table:formula="([^"]*)""#)
        .unwrap()
        .captures_iter(&xml)
        .map(|c| c[1].to_string())
        .collect();
    assert_eq!(
        formulas,
        [
            "of:=[.B2]*2+[.$B$1]",
            "of:=[.B3]*2+[.$B$1]",
            "of:=[.B4]*2+[.$B$1]",
            // Two blank rows sit between the loop and the total.
            "of:=SUM([.C2:.C4])+[.C4]"
        ]
    );
    // Cached results are dropped so that LibreOffice computes them.
    assert!(!xml.contains("office:value=\"0\""), "{xml}");
    assert!(!xml.contains("rusticdoc-"), "{xml}");
}

fn items_data() -> Value {
    json!({"items": [
        {"name": "Lamp", "qty": 2},
        {"name": "Plug", "qty": 1},
        {"name": "Desk", "qty": 1}
    ]})
}

/// A group row repeated per group, with item rows repeated per item inside
/// it, in each format that has rows. (Tags in one cell would repeat text in
/// that cell instead.)
#[test]
fn a_row_loop_nests_in_a_row_loop() {
    let data = json!({"groups": [
        {"name": "A", "items": [{"n": "a1"}, {"n": "a2"}]},
        {"name": "B", "items": []},
        {"name": "C", "items": [{"n": "c1"}]}
    ]});
    let first = ["A", "a1", "a2", "end", "B", "end", "C", "c1", "end"];

    let doc = docx(&tbl(&[
        &["{% for g in groups %}{{ g.name }}", "group"],
        &["{% for i in g.items %}{{ i.n }}", "item{% endfor %}"],
        &["end", "{{ g.name }}{% endfor %}"],
    ]));
    let out = compile(Format::Docx, &doc).unwrap().render(&data).unwrap();
    let text = docx_text(&out);
    let column: Vec<&str> = text.iter().step_by(2).map(String::as_str).collect();
    assert_eq!(column, first);
    assert_eq!(text.len(), 18, "{text:?}");

    let doc = xlsx(
        &[
            row(
                2,
                &[
                    s("A2", "{% for g in groups %}{{ g.name }}"),
                    s("B2", "group"),
                ],
            ),
            row(
                3,
                &[
                    s("A3", "{% for i in g.items %}{{ i.n }}"),
                    s("B3", "item{% endfor %}"),
                ],
            ),
            row(
                4,
                &[
                    s("A4", "end"),
                    s("B4", "{{ g.name }}{% endfor %}"),
                    f("C4", "COUNTA(A2:A3)"),
                ],
            ),
            row(5, &[f("C5", "COUNTA(A2:A4)")]),
        ],
        &[],
    );
    let out = compile(Format::Xlsx, &doc).unwrap().render(&data).unwrap();
    keep("nested.xlsx", &out);
    let c = cells(&part(&out, "xl/worksheets/sheet1.xml"));
    let column: Vec<&str> = (2..=10).map(|r| get(&c, &format!("A{r}"))).collect();
    assert_eq!(column, first);
    assert_eq!(get(&c, "C5"), "=COUNTA(A2:A4)");
    assert_eq!(get(&c, "C7"), "=COUNTA(A6:A6)");
    assert_eq!(get(&c, "C10"), "=COUNTA(A8:A9)");
    assert_eq!(get(&c, "C11"), "=COUNTA(A2:A10)");

    let ods_row = |a: &str, b: &str| {
        format!("<table:table-row><table:table-cell office:value-type=\"string\"><text:p>{a}</text:p></table:table-cell><table:table-cell office:value-type=\"string\"><text:p>{b}</text:p></table:table-cell></table:table-row>")
    };
    let body = format!(
        "<table:table table:name=\"S\">{}{}{}</table:table>",
        ods_row("{% for g in groups %}{{ g.name }}", "group"),
        ods_row("{% for i in g.items %}{{ i.n }}", "item{% endfor %}"),
        ods_row("end", "{{ g.name }}{% endfor %}"),
    );
    let out = compile(Format::Ods, &ods(&body))
        .unwrap()
        .render(&data)
        .unwrap();
    let text = odf_paragraphs(&part(&out, "content.xml"));
    let column: Vec<&str> = text.iter().step_by(2).map(String::as_str).collect();
    assert_eq!(column, first);
}

/// Copies count against the package's limits: a sheet loop over a long list
/// stops with an error instead of building a package nothing can open.
#[test]
fn a_sheet_loop_stops_at_the_package_limits() {
    let mut looped = sheet(
        "{{ s }}",
        &[row(
            1,
            &[s("A1", "{% sheet for s in range(4000) %}{{ s }}")],
        )],
    );
    looped.tail = "<tableParts count=\"1\"><tablePart r:id=\"rId1\"/></tableParts>";
    looped.rels = vec![("rId1", "table", "../tables/table1.xml")];
    let table = format!("{DECL}<table xmlns=\"{MAIN}\" id=\"1\" name=\"T\" displayName=\"T\" ref=\"A1:A2\"><tableColumns count=\"1\"><tableColumn id=\"1\" name=\"A\"/></tableColumns></table>");
    let doc = book(
        &[looped],
        &[(
            "xl/tables/table1.xml",
            "spreadsheetml.table+xml",
            table.as_bytes(),
        )],
    );
    let err = compile(Format::Xlsx, &doc)
        .unwrap()
        .render(&json!({}))
        .unwrap_err();
    assert!(
        err.message.contains("a package holds at most 10000"),
        "{err}"
    );
}

/// Copies that cannot fit are refused before any is built: a sheet owning a
/// 2 MB part, repeated 200 times, would pass the 256 MB package limit.
#[test]
fn a_sheet_loop_that_cannot_fit_is_refused_before_copying() {
    let note = "x".repeat(2 << 20);
    let comments = format!("{DECL}<comments xmlns=\"{MAIN}\"><authors><author>A</author></authors><commentList><comment ref=\"A1\" authorId=\"0\"><text><r><t>{note}</t></r></text></comment></commentList></comments>");
    let mut looped = sheet(
        "{{ s }}",
        &[row(1, &[s("A1", "{% sheet for s in range(200) %}{{ s }}")])],
    );
    looped.rels = vec![("rId1", "comments", "../comments1.xml")];
    let doc = book(
        &[looped],
        &[(
            "xl/comments1.xml",
            "spreadsheetml.comments+xml",
            comments.as_bytes(),
        )],
    );
    let started = std::time::Instant::now();
    let err = compile(Format::Xlsx, &doc)
        .unwrap()
        .render(&json!({}))
        .unwrap_err();
    // "would": the check that runs before copying, not the running total.
    assert!(
        err.message
            .contains("would make the package pass its limit of 268435456 bytes"),
        "{err}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
}

/// Every event of `xml` reads, with matching end tags.
fn well_formed(name: &str, xml: &str) {
    let mut reader = quick_xml::Reader::from_str(xml);
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(_) => {}
            Err(e) => panic!("{name} is not well-formed: {e}\n{xml}"),
        }
    }
}

/// The value of attribute `attr` on the first `elem` start tag in `xml`,
/// unescaped.
fn attr_of(xml: &str, elem: &str, attr: &str) -> String {
    let re = Regex::new(&format!(r#"<{elem}\b[^>]*\s{attr}="([^"]*)""#)).unwrap();
    let raw = re
        .captures(xml)
        .unwrap_or_else(|| panic!("no {elem} {attr} in {xml}"))[1]
        .to_string();
    quick_xml::escape::unescape(&raw).unwrap().into_owned()
}

/// A sheet named from data holding `"`, `<`, `&` and `'` gives well-formed
/// parts, and the references that name it, in text and in attributes, read
/// back as that name.
#[test]
fn a_sheet_name_with_markup_characters_stays_well_formed() {
    let name = "A\"b<c&d'e";
    let quoted = "'A\"b<c&d''e'";
    let data = json!({"sites": [{"name": name}]});

    let mut looped = sheet(
        "{{ s.name }}",
        &[row(
            1,
            &[
                s("A1", "{% sheet for s in sites %}{{ s.name }}"),
                n("B1", "1"),
            ],
        )],
    );
    looped.tail = "<hyperlinks><hyperlink ref=\"A1\" location=\"'{{ s.name }}'!B1\" display=\"Go\"/></hyperlinks>";
    let doc = book(
        &[
            sheet("Summary", &[row(1, &[f("A1", "'{{ s.name }}'!B1")])]),
            looped,
        ],
        &[],
    );
    let out = compile(Format::Xlsx, &doc).unwrap().render(&data).unwrap();
    for (part, bytes) in entries(&out) {
        if part.ends_with(".xml") || part.ends_with(".rels") {
            well_formed(&part, &String::from_utf8(bytes).unwrap());
        }
    }
    let wb = part(&out, "xl/workbook.xml");
    assert_eq!(
        attr_of(&wb, "sheet name=\"Summary\"[^>]*/><sheet", "name"),
        name
    );
    assert!(
        wb.contains(&format!(
            ">{}!$A$1:$D$4</definedName>",
            quoted.replace('&', "&amp;").replace('<', "&lt;")
        )),
        "{wb}"
    );
    let looped = part(&out, "xl/worksheets/sheet2.xml");
    assert_eq!(
        attr_of(&looped, "hyperlink", "location"),
        format!("{quoted}!B1")
    );
    let summary = cells(&part(&out, "xl/worksheets/sheet1.xml"));
    assert_eq!(get(&summary, "A1"), format!("={quoted}!B1"));

    let body = "<table:table table:name=\"Summary\"><table:table-row><table:table-cell table:formula=\"of:=[$'{{ s.name }}'.A1]\"><text:p>0</text:p></table:table-cell></table:table-row></table:table><table:table table:name=\"{{ s.name }}\" table:print-ranges=\"'{{ s.name }}'.A1:'{{ s.name }}'.B2\"><table:table-row><table:table-cell office:value-type=\"string\"><text:p>{% sheet for s in sites %}{{ s.name }}</text:p></table:table-cell></table:table-row></table:table>";
    let out = compile(Format::Ods, &ods(body))
        .unwrap()
        .render(&data)
        .unwrap();
    let xml = part(&out, "content.xml");
    well_formed("content.xml", &xml);
    let names: Vec<String> = Regex::new(r#"<table:table table:name="([^"]*)""#)
        .unwrap()
        .captures_iter(&xml)
        .map(|c| quick_xml::escape::unescape(&c[1]).unwrap().into_owned())
        .collect();
    assert_eq!(names, ["Summary", name]);
    assert_eq!(
        attr_of(&xml, "table:table-cell", "table:formula"),
        format!("of:=[${quoted}.A1]")
    );
    assert_eq!(
        attr_of(&xml, "table:table", "table:print-ranges"),
        format!("{quoted}.A1:{quoted}.B2")
    );
}
