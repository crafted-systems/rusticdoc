//! A small, tolerant HTML reader. It keeps an allow-listed subset as a tree of
//! blocks and runs; no attribute survives except a checked `href` and a checked
//! colour, `script` and `style` go whole, and every other tag is dropped with
//! its text kept. Malformed input is closed as well as can be, never refused,
//! except for a link with a scheme that is not allowed.

/// The longest input accepted.
pub const MAX_INPUT: usize = 1024 * 1024;
/// The deepest nesting accepted.
pub const MAX_DEPTH: usize = 64;

/// Formatting a run carries. `color` is `RRGGBB`; `href` is checked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub sub: bool,
    pub sup: bool,
    pub color: Option<String>,
    pub href: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    /// Unescaped text.
    Text(String, Style),
    Break,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub header: bool,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Para(Vec<Inline>),
    Heading(u8, Vec<Inline>),
    List {
        ordered: bool,
        items: Vec<Vec<Block>>,
    },
    Table(Vec<Vec<Cell>>),
}

/// Checks a link: only `http`, `https` and `mailto`, and nothing a URI cannot
/// hold, so what reaches an attribute is a real link and nothing else.
pub fn check_url(value: &str) -> Result<String, String> {
    let url = value.trim();
    if url.is_empty() {
        return Err("the URL is empty".into());
    }
    let bad = |c: char| {
        c.is_control()
            || c.is_whitespace()
            || matches!(c, '<' | '>' | '"' | '\\' | '^' | '`' | '{' | '|' | '}')
    };
    if url.chars().any(bad) {
        return Err("the URL holds characters a URL cannot hold".into());
    }
    let Some((scheme, rest)) = url.split_once(':') else {
        return Err("the URL needs a scheme: only http, https and mailto are allowed".into());
    };
    let scheme = scheme.to_ascii_lowercase();
    let ok = match scheme.as_str() {
        "http" | "https" => rest
            .strip_prefix("//")
            .and_then(|host| host.chars().next())
            .is_some_and(|c| !matches!(c, '/' | '?' | '#')),
        "mailto" => !rest.is_empty(),
        _ => {
            return Err(format!(
                "the scheme {scheme:?} is not allowed: only http, https and mailto are"
            ))
        }
    };
    if !ok {
        return Err(format!("{url:?} is not a complete {scheme} URL"));
    }
    Ok(url.to_string())
}

/// `#rgb` or `#rrggbb` as upper-case `RRGGBB`; anything else is refused.
pub fn check_hex(value: &str) -> Option<String> {
    let hex = value.strip_prefix('#')?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let long: String = match hex.len() {
        3 => hex.chars().flat_map(|c| [c, c]).collect(),
        6 => hex.to_string(),
        _ => return None,
    };
    Some(long.to_ascii_uppercase())
}

enum Kind {
    /// Style only, or a dropped tag.
    Inline,
    /// Starts and ends a paragraph, as `p` and `div` do.
    Para,
    Heading(u8),
    List(bool, Vec<Vec<Block>>),
    Item(Vec<Block>),
    Table(Vec<Vec<Cell>>),
    Row(Vec<Cell>),
    Cell(bool, Vec<Block>),
}

struct Open {
    tag: String,
    style: Style,
    kind: Kind,
}

#[derive(Default)]
struct Parser {
    stack: Vec<Open>,
    root: Vec<Block>,
    runs: Vec<Inline>,
}

/// Tags that hold no content, so nothing closes them.
const VOID: &[&str] = &[
    "area", "base", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];
/// Dropped tags whose text still stands apart from what surrounds it.
const BLOCK_LIKE: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "center",
    "dd",
    "details",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "header",
    "main",
    "nav",
    "pre",
    "section",
    "summary",
];

impl Parser {
    fn style(&self) -> Style {
        self.stack
            .last()
            .map(|o| o.style.clone())
            .unwrap_or_default()
    }

    fn open(&mut self, tag: &str, style: Style, kind: Kind) -> Result<(), String> {
        if self.stack.len() >= MAX_DEPTH {
            return Err(format!("the HTML nests deeper than {MAX_DEPTH} levels"));
        }
        self.stack.push(Open {
            tag: tag.to_string(),
            style,
            kind,
        });
        Ok(())
    }

    /// Where finished blocks go: the innermost list item or cell, or the root.
    fn blocks(&mut self) -> &mut Vec<Block> {
        for open in self.stack.iter_mut().rev() {
            if let Kind::Item(blocks) | Kind::Cell(_, blocks) = &mut open.kind {
                return blocks;
            }
        }
        &mut self.root
    }

    /// Closes the runs gathered so far into a paragraph or heading.
    fn flush(&mut self) {
        if self.runs.is_empty() {
            return;
        }
        let mut runs = std::mem::take(&mut self.runs);
        if let Some(Inline::Text(text, _)) = runs.last_mut() {
            text.truncate(text.trim_end_matches(' ').len());
        }
        runs.retain(|r| !matches!(r, Inline::Text(t, _) if t.is_empty()));
        if runs.is_empty() {
            return;
        }
        let mut heading = None;
        for open in self.stack.iter().rev() {
            match open.kind {
                Kind::Heading(level) => {
                    heading = Some(level);
                    break;
                }
                Kind::Item(_) | Kind::Cell(..) => break,
                _ => {}
            }
        }
        let block = match heading {
            Some(level) => Block::Heading(level, runs),
            None => Block::Para(runs),
        };
        self.blocks().push(block);
    }

    /// Loose content inside a list or table gets the item or cell it lacks.
    fn ensure_flow(&mut self) -> Result<(), String> {
        let structure = self.stack.iter().rev().find_map(|o| match o.kind {
            Kind::List(..) => Some(1),
            Kind::Table(_) => Some(2),
            Kind::Row(_) => Some(3),
            Kind::Item(_) | Kind::Cell(..) => Some(0),
            _ => None,
        });
        let style = self.style();
        match structure {
            Some(1) => self.open("li", style, Kind::Item(Vec::new())),
            Some(2) => {
                self.open("tr", style.clone(), Kind::Row(Vec::new()))?;
                self.open("td", style, Kind::Cell(false, Vec::new()))
            }
            Some(3) => self.open("td", style, Kind::Cell(false, Vec::new())),
            _ => Ok(()),
        }
    }

    fn text(&mut self, text: &str) -> Result<(), String> {
        let mut clean = String::with_capacity(text.len());
        let mut space = false;
        for c in text.chars() {
            if c.is_ascii_whitespace() {
                space = true;
            } else {
                if space {
                    clean.push(' ');
                    space = false;
                }
                clean.push(c);
            }
        }
        if space {
            clean.push(' ');
        }
        if clean.is_empty() || clean == " " && self.runs.is_empty() {
            return Ok(());
        }
        if self.runs.is_empty() || clean != " " {
            self.ensure_flow()?;
        }
        // A space at the start of a paragraph, or after a space, adds nothing.
        let after_space = match self.runs.last() {
            None | Some(Inline::Break) => true,
            Some(Inline::Text(t, _)) => t.ends_with(' '),
        };
        if after_space {
            clean = clean.trim_start_matches(' ').to_string();
        }
        if clean.is_empty() {
            return Ok(());
        }
        let style = self.style();
        match self.runs.last_mut() {
            Some(Inline::Text(t, s)) if *s == style => t.push_str(&clean),
            _ => self.runs.push(Inline::Text(clean, style)),
        }
        Ok(())
    }

    /// Pops the top element, handing what it built to its parent.
    fn pop(&mut self) {
        if !matches!(self.stack.last().map(|o| &o.kind), Some(Kind::Inline)) {
            self.flush();
        }
        let Some(open) = self.stack.pop() else {
            return;
        };
        match open.kind {
            Kind::List(ordered, items) => self.blocks().push(Block::List { ordered, items }),
            Kind::Item(blocks) => {
                let list = self.stack.iter_mut().rev().find_map(|o| match &mut o.kind {
                    Kind::List(_, items) => Some(items),
                    _ => None,
                });
                if let Some(items) = list {
                    items.push(blocks);
                }
            }
            Kind::Table(rows) => self.blocks().push(Block::Table(rows)),
            Kind::Row(cells) => {
                let table = self.stack.iter_mut().rev().find_map(|o| match &mut o.kind {
                    Kind::Table(rows) => Some(rows),
                    _ => None,
                });
                if let Some(rows) = table {
                    rows.push(cells);
                }
            }
            Kind::Cell(header, blocks) => {
                let row = self.stack.iter_mut().rev().find_map(|o| match &mut o.kind {
                    Kind::Row(cells) => Some(cells),
                    _ => None,
                });
                if let Some(cells) = row {
                    cells.push(Cell { header, blocks });
                }
            }
            Kind::Inline | Kind::Para | Kind::Heading(_) => {}
        }
    }

    /// Pops down to and including the innermost `tag`, if one is open.
    fn close(&mut self, tag: &str) {
        if let Some(at) = self.stack.iter().rposition(|o| o.tag == tag) {
            while self.stack.len() > at {
                self.pop();
            }
        }
    }

    /// Pops elements above the innermost one that satisfies `keep`. Returns
    /// whether there was such an element.
    fn pop_to(&mut self, keep: impl Fn(&Kind) -> bool) -> bool {
        let Some(at) = self.stack.iter().rposition(|o| keep(&o.kind)) else {
            return false;
        };
        while self.stack.len() > at + 1 {
            self.pop();
        }
        true
    }

    fn close_paragraph(&mut self) {
        self.flush();
        let closes = self
            .stack
            .last()
            .is_some_and(|o| o.tag == "p" || matches!(o.kind, Kind::Heading(_)));
        if closes {
            self.pop();
        }
    }

    fn start_tag(&mut self, tag: &str, attrs: &[(String, String)]) -> Result<(), String> {
        let mut style = self.style();
        match tag {
            "br" => {
                self.ensure_flow()?;
                self.runs.push(Inline::Break);
                return Ok(());
            }
            "hr" => {
                self.flush();
                return Ok(());
            }
            _ if VOID.contains(&tag) => return Ok(()),
            "b" | "strong" => style.bold = true,
            "i" | "em" => style.italic = true,
            "u" => style.underline = true,
            "s" => style.strike = true,
            "sub" => style.sub = true,
            "sup" => style.sup = true,
            "a" => {
                if let Some((_, href)) = attrs.iter().find(|(k, _)| k == "href") {
                    style.href = Some(check_url(href)?);
                }
            }
            "span" => {
                if let Some((_, css)) = attrs.iter().find(|(k, _)| k == "style") {
                    if let Some(color) = css_color(css) {
                        style.color = Some(color);
                    }
                }
            }
            "p" => {
                self.close_paragraph();
                return self.open(tag, style, Kind::Para);
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.close_paragraph();
                let level = tag.as_bytes()[1] - b'0';
                return self.open(tag, style, Kind::Heading(level));
            }
            "ul" | "ol" => {
                self.close_paragraph();
                self.ensure_flow()?;
                return self.open(tag, style, Kind::List(tag == "ol", Vec::new()));
            }
            "li" => {
                self.flush();
                let in_list = self.pop_to(|k| matches!(k, Kind::List(..) | Kind::Item(_)));
                if !in_list {
                    self.open("ul", style.clone(), Kind::List(false, Vec::new()))?;
                } else if matches!(self.stack.last().map(|o| &o.kind), Some(Kind::Item(_))) {
                    self.pop();
                }
                return self.open(tag, style, Kind::Item(Vec::new()));
            }
            "table" => {
                self.close_paragraph();
                self.ensure_flow()?;
                return self.open(tag, style, Kind::Table(Vec::new()));
            }
            "tr" => {
                self.flush();
                if !self.pop_to(|k| matches!(k, Kind::Table(_))) {
                    self.open("table", style.clone(), Kind::Table(Vec::new()))?;
                }
                return self.open(tag, style, Kind::Row(Vec::new()));
            }
            "td" | "th" => {
                self.flush();
                if !self.pop_to(|k| matches!(k, Kind::Row(_))) {
                    if !self.pop_to(|k| matches!(k, Kind::Table(_))) {
                        self.open("table", style.clone(), Kind::Table(Vec::new()))?;
                    }
                    self.open("tr", style.clone(), Kind::Row(Vec::new()))?;
                }
                return self.open(tag, style, Kind::Cell(tag == "th", Vec::new()));
            }
            _ if BLOCK_LIKE.contains(&tag) => {
                self.close_paragraph();
                return self.open(tag, style, Kind::Para);
            }
            _ => {}
        }
        self.open(tag, style, Kind::Inline)
    }
}

/// The colour in a `style` attribute, when it is a plain `color:#hex`.
fn css_color(css: &str) -> Option<String> {
    css.split(';').find_map(|decl| {
        let (key, value) = decl.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case("color")
            .then(|| check_hex(value.trim()))
            .flatten()
    })
}

fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let entity = rest[1..]
            .find(';')
            .filter(|n| *n <= 10)
            .map(|n| &rest[1..n + 1]);
        let decoded = entity.and_then(|name| {
            let named = match name {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                "nbsp" => '\u{a0}',
                "copy" => '\u{a9}',
                "reg" => '\u{ae}',
                "ndash" => '\u{2013}',
                "mdash" => '\u{2014}',
                "hellip" => '\u{2026}',
                "euro" => '\u{20ac}',
                _ => {
                    let number = name.strip_prefix('#')?;
                    let code = match number.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => number.parse().ok()?,
                    };
                    char::from_u32(code)?
                }
            };
            Some((named, name.len() + 2))
        });
        match decoded {
            Some((c, used)) => {
                out.push(c);
                rest = &rest[used..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Reads `name="value"` pairs up to the closing `>`; returns the pairs,
/// whether the tag closed itself, and the offset just past it.
type TagAttrs = (Vec<(String, String)>, bool, usize);

fn read_attrs(tag: &str) -> Option<TagAttrs> {
    let b = tag.as_bytes();
    let mut attrs = Vec::new();
    let mut i = 0;
    let mut self_closing = false;
    loop {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            self_closing = b[i] == b'/';
            i += 1;
        }
        if i >= b.len() {
            return None;
        }
        if b[i] == b'>' {
            return Some((attrs, self_closing, i + 1));
        }
        self_closing = false;
        let start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'=' | b'>' | b'/') {
            i += 1;
        }
        let name = tag[start..i].to_ascii_lowercase();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] != b'=' {
            attrs.push((name, String::new()));
            continue;
        }
        i += 1;
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            return None;
        }
        let value = if matches!(b[i], b'"' | b'\'') {
            let quote = b[i];
            i += 1;
            let start = i;
            while i < b.len() && b[i] != quote {
                i += 1;
            }
            if i >= b.len() {
                return None;
            }
            i += 1;
            &tag[start..i - 1]
        } else {
            let start = i;
            while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' {
                i += 1;
            }
            &tag[start..i]
        };
        attrs.push((name, decode_entities(value)));
    }
}

/// Parses HTML into blocks. Only a disallowed link or the input limits fail.
pub fn parse(input: &str) -> Result<Vec<Block>, String> {
    if input.len() > MAX_INPUT {
        return Err(format!("the HTML is longer than {MAX_INPUT} bytes"));
    }
    let lower = input.to_ascii_lowercase();
    let mut parser = Parser::default();
    let mut i = 0;
    while i < input.len() {
        let Some(at) = input[i..].find('<') else {
            parser.text(&decode_entities(&input[i..]))?;
            break;
        };
        if at > 0 {
            parser.text(&decode_entities(&input[i..i + at]))?;
        }
        i += at;
        let rest = &input[i..];
        let lower_rest = &lower[i..];
        if rest.starts_with("<!--") {
            i += rest.find("-->").map_or(rest.len(), |n| n + 3);
        } else if rest.starts_with("<!") || rest.starts_with("<?") {
            i += rest.find('>').map_or(rest.len(), |n| n + 1);
        } else if rest.starts_with("</") {
            let end = rest.find('>').map_or(rest.len(), |n| n + 1);
            let name = lower_rest[2..end.max(2)]
                .trim_end_matches('>')
                .split_whitespace()
                .next()
                .unwrap_or("");
            if name == "br" {
                parser.start_tag("br", &[])?;
            } else {
                parser.close(name);
            }
            i += end;
        } else if rest[1..].starts_with(|c: char| c.is_ascii_alphabetic()) {
            let name_len = rest[1..]
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != ':')
                .unwrap_or(rest.len() - 1);
            let name = &lower_rest[1..=name_len];
            let Some((attrs, self_closing, used)) = read_attrs(&rest[1 + name_len..]) else {
                // A tag that never ends takes the rest of the input with it.
                break;
            };
            i += 1 + name_len + used;
            if name == "script" || name == "style" {
                let closer = format!("</{name}");
                let Some(end) = lower[i..].find(&closer) else {
                    break;
                };
                i += end;
                i += input[i..].find('>').map_or(input.len() - i, |n| n + 1);
                continue;
            }
            parser.start_tag(name, &attrs)?;
            if self_closing && !VOID.contains(&name) && name != "br" {
                parser.close(name);
            }
        } else {
            parser.text("<")?;
            i += 1;
        }
    }
    while !parser.stack.is_empty() {
        parser.pop();
    }
    parser.flush();
    Ok(parser.root)
}

/// The text of the blocks, a line per paragraph, list item, table row or
/// `<br>`, for formats that keep no structure.
pub fn plain_lines(blocks: &[Block]) -> Vec<String> {
    let mut lines = Vec::new();
    collect_lines(blocks, &mut lines);
    lines
}

fn runs_to_lines(runs: &[Inline], lines: &mut Vec<String>) {
    let mut line = String::new();
    for run in runs {
        match run {
            Inline::Text(t, _) => line.push_str(t),
            Inline::Break => lines.push(std::mem::take(&mut line)),
        }
    }
    lines.push(line);
}

fn collect_lines(blocks: &[Block], lines: &mut Vec<String>) {
    for block in blocks {
        match block {
            Block::Para(runs) | Block::Heading(_, runs) => runs_to_lines(runs, lines),
            Block::List { ordered, items } => {
                for (n, item) in items.iter().enumerate() {
                    let at = lines.len();
                    collect_lines(item, lines);
                    let marker = list_marker(*ordered, n);
                    match lines.get_mut(at) {
                        Some(first) => *first = format!("{marker} {first}"),
                        None => lines.push(marker),
                    }
                }
            }
            Block::Table(rows) => {
                for row in rows {
                    let cells: Vec<String> = row
                        .iter()
                        .map(|c| {
                            let mut inner = Vec::new();
                            collect_lines(&c.blocks, &mut inner);
                            inner.join(" ")
                        })
                        .collect();
                    lines.push(cells.join("\t"));
                }
            }
        }
    }
}

/// The literal text that stands for a list item's bullet or number.
pub fn list_marker(ordered: bool, index: usize) -> String {
    if ordered {
        format!("{}.", index + 1)
    } else {
        "\u{2022}".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(text: &str) -> Block {
        Block::Para(vec![Inline::Text(text.into(), Style::default())])
    }

    #[test]
    fn keeps_structure_and_drops_the_rest() {
        let blocks = parse(
            "<div onclick=\"x()\">Hi <b>there</b></div><script>alert(1)</script>\
             <STYLE>p{}</STYLE><custom>kept</custom><img src=\"x\"><!-- gone -->",
        )
        .unwrap();
        assert_eq!(
            blocks,
            [
                Block::Para(vec![
                    Inline::Text("Hi ".into(), Style::default()),
                    Inline::Text(
                        "there".into(),
                        Style {
                            bold: true,
                            ..Style::default()
                        }
                    ),
                ]),
                para("kept"),
            ]
        );
        assert_eq!(plain_lines(&parse("<script>a").unwrap()).len(), 0);
    }

    #[test]
    fn tolerates_broken_markup() {
        assert_eq!(parse("<p>one<p>two").unwrap(), [para("one"), para("two")]);
        assert_eq!(parse("a < b & c").unwrap(), [para("a < b & c")]);
        assert_eq!(parse("</p>x</div>").unwrap(), [para("x")]);
        assert_eq!(
            parse("&lt;b&gt; &#65;&#x42; &nope;").unwrap(),
            [para("<b> AB &nope;")]
        );
        assert_eq!(parse("x<b").unwrap(), [para("x")]);
        let blocks = parse("<ul><li>a<li>b</ul>loose").unwrap();
        assert_eq!(
            blocks,
            [
                Block::List {
                    ordered: false,
                    items: vec![vec![para("a")], vec![para("b")]]
                },
                para("loose")
            ]
        );
        assert_eq!(
            plain_lines(&parse("<table><td>a<td>b<tr><th>c</table>").unwrap()),
            ["a\tb", "c"]
        );
        assert_eq!(
            plain_lines(&parse("<li>orphan").unwrap()),
            ["\u{2022} orphan"]
        );
        assert_eq!(
            plain_lines(&parse("<ol><li>x<ul><li>y</ul></ol>").unwrap()),
            ["1. x", "\u{2022} y"]
        );
    }

    #[test]
    fn checks_links_and_colours() {
        let run = parse("<a href=\"https://a.example/?x=1&amp;y=2\">x</a>").unwrap();
        let Block::Para(runs) = &run[0] else {
            panic!("{run:?}")
        };
        let Inline::Text(_, style) = &runs[0] else {
            panic!("{runs:?}")
        };
        assert_eq!(style.href.as_deref(), Some("https://a.example/?x=1&y=2"));
        assert!(parse("<a href=\"javascript:alert(1)\">x</a>").is_err());
        assert!(parse("<a href=\"java&#10;script:alert(1)\">x</a>").is_err());
        assert!(parse("<a>no link</a>").is_ok());
        for (css, want) in [
            ("color:#abc", Some("AABBCC")),
            ("COLOR : #A1b2C3 ;font:x", Some("A1B2C3")),
            ("color:red", None),
            ("color:#abc\"><x", None),
            ("background-color:#123456", None),
        ] {
            assert_eq!(css_color(css).as_deref(), want, "{css}");
        }
        for (url, ok) in [
            ("http://a.b", true),
            ("HTTPS://a.b/c?d#e", true),
            ("mailto:a@b.c", true),
            (" https://a.b ", true),
            ("file:///etc/passwd", false),
            ("javascript:alert(1)", false),
            ("JaVaScRiPt:alert(1)", false),
            ("data:text/html,x", false),
            ("https:", false),
            ("https:///x", false),
            ("mailto:", false),
            ("/relative", false),
            ("http://a b", false),
            ("http://a\"b", false),
            ("", false),
        ] {
            assert_eq!(check_url(url).is_ok(), ok, "{url}");
        }
        assert_eq!(check_hex("#F0a").as_deref(), Some("FF00AA"));
        assert_eq!(check_hex("#ff0000").as_deref(), Some("FF0000"));
        for bad in ["ff0000", "#ff00", "#gggggg", "#ff0000\"", "\"#fff", ""] {
            assert_eq!(check_hex(bad), None, "{bad}");
        }
    }

    #[test]
    fn enforces_the_limits() {
        assert!(parse(&"<b>".repeat(MAX_DEPTH + 1)).is_err());
        assert!(parse(&"<b>".repeat(MAX_DEPTH)).is_ok());
        assert!(parse(&"<div>".repeat(MAX_DEPTH + 1)).is_err());
        assert!(parse(&"x".repeat(MAX_INPUT + 1)).is_err());
    }
}
