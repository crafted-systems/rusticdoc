//! A small lossless XML tree. Elements keep their raw start tag and text
//! nodes keep their raw (escaped) content, so an untouched subtree writes
//! back exactly as it was read. Only the few things the engine needs are
//! modelled: names, attributes, children and the template pieces attached
//! to a node.

use std::borrow::Cow;

/// Index into the compiled tag table.
pub type TagId = usize;

/// A piece of a text node after tag extraction, or a tag hoisted onto an
/// element's boundary.
#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    /// Raw escaped XML text, copied through.
    Lit(String),
    /// A template tag.
    Tag(TagId),
    /// A spreadsheet cell whose whole content is one expression; rendered
    /// as a number, boolean or inline string depending on the value.
    TypedCell { expr: TagId, open: String },
}

#[derive(Debug, Clone)]
pub enum Node {
    Elem(Elem),
    Text(Text),
    /// Declarations, comments, processing instructions, doctype, CDATA.
    Raw(String),
}

#[derive(Debug, Clone)]
pub struct Elem {
    /// Qualified name, such as `w:p`.
    pub name: String,
    /// The raw start tag including brackets; ends in `/>` when self-closing.
    pub open: String,
    pub self_closing: bool,
    pub children: Vec<Node>,
    /// Tags emitted in front of the start tag.
    pub before: Vec<Piece>,
    /// Tags emitted after the end tag.
    pub after: Vec<Piece>,
}

#[derive(Debug, Clone)]
pub struct Text {
    /// Raw escaped content, used when `pieces` is `None`.
    pub raw: String,
    pub pieces: Option<Vec<Piece>>,
}

/// A path of child indexes from the root node list.
pub type Path = Vec<usize>;

impl Elem {
    pub fn new(name: &str, attrs: &[(&str, &str)], self_closing: bool) -> Elem {
        let mut open = format!("<{name}");
        for (k, v) in attrs {
            open.push_str(&format!(" {k}=\"{}\"", escape_attr(v)));
        }
        open.push_str(if self_closing { "/>" } else { ">" });
        Elem {
            name: name.to_string(),
            open,
            self_closing,
            children: Vec::new(),
            before: Vec::new(),
            after: Vec::new(),
        }
    }

    /// Iterates `(name, raw value)` over the start tag's attributes.
    pub fn attrs(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let body = &self.open[1 + self.name.len()..];
        let b = body.as_bytes();
        let mut i = 0;
        while i < b.len() {
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            let start = i;
            while i < b.len() && !matches!(b[i], b'=' | b'>' | b'/') && !b[i].is_ascii_whitespace()
            {
                i += 1;
            }
            if i == start {
                break;
            }
            let name = &body[start..i];
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i >= b.len() || b[i] != b'=' {
                break;
            }
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i >= b.len() {
                break;
            }
            let quote = b[i];
            if quote != b'"' && quote != b'\'' {
                break;
            }
            i += 1;
            let vstart = i;
            while i < b.len() && b[i] != quote {
                i += 1;
            }
            out.push((name.to_string(), body[vstart..i].to_string()));
            i += 1;
        }
        out
    }

    /// An attribute's unescaped value.
    pub fn attr(&self, name: &str) -> Option<String> {
        self.attrs()
            .into_iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| unescape(&v).into_owned())
    }

    pub fn set_attr(&mut self, name: &str, value: &str) {
        let attrs = self.attrs();
        let mut open = format!("<{}", self.name);
        let mut seen = false;
        for (k, v) in &attrs {
            if k == name {
                seen = true;
                open.push_str(&format!(" {name}=\"{}\"", escape_attr(value)));
            } else {
                open.push_str(&format!(" {k}=\"{v}\""));
            }
        }
        if !seen {
            open.push_str(&format!(" {name}=\"{}\"", escape_attr(value)));
        }
        open.push_str(if self.self_closing { "/>" } else { ">" });
        self.open = open;
    }

    pub fn remove_attr(&mut self, name: &str) {
        let attrs = self.attrs();
        let mut open = format!("<{}", self.name);
        for (k, v) in &attrs {
            if k != name {
                open.push_str(&format!(" {k}=\"{v}\""));
            }
        }
        open.push_str(if self.self_closing { "/>" } else { ">" });
        self.open = open;
    }

    /// Makes a self-closing element open so children can be added.
    pub fn make_open(&mut self) {
        if self.self_closing {
            self.self_closing = false;
            self.open = format!("{}>", self.open.trim_end_matches("/>"));
        }
    }

    pub fn child(&self, name: &str) -> Option<&Elem> {
        self.children.iter().find_map(|n| match n {
            Node::Elem(e) if e.name == name => Some(e),
            _ => None,
        })
    }

    pub fn child_mut(&mut self, name: &str) -> Option<&mut Elem> {
        self.children.iter_mut().find_map(|n| match n {
            Node::Elem(e) if e.name == name => Some(e),
            _ => None,
        })
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Elem> + 'a {
        self.children.iter().filter_map(move |n| match n {
            Node::Elem(e) if e.name == name => Some(e),
            _ => None,
        })
    }

    /// Finds the first descendant with this name, depth first.
    pub fn find(&self, name: &str) -> Option<&Elem> {
        for n in &self.children {
            if let Node::Elem(e) = n {
                if e.name == name {
                    return Some(e);
                }
                if let Some(f) = e.find(name) {
                    return Some(f);
                }
            }
        }
        None
    }

    pub fn find_mut(&mut self, name: &str) -> Option<&mut Elem> {
        for n in &mut self.children {
            if let Node::Elem(e) = n {
                if e.name == name {
                    return Some(e);
                }
                if let Some(f) = e.find_mut(name) {
                    return Some(f);
                }
            }
        }
        None
    }

    /// The unescaped text of every descendant text node.
    pub fn text_content(&self) -> String {
        let mut out = String::new();
        collect_text(&self.children, &mut out);
        out
    }
}

fn collect_text(nodes: &[Node], out: &mut String) {
    for n in nodes {
        match n {
            Node::Elem(e) => collect_text(&e.children, out),
            Node::Text(t) => out.push_str(&unescape(&t.raw)),
            Node::Raw(_) => {}
        }
    }
}

pub fn node_at<'a>(nodes: &'a [Node], path: &[usize]) -> Option<&'a Node> {
    let (first, rest) = path.split_first()?;
    let node = nodes.get(*first)?;
    if rest.is_empty() {
        return Some(node);
    }
    match node {
        Node::Elem(e) => node_at(&e.children, rest),
        _ => None,
    }
}

pub fn node_at_mut<'a>(nodes: &'a mut [Node], path: &[usize]) -> Option<&'a mut Node> {
    let (first, rest) = path.split_first()?;
    let node = nodes.get_mut(*first)?;
    if rest.is_empty() {
        return Some(node);
    }
    match node {
        Node::Elem(e) => node_at_mut(&mut e.children, rest),
        _ => None,
    }
}

pub fn elem_at<'a>(nodes: &'a [Node], path: &[usize]) -> Option<&'a Elem> {
    match node_at(nodes, path)? {
        Node::Elem(e) => Some(e),
        _ => None,
    }
}

pub fn elem_at_mut<'a>(nodes: &'a mut [Node], path: &[usize]) -> Option<&'a mut Elem> {
    match node_at_mut(nodes, path)? {
        Node::Elem(e) => Some(e),
        _ => None,
    }
}

/// Finds the root element.
pub fn root(nodes: &[Node]) -> Option<&Elem> {
    nodes.iter().find_map(|n| match n {
        Node::Elem(e) => Some(e),
        _ => None,
    })
}

pub fn root_mut(nodes: &mut [Node]) -> Option<&mut Elem> {
    nodes.iter_mut().find_map(|n| match n {
        Node::Elem(e) => Some(e),
        _ => None,
    })
}

/// How deep elements may nest. Office documents nest a few dozen deep at
/// most, tables inside tables included.
pub const MAX_DEPTH: usize = 256;

/// Parses a document into its top-level nodes.
pub fn parse(src: &str) -> Result<Vec<Node>, String> {
    let mut stack: Vec<Elem> = Vec::new();
    let mut top: Vec<Node> = Vec::new();
    let b = src.as_bytes();
    let mut i = 0;

    fn push(stack: &mut [Elem], top: &mut Vec<Node>, node: Node) {
        match stack.last_mut() {
            Some(e) => e.children.push(node),
            None => top.push(node),
        }
    }

    while i < b.len() {
        if b[i] != b'<' {
            let start = i;
            while i < b.len() && b[i] != b'<' {
                i += 1;
            }
            push(
                &mut stack,
                &mut top,
                Node::Text(Text {
                    raw: src[start..i].to_string(),
                    pieces: None,
                }),
            );
            continue;
        }
        let rest = &src[i..];
        let raw_until = |end: &str| -> Result<usize, String> {
            rest.find(end)
                .map(|p| p + end.len())
                .ok_or_else(|| format!("unterminated markup at byte {i}"))
        };
        if rest.starts_with("<?") {
            let n = raw_until("?>")?;
            push(&mut stack, &mut top, Node::Raw(rest[..n].to_string()));
            i += n;
        } else if rest.starts_with("<!--") {
            let n = raw_until("-->")?;
            push(&mut stack, &mut top, Node::Raw(rest[..n].to_string()));
            i += n;
        } else if rest.starts_with("<![CDATA[") {
            let n = raw_until("]]>")?;
            push(&mut stack, &mut top, Node::Raw(rest[..n].to_string()));
            i += n;
        } else if rest.starts_with("<!") {
            let n = raw_until(">")?;
            push(&mut stack, &mut top, Node::Raw(rest[..n].to_string()));
            i += n;
        } else if rest.starts_with("</") {
            let n = raw_until(">")?;
            let name = rest[2..n - 1].trim();
            let elem = stack
                .pop()
                .ok_or_else(|| format!("closing tag </{name}> without an open element"))?;
            if elem.name != name {
                return Err(format!("expected </{}>, found </{name}>", elem.name));
            }
            push(&mut stack, &mut top, Node::Elem(elem));
            i += n;
        } else {
            // Start tag: scan to the closing '>' outside quotes.
            let mut j = 1;
            let rb = rest.as_bytes();
            let mut quote = 0u8;
            while j < rb.len() {
                let c = rb[j];
                if quote != 0 {
                    if c == quote {
                        quote = 0;
                    }
                } else if c == b'"' || c == b'\'' {
                    quote = c;
                } else if c == b'>' {
                    break;
                }
                j += 1;
            }
            if j >= rb.len() {
                return Err(format!("unterminated start tag at byte {i}"));
            }
            let open = &rest[..=j];
            let self_closing = open.ends_with("/>");
            let name_end = open[1..]
                .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
                .map(|p| p + 1)
                .unwrap_or(open.len() - 1);
            let name = &open[1..name_end];
            if name.is_empty() {
                return Err(format!("empty element name at byte {i}"));
            }
            let elem = Elem {
                name: name.to_string(),
                open: open.to_string(),
                self_closing,
                children: Vec::new(),
                before: Vec::new(),
                after: Vec::new(),
            };
            if self_closing {
                push(&mut stack, &mut top, Node::Elem(elem));
            } else {
                // Everything that reads the tree walks it by recursion, so its
                // depth is the stack they need.
                if stack.len() >= MAX_DEPTH {
                    return Err(format!(
                        "elements are nested more than {MAX_DEPTH} deep at byte {i}"
                    ));
                }
                stack.push(elem);
            }
            i += j + 1;
        }
    }
    if let Some(e) = stack.pop() {
        return Err(format!("unclosed element <{}>", e.name));
    }
    Ok(top)
}

/// Writes nodes back as XML. Template pieces are written as their literal
/// text only; use the program serializer for a template.
pub fn to_xml(nodes: &[Node]) -> String {
    let mut out = String::new();
    write_xml(nodes, &mut out);
    out
}

pub fn write_xml(nodes: &[Node], out: &mut String) {
    for n in nodes {
        match n {
            Node::Elem(e) => {
                out.push_str(&e.open);
                if !e.self_closing {
                    write_xml(&e.children, out);
                    out.push_str("</");
                    out.push_str(&e.name);
                    out.push('>');
                }
            }
            Node::Text(t) => match &t.pieces {
                None => out.push_str(&t.raw),
                Some(pieces) => {
                    for p in pieces {
                        if let Piece::Lit(s) = p {
                            out.push_str(s);
                        }
                    }
                }
            },
            Node::Raw(r) => out.push_str(r),
        }
    }
}

/// Decodes XML character and entity references.
pub fn unescape(raw: &str) -> Cow<'_, str> {
    if !raw.contains('&') {
        return Cow::Borrowed(raw);
    }
    match quick_xml::escape::unescape(raw) {
        Ok(s) => s,
        Err(_) => Cow::Borrowed(raw),
    }
}

/// Escapes text content: `&`, `<` and `>`.
pub fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// Escapes an attribute value for a double-quoted attribute.
pub fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_raw_markup() {
        let src = "<?xml version=\"1.0\"?>\n<!-- c --><a x=\"1&gt;\" y='2'><b/><c>t &amp; u</c><![CDATA[<x>]]></a>";
        let nodes = parse(src).unwrap();
        assert_eq!(to_xml(&nodes), src);
        let a = root(&nodes).unwrap();
        assert_eq!(a.attr("x").as_deref(), Some("1>"));
        assert_eq!(a.attr("y").as_deref(), Some("2"));
        assert_eq!(a.child("c").unwrap().text_content(), "t & u");
    }

    #[test]
    fn edits_attributes() {
        let mut e = Elem::new("c", &[("r", "A1"), ("t", "s")], false);
        e.set_attr("t", "inlineStr");
        e.set_attr("s", "3");
        assert_eq!(e.open, "<c r=\"A1\" t=\"inlineStr\" s=\"3\">");
        e.remove_attr("t");
        assert_eq!(e.open, "<c r=\"A1\" s=\"3\">");
        let mut sc = Elem::new("x", &[], true);
        sc.make_open();
        assert_eq!(sc.open, "<x>");
    }

    #[test]
    fn reports_mismatched_tags() {
        assert!(parse("<a><b></a>").is_err());
        assert!(parse("<a>").is_err());
    }
}
