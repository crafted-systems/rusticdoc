//! Loops that repeat shapes on one slide, moving each copy by a step.
//!
//! `{% for card in cards | shift(y="4cm") %}` … `{% endfor %}` repeats the
//! shapes that hold the two tags, and puts the second copy 4 cm below the
//! first, the third 8 cm below, and so on. Without `shift`, a loop across
//! shapes repeats the whole slide, because copies of a shape keep its
//! position and would print over one another.
//!
//! The step is read when the template is compiled. The compiler hoists the
//! loop onto the shapes and writes a comment at the start and end of each
//! copy; after rendering, [`apply`] reads those comments, moves the shapes
//! between them and removes them. A comment cannot come from data, since data
//! is escaped, and a hand-written one is only ever read as numbers.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use minijinja::value::{Kwargs, Value};
use minijinja::Error;
use regex::Regex;

use crate::package::{self, Entry};
use crate::rich::pptx::{extent_of, inherited_shape, offset_of};
use crate::rich::{fail, misplaced};
use crate::xml::{self, Elem, Node};
use crate::{Format, TemplateError};

/// The longest step: 56 inches, the largest slide side PowerPoint allows.
const MAX_STEP_EMU: i64 = 51_206_400;
/// The furthest a moved shape may sit from the slide's origin. Offsets are
/// 64-bit in the schema, but PowerPoint and LibreOffice keep them in 32 bits.
const MAX_COORDINATE: i64 = i32::MAX as i64;

const BEGIN: &str = "<!--rusticdoc:shift ";
const END: &str = "<!--rusticdoc:shift-end ";

static SHIFT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\|\s*shift\b\s*(\(([^)]*)\))?").unwrap());
static ARG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*([A-Za-z_]\w*)\s*=\s*(?:"([^"]*)"|'([^']*)'|(.*?))\s*$"#).unwrap()
});
static LENGTH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*([+-]?(?:\d+\.?\d*|\.\d+))\s*([A-Za-z]*)\s*$").unwrap());

/// How far each copy moves from the one before, in EMU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Step {
    pub dx: i64,
    pub dy: i64,
}

/// A length such as `4cm`, `-0.5in` or `12700emu`, in EMU.
fn length(name: &str, text: &str) -> Result<i64, String> {
    let bad = || {
        format!(
            "{name}={text:?} is not a length; write a number and a unit, such as \"4cm\" \
             (units: cm, mm, in, pt, emu)"
        )
    };
    let caps = LENGTH_RE.captures(text).ok_or_else(bad)?;
    let n: f64 = caps[1].parse().map_err(|_| bad())?;
    let per = match caps[2].to_ascii_lowercase().as_str() {
        "cm" => 360_000.0,
        "mm" => 36_000.0,
        "in" => 914_400.0,
        "pt" => 12_700.0,
        "emu" => 1.0,
        "" => {
            return Err(format!(
                "{name}={text:?} has no unit; write it like \"4cm\" (units: cm, mm, in, pt, emu)"
            ))
        }
        other => {
            return Err(format!(
                "{name}={text:?}: {other:?} is not a unit this knows; use cm, mm, in, pt or emu"
            ))
        }
    };
    let emu = (n * per).round();
    if !emu.is_finite() || emu.abs() > MAX_STEP_EMU as f64 {
        return Err(format!(
            "{name}={text:?} is longer than the largest slide (56 in, 142.24 cm)"
        ));
    }
    Ok(emu as i64)
}

/// The step from `x=` and `y=` texts.
fn step(x: Option<&str>, y: Option<&str>) -> Result<Step, String> {
    if x.is_none() && y.is_none() {
        return Err(
            "shift needs a step: x=\"…\" to move copies across, y=\"…\" to move \
                    them down, such as shift(y=\"4cm\")"
                .into(),
        );
    }
    Ok(Step {
        dx: x.map(|x| length("x", x)).transpose()?.unwrap_or(0),
        dy: y.map(|y| length("y", y)).transpose()?.unwrap_or(0),
    })
}

/// The step a `for` tag's `| shift(…)` asks for, or `None` when it has none.
/// `rest` is what follows `for` in the tag.
pub fn of_for(rest: &str) -> Option<Result<Step, String>> {
    let caps = SHIFT_RE.captures(rest)?;
    let Some(args) = caps.get(2) else {
        return Some(Err(
            "shift needs a step in brackets, such as shift(y=\"4cm\")".into(),
        ));
    };
    Some(parse_args(args.as_str()))
}

fn parse_args(args: &str) -> Result<Step, String> {
    let (mut x, mut y) = (None, None);
    for arg in args.split(',').filter(|a| !a.trim().is_empty()) {
        let caps = ARG_RE.captures(arg).ok_or_else(|| {
            format!(
                "shift takes x= and y= by name, such as shift(y=\"4cm\"), not {:?}",
                arg.trim()
            )
        })?;
        let name = &caps[1];
        let Some(value) = caps.get(2).or(caps.get(3)) else {
            return Err(format!(
                "shift: write {name}= as a quoted length such as {name}=\"4cm\"; the step is \
                 read when the template is compiled, so it cannot come from data"
            ));
        };
        let slot = match name {
            "x" => &mut x,
            "y" => &mut y,
            other => return Err(format!("shift takes x= and y=, not {other}=")),
        };
        if slot.replace(value.as_str().to_string()).is_some() {
            return Err(format!("shift: {name}= is given twice"));
        }
    }
    step(x.as_deref(), y.as_deref())
}

/// The comments written before and after each copy of a loop's shapes.
pub fn markers(loop_id: usize, step: Step) -> (String, String) {
    (
        format!("{BEGIN}{loop_id} {} {}-->", step.dx, step.dy),
        format!("{END}{loop_id}-->"),
    )
}

/// The `shift` filter. The compiler has already read the step, so at render
/// time the filter checks its arguments the same way and passes the list on.
pub fn filter(format: Format, value: Value, kwargs: Kwargs) -> Result<Value, Error> {
    let text = |name: &str| -> Result<Option<String>, Error> {
        let v: Option<Value> = kwargs.get(name).map_err(|e| fail("shift", e))?;
        match v {
            Some(v) if !v.is_none() => v
                .as_str()
                .map(|s| Some(s.to_string()))
                .ok_or_else(|| fail("shift", format!("{name}= must be a length such as \"4cm\""))),
            _ => Ok(None),
        }
    };
    let (x, y) = (text("x")?, text("y")?);
    kwargs.assert_all_used().map_err(|e| fail("shift", e))?;
    if format != Format::Pptx {
        return Err(fail(
            "shift",
            format!(
                "shift moves the shapes a loop repeats on a PowerPoint slide; it does not \
                 work in {} templates",
                format.extension().to_ascii_uppercase()
            ),
        ));
    }
    step(x.as_deref(), y.as_deref()).map_err(|e| fail("shift", e))?;
    Ok(value)
}

enum Marker {
    Begin { id: usize, step: Step },
    End { id: usize },
}

fn read_marker(raw: &str) -> Option<Result<Marker, String>> {
    let bad = || Err(format!("a shift marker is malformed: {raw}"));
    if let Some(rest) = raw.strip_prefix(END) {
        let Some(id) = rest.strip_suffix("-->").and_then(|s| s.parse().ok()) else {
            return Some(bad());
        };
        return Some(Ok(Marker::End { id }));
    }
    let rest = raw.strip_prefix(BEGIN)?.strip_suffix("-->");
    let numbers: Option<Vec<i64>> =
        rest.map(|r| r.split(' ').map(|n| n.parse().ok()).collect::<Option<_>>())?;
    match numbers.as_deref() {
        Some(&[id, dx, dy]) if id >= 0 && dx.abs() <= MAX_STEP_EMU && dy.abs() <= MAX_STEP_EMU => {
            Some(Ok(Marker::Begin {
                id: id as usize,
                step: Step { dx, dy },
            }))
        }
        _ => Some(bad()),
    }
}

/// One loop copy being walked: how far its shapes move, and how many copies
/// of each loop inside it have been seen, so a loop nested in a moved loop
/// starts counting again in every copy of the outer one.
struct Frame {
    id: Option<usize>,
    offset: (i64, i64),
    seen: HashMap<usize, i64>,
}

struct Slide<'a> {
    entries: &'a [Entry],
    part: &'a str,
}

impl Slide<'_> {
    fn fail(&self, message: impl Into<String>) -> TemplateError {
        misplaced(self.part, "slide", message)
    }

    /// Moves the shapes between each copy's markers and drops the markers.
    fn walk(&self, tree: &mut Elem) -> Result<(), TemplateError> {
        let mut frames = vec![Frame {
            id: None,
            offset: (0, 0),
            seen: HashMap::new(),
        }];
        let mut out = Vec::with_capacity(tree.children.len());
        for mut node in std::mem::take(&mut tree.children) {
            if let Node::Raw(raw) = &node {
                match read_marker(raw) {
                    Some(Ok(Marker::Begin { id, step })) => {
                        let top = frames.last_mut().expect("the base frame stays");
                        let n = top.seen.entry(id).or_insert(0);
                        let copy = *n;
                        *n += 1;
                        let offset = (top.offset.0, top.offset.1);
                        let moved = copy
                            .checked_mul(step.dx)
                            .and_then(|d| d.checked_add(offset.0))
                            .zip(copy.checked_mul(step.dy).and_then(|d| d.checked_add(offset.1)))
                            .filter(|(x, y)| x.abs() <= MAX_COORDINATE && y.abs() <= MAX_COORDINATE)
                            .ok_or_else(|| {
                                self.fail(format!(
                                    "copy {} of a shifted loop would sit further from the slide \
                                     than PowerPoint can place a shape; shorten the step or the list",
                                    copy + 1
                                ))
                            })?;
                        frames.push(Frame {
                            id: Some(id),
                            offset: moved,
                            seen: HashMap::new(),
                        });
                        continue;
                    }
                    Some(Ok(Marker::End { id })) => {
                        if frames.len() < 2 || frames.last().and_then(|f| f.id) != Some(id) {
                            return Err(self.fail("the shift markers on this slide do not pair up"));
                        }
                        frames.pop();
                        continue;
                    }
                    Some(Err(e)) => return Err(self.fail(e)),
                    None => {}
                }
            }
            let offset = frames.last().map_or((0, 0), |f| f.offset);
            if let Node::Elem(shape) = &mut node {
                if offset != (0, 0) {
                    self.move_shape(shape, offset)?;
                }
            }
            out.push(node);
        }
        if frames.len() != 1 {
            return Err(self.fail("the shift markers on this slide do not pair up"));
        }
        tree.children = out;
        Ok(())
    }

    fn move_shape(&self, shape: &mut Elem, (dx, dy): (i64, i64)) -> Result<(), TemplateError> {
        let props = match shape.name.as_str() {
            "p:sp" | "p:cxnSp" | "p:pic" => "p:spPr",
            "p:grpSp" => "p:grpSpPr",
            "p:graphicFrame" | "p:contentPart" => "p:xfrm",
            "mc:AlternateContent" => {
                // Each branch holds its own copy of the shape.
                for branch in shape.children.iter_mut() {
                    let Node::Elem(branch) = branch else { continue };
                    for inner in branch.children.iter_mut() {
                        if let Node::Elem(inner) = inner {
                            self.move_shape(inner, (dx, dy))?;
                        }
                    }
                }
                return Ok(());
            }
            _ => return Ok(()),
        };
        let at = |v: i64, d: i64| {
            v.checked_add(d)
                .filter(|c| c.abs() <= MAX_COORDINATE)
                .ok_or_else(|| {
                    self.fail("a shifted copy would sit further from the slide than PowerPoint can place a shape")
                })
        };
        // The offset is the first `a:off` of the properties: in a group's
        // properties the child offset `a:chOff` must stay as it is.
        let own = offset_of_props(shape, props);
        let (x, y) = match own {
            Some(found) => found,
            None => {
                // A placeholder PowerPoint never moved has no position of its
                // own; it gets one, taken from its layout, before it moves.
                let inherited = inherited_shape(self.entries, self.part, shape)
                    .and_then(|s| offset_of(&s).zip(extent_of(&s)));
                let Some(((x, y), (cx, cy))) = inherited else {
                    return Err(self.fail(
                        "a shape in a shifted loop has no position to move from; move it \
                         once in PowerPoint so that it has one",
                    ));
                };
                let Some(spr) = shape.child_mut(props) else {
                    return Err(self.fail("a shape in a shifted loop has no properties"));
                };
                let (xs, ys, cxs, cys) =
                    (x.to_string(), y.to_string(), cx.to_string(), cy.to_string());
                let mut xfrm = Elem::new("a:xfrm", &[], false);
                xfrm.children = vec![
                    Node::Elem(Elem::new("a:off", &[("x", &xs), ("y", &ys)], true)),
                    Node::Elem(Elem::new("a:ext", &[("cx", &cxs), ("cy", &cys)], true)),
                ];
                // `a:xfrm` comes first in a shape's properties.
                spr.make_open();
                spr.children.insert(0, Node::Elem(xfrm));
                (x, y)
            }
        };
        let (nx, ny) = (at(x, dx)?, at(y, dy)?);
        if let Some(off) = shape
            .child_mut(props)
            .and_then(|p| {
                if props == "p:xfrm" {
                    Some(p)
                } else {
                    p.child_mut("a:xfrm")
                }
            })
            .and_then(|x| x.child_mut("a:off"))
        {
            off.set_attr("x", &nx.to_string());
            off.set_attr("y", &ny.to_string());
        }
        Ok(())
    }
}

/// The offset in a shape's own properties, never one from inside a group.
fn offset_of_props(shape: &Elem, props: &str) -> Option<(i64, i64)> {
    let props = shape.child(props)?;
    let xfrm = if props.name == "p:xfrm" {
        props
    } else {
        props.child("a:xfrm")?
    };
    let off = xfrm.child("a:off")?;
    let at = |name: &str| off.attr(name).and_then(|v| v.parse::<i64>().ok());
    Some((at("x")?, at("y")?))
}

/// Gives every shape copied by a loop an id of its own: PowerPoint repairs a
/// slide whose shapes share one.
fn renumber(tree: &mut Elem) {
    fn collect(e: &Elem, max: &mut u64) {
        for child in &e.children {
            if let Node::Elem(c) = child {
                if c.name == "p:cNvPr" {
                    if let Some(id) = c.attr("id").and_then(|v| v.parse::<u64>().ok()) {
                        *max = (*max).max(id);
                    }
                }
                collect(c, max);
            }
        }
    }
    fn fix(e: &mut Elem, seen: &mut HashSet<u64>, next: &mut u64) {
        for child in e.children.iter_mut() {
            if let Node::Elem(c) = child {
                if c.name == "p:cNvPr" {
                    let id = c.attr("id").and_then(|v| v.parse::<u64>().ok());
                    if let Some(id) = id {
                        if !seen.insert(id) {
                            c.set_attr("id", &next.to_string());
                            seen.insert(*next);
                            *next += 1;
                        }
                    }
                }
                fix(c, seen, next);
            }
        }
    }
    let mut max = 0;
    collect(tree, &mut max);
    let mut next = max + 1;
    fix(tree, &mut HashSet::new(), &mut next);
}

/// Moves the copies of shifted loops on every slide that has them.
pub fn apply(entries: &mut Vec<Entry>) -> Result<(), TemplateError> {
    let names: Vec<String> = entries
        .iter()
        .filter(|e| {
            e.name.starts_with("ppt/slides/slide")
                && e.name.ends_with(".xml")
                && e.data.windows(BEGIN.len()).any(|w| w == BEGIN.as_bytes())
        })
        .map(|e| e.name.clone())
        .collect();
    for name in names {
        let Some(src) = package::text_of(entries, &name) else {
            continue;
        };
        let mut nodes = xml::parse(&src)
            .map_err(|e| misplaced(&name, "", format!("rendered XML is malformed: {e}")))?;
        let slide = Slide {
            entries,
            part: &name,
        };
        if let Some(tree) = xml::root_mut(&mut nodes).and_then(|r| r.find_mut("p:spTree")) {
            slide.walk(tree)?;
            renumber(tree);
        }
        let out = xml::to_xml(&nodes);
        if out.contains(BEGIN) || out.contains(END) {
            return Err(misplaced(
                &name,
                "slide",
                "a shifted loop must repeat whole shapes on the slide",
            ));
        }
        package::set(entries, &name, out.into_bytes());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_steps_in_every_unit() {
        let s = |rest: &str| of_for(rest).unwrap();
        assert_eq!(
            s("c in cards | shift(y=\"4cm\")").unwrap(),
            Step {
                dx: 0,
                dy: 1_440_000
            }
        );
        assert_eq!(
            s("c in cards | shift(x='1in', y=\"-10mm\") if c.ok").unwrap(),
            Step {
                dx: 914_400,
                dy: -360_000
            }
        );
        assert_eq!(s("c in cards|shift(x=\"72pt\")").unwrap().dx, 914_400);
        assert_eq!(s("c in cards | shift(y=\"12700 emu\")").unwrap().dy, 12_700);
        assert_eq!(s("c in cards | shift(y=\".5in\")").unwrap().dy, 457_200);
        assert!(of_for("c in cards").is_none());
        assert!(of_for("c in shifts").is_none());
    }

    #[test]
    fn refuses_bad_steps_plainly() {
        let e = |rest: &str| of_for(rest).unwrap().unwrap_err();
        assert!(e("c in cards | shift").contains("in brackets"));
        assert!(e("c in cards | shift()").contains("needs a step"));
        assert!(e("c in cards | shift(y=4)").contains("quoted length"));
        assert!(e("c in cards | shift(y=gap)").contains("cannot come from data"));
        assert!(e("c in cards | shift(y=\"4\")").contains("no unit"));
        assert!(e("c in cards | shift(y=\"4px\")").contains("not a unit"));
        assert!(e("c in cards | shift(y=\"four cm\")").contains("not a length"));
        assert!(e("c in cards | shift(z=\"1cm\")").contains("not z="));
        assert!(e("c in cards | shift(\"1cm\")").contains("by name"));
        assert!(e("c in cards | shift(y=\"1cm\", y=\"2cm\")").contains("twice"));
        assert!(e("c in cards | shift(y=\"200cm\")").contains("largest slide"));
        assert!(e("c in cards | shift(y=\"99999999999999999999999cm\")").contains("largest slide"));
        assert!(e("c in cards | shift(y=\"1e9cm\")").contains("not a length"));
    }

    #[test]
    fn markers_round_trip() {
        let (begin, end) = markers(7, Step { dx: -5, dy: 9 });
        assert!(matches!(
            read_marker(&begin),
            Some(Ok(Marker::Begin {
                id: 7,
                step: Step { dx: -5, dy: 9 }
            }))
        ));
        assert!(matches!(read_marker(&end), Some(Ok(Marker::End { id: 7 }))));
        assert!(read_marker("<!-- other -->").is_none());
        assert!(read_marker("<!--rusticdoc:shift 1 2-->").unwrap().is_err());
        assert!(read_marker("<!--rusticdoc:shift 1 2 999999999999-->")
            .unwrap()
            .is_err());
    }

    fn sp(id: u32, x: i64) -> String {
        format!(
            "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"s\"/></p:nvSpPr><p:spPr><a:xfrm>\
             <a:off x=\"{x}\" y=\"100\"/><a:ext cx=\"1\" cy=\"1\"/></a:xfrm></p:spPr></p:sp>"
        )
    }

    fn walked(body: &str) -> Result<String, TemplateError> {
        let mut nodes = xml::parse(&format!("<p:spTree>{body}</p:spTree>")).unwrap();
        let slide = Slide {
            entries: &[],
            part: "ppt/slides/slide1.xml",
        };
        let tree = xml::root_mut(&mut nodes).unwrap();
        slide.walk(tree)?;
        renumber(tree);
        Ok(xml::to_xml(&nodes))
    }

    #[test]
    fn moves_each_copy_one_step_further() {
        let (b, e) = markers(3, Step { dx: 10, dy: 1000 });
        let body = format!(
            "{}{b}{}{e}{b}{}{e}{b}{}{e}",
            sp(9, 0),
            sp(2, 5),
            sp(2, 5),
            sp(2, 5)
        );
        let out = walked(&body).unwrap();
        assert!(!out.contains("rusticdoc"), "{out}");
        assert_eq!(offsets(&out), ["0,100", "5,100", "15,1100", "25,2100"]);
        // The copies get ids of their own.
        assert!(
            out.contains("id=\"9\"") && out.contains("id=\"2\""),
            "{out}"
        );
        assert!(
            out.contains("id=\"10\"") && out.contains("id=\"11\""),
            "{out}"
        );
    }

    #[test]
    fn a_nested_loop_counts_again_in_every_outer_copy() {
        let (ob, oe) = markers(1, Step { dx: 0, dy: 100 });
        let (ib, ie) = markers(2, Step { dx: 10, dy: 0 });
        let cell = sp(5, 0);
        let row = format!("{ib}{cell}{ie}{ib}{cell}{ie}");
        let out = walked(&format!("{ob}{row}{oe}{ob}{row}{oe}")).unwrap();
        assert_eq!(offsets(&out), ["0,100", "10,100", "0,200", "10,200"]);
    }

    fn offsets(xml: &str) -> Vec<String> {
        Regex::new("<a:off x=\"(-?\\d+)\" y=\"(-?\\d+)\"")
            .unwrap()
            .captures_iter(xml)
            .map(|c| format!("{},{}", &c[1], &c[2]))
            .collect()
    }

    #[test]
    fn groups_frames_and_placeholders_move_too() {
        let (b, e) = markers(1, Step { dx: 0, dy: 50 });
        let group = "<p:grpSp><p:nvGrpSpPr><p:cNvPr id=\"4\" name=\"g\"/></p:nvGrpSpPr><p:grpSpPr>\
                     <a:xfrm><a:off x=\"1\" y=\"2\"/><a:ext cx=\"9\" cy=\"9\"/><a:chOff x=\"7\" y=\"7\"/>\
                     <a:chExt cx=\"9\" cy=\"9\"/></a:xfrm></p:grpSpPr>INNER</p:grpSp>"
            .replace("INNER", &sp(5, 3));
        let frame = "<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id=\"6\" name=\"t\"/>\
                     </p:nvGraphicFramePr><p:xfrm><a:off x=\"8\" y=\"9\"/><a:ext cx=\"1\" cy=\"1\"/>\
                     </p:xfrm></p:graphicFrame>";
        let out = walked(&format!("{b}{group}{frame}{e}{b}{group}{frame}{e}")).unwrap();
        // The group moves as a whole: its child offset and its shapes stay.
        assert_eq!(
            offsets(&out),
            ["1,2", "3,100", "8,9", "1,52", "3,100", "8,59"]
        );
        assert_eq!(
            out.matches("<a:chOff x=\"7\" y=\"7\"/>").count(),
            2,
            "{out}"
        );
        // A placeholder without a position of its own, and no layout to ask.
        let ph = "<p:sp><p:nvSpPr><p:cNvPr id=\"2\" name=\"Title\"/><p:cNvSpPr/><p:nvPr>\
                  <p:ph type=\"title\"/></p:nvPr></p:nvSpPr><p:spPr/></p:sp>";
        let err = walked(&format!("{b}{ph}{e}{b}{ph}{e}")).unwrap_err();
        assert!(err.message.contains("no position"), "{err}");
    }

    #[test]
    fn unpaired_markers_and_far_copies_are_errors() {
        let (b, e) = markers(1, Step { dx: 0, dy: 1 });
        assert!(walked(&format!("{b}{}", sp(1, 0))).is_err());
        assert!(walked(&format!("{}{e}", sp(1, 0))).is_err());
        let (b, e) = markers(
            1,
            Step {
                dx: MAX_STEP_EMU,
                dy: 0,
            },
        );
        let many: String = (0..50).map(|_| format!("{b}{}{e}", sp(1, 0))).collect();
        let err = walked(&many).unwrap_err();
        assert!(err.message.contains("further from the slide"), "{err}");
    }
}
