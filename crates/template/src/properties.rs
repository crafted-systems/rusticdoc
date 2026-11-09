//! Template settings from custom document properties.
//!
//! Authors who would rather not type a `{% settings %}` tag into the page can
//! set the same names (`lang`, `timezone`, `currency`, `t.<key>`) as custom
//! properties: File > Properties > Custom in Word, Excel and PowerPoint, and
//! File > Properties > Custom Properties in LibreOffice. OOXML keeps them in
//! `docProps/custom.xml`, ODF as `meta:user-defined` in `meta.xml`. They are
//! read before the document's text, so a tag that sets the same thing wins.

use crate::package::{self, Entry};
use crate::program::apply_setting;
use crate::xml::{self, Elem, Node};
use crate::{Format, Settings};

const CUSTOM_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/custom-properties";

/// The settings the package's custom properties give. Properties with other
/// names are ignored, as unknown keys in the tag are.
pub fn settings(format: Format, entries: &[Entry]) -> Settings {
    let mut settings = Settings::default();
    for (name, value) in properties(format, entries) {
        apply_setting(&mut settings, name.trim(), value);
    }
    settings
}

/// `(name, value)` of every custom property, in document order.
fn properties(format: Format, entries: &[Entry]) -> Vec<(String, String)> {
    match format {
        Format::Docx | Format::Xlsx | Format::Pptx => {
            let part = package::relationships(entries, "_rels/.rels", "")
                .into_iter()
                .find(|(_, ty, _)| ty == CUSTOM_TYPE)
                .map(|(_, _, target)| target)
                .unwrap_or_else(|| "docProps/custom.xml".to_string());
            let Some(nodes) = package::text_of(entries, &part).and_then(|s| xml::parse(&s).ok())
            else {
                return Vec::new();
            };
            let Some(root) = xml::root(&nodes) else {
                return Vec::new();
            };
            root.children_named("property")
                .filter_map(|p| {
                    // The value is the property's one typed child, such as
                    // `vt:lpwstr`.
                    let value = p.children.iter().find_map(|n| match n {
                        Node::Elem(v) => Some(v.text_content()),
                        _ => None,
                    })?;
                    Some((p.attr("name")?, value))
                })
                .collect()
        }
        Format::Odt | Format::Ods | Format::Odp => {
            let Some(nodes) =
                package::text_of(entries, "meta.xml").and_then(|s| xml::parse(&s).ok())
            else {
                return Vec::new();
            };
            let mut out = Vec::new();
            if let Some(root) = xml::root(&nodes) {
                collect_user_defined(root, &mut out);
            }
            out
        }
    }
}

fn collect_user_defined(e: &Elem, out: &mut Vec<(String, String)>) {
    for n in &e.children {
        let Node::Elem(c) = n else { continue };
        if c.name == "meta:user-defined" {
            if let Some(name) = c.attr("meta:name") {
                out.push((name, c.text_content()));
            }
        } else {
            collect_user_defined(c, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, data: &str) -> Entry {
        Entry {
            name: name.to_string(),
            data: data.as_bytes().to_vec(),
            stored: false,
        }
    }

    #[test]
    fn reads_ooxml_custom_properties() {
        let custom = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"><property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="2" name="lang"><vt:lpwstr>fr</vt:lpwstr></property><property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="3" name="t.total.fr"><vt:lpwstr>Montant &amp; taxes</vt:lpwstr></property><property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="4" name="Client"><vt:lpwstr>ignored</vt:lpwstr></property><property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="5" name="tz"><vt:lpwstr>+02:00</vt:lpwstr></property></Properties>"#;
        let s = settings(Format::Docx, &[entry("docProps/custom.xml", custom)]);
        assert_eq!(s.lang.as_deref(), Some("fr"));
        assert_eq!(s.timezone.as_deref(), Some("+02:00"));
        assert_eq!(
            s.translations.get("total.fr").map(String::as_str),
            Some("Montant & taxes")
        );
        assert_eq!(s.translations.len(), 1);
    }

    #[test]
    fn reads_odf_user_defined_metadata() {
        let meta = r#"<office:document-meta xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0"><office:meta><meta:generator>LibreOffice</meta:generator><meta:user-defined meta:name="currency">EUR</meta:user-defined><meta:user-defined meta:name="t.hello" meta:value-type="string">Bonjour</meta:user-defined></office:meta></office:document-meta>"#;
        let s = settings(Format::Ods, &[entry("meta.xml", meta)]);
        assert_eq!(s.currency.as_deref(), Some("EUR"));
        assert_eq!(
            s.translations.get("hello").map(String::as_str),
            Some("Bonjour")
        );
    }

    #[test]
    fn no_properties_give_no_settings() {
        assert_eq!(settings(Format::Xlsx, &[]), Settings::default());
    }
}
