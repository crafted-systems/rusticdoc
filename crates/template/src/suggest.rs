//! A hint for an undefined value whose name differs from a defined one only
//! in case: `{{ Balance }}` when the data has `balance`.
//!
//! Names stay case-sensitive when rendering; this only helps `check` say
//! what was probably meant.

use minijinja::value::{Value, ValueKind};
use minijinja::State;

/// One step of a name path: `customer`, `.name`, `["first name"]`, `[0]`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Key(String),
    Index(i64),
}

/// The name in `expr` that resolves once its case is corrected, written as it
/// should be, such as `customer.name`. None when no name in `expr` is off by
/// case alone, or when more than one name would fit.
pub fn case_hint(state: &State, expr: &str) -> Option<String> {
    paths(expr)
        .into_iter()
        .find_map(|path| correct(state, &path))
}

/// Resolves `path` with case-insensitive fallback at each key, returning the
/// corrected path only if something had to be corrected and it then resolves.
fn correct(state: &State, path: &[Step]) -> Option<String> {
    let Step::Key(root) = path.first()? else {
        return None;
    };
    let mut fixed = false;
    let (root, mut value) = match state.lookup(root) {
        Some(v) if !v.is_undefined() => (root.clone(), v),
        _ => {
            let name = unique_match(state.known_variables().iter().map(|n| n.as_ref()), root)?;
            fixed = true;
            let v = state.lookup(&name)?;
            (name, v)
        }
    };
    let mut out = vec![Step::Key(root)];
    for step in &path[1..] {
        let next = match step {
            Step::Index(i) => value.get_item(&Value::from(*i)).ok(),
            Step::Key(k) => value.get_item(&Value::from(k.as_str())).ok(),
        };
        match next {
            Some(v) if !v.is_undefined() => {
                out.push(step.clone());
                value = v;
            }
            _ => {
                let Step::Key(k) = step else {
                    return None;
                };
                if value.kind() != ValueKind::Map {
                    return None;
                }
                let keys: Vec<String> = value
                    .try_iter()
                    .ok()?
                    .filter_map(|k| k.as_str().map(str::to_string))
                    .collect();
                let name = unique_match(keys.iter().map(String::as_str), k)?;
                value = value.get_item(&Value::from(name.as_str())).ok()?;
                if value.is_undefined() {
                    return None;
                }
                fixed = true;
                out.push(Step::Key(name));
            }
        }
    }
    fixed.then(|| display(&out))
}

/// The one candidate equal to `name` ignoring case, but not equal to it.
fn unique_match<'a>(candidates: impl Iterator<Item = &'a str>, name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    let mut found: Option<&str> = None;
    for c in candidates {
        if c != name && c.to_lowercase() == lower {
            if found.is_some_and(|f| f != c) {
                return None;
            }
            found = Some(c);
        }
    }
    found.map(str::to_string)
}

fn display(path: &[Step]) -> String {
    let mut s = String::new();
    for (i, step) in path.iter().enumerate() {
        match step {
            Step::Key(k) if is_ident(k) => {
                if i > 0 {
                    s.push('.');
                }
                s.push_str(k);
            }
            Step::Key(k) => s.push_str(&format!("[{k:?}]")),
            Step::Index(n) => s.push_str(&format!("[{n}]")),
        }
    }
    s
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

const KEYWORDS: &[&str] = &[
    "and",
    "or",
    "not",
    "in",
    "is",
    "if",
    "else",
    "for",
    "set",
    "elif",
    "true",
    "false",
    "none",
    "True",
    "False",
    "None",
    "recursive",
];

/// The variable paths an expression reads, such as `customer.name` and
/// `items[0]["unit price"]`. Filter and test names, function calls, keyword
/// arguments and the contents of string literals are left out.
fn paths(expr: &str) -> Vec<Vec<Step>> {
    let b = expr.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    // The last significant token before the current position.
    let mut prev: Option<&str> = None;
    while i < b.len() {
        let c = b[i];
        if c == b'\'' || c == b'"' {
            i = skip_string(b, i);
            prev = Some("str");
        } else if c.is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.' || b[i] == b'_') {
                i += 1;
            }
            prev = Some("num");
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            i = ident_end(b, i);
            let name = &expr[start..i];
            let after_dot = prev == Some(".");
            let is_name =
                !KEYWORDS.contains(&name) && !matches!(prev, Some("|") | Some("is")) && !after_dot;
            let mut path = vec![Step::Key(name.to_string())];
            // The rest of the chain.
            loop {
                if i < b.len() && b[i] == b'.' && i + 1 < b.len() && is_ident_start(b[i + 1]) {
                    let s = i + 1;
                    i = ident_end(b, s);
                    path.push(Step::Key(expr[s..i].to_string()));
                } else if i < b.len() && b[i] == b'[' {
                    match subscript(expr, i) {
                        Some((step, end)) => {
                            path.push(step);
                            i = end;
                        }
                        None => break,
                    }
                } else {
                    break;
                }
            }
            let mut j = i;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            let call = j < b.len() && b[j] == b'(';
            let kwarg = j < b.len() && b[j] == b'=' && b.get(j + 1) != Some(&b'=');
            if call {
                // `f(x)` is a function; `name.upper()` reads `name`.
                path.pop();
            }
            if is_name && !kwarg && !path.is_empty() {
                out.push(path);
            }
            prev = Some(match name {
                "is" => "is",
                n if KEYWORDS.contains(&n) => "kw",
                _ => "name",
            });
        } else if c.is_ascii_whitespace() {
            i += 1;
        } else {
            prev = Some(match c {
                b'|' => "|",
                b'.' => ".",
                _ => "op",
            });
            i += 1;
        }
    }
    out
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn ident_end(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
        i += 1;
    }
    i
}

/// The index of the byte after the string literal starting at `i`.
fn skip_string(b: &[u8], i: usize) -> usize {
    let quote = b[i];
    let mut j = i + 1;
    while j < b.len() && b[j] != quote {
        if b[j] == b'\\' {
            j += 1;
        }
        j += 1;
    }
    (j + 1).min(b.len())
}

/// A `["key"]` or `[3]` subscript at `i`, and the index after it.
fn subscript(expr: &str, i: usize) -> Option<(Step, usize)> {
    let b = expr.as_bytes();
    let mut j = i + 1;
    while j < b.len() && b[j].is_ascii_whitespace() {
        j += 1;
    }
    let (step, mut k) = if j < b.len() && (b[j] == b'"' || b[j] == b'\'') {
        let end = skip_string(b, j);
        let inner = expr.get(j + 1..end.checked_sub(1)?)?;
        if inner.contains('\\') {
            return None;
        }
        (Step::Key(inner.to_string()), end)
    } else {
        let start = j;
        if j < b.len() && b[j] == b'-' {
            j += 1;
        }
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        (Step::Index(expr[start..j].parse().ok()?), j)
    };
    while k < b.len() && b[k].is_ascii_whitespace() {
        k += 1;
    }
    (k < b.len() && b[k] == b']').then_some((step, k + 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use minijinja::Environment;
    use serde_json::json;

    fn keys(s: &[&str]) -> Vec<Step> {
        s.iter().map(|k| Step::Key(k.to_string())).collect()
    }

    #[test]
    fn finds_the_paths_an_expression_reads() {
        assert_eq!(paths("{{ Balance }}"), vec![keys(&["Balance"])]);
        assert_eq!(
            paths("customer.Name | upper | default('x', true)"),
            vec![keys(&["customer", "Name"])]
        );
        assert_eq!(
            paths(r#"items[0]["Unit price"] * qty"#),
            vec![
                vec![
                    Step::Key("items".into()),
                    Step::Index(0),
                    Step::Key("Unit price".into())
                ],
                keys(&["qty"])
            ]
        );
        assert_eq!(
            paths("x is defined and not y"),
            vec![keys(&["x"]), keys(&["y"])]
        );
        assert_eq!(
            paths("t('Hello', name=Who) ~ who.Title()"),
            vec![keys(&["Who"]), keys(&["who"])]
        );
        assert_eq!(paths("row in Rows"), vec![keys(&["row"]), keys(&["Rows"])]);
        assert_eq!(paths("1.5 + 'Total'"), Vec::<Vec<Step>>::new());
    }

    fn hint(expr: &str, data: serde_json::Value) -> Option<String> {
        let mut env = Environment::new();
        let expr = expr.to_string();
        env.add_function("__hint", move |state: &State| {
            case_hint(state, &expr).unwrap_or_default()
        });
        let out = env
            .render_str(
                "{{ __hint() }}",
                Value::from(minijinja::value::Serde(&data)),
            )
            .unwrap();
        (!out.is_empty()).then_some(out)
    }

    #[test]
    fn suggests_a_name_that_differs_only_in_case() {
        let data = json!({
            "balance": 3,
            "customer": {"Name": "Ada", "address": {"City": "Nairobi"}},
            "items": [{"unit price": 2}],
            "both": {"x": 1, "X": 2}
        });
        assert_eq!(hint("Balance", data.clone()).as_deref(), Some("balance"));
        assert_eq!(
            hint("BALANCE + 1", data.clone()).as_deref(),
            Some("balance")
        );
        assert_eq!(
            hint("customer.name", data.clone()).as_deref(),
            Some("customer.Name")
        );
        assert_eq!(
            hint("Customer.Address.city", data.clone()).as_deref(),
            Some("customer.address.City")
        );
        assert_eq!(
            hint(r#"items[0]["Unit Price"]"#, data.clone()).as_deref(),
            Some(r#"items[0]["unit price"]"#)
        );
        // Nothing to suggest: right already, missing whatever the case, or
        // ambiguous.
        assert_eq!(hint("balance", data.clone()), None);
        assert_eq!(hint("customer.phone", data.clone()), None);
        assert_eq!(hint("Customer.phone", data.clone()), None);
        assert_eq!(hint("Both", data.clone()), Some("both".into()));
        let data = json!({"Total": 1, "TOTAL": 2});
        assert_eq!(hint("total", data), None);
    }
}
