//! Telling whether a document needs a password, from its bytes alone.
//!
//! LibreOffice's own errors do not say why a document failed to open, so a
//! protected input is recognised before it gets there: encrypted Office Open
//! XML files are OLE2 containers despite their extension, and encrypted
//! OpenDocument files list `encryption-data` in their manifest.

use std::io::{Cursor, Read};

use crate::job::Document;

/// What the bytes of a document say about its protection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protection {
    /// The format cannot tell, as with a legacy `.doc`.
    Unknown,
    /// An unprotected container.
    None,
    Required,
}

const OLE2: [u8; 8] = [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1];
const ZIP: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];
const OOXML: &[&str] = &[
    "docx", "docm", "dotx", "dotm", "xlsx", "xlsm", "xltx", "xltm", "xlsb", "pptx", "pptm", "potx",
    "potm", "ppsx", "ppsm",
];
/// A manifest larger than this is not read.
const MANIFEST_LIMIT: u64 = 1 << 20;

pub fn detect(doc: &Document) -> Protection {
    if doc.bytes.starts_with(&OLE2) {
        return if OOXML.contains(&doc.extension().as_str()) {
            Protection::Required
        } else {
            Protection::Unknown
        };
    }
    if !doc.bytes.starts_with(&ZIP) {
        return Protection::Unknown;
    }
    let Ok(mut archive) = zip::ZipArchive::new(Cursor::new(&doc.bytes[..])) else {
        return Protection::Unknown;
    };
    let Ok(manifest) = archive.by_name("META-INF/manifest.xml") else {
        return Protection::None;
    };
    let mut content = Vec::new();
    if manifest
        .take(MANIFEST_LIMIT)
        .read_to_end(&mut content)
        .is_err()
    {
        return Protection::Unknown;
    }
    if content.windows(15).any(|w| w == b"encryption-data") {
        Protection::Required
    } else {
        Protection::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zipped(name: &str, content: &[u8]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        let mut w = zip::ZipWriter::new(&mut buf);
        w.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(content).unwrap();
        w.finish().unwrap();
        buf.into_inner()
    }

    #[test]
    fn containers_tell() {
        let ole = OLE2.to_vec();
        assert_eq!(
            detect(&Document::new("a.docx", ole.clone())),
            Protection::Required
        );
        assert_eq!(detect(&Document::new("a.doc", ole)), Protection::Unknown);
        assert_eq!(
            detect(&Document::new("a.docx", zipped("word/document.xml", b"x"))),
            Protection::None
        );
        assert_eq!(
            detect(&Document::new(
                "a.odt",
                zipped("META-INF/manifest.xml", b"<manifest:encryption-data/>")
            )),
            Protection::Required
        );
        assert_eq!(
            detect(&Document::new(
                "a.odt",
                zipped("META-INF/manifest.xml", b"<manifest:manifest/>")
            )),
            Protection::None
        );
        assert_eq!(
            detect(&Document::new("a.txt", b"hi".to_vec())),
            Protection::Unknown
        );
    }
}
