//! Barcodes: the symbol is encoded by rxing, then drawn here as an SVG,
//! with a PNG of it for readers without SVG. The PNG has whole device pixels
//! per module, so square edges stay crisp. Around the symbol go an optional
//! line of text, colours, a quiet zone and a quarter-turn rotation; QR codes
//! also take module and finder shapes, gradients, a logo and a label.

use std::fmt::Write as _;
use std::sync::Arc;

use base64::Engine as _;
use rxing::{BarcodeFormat, EncodeHints, Writer};

use crate::rich::font;
use crate::rich::media::{self, Media, MAX_SIDE_PT};
use crate::rich::qr::{self, Hole};
use crate::rich::svg;

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

pub const SYMBOLOGIES: &str = "qr, gs1-qr, swissqr, datamatrix, pdf417, aztec, code128, gs1-128, \
                               code39, code93, ean13, ean8, upca, upce, itf, itf14 or codabar";
/// The smallest QR module printed, in points: 0.25 mm. Phone cameras miss
/// finer ones at reading distance; GS1 asks for at least 0.396 mm.
pub const MIN_QR_MODULE_PT: f64 = 0.25 / 25.4 * 72.0;
/// The Swiss QR-bill's code, without its quiet zone: 46 mm.
const SWISS_SIDE_PT: f64 = 46.0 / 25.4 * 72.0;
/// The quiet zone around it: 5 mm.
const SWISS_QUIET_PT: f64 = 5.0 / 25.4 * 72.0;
/// The Swiss cross in its middle: 7 mm, white border included.
const SWISS_CROSS_SHARE: f64 = 7.0 / 46.0;
/// The lowest contrast accepted between bars and background, WCAG's for
/// graphical objects.
pub const MIN_CONTRAST: f64 = 3.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Qr,
    /// QR with GS1 element strings: FNC1 in first position.
    Gs1Qr,
    /// The Swiss QR-bill's QR code, with its cross.
    SwissQr,
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
            "gs1-qr" | "gs1qr" => Kind::Gs1Qr,
            "swissqr" | "swiss-qr" => Kind::SwissQr,
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
            Kind::Qr | Kind::Gs1Qr | Kind::SwissQr | Kind::DataMatrix | Kind::Pdf417 | Kind::Aztec
        )
    }

    fn is_qr(self) -> bool {
        matches!(self, Kind::Qr | Kind::Gs1Qr | Kind::SwissQr)
    }

    /// Whether the symbol must print square.
    fn is_square(self) -> bool {
        matches!(self, Kind::DataMatrix | Kind::Aztec) || self.is_qr()
    }

    fn format(self) -> BarcodeFormat {
        match self {
            Kind::Qr | Kind::Gs1Qr | Kind::SwissQr => BarcodeFormat::QR_CODE,
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
            Kind::Qr | Kind::Gs1Qr | Kind::SwissQr => 4,
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
    /// Light bars on a dark background: allowed, and the defaults swap.
    pub invert: bool,
    pub quiet_zone: Option<u32>,
    pub rotate: u32,
    /// Error correction as given: a level for QR, a number for the others.
    pub ec: Option<String>,
    /// QR only from here on.
    pub style: qr::Style,
    pub eye_color: Option<String>,
    pub eye_center_color: Option<String>,
    pub gradient: Option<Gradient>,
    pub logo: Option<Logo>,
    pub label: Option<String>,
    pub version: Option<u32>,
}

/// A gradient across the symbol, from `color` to `end`.
#[derive(Debug, Clone)]
pub struct Gradient {
    pub radial: bool,
    /// `RRGGBB`.
    pub end: String,
    /// Degrees clockwise from left to right, for a linear one.
    pub angle: f64,
}

/// A picture in the middle of a QR code.
#[derive(Debug, Clone)]
pub struct Logo {
    pub media: Arc<Media>,
    /// Its width as a share of the symbol's.
    pub share: f64,
}

impl Options {
    /// The QR-only options that are set, by their filter names.
    fn qr_only(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        let s = &self.style;
        if s.shape != qr::Shape::Square {
            out.push("style");
        }
        if (s.scale - 1.0).abs() > 1e-9 {
            out.push("module_scale");
        }
        if s.eye != qr::EyeShape::Square {
            out.push("eye");
        }
        if s.eye_center != qr::EyeShape::Square {
            out.push("eye_center");
        }
        for (set, name) in [
            (self.eye_color.is_some(), "eye_color"),
            (self.eye_center_color.is_some(), "eye_center_color"),
            (self.gradient.is_some(), "gradient"),
            (self.logo.is_some(), "logo"),
            (self.label.is_some(), "label"),
            (self.version.is_some(), "version"),
        ] {
            if set {
                out.push(name);
            }
        }
        out
    }

    /// Every option set, by filter name, for the Swiss QR-bill to refuse.
    fn any_look(&self) -> Vec<&'static str> {
        let mut out = self.qr_only();
        for (set, name) in [
            (self.size.is_some(), "size"),
            (self.width.is_some(), "width"),
            (self.height.is_some(), "height"),
            (self.color.is_some(), "color"),
            (self.background.is_some() || self.transparent, "background"),
            (self.invert, "invert"),
            (self.quiet_zone.is_some(), "quiet_zone"),
            (self.ec.is_some(), "ec"),
        ] {
            if set {
                out.push(name);
            }
        }
        out
    }

    /// The bars' colour.
    fn ink(&self) -> [u8; 3] {
        self.color
            .as_deref()
            .map_or(if self.invert { [255; 3] } else { [0; 3] }, rgb)
    }

    /// The background's colour, `None` when transparent.
    fn paper(&self) -> Option<[u8; 3]> {
        if self.transparent {
            return None;
        }
        Some(
            self.background
                .as_deref()
                .map_or(if self.invert { [0; 3] } else { [255; 3] }, rgb),
        )
    }
}

/// WCAG relative luminance.
fn luminance([r, g, b]: [u8; 3]) -> f64 {
    let channel = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

/// WCAG contrast ratio, from 1 to 21.
pub fn contrast(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Refuses colours a scanner cannot tell apart, and light bars on a dark
/// background unless asked for. A transparent background is the page's,
/// which is not known here.
fn check_colours(opts: &Options) -> Result<(), String> {
    let Some(paper) = opts.paper() else {
        return Ok(());
    };
    let mut inks = vec![("color", opts.ink())];
    for (name, c) in [
        ("eye_color", &opts.eye_color),
        ("eye_center_color", &opts.eye_center_color),
    ] {
        if let Some(c) = c {
            inks.push((name, rgb(c)));
        }
    }
    if let Some(g) = &opts.gradient {
        inks.push(("the gradient's end", rgb(&g.end)));
    }
    for (name, ink) in inks {
        let ratio = contrast(ink, paper);
        if ratio < MIN_CONTRAST {
            return Err(format!(
                "{name} {} on background {} has a contrast of {ratio:.1}:1; scanners need at \
                 least {MIN_CONTRAST}:1",
                hex(ink),
                hex(paper)
            ));
        }
        if !opts.invert && luminance(ink) > luminance(paper) {
            return Err(format!(
                "{name} {} is lighter than background {}: light bars on a dark background do \
                 not scan everywhere; pass invert=true to keep them",
                hex(ink),
                hex(paper)
            ));
        }
    }
    Ok(())
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
    /// Modules cleared for a logo or the Swiss cross.
    hole: Option<Hole>,
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
    if kind.is_matrix() && opts.text {
        return Err("text is drawn under linear codes only; a qr code takes a label".into());
    }
    if !kind.is_matrix() && !opts.text && opts.font_size.is_some() {
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
    if kind == Kind::SwissQr {
        if let Some(first) = opts.any_look().first() {
            return Err(format!(
                "swissqr has the size, colours and error correction the QR-bill sets: {first} \
                 cannot change them"
            ));
        }
        if !(value.starts_with("SPC\n") || value.starts_with("SPC\r\n")) {
            return Err(
                "swissqr takes a QR-bill payload, which starts with SPC on its own line; \
                 qrdata(\"swiss\") builds one"
                    .into(),
            );
        }
        if value.chars().count() > 997 {
            return Err("a QR-bill payload holds at most 997 characters".into());
        }
    } else if !kind.is_qr() {
        if let Some(first) = opts.qr_only().first() {
            return Err(format!(
                "{first} applies to qr and gs1-qr, not {}",
                name(kind)
            ));
        }
    }
    if let Some(label) = &opts.label {
        if opts.height.is_some() {
            return Err("a label sets the height: give width or size".into());
        }
        if label.is_empty() || label.chars().count() > 200 {
            return Err("a label holds 1 to 200 characters".into());
        }
        if let Some(bad) = label.chars().find(|c| !(' '..='~').contains(c)) {
            return Err(format!(
                "a label is printable ASCII, since it is drawn with the barcode's own font, \
                 not {bad:?}"
            ));
        }
    }
    if opts.label.is_none() && opts.font_size.is_some() && kind.is_matrix() {
        return Err("font_size needs a label".into());
    }
    check_colours(opts)?;
    let ec = match (&opts.ec, kind) {
        (_, Kind::SwissQr) => Some("M".to_string()),
        (None, Kind::Qr | Kind::Gs1Qr) if opts.logo.is_some() => Some("H".to_string()),
        (ec, _) => ec.clone(),
    };
    let mut symbol = encode(kind, value, ec.as_deref(), opts.version)?;
    if let Some(logo) = &opts.logo {
        let level = ec.as_deref().unwrap_or("M").to_ascii_uppercase();
        let budget = qr::logo_budget(&level)
            .ok_or("a logo needs error correction M, Q or H; H is the default with one")?;
        loop {
            let n = symbol.rows.len();
            if qr::hole_fits(n, Hole::for_logo(n, logo.share), budget) {
                symbol.hole = Some(Hole::for_logo(n, logo.share));
                break;
            }
            let v = qr::version(n).unwrap_or(40);
            if opts.version.is_some() || v >= 40 {
                return Err(format!(
                    "a logo {:.0}% wide hides too much of this version {v} code at error \
                     correction {level}; use a smaller logo_size{}",
                    logo.share * 100.0,
                    if opts.version.is_some() {
                        ", a higher version or more error correction"
                    } else {
                        " or more error correction"
                    }
                ));
            }
            symbol = encode(kind, value, ec.as_deref(), Some(v as u32 + 1))?;
        }
    }
    if kind == Kind::SwissQr {
        symbol.hole = Some(Hole::covering(symbol.rows.len(), SWISS_CROSS_SHARE));
    }
    draw(kind, &symbol, opts)
}

/// What a barcode is, as alt text that reveals none of its value: a code
/// may hold a password or an account.
pub fn alt_text(kind: &str) -> Option<String> {
    let kind = Kind::parse(kind)?;
    Some(if kind.is_matrix() {
        format!("{} code", name(kind))
    } else {
        format!("{} barcode", name(kind))
    })
}

fn name(kind: Kind) -> &'static str {
    match kind {
        Kind::Qr => "QR",
        Kind::Gs1Qr => "GS1 QR",
        Kind::SwissQr => "Swiss QR",
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

/// A GS1 element string, `(01)09501101530003(10)ABC`, read into its
/// application identifiers and data, checked: lengths the GS1 tables fix,
/// check digits of `(00)`, `(01)` and `(02)`, and the characters GS1 allows.
/// The flag says whether the field has a fixed length.
pub(crate) fn gs1_fields(value: &str) -> Result<Vec<(String, String, bool)>, String> {
    let mut rest = value;
    if !rest.starts_with('(') {
        return Err(
            "gs1-128 takes application identifiers in parentheses, such as (01)09501101530003(10)ABC"
                .into(),
        );
    }
    let mut fields = Vec::new();
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
        fields.push((ai.to_string(), data.to_string(), fixed.is_some()));
        rest = &after[data_end..];
    }
    if !rest.is_empty() {
        return Err(format!(
            "unexpected {rest:?} after the last application identifier"
        ));
    }
    Ok(fields)
}

/// Reads `(01)09501101530003(10)ABC` into Code 128 input with FNC1, and
/// the text to print.
fn gs1(value: &str) -> Result<(String, String), String> {
    const FNC1: char = '\u{f1}';
    let mut encoded = String::from(FNC1);
    let mut need_separator = false;
    for (ai, data, fixed) in gs1_fields(value)? {
        if need_separator {
            encoded.push(FNC1);
        }
        encoded.push_str(&ai);
        encoded.push_str(&data);
        need_separator = !fixed;
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
        return Ok(kind.is_qr().then(|| "M".to_string()));
    };
    let number = |lo: u32, hi: u32, what: &str| {
        ec.parse::<u32>()
            .ok()
            .filter(|n| (lo..=hi).contains(n))
            .map(|n| Some(n.to_string()))
            .ok_or_else(|| format!("ec for {} is {what} from {lo} to {hi}", name(kind)))
    };
    match kind {
        Kind::Qr | Kind::Gs1Qr | Kind::SwissQr => match ec.to_ascii_uppercase().as_str() {
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

fn encode(
    kind: Kind,
    value: &str,
    ec: Option<&str>,
    version: Option<u32>,
) -> Result<Symbol, String> {
    let mut hints = EncodeHints {
        Margin: Some("0".to_string()),
        ErrorCorrection: ec_hint(kind, ec)?,
        QrVersion: version.map(|v| v.to_string()),
        ..EncodeHints::default()
    };
    let mut layout = None;
    let mut bearers = false;
    let input: String = match kind {
        Kind::Gs1Qr => {
            let (encoded, _) = gs1(value).map_err(|e| e.replace("gs1-128", "gs1-qr"))?;
            hints.Gs1Format = Some(true);
            // QR marks GS1 with its mode header; fields end with a group
            // separator instead of FNC1.
            encoded
                .trim_start_matches('\u{f1}')
                .replace('\u{f1}', "\u{1d}")
        }
        Kind::Qr | Kind::SwissQr | Kind::DataMatrix | Kind::Pdf417 | Kind::Aztec => {
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
            if value.len() > 80 {
                return Err("itf takes at most 80 digits".into());
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
        .map_err(|e| match version {
            Some(v) if e.to_string().contains("requested version") => {
                format!("the value does not fit a version {v} {} code", name(kind))
            }
            _ => rxing_error(kind, e),
        })?;
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
        hole: None,
    })
}

fn encoder(kind: Kind) -> Box<dyn Writer> {
    use rxing::oned::*;
    match kind {
        Kind::Qr | Kind::Gs1Qr | Kind::SwissQr => Box::<rxing::qrcode::QRCodeWriter>::default(),
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
    let wanted = ((size_pt * 4.0 / columns.max(1) as f64).ceil() as usize).clamp(2, 16);
    let fits = columns
        .checked_mul(lines)
        .map_or(0, |cells| (MAX_BARCODE_PIXELS / cells.max(1)).isqrt());
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
                // At most the widest quiet zone, however large the text.
                let digit = ((font_pt * font::ADVANCE as f64 / 1000.0 / module_pt).ceil() as usize
                    + 1)
                .min(MAX_QUIET as usize);
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
        if total > MAX_BARCODE_MODULES {
            return Err(format!(
                "the value is too long to print as {} with its text: {total} modules",
                name(kind)
            ));
        }
        let band = if text { font_pt * 1.3 } else { 0.0 };
        height_pt = opts.height.unwrap_or(module_pt * lines as f64 + band);
        if !height_pt.is_finite() || height_pt > MAX_SIDE_PT {
            return Err(format!(
                "the barcode would print taller than {MAX_SIDE_PT} points"
            ));
        }

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
    let swiss = kind == Kind::SwissQr;
    // The QR-bill's 5 mm quiet zone, in whole modules of its 46 mm code.
    let quiet = if swiss {
        (SWISS_QUIET_PT / (SWISS_SIDE_PT / rows as f64)).ceil() as usize
    } else {
        quiet
    };
    let (tw, th) = (cols + 2 * quiet, rows + 2 * quiet);
    let natural_h = |w: f64| w * th as f64 / tw as f64;
    let width = opts.width.or(opts.size);
    width_pt = match (width, opts.height) {
        _ if swiss => SWISS_SIDE_PT * tw as f64 / rows as f64,
        (Some(w), _) => w,
        (None, Some(h)) => h * tw as f64 / th as f64,
        (None, None) => 144.0,
    };
    height_pt = opts.height.unwrap_or_else(|| natural_h(width_pt));
    if kind.is_square() {
        height_pt = width_pt;
    }
    if kind.is_qr() {
        let module_pt = width_pt / tw as f64;
        if module_pt < MIN_QR_MODULE_PT {
            return Err(format!(
                "its modules would print {:.2} mm wide, and phones need at least 0.25 mm: make \
                 it at least {:.0} points wide, or the value shorter",
                module_pt * 25.4 / 72.0,
                (MIN_QR_MODULE_PT * tw as f64).ceil()
            ));
        }
    }
    let font_pt = opts
        .font_size
        .unwrap_or_else(|| (width_pt / 12.0).clamp(6.0, 14.0));
    let band_pt = if opts.label.is_some() {
        font_pt * 1.5
    } else {
        0.0
    };
    let symbol_pt = height_pt;
    height_pt += band_pt;
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
    let px_per_pt = w as f64 / width_pt;
    // Rows stretch when the height asked for is not the natural one.
    let sym_h = ((symbol_pt * px_per_pt).round() as usize).max(1);
    let band_px = (band_pt * px_per_pt).round() as usize;
    let h = sym_h + band_px;
    let mut canvas = Canvas::new(w, h, opts)?;
    if kind.is_qr() {
        qr_symbol(&mut canvas, symbol, opts, m, quiet, swiss)?;
    } else {
        let row_px = |r: usize| ((r + quiet) * sym_h) / th;
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
                canvas.rect((quiet + start) * m, y0, (x - start) * m, y1 - y0);
            }
        }
    }
    if let Some(label) = &opts.label {
        let baseline = sym_h as f64 + band_px as f64 * 0.72;
        canvas.text(
            label,
            w as f64 / 2.0,
            baseline,
            font_pt * px_per_pt,
            w as f64 * 0.96,
        );
    }
    finish(canvas, opts.rotate, (width_pt, height_pt))
}

/// Draws a QR symbol: styled modules, the finder patterns, and the logo
/// or Swiss cross in its hole.
fn qr_symbol(
    canvas: &mut Canvas,
    symbol: &Symbol,
    opts: &Options,
    m: usize,
    quiet: usize,
    swiss: bool,
) -> Result<(), String> {
    let n = symbol.rows.len();
    let mf = m as f64;
    let origin = ((quiet * m) as f64, (quiet * m) as f64);
    let side = n as f64 * mf;
    let data = match &opts.gradient {
        Some(g) => canvas.gradient(opts.ink(), g, origin, side),
        None => canvas.ink.clone(),
    };
    let eye = opts
        .eye_color
        .as_deref()
        .map_or_else(|| data.clone(), |c| hex(rgb(c)));
    let centre = opts
        .eye_center_color
        .as_deref()
        .map_or_else(|| eye.clone(), |c| hex(rgb(c)));
    let hole = symbol.hole;
    let hidden = |r: usize, c: usize| hole.is_some_and(|h| h.contains(r, c));
    let (d, crisp) = qr::modules(&symbol.rows, &hidden, &opts.style, mf, origin);
    canvas.path(&data, crisp, false, &d);
    let (frame, middle, frame_crisp, middle_crisp) = qr::eyes(n, &opts.style, mf, origin);
    canvas.path(&eye, frame_crisp, true, &frame);
    canvas.path(&centre, middle_crisp, true, &middle);
    match (hole, &opts.logo) {
        (Some(h), Some(logo)) => {
            let at = (h.start + 1) as f64 * mf;
            let size = (h.size - 2) as f64 * mf;
            canvas.logo(&logo.media, origin.0 + at, origin.1 + at, size)?;
        }
        _ if swiss => {
            let mid = origin.0 + side / 2.0;
            canvas.swiss_cross(mid, side * SWISS_CROSS_SHARE);
        }
        _ => {}
    }
    Ok(())
}

fn text_width(s: &str, font_px: f64) -> f64 {
    s.chars().count() as f64 * f64::from(font::ADVANCE) / 1000.0 * font_px
}

/// A drawing in device pixels, kept as SVG: the PNG is drawn from it.
struct Canvas {
    w: usize,
    h: usize,
    /// The bars' fill.
    ink: String,
    paper: Option<[u8; 3]>,
    defs: String,
    /// Rectangles in the ink, on the pixel grid.
    bars: String,
    /// Text outlines in the ink.
    glyphs: String,
    /// Further elements, drawn last.
    shapes: String,
    /// Whether drawing it needs the system fonts: an SVG logo with text.
    fonts: bool,
}

impl Canvas {
    fn new(w: usize, h: usize, opts: &Options) -> Result<Canvas, String> {
        let too_large = || "the barcode is too large to draw".to_string();
        if w.checked_mul(h)
            .is_none_or(|area| area > MAX_BARCODE_PIXELS * 2)
            || u32::try_from(w).is_err()
            || u32::try_from(h).is_err()
            || w == 0
            || h == 0
        {
            return Err(too_large());
        }
        Ok(Canvas {
            w,
            h,
            ink: hex(opts.ink()),
            paper: opts.paper(),
            defs: String::new(),
            bars: String::new(),
            glyphs: String::new(),
            shapes: String::new(),
            fonts: false,
        })
    }

    fn rect(&mut self, x: usize, y: usize, w: usize, h: usize) {
        if w > 0 && h > 0 {
            let _ = write!(self.bars, "M{x} {y}h{w}v{h}h-{w}z");
        }
    }

    fn path(&mut self, fill: &str, crisp: bool, evenodd: bool, d: &str) {
        if d.is_empty() {
            return;
        }
        let _ = write!(
            self.shapes,
            "<path fill=\"{fill}\"{}{} d=\"{d}\"/>",
            if crisp {
                " shape-rendering=\"crispEdges\""
            } else {
                ""
            },
            if evenodd {
                " fill-rule=\"evenodd\""
            } else {
                ""
            }
        );
    }

    /// Defines a gradient over the square at `origin`, `side` wide, from
    /// `start` to the gradient's end; returns the fill that uses it.
    fn gradient(&mut self, start: [u8; 3], g: &Gradient, origin: (f64, f64), side: f64) -> String {
        let id = "rusticdoc-gradient";
        let (cx, cy) = (origin.0 + side / 2.0, origin.1 + side / 2.0);
        let stops = format!(
            "<stop offset=\"0\" stop-color=\"{}\"/><stop offset=\"1\" stop-color=\"{}\"/>",
            hex(start),
            hex(rgb(&g.end))
        );
        let n = qr::num;
        if g.radial {
            let _ = write!(
                self.defs,
                "<radialGradient id=\"{id}\" gradientUnits=\"userSpaceOnUse\" cx=\"{}\" \
                 cy=\"{}\" r=\"{}\">{stops}</radialGradient>",
                n(cx),
                n(cy),
                n(side * std::f64::consts::FRAC_1_SQRT_2)
            );
        } else {
            // LibreOffice draws nothing for a gradient at exactly 45, 135,
            // 225 or 315 degrees; a tenth of a degree off, nobody can tell.
            let angle = if (g.angle.rem_euclid(90.0) - 45.0).abs() < 1e-9 {
                g.angle + 0.1
            } else {
                g.angle
            };
            let (dy, dx) = angle.to_radians().sin_cos();
            // Far enough along the direction to reach both corners.
            let half = side / 2.0 * (dx.abs() + dy.abs());
            let _ = write!(
                self.defs,
                "<linearGradient id=\"{id}\" gradientUnits=\"userSpaceOnUse\" x1=\"{}\" \
                 y1=\"{}\" x2=\"{}\" y2=\"{}\">{stops}</linearGradient>",
                n(cx - dx * half),
                n(cy - dy * half),
                n(cx + dx * half),
                n(cy + dy * half)
            );
        }
        format!("url(#{id})")
    }

    /// Places a picture in the square at (`x`, `y`), `size` wide, keeping
    /// its aspect. An SVG nests as it is: it was rebuilt when it was read.
    fn logo(&mut self, media: &Media, x: f64, y: f64, size: f64) -> Result<(), String> {
        let n = qr::num;
        if media.mime == "image/svg+xml" {
            let inner = std::str::from_utf8(&media.bytes).map_err(|_| "the logo is not UTF-8")?;
            let (vw, vh) = (media.physical_pt.0 / 0.75, media.physical_pt.1 / 0.75);
            let _ = write!(
                self.shapes,
                "<svg x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\" \
                 preserveAspectRatio=\"xMidYMid meet\">{inner}</svg>",
                n(x),
                n(y),
                n(size),
                n(size),
                n(vw),
                n(vh)
            );
            self.fonts = true;
        } else {
            let data = base64::engine::general_purpose::STANDARD.encode(&media.bytes);
            let _ = write!(
                self.shapes,
                "<image x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" \
                 preserveAspectRatio=\"xMidYMid meet\" xlink:href=\"data:{};base64,{data}\"/>",
                n(x),
                n(y),
                n(size),
                n(size),
                media.mime
            );
        }
        Ok(())
    }

    /// The QR-bill's Swiss cross centred at (`mid`, `mid`): a black square
    /// with a white cross, on a white square `side` wide.
    fn swiss_cross(&mut self, mid: f64, side: f64) {
        let n = qr::num;
        let black = side * 6.0 / 7.0;
        let (arm, long) = (black * 6.0 / 32.0, black * 20.0 / 32.0);
        let _ = write!(
            self.shapes,
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#ffffff\"/>\
             <rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#000000\"/>\
             <path fill=\"#ffffff\" d=\"M{} {}h{}v{}h-{}zM{} {}h{}v{}h-{}z\"/>",
            n(mid - side / 2.0),
            n(mid - side / 2.0),
            n(side),
            n(side),
            n(mid - black / 2.0),
            n(mid - black / 2.0),
            n(black),
            n(black),
            n(mid - arm / 2.0),
            n(mid - long / 2.0),
            n(arm),
            n(long),
            n(arm),
            n(mid - long / 2.0),
            n(mid - arm / 2.0),
            n(long),
            n(arm),
            n(long),
        );
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
        let scale = font_px / 1000.0;
        let start = cx - text_width(s, font_px) / 2.0;
        for (i, c) in s.chars().enumerate() {
            let Some(glyph) = (c as usize)
                .checked_sub(32)
                .and_then(|i| font::GLYPHS.get(i))
            else {
                continue;
            };
            let ox = start + i as f64 * f64::from(font::ADVANCE) * scale;
            glyph_path(glyph, ox, y, scale, &mut self.glyphs);
        }
    }
}

/// Adds a glyph's outline from `font::GLYPHS` as path data, flipping y.
fn glyph_path(glyph: &str, ox: f64, oy: f64, scale: f64, d: &mut String) {
    let mut nums: Vec<f64> = Vec::with_capacity(6);
    let mut op = ' ';
    let flush = |op: char, nums: &mut Vec<f64>, d: &mut String| {
        let p: Vec<String> = nums
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| {
                format!(
                    "{} {}",
                    qr::num(ox + c[0] * scale),
                    qr::num(oy - c[1] * scale)
                )
            })
            .collect();
        let want = match op {
            'M' | 'L' => 1,
            'Q' => 2,
            'C' => 3,
            _ => 0,
        };
        if want > 0 && p.len() == want {
            d.push(op);
            d.push_str(&p.join(" "));
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
                flush(op, &mut nums, d);
                op = token.chars().next().unwrap_or(' ');
            }
            "Z" => {
                flush(op, &mut nums, d);
                d.push('Z');
                op = ' ';
            }
            n => nums.push(n.parse().unwrap_or(0.0)),
        }
    }
    flush(op, &mut nums, d);
}

/// Rotates the drawing, writes it as SVG and draws its PNG.
fn finish(canvas: Canvas, rotate: u32, printed: (f64, f64)) -> Result<(Media, (f64, f64)), String> {
    let (w, h) = (canvas.w, canvas.h);
    let (rw, rh) = if rotate % 180 == 90 { (h, w) } else { (w, h) };
    let turn = match rotate {
        90 => format!(" transform=\"translate({h} 0) rotate(90)\""),
        180 => format!(" transform=\"translate({w} {h}) rotate(180)\""),
        270 => format!(" transform=\"translate(0 {w}) rotate(270)\""),
        _ => String::new(),
    };
    let mut body = String::new();
    if !canvas.defs.is_empty() {
        let _ = write!(body, "<defs>{}</defs>", canvas.defs);
    }
    if let Some(paper) = canvas.paper {
        let _ = write!(
            body,
            "<rect width=\"{rw}\" height=\"{rh}\" fill=\"{}\"/>",
            hex(paper)
        );
    }
    let _ = write!(body, "<g{turn}>");
    if !canvas.bars.is_empty() {
        let _ = write!(
            body,
            "<path fill=\"{}\" shape-rendering=\"crispEdges\" d=\"{}\"/>",
            canvas.ink, canvas.bars
        );
    }
    if !canvas.glyphs.is_empty() {
        let _ = write!(
            body,
            "<path fill=\"{}\" d=\"{}\"/>",
            canvas.ink, canvas.glyphs
        );
    }
    body.push_str(&canvas.shapes);
    body.push_str("</g>");
    let doc = |width: &str, height: &str| {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" \
             xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"{width}\" \
             height=\"{height}\" viewBox=\"0 0 {rw} {rh}\">{body}</svg>"
        )
    };
    let pixmap = svg::draw_own(
        &doc(&rw.to_string(), &rh.to_string()),
        rw as u32,
        rh as u32,
        canvas.fonts,
    )?;
    let pixels: Vec<[u8; 4]> = pixmap
        .pixels()
        .iter()
        .map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    let opaque = canvas.paper.is_some() && pixels.iter().all(|p| p[3] == 255);
    let gray = pixels.iter().all(|p| p[0] == p[1] && p[1] == p[2]);
    let (bytes, color): (Vec<u8>, _) = if !opaque {
        (pixels.concat(), image::ExtendedColorType::Rgba8)
    } else if gray {
        (
            pixels.iter().map(|p| p[0]).collect(),
            image::ExtendedColorType::L8,
        )
    } else {
        (
            pixels.iter().flat_map(|p| [p[0], p[1], p[2]]).collect(),
            image::ExtendedColorType::Rgb8,
        )
    };
    let mut png = Vec::new();
    use image::ImageEncoder;
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&bytes, rw as u32, rh as u32, color)
        .map_err(|e| format!("cannot encode the barcode: {e}"))?;
    let png = media::inspect(png, Some("image/png"))?;
    let printed = if rotate % 180 == 90 {
        (printed.1, printed.0)
    } else {
        printed
    };
    let svg = doc(
        &format!("{}pt", qr::num(printed.0)),
        &format!("{}pt", qr::num(printed.1)),
    );
    Ok((media::vector(svg, printed, png), printed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(kind: &str, value: &str) -> Result<(Media, (f64, f64)), String> {
        barcode(kind, value, &Options::default())
    }

    /// The PNG drawn beside the SVG.
    fn png(media: &Media) -> &Media {
        assert_eq!(media.mime, "image/svg+xml");
        media.fallback.as_deref().expect("a PNG")
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
        let img = image::load_from_memory(&png(&media).bytes)
            .unwrap()
            .to_rgb8();
        assert!(img.pixels().any(|p| p.0 == [204, 0, 0]));
        let transparent = Options {
            transparent: true,
            ..Options::default()
        };
        let (media, _) = barcode("qr", "x", &transparent).unwrap();
        let img = image::load_from_memory(&png(&media).bytes)
            .unwrap()
            .to_rgba8();
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
            let media = png(&media);
            let pixels = media.px.0 as usize * media.px.1 as usize;
            assert!(pixels <= MAX_BARCODE_PIXELS, "{kind}: {:?}", media.px);
            assert_eq!(printed.0, MAX_SIDE_PT, "the printed size is unaffected");
        }
        assert_eq!(module_px(MAX_SIDE_PT, 4096, 400), 1);
        assert_eq!(module_px(100.0, 29, 29), 14);
        assert_eq!(module_px(MAX_SIDE_PT, 185, 185), 10);
    }

    #[test]
    fn extreme_sizes_are_refused_or_bounded_never_a_crash() {
        let run = |kind: &str, value: &str, f: &dyn Fn(&mut Options)| {
            let mut opts = Options::default();
            f(&mut opts);
            barcode(kind, value, &opts)
        };
        // A huge text on a tiny code would widen the quiet zones without end.
        let err = run("upca", "03600029145", &|o| {
            o.text = true;
            o.width = Some(1.0);
            o.font_size = Some(MAX_SIDE_PT);
        })
        .unwrap_err();
        assert!(err.contains("taller"), "{err}");
        let (media, printed) = run("upca", "03600029145", &|o| {
            o.text = true;
            o.width = Some(10.0);
            o.font_size = Some(200.0);
        })
        .unwrap();
        assert!(png(&media).px.0 as usize * png(&media).px.1 as usize <= MAX_BARCODE_PIXELS * 2);
        assert_eq!(printed.0, 10.0);

        // A long value whose text-widened zones pass the module limit.
        let long = "1".repeat(78);
        let err = run("itf", &long, &|o| {
            o.quiet_zone = Some(MAX_QUIET);
            o.text = true;
            o.font_size = Some(MAX_SIDE_PT);
            o.width = Some(1.0);
        });
        assert!(
            err.as_ref().is_ok_and(
                |(m, _)| png(m).px.0 as usize * png(m).px.1 as usize <= MAX_BARCODE_PIXELS * 2
            ) || err
                .as_ref()
                .is_err_and(|e| e.contains("too") || e.contains("taller")),
            "{:?}",
            err.as_ref().err()
        );
        // Tall and thin: refused, not an enormous canvas.
        for kind in ["code128", "pdf417"] {
            let result = run(kind, "x", &|o| {
                o.width = Some(0.01);
                o.height = Some(MAX_SIDE_PT);
            });
            match result {
                Ok((media, _)) => {
                    assert!(
                        png(&media).px.0 as usize * png(&media).px.1 as usize
                            <= MAX_BARCODE_PIXELS * 2
                    )
                }
                Err(e) => assert!(e.contains("too large") || e.contains("taller"), "{e}"),
            }
        }
        assert_eq!(module_px(1.0, usize::MAX, 2), 1);
        assert!(Canvas::new(usize::MAX, 2, &Options::default()).is_err());
        assert!(Canvas::new(1 << 33, 1, &Options::default()).is_err());
    }

    #[test]
    fn module_size_is_a_whole_number() {
        let opts = Options {
            size: Some(100.0),
            ..Options::default()
        };
        let (media, _) = barcode("qr", "a", &opts).unwrap();
        // Version 1 is 21 modules plus an 8 module quiet zone.
        assert_eq!(png(&media).px.0 % 29, 0);
    }
}
