//! The data a template reads, worked out from its tags: every path it names,
//! which of them it loops over, and the fields it reads from each item. An
//! application can build its data from this, or check data it already has.
//!
//! This reads expressions the way the template engine would, but only far
//! enough to find names: a path built at render time (`row[column]`) shows up as
//! far as it is written, and a name a template sets itself is not data.

use std::collections::HashMap;

use serde::Serialize;
use utoipa::ToSchema;

/// One part of a path: a key, or "each item of" an array.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Seg {
    Key(String),
    Item,
}

type Path = Vec<Seg>;

/// What a template does with a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
#[schema(as = VariableKind)]
pub enum Kind {
    /// Printed or tested: a string, number or boolean.
    Value,
    /// Has fields of its own.
    Object,
    /// Looped over; its items' fields are listed under `path[]`.
    Array,
}

/// One path the template reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Variable {
    /// Dotted, with `[]` for each item of an array: `items[].price`. `items[]`
    /// itself says what each item is.
    #[schema(example = "items[].price")]
    pub path: String,
    #[serde(rename = "type")]
    pub kind: Kind,
}

/// What a template reads, as a list and as a skeleton of data.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Variables {
    /// Every path, parents before their fields, in the order the template
    /// first names them.
    pub variables: Vec<Variable>,
    /// Data of the right shape: objects, one-item arrays, and empty strings
    /// where values go.
    #[schema(value_type = Object)]
    pub example: serde_json::Value,
}

/// Names the engine provides, which are never data.
const KEYWORDS: &[&str] = &[
    "and",
    "or",
    "not",
    "in",
    "is",
    "if",
    "else",
    "true",
    "false",
    "none",
    "True",
    "False",
    "None",
    "loop",
    "recursive",
    "self",
    "super",
    "caller",
    "varargs",
    "kwargs",
];

/// Filters whose `attribute` or first argument names a field of each item.
const ITEM_FILTERS: &[&str] = &["selectattr", "rejectattr", "groupby", "map", "sort"];

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Num,
    Punct(&'static str),
}

fn tokens(expr: &str) -> Vec<Tok> {
    const PUNCTS: &[&str] = &[
        "==", "!=", "<=", ">=", "//", "**", ".", "[", "]", "(", ")", "|", ",", "=", "<", ">", "+",
        "-", "*", "/", "%", "~", ":", "{", "}",
    ];
    let mut out = Vec::new();
    let chars: Vec<char> = expr.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '"' || c == '\'' {
            let mut text = String::new();
            i += 1;
            while i < chars.len() && chars[i] != c {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                }
                text.push(chars[i]);
                i += 1;
            }
            i += 1;
            out.push(Tok::Str(text));
        } else if c.is_ascii_digit() {
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            // A decimal point followed by digits is part of the number.
            if i + 1 < chars.len() && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
                i += 1;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
            out.push(Tok::Num);
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(Tok::Ident(chars[start..i].iter().collect()));
        } else {
            let rest: String = chars[i..chars.len().min(i + 2)].iter().collect();
            match PUNCTS.iter().find(|p| rest.starts_with(**p)) {
                Some(p) => {
                    out.push(Tok::Punct(p));
                    i += p.chars().count();
                }
                None => i += 1,
            }
        }
    }
    out
}

/// What a name means where it is used: data at a path, or something the
/// template made itself.
#[derive(Debug, Clone)]
enum Local {
    Alias(Path),
    Own,
}

#[derive(Debug, Default)]
struct Frame {
    /// The block that opened it: `for`, `with`, `macro` or `call`.
    kind: String,
    names: HashMap<String, Local>,
}

#[derive(Debug, Default)]
struct Analysis {
    frames: Vec<Frame>,
    /// Every path seen, with what was seen of it, in first-seen order.
    seen: Vec<(Path, Kind)>,
    /// Where each path is in `seen`, so finding one does not read them all.
    index: HashMap<Path, usize>,
    /// Set once the template names more than the analysis will hold.
    over: Option<TooMany>,
}

/// Most parts one path may have. Deeper data than this is not written by hand.
pub const MAX_DEPTH: usize = 64;
/// Most distinct paths one template may name.
pub const MAX_PATHS: usize = 10_000;

/// Why a template's variables are not listed: it names more than the analysis
/// holds, and the work grows faster than the template does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TooMany {
    Depth,
    Paths,
}

impl std::fmt::Display for TooMany {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Depth => write!(
                f,
                "the template reads a path more than {MAX_DEPTH} parts deep, which is more than its variables are listed for"
            ),
            Self::Paths => write!(
                f,
                "the template reads more than {MAX_PATHS} different paths, which is more than its variables are listed for"
            ),
        }
    }
}

impl Analysis {
    fn new() -> Self {
        Self {
            frames: vec![Frame::default()],
            seen: Vec::new(),
            index: HashMap::new(),
            over: None,
        }
    }

    fn lookup(&self, name: &str) -> Option<&Local> {
        self.frames.iter().rev().find_map(|f| f.names.get(name))
    }

    fn define(&mut self, name: &str, local: Local) {
        if let Some(frame) = self.frames.last_mut() {
            frame.names.insert(name.to_string(), local);
        }
    }

    fn push(&mut self, kind: &str) {
        self.frames.push(Frame {
            kind: kind.to_string(),
            names: HashMap::new(),
        });
    }

    fn pop(&mut self, kind: &str) {
        if let Some(at) = self.frames.iter().rposition(|f| f.kind == kind)
            && at > 0
        {
            self.frames.truncate(at);
        }
    }

    fn note(&mut self, path: &Path, kind: Kind) {
        if path.is_empty() || self.over.is_some() {
            return;
        }
        if path.len() > MAX_DEPTH {
            self.over = Some(TooMany::Depth);
            return;
        }
        // Every parent is an object, or an array when the next part is an item.
        for end in 1..path.len() {
            let parent_kind = if path[end] == Seg::Item {
                Kind::Array
            } else {
                Kind::Object
            };
            self.record(&path[..end], parent_kind);
        }
        self.record(path, kind);
    }

    fn record(&mut self, path: &[Seg], kind: Kind) {
        match self.index.get(path) {
            Some(&at) => {
                let known = &mut self.seen[at].1;
                // What a path holds wins over its merely being printed.
                if *known == Kind::Value || kind == Kind::Array {
                    *known = kind;
                }
            }
            None if self.seen.len() >= MAX_PATHS => self.over = Some(TooMany::Paths),
            None => {
                self.index.insert(path.to_vec(), self.seen.len());
                self.seen.push((path.to_vec(), kind));
            }
        }
    }

    /// Reads one expression and notes every data path in it. Returns the path
    /// when the whole expression is one, as a loop or `set` target needs.
    fn expression(&mut self, expr: &str) -> Option<Path> {
        let toks = tokens(expr);
        let mut i = 0;
        let mut whole: Option<Path> = None;
        let mut primaries = 0;
        // The path just read, for a filter that names its items' fields.
        let mut subject: Option<Path> = None;
        while i < toks.len() {
            match &toks[i] {
                Tok::Punct("|") => {
                    // A filter's name, then perhaps its arguments.
                    if let Some(Tok::Ident(filter)) = toks.get(i + 1) {
                        i += 2;
                        if toks.get(i) == Some(&Tok::Punct("(")) {
                            let fields = item_fields(filter, &toks[i..]);
                            if let Some(subject) = &subject {
                                for field in fields {
                                    let mut item = subject.clone();
                                    item.push(Seg::Item);
                                    item.push(Seg::Key(field));
                                    self.note(&item, Kind::Value);
                                }
                            }
                        }
                        continue;
                    }
                    i += 1;
                }
                Tok::Ident(word) if word == "is" => {
                    // A test's name follows, perhaps after `not`.
                    i += 1;
                    if toks.get(i) == Some(&Tok::Ident("not".into())) {
                        i += 1;
                    }
                    if matches!(toks.get(i), Some(Tok::Ident(_))) {
                        i += 1;
                    }
                }
                Tok::Ident(word) => {
                    let next = toks.get(i + 1);
                    let after_dot = i > 0 && toks[i - 1] == Tok::Punct(".");
                    let keyword_arg = next == Some(&Tok::Punct("="));
                    let call = next == Some(&Tok::Punct("("));
                    if after_dot || keyword_arg || call || KEYWORDS.contains(&word.as_str()) {
                        if !after_dot && !keyword_arg {
                            subject = None;
                        }
                        i += 1;
                        continue;
                    }
                    let (path, used) = self.chain(&toks[i..]);
                    primaries += 1;
                    i += used;
                    subject = path.clone();
                    if let Some(path) = path {
                        self.note(&path, Kind::Value);
                        if primaries == 1 && i == toks.len() {
                            whole = Some(path);
                        }
                    }
                }
                Tok::Punct(p) if *p != "(" && *p != ")" => {
                    subject = None;
                    i += 1;
                }
                _ => i += 1,
            }
        }
        if primaries == 1 { whole } else { None }
    }

    /// A name and what follows it: `.field`, `["field"]`, `[0]`, and a method
    /// call at the end. Returns the data path, if the name is data, and how
    /// many tokens it took.
    fn chain(&self, toks: &[Tok]) -> (Option<Path>, usize) {
        let Some(Tok::Ident(name)) = toks.first() else {
            return (None, 1);
        };
        let mut path = match self.lookup(name) {
            Some(Local::Alias(path)) => Some(path.clone()),
            Some(Local::Own) => None,
            None => Some(vec![Seg::Key(name.clone())]),
        };
        let mut i = 1;
        loop {
            match (toks.get(i), toks.get(i + 1), toks.get(i + 2)) {
                (Some(Tok::Punct(".")), Some(Tok::Ident(field)), next) => {
                    if next == Some(&Tok::Punct("(")) {
                        // A method: `prices.items()` reads `prices`, as an
                        // object when it is one of a mapping's.
                        if let Some(path) = &mut path
                            && matches!(field.as_str(), "items" | "keys" | "values")
                        {
                            path.push(Seg::Key(String::new()));
                        }
                        i += 2;
                        if toks.get(i) == Some(&Tok::Punct("("))
                            && toks.get(i + 1) == Some(&Tok::Punct(")"))
                        {
                            i += 2;
                        }
                        break;
                    }
                    if let Some(path) = &mut path {
                        path.push(Seg::Key(field.clone()));
                    }
                    i += 2;
                }
                (Some(Tok::Punct("[")), Some(Tok::Str(field)), Some(Tok::Punct("]"))) => {
                    if let Some(path) = &mut path {
                        path.push(Seg::Key(field.clone()));
                    }
                    i += 3;
                }
                (Some(Tok::Punct("[")), Some(Tok::Num), Some(Tok::Punct("]"))) => {
                    if let Some(path) = &mut path {
                        path.push(Seg::Item);
                    }
                    i += 3;
                }
                _ => break,
            }
        }
        // The marker for "a mapping's method" becomes the mapping itself.
        if let Some(p) = &mut path
            && p.last() == Some(&Seg::Key(String::new()))
        {
            p.pop();
        }
        (path, i)
    }

    /// `for target in source`: the source is an array, and the target an item
    /// of it.
    fn for_loop(&mut self, rest: &str) {
        let Some((targets, source)) = split_for(rest) else {
            self.expression(rest);
            return;
        };
        let (source, condition) = match find_word(source, "if") {
            Some(at) => (&source[..at], Some(&source[at + 2..])),
            None => (source, None),
        };
        let source = source.trim().trim_end_matches("recursive").trim();
        let mapping = source.ends_with(".items()") || source.ends_with(".values()");
        let path = self.expression(source);
        self.push("for");
        let targets: Vec<&str> = targets.split(',').map(str::trim).collect();
        match (&path, targets.as_slice()) {
            (Some(path), [one]) if !mapping => {
                self.note(path, Kind::Array);
                let mut item = path.clone();
                item.push(Seg::Item);
                self.define(one, Local::Alias(item));
            }
            (Some(path), _) => {
                self.note(path, if mapping { Kind::Object } else { Kind::Array });
                for target in targets {
                    self.define(target, Local::Own);
                }
            }
            (None, _) => {
                for target in targets {
                    self.define(target, Local::Own);
                }
            }
        }
        if let Some(condition) = condition {
            self.expression(condition);
        }
    }

    fn statement(&mut self, name: &str, rest: &str) {
        match name {
            "for" => self.for_loop(rest),
            "endfor" => self.pop("for"),
            "if" | "elif" => {
                self.expression(rest);
            }
            "set" => match rest.split_once('=') {
                Some((target, value)) if !target.contains('.') => {
                    let local = match self.expression(value) {
                        Some(path) => Local::Alias(path),
                        None => Local::Own,
                    };
                    for name in target.split(',').map(str::trim) {
                        self.define(name, local.clone());
                    }
                }
                // `ns.total = ...` changes a namespace the template made.
                Some((_, value)) => {
                    self.expression(value);
                }
                // A block `set`: what follows is its content.
                None => self.define(rest.trim(), Local::Own),
            },
            "with" => {
                self.push("with");
                for part in split_top(rest, ',') {
                    if let Some((target, value)) = part.split_once('=') {
                        let local = match self.expression(value) {
                            Some(path) => Local::Alias(path),
                            None => Local::Own,
                        };
                        self.define(target.trim(), local);
                    }
                }
            }
            "endwith" => self.pop("with"),
            "macro" | "call" => {
                let (head, args) = match rest.find('(') {
                    Some(at) => (&rest[..at], &rest[at..]),
                    None => (rest, ""),
                };
                if name == "macro" {
                    self.define(head.trim(), Local::Own);
                }
                self.push(name);
                let inner = args.trim().trim_start_matches('(');
                let inner = inner.split(')').next().unwrap_or("");
                for arg in split_top(inner, ',') {
                    let arg = arg.split('=').next().unwrap_or("").trim();
                    if !arg.is_empty() {
                        self.define(arg, Local::Own);
                    }
                }
            }
            "endmacro" => self.pop("macro"),
            "endcall" => self.pop("call"),
            // Blocks that read nothing, or settings.
            "else" | "endif" | "raw" | "endraw" | "filter" | "endfilter" | "block" | "endblock"
            | "autoescape" | "endautoescape" | "endset" | "settings" | "include" | "import"
            | "from" | "extends" | "break" | "continue" => {}
            _ => {
                self.expression(rest);
            }
        }
    }

    fn tag(&mut self, tag: &str) {
        if tag.len() < 4 || !tag.is_char_boundary(2) || !tag.is_char_boundary(tag.len() - 2) {
            return;
        }
        let inner = tag[2..tag.len() - 2].trim().trim_matches('-').trim();
        if tag.starts_with("{{") {
            self.expression(inner);
        } else if tag.starts_with("{%") {
            let (name, rest) = match inner.find(char::is_whitespace) {
                Some(at) => (&inner[..at], inner[at..].trim()),
                None => (inner, ""),
            };
            self.statement(name, rest);
        }
    }

    fn finish(self) -> Result<Variables, TooMany> {
        if let Some(over) = self.over {
            return Err(over);
        }
        let mut seen: Vec<(Path, Kind)> = self.seen;
        // A path whose fields are read is an object; one with items, an array.
        let snapshot = seen.clone();
        for (path, kind) in &mut seen {
            for (other, _) in &snapshot {
                if other.len() > path.len() && other.starts_with(path) {
                    let next = &other[path.len()];
                    if *next == Seg::Item {
                        *kind = Kind::Array;
                    } else if *kind == Kind::Value {
                        *kind = Kind::Object;
                    }
                }
            }
        }
        let example = example(&seen, &[]);
        Ok(Variables {
            variables: seen
                .iter()
                .map(|(path, kind)| Variable {
                    path: show(path),
                    kind: *kind,
                })
                .collect(),
            example,
        })
    }
}

/// The skeleton for the paths under `prefix`.
fn example(seen: &[(Path, Kind)], prefix: &[Seg]) -> serde_json::Value {
    let kind = seen
        .iter()
        .find(|(p, _)| p.as_slice() == prefix)
        .map(|(_, k)| *k)
        .unwrap_or(Kind::Object);
    match kind {
        Kind::Value => serde_json::Value::String(String::new()),
        Kind::Array => {
            let mut item = prefix.to_vec();
            item.push(Seg::Item);
            let has_fields = seen
                .iter()
                .any(|(p, _)| p.len() > item.len() && p.starts_with(&item));
            if has_fields {
                serde_json::Value::Array(vec![example(seen, &item)])
            } else {
                serde_json::json!([""])
            }
        }
        Kind::Object => {
            let mut map = serde_json::Map::new();
            for (path, _) in seen {
                if path.len() == prefix.len() + 1
                    && path.starts_with(prefix)
                    && let Some(Seg::Key(key)) = path.last()
                {
                    map.insert(key.clone(), example(seen, path));
                }
            }
            serde_json::Value::Object(map)
        }
    }
}

fn show(path: &[Seg]) -> String {
    let mut out = String::new();
    for seg in path {
        match seg {
            Seg::Key(key) => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(key);
            }
            Seg::Item => out.push_str("[]"),
        }
    }
    out
}

/// The item fields a filter call names: `sum(attribute="price")`,
/// `selectattr("paid")`.
fn item_fields(filter: &str, call: &[Tok]) -> Vec<String> {
    let mut fields = Vec::new();
    let mut depth = 0;
    let mut first = true;
    for (at, tok) in call.iter().enumerate() {
        match tok {
            Tok::Punct("(") => depth += 1,
            Tok::Punct(")") => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Tok::Punct(",") if depth == 1 => first = false,
            Tok::Str(text) if depth == 1 => {
                let keyword = at >= 2
                    && call[at - 1] == Tok::Punct("=")
                    && call[at - 2] == Tok::Ident("attribute".into());
                if keyword || (first && ITEM_FILTERS.contains(&filter)) {
                    // A dotted attribute names a path inside each item.
                    if let Some(field) = text.split('.').next().filter(|f| !f.is_empty()) {
                        fields.push(field.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    fields
}

/// `a, b in source` as the targets and the source.
fn split_for(rest: &str) -> Option<(&str, &str)> {
    let at = find_word(rest, "in")?;
    Some((rest[..at].trim(), rest[at + 2..].trim()))
}

/// Where `word` stands alone in `text`, outside quotes.
fn find_word(text: &str, word: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut quote = None;
    for (at, &b) in bytes.iter().enumerate() {
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None if b == b'"' || b == b'\'' => quote = Some(b),
            None => {
                let boundary_before = at == 0 || !is_word_byte(bytes[at - 1]);
                let end = at + word.len();
                if boundary_before
                    && text[at..].starts_with(word)
                    && (end == bytes.len() || !is_word_byte(bytes[end]))
                {
                    return Some(at);
                }
            }
        }
    }
    None
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Splits on `sep` where it is not inside brackets or quotes.
fn split_top(text: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut quote, mut start) = (0i32, None, 0);
    for (at, c) in text.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '"' | '\'' => quote = Some(c),
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                c if c == sep && depth == 0 => {
                    parts.push(&text[start..at]);
                    start = at + c.len_utf8();
                }
                _ => {}
            },
        }
    }
    parts.push(&text[start..]);
    parts
}

/// The variables of a template, from its tags in document order.
pub fn analyse(tags: &[String]) -> Result<Variables, TooMany> {
    let mut analysis = Analysis::new();
    for tag in tags {
        analysis.tag(tag);
        if analysis.over.is_some() {
            break;
        }
    }
    analysis.finish()
}

/// The variables of a PDF form: each field's name is a path into the data,
/// with numbers for array items (`items.0.qty`), unless its value holds tags,
/// which are read instead, as filling would.
pub fn analyse_form(fields: &[(String, Option<String>)]) -> Result<Variables, TooMany> {
    let mut analysis = Analysis::new();
    for (name, value) in fields {
        let value = value.as_deref().unwrap_or("");
        if value.contains("{{") || value.contains("{%") {
            for tag in tags_in(value) {
                analysis.tag(&tag);
            }
            continue;
        }
        let path: Path = name
            .split('.')
            .filter(|part| !part.is_empty())
            .map(|part| {
                if part.bytes().all(|b| b.is_ascii_digit()) {
                    Seg::Item
                } else {
                    Seg::Key(part.to_string())
                }
            })
            .collect();
        analysis.note(&path, Kind::Value);
    }
    analysis.finish()
}

/// The `{{ }}` and `{% %}` tags in a piece of text.
fn tags_in(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find('{') {
        let after = &rest[at..];
        let close = match after.as_bytes().get(1) {
            Some(b'{') => "}}",
            Some(b'%') => "%}",
            _ => {
                rest = &after[1..];
                continue;
            }
        };
        let Some(end) = after[2..].find(close) else {
            break;
        };
        found.push(after[..end + 4].to_string());
        rest = &after[end + 4..];
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(tags: &[&str]) -> Vec<(String, Kind)> {
        let tags: Vec<String> = tags.iter().map(|t| t.to_string()).collect();
        analyse(&tags)
            .unwrap()
            .variables
            .into_iter()
            .map(|v| (v.path, v.kind))
            .collect()
    }

    fn has(found: &[(String, Kind)], path: &str, kind: Kind) -> bool {
        found.iter().any(|(p, k)| p == path && *k == kind)
    }

    #[test]
    fn loops_give_arrays_and_their_items_fields() {
        let found = of(&[
            "{{ customer.name }}",
            "{% for item in items %}",
            "{{ item.description }}",
            "{{ item.price | currency }}",
            "{% if not loop.last %}",
            "{% endif %}",
            "{% endfor %}",
            "{{ total }}",
        ]);
        assert!(has(&found, "customer", Kind::Object), "{found:?}");
        assert!(has(&found, "customer.name", Kind::Value));
        assert!(has(&found, "items", Kind::Array));
        assert!(has(&found, "items[].description", Kind::Value));
        assert!(has(&found, "items[].price", Kind::Value));
        assert!(has(&found, "total", Kind::Value));
        assert!(
            !found
                .iter()
                .any(|(p, _)| p.starts_with("item.") || p == "item" || p.starts_with("loop"))
        );
        assert!(!found.iter().any(|(p, _)| p == "currency"));
    }

    #[test]
    fn nested_loops_and_the_example() {
        let tags: Vec<String> = [
            "{% for order in customer.orders %}",
            "{% for line in order.lines if line.qty > 0 %}",
            "{{ line.sku }}",
            "{% endfor %}",
            "{% endfor %}",
        ]
        .iter()
        .map(|t| t.to_string())
        .collect();
        let found = analyse(&tags).unwrap();
        let paths: Vec<&str> = found.variables.iter().map(|v| v.path.as_str()).collect();
        assert!(
            paths.contains(&"customer.orders[].lines[].sku"),
            "{paths:?}"
        );
        assert!(paths.contains(&"customer.orders[].lines[].qty"));
        assert_eq!(
            found.example,
            serde_json::json!({"customer": {"orders": [{"lines": [{"qty": "", "sku": ""}]}]}})
        );
    }

    #[test]
    fn names_the_template_makes_are_not_data() {
        let found = of(&[
            "{% set net = qty * price %}",
            "{{ net }}",
            "{% set first = items[0] %}",
            "{{ first.name }}",
            "{% macro row(label, value) %}",
            "{{ label }}",
            "{% endmacro %}",
            "{{ row('a', amount) }}",
            "{{ range(3) }}",
            "{{ 'text' ~ suffix }}",
            "{{ when | date(format='short') }}",
            "{% if vip is defined %}",
        ]);
        let paths: Vec<&str> = found.iter().map(|(p, _)| p.as_str()).collect();
        for absent in [
            "net", "first", "row", "label", "value", "range", "format", "defined", "short",
        ] {
            assert!(!paths.contains(&absent), "{absent} in {paths:?}");
        }
        for present in [
            "qty",
            "price",
            "items[].name",
            "amount",
            "suffix",
            "when",
            "vip",
        ] {
            assert!(paths.contains(&present), "{present} not in {paths:?}");
        }
    }

    #[test]
    fn filters_that_name_fields_add_them_to_the_items() {
        let found = of(&[
            "{{ items | sum(attribute=\"price\") }}",
            "{{ people | selectattr('active') | list | length }}",
        ]);
        assert!(has(&found, "items", Kind::Array), "{found:?}");
        assert!(has(&found, "items[].price", Kind::Value));
        assert!(has(&found, "people[].active", Kind::Value));
    }

    #[test]
    fn form_fields_are_paths_unless_they_hold_tags() {
        let found = analyse_form(&[
            ("customer.name".into(), None),
            ("items.0.qty".into(), Some(String::new())),
            ("greeting".into(), Some("Dear {{ person.first }}".into())),
        ])
        .unwrap();
        let paths: Vec<&str> = found.variables.iter().map(|v| v.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "customer",
                "customer.name",
                "items",
                "items[]",
                "items[].qty",
                "person",
                "person.first"
            ]
        );
    }

    #[test]
    fn mappings_and_unpacked_loops() {
        let found = of(&[
            "{% for code, price in prices.items() %}",
            "{{ code }}",
            "{% endfor %}",
            "{% for a, b in pairs %}",
            "{% endfor %}",
            "{{ row[\"unit price\"] }}",
        ]);
        assert!(has(&found, "prices", Kind::Object), "{found:?}");
        assert!(has(&found, "pairs", Kind::Array));
        assert!(has(&found, "row.unit price", Kind::Value));
        assert!(!found.iter().any(|(p, _)| p == "code" || p == "a"));
    }

    #[test]
    fn a_path_too_deep_or_too_many_paths_are_refused_quickly() {
        let started = std::time::Instant::now();
        let deep = format!("{{{{ {} }}}}", vec!["a"; 20_000].join("."));
        assert_eq!(analyse(&[deep]).unwrap_err(), TooMany::Depth);
        let many: Vec<String> = (0..200_000).map(|i| format!("{{{{ v{i} }}}}")).collect();
        assert_eq!(analyse(&many).unwrap_err(), TooMany::Paths);
        // As many paths as are allowed, each as deep as is allowed.
        let full: Vec<String> = (0..MAX_PATHS / MAX_DEPTH)
            .map(|i| format!("{{{{ r{i}.{} }}}}", vec!["a"; MAX_DEPTH - 1].join(".")))
            .collect();
        assert_eq!(
            analyse(&full).unwrap().variables.len(),
            MAX_PATHS / MAX_DEPTH * MAX_DEPTH
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
    }
}
