//! The ZIP container: reading and writing entries, finding the parts that
//! can hold text in the order the document shows them, and duplicating or
//! dropping PPTX slides after a slide loop.

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read, Write};

use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

use crate::xml::{self, Elem, Node};
use crate::{Format, TemplateError};

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub data: Vec<u8>,
    /// Written uncompressed, as ODF requires for `mimetype`.
    pub stored: bool,
}

/// The most a single part may inflate to.
pub const MAX_PART_BYTES: u64 = 64 * 1024 * 1024;
/// The most the whole package may inflate to.
pub const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
/// The most entries a package may hold.
pub const MAX_ENTRIES: usize = 10_000;

/// Reads every entry, inflating at most [`MAX_PART_BYTES`] per part and
/// [`MAX_TOTAL_BYTES`] in all. The sizes a ZIP declares are not trusted.
pub fn read(bytes: &[u8]) -> Result<Vec<Entry>, TemplateError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| TemplateError::new(format!("not a valid Office file (ZIP): {e}")))?;
    if archive.len() > MAX_ENTRIES {
        return Err(TemplateError::new(format!(
            "the package has {} entries; at most {MAX_ENTRIES} are allowed",
            archive.len()
        )));
    }
    let mut entries = Vec::new();
    let mut total: u64 = 0;
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| TemplateError::new(format!("cannot read ZIP entry {i}: {e}")))?;
        if file.is_dir() {
            continue;
        }
        let name = file
            .name()
            .map_err(|e| TemplateError::new(format!("the file is not a valid package: {e}")))?
            .to_string();
        // Entries go on to the output as they came, so a name that walks out of
        // its directory would reach whoever unpacks the result.
        if name.starts_with('/') || name.contains('\\') || name.split('/').any(|s| s == "..") {
            return Err(TemplateError::new(format!(
                "the package has an entry with an unsafe name: {name:?}"
            )));
        }
        let allowed = MAX_PART_BYTES.min(MAX_TOTAL_BYTES - total);
        let mut data = Vec::new();
        file.by_ref()
            .take(allowed + 1)
            .read_to_end(&mut data)
            .map_err(|e| TemplateError::new(format!("cannot read {name}: {e}")))?;
        if data.len() as u64 > allowed {
            return Err(TemplateError::new(format!(
                "{name} inflates past the limit ({MAX_PART_BYTES} bytes per part, {MAX_TOTAL_BYTES} in all)"
            )));
        }
        total += data.len() as u64;
        entries.push(Entry {
            name,
            data,
            stored: file.compression() == CompressionMethod::Stored,
        });
    }
    Ok(entries)
}

pub fn write(entries: &[Entry]) -> Result<Vec<u8>, TemplateError> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let fail = |e: zip::result::ZipError| TemplateError::new(format!("cannot write ZIP: {e}"));
    for e in entries {
        let method = if e.stored {
            CompressionMethod::Stored
        } else {
            CompressionMethod::Deflated
        };
        zip.start_file(
            &e.name,
            SimpleFileOptions::default().compression_method(method),
        )
        .map_err(fail)?;
        zip.write_all(&e.data)
            .map_err(|e| TemplateError::new(format!("cannot write ZIP: {e}")))?;
    }
    Ok(zip.finish().map_err(fail)?.into_inner())
}

pub fn get<'a>(entries: &'a [Entry], name: &str) -> Option<&'a Entry> {
    entries.iter().find(|e| e.name == name)
}

pub fn text_of(entries: &[Entry], name: &str) -> Option<String> {
    get(entries, name).and_then(|e| String::from_utf8(e.data.clone()).ok())
}

pub fn set(entries: &mut Vec<Entry>, name: &str, data: Vec<u8>) {
    match entries.iter_mut().find(|e| e.name == name) {
        Some(e) => e.data = data,
        None => entries.push(Entry {
            name: name.to_string(),
            data,
            stored: false,
        }),
    }
}

/// Removes `part`, its content-type override and the relationships from
/// `source` that point at it. Nothing changes when the part is absent.
pub fn remove_part(entries: &mut Vec<Entry>, part: &str, source: &str) {
    if get(entries, part).is_none() {
        return;
    }
    entries.retain(|e| e.name != part);
    let edit = |entries: &mut Vec<Entry>, name: &str, drop: &dyn Fn(&Elem) -> bool| {
        let Some(mut nodes) = text_of(entries, name).and_then(|s| xml::parse(&s).ok()) else {
            return;
        };
        if let Some(root) = xml::root_mut(&mut nodes) {
            root.children
                .retain(|n| !matches!(n, Node::Elem(e) if drop(e)));
        }
        set(entries, name, xml::to_xml(&nodes).into_bytes());
    };
    let override_name = format!("/{part}");
    edit(entries, "[Content_Types].xml", &|e| {
        e.name == "Override" && e.attr("PartName").as_deref() == Some(override_name.as_str())
    });
    let base = source
        .rsplit_once('/')
        .map_or("", |(dir, _)| dir)
        .to_string();
    edit(entries, &rels_name(source), &|e| {
        e.name == "Relationship" && e.attr("Target").is_some_and(|t| resolve(&base, &t) == part)
    });
}

/// A relationship: `(id, type, resolved target part)`.
pub fn relationships(
    entries: &[Entry],
    rels_name: &str,
    base: &str,
) -> Vec<(String, String, String)> {
    let Some(src) = text_of(entries, rels_name) else {
        return Vec::new();
    };
    let Ok(nodes) = xml::parse(&src) else {
        return Vec::new();
    };
    let Some(root) = xml::root(&nodes) else {
        return Vec::new();
    };
    root.children_named("Relationship")
        .filter_map(|r| {
            Some((
                r.attr("Id")?,
                r.attr("Type")?,
                resolve(base, &r.attr("Target")?),
            ))
        })
        .collect()
}

/// Resolves a relationship target against the directory of its source.
pub fn resolve(base: &str, target: &str) -> String {
    if let Some(abs) = target.strip_prefix('/') {
        return abs.to_string();
    }
    let mut segs: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segs.pop();
            }
            s => segs.push(s),
        }
    }
    segs.join("/")
}

/// A part that can hold tags, with the words that prefix its places.
#[derive(Debug, Clone)]
pub struct PartInfo {
    pub name: String,
    pub prefix: String,
}

/// The text parts in document order.
pub fn text_parts(format: Format, entries: &[Entry]) -> Vec<PartInfo> {
    let plain = |name: &str| PartInfo {
        name: name.to_string(),
        prefix: String::new(),
    };
    match format {
        Format::Xlsx => {
            let rels = relationships(entries, "xl/_rels/workbook.xml.rels", "xl");
            let mut parts = Vec::new();
            if let Some(wb) = text_of(entries, "xl/workbook.xml").and_then(|s| xml::parse(&s).ok())
            {
                if let Some(sheets) = xml::root(&wb).and_then(|r| r.find("sheets").cloned()) {
                    for sheet in sheets.children_named("sheet") {
                        let (Some(name), Some(rid)) = (sheet.attr("name"), sheet.attr("r:id"))
                        else {
                            continue;
                        };
                        if let Some((_, _, target)) = rels.iter().find(|(id, _, _)| *id == rid) {
                            if get(entries, target).is_some() {
                                parts.push(PartInfo {
                                    name: target.clone(),
                                    prefix: format!("sheet \"{name}\" "),
                                });
                            }
                        }
                    }
                }
            }
            for e in entries {
                if crate::formats::is_text_part(format, &e.name)
                    && !parts.iter().any(|p| p.name == e.name)
                {
                    parts.push(plain(&e.name));
                }
            }
            parts
        }
        Format::Pptx => slide_order(entries)
            .into_iter()
            .enumerate()
            .map(|(i, name)| PartInfo {
                name,
                prefix: format!("slide {} ", i + 1),
            })
            .collect(),
        _ => {
            let mut names: Vec<&str> = entries
                .iter()
                .map(|e| e.name.as_str())
                .filter(|n| crate::formats::is_text_part(format, n))
                .collect();
            names.sort_by_key(|n| (*n != "word/document.xml" && *n != "content.xml", *n));
            names.into_iter().map(plain).collect()
        }
    }
}

/// Slide part names in presentation order.
pub fn slide_order(entries: &[Entry]) -> Vec<String> {
    let rels = relationships(entries, "ppt/_rels/presentation.xml.rels", "ppt");
    let mut out = Vec::new();
    let Some(pres) = text_of(entries, "ppt/presentation.xml").and_then(|s| xml::parse(&s).ok())
    else {
        return out;
    };
    if let Some(list) = xml::root(&pres).and_then(|r| r.find("p:sldIdLst").cloned()) {
        for id in list.children_named("p:sldId") {
            if let Some(rid) = id.attr("r:id") {
                if let Some((_, _, target)) = rels.iter().find(|(i, _, _)| *i == rid) {
                    if get(entries, target).is_some() {
                        out.push(target.clone());
                    }
                }
            }
        }
    }
    out
}

const SLIDE_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide";
const SLIDE_CT: &str = "application/vnd.openxmlformats-officedocument.presentationml.slide+xml";

pub fn rels_name(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}

fn max_number(it: impl Iterator<Item = String>, prefix: &str, suffix: &str) -> u32 {
    it.filter_map(|s| {
        s.strip_prefix(prefix)?
            .strip_suffix(suffix)?
            .parse::<u32>()
            .ok()
    })
    .max()
    .unwrap_or(0)
}

/// Applies the rendered slides: the output lists every slide in its final
/// order, with looped slides repeated or missing. Copies get new parts,
/// rels and ids; the slide list, presentation rels and content types are
/// rewritten to match.
pub fn apply_slides(
    entries: &mut Vec<Entry>,
    outputs: &[(String, String)],
) -> Result<(), TemplateError> {
    let original = slide_order(entries);
    let mut seen: HashSet<String> = HashSet::new();
    let mut next_slide = max_number(
        entries.iter().map(|e| e.name.clone()),
        "ppt/slides/slide",
        ".xml",
    ) + 1;
    let mut final_order: Vec<String> = Vec::new();
    let mut new_rels: Vec<(String, String)> = Vec::new(); // (copy, original)

    for (name, content) in outputs {
        if !original.contains(name) {
            set(entries, name, content.clone().into_bytes());
            continue;
        }
        if seen.insert(name.clone()) {
            set(entries, name, content.clone().into_bytes());
            final_order.push(name.clone());
        } else {
            let copy = format!("ppt/slides/slide{next_slide}.xml");
            next_slide += 1;
            entries.push(Entry {
                name: copy.clone(),
                data: content.clone().into_bytes(),
                stored: false,
            });
            if let Some(rels) = text_of(entries, &rels_name(name)) {
                let cleaned = strip_notes_rels(&rels);
                entries.push(Entry {
                    name: rels_name(&copy),
                    data: cleaned.into_bytes(),
                    stored: false,
                });
            }
            new_rels.push((copy.clone(), name.clone()));
            final_order.push(copy);
        }
    }

    // Slides the loops dropped.
    let removed: Vec<String> = original
        .iter()
        .filter(|s| !seen.contains(*s))
        .cloned()
        .collect();
    let mut dead: HashSet<String> = HashSet::new();
    for slide in &removed {
        dead.insert(slide.clone());
        dead.insert(rels_name(slide));
        let base = slide.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        for (_, ty, target) in relationships(entries, &rels_name(slide), base) {
            if ty.ends_with("/notesSlide") {
                dead.insert(target.clone());
                dead.insert(rels_name(&target));
            }
        }
    }
    entries.retain(|e| !dead.contains(&e.name));

    // presentation.xml.rels: drop removed, add copies.
    let pres_rels = "ppt/_rels/presentation.xml.rels";
    let src = text_of(entries, pres_rels)
        .ok_or_else(|| TemplateError::new("missing ppt/_rels/presentation.xml.rels"))?;
    let mut nodes = xml::parse(&src).map_err(TemplateError::new)?;
    let root = xml::root_mut(&mut nodes).ok_or_else(|| TemplateError::new("empty rels"))?;
    root.children.retain(|n| match n {
        Node::Elem(e) => !e
            .attr("Target")
            .map(|t| dead.contains(&resolve("ppt", &t)))
            .unwrap_or(false),
        _ => true,
    });
    let mut rid_of: HashMap<String, String> = root
        .children_named("Relationship")
        .filter_map(|r| Some((resolve("ppt", &r.attr("Target")?), r.attr("Id")?)))
        .collect();
    let first_rid = max_number(rid_of.values().cloned(), "rId", "") + 1;
    for (n, (copy, _)) in new_rels.iter().enumerate() {
        let rid = format!("rId{}", first_rid + n as u32);
        root.children.push(Node::Elem(Elem::new(
            "Relationship",
            &[
                ("Id", &rid),
                ("Type", SLIDE_TYPE),
                ("Target", &copy["ppt/".len()..]),
            ],
            true,
        )));
        rid_of.insert(copy.clone(), rid);
    }
    set(entries, pres_rels, xml::to_xml(&nodes).into_bytes());

    // presentation.xml: rebuild the slide list in the final order.
    let src = text_of(entries, "ppt/presentation.xml")
        .ok_or_else(|| TemplateError::new("missing ppt/presentation.xml"))?;
    let mut nodes = xml::parse(&src).map_err(TemplateError::new)?;
    let root = xml::root_mut(&mut nodes).ok_or_else(|| TemplateError::new("empty presentation"))?;
    if let Some(list) = root.find_mut("p:sldIdLst") {
        let existing: HashMap<String, Elem> = list
            .children_named("p:sldId")
            .filter_map(|e| Some((e.attr("r:id")?, e.clone())))
            .collect();
        let mut next_id = existing
            .values()
            .filter_map(|e| e.attr("id")?.parse::<u32>().ok())
            .max()
            .unwrap_or(255)
            + 1;
        let mut children = Vec::new();
        for slide in &final_order {
            let Some(rid) = rid_of.get(slide) else {
                continue;
            };
            match existing.get(rid) {
                Some(e) => children.push(Node::Elem(e.clone())),
                None => {
                    children.push(Node::Elem(Elem::new(
                        "p:sldId",
                        &[("id", &next_id.to_string()), ("r:id", rid)],
                        true,
                    )));
                    next_id += 1;
                }
            }
        }
        list.make_open();
        list.children = children;
    }
    set(
        entries,
        "ppt/presentation.xml",
        xml::to_xml(&nodes).into_bytes(),
    );

    // [Content_Types].xml: overrides for copies, none for removed parts.
    if let Some(src) = text_of(entries, "[Content_Types].xml") {
        let mut nodes = xml::parse(&src).map_err(TemplateError::new)?;
        if let Some(root) = xml::root_mut(&mut nodes) {
            root.children.retain(|n| match n {
                Node::Elem(e) => !e
                    .attr("PartName")
                    .map(|p| dead.contains(p.trim_start_matches('/')))
                    .unwrap_or(false),
                _ => true,
            });
            for (copy, _) in &new_rels {
                root.children.push(Node::Elem(Elem::new(
                    "Override",
                    &[("PartName", &format!("/{copy}")), ("ContentType", SLIDE_CT)],
                    true,
                )));
            }
        }
        set(
            entries,
            "[Content_Types].xml",
            xml::to_xml(&nodes).into_bytes(),
        );
    }
    Ok(())
}

/// A copied slide must not share its original's notes slide.
fn strip_notes_rels(rels: &str) -> String {
    let Ok(mut nodes) = xml::parse(rels) else {
        return rels.to_string();
    };
    if let Some(root) = xml::root_mut(&mut nodes) {
        root.children.retain(|n| match n {
            Node::Elem(e) => !e
                .attr("Type")
                .map(|t| t.ends_with("/notesSlide"))
                .unwrap_or(false),
            _ => true,
        });
    }
    xml::to_xml(&nodes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_targets() {
        assert_eq!(resolve("ppt", "slides/slide1.xml"), "ppt/slides/slide1.xml");
        assert_eq!(resolve("ppt/slides", "../media/a.png"), "ppt/media/a.png");
        assert_eq!(
            resolve("xl", "/xl/worksheets/sheet1.xml"),
            "xl/worksheets/sheet1.xml"
        );
        assert_eq!(
            rels_name("ppt/slides/slide1.xml"),
            "ppt/slides/_rels/slide1.xml.rels"
        );
    }

    #[test]
    fn refuses_entries_that_walk_out_of_the_package() {
        for name in ["../../etc/cron.d/x", "word/../../x", "/etc/passwd", "a\\b"] {
            let entries = [Entry {
                name: name.to_string(),
                data: b"x".to_vec(),
                stored: false,
            }];
            let err = read(&write(&entries).unwrap()).unwrap_err();
            assert!(
                err.message.contains("unsafe name"),
                "{name}: {}",
                err.message
            );
        }
    }

    #[test]
    fn refuses_bombs_and_too_many_entries() {
        let zeros = vec![0u8; MAX_PART_BYTES as usize + 1];
        let bomb = write(&[Entry {
            name: "big.xml".into(),
            data: zeros,
            stored: false,
        }])
        .unwrap();
        assert!(bomb.len() < 200_000, "deflated zeros are small");
        let err = read(&bomb).unwrap_err();
        assert!(err.message.contains("inflates past the limit"), "{err}");

        let many: Vec<Entry> = (0..=MAX_ENTRIES)
            .map(|i| Entry {
                name: format!("e{i}"),
                data: Vec::new(),
                stored: true,
            })
            .collect();
        let err = read(&write(&many).unwrap()).unwrap_err();
        assert!(err.message.contains("entries"), "{err}");
    }

    #[test]
    fn zip_round_trip() {
        let entries = vec![
            Entry {
                name: "mimetype".into(),
                data: b"application/vnd.oasis.opendocument.text".to_vec(),
                stored: true,
            },
            Entry {
                name: "content.xml".into(),
                data: b"<a/>".to_vec(),
                stored: false,
            },
        ];
        let bytes = write(&entries).unwrap();
        let back = read(&bytes).unwrap();
        assert_eq!(back.len(), 2);
        assert!(back[0].stored);
        assert_eq!(back[1].data, b"<a/>");
    }
}
