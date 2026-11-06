//! Editing an XMP packet as text.
//!
//! PDF/A files carry their metadata twice: in the Info dictionary and in
//! the XMP stream, and validators require both to agree. The functions here
//! change one property at a time inside the existing packet, so everything
//! else (the `pdfaid` block, extension schemas, producer-specific
//! properties) stays byte for byte as it was. Properties are recognised in
//! element form (`<dc:title>...</dc:title>`) and in attribute form
//! (`<rdf:Description dc:format="..."/>`); new ones go into an
//! `rdf:Description` that already declares the prefix, or into a fresh one
//! that does, so the packet never gains an undeclared prefix.

/// Namespace URI of each prefix the engines write.
pub fn namespace(prefix: &str) -> &'static str {
    match prefix {
        "dc" => "http://purl.org/dc/elements/1.1/",
        "xmp" => "http://ns.adobe.com/xap/1.0/",
        "pdf" => "http://ns.adobe.com/pdf/1.3/",
        "pdfaid" => "http://www.aiim.org/pdfa/ns/id/",
        "fx" => FACTURX_NS,
        "tiff" => "http://ns.adobe.com/tiff/1.0/",
        "xmpRights" => "http://ns.adobe.com/xap/1.0/rights/",
        _ => "",
    }
}

/// The Factur-X XMP namespace.
pub const FACTURX_NS: &str = "urn:factur-x:pdfa:CrossIndustryDocument:invoice:1p0#";

/// An empty packet, for documents that have no XMP stream yet.
pub fn empty_packet() -> String {
    concat!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n",
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n",
        "<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n",
        "</rdf:RDF>\n",
        "</x:xmpmeta>\n",
        "<?xpacket end=\"w\"?>",
    )
    .to_string()
}

pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

pub fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Where `<prefix:name ...>...</prefix:name>` sits: the span of the whole
/// element and the span of its inner text.
fn find_element(
    xml: &str,
    qname: &str,
) -> Option<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    let open = format!("<{qname}");
    let mut from = 0;
    while let Some(rel) = xml[from..].find(&open) {
        let start = from + rel;
        let after = start + open.len();
        let next = xml.as_bytes().get(after).copied();
        if !matches!(
            next,
            Some(b'>') | Some(b' ') | Some(b'/') | Some(b'\n') | Some(b'\t')
        ) {
            from = after;
            continue;
        }
        let tag_end = after + xml[after..].find('>')?;
        if xml[..tag_end].ends_with('/') {
            return Some((start..tag_end + 1, tag_end..tag_end));
        }
        let close = format!("</{qname}>");
        let close_at = tag_end + 1 + xml[tag_end + 1..].find(&close)?;
        return Some((start..close_at + close.len(), tag_end + 1..close_at));
    }
    None
}

/// Reads a property's text. Language alternatives and sequences yield their
/// first item.
pub fn get(xml: &str, qname: &str) -> Option<String> {
    if let Some((_, inner)) = find_element(xml, qname) {
        let inner = &xml[inner];
        let text = match find_element(inner, "rdf:li") {
            Some((_, li)) => &inner[li],
            None => inner,
        };
        return Some(unescape(text.trim()));
    }
    let attr = format!(" {qname}=\"");
    let at = xml.find(&attr)? + attr.len();
    let end = at + xml[at..].find('"')?;
    Some(unescape(&xml[at..end]))
}

/// Sets a property to `inner` (already escaped XML), or removes it when
/// `inner` is `None`.
pub fn set_raw(xml: &mut String, qname: &str, inner: Option<&str>) {
    let (prefix, _) = qname.split_once(':').unwrap_or((qname, ""));
    // Attribute form: strip it, then (re)add in element form so that
    // structured values fit too.
    let attr = format!(" {qname}=\"");
    if let Some(at) = xml.find(&attr)
        && let Some(len) = xml[at + attr.len()..].find('"')
    {
        xml.replace_range(at..at + attr.len() + len + 1, "");
    }
    if let Some((whole, _)) = find_element(xml, qname) {
        // Drop the surrounding line when the element goes.
        let replacement = match inner {
            Some(v) => format!("<{qname}>{v}</{qname}>"),
            None => String::new(),
        };
        xml.replace_range(whole, &replacement);
        return;
    }
    let Some(inner) = inner else { return };
    let element = format!("<{qname}>{inner}</{qname}>\n");
    // A description that already declares the prefix takes the property.
    let decl = format!("xmlns:{prefix}=\"");
    let mut from = 0;
    while let Some(rel) = xml[from..].find("<rdf:Description") {
        let start = from + rel;
        let Some(tag_len) = xml[start..].find('>') else {
            break;
        };
        let tag = &xml[start..start + tag_len];
        if tag.contains(&decl) && !tag.ends_with('/') {
            xml.insert_str(start + tag_len + 1, &format!("\n{element}"));
            return;
        }
        from = start + tag_len;
    }
    let block = format!(
        "<rdf:Description rdf:about=\"\" xmlns:{prefix}=\"{}\">\n{element}</rdf:Description>\n",
        namespace(prefix)
    );
    insert_description(xml, &block);
}

/// Appends a whole `rdf:Description` block before `</rdf:RDF>`.
pub fn insert_description(xml: &mut String, block: &str) {
    match xml.find("</rdf:RDF>") {
        Some(at) => xml.insert_str(at, block),
        None => {
            *xml = empty_packet();
            insert_description(xml, block);
        }
    }
}

/// A plain text property such as `pdf:Producer`.
pub fn set_text(xml: &mut String, qname: &str, value: Option<&str>) {
    set_raw(xml, qname, value.map(escape).as_deref());
}

/// A language alternative such as `dc:title`.
pub fn set_alt(xml: &mut String, qname: &str, value: Option<&str>) {
    let inner = value.map(|v| {
        format!(
            "<rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt>",
            escape(v)
        )
    });
    set_raw(xml, qname, inner.as_deref());
}

/// An ordered list such as `dc:creator`.
pub fn set_seq(xml: &mut String, qname: &str, value: Option<&str>) {
    let inner = value.map(|v| format!("<rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq>", escape(v)));
    set_raw(xml, qname, inner.as_deref());
}

/// Converts a PDF date `D:20240102030405+01'00'` to XMP's
/// `2024-01-02T03:04:05+01:00`. Returns `None` for anything else.
pub fn pdf_date_to_iso(date: &str) -> Option<String> {
    let s = date.strip_prefix("D:")?;
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() < 4 {
        return None;
    }
    let part = |from: usize, len: usize, default: &str| -> String {
        digits
            .get(from..from + len)
            .map(str::to_string)
            .unwrap_or_else(|| default.to_string())
    };
    let mut out = format!(
        "{}-{}-{}T{}:{}:{}",
        part(0, 4, "0000"),
        part(4, 2, "01"),
        part(6, 2, "01"),
        part(8, 2, "00"),
        part(10, 2, "00"),
        part(12, 2, "00")
    );
    let rest = &s[digits.len()..];
    match rest.chars().next() {
        Some('Z') => out.push('Z'),
        Some(sign @ ('+' | '-')) => {
            let tz: Vec<String> = rest[1..]
                .split('\'')
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect();
            let h = tz.first().cloned().unwrap_or_else(|| "00".into());
            let m = tz.get(1).cloned().unwrap_or_else(|| "00".into());
            out.push_str(&format!("{sign}{h}:{m}"));
        }
        _ => {}
    }
    Some(out)
}

/// Converts an ISO 8601 date to a PDF date. Returns `None` when `date` is
/// not in that form.
pub fn iso_date_to_pdf(date: &str) -> Option<String> {
    let s = date.trim();
    let mut digits = String::new();
    let mut rest = "";
    for (i, c) in s.char_indices() {
        if c.is_ascii_digit() {
            digits.push(c);
        } else if matches!(c, '-' | ':' | 'T' | ' ' | '.') && digits.len() < 14 {
            if c == '.' {
                rest = &s[i..];
                break;
            }
        } else {
            rest = &s[i..];
            break;
        }
    }
    if digits.len() < 8 || digits.len() > 14 {
        return None;
    }
    while digits.len() < 14 {
        digits.push('0');
    }
    let mut out = format!("D:{digits}");
    let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
    match rest.chars().next() {
        Some('Z') => out.push('Z'),
        Some(sign @ ('+' | '-')) => {
            let (h, m) = rest[1..].split_once(':').unwrap_or((&rest[1..], "00"));
            out.push_str(&format!("{sign}{h}'{m}'"));
        }
        _ => {}
    }
    Some(out)
}

/// A date as the Info dictionary and XMP want it: `(pdf, xmp)`. Both forms
/// are accepted; anything else is passed through unchanged.
pub fn date_pair(value: &str) -> (String, String) {
    if value.starts_with("D:") {
        return (
            value.to_string(),
            pdf_date_to_iso(value).unwrap_or_else(|| value.to_string()),
        );
    }
    match iso_date_to_pdf(value) {
        Some(pdf) => {
            let iso = pdf_date_to_iso(&pdf).unwrap_or_else(|| value.to_string());
            (pdf, iso)
        }
        None => (value.to_string(), value.to_string()),
    }
}

/// The Factur-X extension schema, required by PDF/A-3 for the `fx:`
/// properties.
pub const FACTURX_SCHEMA: &str = r#"<rdf:Description rdf:about="" xmlns:pdfaExtension="http://www.aiim.org/pdfa/ns/extension/" xmlns:pdfaSchema="http://www.aiim.org/pdfa/ns/schema#" xmlns:pdfaProperty="http://www.aiim.org/pdfa/ns/property#">
<pdfaExtension:schemas><rdf:Bag><rdf:li rdf:parseType="Resource">
<pdfaSchema:schema>Factur-X PDFA Extension Schema</pdfaSchema:schema>
<pdfaSchema:namespaceURI>urn:factur-x:pdfa:CrossIndustryDocument:invoice:1p0#</pdfaSchema:namespaceURI>
<pdfaSchema:prefix>fx</pdfaSchema:prefix>
<pdfaSchema:property><rdf:Seq>
<rdf:li rdf:parseType="Resource"><pdfaProperty:name>DocumentFileName</pdfaProperty:name><pdfaProperty:valueType>Text</pdfaProperty:valueType><pdfaProperty:category>external</pdfaProperty:category><pdfaProperty:description>name of the embedded XML invoice file</pdfaProperty:description></rdf:li>
<rdf:li rdf:parseType="Resource"><pdfaProperty:name>DocumentType</pdfaProperty:name><pdfaProperty:valueType>Text</pdfaProperty:valueType><pdfaProperty:category>external</pdfaProperty:category><pdfaProperty:description>INVOICE</pdfaProperty:description></rdf:li>
<rdf:li rdf:parseType="Resource"><pdfaProperty:name>Version</pdfaProperty:name><pdfaProperty:valueType>Text</pdfaProperty:valueType><pdfaProperty:category>external</pdfaProperty:category><pdfaProperty:description>The actual version of the Factur-X XML schema</pdfaProperty:description></rdf:li>
<rdf:li rdf:parseType="Resource"><pdfaProperty:name>ConformanceLevel</pdfaProperty:name><pdfaProperty:valueType>Text</pdfaProperty:valueType><pdfaProperty:category>external</pdfaProperty:category><pdfaProperty:description>The conformance level of the embedded Factur-X data</pdfaProperty:description></rdf:li>
</rdf:Seq></pdfaSchema:property>
</rdf:li></rdf:Bag></pdfaExtension:schemas>
</rdf:Description>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn properties_are_replaced_inserted_and_removed() {
        let mut xml = empty_packet();
        set_alt(&mut xml, "dc:title", Some("A & B"));
        set_text(&mut xml, "pdf:Producer", Some("rusticdoc"));
        assert_eq!(get(&xml, "dc:title").as_deref(), Some("A & B"));
        assert_eq!(get(&xml, "pdf:Producer").as_deref(), Some("rusticdoc"));
        // A second dc property lands in the description that declares dc.
        set_seq(&mut xml, "dc:creator", Some("Ada"));
        assert_eq!(xml.matches("xmlns:dc=").count(), 1);
        set_alt(&mut xml, "dc:title", Some("New"));
        assert_eq!(get(&xml, "dc:title").as_deref(), Some("New"));
        assert_eq!(xml.matches("<dc:title>").count(), 1);
        set_text(&mut xml, "pdf:Producer", None);
        assert_eq!(get(&xml, "pdf:Producer"), None);
        // Attribute form is read and rewritten.
        let mut attr = empty_packet().replace(
            "</rdf:RDF>",
            "<rdf:Description rdf:about=\"\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmp:CreatorTool=\"Old\"/>\n</rdf:RDF>",
        );
        assert_eq!(get(&attr, "xmp:CreatorTool").as_deref(), Some("Old"));
        set_text(&mut attr, "xmp:CreatorTool", Some("New"));
        assert_eq!(get(&attr, "xmp:CreatorTool").as_deref(), Some("New"));
        assert!(!attr.contains("xmp:CreatorTool=\"Old\""));
    }

    #[test]
    fn dates_convert_both_ways() {
        assert_eq!(
            pdf_date_to_iso("D:20240102030405+01'00'").as_deref(),
            Some("2024-01-02T03:04:05+01:00")
        );
        assert_eq!(
            pdf_date_to_iso("D:20240102030405Z").as_deref(),
            Some("2024-01-02T03:04:05Z")
        );
        assert_eq!(
            iso_date_to_pdf("2024-01-02T03:04:05Z").as_deref(),
            Some("D:20240102030405Z")
        );
        assert_eq!(
            iso_date_to_pdf("2024-01-02T03:04:05.123+01:00").as_deref(),
            Some("D:20240102030405+01'00'")
        );
        assert_eq!(
            iso_date_to_pdf("2024-01-02").as_deref(),
            Some("D:20240102000000")
        );
        assert_eq!(date_pair("D:20240102030405Z").1, "2024-01-02T03:04:05Z");
        assert_eq!(
            date_pair("yesterday"),
            ("yesterday".into(), "yesterday".into())
        );
    }
}
