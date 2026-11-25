//! Assets as data: images and stylesheets a service holds for the templates and
//! pages that name them. Mounted ones live in `RUSTICDOC_ASSETS_DIR`, and
//! uploaded ones in the store. `rusticdoc_template` reads them, where they are
//! used; this unpacks uploads.

use std::collections::BTreeSet;
use std::io::Read as _;

pub use rusticdoc_template::assets::{
    AssetDirs, AssetKind, MAX_ASSET_BYTES, MountedAsset, SentAssets, check_content, check_name,
    kind_of,
};

use crate::error::{Error, Result};
use crate::fonts::SkippedFile;
use crate::job::Document;

/// Most entries read from one ZIP of assets.
const MAX_ZIP_ENTRIES: usize = 2_000;
/// Largest ZIP accepted.
const MAX_ZIP_BYTES: u64 = 128 * 1024 * 1024;
/// Most bytes one ZIP may expand to, however small it is. For an upload this is
/// memory.
const MAX_UPLOAD_BYTES: u64 = 256 * 1024 * 1024;

/// What an upload held, each asset under its checked name.
#[derive(Debug, Default)]
pub struct AssetUpload {
    pub assets: Vec<Document>,
    pub skipped: Vec<SkippedFile>,
}

fn checked(name: &str, bytes: bytes::Bytes) -> Result<Document> {
    check_name(name).map_err(Error::InvalidRequest)?;
    check_content(name, &bytes).map_err(Error::InvalidRequest)?;
    Ok(Document::new(name, bytes))
}

/// Reads an upload: one image or stylesheet, under `name` when it is given, or a
/// ZIP of them, as [`each_in_zip`] reads it.
pub fn unpack_upload(doc: &Document, name: Option<&str>) -> Result<AssetUpload> {
    if !doc.bytes.starts_with(b"PK") {
        return Ok(AssetUpload {
            assets: vec![checked(name.unwrap_or(&doc.name), doc.bytes.clone())?],
            skipped: Vec::new(),
        });
    }
    if name.is_some() {
        return Err(Error::InvalidRequest(
            "'name' is for one file; in a ZIP each file's path is its name".into(),
        ));
    }
    let mut assets = Vec::new();
    let skipped = each_in_zip(doc, |asset| {
        assets.push(asset);
        Ok::<_, Error>(())
    })?;
    Ok(AssetUpload { assets, skipped })
}

/// Hands each asset in a ZIP to `take` as it is read, under its path. A file named
/// as an asset that is not one fails the whole, so a broken package is noticed;
/// anything else, such as a licence, is returned as skipped. Sizes are counted as
/// bytes come out rather than taken from the archive's headers.
pub fn each_in_zip<E: From<Error>>(
    doc: &Document,
    mut take: impl FnMut(Document) -> std::result::Result<(), E>,
) -> std::result::Result<Vec<SkippedFile>, E> {
    if doc.bytes.len() as u64 > MAX_ZIP_BYTES {
        return Err(Error::InvalidRequest(format!(
            "the upload is larger than {MAX_ZIP_BYTES} bytes"
        ))
        .into());
    }
    let invalid =
        |e: zip::result::ZipError| Error::InvalidRequest(format!("the ZIP cannot be read: {e}"));
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(doc.bytes.as_ref())).map_err(invalid)?;
    if archive.len() > MAX_ZIP_ENTRIES {
        return Err(Error::InvalidRequest(format!(
            "the ZIP has more than {MAX_ZIP_ENTRIES} entries"
        ))
        .into());
    }
    let mut skipped = Vec::new();
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(invalid)?;
        let path = entry.name().map_err(invalid)?.replace('\\', "/");
        let path = path.strip_prefix("./").unwrap_or(&path).to_string();
        let hidden = path
            .split('/')
            .any(|part| part == "__MACOSX" || (part.starts_with('.') && part != ".."));
        if entry.is_dir() || hidden {
            continue;
        }
        let skip = |reason: &str| SkippedFile {
            file: path.clone(),
            reason: reason.to_string(),
        };
        if entry.is_symlink() {
            skipped.push(skip("a link, not a file"));
            continue;
        }
        if kind_of(&path).is_none() {
            let lower = path.to_ascii_lowercase();
            let font = [".ttf", ".otf", ".ttc", ".woff", ".woff2"];
            skipped.push(skip(if font.iter().any(|e| lower.ends_with(e)) {
                "a font; upload fonts to /v1/fonts"
            } else {
                "not an image or a stylesheet"
            }));
            continue;
        }
        // Before the bytes: nothing is read for a name that will be refused.
        check_name(&path).map_err(Error::InvalidRequest)?;
        let mut bytes = Vec::new();
        (&mut entry)
            .take(MAX_ASSET_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| {
                Error::InvalidRequest(format!("'{path}' cannot be read from the ZIP: {e}"))
            })?;
        total += bytes.len() as u64;
        if total > MAX_UPLOAD_BYTES {
            return Err(Error::InvalidRequest(format!(
                "the ZIP expands to more than {MAX_UPLOAD_BYTES} bytes"
            ))
            .into());
        }
        if !seen.insert(path.clone()) {
            return Err(
                Error::InvalidRequest(format!("the ZIP holds '{path}' more than once")).into(),
            );
        }
        take(checked(&path, bytes.into())?)?;
    }
    if seen.is_empty() {
        return Err(Error::InvalidRequest(
            "the ZIP holds no images (PNG, JPEG, GIF, SVG, WebP) or stylesheets (CSS)".into(),
        )
        .into());
    }
    Ok(skipped)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    fn zip_of(entries: &[(&str, &[u8])]) -> Document {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, content) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(content).unwrap();
        }
        Document::new("pack.zip", zip.finish().unwrap().into_inner())
    }

    #[test]
    fn a_zip_keeps_its_folders_and_reports_the_rest() {
        let upload = unpack_upload(
            &zip_of(&[
                ("brand/logo.svg", b"<svg/>"),
                ("./page.css", b"p {}"),
                ("LICENSE.txt", b"terms"),
                ("fonts/a.ttf", b"font"),
                ("__MACOSX/brand/._logo.svg", b"junk"),
                ("brand/.DS_Store", b"junk"),
            ]),
            None,
        )
        .unwrap();
        let names: Vec<_> = upload.assets.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["brand/logo.svg", "page.css"]);
        let skipped: Vec<_> = upload.skipped.iter().map(|s| s.file.as_str()).collect();
        assert_eq!(skipped, ["LICENSE.txt", "fonts/a.ttf"]);
    }

    #[test]
    fn bad_uploads_are_refused_whole() {
        for (entries, why) in [
            (&[("../logo.svg", &b"<svg/>"[..])][..], "not an asset name"),
            (&[("a b.svg", b"<svg/>")], "not an asset name"),
            (&[("ok.css", b""), ("logo.png", b"<svg/>")], "extension"),
            (&[("a.css", b""), ("./a.css", b"")], "more than once"),
            (&[("readme.md", b"")], "no images"),
        ] {
            let e = unpack_upload(&zip_of(entries), None).unwrap_err();
            assert!(e.to_string().contains(why), "{entries:?}: {e}");
        }
        let one = Document::new("logo.svg", &b"<svg/>"[..]);
        let named = unpack_upload(&one, Some("brand/logo.svg")).unwrap();
        assert_eq!(named.assets[0].name, "brand/logo.svg");
        assert!(unpack_upload(&one, Some("brand/logo.png")).is_err());
        assert!(unpack_upload(&zip_of(&[("a.css", b"")]), Some("b.css")).is_err());
    }
}
