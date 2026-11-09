//! What each format calls its paragraphs, runs, rows and tables, and which
//! ZIP parts can hold text.

use crate::Format;

/// Element names that give a format its structure.
#[derive(Debug, Clone, Copy)]
pub struct Dialect {
    /// Elements whose text is joined before tags are looked for.
    pub paragraphs: &'static [&'static str],
    /// Text leaf elements whose text children count; `None` means every
    /// text node under a paragraph counts (the ODF model).
    pub text_leaves: Option<&'static [&'static str]>,
    /// Elements that stand for a character: `(name, text)`.
    pub space_like: &'static [(&'static str, &'static str)],
    pub rows: &'static [&'static str],
    pub tables: &'static [&'static str],
    /// The slide element for formats that keep slides in one part.
    pub slide: Option<&'static str>,
    /// Text leaves get `xml:space="preserve"` when they hold a tag.
    pub preserve_space: bool,
    /// Cells are the paragraphs and places are named by cell reference.
    pub sheet: bool,
}

pub const DOCX: Dialect = Dialect {
    paragraphs: &["w:p"],
    text_leaves: Some(&["w:t"]),
    space_like: &[("w:tab", "\t"), ("w:br", "\n"), ("w:noBreakHyphen", "-")],
    rows: &["w:tr"],
    tables: &["w:tbl"],
    slide: None,
    preserve_space: true,
    sheet: false,
};

pub const ODF_TEXT: Dialect = Dialect {
    paragraphs: &["text:p", "text:h"],
    text_leaves: None,
    space_like: &[
        ("text:s", " "),
        ("text:tab", "\t"),
        ("text:line-break", "\n"),
    ],
    rows: &["table:table-row"],
    tables: &["table:table"],
    slide: None,
    preserve_space: false,
    sheet: false,
};

pub const ODP: Dialect = Dialect {
    slide: Some("draw:page"),
    ..ODF_TEXT
};

pub const PPTX: Dialect = Dialect {
    paragraphs: &["a:p"],
    text_leaves: Some(&["a:t"]),
    space_like: &[("a:br", "\n")],
    rows: &["a:tr"],
    tables: &["a:tbl"],
    slide: None,
    preserve_space: false,
    sheet: false,
};

/// Sheet cells are the paragraphs; their `t` leaves (inline or rich) the runs.
pub const XLSX: Dialect = Dialect {
    paragraphs: &["c"],
    text_leaves: Some(&["t"]),
    space_like: &[],
    rows: &["row"],
    tables: &["sheetData"],
    slide: None,
    preserve_space: true,
    sheet: true,
};

pub fn dialect(format: Format) -> Dialect {
    match format {
        Format::Docx => DOCX,
        Format::Odt | Format::Ods => ODF_TEXT,
        Format::Odp => ODP,
        Format::Pptx => PPTX,
        Format::Xlsx => XLSX,
    }
}

/// Whether a ZIP entry may hold template text for this format. Sheets and
/// slides are ordered by the package module, so they are listed there.
pub fn is_text_part(format: Format, name: &str) -> bool {
    match format {
        Format::Docx => {
            name == "word/document.xml"
                || name == "word/footnotes.xml"
                || name == "word/endnotes.xml"
                || (name.starts_with("word/header") && name.ends_with(".xml"))
                || (name.starts_with("word/footer") && name.ends_with(".xml"))
        }
        Format::Odt => name == "content.xml" || name == "styles.xml",
        Format::Ods | Format::Odp => name == "content.xml",
        Format::Xlsx => {
            // Drawings hold no text tags, only placeholder pictures.
            (name.starts_with("xl/worksheets/sheet") || name.starts_with("xl/drawings/drawing"))
                && name.ends_with(".xml")
        }
        Format::Pptx => name.starts_with("ppt/slides/slide") && name.ends_with(".xml"),
    }
}
