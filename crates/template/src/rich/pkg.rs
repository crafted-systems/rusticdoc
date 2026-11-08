//! Edits to the package around a part: media, chart parts, relationships,
//! content types and the ODF manifest. Parts are read when first needed and
//! written once at the end, so one that gains nothing stays byte-identical.

use std::collections::HashMap;

use crate::package::{self, Entry, MAX_TOTAL_BYTES};
use crate::rich::Media;
use crate::xml::{self, Elem, Node};
use crate::TemplateError;

const REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const TYPES: &str = "[Content_Types].xml";
const MANIFEST: &str = "META-INF/manifest.xml";

pub const OFFICE_RELS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
pub const REL_IMAGE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
pub const REL_HYPERLINK: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";
pub const REL_CHART: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart";
pub const REL_DRAWING: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing";
pub const REL_WORKSHEET: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet";
pub const REL_STYLES: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles";
pub const CHART_CT: &str = "application/vnd.openxmlformats-officedocument.drawingml.chart+xml";

struct RelFile {
    nodes: Vec<Node>,
    next: u32,
    known: HashMap<(String, String), String>,
}

pub struct Pkg<'a> {
    pub entries: &'a mut Vec<Entry>,
    rels: HashMap<String, RelFile>,
    types: Option<Vec<Node>>,
    manifest: Option<Vec<Node>>,
    media: HashMap<[u8; 32], String>,
    next_name: u32,
    next_part: HashMap<(String, String), u32>,
    /// Bytes of generated markup so far.
    generated: u64,
    /// Chart parts written, per placement rather than per filter call.
    charts: usize,
}

/// The path of `to` as seen from the directory of `from`.
pub fn relative(from: &str, to: &str) -> String {
    let from: Vec<&str> = from.split('/').collect();
    let to: Vec<&str> = to.split('/').collect();
    let dir = &from[..from.len() - 1];
    let common = dir.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut out = vec![".."; dir.len() - common];
    out.extend(&to[common..]);
    out.join("/")
}

impl<'a> Pkg<'a> {
    pub fn new(entries: &'a mut Vec<Entry>) -> Self {
        Pkg {
            entries,
            rels: HashMap::new(),
            types: None,
            manifest: None,
            media: HashMap::new(),
            next_name: 1,
            next_part: HashMap::new(),
            generated: 0,
            charts: 0,
        }
    }

    fn free_name(&mut self, dir: &str, stem: &str, ext: &str) -> String {
        let next = self
            .next_part
            .entry((dir.to_string(), stem.to_string()))
            .or_insert(1);
        loop {
            let name = format!("{dir}/{stem}{next}.{ext}");
            *next += 1;
            if package::get(self.entries, &name).is_none() {
                return name;
            }
        }
    }

    /// Counts generated markup against the package limit while it is being
    /// written, so that small input cannot expand without bound before the
    /// final check.
    pub fn charge(&mut self, bytes: usize) -> Result<(), TemplateError> {
        self.generated += bytes as u64;
        if self.generated > MAX_TOTAL_BYTES {
            return Err(TemplateError::new(format!(
                "the generated content is larger than the package limit of {MAX_TOTAL_BYTES} bytes"
            )));
        }
        Ok(())
    }

    /// The part holding these bytes, created the first time. No name comes
    /// from data; all of them are `richN.ext`.
    pub fn media_part(&mut self, media: &Media, dir: &str) -> Result<String, TemplateError> {
        if let Some(name) = self.media.get(&media.hash) {
            return Ok(name.clone());
        }
        let mut name = format!("{dir}/rich{}.{}", self.next_name, media.ext);
        while package::get(self.entries, &name).is_some() {
            self.next_name += 1;
            name = format!("{dir}/rich{}.{}", self.next_name, media.ext);
        }
        self.next_name += 1;
        package::set(self.entries, &name, media.bytes.clone());
        self.media.insert(media.hash, name.clone());
        if dir == "Pictures" {
            self.manifest_entry(&name, media.mime)?;
        } else {
            self.default_type(media.ext, media.mime)?;
        }
        Ok(name)
    }

    /// A new generated XML part with a content type override.
    pub fn new_part(
        &mut self,
        dir: &str,
        stem: &str,
        data: String,
        content_type: &str,
    ) -> Result<String, TemplateError> {
        // Charged before it is stored, so repeated placements of one chart
        // cannot pass the package limit unseen.
        self.charge(data.len())?;
        if stem == "chart" {
            self.charts += 1;
            if self.charts > super::MAX_CHARTS {
                return Err(TemplateError::new(format!(
                    "at most {} charts are allowed in one render",
                    super::MAX_CHARTS
                )));
            }
        }
        let name = self.free_name(dir, stem, "xml");
        package::set(self.entries, &name, data.into_bytes());
        self.override_type(&name, content_type)?;
        Ok(name)
    }

    /// A relationship from `part`; the same type and target share one id.
    pub fn rel(
        &mut self,
        part: &str,
        ty: &str,
        target: &str,
        external: bool,
    ) -> Result<String, TemplateError> {
        let rels_name = package::rels_name(part);
        if !self.rels.contains_key(&rels_name) {
            let file = self.load_rels(&rels_name)?;
            self.rels.insert(rels_name.clone(), file);
        }
        let Some(file) = self.rels.get_mut(&rels_name) else {
            return Err(TemplateError::new("a relationships file went missing"));
        };
        let key = (ty.to_string(), target.to_string());
        if let Some(id) = file.known.get(&key) {
            return Ok(id.clone());
        }
        let id = format!("rId{}", file.next);
        file.next += 1;
        let mut attrs = vec![("Id", id.as_str()), ("Type", ty), ("Target", target)];
        if external {
            attrs.push(("TargetMode", "External"));
        }
        let elem = Elem::new("Relationship", &attrs, true);
        if let Some(root) = xml::root_mut(&mut file.nodes) {
            root.make_open();
            root.children.push(Node::Elem(elem));
        }
        file.known.insert(key, id.clone());
        Ok(id)
    }

    /// A relationship from `part` to another part of the package.
    pub fn rel_to(&mut self, part: &str, ty: &str, to: &str) -> Result<String, TemplateError> {
        self.rel(part, ty, &relative(part, to), false)
    }

    fn load_rels(&self, rels_name: &str) -> Result<RelFile, TemplateError> {
        let src = package::text_of(self.entries, rels_name).unwrap_or_else(|| {
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
                 <Relationships xmlns=\"{REL_NS}\"/>"
            )
        });
        let nodes = xml::parse(&src).map_err(|e| broken(rels_name, e))?;
        let highest = xml::root(&nodes)
            .map(|root| {
                root.children_named("Relationship")
                    .filter_map(|r| r.attr("Id")?.strip_prefix("rId")?.parse::<u32>().ok())
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        Ok(RelFile {
            nodes,
            next: highest + 1,
            known: HashMap::new(),
        })
    }

    fn types_mut(&mut self) -> Result<&mut Elem, TemplateError> {
        if self.types.is_none() {
            let src = package::text_of(self.entries, TYPES)
                .ok_or_else(|| TemplateError::new("the package has no [Content_Types].xml"))?;
            self.types = Some(xml::parse(&src).map_err(|e| broken(TYPES, e))?);
        }
        self.types
            .as_mut()
            .and_then(|nodes| xml::root_mut(nodes))
            .ok_or_else(|| broken(TYPES, "no root element"))
    }

    fn default_type(&mut self, ext: &str, mime: &str) -> Result<(), TemplateError> {
        let root = self.types_mut()?;
        let found = root.children_named("Default").any(|d| {
            d.attr("Extension")
                .is_some_and(|e| e.eq_ignore_ascii_case(ext))
        });
        if !found {
            let at = root
                .children
                .iter()
                .rposition(|n| matches!(n, Node::Elem(e) if e.name == "Default"))
                .map_or(0, |i| i + 1);
            let elem = Elem::new(
                "Default",
                &[("Extension", ext), ("ContentType", mime)],
                true,
            );
            root.make_open();
            root.children.insert(at, Node::Elem(elem));
        }
        Ok(())
    }

    pub fn override_type(&mut self, part: &str, content_type: &str) -> Result<(), TemplateError> {
        let root = self.types_mut()?;
        let name = format!("/{part}");
        let elem = Elem::new(
            "Override",
            &[("PartName", &name), ("ContentType", content_type)],
            true,
        );
        root.make_open();
        root.children.push(Node::Elem(elem));
        Ok(())
    }

    fn manifest_entry(&mut self, path: &str, mime: &str) -> Result<(), TemplateError> {
        if self.manifest.is_none() {
            let src = package::text_of(self.entries, MANIFEST)
                .ok_or_else(|| TemplateError::new("the package has no META-INF/manifest.xml"))?;
            self.manifest = Some(xml::parse(&src).map_err(|e| broken(MANIFEST, e))?);
        }
        let root = self
            .manifest
            .as_mut()
            .and_then(|nodes| xml::root_mut(nodes))
            .ok_or_else(|| broken(MANIFEST, "no root element"))?;
        let elem = Elem::new(
            "manifest:file-entry",
            &[("manifest:full-path", path), ("manifest:media-type", mime)],
            true,
        );
        root.make_open();
        root.children.push(Node::Elem(elem));
        Ok(())
    }

    /// Writes what changed and enforces the package limit on the result.
    pub fn finish(self) -> Result<(), TemplateError> {
        let Pkg {
            entries,
            rels,
            types,
            manifest,
            ..
        } = self;
        // In name order: a new rels file joins the end of the ZIP, and a map's
        // order changes from run to run, which made the same render differ.
        let mut rels: Vec<_> = rels.into_iter().collect();
        rels.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, file) in rels {
            package::set(entries, &name, xml::to_xml(&file.nodes).into_bytes());
        }
        if let Some(nodes) = types {
            package::set(entries, TYPES, xml::to_xml(&nodes).into_bytes());
        }
        if let Some(nodes) = manifest {
            package::set(entries, MANIFEST, xml::to_xml(&nodes).into_bytes());
        }
        check_total(entries)
    }
}

fn broken(part: &str, e: impl std::fmt::Display) -> TemplateError {
    TemplateError::new(format!("{part}: malformed XML: {e}"))
}

fn check_total(entries: &[Entry]) -> Result<(), TemplateError> {
    let total: u64 = entries.iter().map(|e| e.data.len() as u64).sum();
    if total > MAX_TOTAL_BYTES {
        return Err(TemplateError::new(format!(
            "the rendered package would hold {total} bytes; at most {MAX_TOTAL_BYTES} are allowed"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_relative_targets() {
        assert_eq!(
            relative("word/document.xml", "word/media/rich1.png"),
            "media/rich1.png"
        );
        assert_eq!(
            relative("ppt/slides/slide2.xml", "ppt/media/rich1.png"),
            "../media/rich1.png"
        );
        assert_eq!(
            relative("xl/worksheets/sheet1.xml", "xl/drawings/drawing1.xml"),
            "../drawings/drawing1.xml"
        );
    }

    #[test]
    fn generated_markup_counts_against_the_limit() {
        let mut entries = Vec::new();
        let mut pkg = Pkg::new(&mut entries);
        assert!(pkg.charge(MAX_TOTAL_BYTES as usize).is_ok());
        let err = pkg.charge(1).unwrap_err();
        assert!(err.message.contains("package limit"), "{err}");
    }

    #[test]
    fn the_package_limit_counts_everything() {
        let big = |name: &str, n: u64| Entry {
            name: name.into(),
            data: vec![0; n as usize],
            stored: false,
        };
        assert!(check_total(&[big("a", 10)]).is_ok());
        let over = [big("a", MAX_TOTAL_BYTES), big("b", 1)];
        assert!(check_total(&over).unwrap_err().message.contains("at most"));
    }
}
