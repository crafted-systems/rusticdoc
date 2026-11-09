//! Barcodes: the symbol is encoded by rxing, then drawn here as a PNG with
//! whole device pixels per module, an optional line of text, colours, a
//! quiet zone and a quarter-turn rotation.

use resvg::tiny_skia::{self, FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};
use rxing::{BarcodeFormat, EncodeHints, Writer};

use crate::rich::font;
use crate::rich::media::{self, Media, MAX_SIDE_PT};

/// The longest value a barcode accepts.
pub const MAX_BARCODE_INPUT: usize = 4096;
/// The widest a printed linear barcode gets, in modules.
const MAX_BARCODE_MODULES: usize = 4096;
/// The tallest a linear barcode gets, in modules.
const MAX_BARCODE_LINES: usize = 400;
/// The largest barcode image generated, width times height.
pub const MAX_BARCODE_PIXELS: usize = 4_000_000;
/// The widest quiet zone, in modules.
pub const MAX_QUIET: u32 = 100;

pub const SYMBOLOGIES: &str = "qr, datamatrix, pdf417, aztec, code128, gs1-128, code39, code93, \
                               ean13, ean8, upca, upce, itf, itf14 or codabar";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Qr,
    DataMatrix,
    Pdf417,
    Aztec,
    Code128,
    Gs1_128,
    Code39,
    Code93,
    Ean13,
    Ean8,
    UpcA,
    UpcE,
    Itf,
    Itf14,
    Codabar,
}

impl Kind {
    fn parse(name: &str) -> Option<Kind> {
        Some(match name {
            "qr" => Kind::Qr,
            "datamatrix" => Kind::DataMatrix,
            "pdf417" => Kind::Pdf417,
            "aztec" => Kind::Aztec,
            "code128" => Kind::Code128,
            "gs1-128" | "gs1128" => Kind::Gs1_128,
            "code39" => Kind::Code39,
            "code93" => Kind::Code93,
            "ean13" => Kind::Ean13,
            "ean8" => Kind::Ean8,
            "upca" => Kind::UpcA,
            "upce" => Kind::UpcE,
            "itf" => Kind::Itf,
            "itf14" => Kind::Itf14,
            "codabar" => Kind::Codabar,
            _ => return None,
        })
    }

    fn is_matrix(self) -> bool {
        matches!(
            self,
            Kind::Qr | Kind::DataMatrix | Kind::Pdf417 | Kind::Aztec
        )
    }

    /// Whether the symbol must print square.
    fn is_square(self) -> bool {
        matches!(self, Kind::Qr | Kind::DataMatrix | Kind::Aztec)
    }

    fn format(self) -> BarcodeFormat {
        match self {
            Kind::Qr => BarcodeFormat::QR_CODE,
            Kind::DataMatrix => BarcodeFormat::DATA_MATRIX,
            Kind::Pdf417 => BarcodeFormat::PDF_417,
            Kind::Aztec => BarcodeFormat::AZTEC,
            Kind::Code128 | Kind::Gs1_128 => BarcodeFormat::CODE_128,
            Kind::Code39 => BarcodeFormat::CODE_39,
            Kind::Code93 => BarcodeFormat::CODE_93,
            Kind::Ean13 => BarcodeFormat::EAN_13,
            Kind::Ean8 => BarcodeFormat::EAN_8,
            Kind::UpcA => BarcodeFormat::UPC_A,
            Kind::UpcE => BarcodeFormat::UPC_E,
            Kind::Itf | Kind::Itf14 => BarcodeFormat::ITF,
            Kind::Codabar => BarcodeFormat::CODABAR,
        }
    }

    fn default_quiet(self) -> u32 {
        match self {
            Kind::Qr => 4,
            Kind::DataMatrix | Kind::Aztec | Kind::Pdf417 => 2,
            _ => 10,
        }
    }
}

/// The filter's options, already checked for type and range.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Printed width in points (the historical name).
    pub size: Option<f64>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub text: bool,
    pub font_size: Option<f64>,
    /// `RRGGBB`.
    pub color: Option<String>,
    /// `RRGGBB`, or `None` for white; `transparent` for none at all.
    pub background: Option<String>,
    pub transparent: bool,
    pub quiet_zone: Option<u32>,
    pub rotate: u32,
    /// Error correction as given: a level for QR, a number for the others.
    pub ec: Option<String>,
}

/// Where the text under a linear code goes.
enum Layout {
    /// One line centred under the bars.
    Centred(String),
    /// Digits between guard bars that reach down, as retail codes print:
    /// the text outside the bars on each side, and each group of digits
    /// centred on its module range.
    Retail {
        left: Option<String>,
        groups: Vec<(usize, usize, String)>,
        right: Option<String>,
        guards: Vec<(usize, usize)>,
    },
}

/// An encoded symbol: rows of modules, dark is `true`.
struct Symbol {
    rows: Vec<Vec<bool>>,
    linear: bool,
    layout: Option<Layout>,
    /// Bearer bars above and below (ITF-14).
    bearers: bool,
}

fn rgb(hex: &str) -> [u8; 3] {
    let n = u32::from_str_radix(hex, 16).unwrap_or(0);
    [(n >> 16) as u8, (n >> 8) as u8, n as u8]
}

/// Generates a barcode PNG and its printed size in points.
pub fn barcode(kind: &str, value: &str, opts: &Options) -> Result<(Media, (f64, f64)), String> {
    let kind = Kind::parse(kind)
        .ok_or_else(|| format!("unknown symbology {kind:?}; use {SYMBOLOGIES}"))?;
    if value.is_empty() {
        return Err("a barcode needs a value".into());
    }
    if value.len() > MAX_BARCODE_INPUT {
        return Err(format!(
            "the value is longer than {MAX_BARCODE_INPUT} bytes"
        ));
    }
    if opts.size.is_some() && opts.width.is_some() {
        return Err("give size or width, not both: they are the same thing".into());
    }
    if kind.is_matrix() && (opts.text || opts.font_size.is_some()) {
        return Err("text is drawn under linear codes only".into());
    }
    if !opts.text && opts.font_size.is_some() {
        return Err("font_size needs text=true".into());
    }
    if kind.is_square() {
        if let (Some(w), Some(h)) = (opts.width.or(opts.size), opts.height) {
            if (w - h).abs() > 1e-9 {
                return Err(format!(
                    "a {} code is square: give width or height, or equal ones",
                    name(kind)
                ));
            }
        }
    }
    let symbol = encode(kind, value, opts.ec.as_deref())?;
    draw(kind, &symbol, opts)
}

fn name(kind: Kind) -> &'static str {
    match kind {
        Kind::Qr => "QR",
        Kind::DataMatrix => "Data Matrix",
        Kind::Pdf417 => "PDF417",
        Kind::Aztec => "Aztec",
        Kind::Code128 => "Code 128",
        Kind::Gs1_128 => "GS1-128",
        Kind::Code39 => "Code 39",
        Kind::Code93 => "Code 93",
        Kind::Ean13 => "EAN-13",
        Kind::Ean8 => "EAN-8",
        Kind::UpcA => "UPC-A",
        Kind::UpcE => "UPC-E",
        Kind::Itf => "ITF",
        Kind::Itf14 => "ITF-14",
        Kind::Codabar => "Codabar",
    }
}

/// The GS1 mod-10 check digit of `digits`.
fn check_digit(digits: &str) -> u32 {
    let sum: u32 = digits
        .bytes()
        .rev()
        .enumerate()
        .map(|(i, b)| u32::from(b - b'0') * if i % 2 == 0 { 3 } else { 1 })
        .sum();
    (10 - sum % 10) % 10
}

fn digits_only(kind: Kind, value: &str) -> Result<(), String> {
    if !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("{} takes digits only", name(kind)));
    }
    Ok(())
}

/// The digits with the check digit: added when `short`, verified when one
/// longer.
fn with_check(kind: Kind, value: &str, short: usize) -> Result<String, String> {
    digits_only(kind, value)?;
    match value.len() {
        n if n == short => Ok(format!("{value}{}", check_digit(value))),
        n if n == short + 1 => {
            let (body, last) = value.split_at(short);
            if check_digit(body).to_string() != last {
                return Err(format!(
                    "{value:?} has a wrong check digit: {} expects {}",
                    name(kind),
                    check_digit(body)
                ));
            }
            Ok(value.to_string())
        }
        _ => Err(format!(
            "{} takes {short} digits, or {} with the check digit",
            name(kind),
            short + 1
        )),
    }
}

/// UPC-E's eight digits expanded to the UPC-A they stand for, without
/// the check digit.
fn upce_to_upca(upce: &str) -> String {
    let d: Vec<char> = upce.chars().collect();
    let (ns, m) = (d[0], &d[1..7]);
    let body: String = match m[5] {
        '0' | '1' | '2' => format!("{}{}{}0000{}{}{}", m[0], m[1], m[5], m[2], m[3], m[4]),
        '3' => format!("{}{}{}00000{}{}", m[0], m[1], m[2], m[3], m[4]),
        '4' => format!("{}{}{}{}00000{}", m[0], m[1], m[2], m[3], m[4]),
        _ => format!("{}{}{}{}{}0000{}", m[0], m[1], m[2], m[3], m[4], m[5]),
    };
    format!("{ns}{body}")
}

/// GS1 application identifiers whose data has a fixed length, by their
/// first two digits: the length of AI and data together.
const GS1_FIXED: &[(&str, usize)] = &[
    ("00", 20),
    ("01", 16),
    ("02", 16),
    ("03", 16),
    ("04", 18),
    ("11", 8),
    ("12", 8),
    ("13", 8),
    ("14", 8),
    ("15", 8),
    ("16", 8),
    ("17", 8),
    ("18", 8),
    ("19", 8),
    ("20", 4),
    ("31", 10),
    ("32", 10),
    ("33", 10),
    ("34", 10),
    ("35", 10),
    ("36", 10),
    ("41", 16),
];

/// Reads `(01)09501101530003(10)ABC` into Code 128 input with FNC1, and
/// the text to print.
fn gs1(value: &str) -> Result<(String, String), String> {
    const FNC1: char = '\u{f1}';
    let mut encoded = String::from(FNC1);
    let mut rest = value;
    let mut need_separator = false;
    if !rest.starts_with('(') {
        return Err(
            "gs1-128 takes application identifiers in parentheses, such as (01)09501101530003(10)ABC"
                .into(),
        );
    }
    while let Some(after) = rest.strip_prefix('(') {
        let close = after
            .find(')')
            .ok_or("an application identifier is not closed with )")?;
        let ai = &after[..close];
        if !(2..=4).contains(&ai.len()) || !ai.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!(
                "({ai}) is not an application identifier: they are 2 to 4 digits"
            ));
        }
        let data_end = after[close + 1..]
            .find('(')
            .map_or(after.len(), |i| close + 1 + i);
        let data = &after[close + 1..data_end];
        if data.is_empty() {
            return Err(format!("({ai}) has no data"));
        }
        let allowed = |c: char| c.is_ascii_alphanumeric() || "!\"%&'*+,-./:;<=>?_".contains(c);
        if let Some(bad) = data.chars().find(|c| !allowed(*c)) {
            return Err(format!("({ai}) holds {bad:?}, which GS1 does not allow"));
        }
        let fixed = GS1_FIXED
            .iter()
            .find(|(p, _)| ai.starts_with(p))
            .map(|(_, n)| n - ai.len());
        if let Some(len) = fixed {
            if data.len() != len {
                return Err(format!("({ai}) takes {len} characters, not {}", data.len()));
            }
            if matches!(&ai[..2], "00" | "01" | "02") {
                digits_only(Kind::Gs1_128, data)?;
                let (body, last) = data.split_at(data.len() - 1);
                if check_digit(body).to_string() != last {
                    return Err(format!(
                        "({ai}){data} has a wrong check digit: it should end in {}",
                        check_digit(body)
                    ));
                }
            }
        } else if data.len() > 90 {
            return Err(format!("({ai}) holds more than 90 characters"));
        }
        if need_separator {
            encoded.push(FNC1);
        }
        encoded.push_str(ai);
        encoded.push_str(data);
        need_separator = fixed.is_none();
        rest = &after[data_end..];
    }
    if !rest.is_empty() {
        return Err(format!(
            "unexpected {rest:?} after the last application identifier"
        ));
    }
    Ok((encoded, value.to_string()))
}

fn rxing_error(kind: Kind, e: rxing::Exceptions) -> String {
    let text = e.to_string();
    let lower = text.to_ascii_lowercase();
    if ["too big", "too long", "too large", "exceed", "capacity"]
        .iter()
        .any(|w| lower.contains(w))
    {
        return format!("the value is too long for a {} code", name(kind));
    }
    format!("cannot encode this value as {}: {text}", name(kind))
}

fn ec_hint(kind: Kind, ec: Option<&str>) -> Result<Option<String>, String> {
    let Some(ec) = ec.map(str::trim) else {
        return Ok(match kind {
            Kind::Qr => Some("M".to_string()),
            _ => None,
        });
    };
    let number = |lo: u32, hi: u32, what: &str| {
        ec.parse::<u32>()
            .ok()
            .filter(|n| (lo..=hi).contains(n))
            .map(|n| Some(n.to_string()))
            .ok_or_else(|| format!("ec for {} is {what} from {lo} to {hi}", name(kind)))
    };
    match kind {
        Kind::Qr => match ec.to_ascii_uppercase().as_str() {
            l @ ("L" | "M" | "Q" | "H") => Ok(Some(l.to_string())),
            _ => Err("ec for QR is L, M, Q or H".into()),
        },
        Kind::Aztec => number(5, 95, "a percentage"),
        Kind::Pdf417 => number(0, 8, "a level"),
        _ => Err(format!(
            "ec applies to qr, aztec and pdf417, not {}",
            name(kind)
        )),
    }
}

fn encode(kind: Kind, value: &str, ec: Option<&str>) -> Result<Symbol, String> {
    let mut hints = EncodeHints {
        Margin: Some("0".to_string()),
        ErrorCorrection: ec_hint(kind, ec)?,
        ..EncodeHints::default()
    };
    let mut layout = None;
    let mut bearers = false;
    let input: String = match kind {
        Kind::Qr | Kind::DataMatrix | Kind::Pdf417 | Kind::Aztec => {
            if !value.is_ascii() && kind != Kind::DataMatrix {
                hints.CharacterSet = Some("UTF-8".to_string());
            }
            if kind == Kind::DataMatrix && !value.is_ascii() {
                return Err("datamatrix takes ASCII text only".into());
            }
            value.to_string()
        }
        Kind::Code128 => {
            if !value.bytes().all(|b| (32..127).contains(&b)) {
                return Err("code128 takes printable ASCII only".into());
            }
            layout = Some(Layout::Centred(value.to_string()));
            value.to_string()
        }
        Kind::Gs1_128 => {
            let (encoded, text) = gs1(value)?;
            layout = Some(Layout::Centred(text));
            encoded
        }
        Kind::Code39 => {
            if let Some(bad) = value
                .chars()
                .find(|c| !(c.is_ascii_uppercase() || c.is_ascii_digit() || "-. $/+%".contains(*c)))
            {
                return Err(format!(
                    "code39 takes upper-case letters, digits and - . $ / + % and space, not {bad:?}"
                ));
            }
            layout = Some(Layout::Centred(value.to_string()));
            value.to_string()
        }
        Kind::Code93 => {
            if !value.bytes().all(|b| (32..127).contains(&b)) {
                return Err("code93 takes printable ASCII only".into());
            }
            layout = Some(Layout::Centred(value.to_string()));
            value.to_string()
        }
        Kind::Ean13 => {
            let full = with_check(kind, value, 12)?;
            layout = Some(retail(
                Some(&full[..1]),
                &[(3, 45, &full[1..7]), (50, 92, &full[7..])],
                None,
                &[(0, 3), (45, 50), (92, 95)],
            ));
            full
        }
        Kind::Ean8 => {
            let full = with_check(kind, value, 7)?;
            layout = Some(retail(
                None,
                &[(3, 31, &full[..4]), (36, 64, &full[4..])],
                None,
                &[(0, 3), (31, 36), (64, 67)],
            ));
            full
        }
        Kind::UpcA => {
            let full = with_check(kind, value, 11)?;
            layout = Some(retail(
                Some(&full[..1]),
                &[(10, 45, &full[1..6]), (50, 85, &full[6..11])],
                Some(&full[11..]),
                &[(0, 10), (45, 50), (85, 95)],
            ));
            full
        }
        Kind::UpcE => {
            digits_only(kind, value)?;
            if !matches!(value.len(), 7 | 8) || !value.starts_with(['0', '1']) {
                return Err(
                    "upce takes 7 digits starting with 0 or 1, or 8 with the check digit".into(),
                );
            }
            let check = check_digit(&upce_to_upca(&value[..7]));
            if value.len() == 8 && value[7..] != check.to_string() {
                return Err(format!(
                    "{value:?} has a wrong check digit: UPC-E expects {check}"
                ));
            }
            let full = format!("{}{check}", &value[..7]);
            layout = Some(retail(
                Some(&full[..1]),
                &[(3, 45, &full[1..7])],
                Some(&full[7..]),
                &[(0, 3), (45, 51)],
            ));
            full
        }
        Kind::Itf => {
            digits_only(kind, value)?;
            if !value.len().is_multiple_of(2) {
                return Err("itf takes an even number of digits; add a leading 0".into());
            }
            layout = Some(Layout::Centred(value.to_string()));
            value.to_string()
        }
        Kind::Itf14 => {
            let full = with_check(kind, value, 13)?;
            layout = Some(Layout::Centred(full.clone()));
            bearers = true;
            full
        }
        Kind::Codabar => {
            let upper = value.to_ascii_uppercase();
            let ends = |c: char| "ABCD".contains(c);
            let body = match (upper.chars().next(), upper.chars().last()) {
                (Some(a), Some(b)) if upper.len() >= 2 && ends(a) && ends(b) => {
                    &upper[1..upper.len() - 1]
                }
                (Some(a), _) if ends(a) => {
                    return Err("a codabar value that starts with A to D must end with one".into())
                }
                (_, Some(b)) if ends(b) => {
                    return Err("a codabar value that ends with A to D must start with one".into())
                }
                _ => &upper[..],
            };
            if body.is_empty()
                || !body
                    .chars()
                    .all(|c| c.is_ascii_digit() || "-$:/.+".contains(c))
            {
                return Err("codabar takes digits and - $ : / . +, between optional A to D".into());
            }
            layout = Some(Layout::Centred(value.to_string()));
            upper
        }
    };
    let matrix = encoder(kind)
        .encode_with_hints(&input, &kind.format(), 0, 0, &hints)
        .map_err(|e| rxing_error(kind, e))?;
    let (w, h) = (matrix.getWidth(), matrix.getHeight());
    if w == 0 || h == 0 {
        return Err(format!("the value gives an empty {} code", name(kind)));
    }
    let linear = !kind.is_matrix();
    let rows: Vec<Vec<bool>> = if linear {
        vec![(0..w).map(|x| matrix.get(x, 0)).collect()]
    } else {
        (0..h)
            .map(|y| (0..w).map(|x| matrix.get(x, y)).collect())
            .collect()
    };
    Ok(Symbol {
        rows,
        linear,
        layout,
        bearers,
    })
}

fn encoder(kind: Kind) -> Box<dyn Writer> {
    use rxing::oned::*;
    match kind {
        Kind::Qr => Box::<rxing::qrcode::QRCodeWriter>::default(),
        Kind::DataMatrix => Box::<rxing::datamatrix::DataMatrixWriter>::default(),
        Kind::Pdf417 => Box::<rxing::pdf417::PDF417Writer>::default(),
        Kind::Aztec => Box::<rxing::aztec::AztecWriter>::default(),
        Kind::Code128 | Kind::Gs1_128 => Box::<Code128Writer>::default(),
        Kind::Code39 => Box::<Code39Writer>::default(),
        Kind::Code93 => Box::<Code93Writer>::default(),
        Kind::Ean13 => Box::<EAN13Writer>::default(),
        Kind::Ean8 => Box::<EAN8Writer>::default(),
        Kind::UpcA => Box::<UPCAWriter>::default(),
        Kind::UpcE => Box::<UPCEWriter>::default(),
        Kind::Itf | Kind::Itf14 => Box::<ITFWriter>::default(),
        Kind::Codabar => Box::<CodaBarWriter>::default(),
    }
}

fn retail(
    left: Option<&str>,
    groups: &[(usize, usize, &str)],
    right: Option<&str>,
    guards: &[(usize, usize)],
) -> Layout {
    Layout::Retail {
        left: left.map(str::to_string),
        groups: groups
            .iter()
            .map(|(a, b, s)| (*a, *b, s.to_string()))
            .collect(),
        right: right.map(str::to_string),
        guards: guards.to_vec(),
    }
}

/// A whole number of device pixels per module, so edges stay crisp; as many as
/// the size asks for, as far as the image stays within its pixel limit.
pub fn module_px(size_pt: f64, columns: usize, lines: usize) -> usize {
    let wanted = ((size_pt * 4.0 / columns as f64).ceil() as usize).clamp(2, 16);
    let fits = (MAX_BARCODE_PIXELS / (columns * lines).max(1)).isqrt();
    wanted.min(fits).max(1)
}

fn draw(kind: Kind, symbol: &Symbol, opts: &Options) -> Result<(Media, (f64, f64)), String> {
    let quiet = opts.quiet_zone.unwrap_or_else(|| kind.default_quiet()) as usize;
    let cols = symbol.rows[0].len();
    let mut left = quiet;
    let mut right = quiet;
    let total_rows_modules;
    let (width_pt, mut height_pt);
    let text = opts.text && symbol.layout.is_some();
    if symbol.linear {
        let total = cols + 2 * quiet;
        if total > MAX_BARCODE_MODULES {
            return Err(format!(
                "the value is too long to print as {}: {total} modules",
                name(kind)
            ));
        }
        let lines = (total / 4).clamp(50, MAX_BARCODE_LINES);
        width_pt = opts.width.or(opts.size).unwrap_or(200.0);
        let module_pt = width_pt / total as f64;
        let font_pt = opts
            .font_size
            .unwrap_or_else(|| (module_pt * 10.0).min(width_pt / 18.0).clamp(6.0, 12.0));
        // Room for a digit printed outside the bars of a retail code.
        if text {
            if let Some(Layout::Retail {
                left: l, right: r, ..
            }) = &symbol.layout
            {
                let digit =
                    (font_pt * font::ADVANCE as f64 / 1000.0 / module_pt).ceil() as usize + 1;
                if l.is_some() {
                    left = left.max(digit);
                }
                if r.is_some() {
                    right = right.max(digit);
                }
            }
        }
        let total = cols + left + right;
        let module_pt = width_pt / total as f64;
        let band = if text { font_pt * 1.3 } else { 0.0 };
        height_pt = opts.height.unwrap_or(module_pt * lines as f64 + band);
        if height_pt <= band + module_pt {
            return Err(format!(
                "height {height_pt} leaves no room for the bars under a {font_pt}-point text"
            ));
        }
        total_rows_modules = (height_pt / module_pt).ceil() as usize;
        let m = module_px(width_pt, total, total_rows_modules.max(1));
        let px_per_pt = (total * m) as f64 / width_pt;
        let w = total * m;
        let h = ((height_pt * px_per_pt).round() as usize).max(1);
        let band_px = (band * px_per_pt).round() as usize;
        let font_px = font_pt * px_per_pt;
        let mut canvas = Canvas::new(w, h, opts)?;
        let bars_h = h - band_px;
        // Retail guard bars reach halfway into the text.
        let guard_h = if text { bars_h + band_px / 2 } else { bars_h };
        let guards: &[(usize, usize)] = match &symbol.layout {
            Some(Layout::Retail { guards, .. }) if text => guards,
            _ => &[],
        };
        let row = &symbol.rows[0];
        let mut x = 0;
        while x < cols {
            if !row[x] {
                x += 1;
                continue;
            }
            let start = x;
            while x < cols && row[x] {
                x += 1;
            }
            let long = guards.iter().any(|(a, b)| start >= *a && start < *b);
            canvas.rect(
                (left + start) * m,
                0,
                (x - start) * m,
                if long { guard_h } else { bars_h },
            );
        }
        if symbol.bearers {
            let thick = 2 * m;
            canvas.rect(0, 0, w, thick.min(bars_h));
            canvas.rect(0, bars_h.saturating_sub(thick), w, thick.min(bars_h));
        }
        if text {
            let baseline = h as f64 - band_px as f64 * 0.12;
            match &symbol.layout {
                Some(Layout::Centred(s)) => {
                    canvas.text(s, (w as f64) / 2.0, baseline, font_px, w as f64);
                }
                Some(Layout::Retail {
                    left: l,
                    groups,
                    right: r,
                    ..
                }) => {
                    let mpx = m as f64;
                    let side = font_px * 0.8;
                    if let Some(s) = l {
                        let cx = (left as f64 - 1.0) * mpx - text_width(s, side) / 2.0;
                        canvas.text(s, cx, baseline, side, left as f64 * mpx);
                    }
                    for (a, b, s) in groups {
                        let cx = (left as f64 + (*a + *b) as f64 / 2.0) * mpx;
                        canvas.text(s, cx, baseline, font_px, (*b - *a) as f64 * mpx);
                    }
                    if let Some(s) = r {
                        let cx = ((left + cols) as f64 + 1.0) * mpx + text_width(s, side) / 2.0;
                        canvas.text(s, cx, baseline, side, right as f64 * mpx);
                    }
                }
                None => {}
            }
        }
        return finish(canvas, opts.rotate, (width_pt, height_pt));
    }
    // Two-dimensional symbols.
    let rows = symbol.rows.len();
    let (tw, th) = (cols + left + right, rows + 2 * quiet);
    let natural_h = |w: f64| w * th as f64 / tw as f64;
    let width = opts.width.or(opts.size);
    width_pt = match (width, opts.height) {
        (Some(w), _) => w,
        (None, Some(h)) => h * tw as f64 / th as f64,
        (None, None) => 144.0,
    };
    height_pt = opts.height.unwrap_or_else(|| natural_h(width_pt));
    if kind.is_square() {
        height_pt = width_pt;
    }
    if width_pt > MAX_SIDE_PT || height_pt > MAX_SIDE_PT {
        return Err(format!(
            "the barcode would print larger than {MAX_SIDE_PT} points"
        ));
    }
    if tw * th > MAX_BARCODE_PIXELS {
        return Err("the barcode is too large to draw".into());
    }
    let m = module_px(width_pt, tw, th);
    let w = tw * m;
    // Rows stretch when the height asked for is not the natural one.
    let h = ((height_pt / width_pt * w as f64).round() as usize).max(1);
    let mut canvas = Canvas::new(w, h, opts)?;
    let row_px = |r: usize| ((r + quiet) * h) / th;
    for (r, row) in symbol.rows.iter().enumerate() {
        let (y0, y1) = (row_px(r), row_px(r + 1));
        let mut x = 0;
        while x < cols {
            if !row[x] {
                x += 1;
                continue;
            }
            let start = x;
            while x < cols && row[x] {
                x += 1;
            }
            canvas.rect((left + start) * m, y0, (x - start) * m, y1 - y0);
        }
    }
    finish(canvas, opts.rotate, (width_pt, height_pt))
}

fn text_width(s: &str, font_px: f64) -> f64 {
    s.chars().count() as f64 * f64::from(font::ADVANCE) / 1000.0 * font_px
}

struct Canvas {
    pixmap: Pixmap,
    ink: [u8; 3],
    paper: Option<[u8; 3]>,
}

impl Canvas {
    fn new(w: usize, h: usize, opts: &Options) -> Result<Canvas, String> {
        if w * h > MAX_BARCODE_PIXELS * 2 {
            return Err("the barcode is too large to draw".into());
        }
        let mut pixmap =
            Pixmap::new(w as u32, h as u32).ok_or("the barcode is too large to draw")?;
        let ink = opts.color.as_deref().map_or([0, 0, 0], rgb);
        let paper = if opts.transparent {
            None
        } else {
            Some(opts.background.as_deref().map_or([255, 255, 255], rgb))
        };
        if let Some([r, g, b]) = paper {
            pixmap.fill(tiny_skia::Color::from_rgba8(r, g, b, 255));
        }
        Ok(Canvas { pixmap, ink, paper })
    }

    fn paint(&self) -> Paint<'static> {
        let mut paint = Paint::default();
        let [r, g, b] = self.ink;
        paint.set_color_rgba8(r, g, b, 255);
        paint
    }

    fn rect(&mut self, x: usize, y: usize, w: usize, h: usize) {
        if let Some(rect) = Rect::from_xywh(x as f32, y as f32, w as f32, h as f32) {
            let mut paint = self.paint();
            paint.anti_alias = false;
            self.pixmap
                .fill_rect(rect, &paint, Transform::identity(), None);
        }
    }

    /// Draws `s` centred on `cx` with its baseline at `y`, shrunk to fit
    /// `room` pixels.
    fn text(&mut self, s: &str, cx: f64, y: f64, font_px: f64, room: f64) {
        let wanted = text_width(s, font_px);
        let font_px = if wanted > room && wanted > 0.0 {
            font_px * room / wanted
        } else {
            font_px
        };
        let scale = (font_px / 1000.0) as f32;
        let start = cx - text_width(s, font_px) / 2.0;
        let mut pb = PathBuilder::new();
        for (i, c) in s.chars().enumerate() {
            let Some(glyph) = (c as usize)
                .checked_sub(32)
                .and_then(|i| font::GLYPHS.get(i))
            else {
                continue;
            };
            let ox = start as f32 + i as f32 * font::ADVANCE * scale;
            glyph_path(glyph, ox, y as f32, scale, &mut pb);
        }
        if let Some(path) = pb.finish() {
            let paint = self.paint();
            self.pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
}

/// Adds a glyph's outline from `font::GLYPHS`, flipping y.
fn glyph_path(glyph: &str, ox: f32, oy: f32, scale: f32, pb: &mut PathBuilder) {
    let mut nums = Vec::with_capacity(6);
    let mut op = ' ';
    let flush = |op: char, nums: &mut Vec<f32>, pb: &mut PathBuilder| {
        let p: Vec<(f32, f32)> = nums
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| (ox + c[0] * scale, oy - c[1] * scale))
            .collect();
        match (op, p.as_slice()) {
            ('M', [a]) => pb.move_to(a.0, a.1),
            ('L', [a]) => pb.line_to(a.0, a.1),
            ('Q', [a, b]) => pb.quad_to(a.0, a.1, b.0, b.1),
            ('C', [a, b, c]) => pb.cubic_to(a.0, a.1, b.0, b.1, c.0, c.1),
            _ => {}
        }
        nums.clear();
    };
    for token in glyph
        .replace('M', " M ")
        .replace('L', " L ")
        .replace('Q', " Q ")
        .replace('C', " C ")
        .replace('Z', " Z ")
        .split_whitespace()
    {
        match token {
            "M" | "L" | "Q" | "C" => {
                flush(op, &mut nums, pb);
                op = token.chars().next().unwrap_or(' ');
            }
            "Z" => {
                flush(op, &mut nums, pb);
                pb.close();
                op = ' ';
            }
            n => nums.push(n.parse().unwrap_or(0.0)),
        }
    }
    flush(op, &mut nums, pb);
}

/// Rotates, encodes and wraps the drawing.
fn finish(canvas: Canvas, rotate: u32, printed: (f64, f64)) -> Result<(Media, (f64, f64)), String> {
    let (w, h) = (
        canvas.pixmap.width() as usize,
        canvas.pixmap.height() as usize,
    );
    let src: Vec<[u8; 4]> = canvas
        .pixmap
        .pixels()
        .iter()
        .map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    let (rw, rh) = if rotate % 180 == 90 { (h, w) } else { (w, h) };
    let mut out = vec![[0u8; 4]; w * h];
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = match rotate {
                90 => (h - 1 - y, x),
                180 => (w - 1 - x, h - 1 - y),
                270 => (y, w - 1 - x),
                _ => (x, y),
            };
            out[ny * rw + nx] = src[y * w + x];
        }
    }
    let gray = canvas.paper.is_some_and(|p| p[0] == p[1] && p[1] == p[2])
        && canvas.ink[0] == canvas.ink[1]
        && canvas.ink[1] == canvas.ink[2];
    let (pixels, color): (Vec<u8>, _) = if canvas.paper.is_none() {
        (out.concat(), image::ExtendedColorType::Rgba8)
    } else if gray {
        (
            out.iter().map(|p| p[0]).collect(),
            image::ExtendedColorType::L8,
        )
    } else {
        (
            out.iter().flat_map(|p| [p[0], p[1], p[2]]).collect(),
            image::ExtendedColorType::Rgb8,
        )
    };
    let mut png = Vec::new();
    use image::ImageEncoder;
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&pixels, rw as u32, rh as u32, color)
        .map_err(|e| format!("cannot encode the barcode: {e}"))?;
    let media = media::inspect(png, Some("image/png"))?;
    let printed = if rotate % 180 == 90 {
        (printed.1, printed.0)
    } else {
        printed
    };
    Ok((media, printed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(kind: &str, value: &str) -> Result<(Media, (f64, f64)), String> {
        barcode(kind, value, &Options::default())
    }

    #[test]
    fn barcodes_validate_and_are_real_pngs() {
        for (kind, value) in [
            ("qr", "https://example.com"),
            ("code128", "SKU-123 a"),
            ("code39", "ABC-123"),
            ("ean13", "5901234123457"),
            ("ean13", "590123412345"),
            ("ean8", "96385074"),
            ("datamatrix", "Hello"),
            ("pdf417", "Hello PDF417"),
            ("aztec", "Hello Aztec"),
            ("code93", "Code 93"),
            ("upca", "03600029145"),
            ("upca", "036000291452"),
            ("upce", "0123456"),
            ("itf", "1234567890"),
            ("itf14", "1540014128876"),
            ("codabar", "A40156B"),
            ("codabar", "40156"),
            ("gs1-128", "(01)09501101530003(10)ABC123"),
        ] {
            let (media, size) = plain(kind, value).unwrap_or_else(|e| panic!("{kind}: {e}"));
            assert!(media.px.0 >= 50, "{kind}: {:?}", media.px);
            assert!(size.0 > 0.0 && size.1 > 0.0, "{kind}");
        }
        for (kind, value) in [
            ("ean13", "5901234123458"),
            ("ean13", "12"),
            ("ean8", "96385075"),
            ("code39", "lower"),
            ("code128", "caf\u{e9}"),
            ("upca", "036000291453"),
            ("upce", "2123456"),
            ("itf", "123"),
            ("itf14", "15400141288760"),
            ("codabar", "A123"),
            ("codabar", "12x"),
            ("gs1-128", "0109501101530003"),
            ("gs1-128", "(01)09501101530004"),
            ("gs1-128", "(01)0950110153000"),
            ("gs1-128", "(1)x"),
            ("maxicode", "x"),
            ("qr", ""),
        ] {
            assert!(plain(kind, value).is_err(), "{kind} {value}");
        }
        assert!(plain("qr", &"x".repeat(MAX_BARCODE_INPUT + 1)).is_err());
        assert!(plain("code128", &"x".repeat(MAX_BARCODE_INPUT)).is_err());
        let err = plain("qr", &"x".repeat(4000)).unwrap_err();
        assert!(err.contains("too long"), "{err}");
    }

    #[test]
    fn gs1_puts_fnc1_where_the_standard_wants_it() {
        let (encoded, text) = gs1("(10)AB(01)09501101530003(21)X").unwrap();
        assert_eq!(
            encoded,
            "\u{f1}10AB\u{f1}0109501101530003".to_string() + "21X"
        );
        assert_eq!(text, "(10)AB(01)09501101530003(21)X");
    }

    #[test]
    fn options_are_checked() {
        let o = |f: &dyn Fn(&mut Options)| {
            let mut opts = Options::default();
            f(&mut opts);
            opts
        };
        assert!(barcode("qr", "x", &o(&|o| o.text = true)).is_err());
        assert!(barcode("code128", "x", &o(&|o| o.font_size = Some(9.0))).is_err());
        assert!(barcode(
            "qr",
            "x",
            &o(&|o| {
                o.width = Some(100.0);
                o.height = Some(50.0)
            })
        )
        .is_err());
        assert!(barcode(
            "code128",
            "x",
            &o(&|o| {
                o.width = Some(100.0);
                o.size = Some(100.0)
            })
        )
        .is_err());
        assert!(barcode("code128", "x", &o(&|o| o.ec = Some("H".into()))).is_err());
        assert!(barcode("qr", "x", &o(&|o| o.ec = Some("Z".into()))).is_err());
        assert!(barcode("pdf417", "x", &o(&|o| o.ec = Some("9".into()))).is_err());
        assert!(barcode("aztec", "x", &o(&|o| o.ec = Some("50".into()))).is_ok());
    }

    #[test]
    fn sizes_rotation_and_colours_land_in_the_png() {
        let opts = Options {
            width: Some(300.0),
            height: Some(80.0),
            text: true,
            rotate: 90,
            color: Some("CC0000".into()),
            ..Options::default()
        };
        let (media, printed) = barcode("ean13", "590123412345", &opts).unwrap();
        assert_eq!(printed, (80.0, 300.0));
        assert!(media.px.1 > media.px.0, "rotated: {:?}", media.px);
        let img = image::load_from_memory(&media.bytes).unwrap().to_rgb8();
        assert!(img.pixels().any(|p| p.0 == [204, 0, 0]));
        let transparent = Options {
            transparent: true,
            ..Options::default()
        };
        let (media, _) = barcode("qr", "x", &transparent).unwrap();
        let img = image::load_from_memory(&media.bytes).unwrap().to_rgba8();
        assert_eq!(img.get_pixel(0, 0).0[3], 0);
    }

    #[test]
    fn no_size_draws_more_than_the_pixel_limit() {
        // Without a limit this asks for 57,000 by 5,600 pixels.
        for (kind, value) in [("code128", "x".repeat(360)), ("qr", "x".repeat(2000))] {
            let opts = Options {
                size: Some(MAX_SIDE_PT),
                ..Options::default()
            };
            let (media, printed) = barcode(kind, &value, &opts).unwrap();
            let pixels = media.px.0 as usize * media.px.1 as usize;
            assert!(pixels <= MAX_BARCODE_PIXELS, "{kind}: {:?}", media.px);
            assert_eq!(printed.0, MAX_SIDE_PT, "the printed size is unaffected");
        }
        assert_eq!(module_px(MAX_SIDE_PT, 4096, 400), 1);
        assert_eq!(module_px(100.0, 29, 29), 14);
        assert_eq!(module_px(MAX_SIDE_PT, 185, 185), 10);
    }

    #[test]
    fn module_size_is_a_whole_number() {
        let opts = Options {
            size: Some(100.0),
            ..Options::default()
        };
        let (media, _) = barcode("qr", "a", &opts).unwrap();
        // Version 1 is 21 modules plus an 8 module quiet zone.
        assert_eq!(media.px.0 % 29, 0);
    }
}
