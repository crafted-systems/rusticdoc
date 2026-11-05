//! Text strings, WinAnsi bytes, Helvetica metrics and line layout for the
//! appearance streams this module writes.

use std::fmt::Write as _;

/// Decodes a PDF text string: UTF-16BE with a byte order mark, UTF-8 with
/// one, or PDFDocEncoding, which this treats as Latin-1.
pub(super) fn decode_text_string(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest
            .chunks(2)
            .map(|pair| u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)]))
            .collect();
        String::from_utf16_lossy(&units)
    } else if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(rest).into_owned()
    } else {
        bytes.iter().map(|&b| b as char).collect()
    }
}

/// Encodes text as a PDF text string: PDFDocEncoding when every character
/// fits it, UTF-16BE with a byte order mark otherwise.
pub(super) fn encode_text_string(text: &str) -> lopdf::Object {
    let fits = text.chars().all(|c| {
        matches!(c, '\t' | '\n' | '\r') || ('\u{20}'..='\u{7E}').contains(&c) || {
            let n = c as u32;
            (0xA1..=0xFF).contains(&n) && n != 0xAD
        }
    });
    if fits {
        let bytes = text.chars().map(|c| c as u32 as u8).collect::<Vec<_>>();
        lopdf::Object::String(bytes, lopdf::StringFormat::Literal)
    } else {
        let mut bytes = vec![0xFE, 0xFF];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        lopdf::Object::String(bytes, lopdf::StringFormat::Hexadecimal)
    }
}

/// True when the text carries a template tag.
pub(super) fn is_template(text: &str) -> bool {
    text.contains("{{") || text.contains("{%")
}

/// Maps a character to its WinAnsiEncoding byte, `?` when it has none.
pub(super) fn winansi_byte(c: char) -> u8 {
    let n = c as u32;
    if (0x20..=0x7E).contains(&n) || (0xA0..=0xFF).contains(&n) {
        return n as u8;
    }
    match c {
        '€' => 0x80,
        '‚' => 0x82,
        'ƒ' => 0x83,
        '„' => 0x84,
        '…' => 0x85,
        '†' => 0x86,
        '‡' => 0x87,
        'ˆ' => 0x88,
        '‰' => 0x89,
        'Š' => 0x8A,
        '‹' => 0x8B,
        'Œ' => 0x8C,
        'Ž' => 0x8E,
        '‘' => 0x91,
        '’' => 0x92,
        '“' => 0x93,
        '”' => 0x94,
        '•' => 0x95,
        '–' => 0x96,
        '—' => 0x97,
        '˜' => 0x98,
        '™' => 0x99,
        'š' => 0x9A,
        '›' => 0x9B,
        'œ' => 0x9C,
        'ž' => 0x9E,
        'Ÿ' => 0x9F,
        _ => b'?',
    }
}

/// Advance widths of Helvetica for the printable ASCII range, in 1/1000 em,
/// from the AFM file. Everything else is taken as 556, the width of a digit.
const HELVETICA_WIDTHS: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];

/// Width of `text` set in Helvetica at `size` points.
pub(super) fn text_width(text: &str, size: f32) -> f32 {
    let units: u32 = text
        .chars()
        .map(|c| match c as u32 {
            n @ 0x20..=0x7E => u32::from(HELVETICA_WIDTHS[(n - 0x20) as usize]),
            _ => 556,
        })
        .sum();
    units as f32 * size / 1000.0
}

/// Helvetica ascender and descender, as fractions of the font size.
const ASCENT: f32 = 0.718;
const DESCENT: f32 = 0.207;
const LINE_HEIGHT: f32 = 1.15;

/// One positioned line of text: the baseline origin and the text.
pub(super) struct Line {
    pub x: f32,
    pub y: f32,
    pub text: String,
}

/// Lays `text` out in a `width` by `height` box whose origin is the bottom
/// left. A `size` of zero picks one that fits; `quadding` is the PDF `/Q`
/// value (0 left, 1 centre, 2 right). Returns the size used and the lines.
pub(super) fn layout(
    text: &str,
    width: f32,
    height: f32,
    size: f32,
    quadding: i64,
    multiline: bool,
) -> (f32, Vec<Line>) {
    let pad = 2.0;
    let inner_w = (width - 2.0 * pad).max(1.0);
    let inner_h = (height - 2.0 * pad).max(1.0);
    let single = text.split(['\n', '\r']).collect::<Vec<_>>().join(" ");
    let size = if size > 0.0 {
        size
    } else if multiline {
        12.0f32.min(inner_h / LINE_HEIGHT).max(4.0)
    } else {
        let by_height = inner_h / (ASCENT + DESCENT + 0.1);
        let by_width = inner_w / text_width(&single, 1.0).max(0.001);
        by_height.min(by_width).clamp(4.0, 48.0)
    };
    let rows: Vec<String> = if multiline {
        wrap(text, inner_w, size)
    } else {
        vec![single]
    };
    let place_x = |row: &str| {
        let w = text_width(row, size);
        match quadding {
            1 => pad + ((inner_w - w) / 2.0).max(0.0),
            2 => pad + (inner_w - w).max(0.0),
            _ => pad,
        }
    };
    let lines = if multiline {
        let mut y = height - pad - ASCENT * size;
        rows.into_iter()
            .map(|row| {
                let line = Line {
                    x: place_x(&row),
                    y,
                    text: row,
                };
                y -= LINE_HEIGHT * size;
                line
            })
            .collect()
    } else {
        let y = (height - (ASCENT + DESCENT) * size) / 2.0 + DESCENT * size;
        rows.into_iter()
            .map(|row| Line {
                x: place_x(&row),
                y,
                text: row,
            })
            .collect()
    };
    (size, lines)
}

/// Greedy word wrap at `width` points; explicit line breaks are kept.
fn wrap(text: &str, width: f32, size: f32) -> Vec<String> {
    let mut rows = Vec::new();
    for paragraph in text.replace("\r\n", "\n").split(['\n', '\r']) {
        let mut row = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if row.is_empty() {
                word.to_string()
            } else {
                format!("{row} {word}")
            };
            if row.is_empty() || text_width(&candidate, size) <= width {
                row = candidate;
            } else {
                rows.push(std::mem::take(&mut row));
                row = word.to_string();
            }
        }
        rows.push(row);
    }
    rows
}

/// Writes a number the way content streams want it: no exponent, no
/// trailing zeros.
pub(super) fn num(n: f32) -> String {
    let s = format!("{n:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" || s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

/// A literal string operand, WinAnsi encoded and escaped.
pub(super) fn literal(text: &str) -> String {
    let mut out = String::from("(");
    for c in text.chars() {
        match winansi_byte(c) {
            b'(' => out.push_str("\\("),
            b')' => out.push_str("\\)"),
            b'\\' => out.push_str("\\\\"),
            b @ 0x20..=0x7E => out.push(b as char),
            b => {
                let _ = write!(out, "\\{b:03o}");
            }
        }
    }
    out.push(')');
    out
}

/// The `BT ... ET` block that shows `lines` with font resource `font` at
/// `size`, after the colour operators in `colour` (such as `0 g`).
pub(super) fn text_block(font: &str, size: f32, colour: &str, lines: &[Line]) -> String {
    let mut out = format!("BT /{font} {} Tf {colour}\n", num(size));
    for line in lines {
        let _ = writeln!(
            out,
            "1 0 0 1 {} {} Tm {} Tj",
            num(line.x),
            num(line.y),
            literal(&line.text)
        );
    }
    out.push_str("ET\n");
    out
}

/// The font name, size and remaining operators (colour) of a `/DA` string.
/// Missing parts fall back to Helvetica, auto size and black.
pub(super) fn parse_da(da: &str) -> (String, f32, String) {
    let tokens: Vec<&str> = da.split_whitespace().collect();
    let mut font = None;
    let mut size = 0.0;
    let mut rest = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if tokens[i] == "Tf" && i >= 2 {
            rest.truncate(rest.len() - 2);
            font = Some(tokens[i - 2].trim_start_matches('/').to_string());
            size = tokens[i - 1].parse().unwrap_or(0.0);
        } else {
            rest.push(tokens[i]);
        }
        i += 1;
    }
    let colour = if rest.is_empty() {
        "0 g".to_string()
    } else {
        rest.join(" ")
    };
    (
        font.filter(|f| !f.is_empty())
            .unwrap_or_else(|| "Helv".into()),
        size,
        colour,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_strings_round_trip() {
        let latin = encode_text_string("Café (1)");
        assert_eq!(decode_text_string(latin.as_str().unwrap()), "Café (1)");
        let wide = encode_text_string("Привет");
        let bytes = wide.as_str().unwrap();
        assert_eq!(&bytes[..2], &[0xFE, 0xFF]);
        assert_eq!(decode_text_string(bytes), "Привет");
    }

    #[test]
    fn da_parses_font_size_and_colour() {
        assert_eq!(
            parse_da("/Helv 0 Tf 0 g"),
            ("Helv".into(), 0.0, "0 g".into())
        );
        assert_eq!(
            parse_da("0 0 1 rg /TiRo 12 Tf"),
            ("TiRo".into(), 12.0, "0 0 1 rg".into())
        );
        assert_eq!(parse_da(""), ("Helv".into(), 0.0, "0 g".into()));
    }

    #[test]
    fn literal_escapes_and_encodes() {
        assert_eq!(literal("a(b)\\"), "(a\\(b\\)\\\\)");
        assert_eq!(literal("€é"), "(\\200\\351)");
        assert_eq!(literal("Ж"), "(?)");
    }

    #[test]
    fn layout_wraps_and_aligns() {
        let (size, lines) = layout("one two three four", 60.0, 40.0, 10.0, 0, true);
        assert_eq!(size, 10.0);
        assert!(lines.len() >= 2);
        assert!(lines[0].y > lines[1].y);
        let (_, right) = layout("ab", 100.0, 20.0, 10.0, 2, false);
        assert!(right[0].x > 50.0);
        let (auto, _) = layout("wide text here", 50.0, 30.0, 0.0, 0, false);
        assert!(auto >= 4.0 && text_width("wide text here", auto) <= 46.01);
    }
}
