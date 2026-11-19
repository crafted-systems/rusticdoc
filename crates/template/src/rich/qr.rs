//! QR code styling: which modules belong to which function pattern, module
//! and finder ("eye") shapes as SVG path data, and the centred hole a logo
//! or the QR-bill's Swiss cross sits in.
//!
//! Only the finder patterns get shapes of their own. Timing and alignment
//! patterns take the module style like data does, as other generators draw
//! them and as decoders read them: the grid is sampled from the finders and
//! alignment centres, which keep their place whatever their outline.

use std::fmt::Write as _;

/// The QR version of a symbol `n` modules wide.
pub fn version(n: usize) -> Option<usize> {
    (n >= 21 && (n - 17).is_multiple_of(4) && n <= 177).then(|| (n - 17) / 4)
}

/// The rows and columns alignment patterns are centred on (ISO/IEC 18004
/// Annex E), computed the way the table is built.
pub fn alignment_centres(version: usize) -> Vec<usize> {
    if version < 2 {
        return Vec::new();
    }
    let count = version / 7 + 2;
    let size = version * 4 + 17;
    let step = if version == 32 {
        26
    } else {
        (version * 4 + count * 2 + 1) / (count * 2 - 2) * 2
    };
    let mut out = vec![6; count];
    let mut pos = size - 7;
    for slot in out.iter_mut().skip(1).rev() {
        *slot = pos;
        pos = pos.saturating_sub(step);
    }
    out
}

/// What a module of a QR symbol is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Finder,
    Separator,
    Timing,
    Alignment,
    /// Format information, with the dark module beside it.
    Format,
    Version,
    Data,
}

/// The function patterns of a symbol `n` modules wide, row by row.
pub struct Layout {
    n: usize,
    parts: Vec<Part>,
}

impl Layout {
    pub fn new(n: usize) -> Layout {
        let v = version(n).unwrap_or(1);
        let mut parts = vec![Part::Data; n * n];
        let mut set = |r: usize, c: usize, p: Part| {
            if r < n && c < n {
                parts[r * n + c] = p;
            }
        };
        for r in 0..n {
            set(r, 6, Part::Timing);
            set(6, r, Part::Timing);
        }
        let centres = alignment_centres(v);
        for &r in &centres {
            for &c in &centres {
                let corner = (r == 6 && c == 6) || (r == 6 && c == n - 7) || (r == n - 7 && c == 6);
                if corner {
                    continue;
                }
                for dr in 0..5 {
                    for dc in 0..5 {
                        set(r + dr - 2, c + dc - 2, Part::Alignment);
                    }
                }
            }
        }
        if v >= 7 {
            for a in 0..6 {
                for b in n - 11..n - 8 {
                    set(a, b, Part::Version);
                    set(b, a, Part::Version);
                }
            }
        }
        for i in 0..9 {
            set(8, i, Part::Format);
            set(i, 8, Part::Format);
        }
        for i in n - 8..n {
            set(8, i, Part::Format);
            set(i, 8, Part::Format);
        }
        for (r0, c0) in [(0, 0), (0, n - 8), (n - 8, 0)] {
            for dr in 0..8 {
                for dc in 0..8 {
                    set(r0 + dr, c0 + dc, Part::Separator);
                }
            }
        }
        for (r0, c0) in finders(n) {
            for dr in 0..7 {
                for dc in 0..7 {
                    set(r0 + dr, c0 + dc, Part::Finder);
                }
            }
        }
        Layout { n, parts }
    }

    pub fn part(&self, row: usize, col: usize) -> Part {
        self.parts[row * self.n + col]
    }

    /// How many modules carry data and error correction.
    pub fn data_modules(&self) -> usize {
        self.parts.iter().filter(|p| **p == Part::Data).count()
    }
}

/// The top-left module of each finder pattern.
pub fn finders(n: usize) -> [(usize, usize); 3] {
    [(0, 0), (0, n - 7), (n - 7, 0)]
}

/// How data modules are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Shape {
    #[default]
    Square,
    Dots,
    /// Modules that join their neighbours, with the outer corners rounded.
    Rounded,
}

impl Shape {
    pub fn parse(name: &str) -> Option<Shape> {
        Some(match name {
            "square" => Shape::Square,
            "dots" => Shape::Dots,
            "rounded" => Shape::Rounded,
            _ => return None,
        })
    }
}

/// How the frame or the centre of a finder pattern is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EyeShape {
    #[default]
    Square,
    Rounded,
    Circle,
}

impl EyeShape {
    pub fn parse(name: &str) -> Option<EyeShape> {
        Some(match name {
            "square" => EyeShape::Square,
            "rounded" => EyeShape::Rounded,
            "circle" => EyeShape::Circle,
            _ => return None,
        })
    }
}

/// The styling of a QR symbol.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub shape: Shape,
    /// The size of a data module's mark, as a share of the module.
    pub scale: f64,
    pub eye: EyeShape,
    pub eye_center: EyeShape,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            shape: Shape::Square,
            scale: 1.0,
            eye: EyeShape::Square,
            eye_center: EyeShape::Square,
        }
    }
}

/// A number for path data: at most two decimals, no trailing zeros.
pub fn num(v: f64) -> String {
    let s = format!("{:.2}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

/// A rectangle with its own radius at each corner: top left, top right,
/// bottom right, bottom left.
pub fn round_rect(d: &mut String, x: f64, y: f64, w: f64, h: f64, r: [f64; 4]) {
    let [tl, tr, br, bl] = r;
    let _ = write!(d, "M{} {}H{}", num(x + tl), num(y), num(x + w - tr));
    if tr > 0.0 {
        let _ = write!(
            d,
            "A{0} {0} 0 0 1 {1} {2}",
            num(tr),
            num(x + w),
            num(y + tr)
        );
    }
    let _ = write!(d, "V{}", num(y + h - br));
    if br > 0.0 {
        let _ = write!(
            d,
            "A{0} {0} 0 0 1 {1} {2}",
            num(br),
            num(x + w - br),
            num(y + h)
        );
    }
    let _ = write!(d, "H{}", num(x + bl));
    if bl > 0.0 {
        let _ = write!(
            d,
            "A{0} {0} 0 0 1 {1} {2}",
            num(bl),
            num(x),
            num(y + h - bl)
        );
    }
    let _ = write!(d, "V{}", num(y + tl));
    if tl > 0.0 {
        let _ = write!(d, "A{0} {0} 0 0 1 {1} {2}", num(tl), num(x + tl), num(y));
    }
    d.push('Z');
}

pub fn circle(d: &mut String, cx: f64, cy: f64, r: f64) {
    let _ = write!(
        d,
        "M{} {}A{2} {2} 0 1 0 {3} {1}A{2} {2} 0 1 0 {0} {1}Z",
        num(cx - r),
        num(cy),
        num(r),
        num(cx + r)
    );
}

/// The data modules as path data, in pixels: `m` per module, the symbol's
/// top left at `origin`. Finder patterns and modules for which `hidden`
/// holds are left out. Returns whether the edges are on the pixel grid.
pub fn modules(
    rows: &[Vec<bool>],
    hidden: &dyn Fn(usize, usize) -> bool,
    style: &Style,
    m: f64,
    origin: (f64, f64),
) -> (String, bool) {
    let n = rows.len();
    let layout = Layout::new(n);
    let drawn = |r: usize, c: usize| -> bool {
        r < n && c < n && rows[r][c] && layout.part(r, c) != Part::Finder && !hidden(r, c)
    };
    let mut d = String::new();
    let (ox, oy) = origin;
    let full = (style.scale - 1.0).abs() < 1e-9;
    if style.shape == Shape::Square && full {
        for r in 0..n {
            let mut c = 0;
            while c < n {
                if !drawn(r, c) {
                    c += 1;
                    continue;
                }
                let start = c;
                while c < n && drawn(r, c) {
                    c += 1;
                }
                let _ = write!(
                    d,
                    "M{} {}h{}v{}h-{}z",
                    num(ox + start as f64 * m),
                    num(oy + r as f64 * m),
                    num((c - start) as f64 * m),
                    num(m),
                    num((c - start) as f64 * m)
                );
            }
        }
        return (
            d,
            m.fract() == 0.0 && ox.fract() == 0.0 && oy.fract() == 0.0,
        );
    }
    let size = m * style.scale;
    let inset = (m - size) / 2.0;
    for r in 0..n {
        for c in 0..n {
            if !drawn(r, c) {
                continue;
            }
            let x = ox + c as f64 * m + inset;
            let y = oy + r as f64 * m + inset;
            match style.shape {
                Shape::Square => {
                    let _ = write!(
                        d,
                        "M{} {}h{}v{}h-{}z",
                        num(x),
                        num(y),
                        num(size),
                        num(size),
                        num(size)
                    );
                }
                Shape::Dots => circle(&mut d, x + size / 2.0, y + size / 2.0, size / 2.0),
                Shape::Rounded => {
                    let up = r > 0 && drawn(r - 1, c);
                    let down = drawn(r + 1, c);
                    let left = c > 0 && drawn(r, c - 1);
                    let right = drawn(r, c + 1);
                    let rad = size / 2.0;
                    let corner = |a: bool, b: bool| if a || b { 0.0 } else { rad };
                    round_rect(
                        &mut d,
                        x,
                        y,
                        size,
                        size,
                        [
                            corner(up, left),
                            corner(up, right),
                            corner(down, right),
                            corner(down, left),
                        ],
                    );
                }
            }
        }
    }
    (d, false)
}

/// The finder patterns' frames and centres as path data, drawn with the
/// even-odd rule. Returns whether each is on the pixel grid.
pub fn eyes(n: usize, style: &Style, m: f64, origin: (f64, f64)) -> (String, String, bool, bool) {
    let (mut frame, mut centre) = (String::new(), String::new());
    for (r, c) in finders(n) {
        let x = origin.0 + c as f64 * m;
        let y = origin.1 + r as f64 * m;
        match style.eye {
            EyeShape::Square => {
                round_rect(&mut frame, x, y, 7.0 * m, 7.0 * m, [0.0; 4]);
                round_rect(&mut frame, x + m, y + m, 5.0 * m, 5.0 * m, [0.0; 4]);
            }
            EyeShape::Rounded => {
                round_rect(&mut frame, x, y, 7.0 * m, 7.0 * m, [2.0 * m; 4]);
                round_rect(&mut frame, x + m, y + m, 5.0 * m, 5.0 * m, [1.2 * m; 4]);
            }
            EyeShape::Circle => {
                circle(&mut frame, x + 3.5 * m, y + 3.5 * m, 3.5 * m);
                circle(&mut frame, x + 3.5 * m, y + 3.5 * m, 2.5 * m);
            }
        }
        let (cx, cy, cs) = (x + 2.0 * m, y + 2.0 * m, 3.0 * m);
        match style.eye_center {
            EyeShape::Square => round_rect(&mut centre, cx, cy, cs, cs, [0.0; 4]),
            EyeShape::Rounded => round_rect(&mut centre, cx, cy, cs, cs, [0.9 * m; 4]),
            EyeShape::Circle => circle(&mut centre, x + 3.5 * m, y + 3.5 * m, 1.5 * m),
        }
    }
    let grid = m.fract() == 0.0 && origin.0.fract() == 0.0 && origin.1.fract() == 0.0;
    (
        frame,
        centre,
        grid && style.eye == EyeShape::Square,
        grid && style.eye_center == EyeShape::Square,
    )
}

/// A square of modules cleared in the middle of a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hole {
    pub start: usize,
    pub size: usize,
}

impl Hole {
    pub fn contains(&self, row: usize, col: usize) -> bool {
        (self.start..self.start + self.size).contains(&row)
            && (self.start..self.start + self.size).contains(&col)
    }

    /// The hole for a logo `share` of the symbol wide, with a module of
    /// margin on each side, centred on whole modules.
    pub fn for_logo(n: usize, share: f64) -> Hole {
        let logo = (share * n as f64).ceil() as usize;
        let mut size = logo + 2;
        if !(n - size.min(n)).is_multiple_of(2) {
            size += 1;
        }
        let size = size.min(n);
        Hole {
            start: (n - size) / 2,
            size,
        }
    }

    /// The modules a centred square `share` of the symbol wide touches.
    pub fn covering(n: usize, share: f64) -> Hole {
        let half = share * n as f64 / 2.0;
        let mid = n as f64 / 2.0;
        let start = (mid - half).floor().max(0.0) as usize;
        let end = ((mid + half).ceil() as usize).min(n);
        Hole {
            start,
            size: end - start,
        }
    }
}

/// The share of data modules a logo may hide at each error correction
/// level: half of what the level restores, since a square cuts across more
/// codewords than its area holds.
pub fn logo_budget(ec: &str) -> Option<f64> {
    match ec {
        "M" => Some(0.075),
        "Q" => Some(0.125),
        "H" => Some(0.15),
        _ => None,
    }
}

/// Whether a hole leaves the function patterns whole and hides no more
/// than `budget` of the data modules.
pub fn hole_fits(n: usize, hole: Hole, budget: f64) -> bool {
    if hole.start < 9 || hole.start + hole.size > n.saturating_sub(9) {
        return false;
    }
    let layout = Layout::new(n);
    let hidden = hole.size * hole.size;
    hidden as f64 <= budget * layout.data_modules() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alignment_centres_match_the_standard_table() {
        assert!(alignment_centres(1).is_empty());
        assert_eq!(alignment_centres(2), [6, 18]);
        assert_eq!(alignment_centres(7), [6, 22, 38]);
        assert_eq!(alignment_centres(14), [6, 26, 46, 66]);
        assert_eq!(alignment_centres(32), [6, 34, 60, 86, 112, 138]);
        assert_eq!(alignment_centres(36), [6, 24, 50, 76, 102, 128, 154]);
        assert_eq!(alignment_centres(40), [6, 30, 58, 86, 114, 142, 170]);
    }

    #[test]
    fn the_layout_counts_the_data_modules_of_each_version() {
        // ISO/IEC 18004 Table 1: data modules, data and error correction
        // together, are the codewords times eight plus the remainder bits.
        for (v, codewords, remainder) in [
            (1, 26, 0),
            (2, 44, 7),
            (7, 196, 0),
            (14, 581, 3),
            (40, 3706, 0),
        ] {
            let layout = Layout::new(v * 4 + 17);
            assert_eq!(
                layout.data_modules(),
                codewords * 8 + remainder,
                "version {v}"
            );
        }
        let layout = Layout::new(25);
        assert_eq!(layout.part(0, 0), Part::Finder);
        assert_eq!(layout.part(7, 7), Part::Separator);
        assert_eq!(layout.part(6, 12), Part::Timing);
        assert_eq!(layout.part(18, 18), Part::Alignment);
        assert_eq!(layout.part(8, 3), Part::Format);
        assert_eq!(layout.part(12, 12), Part::Data);
        let layout = Layout::new(45);
        assert_eq!(layout.part(0, 34), Part::Version);
    }

    #[test]
    fn holes_are_centred_and_checked() {
        let hole = Hole::for_logo(25, 0.2);
        assert_eq!(hole, Hole { start: 9, size: 7 });
        assert!(hole_fits(25, hole, 0.15));
        assert!(!hole_fits(21, Hole::for_logo(21, 0.2), 0.15));
        assert!(!hole_fits(25, hole, 0.05));
        assert!(hole.contains(9, 15) && !hole.contains(8, 12));
        let cross = Hole::covering(53, 7.0 / 46.0);
        assert_eq!(cross.start + cross.size / 2, 26);
    }

    #[test]
    fn numbers_are_short() {
        assert_eq!(num(3.0), "3");
        assert_eq!(num(2.505), "2.5");
        assert_eq!(num(-0.001), "0");
        assert_eq!(num(1.25), "1.25");
    }
}
