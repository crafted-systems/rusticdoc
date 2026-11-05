//! Builders for minimal but valid Office files, and readers for their text.
#![allow(dead_code)]

use std::io::{Cursor, Read, Write};

use regex::Regex;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

const XML_DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";
const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const CT_NS: &str = "http://schemas.openxmlformats.org/package/2006/content-types";

/// Writes entries into a ZIP; `mimetype` is stored uncompressed and first.
pub fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in entries {
        let method = if *name == "mimetype" {
            CompressionMethod::Stored
        } else {
            CompressionMethod::Deflated
        };
        z.start_file(
            *name,
            SimpleFileOptions::default().compression_method(method),
        )
        .unwrap();
        z.write_all(data).unwrap();
    }
    z.finish().unwrap().into_inner()
}

/// The entries of a ZIP in order.
pub fn entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut a = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    (0..a.len())
        .map(|i| {
            let mut f = a.by_index(i).unwrap();
            let mut data = Vec::new();
            f.read_to_end(&mut data).unwrap();
            (f.name().to_string(), data)
        })
        .collect()
}

pub fn part(bytes: &[u8], name: &str) -> String {
    let (_, data) = entries(bytes)
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("no part {name}"));
    String::from_utf8(data).unwrap()
}

fn rels(items: &[(&str, &str, &str)]) -> String {
    let mut s = format!("{XML_DECL}<Relationships xmlns=\"{REL_NS}\">");
    for (id, ty, target) in items {
        s.push_str(&format!(
            "<Relationship Id=\"{id}\" Type=\"{R_NS}/{ty}\" Target=\"{target}\"/>"
        ));
    }
    s.push_str("</Relationships>");
    s
}

fn content_types(overrides: &[(&str, &str)]) -> String {
    let mut s = format!(
        "{XML_DECL}<Types xmlns=\"{CT_NS}\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Default Extension=\"png\" ContentType=\"image/png\"/>"
    );
    for (part, ct) in overrides {
        s.push_str(&format!(
            "<Override PartName=\"{part}\" ContentType=\"application/vnd.openxmlformats-officedocument.{ct}\"/>"
        ));
    }
    s.push_str("</Types>");
    s
}

// --- DOCX -----------------------------------------------------------------

/// A paragraph of runs. `"a§b"` splits text into two runs; `"a§!b"` puts a
/// `w:proofErr` between them, as Word does around spelling marks.
pub fn p(text: &str) -> String {
    let mut s = String::from("<w:p><w:pPr><w:pStyle w:val=\"Normal\"/></w:pPr>");
    for (i, piece) in text.split('§').enumerate() {
        let (marker, piece) = match piece.strip_prefix('!') {
            Some(rest) => ("<w:proofErr w:type=\"spellStart\"/>", rest),
            None => ("", piece),
        };
        if i > 0 {
            s.push_str(marker);
        }
        s.push_str(&format!(
            "<w:r><w:rPr><w:rsid w:val=\"00{i}\"/></w:rPr><w:t xml:space=\"preserve\">{}</w:t></w:r>",
            escape(piece)
        ));
    }
    s.push_str("</w:p>");
    s
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
}

/// A table; each cell holds one paragraph built with [`p`].
pub fn tbl(rows: &[&[&str]]) -> String {
    let mut s = String::from("<w:tbl><w:tblPr><w:tblStyle w:val=\"TableGrid\"/><w:tblW w:w=\"0\" w:type=\"auto\"/></w:tblPr><w:tblGrid>");
    for _ in rows[0] {
        s.push_str("<w:gridCol w:w=\"3000\"/>");
    }
    s.push_str("</w:tblGrid>");
    for row in rows {
        s.push_str("<w:tr>");
        for cell in *row {
            s.push_str(&format!(
                "<w:tc><w:tcPr><w:tcW w:w=\"3000\" w:type=\"dxa\"/></w:tcPr>{}</w:tc>",
                p(cell)
            ));
        }
        s.push_str("</w:tr>");
    }
    s.push_str("</w:tbl>");
    s
}

pub fn document_xml(body: &str) -> String {
    format!(
        "{XML_DECL}<w:document xmlns:w=\"{W_NS}\" xmlns:r=\"{R_NS}\"><w:body>{body}<w:sectPr><w:headerReference w:type=\"default\" r:id=\"rId1\"/><w:pgSz w:w=\"11906\" w:h=\"16838\"/></w:sectPr></w:body></w:document>"
    )
}

/// A DOCX with this body, a header holding `header`, and a small PNG.
pub fn docx_with(body: &str, header: &str) -> Vec<u8> {
    let header_xml = format!("{XML_DECL}<w:hdr xmlns:w=\"{W_NS}\">{}</w:hdr>", p(header));
    zip(&[
        (
            "[Content_Types].xml",
            content_types(&[
                ("/word/document.xml", "wordprocessingml.document.main+xml"),
                ("/word/header1.xml", "wordprocessingml.header+xml"),
            ])
            .as_bytes(),
        ),
        (
            "_rels/.rels",
            rels(&[("rId1", "officeDocument", "word/document.xml")]).as_bytes(),
        ),
        ("word/document.xml", document_xml(body).as_bytes()),
        (
            "word/_rels/document.xml.rels",
            rels(&[
                ("rId1", "header", "header1.xml"),
                ("rId2", "image", "media/image1.png"),
            ])
            .as_bytes(),
        ),
        ("word/header1.xml", header_xml.as_bytes()),
        (
            "word/media/image1.png",
            &[0x89, b'P', b'N', b'G', 0, 1, 2, 3, 0xff],
        ),
    ])
}

pub fn docx(body: &str) -> Vec<u8> {
    docx_with(body, "Header text")
}

/// The text of each paragraph-like element, in order.
pub fn paragraphs(xml: &str, para: &str, text: &str) -> Vec<String> {
    let pre = Regex::new(&format!("(?s)<{para}[ >].*?</{para}>")).unwrap();
    let tre = Regex::new(&format!("(?s)<{text}(?: [^>]*)?>(.*?)</{text}>|<{text}/>")).unwrap();
    pre.find_iter(xml)
        .map(|m| {
            tre.captures_iter(m.as_str())
                .map(|c| c.get(1).map(|g| unescape(g.as_str())).unwrap_or_default())
                .collect::<Vec<_>>()
                .join("")
        })
        .collect()
}

pub fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

pub fn docx_text(bytes: &[u8]) -> Vec<String> {
    paragraphs(&part(bytes, "word/document.xml"), "w:p", "w:t")
}

// --- XLSX -----------------------------------------------------------------

/// A cell; `t` is `s` (shared index in `v`), `inlineStr`, `n` or `f`
/// (formula in `v`).
pub fn cell(r: &str, t: &str, v: &str) -> String {
    match t {
        "s" => format!("<c r=\"{r}\" t=\"s\"><v>{v}</v></c>"),
        "inlineStr" => format!(
            "<c r=\"{r}\" t=\"inlineStr\"><is><t>{}</t></is></c>",
            escape(v)
        ),
        "f" => format!("<c r=\"{r}\"><f>{}</f><v>0</v></c>", escape(v)),
        _ => format!("<c r=\"{r}\"><v>{v}</v></c>"),
    }
}

pub fn row(r: u32, cells: &[String]) -> String {
    format!("<row r=\"{r}\">{}</row>", cells.join(""))
}

/// An XLSX with one sheet named `Sales` and these shared strings. Shared
/// strings may use `<r>` rich runs by starting with `<`.
pub fn xlsx(rows: &[String], shared: &[&str]) -> Vec<u8> {
    let last = rows.len().max(1);
    let sheet = format!(
        "{XML_DECL}<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"{R_NS}\"><dimension ref=\"A1:D{last}\"/><sheetData>{}</sheetData></worksheet>",
        rows.join("")
    );
    let mut sst = format!(
        "{XML_DECL}<sst xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" count=\"{}\" uniqueCount=\"{}\">",
        shared.len(),
        shared.len()
    );
    for s in shared {
        if s.starts_with('<') {
            sst.push_str(&format!("<si>{s}</si>"));
        } else {
            sst.push_str(&format!("<si><t>{}</t></si>", escape(s)));
        }
    }
    sst.push_str("</sst>");
    let workbook = format!(
        "{XML_DECL}<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"{R_NS}\"><sheets><sheet name=\"Sales\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>"
    );
    zip(&[
        (
            "[Content_Types].xml",
            content_types(&[
                ("/xl/workbook.xml", "spreadsheetml.sheet.main+xml"),
                ("/xl/worksheets/sheet1.xml", "spreadsheetml.worksheet+xml"),
                ("/xl/sharedStrings.xml", "spreadsheetml.sharedStrings+xml"),
            ])
            .as_bytes(),
        ),
        (
            "_rels/.rels",
            rels(&[("rId1", "officeDocument", "xl/workbook.xml")]).as_bytes(),
        ),
        ("xl/workbook.xml", workbook.as_bytes()),
        (
            "xl/_rels/workbook.xml.rels",
            rels(&[
                ("rId1", "worksheet", "worksheets/sheet1.xml"),
                ("rId2", "sharedStrings", "sharedStrings.xml"),
            ])
            .as_bytes(),
        ),
        ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
        ("xl/sharedStrings.xml", sst.as_bytes()),
    ])
}

/// `(ref, value)` of every cell: formulas as `=...`, inline strings as text.
pub fn cells(sheet_xml: &str) -> Vec<(String, String)> {
    let cre = Regex::new("(?s)<c r=\"([A-Z]+[0-9]+)\"([^>]*?)(?:/>|>(.*?)</c>)").unwrap();
    let fre = Regex::new("(?s)<f[^>]*>(.*?)</f>").unwrap();
    let vre = Regex::new("(?s)<v>(.*?)</v>").unwrap();
    let tre = Regex::new("(?s)<t[^>]*>(.*?)</t>").unwrap();
    cre.captures_iter(sheet_xml)
        .map(|c| {
            let body = c.get(3).map(|m| m.as_str()).unwrap_or("");
            let attrs = c.get(2).map(|m| m.as_str()).unwrap_or("");
            let value = if let Some(f) = fre.captures(body) {
                format!("={}", unescape(&f[1]))
            } else if let Some(t) = tre.captures(body) {
                unescape(&t[1])
            } else if let Some(v) = vre.captures(body) {
                if attrs.contains("t=\"b\"") {
                    format!("bool:{}", &v[1])
                } else {
                    v[1].to_string()
                }
            } else {
                String::new()
            };
            (c[1].to_string(), value)
        })
        .collect()
}

/// Like [`cells`] for a whole package, with shared strings resolved.
pub fn cells_resolved(bytes: &[u8]) -> Vec<(String, String)> {
    let sst = part(bytes, "xl/sharedStrings.xml");
    let si = Regex::new("(?s)<si>(.*?)</si>").unwrap();
    let t = Regex::new("(?s)<t[^>]*>(.*?)</t>").unwrap();
    let shared: Vec<String> = si
        .captures_iter(&sst)
        .map(|c| {
            t.captures_iter(&c[1])
                .map(|m| unescape(&m[1]))
                .collect::<String>()
        })
        .collect();
    let sheet = part(bytes, "xl/worksheets/sheet1.xml");
    let sre = Regex::new("<c r=\"([A-Z]+[0-9]+)\"[^>]*t=\"s\"[^>]*><v>([0-9]+)</v>").unwrap();
    let mut out = cells(&sheet);
    for c in sre.captures_iter(&sheet) {
        if let Some(cell) = out.iter_mut().find(|(r, _)| r == &c[1]) {
            cell.1 = shared[c[2].parse::<usize>().unwrap()].clone();
        }
    }
    out
}

// --- PPTX -----------------------------------------------------------------

const A_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const P_NS: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";

/// A text shape with one paragraph per entry; `§` splits runs.
pub fn shape(paras: &[&str]) -> String {
    let mut s = String::from("<p:sp><p:nvSpPr><p:cNvPr id=\"2\" name=\"Text\"/><p:cNvSpPr txBox=\"1\"/><p:nvPr/></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/>");
    for para in paras {
        s.push_str("<a:p>");
        for run in para.split('§') {
            s.push_str(&format!(
                "<a:r><a:rPr lang=\"en-US\"/><a:t>{}</a:t></a:r>",
                escape(run)
            ));
        }
        s.push_str("</a:p>");
    }
    s.push_str("</p:txBody></p:sp>");
    s
}

pub fn ptbl(rows: &[&[&str]]) -> String {
    let mut s = String::from("<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id=\"3\" name=\"Table\"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"1\" cy=\"1\"/></p:xfrm><a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/table\"><a:tbl><a:tblGrid>");
    for _ in rows[0] {
        s.push_str("<a:gridCol w=\"2000000\"/>");
    }
    s.push_str("</a:tblGrid>");
    for r in rows {
        s.push_str("<a:tr h=\"370840\">");
        for c in *r {
            s.push_str(&format!(
                "<a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>{}</a:t></a:r></a:p></a:txBody></a:tc>",
                escape(c)
            ));
        }
        s.push_str("</a:tr>");
    }
    s.push_str("</a:tbl></a:graphicData></a:graphic></p:graphicFrame>");
    s
}

/// A PPTX with these slides, each a string of shapes.
pub fn pptx(slides: &[&str]) -> Vec<u8> {
    let mut parts: Vec<(String, Vec<u8>)> = Vec::new();
    let mut overrides = vec![(
        "/ppt/presentation.xml".to_string(),
        "presentationml.presentation.main+xml",
    )];
    let mut pres_rels = Vec::new();
    let mut ids = String::new();
    for (i, body) in slides.iter().enumerate() {
        let n = i + 1;
        let xml = format!(
            "{XML_DECL}<p:sld xmlns:a=\"{A_NS}\" xmlns:r=\"{R_NS}\" xmlns:p=\"{P_NS}\"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{body}</p:spTree></p:cSld></p:sld>"
        );
        parts.push((format!("ppt/slides/slide{n}.xml"), xml.into_bytes()));
        parts.push((
            format!("ppt/slides/_rels/slide{n}.xml.rels"),
            rels(&[("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml")]).into_bytes(),
        ));
        overrides.push((
            format!("/ppt/slides/slide{n}.xml"),
            "presentationml.slide+xml",
        ));
        pres_rels.push((
            format!("rId{}", n + 1),
            "slide",
            format!("slides/slide{n}.xml"),
        ));
        ids.push_str(&format!(
            "<p:sldId id=\"{}\" r:id=\"rId{}\"/>",
            255 + n,
            n + 1
        ));
    }
    let pres = format!(
        "{XML_DECL}<p:presentation xmlns:a=\"{A_NS}\" xmlns:r=\"{R_NS}\" xmlns:p=\"{P_NS}\"><p:sldMasterIdLst><p:sldMasterId id=\"2147483648\" r:id=\"rId1\"/></p:sldMasterIdLst><p:sldIdLst>{ids}</p:sldIdLst><p:sldSz cx=\"9144000\" cy=\"6858000\"/></p:presentation>"
    );
    let mut pr: Vec<(&str, &str, &str)> =
        vec![("rId1", "slideMaster", "slideMasters/slideMaster1.xml")];
    for (id, ty, target) in &pres_rels {
        pr.push((id, ty, target));
    }
    let ov: Vec<(&str, &str)> = overrides.iter().map(|(a, b)| (a.as_str(), *b)).collect();
    let mut all: Vec<(&str, &[u8])> = Vec::new();
    let ct = content_types(&ov);
    let root_rels = rels(&[("rId1", "officeDocument", "ppt/presentation.xml")]);
    let pres_rels_xml = rels(&pr);
    all.push(("[Content_Types].xml", ct.as_bytes()));
    all.push(("_rels/.rels", root_rels.as_bytes()));
    all.push(("ppt/presentation.xml", pres.as_bytes()));
    all.push(("ppt/_rels/presentation.xml.rels", pres_rels_xml.as_bytes()));
    for (name, data) in &parts {
        all.push((name.as_str(), data.as_slice()));
    }
    zip(&all)
}

/// Slide part names in presentation order, from the rendered package.
pub fn slide_order(bytes: &[u8]) -> Vec<String> {
    let pres = part(bytes, "ppt/presentation.xml");
    let rels_xml = part(bytes, "ppt/_rels/presentation.xml.rels");
    let id_re = Regex::new("<p:sldId [^>]*r:id=\"(rId[0-9]+)\"").unwrap();
    let rel_re = Regex::new("Id=\"(rId[0-9]+)\"[^>]*Target=\"([^\"]+)\"").unwrap();
    let rels: Vec<(String, String)> = rel_re
        .captures_iter(&rels_xml)
        .map(|c| (c[1].to_string(), c[2].to_string()))
        .collect();
    id_re
        .captures_iter(&pres)
        .map(|c| {
            let rid = &c[1];
            let target = &rels.iter().find(|(id, _)| id == rid).unwrap().1;
            format!("ppt/{target}")
        })
        .collect()
}

pub fn slide_text(bytes: &[u8], name: &str) -> Vec<String> {
    paragraphs(&part(bytes, name), "a:p", "a:t")
}

// --- ODF ------------------------------------------------------------------

const ODF_NS: &str = "xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\" xmlns:table=\"urn:oasis:names:tc:opendocument:xmlns:table:1.0\" xmlns:draw=\"urn:oasis:names:tc:opendocument:xmlns:drawing:1.0\" xmlns:style=\"urn:oasis:names:tc:opendocument:xmlns:style:1.0\" xmlns:fo=\"urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0\" office:version=\"1.3\"";

fn odf(mime: &str, body_tag: &str, body: &str) -> Vec<u8> {
    let content = format!(
        "{XML_DECL}<office:document-content {ODF_NS}><office:body><{body_tag}>{body}</{body_tag}></office:body></office:document-content>"
    );
    let manifest = format!(
        "{XML_DECL}<manifest:manifest xmlns:manifest=\"urn:oasis:names:tc:opendocument:xmlns:manifest:1.0\" manifest:version=\"1.3\"><manifest:file-entry manifest:full-path=\"/\" manifest:version=\"1.3\" manifest:media-type=\"{mime}\"/><manifest:file-entry manifest:full-path=\"content.xml\" manifest:media-type=\"text/xml\"/><manifest:file-entry manifest:full-path=\"styles.xml\" manifest:media-type=\"text/xml\"/></manifest:manifest>"
    );
    let styles = format!(
        "{XML_DECL}<office:document-styles {ODF_NS}><office:styles/></office:document-styles>"
    );
    zip(&[
        ("mimetype", mime.as_bytes()),
        ("META-INF/manifest.xml", manifest.as_bytes()),
        ("content.xml", content.as_bytes()),
        ("styles.xml", styles.as_bytes()),
    ])
}

pub fn odt(body: &str) -> Vec<u8> {
    odf(
        "application/vnd.oasis.opendocument.text",
        "office:text",
        body,
    )
}

pub fn ods(body: &str) -> Vec<u8> {
    odf(
        "application/vnd.oasis.opendocument.spreadsheet",
        "office:spreadsheet",
        body,
    )
}

pub fn odp(body: &str) -> Vec<u8> {
    odf(
        "application/vnd.oasis.opendocument.presentation",
        "office:presentation",
        body,
    )
}

/// Text of every `text:p` and `text:h`, with spans, `text:s` and tabs
/// flattened.
pub fn odf_paragraphs(xml: &str) -> Vec<String> {
    let pre = Regex::new("(?s)<text:[ph](?: [^>]*)?>(.*?)</text:[ph]>").unwrap();
    let tag = Regex::new("<[^>]+>").unwrap();
    pre.captures_iter(xml)
        .map(|c| {
            let inner = c[1]
                .replace("<text:s/>", " ")
                .replace("<text:tab/>", "\t")
                .replace("<text:line-break/>", "\n");
            unescape(&tag.replace_all(&inner, ""))
        })
        .collect()
}

/// Runs a command, returning stdout, or `None` when it cannot run at all.
pub fn run(cmd: &str, args: &[&str]) -> Option<(bool, String)> {
    let out = std::process::Command::new(cmd).args(args).output().ok()?;
    Some((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr),
    ))
}
