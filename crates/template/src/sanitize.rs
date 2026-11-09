//! What a template must not carry into its output.
//!
//! A template is uploaded by whoever calls the service, and the document made
//! from it is handed to someone else, so it must not bring along anything
//! that runs or reaches out when opened: macro projects, OLE objects and
//! ActiveX controls, links that pull in data, pictures or a template from
//! elsewhere, and hyperlinks into the reader's own network. [`sanitize`] takes
//! those out of a rendered package unless the caller asks to keep them, and
//! leaves a document that still opens cleanly: no relationship, content type
//! or manifest entry points at something that is gone, an object keeps its
//! preview picture where it has one, and a link keeps its text.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::package::{self, Entry};
use crate::xml::{self, Elem, Node};
use crate::{Format, TemplateError};

/// What [`sanitize`] may leave in. The default keeps nothing, which is what
/// every render gets unless a request opts in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keep {
    /// VBA projects (OOXML) and Basic or script libraries (ODF), with the
    /// event bindings and macro links that call them.
    pub macros: bool,
    /// OLE objects, ActiveX controls, applets and plugins. Charts and
    /// formulas are not objects in this sense and always stay.
    pub objects: bool,
    /// Relationships and links that load something from outside the file:
    /// linked data and workbooks, linked pictures, attached templates.
    pub external_links: bool,
    /// Hyperlinks to local or private network addresses.
    pub private_links: bool,
}

impl Keep {
    /// Keeps everything: the output carries what the template carried.
    pub const ALL: Keep = Keep {
        macros: true,
        objects: true,
        external_links: true,
        private_links: true,
    };
}

/// How many things of each kind were taken out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Removed {
    pub macros: usize,
    pub objects: usize,
    pub external_links: usize,
    pub private_links: usize,
}

impl Removed {
    pub fn is_empty(&self) -> bool {
        *self == Removed::default()
    }

    fn count(&mut self, why: Why) {
        match why {
            Why::Macro => self.macros += 1,
            Why::Object => self.objects += 1,
            Why::External => self.external_links += 1,
            Why::Private => self.private_links += 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Why {
    Macro,
    Object,
    External,
    Private,
}

/// Takes out of a package what `keep` does not keep, and reports what went.
pub fn sanitize(
    format: Format,
    entries: &mut Vec<Entry>,
    keep: Keep,
) -> Result<Removed, TemplateError> {
    let mut removed = Removed::default();
    if keep == Keep::ALL {
        return Ok(removed);
    }
    match format {
        Format::Docx | Format::Xlsx | Format::Pptx => ooxml(format, entries, keep, &mut removed)?,
        Format::Odt | Format::Ods | Format::Odp => odf(entries, keep, &mut removed)?,
    }
    Ok(removed)
}

// ---------------------------------------------------------------------------
// Addresses.

/// Whether a link points into the reader's own machine or network: `file:`
/// and UNC paths, `localhost`, `.local` and `.internal` names, and loopback,
/// private, link-local and unique-local addresses, however they are spelt.
pub fn is_private_url(url: &str) -> bool {
    let url = url.trim();
    if url.starts_with("\\\\") {
        return true;
    }
    let Some(colon) = url.find(':') else {
        return false;
    };
    let scheme = &url[..colon];
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return false;
    }
    if scheme.eq_ignore_ascii_case("file") {
        return true;
    }
    let Some(rest) = url[colon + 1..].strip_prefix("//") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#', '\\']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or("");
    let host = if let Some(inner) = host_port.strip_prefix('[') {
        inner.split(']').next().unwrap_or("")
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    is_private_host(&percent_decode(host))
}

fn is_private_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        // `http:///x` and the like reach the local machine in some readers.
        return true;
    }
    if host == "localhost"
        || [".localhost", ".local", ".internal"]
            .iter()
            .any(|s| host.ends_with(s))
    {
        return true;
    }
    // An IPv6 zone, `fe80::1%en0`, names the interface, not the address.
    let unzoned = host.split('%').next().unwrap_or("");
    if let Ok(v6) = unzoned.parse::<Ipv6Addr>() {
        return private_v6(v6);
    }
    match loose_ipv4(&host) {
        Some(v4) => private_v4(v4),
        None => false,
    }
}

fn private_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 0
        || a == 127
        || a == 10
        || (a == 172 && (16..32).contains(&b))
        || (a == 192 && b == 168)
        || (a == 169 && b == 254)
}

fn private_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return private_v4(v4);
    }
    let first = ip.segments()[0];
    ip.is_loopback()
        || ip.is_unspecified()
        || (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80
}

/// An IPv4 address as browsers read one: `127.1`, `2130706433` and
/// `0x7f.0.0.1` all name the loopback address.
fn loose_ipv4(host: &str) -> Option<Ipv4Addr> {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    let mut nums = Vec::with_capacity(parts.len());
    for p in &parts {
        let n = if let Some(hex) = p.strip_prefix("0x").or_else(|| p.strip_prefix("0X")) {
            u64::from_str_radix(hex, 16).ok()?
        } else if p.len() > 1 && p.starts_with('0') {
            u64::from_str_radix(&p[1..], 8).ok()?
        } else {
            p.parse::<u64>().ok()?
        };
        nums.push(n);
    }
    let (last, head) = nums.split_last()?;
    if head.iter().any(|n| *n > 255) || *last >= 1u64 << (8 * (5 - nums.len())) {
        return None;
    }
    let mut value: u64 = 0;
    for (i, n) in head.iter().enumerate() {
        value |= n << (24 - 8 * i);
    }
    value |= last;
    u32::try_from(value).ok().map(Ipv4Addr::from)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            if let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------
// OOXML.

const VBA_TYPES: &[&str] = &[
    "vbaProject",
    "vbaProjectSignature",
    "vbaProjectSignatureAgile",
    "vbaProjectSignatureV3",
    "wordVbaData",
    "keyMapCustomizations",
];
const OBJECT_TYPES: &[&str] = &["oleObject", "control", "activeXControlBinary", "package"];

const REL_NAMESPACES: &[&str] = &[
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    "http://purl.oclc.org/ooxml/officeDocument/relationships",
];

/// Macro-enabled main content types and the plain types that replace them.
const MACRO_MAIN_TYPES: &[(&str, &str)] = &[
    (
        "application/vnd.ms-word.document.macroEnabled.main+xml",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
    ),
    (
        "application/vnd.ms-word.template.macroEnabledTemplate.main+xml",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
    ),
    (
        "application/vnd.ms-excel.sheet.macroEnabled.main+xml",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml",
    ),
    (
        "application/vnd.ms-excel.template.macroEnabled.main+xml",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml",
    ),
    (
        "application/vnd.ms-powerpoint.presentation.macroEnabled.main+xml",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml",
    ),
    (
        "application/vnd.ms-powerpoint.slideshow.macroEnabled.main+xml",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml",
    ),
    (
        "application/vnd.ms-powerpoint.template.macroEnabled.main+xml",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml",
    ),
];
const VBA_CONTENT_TYPE: &str = "application/vnd.ms-office.vbaProject";

static EXTERNAL_REF_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\d+\]").unwrap());
static HYPERLINK_FN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)HYPERLINK\(\s*"([^"]*)""#).unwrap());
static FIELD_URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)^\s*HYPERLINK\s+"([^"]*)""#).unwrap());

/// The part a relationships file belongs to: `word/_rels/document.xml.rels`
/// is `word/document.xml`'s, `_rels/.rels` the package's (an empty name).
fn source_of(rels: &str) -> Option<String> {
    let (dir, file) = rels.rsplit_once('/')?;
    let file = file.strip_suffix(".rels")?;
    let dir = dir.strip_suffix("_rels")?.trim_end_matches('/');
    Some(if dir.is_empty() {
        file.to_string()
    } else {
        format!("{dir}/{file}")
    })
}

fn base_of(part: &str) -> &str {
    part.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn last_segment(ty: &str) -> &str {
    ty.rsplit('/').next().unwrap_or(ty)
}

/// Why a relationship must go, if it must.
fn classify(ty: &str, target: &str, external: bool, source: &str, keep: Keep) -> Option<Why> {
    let kind = last_segment(ty);
    if external {
        if kind == "hyperlink" {
            // LibreOffice writes a link that runs a Basic macro as a hyperlink.
            if is_macro_url(target) {
                return (!keep.macros).then_some(Why::Macro);
            }
            return (!keep.private_links && is_private_url(target)).then_some(Why::Private);
        }
        return (!keep.external_links).then_some(Why::External);
    }
    if kind == "externalLink" {
        return (!keep.external_links).then_some(Why::External);
    }
    if VBA_TYPES.contains(&kind) {
        return (!keep.macros).then_some(Why::Macro);
    }
    // A chart's own workbook is its data, not an object on the page.
    if OBJECT_TYPES.contains(&kind) && !(kind == "package" && source.contains("/charts/")) {
        return (!keep.objects).then_some(Why::Object);
    }
    None
}

struct Ooxml<'a> {
    entries: &'a mut Vec<Entry>,
    keep: Keep,
    removed: &'a mut Removed,
}

fn ooxml(
    format: Format,
    entries: &mut Vec<Entry>,
    keep: Keep,
    removed: &mut Removed,
) -> Result<(), TemplateError> {
    let mut ctx = Ooxml {
        entries,
        keep,
        removed,
    };
    // 1. Relationships: drop those that must go, noting their ids and targets.
    let mut lost: HashMap<String, HashMap<String, Why>> = HashMap::new();
    let mut doomed: BTreeSet<String> = BTreeSet::new();
    let mut external_data = false;
    let mut dropped: Vec<(String, Why)> = Vec::new();
    let rels_files: Vec<String> = ctx
        .entries
        .iter()
        .filter(|e| e.name.ends_with(".rels"))
        .map(|e| e.name.clone())
        .collect();
    for rels in &rels_files {
        let Some(source) = source_of(rels) else {
            continue;
        };
        let Some(src) = package::text_of(ctx.entries, rels) else {
            continue;
        };
        let mut nodes = xml::parse(&src).map_err(|e| broken(rels, e))?;
        let Some(root) = xml::root_mut(&mut nodes) else {
            continue;
        };
        let mut changed = false;
        let base = base_of(&source).to_string();
        root.children.retain(|n| {
            let Node::Elem(r) = n else { return true };
            if r.name != "Relationship" {
                return true;
            }
            let (Some(id), Some(ty), Some(target)) =
                (r.attr("Id"), r.attr("Type"), r.attr("Target"))
            else {
                return true;
            };
            let external = r
                .attr("TargetMode")
                .is_some_and(|m| m.eq_ignore_ascii_case("External"));
            let Some(why) = classify(&ty, &target, external, &source, keep) else {
                return true;
            };
            if last_segment(&ty) == "externalLink" {
                external_data = true;
            }
            dropped.push((source.clone(), why));
            lost.entry(source.clone()).or_default().insert(id, why);
            if !external {
                doomed.insert(package::resolve(&base, &target));
            }
            changed = true;
            false
        });
        if changed {
            package::set(ctx.entries, rels, xml::to_xml(&nodes).into_bytes());
        }
    }

    // 2. Parts nothing points at any more go, with what only they point at.
    let gone = ctx.drop_parts(doomed);
    // A part that went with another, such as an ActiveX control's binary,
    // is not counted again.
    for (source, why) in dropped {
        if !gone.contains(&source) {
            ctx.removed.count(why);
        }
    }

    // 3. Markup that named a dropped relationship.
    let mut sources: Vec<&String> = lost.keys().filter(|s| !gone.contains(*s)).collect();
    sources.sort();
    for source in sources {
        ctx.edit_part(source, &lost[source])?;
    }
    if format == Format::Docx && !keep.private_links {
        ctx.word_fields()?;
    }
    if format == Format::Xlsx {
        ctx.sheet_formulas(external_data)?;
    }

    // 4. Content types.
    ctx.content_types(&gone)?;
    Ok(())
}

impl Ooxml<'_> {
    /// Removes the doomed parts that no remaining relationship names, then
    /// whatever only those parts named, until nothing more goes.
    fn drop_parts(&mut self, mut doomed: BTreeSet<String>) -> HashSet<String> {
        let mut gone: HashSet<String> = HashSet::new();
        loop {
            // Targets named from parts still there. A doomed part's own
            // targets join the doomed only once it is gone.
            let mut named: HashSet<String> = HashSet::new();
            for e in self.entries.iter().filter(|e| e.name.ends_with(".rels")) {
                let Some(source) = source_of(&e.name) else {
                    continue;
                };
                if gone.contains(&source) {
                    continue;
                }
                for (_, _, target) in
                    package::relationships(self.entries, &e.name, base_of(&source))
                {
                    named.insert(target);
                }
            }
            let now: Vec<String> = doomed
                .iter()
                .filter(|p| !named.contains(*p) && !gone.contains(*p))
                .cloned()
                .collect();
            if now.is_empty() {
                break;
            }
            for part in now {
                let rels = package::rels_name(&part);
                for (_, ty, target) in package::relationships(self.entries, &rels, base_of(&part)) {
                    // A part's own hyperlinks are not parts.
                    if self.entries.iter().any(|e| e.name == target)
                        && last_segment(&ty) != "hyperlink"
                    {
                        doomed.insert(target);
                    }
                }
                self.entries.retain(|e| e.name != part && e.name != rels);
                gone.insert(part.clone());
                gone.insert(rels);
            }
        }
        gone
    }

    fn edit_part(&mut self, part: &str, ids: &HashMap<String, Why>) -> Result<(), TemplateError> {
        if !part.ends_with(".xml") {
            return Ok(());
        }
        let Some(src) = package::text_of(self.entries, part) else {
            return Ok(());
        };
        let mut nodes = xml::parse(&src).map_err(|e| broken(part, e))?;
        let Some(root) = xml::root_mut(&mut nodes) else {
            return Ok(());
        };
        let mut prefixes: Vec<String> = root
            .attrs()
            .into_iter()
            .filter(|(k, v)| k.starts_with("xmlns:") && REL_NAMESPACES.contains(&v.as_str()))
            .map(|(k, _)| k["xmlns:".len()..].to_string())
            .collect();
        if prefixes.is_empty() {
            prefixes.push("r".to_string());
        }
        let mut edit = Edit {
            ids,
            prefixes,
            objects_gone: !self.keep.objects,
            pictures: Vec::new(),
        };
        edit.children(&mut root.children);
        // ActiveX previews on a slide join its shapes.
        if !edit.pictures.is_empty() {
            if let Some(tree) = root.find_mut("p:spTree") {
                let at = tree
                    .children
                    .iter()
                    .position(|n| matches!(n, Node::Elem(e) if e.name == "p:extLst"))
                    .unwrap_or(tree.children.len());
                let pics = std::mem::take(&mut edit.pictures);
                tree.children.splice(at..at, pics);
            }
        }
        package::set(self.entries, part, xml::to_xml(&nodes).into_bytes());
        Ok(())
    }

    /// `HYPERLINK` fields written as simple fields keep their text and lose
    /// the field when they point into a private network.
    fn word_fields(&mut self) -> Result<(), TemplateError> {
        let names: Vec<String> = self
            .entries
            .iter()
            .filter(|e| {
                e.name.starts_with("word/")
                    && e.name.ends_with(".xml")
                    && contains(&e.data, b"w:fldSimple")
            })
            .map(|e| e.name.clone())
            .collect();
        for name in names {
            let Some(src) = package::text_of(self.entries, &name) else {
                continue;
            };
            let mut nodes = xml::parse(&src).map_err(|e| broken(&name, e))?;
            let mut count = 0;
            unwrap_where(&mut nodes, &mut |e| {
                let private = e.name == "w:fldSimple"
                    && e.attr("w:instr").is_some_and(|i| {
                        FIELD_URL_RE
                            .captures(&i)
                            .is_some_and(|c| is_private_url(&c[1]))
                    });
                count += usize::from(private);
                private
            });
            if count > 0 {
                self.removed.private_links += count;
                package::set(self.entries, &name, xml::to_xml(&nodes).into_bytes());
            }
        }
        Ok(())
    }

    /// A formula that reads a linked workbook, once the link is gone, or one
    /// that builds a private hyperlink, loses its formula. The cell keeps
    /// the value it last showed, where the package still holds one (rendering
    /// drops cached values so that Excel recomputes them).
    fn sheet_formulas(&mut self, external_data: bool) -> Result<(), TemplateError> {
        let check_links = !self.keep.private_links;
        if !external_data && !check_links {
            return Ok(());
        }
        let names: Vec<String> = self
            .entries
            .iter()
            .filter(|e| {
                (e.name.starts_with("xl/worksheets/") && e.name.ends_with(".xml")
                    || e.name == "xl/workbook.xml")
                    && (external_data || contains(&e.data, b"HYPERLINK("))
            })
            .map(|e| e.name.clone())
            .collect();
        for name in names {
            let Some(src) = package::text_of(self.entries, &name) else {
                continue;
            };
            let mut nodes = xml::parse(&src).map_err(|e| broken(&name, e))?;
            let mut shared_gone: HashSet<String> = HashSet::new();
            let mut count = (0, 0);
            let doomed = |text: &str| -> Option<Why> {
                if external_data && EXTERNAL_REF_RE.is_match(text) {
                    return Some(Why::External);
                }
                if check_links
                    && HYPERLINK_FN_RE
                        .captures_iter(text)
                        .any(|c| is_private_url(&c[1]))
                {
                    return Some(Why::Private);
                }
                None
            };
            // Shared formulas: the first cell holds the text, the others its
            // index, so the others go with it.
            remove_where(&mut nodes, &mut |e| {
                if e.name != "f" && e.name != "definedName" {
                    return false;
                }
                let why = doomed(&e.text_content());
                match why {
                    Some(why) => {
                        if e.attr("t").as_deref() == Some("shared") {
                            if let Some(si) = e.attr("si") {
                                shared_gone.insert(si);
                            }
                        }
                        if why == Why::External {
                            count.0 += 1;
                        } else {
                            count.1 += 1;
                        }
                        true
                    }
                    None => false,
                }
            });
            if !shared_gone.is_empty() {
                remove_where(&mut nodes, &mut |e| {
                    e.name == "f"
                        && e.attr("t").as_deref() == Some("shared")
                        && e.attr("si").is_some_and(|si| shared_gone.contains(&si))
                });
            }
            if count != (0, 0) {
                // Counted with the link they used, not again.
                self.removed.private_links += count.1;
                if name == "xl/workbook.xml" {
                    remove_empty(&mut nodes, &["definedNames"]);
                }
                package::set(self.entries, &name, xml::to_xml(&nodes).into_bytes());
            }
        }
        Ok(())
    }

    fn content_types(&mut self, gone: &HashSet<String>) -> Result<(), TemplateError> {
        const TYPES: &str = "[Content_Types].xml";
        let Some(src) = package::text_of(self.entries, TYPES) else {
            return Ok(());
        };
        let mut nodes = xml::parse(&src).map_err(|e| broken(TYPES, e))?;
        let Some(root) = xml::root_mut(&mut nodes) else {
            return Ok(());
        };
        let before = xml::to_xml(std::slice::from_ref(&Node::Elem(root.clone())));
        let names: HashSet<&str> = self.entries.iter().map(|e| e.name.as_str()).collect();
        let overridden: HashSet<String> = root
            .children_named("Override")
            .filter_map(|o| o.attr("PartName"))
            .map(|p| p.trim_start_matches('/').to_string())
            .collect();
        let macros_gone = !self.keep.macros;
        root.children.retain(|n| {
            let Node::Elem(e) = n else { return true };
            match e.name.as_str() {
                "Override" => e.attr("PartName").is_none_or(|p| {
                    let p = p.trim_start_matches('/');
                    !gone.contains(p) && names.contains(p)
                }),
                // A default for macro projects stays only while a part uses it.
                "Default"
                    if macros_gone
                        && e.attr("ContentType").as_deref() == Some(VBA_CONTENT_TYPE) =>
                {
                    let ext = e.attr("Extension").unwrap_or_default().to_ascii_lowercase();
                    names.iter().any(|n| {
                        n.to_ascii_lowercase().ends_with(&format!(".{ext}"))
                            && !overridden.contains(*n)
                    })
                }
                _ => true,
            }
        });
        if macros_gone {
            for n in root.children.iter_mut() {
                let Node::Elem(e) = n else { continue };
                if e.name != "Override" {
                    continue;
                }
                let Some(ct) = e.attr("ContentType") else {
                    continue;
                };
                if let Some((_, plain)) = MACRO_MAIN_TYPES.iter().find(|(m, _)| *m == ct) {
                    e.set_attr("ContentType", plain);
                }
            }
        }
        let after = xml::to_xml(std::slice::from_ref(&Node::Elem(root.clone())));
        if before != after {
            package::set(self.entries, TYPES, xml::to_xml(&nodes).into_bytes());
        }
        Ok(())
    }
}

/// One part's markup, cleaned of what named a dropped relationship.
struct Edit<'a> {
    ids: &'a HashMap<String, Why>,
    prefixes: Vec<String>,
    objects_gone: bool,
    /// Preview pictures of ActiveX controls on a slide.
    pictures: Vec<Node>,
}

enum Action {
    Keep,
    Remove,
    Unwrap,
    Replace(Vec<Node>),
}

impl Edit<'_> {
    /// The relationship attribute naming a dropped id, if one does.
    fn dangling(&self, e: &Elem) -> Option<String> {
        e.attrs().into_iter().find_map(|(k, v)| {
            let (prefix, _) = k.split_once(':')?;
            (self.prefixes.iter().any(|p| p == prefix) && self.ids.contains_key(&v)).then_some(k)
        })
    }

    fn children(&mut self, children: &mut Vec<Node>) {
        let mut i = 0;
        while i < children.len() {
            let action = match &mut children[i] {
                Node::Elem(e) => self.element(e),
                _ => Action::Keep,
            };
            match action {
                Action::Keep => i += 1,
                Action::Remove => {
                    children.remove(i);
                }
                Action::Unwrap => {
                    let Node::Elem(e) = children.remove(i) else {
                        continue;
                    };
                    let n = e.children.len();
                    children.splice(i..i, e.children);
                    i += n;
                }
                Action::Replace(nodes) => {
                    let n = nodes.len();
                    children.splice(i..=i, nodes);
                    i += n;
                }
            }
        }
    }

    fn element(&mut self, e: &mut Elem) -> Action {
        if self.objects_gone {
            match e.name.as_str() {
                "w:object" => return self.word_object(e),
                "p:graphicFrame" if is_ole_frame(e) => return ole_frame_picture(e),
                "p:controls" => {
                    collect_named(e, "p:pic", &mut self.pictures);
                    return Action::Remove;
                }
                // Sheet objects and controls: their pictures are drawn from the
                // sheet's legacy drawing, which stays.
                "oleObjects" | "controls" => return Action::Remove,
                _ => {}
            }
        }
        self.children(&mut e.children);
        if let Some(attr) = self.dangling(e) {
            return match e.name.as_str() {
                "w:hyperlink" => Action::Unwrap,
                // A picture element stays, without its link to the outside.
                "a:blip" | "v:imagedata" | "v:fill" | "v:stroke" | "o:signatureline" => {
                    e.remove_attr(&attr);
                    Action::Keep
                }
                _ => Action::Remove,
            };
        }
        let empty = e.children.iter().all(|n| !matches!(n, Node::Elem(_)));
        if empty
            && !e.self_closing
            && matches!(
                e.name.as_str(),
                "hyperlinks" | "externalReferences" | "oleObjects" | "controls"
            )
        {
            return Action::Remove;
        }
        Action::Keep
    }

    /// An OLE object or control in Word becomes the picture it showed: a VML
    /// `w:pict`, or the DrawingML picture newer versions write.
    fn word_object(&mut self, e: &mut Elem) -> Action {
        self.children(&mut e.children);
        let drawings: Vec<Node> = e
            .children
            .iter()
            .filter(|n| matches!(n, Node::Elem(c) if c.name == "w:drawing"))
            .cloned()
            .collect();
        if !drawings.is_empty() {
            return Action::Replace(drawings);
        }
        let mut shapes: Vec<Node> = e
            .children
            .iter()
            .filter(|n| matches!(n, Node::Elem(c) if c.name.starts_with("v:")))
            .cloned()
            .collect();
        if shapes.is_empty() {
            return Action::Remove;
        }
        for s in shapes.iter_mut() {
            if let Node::Elem(s) = s {
                s.remove_attr("o:ole");
            }
        }
        let mut pict = Elem::new("w:pict", &[], false);
        pict.children = shapes;
        Action::Replace(vec![Node::Elem(pict)])
    }
}

fn is_ole_frame(e: &Elem) -> bool {
    e.find("a:graphicData")
        .and_then(|g| g.attr("uri"))
        .is_some_and(|u| u.ends_with("/ole"))
}

/// A slide's OLE object becomes its preview picture, at the frame's place.
fn ole_frame_picture(frame: &Elem) -> Action {
    let Some(mut pic) = frame.find("p:pic").cloned() else {
        return Action::Remove;
    };
    if let Some(xfrm) = frame.child("p:xfrm") {
        let mut own = Elem::new("a:xfrm", &[], false);
        own.children = xfrm.children.clone();
        if let Some(spr) = pic.child_mut("p:spPr") {
            spr.children
                .retain(|n| !matches!(n, Node::Elem(c) if c.name == "a:xfrm"));
            spr.make_open();
            spr.children.insert(0, Node::Elem(own));
        }
    }
    if let (Some(from), Some(to)) = (frame.find("p:cNvPr"), pic.find_mut("p:cNvPr")) {
        if let (Some(id), Some(name)) = (from.attr("id"), from.attr("name")) {
            to.set_attr("id", &id);
            to.set_attr("name", &name);
        }
    }
    Action::Replace(vec![Node::Elem(pic)])
}

fn collect_named(e: &Elem, name: &str, out: &mut Vec<Node>) {
    for n in &e.children {
        if let Node::Elem(c) = n {
            if c.name == name {
                out.push(n.clone());
            } else {
                collect_named(c, name, out);
            }
        }
    }
}

fn contains(data: &[u8], needle: &[u8]) -> bool {
    data.windows(needle.len()).any(|w| w == needle)
}

/// Removes every element `pred` picks, at any depth.
fn remove_where(nodes: &mut Vec<Node>, pred: &mut dyn FnMut(&Elem) -> bool) {
    nodes.retain_mut(|n| match n {
        Node::Elem(e) => {
            if pred(e) {
                return false;
            }
            remove_where(&mut e.children, pred);
            true
        }
        _ => true,
    });
}

/// Replaces every element `pred` picks by its content, at any depth.
fn unwrap_where(nodes: &mut Vec<Node>, pred: &mut dyn FnMut(&Elem) -> bool) {
    let mut i = 0;
    while i < nodes.len() {
        let picked = match &mut nodes[i] {
            Node::Elem(e) => {
                unwrap_where(&mut e.children, pred);
                pred(e)
            }
            _ => false,
        };
        if picked {
            let Node::Elem(e) = nodes.remove(i) else {
                continue;
            };
            let n = e.children.len();
            nodes.splice(i..i, e.children);
            i += n;
        } else {
            i += 1;
        }
    }
}

fn remove_empty(nodes: &mut Vec<Node>, names: &[&str]) {
    remove_where(nodes, &mut |e| {
        names.contains(&e.name.as_str()) && !e.children.iter().any(|n| matches!(n, Node::Elem(_)))
    });
}

fn broken(part: &str, e: impl std::fmt::Display) -> TemplateError {
    TemplateError::new(format!("{part}: malformed XML: {e}"))
}

// ---------------------------------------------------------------------------
// ODF.

const MANIFEST: &str = "META-INF/manifest.xml";
/// Script libraries and their dialogs.
const SCRIPT_DIRS: &[&str] = &["Basic/", "Scripts/", "Dialogs/"];
/// Embedded documents that are content rather than objects.
const KEPT_OBJECT_TYPES: &[&str] = &[
    "application/vnd.oasis.opendocument.chart",
    "application/vnd.oasis.opendocument.formula",
];

/// Whether an `xlink:href` in ODF points outside the package.
fn outside(href: &str) -> bool {
    let href = href.trim();
    if href.starts_with("../") || href.starts_with('/') || href.starts_with("\\\\") {
        return true;
    }
    match href.find(':') {
        Some(colon) => {
            let scheme = &href[..colon];
            !scheme.is_empty()
                && href[..colon]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
                && !scheme.eq_ignore_ascii_case("vnd.sun.star.package")
        }
        None => false,
    }
}

fn is_macro_url(href: &str) -> bool {
    let h = href.trim().to_ascii_lowercase();
    h.starts_with("vnd.sun.star.script:") || h.starts_with("macro:")
}

fn package_path(href: &str) -> String {
    let h = href.trim();
    let h = h.strip_prefix("vnd.sun.star.package:").unwrap_or(h);
    h.trim_start_matches("./").trim_end_matches('/').to_string()
}

fn odf(entries: &mut Vec<Entry>, keep: Keep, removed: &mut Removed) -> Result<(), TemplateError> {
    let mut gone: Vec<String> = Vec::new();
    if !keep.macros {
        for dir in SCRIPT_DIRS {
            if entries.iter().any(|e| e.name.starts_with(dir)) {
                removed.macros += 1;
                entries.retain(|e| !e.name.starts_with(dir));
                gone.push(dir.trim_end_matches('/').to_string());
            }
        }
    }
    let object_types = manifest_types(entries);
    for part in ["content.xml", "styles.xml"] {
        let Some(src) = package::text_of(entries, part) else {
            continue;
        };
        if !odf_needs_work(&src, keep) {
            continue;
        }
        let mut nodes = xml::parse(&src).map_err(|e| broken(part, e))?;
        let mut walk = OdfWalk {
            keep,
            removed: &mut *removed,
            object_types: &object_types,
            dropped: Vec::new(),
            changed: false,
        };
        walk.children(&mut nodes);
        let dropped = std::mem::take(&mut walk.dropped);
        if walk.changed {
            package::set(entries, part, xml::to_xml(&nodes).into_bytes());
        }
        for path in dropped {
            let prefix = format!("{path}/");
            entries.retain(|e| e.name != path && !e.name.starts_with(&prefix));
            gone.push(path);
        }
    }
    if !gone.is_empty() {
        if let Some(src) = package::text_of(entries, MANIFEST) {
            let mut nodes = xml::parse(&src).map_err(|e| broken(MANIFEST, e))?;
            remove_where(&mut nodes, &mut |e| {
                e.name == "manifest:file-entry"
                    && e.attr("manifest:full-path").is_some_and(|p| {
                        let p = p.trim_end_matches('/');
                        gone.iter()
                            .any(|g| p == g || p.starts_with(&format!("{g}/")))
                    })
            });
            package::set(entries, MANIFEST, xml::to_xml(&nodes).into_bytes());
        }
    }
    Ok(())
}

/// The media type the manifest gives each directory, such as `Object 1`.
fn manifest_types(entries: &[Entry]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(src) = package::text_of(entries, MANIFEST) else {
        return out;
    };
    let Ok(nodes) = xml::parse(&src) else {
        return out;
    };
    if let Some(root) = xml::root(&nodes) {
        for e in root.children_named("manifest:file-entry") {
            if let (Some(path), Some(ty)) =
                (e.attr("manifest:full-path"), e.attr("manifest:media-type"))
            {
                out.insert(path.trim_end_matches('/').to_string(), ty);
            }
        }
    }
    out
}

static HREF_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"xlink:href="([^"]*)""#).unwrap());

/// A cheap look for anything the walk would change, since most parts hold
/// nothing of the kind and parsing a large one costs.
fn odf_needs_work(src: &str, keep: Keep) -> bool {
    let has = |s: &str| src.contains(s);
    (!keep.macros
        && (has("<office:scripts")
            || has("script:event-listener")
            || has("text:execute-macro")
            || has("vnd.sun.star.script")
            || has("macro:")))
        || (!keep.objects && (has("<draw:object") || has("<draw:applet") || has("<draw:plugin")))
        || (!keep.external_links && has("dde-"))
        || ((!keep.external_links || !keep.private_links)
            && HREF_RE.captures_iter(src).any(|c| {
                let href = xml::unescape(&c[1]).into_owned();
                outside(&href)
            }))
}

struct OdfWalk<'a> {
    keep: Keep,
    removed: &'a mut Removed,
    object_types: &'a HashMap<String, String>,
    /// Package paths of objects taken out.
    dropped: Vec<String>,
    changed: bool,
}

impl OdfWalk<'_> {
    fn children(&mut self, children: &mut Vec<Node>) {
        let mut i = 0;
        while i < children.len() {
            let action = match &mut children[i] {
                Node::Elem(e) => self.element(e),
                _ => Action::Keep,
            };
            match action {
                Action::Keep | Action::Replace(_) => i += 1,
                Action::Remove => {
                    self.changed = true;
                    children.remove(i);
                }
                Action::Unwrap => {
                    self.changed = true;
                    let Node::Elem(e) = children.remove(i) else {
                        continue;
                    };
                    let n = e.children.len();
                    children.splice(i..i, e.children);
                    i += n;
                }
            }
        }
    }

    fn take(&mut self, why: Why) -> Action {
        self.removed.count(why);
        Action::Remove
    }

    fn element(&mut self, e: &mut Elem) -> Action {
        let href = e.attr("xlink:href");
        let keep = self.keep;
        let name = e.name.clone();
        let name = name.as_str();
        if !keep.macros {
            match name {
                "office:scripts" | "script:event-listener" => return self.take(Why::Macro),
                "presentation:event-listener" if href.as_deref().is_some_and(is_macro_url) => {
                    return self.take(Why::Macro)
                }
                "text:execute-macro" => {
                    self.removed.macros += 1;
                    e.children.retain(
                        |n| !matches!(n, Node::Elem(c) if c.name == "office:event-listeners"),
                    );
                    self.children(&mut e.children);
                    return Action::Unwrap;
                }
                "text:a" | "draw:a" if href.as_deref().is_some_and(is_macro_url) => {
                    self.removed.macros += 1;
                    self.children(&mut e.children);
                    return Action::Unwrap;
                }
                _ => {}
            }
        }
        if !keep.objects {
            let object = match name {
                "draw:object-ole" | "draw:applet" | "draw:plugin" => true,
                "draw:object" => href.as_deref().is_some_and(|h| {
                    !outside(h)
                        && !self
                            .object_types
                            .get(&package_path(h))
                            .is_some_and(|t| KEPT_OBJECT_TYPES.contains(&t.as_str()))
                }),
                _ => false,
            };
            if object {
                if let Some(h) = href.as_deref().filter(|h| !outside(h)) {
                    self.dropped.push(package_path(h));
                }
                return self.take(Why::Object);
            }
        }
        if !keep.external_links {
            match name {
                "text:dde-connection-decls"
                | "table:dde-links"
                | "office:dde-source"
                | "text:dde-connection" => return self.take(Why::External),
                "draw:image"
                | "draw:object"
                | "draw:object-ole"
                | "draw:plugin"
                | "draw:fill-image"
                | "text:section-source"
                | "table:table-source"
                | "table:cell-range-source"
                    if href.as_deref().is_some_and(outside) =>
                {
                    return self.take(Why::External)
                }
                "style:background-image" | "text:list-level-style-image"
                    if href.as_deref().is_some_and(outside) =>
                {
                    self.removed.external_links += 1;
                    self.changed = true;
                    for attr in ["xlink:href", "xlink:type", "xlink:show", "xlink:actuate"] {
                        e.remove_attr(attr);
                    }
                }
                _ => {}
            }
        }
        if !keep.private_links
            && matches!(name, "text:a" | "draw:a")
            && href.as_deref().is_some_and(is_private_url)
        {
            self.removed.private_links += 1;
            self.children(&mut e.children);
            return Action::Unwrap;
        }
        let showed = shows_something(e);
        self.children(&mut e.children);
        // A frame left with nothing to show goes too.
        if e.name == "draw:frame" && showed && !shows_something(e) {
            return Action::Remove;
        }
        Action::Keep
    }
}

fn shows_something(frame: &Elem) -> bool {
    frame.children.iter().any(|n| {
        matches!(n, Node::Elem(c) if !matches!(
            c.name.as_str(),
            "svg:title" | "svg:desc" | "draw:contour-polygon" | "draw:contour-path"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_addresses_in_every_spelling() {
        for url in [
            "http://localhost/x",
            "https://LOCALHOST.:8080",
            "http://app.localhost",
            "http://127.0.0.1/",
            "http://127.1",
            "http://2130706433/",
            "http://0x7f000001/",
            "http://0177.0.0.1/",
            "http://10.2.3.4",
            "http://172.16.0.1",
            "http://172.31.255.255",
            "http://192.168.1.1/admin",
            "http://169.254.169.254/latest/meta-data",
            "http://0.0.0.0",
            "http://[::1]/",
            "http://[fc00::1]",
            "http://[fd12:3456::1]",
            "http://[fe80::1%25en0]",
            "http://[::ffff:127.0.0.1]",
            "http://user:pw@10.0.0.1:81/x",
            "http://printer.local/",
            "https://wiki.corp.internal/page",
            "http://%6c%6fcalhost/",
            "file:///etc/passwd",
            "FILE://server/share",
            "\\\\server\\share\\doc.docx",
            "smb://192.168.0.2/share",
        ] {
            assert!(is_private_url(url), "{url} is private");
        }
        for url in [
            "https://example.com",
            "http://172.32.0.1",
            "http://172.15.0.1",
            "http://11.0.0.1",
            "http://[2001:db8::1]",
            "mailto:a@localhost",
            "#bookmark",
            "doc.docx",
            "http://localhost.example.com",
            "http://192.169.0.1",
            "http://1.2.3.4.5",
        ] {
            assert!(!is_private_url(url), "{url} is public");
        }
    }

    #[test]
    fn rels_belong_to_their_parts() {
        assert_eq!(source_of("_rels/.rels").as_deref(), Some(""));
        assert_eq!(
            source_of("word/_rels/document.xml.rels").as_deref(),
            Some("word/document.xml")
        );
        assert_eq!(
            source_of("word/_rels/vbaProject.bin.rels").as_deref(),
            Some("word/vbaProject.bin")
        );
    }

    #[test]
    fn classifies_relationships() {
        let none = Keep::default();
        let rel = |t: &str| {
            format!("http://schemas.openxmlformats.org/officeDocument/2006/relationships/{t}")
        };
        let ms = |t: &str| format!("http://schemas.microsoft.com/office/2006/relationships/{t}");
        let src = "word/document.xml";
        assert_eq!(
            classify(&ms("vbaProject"), "vbaProject.bin", false, src, none),
            Some(Why::Macro)
        );
        assert_eq!(
            classify(&rel("oleObject"), "embeddings/o.bin", false, src, none),
            Some(Why::Object)
        );
        assert_eq!(
            classify(&rel("package"), "embeddings/a.xlsx", false, src, none),
            Some(Why::Object)
        );
        assert_eq!(
            classify(
                &rel("package"),
                "../embeddings/a.xlsx",
                false,
                "word/charts/chart1.xml",
                none
            ),
            None
        );
        assert_eq!(
            classify(&rel("image"), "http://x.example/a.png", true, src, none),
            Some(Why::External)
        );
        assert_eq!(
            classify(&rel("attachedTemplate"), "file:///t.dotm", true, src, none),
            Some(Why::External)
        );
        assert_eq!(
            classify(
                &rel("externalLink"),
                "externalLinks/externalLink1.xml",
                false,
                "xl/workbook.xml",
                none
            ),
            Some(Why::External)
        );
        assert_eq!(
            classify(&rel("hyperlink"), "http://10.0.0.1", true, src, none),
            Some(Why::Private)
        );
        assert_eq!(
            classify(&rel("hyperlink"), "https://example.com", true, src, none),
            None
        );
        assert_eq!(
            classify(&rel("image"), "media/image1.png", false, src, none),
            None
        );
        assert_eq!(
            classify(&ms("vbaProject"), "vbaProject.bin", false, src, Keep::ALL),
            None
        );
    }
}
