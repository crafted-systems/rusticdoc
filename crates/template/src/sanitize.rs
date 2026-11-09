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

/// Schemes a browser parses with the WHATWG "special" rules: backslashes are
/// slashes and the slashes after the colon may be missing or doubled.
const SPECIAL_SCHEMES: &[&str] = &["http", "https", "ws", "wss", "ftp"];

/// Whether a link points into the reader's own machine or network: `file:`,
/// drive and UNC paths, `localhost`, `.local` and `.internal` names, and
/// loopback, private, shared, link-local, unique-local, benchmark, multicast
/// and reserved addresses, however they are spelt. Web addresses are read as
/// a browser reads them (the WHATWG URL rules, through the `url` crate), so
/// `http:\\10.0.0.1`, `http:/10.0.0.1`, tabs and line breaks inside the
/// address, and IPv4 in decimal, octal or hex all count. An address that a
/// browser would refuse to parse counts as private: what cannot be read is
/// not let through.
pub fn is_private_url(url: &str) -> bool {
    // WHATWG removes these anywhere in the input, then trims C0 and spaces.
    let cleaned: String = url
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let s = cleaned.trim_matches(|c: char| c <= ' ');
    if s.starts_with("\\\\") || s.starts_with("/\\") || s.starts_with("\\/") {
        // A UNC path: a share on the local network.
        return true;
    }
    if let Some(rest) = s.strip_prefix("//") {
        // Scheme-relative: whatever scheme the reader assumes, the host counts.
        return private_special(&format!("http://{rest}"));
    }
    let Some(colon) = s.find(':') else {
        return false;
    };
    let scheme = &s[..colon];
    let valid = scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !valid {
        return false;
    }
    let scheme = scheme.to_ascii_lowercase();
    // `C:\docs\x.docx`: a drive letter, so a local file.
    if scheme == "file" || scheme.len() == 1 {
        return true;
    }
    if SPECIAL_SCHEMES.contains(&scheme.as_str()) {
        return private_special(s);
    }
    // Other schemes (`smb:`, `ldap:`) name a host only after `//`.
    match url::Url::parse(s) {
        Ok(u) => match u.host() {
            Some(url::Host::Domain(d)) => is_private_host(&percent_decode(d)),
            Some(url::Host::Ipv4(ip)) => private_v4(ip),
            Some(url::Host::Ipv6(ip)) => private_v6(ip),
            None => s[colon + 1..].starts_with("//"),
        },
        Err(_) => s[colon + 1..].starts_with("//"),
    }
}

fn private_special(s: &str) -> bool {
    match url::Url::parse(s) {
        Ok(u) => match u.host() {
            Some(url::Host::Domain(d)) => is_private_host(d),
            Some(url::Host::Ipv4(ip)) => private_v4(ip),
            Some(url::Host::Ipv6(ip)) => private_v6(ip),
            None => true,
        },
        Err(_) => true,
    }
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
    let [a, b, c, _] = ip.octets();
    a == 0 // this network
        || a == 10
        || a == 127
        || (a == 100 && (64..128).contains(&b)) // shared address space
        || (a == 169 && b == 254)
        || (a == 172 && (16..32).contains(&b))
        || (a == 192 && b == 0 && c == 0) // protocol assignments
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19)) // benchmarking
        || a >= 224 // multicast, reserved and broadcast
}

fn private_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    let embedded = |hi: u16, lo: u16| {
        let [a, b] = hi.to_be_bytes();
        let [c, d] = lo.to_be_bytes();
        Ipv4Addr::new(a, b, c, d)
    };
    // IPv4-mapped (::ffff:0:0/96) and IPv4-compatible (::/96) addresses.
    if s[..5] == [0; 5] && (s[5] == 0xffff || s[5] == 0) {
        return ip.is_loopback() || ip.is_unspecified() || private_v4(embedded(s[6], s[7]));
    }
    // NAT64 (64:ff9b::/96) and 6to4 (2002::/16) carry an IPv4 address.
    if s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0; 4] {
        return private_v4(embedded(s[6], s[7]));
    }
    if s[0] == 0x2002 {
        return private_v4(embedded(s[1], s[2]));
    }
    (s[0] & 0xfe00) == 0xfc00 // unique local
        || (s[0] & 0xffc0) == 0xfe80 // link local
        || (s[0] & 0xffc0) == 0xfec0 // site local
        || (s[0] & 0xff00) == 0xff00 // multicast
        || (s[0] == 0x100 && s[1..4] == [0; 3]) // discard
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
// Reading parts. The sanitiser fails closed: a part it must look at but
// cannot read refuses the document rather than passing it through unseen.

/// A part's text, read as Office reads it: UTF-8, or UTF-16 with or without
/// a byte-order mark. Its XML declaration is made to say UTF-8, since that is
/// what any rewritten part is. `None` when the part does not exist.
fn read_part(entries: &[Entry], name: &str) -> Result<Option<String>, TemplateError> {
    let Some(e) = entries.iter().find(|e| e.name == name) else {
        return Ok(None);
    };
    decode(&e.data).map(Some).ok_or_else(|| {
        TemplateError::new(format!(
            "{name}: the part is neither UTF-8 nor UTF-16 text, so it cannot be checked \
             for macros, objects and links; the document is refused"
        ))
    })
}

fn decode(b: &[u8]) -> Option<String> {
    let utf16 = |le: bool, rest: &[u8]| -> Option<String> {
        let (pairs, odd) = rest.as_chunks::<2>();
        if !odd.is_empty() {
            return None;
        }
        let units: Vec<u16> = pairs
            .iter()
            .map(|c| {
                if le {
                    u16::from_le_bytes(*c)
                } else {
                    u16::from_be_bytes(*c)
                }
            })
            .collect();
        String::from_utf16(&units).ok()
    };
    let text = if let Some(rest) = b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8(rest.to_vec()).ok()?
    } else if let Some(rest) = b.strip_prefix(&[0xFF, 0xFE]) {
        utf16(true, rest)?
    } else if let Some(rest) = b.strip_prefix(&[0xFE, 0xFF]) {
        utf16(false, rest)?
    } else if b.starts_with(&[b'<', 0]) {
        utf16(true, b)?
    } else if b.starts_with(&[0, b'<']) {
        utf16(false, b)?
    } else {
        String::from_utf8(b.to_vec()).ok()?
    };
    Some(declare_utf8(text))
}

static ENCODING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^(<\?xml[^>]*?encoding\s*=\s*)("[^"]*"|'[^']*')"#).unwrap());

fn declare_utf8(text: String) -> String {
    match ENCODING_RE.captures(&text) {
        Some(c) if !c[2].eq_ignore_ascii_case("\"utf-8\"") => {
            let end = c.get(0).map_or(0, |m| m.end());
            format!("{}\"UTF-8\"{}", &c[1], &text[end..])
        }
        _ => text,
    }
}

/// The namespaces the sanitiser recognises markup by, with the prefixes
/// Office and LibreOffice always give them. Elements are matched by these
/// qualified names, so a part that binds one of these namespaces to another
/// prefix, or one of these prefixes to another namespace, is refused: it
/// would otherwise hide a macro or link under a name that is not looked for.
/// The template engine reads the same fixed prefixes.
const KNOWN_NAMESPACES: &[(&str, &str)] = &[
    (
        "",
        "http://schemas.openxmlformats.org/package/2006/relationships",
    ),
    (
        "",
        "http://schemas.openxmlformats.org/package/2006/content-types",
    ),
    (
        "",
        "http://schemas.openxmlformats.org/spreadsheetml/2006/main",
    ),
    ("", "http://purl.oclc.org/ooxml/spreadsheetml/main"),
    (
        "r",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    ),
    (
        "r",
        "http://purl.oclc.org/ooxml/officeDocument/relationships",
    ),
    (
        "w",
        "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
    ),
    ("w", "http://purl.oclc.org/ooxml/wordprocessingml/main"),
    ("a", "http://schemas.openxmlformats.org/drawingml/2006/main"),
    ("a", "http://purl.oclc.org/ooxml/drawingml/main"),
    (
        "p",
        "http://schemas.openxmlformats.org/presentationml/2006/main",
    ),
    ("p", "http://purl.oclc.org/ooxml/presentationml/main"),
    ("v", "urn:schemas-microsoft-com:vml"),
    ("o", "urn:schemas-microsoft-com:office:office"),
    ("office", "urn:oasis:names:tc:opendocument:xmlns:office:1.0"),
    ("text", "urn:oasis:names:tc:opendocument:xmlns:text:1.0"),
    ("table", "urn:oasis:names:tc:opendocument:xmlns:table:1.0"),
    ("draw", "urn:oasis:names:tc:opendocument:xmlns:drawing:1.0"),
    ("style", "urn:oasis:names:tc:opendocument:xmlns:style:1.0"),
    (
        "svg",
        "urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0",
    ),
    ("script", "urn:oasis:names:tc:opendocument:xmlns:script:1.0"),
    (
        "presentation",
        "urn:oasis:names:tc:opendocument:xmlns:presentation:1.0",
    ),
    (
        "manifest",
        "urn:oasis:names:tc:opendocument:xmlns:manifest:1.0",
    ),
    ("xlink", "http://www.w3.org/1999/xlink"),
];

static XMLNS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\sxmlns(?::([^\s=/>]+))?\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap()
});

/// Refuses a part whose namespace declarations would let markup hide from
/// the names the sanitiser looks for.
fn check_namespaces(part: &str, src: &str) -> Result<(), TemplateError> {
    for c in XMLNS_RE.captures_iter(src) {
        let prefix = c.get(1).map_or("", |m| m.as_str());
        let raw = c.get(2).or(c.get(3)).map_or("", |m| m.as_str());
        let uri = xml::unescape(raw);
        let uri = uri.trim();
        let wanted: Vec<&str> = KNOWN_NAMESPACES
            .iter()
            .filter(|(_, u)| *u == uri)
            .map(|(p, _)| *p)
            .collect();
        let uris: Vec<&str> = KNOWN_NAMESPACES
            .iter()
            .filter(|(p, _)| !p.is_empty() && *p == prefix)
            .map(|(_, u)| *u)
            .collect();
        let bad_uri = !wanted.is_empty() && !wanted.contains(&prefix);
        let bad_prefix = !uris.is_empty() && !uris.contains(&uri);
        if bad_uri || bad_prefix {
            let shown = if prefix.is_empty() {
                "the default namespace".to_string()
            } else {
                format!("the prefix \"{prefix}\"")
            };
            return Err(TemplateError::new(format!(
                "{part}: {shown} is bound to {uri}, which Office and LibreOffice never write; \
                 the sanitiser reads the standard prefixes only, so the document is refused"
            )));
        }
    }
    Ok(())
}

/// A part read and checked: its text, decoded, with standard prefixes.
fn checked_part(entries: &[Entry], name: &str) -> Result<Option<String>, TemplateError> {
    let Some(src) = read_part(entries, name)? else {
        return Ok(None);
    };
    check_namespaces(name, &src)?;
    Ok(Some(src))
}

/// Refuses a package with two entries whose names differ only in case:
/// OPC part names are case-insensitive, so which one a reader takes is not
/// known, and the sanitiser could clean the other.
fn check_case(entries: &[Entry]) -> Result<(), TemplateError> {
    let mut seen: HashSet<String> = HashSet::new();
    for e in entries {
        if !seen.insert(e.name.to_ascii_lowercase()) {
            return Err(TemplateError::new(format!(
                "{}: two parts have this name in different cases; the document is refused",
                e.name
            )));
        }
    }
    Ok(())
}

/// The entry with this name, compared without case.
fn entry_name(entries: &[Entry], name: &str) -> Option<String> {
    entries
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case(name))
        .map(|e| e.name.clone())
}

/// `(type, target, external)` of each relationship in a relationships file,
/// internal targets resolved against `base` and in lower case.
fn relationships(
    entries: &[Entry],
    rels: &str,
    base: &str,
) -> Result<Vec<(String, String, bool)>, TemplateError> {
    let Some(src) = checked_part(entries, rels)? else {
        return Ok(Vec::new());
    };
    let nodes = xml::parse(&src).map_err(|e| broken(rels, e))?;
    let Some(root) = xml::root(&nodes) else {
        return Ok(Vec::new());
    };
    Ok(root
        .children_named("Relationship")
        .filter_map(|r| {
            let ty = r.attr("Type")?;
            let target = r.attr("Target")?;
            let external = r
                .attr("TargetMode")
                .is_some_and(|m| m.eq_ignore_ascii_case("External"));
            let target = if external {
                target
            } else {
                package::resolve(base, &target).to_ascii_lowercase()
            };
            Some((ty, target, external))
        })
        .collect())
}

fn is_rels(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".rels")
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

/// A reference into a linked workbook, `[1]Sheet1!A1`, as XLSX stores one.
/// A table reference such as `Table1[[#This Row],[2019]]` is followed by `]`
/// or `:` and is not one.
static EXTERNAL_REF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\d+\]([^\]:]|$)").unwrap());
/// A reference into another file in ODF: `['file:///x.ods'#$Sheet1.A1]`.
static ODF_EXTERNAL_REF_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"'[^']*'#").unwrap());
/// Functions that reach outside the file or run code.
static OUTSIDE_FN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(^|[^A-Za-z0-9_.])(WEBSERVICE|RTD|CALL|REGISTER|REGISTER\.ID|EXEC|DDE|IMPORTDATA|IMPORTXML|IMPORTHTML|IMPORTFEED|IMPORTRANGE)\s*\(").unwrap()
});
/// A DDE reference, `cmd|'/c calc'!A0`: the pipe appears nowhere else
/// outside strings.
static DDE_REF_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\|").unwrap());
static HYPERLINK_FN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(^|[^A-Za-z0-9_.])HYPERLINK\s*\(").unwrap());
static STRING_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""(?:[^"]|"")*""#).unwrap());
static LITERAL_ARG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*"((?:[^"]|"")*)"\s*[,;)]"#).unwrap());

/// Why a formula must go, if it must. Function names are looked for outside
/// string literals; a `HYPERLINK` passes only with a plain, public literal.
fn formula_threat(formula: &str, keep: Keep) -> Option<Why> {
    let code = STRING_RE.replace_all(formula, "\"\"");
    if !keep.external_links
        && (EXTERNAL_REF_RE.is_match(&code)
            || ODF_EXTERNAL_REF_RE.is_match(formula)
            || OUTSIDE_FN_RE.is_match(&code)
            || DDE_REF_RE.is_match(&code))
    {
        return Some(Why::External);
    }
    if !keep.private_links {
        for m in HYPERLINK_FN_RE.find_iter(&code) {
            // The same place in the original, where the literal still is.
            let rest = &formula[m.end()..];
            match LITERAL_ARG_RE.captures(rest) {
                Some(c) if !is_private_url(&c[1].replace("\"\"", "\"")) => {}
                _ => return Some(Why::Private),
            }
        }
    }
    None
}

/// Why a Word field must go, if it must. `nested` says the instruction held
/// another field, so its target is computed and cannot be checked.
fn field_threat(instr: &str, nested: bool, keep: Keep) -> Option<Why> {
    let instr = instr.trim();
    let word: String = instr
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '"')
        .collect::<String>()
        .to_ascii_uppercase();
    match word.as_str() {
        "HYPERLINK" if !keep.private_links => {
            if nested {
                return Some(Why::Private);
            }
            let target = field_argument(&instr[word.len()..]);
            match target {
                Some(t) if is_private_url(&t) => Some(Why::Private),
                _ => None,
            }
        }
        "INCLUDEPICTURE" | "INCLUDETEXT" | "INCLUDE" | "IMPORT" | "LINK" | "DDE" | "DDEAUTO"
            if !keep.external_links =>
        {
            Some(Why::External)
        }
        _ => None,
    }
}

/// The first argument of a field that is not a switch, quoted or bare.
fn field_argument(rest: &str) -> Option<String> {
    let mut chars = rest.trim_start().chars().peekable();
    loop {
        match chars.peek()? {
            '"' => {
                chars.next();
                return Some(chars.take_while(|c| *c != '"').collect());
            }
            '\\' => {
                // A switch and, for `\l`, `\o` and the like, its argument:
                // a bookmark or tip, not the target.
                chars.next();
                let _: String = chars.by_ref().take_while(|c| !c.is_whitespace()).collect();
                while chars.peek().is_some_and(|c| c.is_whitespace()) {
                    chars.next();
                }
                if chars.peek() == Some(&'"') {
                    chars.next();
                    let _: String = chars.by_ref().take_while(|c| *c != '"').collect();
                }
            }
            c if c.is_whitespace() => {
                chars.next();
            }
            _ => return Some(chars.take_while(|c| !c.is_whitespace()).collect()),
        }
    }
}

/// Takes the complex fields `field_threat` picks out of a Word part: the
/// field characters and instruction runs go, the result runs stay. Fields
/// nested in the instruction go with it. Runs are only emptied, never
/// removed, so the paths found first stay good.
fn complex_fields(nodes: &mut [Node], keep: Keep) -> Vec<Why> {
    let mut runs: Vec<Vec<usize>> = Vec::new();
    collect_runs(nodes, &mut Vec::new(), &mut runs);
    struct Field {
        begin: (usize, usize),
        separate: Option<(usize, usize)>,
        code: String,
        nested: bool,
    }
    /// A child of a run: `(run, child)` indexes.
    type At = (usize, usize);
    let mut open: Vec<Field> = Vec::new();
    // `(from, to, end)` positions to clear for each field that must go.
    let mut doomed: Vec<(At, At, At)> = Vec::new();
    let mut found = Vec::new();
    for (ri, path) in runs.iter().enumerate() {
        let Some(run) = xml::elem_at(nodes, path) else {
            continue;
        };
        for (ci, child) in run.children.iter().enumerate() {
            let Node::Elem(c) = child else { continue };
            match c.name.as_str() {
                "w:fldChar" => match c.attr("w:fldCharType").as_deref() {
                    Some("begin") => {
                        if let Some(top) = open.last_mut() {
                            if top.separate.is_none() {
                                top.nested = true;
                            }
                        }
                        open.push(Field {
                            begin: (ri, ci),
                            separate: None,
                            code: String::new(),
                            nested: false,
                        });
                    }
                    Some("separate") => {
                        if let Some(top) = open.last_mut() {
                            top.separate.get_or_insert((ri, ci));
                        }
                    }
                    Some("end") => {
                        if let Some(f) = open.pop() {
                            if let Some(why) = field_threat(&f.code, f.nested, keep) {
                                found.push(why);
                                doomed.push((f.begin, f.separate.unwrap_or((ri, ci)), (ri, ci)));
                            }
                        }
                    }
                    _ => {}
                },
                "w:instrText" => {
                    if let Some(top) = open.last_mut() {
                        if top.separate.is_none() {
                            top.code.push_str(&c.text_content());
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let clear = |at: (usize, usize)| {
        doomed
            .iter()
            .any(|(from, to, end)| (at >= *from && at <= *to) || at == *end)
    };
    // Last first: a run inside a text box sits under an earlier run, and
    // emptying that one first would move it.
    for (ri, path) in runs.iter().enumerate().rev() {
        let Some(run) = xml::elem_at_mut(nodes, path) else {
            continue;
        };
        let mut ci = 0;
        run.children.retain(|n| {
            let here = (ri, ci);
            ci += 1;
            let keep_it = matches!(n, Node::Elem(e) if e.name == "w:rPr") || !clear(here);
            keep_it || matches!(n, Node::Text(_))
        });
    }
    found
}

/// Paths of every `w:r`, in document order.
fn collect_runs(nodes: &[Node], path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
    for (i, n) in nodes.iter().enumerate() {
        if let Node::Elem(e) = n {
            path.push(i);
            if e.name == "w:r" {
                out.push(path.clone());
            }
            collect_runs(&e.children, path, out);
            path.pop();
        }
    }
}

/// The part a relationships file belongs to: `word/_rels/document.xml.rels`
/// is `word/document.xml`'s, `_rels/.rels` the package's (an empty name).
/// Part names are compared without case, as OPC says.
fn source_of(rels: &str) -> Option<String> {
    let (dir, file) = rels.rsplit_once('/')?;
    if file.len() < 5 || !file[file.len() - 5..].eq_ignore_ascii_case(".rels") {
        return None;
    }
    let file = &file[..file.len() - 5];
    if dir.len() < 5 || !dir[dir.len() - 5..].eq_ignore_ascii_case("_rels") {
        return None;
    }
    let dir = dir[..dir.len() - 5].trim_end_matches('/');
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

/// Relationship types the sanitiser cannot clean safely, so a package that
/// holds one is refused unless the caller keeps that kind: Excel 4.0 macro
/// and dialog sheets (removing one renumbers the sheets formulas point at),
/// and data connections with their query tables.
const REFUSED_MACRO_TYPES: &[&str] = &["xlmacrosheet", "xlintlmacrosheet", "dialogsheet"];
const REFUSED_LINK_TYPES: &[&str] = &["connections", "querytable"];

/// Why a relationship must go, if it must. Types are compared without case.
fn classify(ty: &str, target: &str, external: bool, source: &str, keep: Keep) -> Option<Why> {
    let kind = last_segment(ty).to_ascii_lowercase();
    let kind = kind.as_str();
    let source = source.to_ascii_lowercase();
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
    if kind == "externallink" {
        return (!keep.external_links).then_some(Why::External);
    }
    if VBA_TYPES.iter().any(|t| t.eq_ignore_ascii_case(kind)) {
        return (!keep.macros).then_some(Why::Macro);
    }
    // A chart's own workbook is its data, not an object on the page.
    if OBJECT_TYPES.iter().any(|t| t.eq_ignore_ascii_case(kind))
        && !(kind == "package" && source.contains("/charts/"))
    {
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
    check_case(entries)?;
    let mut ctx = Ooxml {
        entries,
        keep,
        removed,
    };
    // 1. Relationships: drop those that must go, noting their ids and targets.
    // Every relationships file is read and checked, whatever its case, and
    // one that cannot be read refuses the document.
    let mut lost: HashMap<String, HashMap<String, Why>> = HashMap::new();
    let mut doomed: BTreeSet<String> = BTreeSet::new();
    let mut dropped: Vec<(String, Why)> = Vec::new();
    let mut refused: Option<String> = None;
    let rels_files: Vec<String> = ctx
        .entries
        .iter()
        .filter(|e| is_rels(&e.name))
        .map(|e| e.name.clone())
        .collect();
    for rels in &rels_files {
        let Some(src) = checked_part(ctx.entries, rels)? else {
            continue;
        };
        let Some(source) = source_of(rels) else {
            continue;
        };
        // The source as the package names it, so that edits reach it.
        let source = entry_name(ctx.entries, &source).unwrap_or(source);
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
            let kind = last_segment(&ty).to_ascii_lowercase();
            if (!keep.macros && REFUSED_MACRO_TYPES.contains(&kind.as_str()))
                || (!keep.external_links && REFUSED_LINK_TYPES.contains(&kind.as_str()))
            {
                refused.get_or_insert(format!(
                    "{rels}: the package holds a {} relationship, which the sanitiser cannot \
                     take out safely; the document is refused",
                    last_segment(&ty)
                ));
            }
            let external = r
                .attr("TargetMode")
                .is_some_and(|m| m.eq_ignore_ascii_case("External"));
            let Some(why) = classify(&ty, &target, external, &source, keep) else {
                return true;
            };
            dropped.push((source.clone(), why));
            lost.entry(source.clone()).or_default().insert(id, why);
            if !external {
                doomed.insert(package::resolve(&base, &target).to_ascii_lowercase());
            }
            changed = true;
            false
        });
        if let Some(message) = refused.take() {
            return Err(TemplateError::new(message));
        }
        if changed {
            package::set(ctx.entries, rels, xml::to_xml(&nodes).into_bytes());
        }
    }
    // A macro project nothing points at still goes, found by name or type.
    if !keep.macros {
        for e in ctx.entries.iter() {
            if e.name.to_ascii_lowercase().ends_with("vbaproject.bin") {
                doomed.insert(e.name.to_ascii_lowercase());
            }
        }
        doomed.extend(ctx.macro_parts()?);
    }

    // 2. Parts nothing points at any more go, with what only they point at.
    let gone = ctx.drop_parts(doomed)?;
    // A part that went with another, such as an ActiveX control's binary,
    // is not counted again.
    for (source, why) in dropped {
        if !gone.contains(&source.to_ascii_lowercase()) {
            ctx.removed.count(why);
        }
    }

    // 3. Markup that named a dropped relationship.
    let mut sources: Vec<&String> = lost
        .keys()
        .filter(|s| !gone.contains(&s.to_ascii_lowercase()))
        .collect();
    sources.sort();
    for source in sources {
        ctx.edit_part(source, &lost[source])?;
    }
    if format == Format::Docx && (!keep.private_links || !keep.external_links) {
        ctx.word_fields()?;
    }
    if format == Format::Xlsx && (!keep.private_links || !keep.external_links) {
        ctx.sheet_formulas()?;
    }

    // 4. Content types.
    ctx.content_types(&gone)?;
    Ok(())
}

impl Ooxml<'_> {
    /// Removes the doomed parts that no remaining relationship names, then
    /// whatever only those parts named, until nothing more goes.
    /// Part names in the sets are lower case, since OPC compares them so.
    fn drop_parts(
        &mut self,
        mut doomed: BTreeSet<String>,
    ) -> Result<HashSet<String>, TemplateError> {
        let mut gone: HashSet<String> = HashSet::new();
        loop {
            // Targets named from parts still there. A doomed part's own
            // targets join the doomed only once it is gone.
            let mut named: HashSet<String> = HashSet::new();
            let rels_files: Vec<String> = self
                .entries
                .iter()
                .filter(|e| is_rels(&e.name))
                .map(|e| e.name.clone())
                .collect();
            for rels in rels_files {
                let Some(source) = source_of(&rels) else {
                    continue;
                };
                if gone.contains(&source.to_ascii_lowercase()) {
                    continue;
                }
                for (_, target, _) in relationships(self.entries, &rels, base_of(&source))? {
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
                if let Some(rels_entry) = entry_name(self.entries, &rels) {
                    for (ty, target, external) in
                        relationships(self.entries, &rels_entry, base_of(&part))?
                    {
                        // A part's own hyperlinks are not parts.
                        if !external && !last_segment(&ty).eq_ignore_ascii_case("hyperlink") {
                            doomed.insert(target);
                        }
                    }
                }
                let rels = rels.to_ascii_lowercase();
                self.entries.retain(|e| {
                    let lower = e.name.to_ascii_lowercase();
                    lower != part && lower != rels
                });
                gone.insert(part.clone());
                gone.insert(rels);
            }
        }
        Ok(gone)
    }

    fn edit_part(&mut self, part: &str, ids: &HashMap<String, Why>) -> Result<(), TemplateError> {
        if !part.to_ascii_lowercase().ends_with(".xml") {
            return Ok(());
        }
        let Some(src) = checked_part(self.entries, part)? else {
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

    /// Word fields that link out or pull content in: `HYPERLINK` to a private
    /// address, and `INCLUDEPICTURE`, `INCLUDETEXT`, `LINK`, `DDE` and
    /// `DDEAUTO`, whether written as simple fields or as complex ones (field
    /// characters around instruction runs). The field goes and its last
    /// result stays as plain text.
    fn word_fields(&mut self) -> Result<(), TemplateError> {
        let names: Vec<String> = self
            .entries
            .iter()
            .filter(|e| {
                let lower = e.name.to_ascii_lowercase();
                lower.starts_with("word/") && lower.ends_with(".xml")
            })
            .map(|e| e.name.clone())
            .collect();
        let keep = self.keep;
        for name in names {
            let Some(src) = checked_part(self.entries, &name)? else {
                continue;
            };
            if !src.contains("w:fldSimple") && !src.contains("w:fldChar") {
                continue;
            }
            let mut nodes = xml::parse(&src).map_err(|e| broken(&name, e))?;
            let mut found: Vec<Why> = Vec::new();
            unwrap_where(&mut nodes, &mut |e| {
                if e.name != "w:fldSimple" {
                    return false;
                }
                let why = e
                    .attr("w:instr")
                    .and_then(|i| field_threat(&i, false, keep));
                found.extend(why);
                why.is_some()
            });
            found.extend(complex_fields(&mut nodes, keep));
            if !found.is_empty() {
                for why in found {
                    self.removed.count(why);
                }
                package::set(self.entries, &name, xml::to_xml(&nodes).into_bytes());
            }
        }
        Ok(())
    }

    /// Formulas that reach outside the workbook keep only their last value
    /// (rendering drops cached values so that Excel recomputes them, so a
    /// rendered cell is left empty): references into linked workbooks,
    /// `WEBSERVICE`, `RTD`, `CALL` and `REGISTER`, DDE references, and
    /// `HYPERLINK` to a private address or to anything but a plain text
    /// literal, which could build one.
    fn sheet_formulas(&mut self) -> Result<(), TemplateError> {
        let keep = self.keep;
        let names: Vec<String> = self
            .entries
            .iter()
            .filter(|e| {
                let lower = e.name.to_ascii_lowercase();
                lower.starts_with("xl/") && lower.ends_with(".xml") && !lower.contains("/_rels/")
            })
            .map(|e| e.name.clone())
            .collect();
        for name in names {
            let Some(src) = checked_part(self.entries, &name)? else {
                continue;
            };
            if !src.contains("<f") && !src.contains("<definedName") {
                continue;
            }
            let mut nodes = xml::parse(&src).map_err(|e| broken(&name, e))?;
            let mut shared_gone: HashSet<String> = HashSet::new();
            let mut found: Vec<Why> = Vec::new();
            // Shared formulas: the first cell holds the text, the others its
            // index, so the others go with it.
            remove_where(&mut nodes, &mut |e| {
                if e.name != "f" && e.name != "definedName" {
                    return false;
                }
                let Some(why) = formula_threat(&e.text_content(), keep) else {
                    return false;
                };
                if e.attr("t").as_deref() == Some("shared") {
                    if let Some(si) = e.attr("si") {
                        shared_gone.insert(si);
                    }
                }
                found.push(why);
                true
            });
            if !shared_gone.is_empty() {
                remove_where(&mut nodes, &mut |e| {
                    e.name == "f"
                        && e.attr("t").as_deref() == Some("shared")
                        && e.attr("si").is_some_and(|si| shared_gone.contains(&si))
                });
            }
            if !found.is_empty() {
                for why in found {
                    self.removed.count(why);
                }
                remove_empty(&mut nodes, &["definedNames"]);
                package::set(self.entries, &name, xml::to_xml(&nodes).into_bytes());
            }
        }
        Ok(())
    }

    fn content_types(&mut self, gone: &HashSet<String>) -> Result<(), TemplateError> {
        let Some(types) = entry_name(self.entries, "[Content_Types].xml") else {
            return Ok(());
        };
        let Some(src) = checked_part(self.entries, &types)? else {
            return Ok(());
        };
        let mut nodes = xml::parse(&src).map_err(|e| broken(&types, e))?;
        let Some(root) = xml::root_mut(&mut nodes) else {
            return Ok(());
        };
        let before = xml::to_xml(std::slice::from_ref(&Node::Elem(root.clone())));
        let names: HashSet<String> = self
            .entries
            .iter()
            .map(|e| e.name.to_ascii_lowercase())
            .collect();
        let overridden: HashSet<String> = root
            .children_named("Override")
            .filter_map(|o| o.attr("PartName"))
            .map(|p| p.trim_start_matches('/').to_ascii_lowercase())
            .collect();
        let macros_gone = !self.keep.macros;
        root.children.retain(|n| {
            let Node::Elem(e) = n else { return true };
            match e.name.as_str() {
                "Override" => e.attr("PartName").is_none_or(|p| {
                    let p = p.trim_start_matches('/').to_ascii_lowercase();
                    !gone.contains(&p) && names.contains(&p)
                }),
                // A default for macro projects stays only while a part uses it.
                "Default"
                    if macros_gone
                        && e.attr("ContentType")
                            .is_some_and(|t| t.trim().eq_ignore_ascii_case(VBA_CONTENT_TYPE)) =>
                {
                    let ext = e.attr("Extension").unwrap_or_default().to_ascii_lowercase();
                    names
                        .iter()
                        .any(|n| n.ends_with(&format!(".{ext}")) && !overridden.contains(n))
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
                let ct = ct.trim();
                if let Some((_, plain)) = MACRO_MAIN_TYPES
                    .iter()
                    .find(|(m, _)| m.eq_ignore_ascii_case(ct))
                {
                    e.set_attr("ContentType", plain);
                }
            }
        }
        let after = xml::to_xml(std::slice::from_ref(&Node::Elem(root.clone())));
        if before != after {
            package::set(self.entries, &types, xml::to_xml(&nodes).into_bytes());
        }
        Ok(())
    }

    /// Parts the content types call macro projects, whatever their name.
    fn macro_parts(&self) -> Result<Vec<String>, TemplateError> {
        let Some(types) = entry_name(self.entries, "[Content_Types].xml") else {
            return Ok(Vec::new());
        };
        let Some(src) = checked_part(self.entries, &types)? else {
            return Ok(Vec::new());
        };
        let nodes = xml::parse(&src).map_err(|e| broken(&types, e))?;
        let Some(root) = xml::root(&nodes) else {
            return Ok(Vec::new());
        };
        let is_vba = |e: &Elem| {
            e.attr("ContentType")
                .is_some_and(|t| t.trim().eq_ignore_ascii_case(VBA_CONTENT_TYPE))
        };
        let mut out: Vec<String> = root
            .children_named("Override")
            .filter(|e| is_vba(e))
            .filter_map(|e| e.attr("PartName"))
            .map(|p| p.trim_start_matches('/').to_ascii_lowercase())
            .collect();
        let overridden: HashSet<String> = root
            .children_named("Override")
            .filter_map(|e| e.attr("PartName"))
            .map(|p| p.trim_start_matches('/').to_ascii_lowercase())
            .collect();
        for d in root.children_named("Default").filter(|e| is_vba(e)) {
            let ext = format!(
                ".{}",
                d.attr("Extension").unwrap_or_default().to_ascii_lowercase()
            );
            out.extend(
                self.entries
                    .iter()
                    .map(|e| e.name.to_ascii_lowercase())
                    .filter(|n| n.ends_with(&ext) && !overridden.contains(n)),
            );
        }
        Ok(out)
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

/// An `xlink:href` with what readers ignore taken out.
fn clean_href(href: &str) -> String {
    let s: String = href
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    s.trim_matches(|c: char| c <= ' ').to_string()
}

/// Whether an `xlink:href` in ODF points outside the package.
fn outside(href: &str) -> bool {
    let href = clean_href(href);
    if href.starts_with("../")
        || href.starts_with("..\\")
        || href.starts_with('/')
        || href.starts_with('\\')
    {
        return true;
    }
    match href.find(':') {
        Some(colon) => {
            let scheme = &href[..colon];
            !scheme.is_empty()
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
                && !scheme.eq_ignore_ascii_case("vnd.sun.star.package")
        }
        None => false,
    }
}

fn is_macro_url(href: &str) -> bool {
    let h = clean_href(href).to_ascii_lowercase();
    h.starts_with("vnd.sun.star.script:") || h.starts_with("macro:")
}

/// The package path an internal `xlink:href` names, from the directory of
/// the part that holds it (`Object 1/` for an embedded object's content).
fn package_path(base: &str, href: &str) -> String {
    let h = clean_href(href);
    let h = h.strip_prefix("vnd.sun.star.package:").unwrap_or(&h);
    let h = h.trim_start_matches("./").trim_end_matches('/');
    format!("{base}{h}")
}

/// Whether a path lies in a script library at any depth: `Basic/`,
/// `Scripts/` or `Dialogs/`, in any case.
fn in_script_dir(path: &str) -> bool {
    path.split('/').rev().skip(1).any(|dir| {
        SCRIPT_DIRS
            .iter()
            .any(|d| d.trim_end_matches('/').eq_ignore_ascii_case(dir))
    })
}

fn odf(entries: &mut Vec<Entry>, keep: Keep, removed: &mut Removed) -> Result<(), TemplateError> {
    let mut gone: Vec<String> = Vec::new();
    if !keep.macros {
        let dirs: BTreeSet<String> = entries
            .iter()
            .filter(|e| in_script_dir(&e.name))
            .map(|e| e.name.clone())
            .collect();
        if !dirs.is_empty() {
            removed.macros += 1;
            entries.retain(|e| !dirs.contains(&e.name));
            gone.extend(dirs);
        }
    }
    let object_types = manifest_types(entries)?;
    let kept_objects = verified_objects(entries, &object_types)?;
    // The document's own parts and those of every embedded document left.
    let parts: Vec<String> = entries
        .iter()
        .map(|e| e.name.clone())
        .filter(|n| {
            let file = n.rsplit('/').next().unwrap_or(n);
            file == "content.xml" || file == "styles.xml"
        })
        .collect();
    for part in parts {
        if gone.iter().any(|g| part.starts_with(&format!("{g}/"))) {
            continue;
        }
        let Some(src) = checked_part(entries, &part)? else {
            continue;
        };
        if !odf_needs_work(&src, keep) {
            continue;
        }
        let base = part
            .rsplit_once('/')
            .map_or(String::new(), |(d, _)| format!("{d}/"));
        let mut nodes = xml::parse(&src).map_err(|e| broken(&part, e))?;
        let mut walk = OdfWalk {
            keep,
            removed: &mut *removed,
            kept_objects: &kept_objects,
            base,
            dropped: Vec::new(),
            changed: false,
        };
        walk.children(&mut nodes);
        let dropped = std::mem::take(&mut walk.dropped);
        if walk.changed {
            package::set(entries, &part, xml::to_xml(&nodes).into_bytes());
        }
        for path in dropped {
            let prefix = format!("{path}/");
            entries.retain(|e| e.name != path && !e.name.starts_with(&prefix));
            gone.push(path);
        }
    }
    if !gone.is_empty() {
        if let Some(src) = checked_part(entries, MANIFEST)? {
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

/// The media type the manifest gives each path, such as `Object 1`.
fn manifest_types(entries: &[Entry]) -> Result<HashMap<String, String>, TemplateError> {
    let mut out = HashMap::new();
    let Some(src) = checked_part(entries, MANIFEST)? else {
        return Ok(out);
    };
    let nodes = xml::parse(&src).map_err(|e| broken(MANIFEST, e))?;
    if let Some(root) = xml::root(&nodes) {
        for e in root.children_named("manifest:file-entry") {
            if let (Some(path), Some(ty)) =
                (e.attr("manifest:full-path"), e.attr("manifest:media-type"))
            {
                out.insert(path.trim_end_matches('/').to_string(), ty);
            }
        }
    }
    Ok(out)
}

const MATHML: &str = "http://www.w3.org/1998/Math/MathML";

/// The embedded documents that are charts or formulas by what they hold, not
/// only by what the manifest, which the template's author wrote, says: a
/// chart's content has an `office:chart` body, a formula's root is MathML.
fn verified_objects(
    entries: &[Entry],
    types: &HashMap<String, String>,
) -> Result<HashSet<String>, TemplateError> {
    let mut out = HashSet::new();
    for (path, ty) in types {
        let ty = ty.trim();
        let chart = ty.eq_ignore_ascii_case(KEPT_OBJECT_TYPES[0]);
        let formula = ty.eq_ignore_ascii_case(KEPT_OBJECT_TYPES[1]);
        if !chart && !formula {
            continue;
        }
        let Some(src) = checked_part(entries, &format!("{path}/content.xml"))? else {
            continue;
        };
        let Ok(nodes) = xml::parse(&src) else {
            continue;
        };
        let Some(root) = xml::root(&nodes) else {
            continue;
        };
        let agrees = if chart {
            root.name == "office:document-content"
                && root
                    .child("office:body")
                    .is_some_and(|b| b.child("office:chart").is_some())
                && root.child("office:body").is_some_and(|b| {
                    b.children.iter().all(|n| match n {
                        Node::Elem(e) => e.name == "office:chart",
                        _ => true,
                    })
                })
        } else {
            let local = root.name.rsplit(':').next().unwrap_or(&root.name);
            let key = match root.name.split_once(':') {
                Some((p, _)) => format!("xmlns:{p}"),
                None => "xmlns".to_string(),
            };
            local == "math" && root.attr(&key).as_deref() == Some(MATHML)
        };
        if agrees {
            out.insert(path.clone());
        }
    }
    Ok(out)
}

/// A cheap look for anything the walk would change, since most parts hold
/// nothing of the kind and parsing a large one costs. The part's prefixes
/// were checked first, so these names are the only spellings.
fn odf_needs_work(src: &str, keep: Keep) -> bool {
    let has = |s: &str| src.contains(s);
    (!keep.macros
        && (has("<office:scripts")
            || has("script:event-listener")
            || has("text:execute-macro")
            || has("presentation:event-listener")
            || has("xlink:href")))
        || (!keep.objects && (has("<draw:object") || has("<draw:applet") || has("<draw:plugin")))
        || (!keep.external_links && (has("dde") || has("xlink:href") || has("formula")))
        || (!keep.private_links && (has("xlink:href") || has("formula")))
}

struct OdfWalk<'a> {
    keep: Keep,
    removed: &'a mut Removed,
    /// Embedded documents shown to be charts or formulas.
    kept_objects: &'a HashSet<String>,
    /// The directory of the part being walked, for its relative links.
    base: String,
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

    /// Formulas in sheets and text tables that reach outside the file lose
    /// the formula and keep their value.
    fn formulas(&mut self, e: &mut Elem) {
        for attr in ["table:formula", "table:expression"] {
            if let Some(f) = e.attr(attr) {
                if let Some(why) = formula_threat(&f, self.keep) {
                    self.removed.count(why);
                    self.changed = true;
                    e.remove_attr(attr);
                }
            }
        }
    }

    fn element(&mut self, e: &mut Elem) -> Action {
        let href = e.attr("xlink:href");
        let keep = self.keep;
        let name = e.name.clone();
        let name = name.as_str();
        if !keep.external_links || !keep.private_links {
            self.formulas(e);
        }
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
                    !outside(h) && !self.kept_objects.contains(&package_path(&self.base, h))
                }),
                _ => false,
            };
            if object {
                if let Some(h) = href.as_deref().filter(|h| !outside(h)) {
                    self.dropped.push(package_path(&self.base, h));
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
            // As a browser reads them: slashes, backslashes, controls.
            "http:\\\\10.0.0.1\\admin",
            "http:/10.0.0.1",
            "http:10.0.0.1",
            "https:\\/10.0.0.1",
            "h\tttp://10.0.0.1",
            "http://10.0.\n0.1",
            "  http://10.0.0.1  ",
            "HTTP://LocalHost",
            "//10.0.0.1/x",
            "\\\\?\\UNC\\server\\share",
            "C:\\Users\\ada\\secret.docx",
            "http://１２７.０.０.１/",
            "http://0x0a.0.0.1",
            "http://012.0.0.1",
            "http://167772161",
            // Ranges beyond the private ones.
            "http://100.64.0.1",
            "http://198.18.0.1",
            "http://192.0.0.8",
            "http://224.0.0.251",
            "http://255.255.255.255",
            "http://[::ffff:10.0.0.1]",
            "http://[::ffff:a00:1]",
            "http://[::10.0.0.1]",
            "http://[64:ff9b::a00:1]",
            "http://[2002:a00:1::]",
            "http://[fec0::1]",
            "http://[ff02::1]",
            "http://[::]",
            // What a browser cannot parse is not let through.
            "http://1.2.3.4.5",
            "http://[zz]/",
        ] {
            assert!(is_private_url(url), "{url} is private");
        }
        for url in [
            "https://example.com",
            "http://172.32.0.1",
            "http://172.15.0.1",
            "http://11.0.0.1",
            "http://100.128.0.1",
            "http://[2001:db8::1]",
            "http://[::ffff:8.8.8.8]",
            "mailto:a@localhost",
            "tel:+254700000000",
            "#bookmark",
            "doc.docx",
            "http://localhost.example.com",
            "http://192.169.0.1",
            "https://example.com/?next=http://10.0.0.1",
        ] {
            assert!(!is_private_url(url), "{url} is public");
        }
    }

    #[test]
    fn utf16_parts_are_read_and_unreadable_ones_refused() {
        let text = "<?xml version=\"1.0\" encoding=\"UTF-16\"?><a>é</a>";
        let mut le = vec![0xFF, 0xFE];
        le.extend(text.encode_utf16().flat_map(|u| u.to_le_bytes()));
        let mut be = vec![0xFE, 0xFF];
        be.extend(text.encode_utf16().flat_map(|u| u.to_be_bytes()));
        let no_bom: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        for bytes in [le, be, no_bom] {
            assert_eq!(
                decode(&bytes).unwrap(),
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><a>é</a>"
            );
        }
        assert_eq!(decode(b"\xEF\xBB\xBF<a/>").unwrap(), "<a/>");
        assert!(decode(b"<a>\xFF</a>").is_none());
        let entries = vec![Entry {
            name: "word/_rels/document.xml.rels".into(),
            data: b"<a>\xFF</a>".to_vec(),
            stored: false,
        }];
        let err = read_part(&entries, "word/_rels/document.xml.rels").unwrap_err();
        assert!(err.message.contains("refused"), "{err}");
    }

    #[test]
    fn namespaces_bound_to_other_prefixes_are_refused() {
        let ok = "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:x14=\"urn:other\"/>";
        assert!(check_namespaces("p", ok).is_ok());
        for bad in [
            "<pr:Relationships xmlns:pr=\"http://schemas.openxmlformats.org/package/2006/relationships\"/>",
            "<o2:document-content xmlns:o2=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\"/>",
            "<document xmlns=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"/>",
            "<w:document xmlns:w=\"urn:not-word\"/>",
            "<a xmlns:xlink='http://www.w3.org/1999/xlink2'/>",
            "<a><b xmlns:rel=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"/></a>",
        ] {
            let err = check_namespaces("p", bad).unwrap_err();
            assert!(err.message.contains("refused"), "{bad}: {err}");
        }
    }

    #[test]
    fn formulas_that_reach_outside_are_found() {
        let none = Keep::default();
        for (f, why) in [
            ("[1]Sheet1!A1*2", Why::External),
            ("SUM([2]S!A1:A3)", Why::External),
            ("WEBSERVICE(\"https://example.com\")", Why::External),
            ("webservice (A1)", Why::External),
            ("RTD(\"x\",,\"y\")", Why::External),
            ("cmd|' /c calc'!A0", Why::External),
            ("['file:///srv/x.ods'#$Sheet1.A1]", Why::External),
            ("HYPERLINK(A1)", Why::Private),
            ("HYPERLINK(\"http://\"&A1)", Why::Private),
            ("HYPERLINK(\"http://10.0.0.1\",\"x\")", Why::Private),
            ("IF(1,HYPERLINK(\"http:\\\\10.0.0.1\"),0)", Why::Private),
        ] {
            assert_eq!(formula_threat(f, none), Some(why), "{f}");
        }
        for f in [
            "SUM(A1:A3)",
            "Table1[[#This Row],[2019]]",
            "\"a|b\"&\"WEBSERVICE(\"",
            "HYPERLINK(\"https://example.com\",\"site\")",
        ] {
            assert_eq!(formula_threat(f, none), None, "{f}");
        }
        assert_eq!(formula_threat("[1]S!A1", Keep::ALL), None);
    }

    #[test]
    fn fields_that_link_out_are_found() {
        let none = Keep::default();
        for (instr, why) in [
            (" HYPERLINK \"http://10.0.0.1/\" ", Why::Private),
            ("HYPERLINK http://localhost", Why::Private),
            (
                " HYPERLINK \\o \"tip\" \"http://192.168.0.1\"",
                Why::Private,
            ),
            (
                " INCLUDEPICTURE \"http://example.com/a.png\" \\d",
                Why::External,
            ),
            ("includetext \"c:\\\\x.docx\"", Why::External),
            (
                " LINK Excel.Sheet.12 \"C:\\\\a.xlsx\" \"\" \\a ",
                Why::External,
            ),
            (
                " DDEAUTO c:\\\\windows\\\\system32\\\\cmd.exe \"/k calc\" ",
                Why::External,
            ),
        ] {
            assert_eq!(field_threat(instr, false, none), Some(why), "{instr}");
        }
        assert_eq!(
            field_threat(" HYPERLINK \"https://example.com\" ", false, none),
            None
        );
        assert_eq!(field_threat(" HYPERLINK \\l \"intro\" ", false, none), None);
        assert_eq!(field_threat(" MERGEFIELD name ", false, none), None);
        // A computed target cannot be checked.
        assert_eq!(field_threat(" HYPERLINK ", true, none), Some(Why::Private));
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
