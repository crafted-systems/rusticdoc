//! What rendering takes out of a template: macro projects, OLE objects and
//! ActiveX controls, external links and private hyperlinks.
//!
//! The templates start as LibreOffice made them from the flat ODF sources in
//! `fixtures/` (Basic macros, links into a private network, a linked picture,
//! a linked workbook), and gain what LibreOffice cannot write, as Word, Excel
//! and PowerPoint write it: a VBA project, an OLE object with its preview
//! picture, an ActiveX control and an attached template. The VBA project and
//! OLE storage are stand-ins with the compound-file signature; the sanitiser
//! only looks at where they sit and what points at them.

mod common;

use std::collections::HashSet;

use common::*;
use regex::Regex;
use rusticdoc_template::{compile, sanitize, Format, Keep};
use serde_json::{json, Value};

const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const MS_REL: &str = "http://schemas.microsoft.com/office/2006/relationships";
/// The first bytes of a compound file, which a VBA project and OLE storage are.
const CFB: &[u8] = &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0, 0, 0, 0];

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn data() -> Value {
    json!({
        "name": "Ada",
        "intranet": "http://intranet.local/wiki",
        "rows": [{"month": "Jan", "sales": 3}, {"month": "Feb", "sales": 5}],
    })
}

/// A package being edited, entries kept in order.
struct Pkg(Vec<(String, Vec<u8>)>);

impl Pkg {
    fn open(bytes: &[u8]) -> Pkg {
        Pkg(entries(bytes))
    }

    fn text(&self, name: &str) -> String {
        let (_, d) = self
            .0
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("no {name}"));
        String::from_utf8(d.clone()).unwrap()
    }

    fn has(&self, name: &str) -> bool {
        self.0.iter().any(|(n, _)| n == name)
    }

    fn set(&mut self, name: &str, data: impl Into<Vec<u8>>) {
        let data = data.into();
        match self.0.iter_mut().find(|(n, _)| n == name) {
            Some(e) => e.1 = data,
            None => self.0.push((name.to_string(), data)),
        }
    }

    /// Replaces the first `at` in a part with `with`, which must happen.
    fn edit(&mut self, name: &str, at: &str, with: &str) {
        let text = self.text(name);
        assert!(text.contains(at), "{name} has no {at:?}");
        self.set(name, text.replacen(at, with, 1));
    }

    fn rels(&mut self, name: &str, items: &[(&str, &str, &str, bool)]) {
        let mut s = if self.has(name) {
            self.text(name).replace("</Relationships>", "")
        } else {
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">".to_string()
        };
        for (id, ty, target, external) in items {
            let mode = if *external {
                " TargetMode=\"External\""
            } else {
                ""
            };
            s.push_str(&format!(
                "<Relationship Id=\"{id}\" Type=\"{ty}\" Target=\"{target}\"{mode}/>"
            ));
        }
        s.push_str("</Relationships>");
        self.set(name, s);
    }

    fn types(&mut self, add: &str) {
        self.edit("[Content_Types].xml", "</Types>", &format!("{add}</Types>"));
    }

    fn bytes(&self) -> Vec<u8> {
        let refs: Vec<(&str, &[u8])> = self
            .0
            .iter()
            .map(|(n, d)| (n.as_str(), d.as_slice()))
            .collect();
        zip(&refs)
    }
}

const VML_SHAPETYPE: &str = "<v:shapetype id=\"_x0000_t75\" coordsize=\"21600,21600\" o:spt=\"75\" o:preferrelative=\"t\" path=\"m@4@5l@4@11@9@11@9@5xe\" filled=\"f\" stroked=\"f\"><v:stroke joinstyle=\"miter\"/><v:path o:extrusionok=\"f\" gradientshapeok=\"t\" o:connecttype=\"rect\"/><o:lock v:ext=\"edit\" aspectratio=\"t\"/></v:shapetype>";

/// What Word adds: a VBA project with its data part, an OLE object, an
/// ActiveX check box, and an attached template.
fn word_template() -> Vec<u8> {
    let mut p = Pkg::open(&fixture("sanitize.docx"));
    p.set("word/vbaProject.bin", CFB);
    p.set(
        "word/vbaData.xml",
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<wne:vbaSuppData xmlns:wne=\"http://schemas.microsoft.com/office/word/2006/wordml\"><wne:mcds/></wne:vbaSuppData>",
    );
    p.rels(
        "word/_rels/vbaProject.bin.rels",
        &[(
            "rId1",
            &format!("{MS_REL}/wordVbaData"),
            "vbaData.xml",
            false,
        )],
    );
    p.set("word/embeddings/oleObject1.bin", CFB);
    p.set("word/media/image101.png", png(80, 40));
    p.set("word/media/image102.png", png(60, 15));
    p.set(
        "word/activeX/activeX1.xml",
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n<ax:ocx xmlns:ax=\"http://schemas.microsoft.com/office/2006/activeX\" xmlns:r=\"{REL}\" ax:classid=\"{{8BD21D40-EC42-11CE-9E0D-00AA006002F3}}\" ax:persistence=\"persistStreamInit\" r:id=\"rId1\"/>"),
    );
    p.set("word/activeX/activeX1.bin", CFB);
    p.rels(
        "word/activeX/_rels/activeX1.xml.rels",
        &[(
            "rId1",
            &format!("{MS_REL}/activeXControlBinary"),
            "activeX1.bin",
            false,
        )],
    );
    p.rels(
        "word/_rels/document.xml.rels",
        &[
            (
                "rId101",
                &format!("{MS_REL}/vbaProject"),
                "vbaProject.bin",
                false,
            ),
            (
                "rId102",
                &format!("{REL}/oleObject"),
                "embeddings/oleObject1.bin",
                false,
            ),
            (
                "rId103",
                &format!("{REL}/image"),
                "media/image101.png",
                false,
            ),
            (
                "rId104",
                &format!("{REL}/control"),
                "activeX/activeX1.xml",
                false,
            ),
            (
                "rId105",
                &format!("{REL}/image"),
                "media/image102.png",
                false,
            ),
        ],
    );
    p.edit(
        "word/document.xml",
        "<w:sectPr>",
        &format!(
            "<w:p><w:r><w:t xml:space=\"preserve\">Embedded file: </w:t></w:r><w:r><w:object w:dxaOrig=\"1600\" w:dyaOrig=\"800\">{VML_SHAPETYPE}<v:shape id=\"_x0000_i1025\" type=\"#_x0000_t75\" style=\"width:80pt;height:40pt\" o:ole=\"\"><v:imagedata r:id=\"rId103\" o:title=\"\"/></v:shape><o:OLEObject Type=\"Embed\" ProgID=\"Package\" ShapeID=\"_x0000_i1025\" DrawAspect=\"Content\" ObjectID=\"_1790000001\" r:id=\"rId102\"/></w:object></w:r></w:p>\
             <w:p><w:r><w:t xml:space=\"preserve\">Check box: </w:t></w:r><w:r><w:object w:dxaOrig=\"1200\" w:dyaOrig=\"300\"><v:shape id=\"_x0000_i1026\" type=\"#_x0000_t75\" style=\"width:60pt;height:15pt\"><v:imagedata r:id=\"rId105\" o:title=\"\"/></v:shape><w:control r:id=\"rId104\" w:name=\"CheckBox1\" w:shapeid=\"_x0000_i1026\"/></w:object></w:r></w:p><w:sectPr>"
        ),
    );
    p.edit(
        "word/settings.xml",
        "<w:settings ",
        &format!("<w:settings xmlns:r=\"{REL}\" "),
    );
    p.edit(
        "word/settings.xml",
        "<w:defaultTabStop",
        "<w:attachedTemplate r:id=\"rId1\"/><w:defaultTabStop",
    );
    p.rels(
        "word/_rels/settings.xml.rels",
        &[(
            "rId1",
            &format!("{REL}/attachedTemplate"),
            "file:///C:/Users/ada/Templates/Letter.dotm",
            true,
        )],
    );
    p.edit(
        "[Content_Types].xml",
        "wordprocessingml.document.main+xml",
        "MACRO_MAIN",
    );
    p.edit(
        "[Content_Types].xml",
        "application/vnd.openxmlformats-officedocument.MACRO_MAIN",
        "application/vnd.ms-word.document.macroEnabled.main+xml",
    );
    p.types(
        "<Default Extension=\"bin\" ContentType=\"application/vnd.openxmlformats-officedocument.oleObject\"/>\
         <Override PartName=\"/word/vbaProject.bin\" ContentType=\"application/vnd.ms-office.vbaProject\"/>\
         <Override PartName=\"/word/vbaData.xml\" ContentType=\"application/vnd.ms-word.vbaData+xml\"/>\
         <Override PartName=\"/word/activeX/activeX1.xml\" ContentType=\"application/vnd.ms-office.activeX+xml\"/>\
         <Override PartName=\"/word/activeX/activeX1.bin\" ContentType=\"application/vnd.ms-office.activeX\"/>",
    );
    p.bytes()
}

/// What Excel adds: a VBA project and an OLE object drawn from the sheet's
/// legacy drawing. LibreOffice already wrote the linked workbook.
fn excel_template() -> Vec<u8> {
    let mut p = Pkg::open(&fixture("sanitize.xlsx"));
    p.set("xl/vbaProject.bin", CFB);
    p.rels(
        "xl/_rels/workbook.xml.rels",
        &[(
            "rId101",
            &format!("{MS_REL}/vbaProject"),
            "vbaProject.bin",
            false,
        )],
    );
    p.set("xl/embeddings/oleObject1.bin", CFB);
    p.set("xl/media/image101.png", png(80, 40));
    p.set(
        "xl/drawings/vmlDrawing1.vml",
        "<xml xmlns:v=\"urn:schemas-microsoft-com:vml\" xmlns:o=\"urn:schemas-microsoft-com:office:office\" xmlns:x=\"urn:schemas-microsoft-com:office:excel\"><o:shapelayout v:ext=\"edit\"><o:idmap v:ext=\"edit\" data=\"1\"/></o:shapelayout><v:shapetype id=\"_x0000_t75\" coordsize=\"21600,21600\" o:spt=\"75\" o:preferrelative=\"t\" path=\"m@4@5l@4@11@9@11@9@5xe\" filled=\"f\" stroked=\"f\"><v:stroke joinstyle=\"miter\"/><v:path o:extrusionok=\"f\" gradientshapeok=\"t\" o:connecttype=\"rect\"/><o:lock v:ext=\"edit\" aspectratio=\"t\"/></v:shapetype><v:shape id=\"_x0000_s1025\" type=\"#_x0000_t75\" style=\"position:absolute;margin-left:200pt;margin-top:20pt;width:80pt;height:40pt;z-index:1\"><v:imagedata o:relid=\"rId1\" o:title=\"\"/><x:ClientData ObjectType=\"Pict\"><x:SizeWithCells/><x:Anchor>3, 0, 1, 0, 4, 0, 3, 0</x:Anchor><x:CF>Pict</x:CF><x:AutoPict/></x:ClientData></v:shape></xml>",
    );
    p.rels(
        "xl/drawings/_rels/vmlDrawing1.vml.rels",
        &[(
            "rId1",
            &format!("{REL}/image"),
            "../media/image101.png",
            false,
        )],
    );
    p.rels(
        "xl/worksheets/_rels/sheet1.xml.rels",
        &[
            (
                "rId101",
                &format!("{REL}/oleObject"),
                "../embeddings/oleObject1.bin",
                false,
            ),
            (
                "rId102",
                &format!("{REL}/vmlDrawing"),
                "../drawings/vmlDrawing1.vml",
                false,
            ),
            (
                "rId103",
                &format!("{REL}/image"),
                "../media/image101.png",
                false,
            ),
        ],
    );
    p.edit(
        "xl/worksheets/sheet1.xml",
        "</worksheet>",
        "<legacyDrawing r:id=\"rId102\"/><oleObjects><mc:AlternateContent><mc:Choice Requires=\"x14\"><oleObject progId=\"Package\" shapeId=\"1025\" r:id=\"rId101\"><objectPr defaultSize=\"0\" r:id=\"rId103\"><anchor moveWithCells=\"1\"><from><xdr:col>3</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></from><to><xdr:col>4</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>3</xdr:row><xdr:rowOff>0</xdr:rowOff></to></anchor></objectPr></oleObject></mc:Choice><mc:Fallback><oleObject progId=\"Package\" shapeId=\"1025\" r:id=\"rId101\"/></mc:Fallback></mc:AlternateContent></oleObjects></worksheet>",
    );
    p.edit(
        "[Content_Types].xml",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml",
        "application/vnd.ms-excel.sheet.macroEnabled.main+xml",
    );
    p.types(
        "<Default Extension=\"bin\" ContentType=\"application/vnd.ms-office.vbaProject\"/>\
         <Default Extension=\"vml\" ContentType=\"application/vnd.openxmlformats-officedocument.vmlDrawing\"/>\
         <Override PartName=\"/xl/embeddings/oleObject1.bin\" ContentType=\"application/vnd.openxmlformats-officedocument.oleObject\"/>",
    );
    p.bytes()
}

/// What PowerPoint adds: a VBA project, an OLE object with its preview, an
/// ActiveX check box and a linked picture.
fn powerpoint_template() -> Vec<u8> {
    let mut p = Pkg::open(&fixture("sanitize.pptx"));
    p.set("ppt/vbaProject.bin", CFB);
    p.rels(
        "ppt/_rels/presentation.xml.rels",
        &[(
            "rId101",
            &format!("{MS_REL}/vbaProject"),
            "vbaProject.bin",
            false,
        )],
    );
    p.set("ppt/embeddings/oleObject1.bin", CFB);
    p.set("ppt/media/image101.png", png(80, 40));
    p.set("ppt/media/image102.png", png(60, 15));
    p.set(
        "ppt/activeX/activeX1.xml",
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n<ax:ocx xmlns:ax=\"http://schemas.microsoft.com/office/2006/activeX\" xmlns:r=\"{REL}\" ax:classid=\"{{8BD21D40-EC42-11CE-9E0D-00AA006002F3}}\" ax:persistence=\"persistStreamInit\" r:id=\"rId1\"/>"),
    );
    p.set("ppt/activeX/activeX1.bin", CFB);
    p.rels(
        "ppt/activeX/_rels/activeX1.xml.rels",
        &[(
            "rId1",
            &format!("{MS_REL}/activeXControlBinary"),
            "activeX1.bin",
            false,
        )],
    );
    p.rels(
        "ppt/slides/_rels/slide1.xml.rels",
        &[
            (
                "rId101",
                &format!("{REL}/oleObject"),
                "../embeddings/oleObject1.bin",
                false,
            ),
            (
                "rId102",
                &format!("{REL}/image"),
                "../media/image101.png",
                false,
            ),
            (
                "rId103",
                &format!("{REL}/control"),
                "../activeX/activeX1.xml",
                false,
            ),
            (
                "rId104",
                &format!("{REL}/image"),
                "../media/image102.png",
                false,
            ),
            (
                "rId105",
                &format!("{REL}/image"),
                "https://images.example.com/logo.png",
                true,
            ),
        ],
    );
    let pic = |id: u32, name: &str, rid: &str, attr: &str, y: u32, cx: u32, cy: u32| {
        format!("<p:pic><p:nvPicPr><p:cNvPr id=\"{id}\" name=\"{name}\"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip {attr}=\"{rid}\"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x=\"6000000\" y=\"{y}\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr></p:pic>")
    };
    let ole = format!(
        "<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id=\"50\" name=\"Object 49\"/><p:cNvGraphicFramePr><a:graphicFrameLocks noChangeAspect=\"1\"/></p:cNvGraphicFramePr><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x=\"6000000\" y=\"3000000\"/><a:ext cx=\"1828800\" cy=\"914400\"/></p:xfrm><a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/presentationml/2006/ole\"><mc:AlternateContent><mc:Choice xmlns:v=\"urn:schemas-microsoft-com:vml\" Requires=\"v\"><p:oleObj spid=\"_x0000_s1026\" name=\"Packager Shell Object\" r:id=\"rId101\" imgW=\"1828800\" imgH=\"914400\" progId=\"Package\"><p:embed/></p:oleObj></mc:Choice><mc:Fallback><p:oleObj name=\"Packager Shell Object\" r:id=\"rId101\" imgW=\"1828800\" imgH=\"914400\" progId=\"Package\"><p:embed/>{}</p:oleObj></mc:Fallback></mc:AlternateContent></a:graphicData></a:graphic></p:graphicFrame>",
        pic(0, "", "rId102", "r:embed", 3000000, 1828800, 914400)
    );
    let linked = pic(
        52,
        "Linked picture",
        "rId105",
        "r:link",
        4500000,
        914400,
        457200,
    );
    p.edit(
        "ppt/slides/slide1.xml",
        "</p:spTree>",
        &format!("{ole}{linked}</p:spTree><p:controls><mc:AlternateContent><mc:Choice xmlns:v=\"urn:schemas-microsoft-com:vml\" Requires=\"v\"><p:control spid=\"_x0000_s1027\" name=\"CheckBox1\" r:id=\"rId103\" imgW=\"914400\" imgH=\"228600\"/></mc:Choice><mc:Fallback><p:control name=\"CheckBox1\" r:id=\"rId103\" imgW=\"914400\" imgH=\"228600\">{}</p:control></mc:Fallback></mc:AlternateContent></p:controls>",
            pic(51, "CheckBox1", "rId104", "r:embed", 5500000, 914400, 228600)),
    );
    p.edit(
        "[Content_Types].xml",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml",
        "application/vnd.ms-powerpoint.presentation.macroEnabled.main+xml",
    );
    p.types(
        "<Default Extension=\"bin\" ContentType=\"application/vnd.ms-office.vbaProject\"/>\
         <Override PartName=\"/ppt/embeddings/oleObject1.bin\" ContentType=\"application/vnd.openxmlformats-officedocument.oleObject\"/>\
         <Override PartName=\"/ppt/activeX/activeX1.xml\" ContentType=\"application/vnd.ms-office.activeX+xml\"/>\
         <Override PartName=\"/ppt/activeX/activeX1.bin\" ContentType=\"application/vnd.ms-office.activeX\"/>",
    );
    p.bytes()
}

/// What a LibreOffice document holding foreign objects carries: an OLE
/// object (`draw:object-ole` and its storage) and an embedded spreadsheet,
/// each with the replacement picture LibreOffice shows.
fn libreoffice_template(fixture_name: &str, format: Format) -> Vec<u8> {
    let mut p = Pkg::open(&fixture(fixture_name));
    p.set("Object 9", CFB);
    p.set("ObjectReplacements/Object 9", png(80, 40));
    p.set(
        "Object 10/content.xml",
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<office:document-content xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" xmlns:table=\"urn:oasis:names:tc:opendocument:xmlns:table:1.0\" xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\" office:version=\"1.3\"><office:body><office:spreadsheet><table:table table:name=\"Inner\"><table:table-row><table:table-cell office:value-type=\"float\" office:value=\"42\"><text:p>42</text:p></table:table-cell></table:table-row></table:table></office:spreadsheet></office:body></office:document-content>",
    );
    p.set("ObjectReplacements/Object 10", png(80, 40));
    p.edit(
        "META-INF/manifest.xml",
        "</manifest:manifest>",
        " <manifest:file-entry manifest:full-path=\"Object 9\" manifest:media-type=\"application/vnd.sun.star.oleobject\"/>\n <manifest:file-entry manifest:full-path=\"ObjectReplacements/Object 9\" manifest:media-type=\"image/png\"/>\n <manifest:file-entry manifest:full-path=\"Object 10/content.xml\" manifest:media-type=\"text/xml\"/>\n <manifest:file-entry manifest:full-path=\"Object 10/\" manifest:media-type=\"application/vnd.oasis.opendocument.spreadsheet\"/>\n <manifest:file-entry manifest:full-path=\"ObjectReplacements/Object 10\" manifest:media-type=\"image/png\"/>\n</manifest:manifest>",
    );
    let frame = |n: u32, element: &str, place: &str| {
        format!("<draw:frame draw:name=\"Foreign {n}\" {place} svg:width=\"2.8cm\" svg:height=\"1.4cm\"><{element} xlink:href=\"./Object {n}\" xlink:type=\"simple\" xlink:show=\"embed\" xlink:actuate=\"onLoad\"/><draw:image xlink:href=\"./ObjectReplacements/Object {n}\" xlink:type=\"simple\" xlink:show=\"embed\" xlink:actuate=\"onLoad\"/></draw:frame>")
    };
    match format {
        Format::Odt => {
            let place = "text:anchor-type=\"as-char\"";
            p.edit(
                "content.xml",
                "</office:text>",
                &format!(
                    "<text:p>Foreign objects: {} and {} end.</text:p></office:text>",
                    frame(9, "draw:object-ole", place),
                    frame(10, "draw:object", place)
                ),
            );
        }
        Format::Ods => {
            let text = p.text("content.xml");
            let cell = Regex::new("<table:table-cell [^>]*>").unwrap();
            let at = cell.find(&text).unwrap().end();
            let place = "svg:x=\"0cm\" svg:y=\"0cm\"";
            let frames = format!(
                "{}{}",
                frame(9, "draw:object-ole", place),
                frame(10, "draw:object", "svg:x=\"3cm\" svg:y=\"0cm\"")
            );
            p.set(
                "content.xml",
                format!("{}{frames}{}", &text[..at], &text[at..]),
            );
        }
        _ => {
            let text = p.text("content.xml");
            let at = text
                .find("<presentation:notes")
                .unwrap_or_else(|| text.find("</draw:page>").unwrap());
            let frames = format!(
                "{}{}",
                frame(9, "draw:object-ole", "svg:x=\"14cm\" svg:y=\"12cm\""),
                frame(10, "draw:object", "svg:x=\"18cm\" svg:y=\"12cm\"")
            );
            p.set(
                "content.xml",
                format!("{}{frames}{}", &text[..at], &text[at..]),
            );
        }
    }
    p.bytes()
}

fn render_keeping(format: Format, template: &[u8], keep: Keep) -> Vec<u8> {
    compile(format, template)
        .expect("compiles")
        .render_with(&data(), keep)
        .expect("renders")
}

fn names(bytes: &[u8]) -> Vec<String> {
    entries(bytes).into_iter().map(|(n, _)| n).collect()
}

fn text_of(xml: &str) -> String {
    Regex::new("<[^>]+>")
        .unwrap()
        .replace_all(xml, "")
        .into_owned()
}

/// Every relationship lands on a part, every relationship id a part uses
/// exists, and every part has a content type, with none for missing parts.
fn assert_ooxml_consistent(bytes: &[u8]) {
    let all = entries(bytes);
    let have: HashSet<&str> = all.iter().map(|(n, _)| n.as_str()).collect();
    let rel_re = Regex::new("<Relationship [^>]*>").unwrap();
    let attr = |tag: &str, name: &str| {
        Regex::new(&format!(" {name}=\"([^\"]*)\""))
            .unwrap()
            .captures(tag)
            .map(|c| c[1].to_string())
    };
    let types = part(bytes, "[Content_Types].xml");
    let overrides: Vec<String> = Regex::new("PartName=\"/([^\"]+)\"")
        .unwrap()
        .captures_iter(&types)
        .map(|c| c[1].to_string())
        .collect();
    for o in &overrides {
        assert!(have.contains(o.as_str()), "an override names missing {o}");
    }
    let defaults: Vec<String> = Regex::new("Extension=\"([^\"]+)\"")
        .unwrap()
        .captures_iter(&types)
        .map(|c| c[1].to_ascii_lowercase())
        .collect();
    let id_re = Regex::new(r#"\br:(?:id|embed|link|pict)="([^"]+)""#).unwrap();
    for (name, data) in &all {
        if name.ends_with('/') || name == "[Content_Types].xml" {
            continue;
        }
        let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        assert!(
            overrides.contains(name) || defaults.contains(&ext),
            "{name} has no content type"
        );
        if !name.ends_with(".rels") {
            continue;
        }
        let rels = String::from_utf8(data.clone()).unwrap();
        let source = name
            .replace("_rels/", "")
            .trim_end_matches(".rels")
            .to_string();
        let base = source.rsplit_once('/').map_or("", |(d, _)| d).to_string();
        let mut ids = HashSet::new();
        for tag in rel_re.find_iter(&rels).map(|m| m.as_str()) {
            ids.insert(attr(tag, "Id").unwrap());
            if attr(tag, "TargetMode").as_deref() == Some("External") {
                continue;
            }
            let target = unescape(&attr(tag, "Target").unwrap());
            let mut segs: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
            for s in target.split('/') {
                match s {
                    "" | "." => {}
                    ".." => {
                        segs.pop();
                    }
                    s => segs.push(s),
                }
            }
            let resolved = segs.join("/");
            assert!(
                have.contains(resolved.as_str()),
                "{name} points at missing {resolved}"
            );
        }
        if let Some((_, src)) = all.iter().find(|(n, _)| *n == source) {
            let src = String::from_utf8_lossy(src);
            for c in id_re.captures_iter(&src) {
                assert!(ids.contains(&c[1]), "{source} names missing {}", &c[1]);
            }
        }
    }
}

/// Every manifest entry exists and every file is listed; internal links in
/// the content land on something.
fn assert_odf_consistent(bytes: &[u8]) {
    let all = names(bytes);
    let manifest = part(bytes, "META-INF/manifest.xml");
    let listed: Vec<String> = Regex::new("manifest:full-path=\"([^\"]+)\"")
        .unwrap()
        .captures_iter(&manifest)
        .map(|c| unescape(&c[1]))
        .collect();
    for path in &listed {
        // LibreOffice lists its empty settings folder, which the package
        // reader never keeps as an entry, before rendering or after.
        if path == "/" || path == "Configurations2/" {
            continue;
        }
        let found = if path.ends_with('/') {
            all.iter().any(|n| n.starts_with(path.as_str()))
        } else {
            all.contains(path)
        };
        assert!(found, "the manifest lists missing {path}");
    }
    for name in &all {
        if name == "mimetype" || name.starts_with("META-INF/") || name.ends_with('/') {
            continue;
        }
        assert!(listed.contains(name), "{name} is not in the manifest");
    }
    let content = part(bytes, "content.xml");
    for c in Regex::new(r#"xlink:href="(\./)?([^":#]+)""#)
        .unwrap()
        .captures_iter(&content)
    {
        let path = unescape(&c[2]);
        assert!(
            all.iter()
                .any(|n| *n == path || n.starts_with(&format!("{path}/"))),
            "content.xml links to missing {path}"
        );
    }
}

fn hyperlink_targets(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let link_re = Regex::new("<Relationship [^>]*Type=\"[^\"]*/hyperlink\"[^>]*>").unwrap();
    let target_re = Regex::new("Target=\"([^\"]*)\"").unwrap();
    for (name, data) in entries(bytes) {
        if name.ends_with(".rels") {
            let rels = String::from_utf8(data).unwrap();
            for c in link_re.find_iter(&rels) {
                let t = target_re.captures(c.as_str()).unwrap()[1].to_string();
                out.push(unescape(&t));
            }
        }
    }
    out
}

fn external_rels(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let external_re = Regex::new("<Relationship [^>]*TargetMode=\"External\"[^>]*>").unwrap();
    for (name, data) in entries(bytes) {
        if name.ends_with(".rels") {
            let rels = String::from_utf8(data).unwrap();
            for m in external_re.find_iter(&rels) {
                if !m.as_str().contains("/hyperlink\"") {
                    out.push(format!("{name}: {}", m.as_str()));
                }
            }
        }
    }
    out
}

#[test]
fn word_output_is_a_plain_docx_without_macros_objects_or_outside_links() {
    let template = word_template();
    assert_ooxml_consistent(&template);
    let out = render_keeping(Format::Docx, &template, Keep::default());
    assert_ooxml_consistent(&out);
    let all = names(&out);
    for gone in [
        "word/vbaProject.bin",
        "word/_rels/vbaProject.bin.rels",
        "word/vbaData.xml",
        "word/embeddings/oleObject1.bin",
        "word/activeX/activeX1.xml",
        "word/activeX/activeX1.bin",
        "word/activeX/_rels/activeX1.xml.rels",
    ] {
        assert!(!all.iter().any(|n| n == gone), "{gone} stayed");
    }
    // The previews stay, and show where the objects were.
    assert!(all.iter().any(|n| n == "word/media/image101.png"));
    assert!(all.iter().any(|n| n == "word/media/image102.png"));
    let types = part(&out, "[Content_Types].xml");
    assert!(!types.contains("macroEnabled"), "{types}");
    assert!(
        types.contains("wordprocessingml.document.main+xml"),
        "{types}"
    );
    let doc = part(&out, "word/document.xml");
    assert!(
        !doc.contains("o:OLEObject") && !doc.contains("w:control"),
        "{doc}"
    );
    assert!(!doc.contains("<w:object"), "{doc}");
    assert_eq!(doc.matches("<w:pict>").count(), 2, "{doc}");
    assert!(doc.contains("<v:imagedata r:id=\"rId103\""), "{doc}");
    // The linked picture keeps its place without its link.
    assert!(!doc.contains("r:link"), "{doc}");
    assert!(external_rels(&out).is_empty(), "{:?}", external_rels(&out));
    assert!(!part(&out, "word/settings.xml").contains("attachedTemplate"));
    // Private links and the macro link lose the link, never the words.
    let links = hyperlink_targets(&out);
    assert_eq!(links, ["https://example.com/"], "{links:?}");
    let text = text_of(&doc);
    for words in [
        "Letter for Ada",
        "website",
        "router page",
        "intranet wiki",
        "Macro button",
    ] {
        assert!(text.contains(words), "no {words:?} in {text}");
    }
}

#[test]
fn excel_output_is_a_plain_xlsx_without_macros_objects_or_outside_links() {
    let template = excel_template();
    assert_ooxml_consistent(&template);
    let out = render_keeping(Format::Xlsx, &template, Keep::default());
    assert_ooxml_consistent(&out);
    let all = names(&out);
    for gone in [
        "xl/vbaProject.bin",
        "xl/embeddings/oleObject1.bin",
        "xl/externalLinks/externalLink1.xml",
        "xl/externalLinks/_rels/externalLink1.xml.rels",
    ] {
        assert!(!all.iter().any(|n| n == gone), "{gone} stayed");
    }
    // The sheet's legacy drawing keeps drawing the object's picture.
    assert!(all.iter().any(|n| n == "xl/drawings/vmlDrawing1.vml"));
    assert!(all.iter().any(|n| n == "xl/media/image101.png"));
    let types = part(&out, "[Content_Types].xml");
    assert!(
        !types.contains("macroEnabled") && !types.contains("vbaProject"),
        "{types}"
    );
    assert!(types.contains("spreadsheetml.sheet.main+xml"), "{types}");
    let sheet = part(&out, "xl/worksheets/sheet1.xml");
    assert!(!sheet.contains("oleObject"), "{sheet}");
    assert!(
        sheet.contains("<legacyDrawing r:id=\"rId102\"/>"),
        "{sheet}"
    );
    // The formula that read the linked workbook goes. Rendering already
    // dropped its cached value, so the cell is left empty.
    assert!(!sheet.contains("[1]"), "{sheet}");
    assert!(sheet.contains("<c r=\"A3\" s=\"0\"></c>"), "{sheet}");
    let workbook = part(&out, "xl/workbook.xml");
    assert!(!workbook.contains("externalReference"), "{workbook}");
    assert!(external_rels(&out).is_empty(), "{:?}", external_rels(&out));
    assert_eq!(hyperlink_targets(&out), ["https://example.com/"]);
    assert_eq!(sheet.matches("<hyperlink ").count(), 1, "{sheet}");
    let strings = text_of(&part(&out, "xl/sharedStrings.xml"));
    assert!(strings.contains("Payroll server"), "{strings}");
    assert!(
        text_of(&sheet).contains("Intranet wiki") || strings.contains("Intranet wiki"),
        "{sheet}"
    );
}

#[test]
fn powerpoint_output_is_a_plain_pptx_without_macros_objects_or_outside_links() {
    let template = powerpoint_template();
    assert_ooxml_consistent(&template);
    let out = render_keeping(Format::Pptx, &template, Keep::default());
    assert_ooxml_consistent(&out);
    let all = names(&out);
    for gone in [
        "ppt/vbaProject.bin",
        "ppt/embeddings/oleObject1.bin",
        "ppt/activeX/activeX1.xml",
        "ppt/activeX/activeX1.bin",
    ] {
        assert!(!all.iter().any(|n| n == gone), "{gone} stayed");
    }
    let types = part(&out, "[Content_Types].xml");
    assert!(
        !types.contains("macroEnabled") && !types.contains("vbaProject"),
        "{types}"
    );
    let slide = part(&out, "ppt/slides/slide1.xml");
    assert!(
        !slide.contains("p:oleObj") && !slide.contains("p:controls"),
        "{slide}"
    );
    // The object became its preview, at the frame's place and with its name.
    assert!(
        slide.contains("<p:cNvPr id=\"50\" name=\"Object 49\"/>"),
        "{slide}"
    );
    assert!(slide.contains("<a:blip r:embed=\"rId102\"/>"), "{slide}");
    assert!(
        slide.contains("<a:xfrm><a:off x=\"6000000\" y=\"3000000\"/><a:ext cx=\"1828800\" cy=\"914400\"/></a:xfrm>"),
        "{slide}"
    );
    // The check box's picture joined the shapes.
    assert!(slide.contains("<a:blip r:embed=\"rId104\"/>"), "{slide}");
    let tree_end = slide.find("</p:spTree>").unwrap();
    assert!(slide.find("rId104").unwrap() < tree_end, "{slide}");
    assert!(!slide.contains("r:link"), "{slide}");
    assert!(external_rels(&out).is_empty(), "{:?}", external_rels(&out));
    assert_eq!(hyperlink_targets(&out), ["https://example.com/"]);
    let text = text_of(&slide);
    for words in ["Deck for Ada", "website", "printer page", "intranet wiki"] {
        assert!(text.contains(words), "no {words:?} in {text}");
    }
}

fn assert_odf_clean(format: Format, out: &[u8], words: &[&str]) {
    assert_odf_consistent(out);
    let all = names(out);
    assert!(!all.iter().any(|n| n.starts_with("Basic/")), "{all:?}");
    assert!(
        !all.iter().any(|n| n == "Object 9"),
        "the OLE storage stayed"
    );
    assert!(
        !all.iter().any(|n| n.starts_with("Object 10/")),
        "the embedded sheet stayed"
    );
    assert!(all.iter().any(|n| n == "ObjectReplacements/Object 9"));
    let content = part(out, "content.xml");
    for gone in [
        "office:scripts",
        "script:event-listener",
        "vnd.sun.star.script",
        "draw:object-ole",
        "./Object 10\"",
        "192.168.1.10",
        "10.0.0.5",
        "printer.local",
        "intranet.local",
        "images.example.com",
    ] {
        assert!(
            !content.contains(gone),
            "{format:?}: {gone} stayed in {content}"
        );
    }
    assert!(
        content.contains("xlink:href=\"https://example.com/\""),
        "{content}"
    );
    // Each object leaves its replacement picture behind.
    assert!(
        content.contains("xlink:href=\"./ObjectReplacements/Object 9\""),
        "{content}"
    );
    assert!(
        content.contains("xlink:href=\"./ObjectReplacements/Object 10\""),
        "{content}"
    );
    let text = text_of(&content);
    for w in words {
        assert!(text.contains(w), "{format:?}: no {w:?} in {text}");
    }
}

#[test]
fn libreoffice_outputs_lose_basic_objects_and_outside_links() {
    let odt = libreoffice_template("sanitize.odt", Format::Odt);
    assert_odf_consistent(&odt);
    let out = render_keeping(Format::Odt, &odt, Keep::default());
    assert_odf_clean(
        Format::Odt,
        &out,
        &[
            "Letter for Ada",
            "router page",
            "intranet wiki",
            "Macro button",
        ],
    );
    // The chart is content, not a foreign object, and stays.
    assert!(names(&out).iter().any(|n| n == "Object 1/content.xml"));
    assert!(part(&out, "content.xml").contains("xlink:href=\"./Object 1\""));

    let ods = libreoffice_template("sanitize.ods", Format::Ods);
    let out = render_keeping(Format::Ods, &ods, Keep::default());
    assert_odf_clean(
        Format::Ods,
        &out,
        &["Report for Ada", "Payroll server", "Intranet wiki"],
    );

    let odp = libreoffice_template("sanitize.odp", Format::Odp);
    let out = render_keeping(Format::Odp, &odp, Keep::default());
    assert_odf_clean(
        Format::Odp,
        &out,
        &["Deck for Ada", "printer page", "intranet wiki"],
    );
}

#[test]
fn a_request_can_keep_each_kind() {
    let template = word_template();
    let keep = |k: Keep| render_keeping(Format::Docx, &template, k);
    let macros = keep(Keep {
        macros: true,
        ..Keep::default()
    });
    assert!(names(&macros).iter().any(|n| n == "word/vbaProject.bin"));
    assert!(names(&macros).iter().any(|n| n == "word/vbaData.xml"));
    assert!(part(&macros, "[Content_Types].xml").contains("macroEnabled"));
    assert!(!names(&macros).iter().any(|n| n.contains("oleObject")));
    assert_ooxml_consistent(&macros);

    let objects = keep(Keep {
        objects: true,
        ..Keep::default()
    });
    assert!(names(&objects)
        .iter()
        .any(|n| n == "word/embeddings/oleObject1.bin"));
    assert!(names(&objects)
        .iter()
        .any(|n| n == "word/activeX/activeX1.bin"));
    assert!(part(&objects, "word/document.xml").contains("o:OLEObject"));
    assert!(!names(&objects).iter().any(|n| n.contains("vbaProject")));
    assert_ooxml_consistent(&objects);

    let external = keep(Keep {
        external_links: true,
        ..Keep::default()
    });
    assert!(part(&external, "word/settings.xml").contains("attachedTemplate"));
    assert!(part(&external, "word/document.xml").contains("r:link=\"rId5\""));
    assert_ooxml_consistent(&external);

    let private = keep(Keep {
        private_links: true,
        ..Keep::default()
    });
    let links = hyperlink_targets(&private);
    assert!(
        links.iter().any(|l| l.contains("192.168.1.10")),
        "{links:?}"
    );
    assert!(
        links.iter().any(|l| l.contains("intranet.local")),
        "{links:?}"
    );
    assert_ooxml_consistent(&private);

    // Keeping everything changes nothing the template brought.
    let all = keep(Keep::ALL);
    for name in names(&template) {
        // Rendering fills in the body and adds the data's link and the chart.
        if name == "word/document.xml"
            || name == "word/_rels/document.xml.rels"
            || name == "[Content_Types].xml"
        {
            continue;
        }
        let text = |b: &[u8]| String::from_utf8_lossy(&bytes_of(b, &name).unwrap()).into_owned();
        assert_eq!(text(&all), text(&template), "{name}");
    }
    let types = part(&all, "[Content_Types].xml");
    assert!(
        types.contains("macroEnabled") && types.contains("/word/vbaProject.bin"),
        "{types}"
    );
    assert_ooxml_consistent(&all);

    let odt = libreoffice_template("sanitize.odt", Format::Odt);
    let kept = render_keeping(Format::Odt, &odt, Keep::ALL);
    assert!(names(&kept).iter().any(|n| n.starts_with("Basic/")));
    assert!(part(&kept, "content.xml").contains("draw:object-ole"));
    let macros = render_keeping(
        Format::Odt,
        &odt,
        Keep {
            macros: true,
            ..Keep::default()
        },
    );
    assert!(names(&macros).iter().any(|n| n.starts_with("Basic/")));
    assert!(part(&macros, "content.xml").contains("office:scripts"));
    assert!(!part(&macros, "content.xml").contains("draw:object-ole"));
    assert_odf_consistent(&macros);
}

#[test]
fn any_package_can_be_sanitised_and_reports_what_went() {
    let (out, removed) = sanitize(Format::Pptx, &powerpoint_template(), Keep::default()).unwrap();
    assert_ooxml_consistent(&out);
    // The VBA project, the OLE object and the control with its binary, the
    // linked picture, and the private link.
    assert_eq!(removed.macros, 1, "{removed:?}");
    assert_eq!(removed.objects, 2, "{removed:?}");
    assert_eq!(removed.external_links, 1, "{removed:?}");
    assert_eq!(removed.private_links, 1, "{removed:?}");
    let (_, nothing) = sanitize(Format::Pptx, &out, Keep::default()).unwrap();
    assert!(
        nothing.is_empty(),
        "a second pass finds nothing: {nothing:?}"
    );
    let (same, none) = sanitize(Format::Docx, &word_template(), Keep::ALL).unwrap();
    assert!(none.is_empty());
    assert_eq!(names(&same), names(&word_template()));
}

#[test]
fn a_simple_hyperlink_field_to_a_private_address_keeps_its_text() {
    let doc = docx(&format!(
        "{}<w:p><w:fldSimple w:instr=\" HYPERLINK &quot;http://127.0.0.1:8080/&quot; \"><w:r><w:t>local app</w:t></w:r></w:fldSimple></w:p><w:p><w:fldSimple w:instr=\" HYPERLINK &quot;https://example.com/&quot; \"><w:r><w:t>public</w:t></w:r></w:fldSimple></w:p>",
        p("x")
    ));
    let out = render_keeping(Format::Docx, &doc, Keep::default());
    let xml = part(&out, "word/document.xml");
    assert_eq!(xml.matches("<w:fldSimple").count(), 1, "{xml}");
    assert!(
        xml.contains("example.com") && xml.contains("local app"),
        "{xml}"
    );
}

/// The output file, its format, the template, words the PDF must show, and
/// the fewest pictures it must hold.
type Case = (
    &'static str,
    Format,
    Vec<u8>,
    &'static [&'static str],
    usize,
);

/// Converts every sanitised output to PDF with LibreOffice: each must open
/// and show its text, its previews and its chart. Skips without the images.
#[test]
fn libreoffice_opens_every_sanitised_output() {
    if !libreoffice_available() {
        eprintln!("skipping: docker images not available");
        return;
    }
    let keep = std::env::var("RICH_KEEP").ok();
    let temp = tempfile::tempdir().unwrap();
    let dir = keep.as_deref().unwrap_or(temp.path().to_str().unwrap());
    let cases: [Case; 6] = [
        (
            "clean.docx",
            Format::Docx,
            word_template(),
            &["Letter for Ada", "router page", "Embedded file"],
            2,
        ),
        (
            "clean.xlsx",
            Format::Xlsx,
            excel_template(),
            &["Report for Ada", "Payroll server"],
            0,
        ),
        (
            "clean.pptx",
            Format::Pptx,
            powerpoint_template(),
            &["Deck for Ada", "printer page"],
            2,
        ),
        (
            "clean.odt",
            Format::Odt,
            libreoffice_template("sanitize.odt", Format::Odt),
            &["Letter for Ada", "router page", "Chart stays", "Feb"],
            2,
        ),
        (
            "clean.ods",
            Format::Ods,
            libreoffice_template("sanitize.ods", Format::Ods),
            &["Report for Ada", "Payroll server"],
            2,
        ),
        (
            "clean.odp",
            Format::Odp,
            libreoffice_template("sanitize.odp", Format::Odp),
            &["Deck for Ada", "printer page"],
            2,
        ),
    ];
    for (file, format, template, words, images) in cases {
        let out = render_keeping(format, &template, Keep::default());
        std::fs::write(format!("{dir}/{file}"), &out).unwrap();
        assert!(soffice(dir, "pdf", file), "{file}: conversion failed");
        let pdf = file.replace('.', "_") + ".pdf";
        std::fs::rename(format!("{dir}/clean.pdf"), format!("{dir}/{pdf}")).unwrap();
        let text = pdf_text(dir, &pdf);
        for w in words {
            assert!(text.contains(w), "{file}: no {w:?} in {text}");
        }
        let found = pdf_images(dir, &pdf).len();
        assert!(found >= images, "{file}: {found} images");
        let links = pdf_links(dir, &pdf);
        assert!(
            links.iter().all(|l| !l.contains("192.168")
                && !l.contains("10.0.0.5")
                && !l.contains(".local")),
            "{file}: {links:?}"
        );
    }
}
