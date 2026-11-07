//! Fonts as data: what is loaded, what a template asks for, and what a PDF
//! used. Mounted fonts live in `RUSTICDOC_FONTS_DIR`; LibreOffice workers
//! see them through a generated fontconfig file.

use std::collections::BTreeSet;
use std::io::Read as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::job::Document;

/// Largest part read when looking for font names.
const MAX_PART: u64 = 32 * 1024 * 1024;
/// Most font names reported for one PDF.
const MAX_NAMES: usize = 64;

/// A loaded font face.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FontInfo {
    pub family: String,
    pub style: String,
    pub weight: u16,
    /// The file's base name, never a full path.
    pub file: String,
    /// True when it came from the mounted fonts directory.
    pub mounted: bool,
    /// The stored font this face belongs to, for deleting it. Absent for fonts
    /// in the image or the mounted directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// Every face fontconfig would find: the system's, the mounted directory's, and
/// those in the copy of the stored fonts, if there is one.
pub fn list(fonts_dir: &Path, stored_dir: Option<&Path>) -> Vec<FontInfo> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    db.load_fonts_dir(fonts_dir);
    if let Some(dir) = stored_dir {
        db.load_fonts_dir(dir);
    }
    let mut out: Vec<FontInfo> = db
        .faces()
        .map(|face| {
            let path = match &face.source {
                fontdb::Source::File(p) => Some(p.clone()),
                _ => None,
            };
            FontInfo {
                family: face
                    .families
                    .first()
                    .map(|(n, _)| n.clone())
                    .unwrap_or_default(),
                style: format!("{:?}", face.style).to_ascii_lowercase(),
                weight: face.weight.0,
                file: path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string(),
                id: path
                    .as_ref()
                    .filter(|p| stored_dir.is_some_and(|dir| p.starts_with(dir)))
                    .and_then(|p| p.file_stem())
                    .and_then(|stem| stem.to_str())
                    .map(str::to_string),
                mounted: path.is_some_and(|p| p.starts_with(fonts_dir)),
            }
        })
        .collect();
    out.sort_by(|a, b| (&a.family, a.weight, &a.style).cmp(&(&b.family, b.weight, &b.style)));
    out.dedup();
    out
}

/// A digest of the fonts directory's file names, sizes and modification
/// times, to tell whether a reload has anything to do. Bounded in depth and
/// in entries, so a huge mount cannot make this slow.
pub fn directory_signature(dir: &Path) -> String {
    use sha2::{Digest, Sha256};
    fn walk(dir: &Path, depth: u32, rows: &mut Vec<String>) {
        if depth > 8 || rows.len() > 20_000 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let path = entry.path();
            if meta.is_dir() {
                walk(&path, depth + 1, rows);
            } else {
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                rows.push(format!("{}|{}|{modified}", path.display(), meta.len()));
            }
        }
    }
    let mut rows = Vec::new();
    walk(dir, 0, &mut rows);
    rows.sort_unstable();
    hex::encode(Sha256::digest(rows.join("\n").as_bytes()))
}

/// One face of an uploaded font, as read from the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FontFace {
    pub family: String,
    pub style: String,
    pub weight: u16,
}

/// A font ready to store: read, parsed, and named by its content.
#[derive(Debug, Clone)]
pub struct ParsedFont {
    /// Hex SHA-256 of the file.
    pub id: String,
    /// The name it was uploaded under, without any directory.
    pub file: String,
    pub bytes: bytes::Bytes,
    pub faces: Vec<FontFace>,
}

/// A file in an upload that was left out, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkippedFile {
    pub file: String,
    pub reason: String,
}

/// What an upload held.
#[derive(Debug, Default)]
pub struct FontUpload {
    pub fonts: Vec<ParsedFont>,
    pub skipped: Vec<SkippedFile>,
}

/// Most entries read from one ZIP of fonts.
const MAX_ZIP_ENTRIES: usize = 2_000;
/// Largest font accepted. The largest common files, CJK collections, are under this.
const MAX_FONT_BYTES: u64 = 128 * 1024 * 1024;
/// Most bytes one ZIP may expand to, however small it is. This is memory.
const MAX_UPLOAD_BYTES: u64 = 256 * 1024 * 1024;

/// The faces a file holds, or `None` when it is not a font that can be read.
pub fn faces_of(bytes: &[u8]) -> Option<Vec<FontFace>> {
    if !is_font(bytes) {
        return None;
    }
    let mut db = fontdb::Database::new();
    db.load_font_data(bytes.to_vec());
    let faces: Vec<FontFace> = db
        .faces()
        .filter_map(|face| {
            let family = face.families.first()?.0.trim().to_string();
            (!family.is_empty()).then(|| FontFace {
                family,
                style: format!("{:?}", face.style).to_ascii_lowercase(),
                weight: face.weight.0,
            })
        })
        .collect();
    (!faces.is_empty()).then_some(faces)
}

fn parse_font(file: &str, bytes: bytes::Bytes) -> Result<ParsedFont> {
    use sha2::{Digest, Sha256};
    let faces = faces_of(&bytes)
        .ok_or_else(|| Error::InvalidRequest(format!("'{file}' is not a font that can be read")))?;
    Ok(ParsedFont {
        id: hex::encode(Sha256::digest(&bytes)),
        file: crate::job::safe_file_name(file, "font"),
        bytes,
        faces,
    })
}

/// True for the extensions fontconfig is given. Others are reported, not guessed at.
fn has_font_extension(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".ttf", ".otf", ".ttc"].iter().any(|e| lower.ends_with(e))
}

/// Reads an upload: one font file, or a ZIP of them. A file named as a font that
/// is not one fails the upload, so a broken package is noticed; anything else in
/// a ZIP, such as a licence, is reported as skipped. Names in the archive are used
/// only as labels, never as paths, and sizes are counted as bytes come out rather
/// than taken from its headers.
pub fn unpack_upload(doc: &Document) -> Result<FontUpload> {
    if doc.bytes.len() as u64 > MAX_FONT_BYTES {
        return Err(Error::InvalidRequest(format!(
            "the upload is larger than {MAX_FONT_BYTES} bytes"
        )));
    }
    if !doc.bytes.starts_with(b"PK") {
        return Ok(FontUpload {
            fonts: vec![parse_font(&doc.name, doc.bytes.clone())?],
            skipped: Vec::new(),
        });
    }
    let invalid =
        |e: zip::result::ZipError| Error::InvalidRequest(format!("the ZIP cannot be read: {e}"));
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(doc.bytes.as_ref())).map_err(invalid)?;
    if archive.len() > MAX_ZIP_ENTRIES {
        return Err(Error::InvalidRequest(format!(
            "the ZIP has more than {MAX_ZIP_ENTRIES} entries"
        )));
    }
    let mut upload = FontUpload::default();
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(invalid)?;
        let path = entry.name().map_err(invalid)?.to_string();
        let base = crate::job::safe_file_name(&path, "file");
        let hidden = base.starts_with('.') || path.split(['/', '\\']).any(|p| p == "__MACOSX");
        if entry.is_dir() || hidden {
            continue;
        }
        let skip = |reason: &str| SkippedFile {
            file: base.clone(),
            reason: reason.to_string(),
        };
        if entry.is_symlink() {
            upload.skipped.push(skip("a link, not a file"));
            continue;
        }
        if !has_font_extension(&base) {
            let lower = base.to_ascii_lowercase();
            let web = [".woff", ".woff2", ".eot", ".pfb", ".pfa"];
            upload
                .skipped
                .push(skip(if web.iter().any(|e| lower.ends_with(e)) {
                    "this font format is not supported; use TTF, OTF or TTC"
                } else {
                    "not a font"
                }));
            continue;
        }
        let mut bytes = Vec::new();
        (&mut entry)
            .take(MAX_FONT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| {
                Error::InvalidRequest(format!("'{base}' cannot be read from the ZIP: {e}"))
            })?;
        if bytes.len() as u64 > MAX_FONT_BYTES {
            return Err(Error::InvalidRequest(format!(
                "'{base}' is larger than {MAX_FONT_BYTES} bytes"
            )));
        }
        total += bytes.len() as u64;
        if total > MAX_UPLOAD_BYTES {
            return Err(Error::InvalidRequest(format!(
                "the ZIP expands to more than {MAX_UPLOAD_BYTES} bytes"
            )));
        }
        let font = parse_font(&base, bytes.into())?;
        if seen.insert(font.id.clone()) {
            upload.fonts.push(font);
        }
    }
    if upload.fonts.is_empty() {
        return Err(Error::InvalidRequest(
            "the ZIP holds no TTF, OTF or TTC fonts".into(),
        ));
    }
    Ok(upload)
}

/// True for a TrueType, OpenType or collection file, by its magic bytes.
pub fn is_font(bytes: &[u8]) -> bool {
    matches!(
        bytes.get(..4),
        Some([0, 1, 0, 0]) | Some(b"OTTO") | Some(b"true") | Some(b"ttcf")
    )
}

/// The font families an Office document names. Empty for other formats or
/// a file that cannot be read: this feeds warnings, never failures.
pub fn referenced(doc: &Document) -> Vec<String> {
    let ext = doc.extension();
    let parts: &[&str] = match ext.as_str() {
        "docx" => &[
            "word/document.xml",
            "word/styles.xml",
            "word/theme/theme1.xml",
        ],
        "xlsx" => &["xl/styles.xml"],
        "pptx" => &["ppt/theme/theme1.xml"],
        "odt" | "ods" | "odp" => &["content.xml", "styles.xml"],
        _ => return Vec::new(),
    };
    let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(doc.bytes.as_ref())) else {
        return Vec::new();
    };
    let mut names = BTreeSet::new();
    for part in parts {
        let Ok(file) = archive.by_name(part) else {
            continue;
        };
        let mut xml = String::new();
        if file.take(MAX_PART).read_to_string(&mut xml).is_err() {
            continue;
        }
        let needles: &[&str] = match ext.as_str() {
            "docx" => &["w:ascii=\"", "w:hAnsi=\"", "<a:latin typeface=\""],
            "xlsx" => &["<name val=\""],
            "pptx" => &["<a:latin typeface=\""],
            _ => &["svg:font-family=\""],
        };
        for needle in needles {
            collect(&xml, needle, &mut names);
        }
    }
    names.into_iter().collect()
}

fn collect(xml: &str, needle: &str, out: &mut BTreeSet<String>) {
    let mut rest = xml;
    while let Some(at) = rest.find(needle) {
        rest = &rest[at + needle.len()..];
        let Some(end) = rest.find('"') else {
            return;
        };
        let name = rest[..end]
            .replace("&apos;", "")
            .replace("&quot;", "")
            .replace("&amp;", "&")
            .replace('\'', "");
        let name = name.trim();
        // `+mn-lt` and friends are references to a theme, not fonts.
        if !name.is_empty() && !name.starts_with('+') {
            out.insert(name.to_string());
        }
        rest = &rest[end..];
    }
}

/// The families in `wanted` that neither a loaded face nor a metric
/// alias in the image's fontconfig can serve.
pub fn missing(wanted: &[String], loaded: &[FontInfo]) -> Vec<String> {
    const ALIASES: &[(&str, &str)] = &[
        ("arial", "liberation sans"),
        ("helvetica", "liberation sans"),
        ("times new roman", "liberation serif"),
        ("times", "liberation serif"),
        ("courier new", "liberation mono"),
        ("courier", "liberation mono"),
        ("calibri", "carlito"),
        ("cambria", "caladea"),
        ("segoe ui", "noto sans"),
    ];
    let have: BTreeSet<String> = loaded.iter().map(|f| f.family.to_lowercase()).collect();
    wanted
        .iter()
        .filter(|name| {
            let lower = name.to_lowercase();
            if have.contains(&lower) {
                return false;
            }
            !ALIASES
                .iter()
                .any(|(from, to)| *from == lower && have.contains(*to))
        })
        .cloned()
        .collect()
}

/// The fonts a PDF names, from `/BaseFont` entries, without subset prefixes.
/// Fonts inside compressed object streams are not seen; this is a
/// diagnostic for pagination differences, not an audit.
pub fn pdf_font_names(pdf: &[u8]) -> Vec<String> {
    const NEEDLE: &[u8] = b"/BaseFont";
    let mut names = BTreeSet::new();
    let mut i = 0;
    while i + NEEDLE.len() < pdf.len() && names.len() < MAX_NAMES {
        if &pdf[i..i + NEEDLE.len()] != NEEDLE {
            i += 1;
            continue;
        }
        i += NEEDLE.len();
        while pdf.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
            i += 1;
        }
        if pdf.get(i) != Some(&b'/') {
            continue;
        }
        i += 1;
        let start = i;
        while pdf
            .get(i)
            .is_some_and(|b| b.is_ascii_graphic() && !b"/<>[]()".contains(b))
        {
            i += 1;
        }
        if let Ok(name) = std::str::from_utf8(&pdf[start..i]) {
            let name = match name.split_once('+') {
                Some((prefix, rest)) if prefix.len() == 6 => rest,
                _ => name,
            };
            if !name.is_empty() {
                names.insert(name.replace("#20", " "));
            }
        }
    }
    names.into_iter().collect()
}

/// Refuses a font that is not one, or too many, or too large.
pub fn check_uploads(fonts: &[(String, Vec<u8>)]) -> Result<()> {
    if fonts.len() > 8 {
        return Err(Error::InvalidRequest("at most 8 fonts per request".into()));
    }
    for (name, bytes) in fonts {
        if bytes.len() > 32 * 1024 * 1024 {
            return Err(Error::InvalidRequest(format!(
                "font '{name}' is over 32 MB"
            )));
        }
        if !is_font(bytes) {
            return Err(Error::InvalidRequest(format!(
                "'{name}' is not a TrueType or OpenType font"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(family: &str) -> FontInfo {
        FontInfo {
            id: None,
            family: family.into(),
            style: "normal".into(),
            weight: 400,
            file: "x.ttf".into(),
            mounted: false,
        }
    }

    #[test]
    fn names_come_out_of_pdfs_without_subset_prefixes() {
        let pdf = b"<</Type/Font/BaseFont/BAAAAA+Liberation#20Mono>> << /BaseFont /Helvetica >> /BaseFont/CCCCCC+Noto";
        assert_eq!(
            pdf_font_names(pdf),
            vec!["Helvetica", "Liberation Mono", "Noto"]
        );
        assert!(pdf_font_names(b"%PDF nothing here").is_empty());
    }

    #[test]
    fn aliases_serve_microsoft_names() {
        let loaded = vec![face("Liberation Sans"), face("Carlito")];
        let wanted = vec![
            "Arial".to_string(),
            "Calibri".into(),
            "Comic Sans MS".into(),
            "carlito".into(),
        ];
        assert_eq!(missing(&wanted, &loaded), vec!["Comic Sans MS"]);
    }

    #[test]
    fn the_directory_signature_follows_its_files() {
        let dir = tempfile::tempdir().unwrap();
        let empty = directory_signature(dir.path());
        assert_eq!(empty, directory_signature(dir.path()));
        std::fs::write(dir.path().join("a.ttf"), b"x").unwrap();
        assert_ne!(empty, directory_signature(dir.path()));
    }

    #[test]
    fn font_uploads_are_checked_by_content() {
        assert!(check_uploads(&[("a.ttf".into(), vec![0, 1, 0, 0, 9])]).is_ok());
        assert!(check_uploads(&[("a.otf".into(), b"OTTO....".to_vec())]).is_ok());
        assert!(check_uploads(&[("evil.ttf".into(), b"<?xml".to_vec())]).is_err());
        let many: Vec<_> = (0..9)
            .map(|i| (format!("{i}.ttf"), vec![0, 1, 0, 0]))
            .collect();
        assert!(check_uploads(&many).is_err());
    }

    #[test]
    fn collect_skips_theme_references_and_decodes_quotes() {
        let mut out = BTreeSet::new();
        collect(
            r#"<a:latin typeface="+mn-lt"/><a:latin typeface="Aptos"/><x svg:font-family="&apos;Liberation Serif&apos;"/>"#,
            "<a:latin typeface=\"",
            &mut out,
        );
        collect(
            r#"<x svg:font-family="&apos;Liberation Serif&apos;"/>"#,
            "svg:font-family=\"",
            &mut out,
        );
        assert_eq!(
            out.into_iter().collect::<Vec<_>>(),
            vec!["Aptos", "Liberation Serif"]
        );
    }
}
