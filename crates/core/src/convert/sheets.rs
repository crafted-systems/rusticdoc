//! `SinglePageSheets` starts each sheet's page at the workbook's saved scroll
//! position, cutting off everything above and to the left of it. This clears
//! that position from a copy of the workbook before LibreOffice opens it.

use std::io::{Cursor, Read, Write};

use bytes::Bytes;

/// A worksheet larger than this once inflated leaves the workbook as it was.
const WORKSHEET_LIMIT: u64 = 128 << 20;
/// A workbook with more entries than this is left as it was.
const ENTRY_LIMIT: usize = 100_000;

/// The workbook with `topLeftCell` cleared, or the input whenever that cannot
/// be done: a scrolled page is better than a failed conversion.
pub(super) fn reset_scroll(bytes: Bytes, ext: &str) -> Bytes {
    if !matches!(ext, "xlsx" | "xlsm") {
        return bytes;
    }
    match rewrite(&bytes) {
        Some(out) => Bytes::from(out),
        None => bytes,
    }
}

fn rewrite(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    if archive.len() > ENTRY_LIMIT {
        return None;
    }
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let mut changed = false;
    for i in 0..archive.len() {
        let name = archive.by_index_raw(i).ok()?.name().ok()?.to_string();
        let is_sheet =
            name.starts_with("xl/worksheets/") && name.to_ascii_lowercase().ends_with(".xml");
        if is_sheet {
            let mut data = Vec::new();
            archive
                .by_index(i)
                .ok()?
                .take(WORKSHEET_LIMIT + 1)
                .read_to_end(&mut data)
                .ok()?;
            if data.len() as u64 > WORKSHEET_LIMIT {
                return None;
            }
            if let Some(clean) = strip_top_left_cell(&data) {
                let options = zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated);
                out.start_file(name, options).ok()?;
                out.write_all(&clean).ok()?;
                changed = true;
                continue;
            }
        }
        out.raw_copy_file(archive.by_index_raw(i).ok()?).ok()?;
    }
    if !changed {
        return None;
    }
    Some(out.finish().ok()?.into_inner())
}

/// `data` without its ` topLeftCell="..."` attributes, or `None` if it has none.
fn strip_top_left_cell(data: &[u8]) -> Option<Vec<u8>> {
    const ATTR: &[u8] = b" topLeftCell=\"";
    let mut out = Vec::with_capacity(data.len());
    let mut at = 0;
    let mut found = false;
    while let Some(rel) = data[at..].windows(ATTR.len()).position(|w| w == ATTR) {
        let start = at + rel;
        let value = start + ATTR.len();
        let end = value + data[value..].iter().position(|b| *b == b'"')?;
        out.extend_from_slice(&data[at..start]);
        at = end + 1;
        found = true;
    }
    if !found {
        return None;
    }
    out.extend_from_slice(&data[at..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workbook(sheet: &[u8]) -> Vec<u8> {
        let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        out.start_file("xl/worksheets/sheet1.xml", options).unwrap();
        out.write_all(sheet).unwrap();
        out.start_file("xl/workbook.xml", options).unwrap();
        out.write_all(b"<workbook/>").unwrap();
        out.finish().unwrap().into_inner()
    }

    #[test]
    fn scroll_positions_are_cleared() {
        let bytes = workbook(b"<sheetView topLeftCell=\"A42\" tabSelected=\"1\"/>");
        let out = reset_scroll(Bytes::from(bytes), "xlsx");
        let mut archive = zip::ZipArchive::new(Cursor::new(&out[..])).unwrap();
        let mut sheet = String::new();
        archive
            .by_name("xl/worksheets/sheet1.xml")
            .unwrap()
            .read_to_string(&mut sheet)
            .unwrap();
        assert_eq!(sheet, "<sheetView tabSelected=\"1\"/>");
        assert!(archive.by_name("xl/workbook.xml").is_ok());
    }

    #[test]
    fn other_input_is_untouched() {
        let plain = Bytes::from_static(b"not a zip");
        assert_eq!(reset_scroll(plain.clone(), "xlsx"), plain);
        assert_eq!(reset_scroll(plain.clone(), "ods"), plain);
        let clean = Bytes::from(workbook(b"<sheetView/>"));
        assert_eq!(reset_scroll(clean.clone(), "xlsm"), clean);
    }
}
