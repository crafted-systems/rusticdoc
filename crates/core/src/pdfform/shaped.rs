//! Text outside WinAnsi: shaped by Parley, written with subset fonts by
//! Krilla.
//!
//! Base-14 Helvetica covers Latin only. Every value that has a character
//! outside WinAnsi is queued here instead, and once the whole form is filled
//! the queue is laid out and written as one small PDF with a page the size
//! of each box. Parley, the text engine inside Fulgur, shapes each value with
//! HarfRust, resolves its bidirectional runs, breaks its lines and falls back
//! font by font through the system's fonts (fontconfig on Linux, which in a
//! service includes the mounted and stored fonts). So Arabic joins and runs
//! right to left, CJK finds a CJK face, and a value mixing scripts uses a
//! face for each. Krilla, the PDF writer inside Fulgur, embeds each face
//! once, subset to the glyphs used, with a ToUnicode map built from the
//! shaping clusters, so a ligature such as lam-alef copies as both letters.
//! Each page then comes back as a form XObject that the widget's appearance
//! stream, or the page, draws inside the rectangle.
//!
//! One document for the whole form means one font scan and one subset per
//! face however many fields there are. Nothing is read but fonts, and a
//! value is only ever text to shape. It is the caller's data, so what one
//! form may queue is capped in boxes and characters, and the imported
//! content is held to the same inflation budget as an overlay.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};

use super::overlay::inherited;
use super::text::num;
use super::{Rect, add_page_resource, isolate_page_content, malformed};
use crate::error::{Error, Result};
use crate::pdfops::Deadline;
use crate::pdfops::lopdf::{DEFAULT_MAX_BYTES, page_content_capped};

/// Characters of one value that are drawn; the field still stores all of
/// it. No rectangle shows more.
pub(super) const MAX_VALUE_CHARS: usize = 4_096;
/// Boxes one form may queue.
pub(super) const MAX_BOXES: usize = 2_000;
/// Characters one form may queue in all.
pub(super) const MAX_TOTAL_CHARS: usize = 100_000;
/// Largest side of a box, in points: the largest page PDF allows.
const MAX_SIDE: f32 = 14_400.0;
/// Inner padding, as the Helvetica appearances have.
const PAD: f32 = 2.0;
/// Line height, as a multiple of the font size.
const LINE_HEIGHT: f32 = 1.15;

/// The generic look of the field's font.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Family {
    Sans,
    Serif,
    Mono,
}

/// What the field asks of the font: its family, weight and slant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FontChoice {
    pub family: Family,
    pub bold: bool,
    pub italic: bool,
}

impl FontChoice {
    pub(super) const SANS: FontChoice = FontChoice {
        family: Family::Sans,
        bold: false,
        italic: false,
    };

    /// Reads the look from the `/DA` font resource name and, when the form
    /// has it, the font's `/BaseFont`: `Helv`, `TiRo`, `Cour`,
    /// `Arial-BoldMT` and the like.
    pub(super) fn from_names(resource: &str, base_font: Option<&str>) -> FontChoice {
        let names = format!("{resource} {}", base_font.unwrap_or("")).to_ascii_lowercase();
        let family = if names.contains("cour") || names.contains("mono") {
            Family::Mono
        } else if names.contains("tiro")
            || names.contains("times")
            || (names.contains("serif") && !names.contains("sans"))
        {
            Family::Serif
        } else {
            Family::Sans
        };
        FontChoice {
            family,
            bold: names.contains("bold") || names.contains("hebo") || names.contains("tibo"),
            italic: names.contains("italic") || names.contains("oblique"),
        }
    }
}

/// Where a line sits across the box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Align {
    Left,
    Centre,
    Right,
    /// Left for left-to-right text, right for right-to-left.
    Start,
}

impl Align {
    /// From the PDF `/Q` value.
    pub(super) fn from_quadding(q: i64) -> Align {
        match q {
            1 => Align::Centre,
            2 => Align::Right,
            _ => Align::Left,
        }
    }
}

/// One value to draw in a `width` by `height` box.
#[derive(Debug, Clone)]
pub(super) struct TextBox {
    pub text: String,
    pub width: f32,
    pub height: f32,
    /// Points; zero picks one that fits, as Acrobat's auto size does.
    pub size: f32,
    pub align: Align,
    pub multiline: bool,
    pub font: FontChoice,
    /// RGB, 0 to 1.
    pub colour: [f32; 3],
}

/// Where a laid-out box lands.
#[derive(Debug, Clone)]
enum Target {
    /// The normal appearance of a widget, after the background and border
    /// operators given.
    Widget { id: ObjectId, background: String },
    /// The page, with the box's bottom left at `x`, `y`.
    Page { id: ObjectId, x: f32, y: f32 },
}

/// The boxes queued while a form is filled, drawn at the end in one layout.
#[derive(Debug, Default)]
pub(super) struct Pending {
    items: Vec<(Target, TextBox)>,
    chars: usize,
}

/// True when the text has a character that base-14 Helvetica with
/// `WinAnsiEncoding` cannot show. Control characters do not count: they are
/// not drawn either way.
pub(super) fn needs_shaping(text: &str) -> bool {
    text.chars()
        .any(|c| !c.is_control() && c != '?' && super::text::winansi_byte(c) == b'?')
}

impl Pending {
    pub(super) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Queues `text` as the appearance of widget `id`, replacing what was
    /// queued for it before.
    pub(super) fn widget(&mut self, id: ObjectId, background: String, text: TextBox) -> Result<()> {
        self.forget(id);
        self.push(Target::Widget { id, background }, text)
    }

    /// Queues `text` to be drawn on page `id` inside `rect`.
    pub(super) fn page(&mut self, id: ObjectId, rect: Rect, text: TextBox) -> Result<()> {
        self.push(
            Target::Page {
                id,
                x: rect.x1,
                y: rect.y1,
            },
            text,
        )
    }

    /// Drops what was queued for widget `id`: a later value took the
    /// Helvetica path.
    pub(super) fn forget(&mut self, id: ObjectId) {
        let chars = &mut self.chars;
        self.items.retain(|(target, text)| match target {
            Target::Widget { id: queued, .. } if *queued == id => {
                *chars -= text.text.chars().count();
                false
            }
            _ => true,
        });
    }

    fn push(&mut self, target: Target, mut text: TextBox) -> Result<()> {
        if !(text.width >= 1.0 && text.height >= 1.0) {
            return Ok(());
        }
        text.width = text.width.min(MAX_SIDE);
        text.height = text.height.min(MAX_SIDE);
        text.text = clean(&text.text, text.multiline);
        if text.text.trim().is_empty() {
            return Ok(());
        }
        if self.items.len() >= MAX_BOXES {
            return Err(Error::InvalidRequest(format!(
                "the form has more than {MAX_BOXES} boxes of text outside Latin to draw"
            )));
        }
        self.chars += text.text.chars().count();
        if self.chars > MAX_TOTAL_CHARS {
            return Err(Error::InvalidRequest(format!(
                "the form has more than {MAX_TOTAL_CHARS} characters of text outside Latin to draw"
            )));
        }
        self.items.push((target, text));
        Ok(())
    }

    /// Lays out every queued box in one PDF and draws each into its widget
    /// or page.
    pub(super) fn draw(self, doc: &mut Document, deadline: Deadline) -> Result<()> {
        if self.is_empty() {
            return Ok(());
        }
        let pdf = lay_out(&self.items, deadline)?;
        deadline.check()?;
        let laid = Document::load_mem(&pdf).map_err(|e| Error::Engine {
            engine: "krilla",
            message: format!("cannot read the laid-out text: {e}"),
        })?;
        let forms = import(doc, laid)?;
        if forms.len() != self.items.len() {
            return Err(Error::Engine {
                engine: "krilla",
                message: format!(
                    "laying out {} boxes of text gave {} pages",
                    self.items.len(),
                    forms.len()
                ),
            });
        }
        let mut isolated = HashSet::new();
        for (n, ((target, text), form)) in self.items.iter().zip(forms).enumerate() {
            deadline.check()?;
            // The page is the box; this only moves a page whose media box
            // does not start at the origin.
            let draw = |name: &str| {
                format!(
                    "q 1 0 0 1 {} {} cm /{name} Do Q",
                    num(-form.x1),
                    num(text.height - form.y2)
                )
            };
            match target {
                Target::Widget { id, background } => {
                    let content = format!(
                        "{background}/Tx BMC q 1 1 {} {} re W n\n{}\nQ EMC\n",
                        num((text.width - 2.0).max(0.0)),
                        num((text.height - 2.0).max(0.0)),
                        draw("RdShaped")
                    );
                    let stream = Stream::new(
                        dictionary! {
                            "Type" => "XObject",
                            "Subtype" => "Form",
                            "BBox" => vec![0.into(), 0.into(), text.width.into(), text.height.into()],
                            "Resources" => dictionary! {
                                "XObject" => dictionary! { "RdShaped" => Object::Reference(form.id) },
                            },
                        },
                        content.into_bytes(),
                    );
                    let stream_id = doc.add_object(stream);
                    doc.get_dictionary_mut(*id)
                        .map_err(malformed)?
                        .set("AP", dictionary! { "N" => Object::Reference(stream_id) });
                }
                Target::Page { id, x, y } => {
                    if isolated.insert(*id) {
                        isolate_page_content(doc, *id)?;
                    }
                    let name = format!("RdShaped{}", n + 1);
                    add_page_resource(doc, *id, b"XObject", &name, form.id)?;
                    let content = format!(
                        "\nq 1 0 0 1 {} {} cm 0 0 {} {} re W n\n{}\nQ\n",
                        num(*x),
                        num(*y),
                        num(text.width),
                        num(text.height),
                        draw(&name)
                    );
                    doc.add_page_contents(*id, content.into_bytes())
                        .map_err(malformed)?;
                }
            }
        }
        Ok(())
    }
}

/// Drops control characters but line breaks, which a single line turns
/// into spaces like tabs, and cuts the text at [`MAX_VALUE_CHARS`].
fn clean(text: &str, multiline: bool) -> String {
    text.replace("\r\n", "\n")
        .chars()
        .map(|c| match c {
            '\r' | '\n' if multiline => '\n',
            '\r' | '\n' | '\t' => ' ',
            c => c,
        })
        .filter(|&c| c == '\n' || !c.is_control())
        .take(MAX_VALUE_CHARS)
        .collect()
}

/// The language a value is most likely in, for the scripts where it picks
/// the font: Han characters are drawn differently in Japan, Korea and China.
fn language(text: &str) -> Option<&'static str> {
    let has = |ranges: &[(u32, u32)]| {
        text.chars().any(|c| {
            ranges
                .iter()
                .any(|&(lo, hi)| (lo..=hi).contains(&(c as u32)))
        })
    };
    if has(&[(0x3040, 0x30FF), (0x31F0, 0x31FF), (0xFF66, 0xFF9F)]) {
        Some("ja")
    } else if has(&[(0x1100, 0x11FF), (0x3130, 0x318F), (0xAC00, 0xD7AF)]) {
        Some("ko")
    } else if has(&[
        (0x3400, 0x4DBF),
        (0x4E00, 0x9FFF),
        (0xF900, 0xFAFF),
        (0x20000, 0x3134F),
    ]) {
        Some("zh-CN")
    } else {
        None
    }
}

/// The CSS font stack: faces metric-compatible with the base-14 font the
/// field names, then the CJK face for the language, then the generic family.
/// Any other script falls back through fontconfig. The macOS CJK names are
/// there for the CLI on a Mac, whose default Han fallback cannot be embedded.
fn font_stack(font: FontChoice, lang: Option<&str>) -> String {
    let base = match font.family {
        Family::Sans => "'Liberation Sans', Arimo, Helvetica, Arial",
        Family::Serif => "'Liberation Serif', Tinos, 'Times New Roman', Times",
        Family::Mono => "'Liberation Mono', Cousine, 'Courier New', Courier",
    };
    let cjk = match lang {
        Some("ja") => ", 'Noto Sans CJK JP', 'Hiragino Sans'",
        Some("ko") => ", 'Noto Sans CJK KR', 'Apple SD Gothic Neo'",
        Some(_) => ", 'Noto Sans CJK SC', 'Hiragino Sans GB'",
        None => "",
    };
    let generic = match font.family {
        Family::Sans => "sans-serif",
        Family::Serif => "serif",
        Family::Mono => "monospace",
    };
    format!("{base}{cjk}, {generic}")
}

/// Lays out every box with Parley and writes it with Krilla, one page per
/// box, the page the size of the box.
fn lay_out(items: &[(Target, TextBox)], deadline: Deadline) -> Result<Vec<u8>> {
    let mut shaper = Shaper::new();
    let mut pdf = krilla::Document::new();
    for (_, text) in items {
        deadline.check()?;
        let (layout, top) = shaper.layout(text);
        let settings =
            krilla::page::PageSettings::from_wh(text.width, text.height).ok_or_else(|| {
                Error::Engine {
                    engine: "krilla",
                    message: format!("a box of {} by {} points", text.width, text.height),
                }
            })?;
        let mut page = pdf.start_page_with(settings);
        let mut surface = page.surface();
        let [r, g, b] = text
            .colour
            .map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
        surface.set_fill(Some(krilla::paint::Fill {
            paint: krilla::color::rgb::Color::new(r, g, b).into(),
            opacity: krilla::num::NormalizedF32::ONE,
            rule: Default::default(),
        }));
        shaper.draw(&mut surface, &layout, &text.text, PAD, top);
        surface.finish();
        page.finish();
    }
    pdf.finish().map_err(|e| Error::Engine {
        engine: "krilla",
        message: format!("cannot write the laid-out text: {e:?}"),
    })
}

/// Parley's font and layout contexts, and the Krilla face for each font file
/// used, so a face is parsed and embedded once.
struct Shaper {
    fonts: parley::FontContext,
    layouts: parley::LayoutContext<()>,
    faces: HashMap<(u64, u32), Option<krilla::text::Font>>,
}

impl Shaper {
    fn new() -> Self {
        Shaper {
            fonts: parley::FontContext::new(),
            layouts: parley::LayoutContext::new(),
            faces: HashMap::new(),
        }
    }

    /// One paragraph of `text` at `size` points, shaped but not broken.
    fn shape(&mut self, text: &str, font: FontChoice, size: f32) -> parley::Layout<()> {
        let lang = language(text);
        let stack = font_stack(font, lang);
        let mut builder = self
            .layouts
            .ranged_builder(&mut self.fonts, text, 1.0, false);
        builder.push_default(parley::StyleProperty::FontStack(parley::FontStack::Source(
            stack.into(),
        )));
        builder.push_default(parley::StyleProperty::FontSize(size));
        builder.push_default(parley::StyleProperty::LineHeight(
            parley::LineHeight::FontSizeRelative(LINE_HEIGHT),
        ));
        builder.push_default(parley::StyleProperty::Locale(lang));
        if font.bold {
            builder.push_default(parley::StyleProperty::FontWeight(parley::FontWeight::BOLD));
        }
        if font.italic {
            builder.push_default(parley::StyleProperty::FontStyle(parley::FontStyle::Italic));
        }
        builder.build(text)
    }

    /// The box's text laid out to fit it, with the offset of its first line
    /// from the top. An auto size (zero) is the largest that fits, found the
    /// way the Helvetica appearances find it but measured with the faces
    /// the text is drawn with.
    fn layout(&mut self, text: &TextBox) -> (parley::Layout<()>, f32) {
        let inner_w = (text.width - 2.0 * PAD).max(1.0);
        let inner_h = (text.height - 2.0 * PAD).max(1.0);
        let size = if text.size > 0.0 {
            text.size
        } else if text.multiline {
            12.0f32.min(inner_h / LINE_HEIGHT).max(4.0)
        } else {
            const AT: f32 = 100.0;
            let mut probe = self.shape(&text.text, text.font, AT);
            probe.break_all_lines(None);
            let unit = probe.full_width() / AT;
            let by_height = inner_h / LINE_HEIGHT;
            let by_width = if unit > 0.0 {
                inner_w / unit
            } else {
                by_height
            };
            by_height.min(by_width).clamp(4.0, 48.0)
        };
        let mut layout = self.shape(&text.text, text.font, size);
        layout.break_all_lines(text.multiline.then_some(inner_w));
        let alignment = match text.align {
            Align::Left => parley::Alignment::Left,
            Align::Centre => parley::Alignment::Center,
            Align::Right => parley::Alignment::Right,
            Align::Start => parley::Alignment::Start,
        };
        layout.align(
            Some(inner_w),
            alignment,
            parley::AlignmentOptions {
                align_when_overflowing: false,
            },
        );
        let top = if text.multiline {
            PAD
        } else {
            (text.height - layout.height()) / 2.0
        };
        (layout, top)
    }

    /// The Krilla face for a Parley font, parsed once per file and index.
    fn face(&mut self, font: &parley::FontData) -> Option<krilla::text::Font> {
        self.faces
            .entry((font.data.id(), font.index))
            .or_insert_with(|| {
                let data: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(font.data.clone());
                krilla::text::Font::new(data.into(), font.index)
            })
            .clone()
    }

    /// Draws every glyph run of `layout` with its top left at `left`, `top`.
    fn draw(
        &mut self,
        surface: &mut krilla::surface::Surface<'_>,
        layout: &parley::Layout<()>,
        text: &str,
        left: f32,
        top: f32,
    ) {
        for line in layout.lines() {
            for item in line.items() {
                let parley::PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let run = glyph_run.run();
                let size = run.font_size();
                let Some(face) = self.face(run.font()) else {
                    continue;
                };
                let ranges = cluster_ranges(run);
                let glyphs: Vec<krilla::text::KrillaGlyph> = glyph_run
                    .glyphs()
                    .zip(visual_glyph_ranges(run, &ranges))
                    .map(|(g, range)| krilla::text::KrillaGlyph {
                        glyph_id: krilla::text::GlyphId::new(g.id),
                        text_range: range,
                        x_advance: g.advance / size,
                        x_offset: g.x / size,
                        y_offset: g.y / size,
                        y_advance: 0.0,
                        location: None,
                    })
                    .collect();
                if glyphs.is_empty() {
                    continue;
                }
                surface.draw_glyphs(
                    krilla::geom::Point::from_xy(
                        left + glyph_run.offset(),
                        top + glyph_run.baseline(),
                    ),
                    &glyphs,
                    face,
                    text,
                    size,
                    false,
                );
            }
        }
    }
}

/// The text each cluster of a run stands for, by where the cluster starts. A
/// ligature's start cluster carries the glyph and its continuation clusters
/// none, so the start takes their text too: the lam-alef glyph stands for
/// lam and alef, and copies as both. The continuations follow the start in
/// a left-to-right run and come before it in a right-to-left one.
fn cluster_ranges(run: &parley::Run<'_, ()>) -> HashMap<usize, Range<usize>> {
    let mut clusters: Vec<_> = run.clusters().collect();
    if run.is_rtl() {
        clusters.reverse();
    }
    let mut out = HashMap::with_capacity(clusters.len());
    let mut i = 0;
    while i < clusters.len() {
        let mut range = clusters[i].text_range();
        let mut j = i + 1;
        if clusters[i].is_ligature_start() {
            while j < clusters.len() && clusters[j].is_ligature_continuation() {
                let next = clusters[j].text_range();
                range = range.start.min(next.start)..range.end.max(next.end);
                j += 1;
            }
        }
        for cluster in &clusters[i..j] {
            out.insert(cluster.text_range().start, range.clone());
        }
        i = j;
    }
    out
}

/// The text range of each glyph of a run, in the visual order Parley gives
/// the glyphs in.
fn visual_glyph_ranges(
    run: &parley::Run<'_, ()>,
    ranges: &HashMap<usize, Range<usize>>,
) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    for cluster in run.visual_clusters() {
        let own = cluster.text_range();
        let range = ranges.get(&own.start).cloned().unwrap_or(own);
        out.extend(cluster.glyphs().map(|_| range.clone()));
    }
    out
}

/// A laid-out page, now a form XObject in the target document, with the
/// left and top edges of its page.
struct Form {
    id: ObjectId,
    x1: f32,
    y2: f32,
}

/// Moves every page of `laid` into `doc` as a form XObject, with only the
/// objects its resources reach (fonts, graphics states), so the outline,
/// catalog and page tree of the laid-out document stay behind.
fn import(doc: &mut Document, mut laid: Document) -> Result<Vec<Form>> {
    let start = doc
        .objects
        .keys()
        .map(|id| id.0)
        .max()
        .unwrap_or(0)
        .max(doc.max_id);
    laid.renumber_objects_with(start + 1);
    let mut budget = DEFAULT_MAX_BYTES;
    let mut pages = Vec::new();
    for id in laid.page_iter().collect::<Vec<_>>() {
        let page = laid.get_dictionary(id).map_err(malformed)?;
        let content = page_content_capped(&laid, id, &mut budget)?;
        let resources =
            inherited(&laid, page, b"Resources").unwrap_or_else(|| Dictionary::new().into());
        let media = inherited(&laid, page, b"MediaBox")
            .and_then(|m| Rect::from_object(&laid, &m))
            .unwrap_or(Rect {
                x1: 0.0,
                y1: 0.0,
                x2: 612.0,
                y2: 792.0,
            });
        pages.push((content, resources, media));
    }
    let reached = reachable(&laid, pages.iter().map(|(_, resources, _)| resources));
    for id in reached {
        if let Some(object) = laid.objects.remove(&id) {
            doc.objects.insert(id, object);
        }
    }
    doc.max_id = doc.max_id.max(laid.max_id);
    let mut forms = Vec::with_capacity(pages.len());
    for (content, resources, media) in pages {
        let mut stream = Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Form",
                "BBox" => vec![media.x1.into(), media.y1.into(), media.x2.into(), media.y2.into()],
                "Resources" => resources,
            },
            content,
        );
        // Content that will not compress stays as it is.
        let _ = stream.compress();
        forms.push(Form {
            id: doc.add_object(stream),
            x1: media.x1,
            y2: media.y2,
        });
    }
    Ok(forms)
}

/// Every object reachable by reference from `roots`.
fn reachable<'a>(doc: &Document, roots: impl Iterator<Item = &'a Object>) -> BTreeSet<ObjectId> {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<&Object> = roots.collect();
    while let Some(object) = stack.pop() {
        match object {
            Object::Reference(id) => {
                if seen.insert(*id)
                    && let Ok(next) = doc.get_object(*id)
                {
                    stack.push(next);
                }
            }
            Object::Array(items) => stack.extend(items),
            Object::Dictionary(dict) => stack.extend(dict.iter().map(|(_, v)| v)),
            Object::Stream(stream) => stack.extend(stream.dict.iter().map(|(_, v)| v)),
            _ => {}
        }
    }
    seen
}

/// The colour of a `/DA` string's operators (`0 g`, `0 0 1 rg`,
/// `0 0 0 1 k`) as RGB; black when there is none.
pub(super) fn da_colour(ops: &str) -> [f32; 3] {
    let tokens: Vec<&str> = ops.split_whitespace().collect();
    let args = |n: usize, at: usize| -> Option<Vec<f32>> {
        (at >= n)
            .then(|| tokens[at - n..at].iter().map(|t| t.parse().ok()).collect())
            .flatten()
    };
    let mut rgb = [0.0; 3];
    for (i, op) in tokens.iter().enumerate() {
        match *op {
            "g" => {
                if let Some(v) = args(1, i) {
                    rgb = [v[0]; 3];
                }
            }
            "rg" => {
                if let Some(v) = args(3, i) {
                    rgb = [v[0], v[1], v[2]];
                }
            }
            "k" => {
                if let Some(v) = args(4, i) {
                    let k = 1.0 - v[3];
                    rgb = [(1.0 - v[0]) * k, (1.0 - v[1]) * k, (1.0 - v[2]) * k];
                }
            }
            _ => {}
        }
    }
    rgb
}

#[cfg(test)]
mod tests;
