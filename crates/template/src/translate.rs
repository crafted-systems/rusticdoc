//! Translations for the `t` filter: tables by language from the request and
//! the service's dictionaries, checked against limits because both come from
//! outside; the lookup that falls back from `fr-ca` to `fr`; and the literal
//! keys a template passes to `t`, for `rusticdoc translate` and validation.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::locale::{fallbacks, normalize_lang};
use crate::{RenderOptions, Settings};

/// Translations by language: `{"fr": {"total": "Montant"}}`. Languages are
/// normalised tags, such as `fr-ca`.
pub type Translations = BTreeMap<String, BTreeMap<String, String>>;

/// Most entries one set of translations may hold, across its languages.
pub const MAX_ENTRIES: usize = 10_000;
/// Longest key, in bytes.
pub const MAX_KEY_BYTES: usize = 256;
/// Longest text, in bytes.
pub const MAX_TEXT_BYTES: usize = 16 * 1024;
/// Most bytes of keys and text one set may hold in all.
pub const MAX_TOTAL_BYTES: usize = 2 * 1024 * 1024;

/// Reads `{"<lang>": {"<key>": "<text>"}}`, sent as `field`. Language tags are
/// normalised, and two that become the same (`fr-FR`, `fr-fr`) are merged.
/// Errors name the field and say what is wrong, for a 400.
pub fn parse_translations(value: &serde_json::Value, field: &str) -> Result<Translations, String> {
    let serde_json::Value::Object(langs) = value else {
        return Err(format!(
            "'{field}' must be a JSON object of languages, such as {{\"fr\": {{\"total\": \"Montant\"}}}}"
        ));
    };
    let mut out = Translations::new();
    let mut budget = Budget::default();
    for (lang, table) in langs {
        let tag = normalize_lang(lang).ok_or_else(|| {
            format!("'{field}' has {lang:?}, which is not a language tag such as fr or fr-CA")
        })?;
        let entries = out.entry(tag).or_default();
        read_table(table, &format!("{field}.{lang}"), entries, &mut budget)?;
    }
    Ok(out)
}

/// Reads one flat `{"<key>": "<text>"}` table, such as a dictionary file.
pub fn parse_table(
    value: &serde_json::Value,
    what: &str,
) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    read_table(value, what, &mut out, &mut Budget::default())?;
    Ok(out)
}

#[derive(Default)]
struct Budget {
    entries: usize,
    bytes: usize,
}

fn read_table(
    value: &serde_json::Value,
    what: &str,
    out: &mut BTreeMap<String, String>,
    budget: &mut Budget,
) -> Result<(), String> {
    let serde_json::Value::Object(map) = value else {
        return Err(format!(
            "'{what}' must be an object of keys and texts, such as {{\"total\": \"Montant\"}}"
        ));
    };
    for (key, text) in map {
        let serde_json::Value::String(text) = text else {
            return Err(format!(
                "'{what}' has a {} for {key:?}; texts are strings",
                kind(text)
            ));
        };
        if key.is_empty() {
            return Err(format!("'{what}' has an empty key"));
        }
        if key.len() > MAX_KEY_BYTES {
            return Err(format!(
                "'{what}' has a key of {} bytes; the limit is {MAX_KEY_BYTES}",
                key.len()
            ));
        }
        if text.len() > MAX_TEXT_BYTES {
            return Err(format!(
                "'{what}' has a text of {} bytes for {key:?}; the limit is {MAX_TEXT_BYTES}",
                text.len()
            ));
        }
        budget.entries += 1;
        budget.bytes += key.len() + text.len();
        if budget.entries > MAX_ENTRIES {
            return Err(format!("'{what}' has more than {MAX_ENTRIES} translations"));
        }
        if budget.bytes > MAX_TOTAL_BYTES {
            return Err(format!(
                "'{what}' holds more than {MAX_TOTAL_BYTES} bytes of translations"
            ));
        }
        out.insert(key.clone(), text.clone());
    }
    Ok(())
}

fn kind(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Everything `t` looks in for one render, highest first: the request's
/// translations, the service's dictionaries, then the template's settings and
/// properties.
#[derive(Debug, Clone, Default)]
pub(crate) struct Translator {
    /// The language as written, for keys such as `total.fr-FR` that templates
    /// have always been able to use.
    raw: Option<String>,
    /// The language normalised, when it is a language tag.
    lang: Option<String>,
    request: Translations,
    dictionary: Translations,
    template: BTreeMap<String, String>,
}

impl Translator {
    pub(crate) fn new(settings: &Settings, options: &RenderOptions) -> Self {
        let raw = options.lang.clone().or_else(|| settings.lang.clone());
        let lang = raw.as_deref().and_then(normalize_lang);
        let mut template = settings.translations.clone();
        // `t.total.fr-CA` answers for `fr-ca` too.
        for (key, text) in &settings.translations {
            if let Some((base, suffix)) = key.rsplit_once('.') {
                if let Some(tag) = normalize_lang(suffix).filter(|t| t != suffix) {
                    template
                        .entry(format!("{base}.{tag}"))
                        .or_insert_with(|| text.clone());
                }
            }
        }
        Self {
            raw,
            lang,
            request: options.translations.clone(),
            dictionary: options.dictionary.clone(),
            template,
        }
    }

    /// The effective language, normalised.
    pub(crate) fn lang(&self) -> Option<&str> {
        self.lang.as_deref()
    }

    /// The translation in the language or a tag it falls back to, without
    /// the template's no-language entries.
    fn in_language(&self, key: &str) -> Option<&String> {
        let lang = self.lang.as_deref();
        if let (Some(raw), true) = (&self.raw, lang != self.raw.as_deref()) {
            if let Some(text) = self.template.get(&format!("{key}.{raw}")) {
                return Some(text);
            }
        }
        for tag in lang.into_iter().flat_map(fallbacks) {
            let found = self
                .request
                .get(tag)
                .and_then(|t| t.get(key))
                .or_else(|| self.dictionary.get(tag).and_then(|t| t.get(key)))
                .or_else(|| self.template.get(&format!("{key}.{tag}")));
            if found.is_some() {
                return found;
            }
        }
        None
    }

    /// `t`: the language and its fallbacks, then the template's entry with no
    /// language, then the key itself.
    pub(crate) fn translate(&self, key: &str) -> String {
        self.in_language(key)
            .or_else(|| self.template.get(key))
            .cloned()
            .unwrap_or_else(|| key.to_string())
    }

    /// True when the key has a text for the language or one it falls back to.
    pub(crate) fn has(&self, key: &str) -> bool {
        self.in_language(key).is_some()
    }
}

/// The effective language of a render, normalised: the request's, else the
/// template's. None when neither is set or it is not a language tag.
pub fn effective_lang(settings: &Settings, options: &RenderOptions) -> Option<String> {
    Translator::new(settings, options)
        .lang()
        .map(str::to_string)
}

/// The keys among `keys` with no translation for the effective language or
/// a tag it falls back to. Empty when there is no language.
pub fn missing_translations<'a>(
    settings: &Settings,
    options: &RenderOptions,
    keys: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let t = Translator::new(settings, options);
    if t.lang().is_none() {
        return Vec::new();
    }
    keys.into_iter()
        .filter(|k| !t.has(k))
        .map(str::to_string)
        .collect()
}

/// A quoted string followed by `| t`, such as `"total" | t` or `'a b'|t`.
static LITERAL_T: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:"((?:[^"\\]|\\.)*)"|'((?:[^'\\]|\\.)*)')\s*\|\s*t\b"#).unwrap()
});

/// The literal keys a tag passes to `t`, wherever they sit in it: `{{ "total"
/// | t }}`, `{{ ("a" | t) ~ ":" }}`, `{% set x = "b" | t %}`. A key from a
/// variable cannot be known before the data arrives and is not found.
pub fn literal_keys(tag: &str) -> Vec<String> {
    LITERAL_T
        .captures_iter(tag)
        .filter_map(|c| c.get(1).or_else(|| c.get(2)))
        .map(|m| unescape(m.as_str()))
        .filter(|k| !k.is_empty())
        .collect()
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// The keys a template's settings translate, less any language suffix:
/// `t.total.fr` and `t.total` both give `total`.
pub fn settings_keys(settings: &Settings) -> BTreeSet<String> {
    settings
        .translations
        .keys()
        .map(|key| match key.rsplit_once('.') {
            Some((base, suffix)) if !base.is_empty() && normalize_lang(suffix).is_some() => {
                base.to_string()
            }
            _ => key.clone(),
        })
        .collect()
}

/// A `{% settings %}` tag in a text template.
static TEXT_SETTINGS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)\{%-?\s*settings\b(.*?)-?%\}").unwrap());

/// Takes the `{% settings %}` tags out of a text template and reads them, as
/// an Office template's are. Newlines inside a tag are kept, so error line
/// numbers still match the file.
pub fn split_settings(source: &str) -> (std::borrow::Cow<'_, str>, Settings) {
    let mut settings = Settings::default();
    if !source.contains("settings") {
        return (source.into(), settings);
    }
    let out = TEXT_SETTINGS.replace_all(source, |c: &regex::Captures| {
        crate::program::apply_settings(&c[1], &mut settings);
        c[0].chars().filter(|&ch| ch == '\n').collect::<String>()
    });
    (out, settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn settings(lang: Option<&str>, t: &[(&str, &str)]) -> Settings {
        Settings {
            lang: lang.map(str::to_string),
            translations: t
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn lookup_falls_back_by_language_then_source() {
        let s = settings(
            Some("fr-ca"),
            &[
                ("total", "Total"),
                ("total.fr", "Montant (gabarit)"),
                ("tax", "Tax"),
                ("only.fr-ca", "Canadien"),
                ("note", "Note"),
            ],
        );
        let options = RenderOptions {
            translations: parse_translations(&json!({"fr": {"total": "Montant"}}), "translations")
                .unwrap(),
            dictionary: parse_translations(
                &json!({"fr": {"total": "Montant (serveur)", "tax": "Taxe"}, "fr-CA": {"tax": "TPS"}}),
                "dictionary",
            )
            .unwrap(),
            ..Default::default()
        };
        let t = Translator::new(&s, &options);
        // The request beats the dictionary and the template for one language.
        assert_eq!(t.translate("total"), "Montant");
        // A more exact language beats a higher source.
        assert_eq!(t.translate("tax"), "TPS");
        assert_eq!(t.translate("only"), "Canadien");
        // No language table has it: the template's plain entry, then the key.
        assert_eq!(t.translate("note"), "Note");
        assert_eq!(t.translate("missing"), "missing");
        assert!(!t.has("note"));
        assert!(t.has("tax"));
    }

    #[test]
    fn the_request_language_wins_and_old_keys_still_work() {
        let s = settings(
            Some("fr-FR"),
            &[("k.fr-FR", "exact"), ("k.de", "Schlüssel")],
        );
        let t = Translator::new(&s, &RenderOptions::default());
        assert_eq!(t.translate("k"), "exact");
        let options = RenderOptions {
            lang: Some("de-at".into()),
            ..Default::default()
        };
        assert_eq!(Translator::new(&s, &options).translate("k"), "Schlüssel");
        // A language that is not a tag still finds its own keys, as before.
        let s = settings(Some("French"), &[("k.French", "oui"), ("k", "plain")]);
        assert_eq!(
            Translator::new(&s, &RenderOptions::default()).translate("k"),
            "oui"
        );
    }

    #[test]
    fn translations_are_checked() {
        let ok = parse_translations(
            &json!({"FR_ca": {"a": "b"}, "fr-CA": {"c": "d"}}),
            "translations",
        )
        .unwrap();
        assert_eq!(ok["fr-ca"].len(), 2);
        let err = |v: serde_json::Value| parse_translations(&v, "translations").unwrap_err();
        assert!(err(json!([])).contains("'translations' must be a JSON object"));
        assert!(err(json!({"french!": {}})).contains("not a language tag"));
        assert!(err(json!({"fr": {"a": 1}})).contains("number for \"a\""));
        assert!(err(json!({"fr": {"": "x"}})).contains("empty key"));
        assert!(err(json!({"fr": {"k".repeat(300): "x"}})).contains("limit is 256"));
        assert!(err(json!({"fr": {"k": "x".repeat(MAX_TEXT_BYTES + 1)}})).contains("text of"));
        let many: serde_json::Map<String, serde_json::Value> = (0..=MAX_ENTRIES)
            .map(|i| (format!("k{i}"), json!("v")))
            .collect();
        assert!(err(json!({"fr": many})).contains("more than 10000"));
    }

    #[test]
    fn literal_keys_are_found_anywhere_in_a_tag() {
        assert_eq!(literal_keys(r#"{{ "total" | t }}"#), ["total"]);
        assert_eq!(
            literal_keys(r#"{{ ("a b"|t) ~ ": " ~ 'it\'s' | t }}"#),
            ["a b", "it's"]
        );
        assert_eq!(literal_keys(r#"{% set x = "due" | t %}"#), ["due"]);
        assert!(literal_keys(r#"{{ "x" | title }} {{ key | t }}"#).is_empty());
        let s = settings(
            None,
            &[
                ("total.fr", "a"),
                ("total", "b"),
                ("invoice.number.de-CH", "c"),
            ],
        );
        assert_eq!(
            settings_keys(&s).into_iter().collect::<Vec<_>>(),
            ["invoice.number", "total"]
        );
    }

    #[test]
    fn text_templates_lose_their_settings_tags() {
        let (out, s) = split_settings("{% settings lang=\"fr\"\n t.a=\"b\" %}x\n{{ y }}");
        assert_eq!(out, "\nx\n{{ y }}");
        assert_eq!(s.lang.as_deref(), Some("fr"));
        assert_eq!(s.translations["a"], "b");
        let (out, _) = split_settings("{{ settings }}");
        assert_eq!(out, "{{ settings }}");
    }
}
