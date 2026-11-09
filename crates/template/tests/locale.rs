//! Formatting by language, translations from the request, and named time
//! zones, through the public API. Expected strings are CLDR 48's, as ICU
//! prints them.

mod common;

use common::*;
use rusticdoc_template::translate::parse_translations;
use rusticdoc_template::{
    add_filters, compile, validate_with, Format, RenderOptions, Settings, Severity,
};
use serde_json::{json, Value};

/// Renders `source` in a text environment with `lang` set by the request.
fn text(source: &str, lang: Option<&str>, data: Value) -> String {
    let options = RenderOptions {
        lang: lang.map(str::to_string),
        ..Default::default()
    };
    text_with(source, &Settings::default(), &options, data)
}

fn text_with(source: &str, settings: &Settings, options: &RenderOptions, data: Value) -> String {
    let mut env = minijinja::Environment::new();
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
    add_filters(&mut env, settings, options);
    env.render_str(
        source,
        minijinja::Value::from(minijinja::value::Serde(&data)),
    )
    .expect("renders")
}

/// Every language the brief names, with: `1234567.891 | number`,
/// `-1234.5 | number(2)`, `1500 | currency("EUR")`, `-1500 | currency("USD")`,
/// `1500 | currency("JPY")`, `1500 | currency("KWD")`, Monday 5 January 2026
/// as `%A %d %B %Y`, and `%B|%b|%a|%p` at 13:00.
const LANGUAGES: &[(&str, [&str; 8])] = &[
    (
        "en",
        [
            "1,234,567.891",
            "-1,234.50",
            "€1,500.00",
            "-$1,500.00",
            "¥1,500",
            "KWD\u{a0}1,500.000",
            "Monday 05 January 2026",
            "January|Jan|Mon|PM",
        ],
    ),
    (
        "en-gb",
        [
            "1,234,567.891",
            "-1,234.50",
            "€1,500.00",
            "-US$1,500.00",
            "JP¥1,500",
            "KWD\u{a0}1,500.000",
            "Monday 05 January 2026",
            "January|Jan|Mon|pm",
        ],
    ),
    (
        "en-us",
        [
            "1,234,567.891",
            "-1,234.50",
            "€1,500.00",
            "-$1,500.00",
            "¥1,500",
            "KWD\u{a0}1,500.000",
            "Monday 05 January 2026",
            "January|Jan|Mon|PM",
        ],
    ),
    (
        "fr",
        [
            "1\u{202f}234\u{202f}567,891",
            "-1\u{202f}234,50",
            "1\u{202f}500,00\u{a0}€",
            "-1\u{202f}500,00\u{a0}$US",
            "1\u{202f}500\u{a0}JPY",
            "1\u{202f}500,000\u{a0}KWD",
            "lundi 05 janvier 2026",
            "janvier|janv.|lun.|PM",
        ],
    ),
    (
        "fr-ca",
        [
            "1\u{a0}234\u{a0}567,891",
            "-1\u{a0}234,50",
            "1\u{a0}500,00\u{a0}€",
            "-1\u{a0}500,00\u{a0}$\u{a0}US",
            "1\u{a0}500\u{a0}¥",
            "1\u{a0}500,000\u{a0}KWD",
            "lundi 05 janvier 2026",
            "janvier|janv.|lun.|p.m.",
        ],
    ),
    (
        "de",
        [
            "1.234.567,891",
            "-1.234,50",
            "1.500,00\u{a0}€",
            "-1.500,00\u{a0}$",
            "1.500\u{a0}¥",
            "1.500,000\u{a0}KWD",
            "Montag 05 Januar 2026",
            "Januar|Jan|Mo.|PM",
        ],
    ),
    (
        "de-ch",
        [
            "1'234'567.891",
            "-1'234.50",
            "EUR\u{a0}1'500.00",
            "$-1'500.00",
            "¥\u{a0}1'500",
            "KWD\u{a0}1'500.000",
            "Montag 05 Januar 2026",
            "Januar|Jan|Mo.|PM",
        ],
    ),
    (
        "es",
        [
            "1.234.567,891",
            "-1234,50",
            "1500,00\u{a0}€",
            "-1500,00\u{a0}US$",
            "1500\u{a0}JPY",
            "1500,000\u{a0}KWD",
            "lunes 05 enero 2026",
            "enero|ene|lun|p.\u{202f}m.",
        ],
    ),
    (
        "it",
        [
            "1.234.567,891",
            "-1234,50",
            "1500,00\u{a0}€",
            "-1500,00\u{a0}USD",
            "1500\u{a0}JPY",
            "1500,000\u{a0}KWD",
            "lunedì 05 gennaio 2026",
            "gennaio|gen|lun|PM",
        ],
    ),
    (
        "pt",
        [
            "1.234.567,891",
            "-1.234,50",
            "€\u{a0}1.500,00",
            "-US$\u{a0}1.500,00",
            "JP¥\u{a0}1.500",
            "KWD\u{a0}1.500,000",
            "segunda-feira 05 janeiro 2026",
            "janeiro|jan.|seg.|PM",
        ],
    ),
    (
        "pt-br",
        [
            "1.234.567,891",
            "-1.234,50",
            "€\u{a0}1.500,00",
            "-US$\u{a0}1.500,00",
            "JP¥\u{a0}1.500",
            "KWD\u{a0}1.500,000",
            "segunda-feira 05 janeiro 2026",
            "janeiro|jan.|seg.|PM",
        ],
    ),
    (
        "nl",
        [
            "1.234.567,891",
            "-1.234,50",
            "€\u{a0}1.500,00",
            "US$\u{a0}-1.500,00",
            "JP¥\u{a0}1.500",
            "KWD\u{a0}1.500,000",
            "maandag 05 januari 2026",
            "januari|jan|ma|p.m.",
        ],
    ),
    (
        "sv",
        [
            "1\u{a0}234\u{a0}567,891",
            "\u{2212}1\u{a0}234,50",
            "1\u{a0}500,00\u{a0}€",
            "\u{2212}1\u{a0}500,00\u{a0}US$",
            "1\u{a0}500\u{a0}JPY",
            "1\u{a0}500,000\u{a0}KWD",
            "måndag 05 januari 2026",
            "januari|jan.|mån|em",
        ],
    ),
    (
        "da",
        [
            "1.234.567,891",
            "-1.234,50",
            "1.500,00\u{a0}€",
            "-1.500,00\u{a0}US$",
            "1.500\u{a0}JP¥",
            "1.500,000\u{a0}KWD",
            "mandag 05 januar 2026",
            "januar|jan.|man.|PM",
        ],
    ),
    (
        "nb",
        [
            "1\u{a0}234\u{a0}567,891",
            "\u{2212}1\u{a0}234,50",
            "1\u{a0}500,00\u{a0}€",
            "\u{2212}1\u{a0}500,00\u{a0}USD",
            "1\u{a0}500\u{a0}JPY",
            "1\u{a0}500,000\u{a0}KWD",
            "mandag 05 januar 2026",
            "januar|jan|man.|p.m.",
        ],
    ),
    (
        "fi",
        [
            "1\u{a0}234\u{a0}567,891",
            "\u{2212}1\u{a0}234,50",
            "1\u{a0}500,00\u{a0}€",
            "\u{2212}1\u{a0}500,00\u{a0}$",
            "1\u{a0}500\u{a0}¥",
            "1\u{a0}500,000\u{a0}KWD",
            "maanantaina 05 tammikuuta 2026",
            "tammikuu|tammi|ma|ip.",
        ],
    ),
    (
        "pl",
        [
            "1\u{a0}234\u{a0}567,891",
            "-1234,50",
            "1500,00\u{a0}€",
            "-1500,00\u{a0}USD",
            "1500\u{a0}JPY",
            "1500,000\u{a0}KWD",
            "poniedziałek 05 stycznia 2026",
            "styczeń|sty|pon.|PM",
        ],
    ),
    (
        "cs",
        [
            "1\u{a0}234\u{a0}567,891",
            "-1\u{a0}234,50",
            "1\u{a0}500,00\u{a0}€",
            "-1\u{a0}500,00\u{a0}US$",
            "1\u{a0}500\u{a0}JP¥",
            "1\u{a0}500,000\u{a0}KWD",
            "pondělí 05 ledna 2026",
            "leden|led|po|odp.",
        ],
    ),
    (
        "ru",
        [
            "1\u{a0}234\u{a0}567,891",
            "-1\u{a0}234,50",
            "1\u{a0}500,00\u{a0}€",
            "-1\u{a0}500,00\u{a0}$",
            "1\u{a0}500\u{a0}¥",
            "1\u{a0}500,000\u{a0}KWD",
            "понедельник 05 января 2026",
            "январь|янв.|пн|PM",
        ],
    ),
    (
        "tr",
        [
            "1.234.567,891",
            "-1.234,50",
            "€1.500,00",
            "-$1.500,00",
            "¥1.500",
            "KWD\u{a0}1.500,000",
            "Pazartesi 05 Ocak 2026",
            "Ocak|Oca|Pzt|ÖS",
        ],
    ),
    (
        "ar",
        [
            "1,234,567.891",
            "\u{200e}-1,234.50",
            "\u{200f}1,500.00\u{a0}€",
            "\u{200f}\u{200e}-1,500.00\u{a0}US$",
            "\u{200f}1,500\u{a0}JP¥",
            "\u{200f}1,500.000\u{a0}د.ك.\u{200f}",
            "الاثنين 05 يناير 2026",
            "يناير|يناير|الاثنين|م",
        ],
    ),
    (
        "he",
        [
            "1,234,567.891",
            "\u{200e}-1,234.50",
            "\u{200f}1,500.00\u{a0}\u{200f}€",
            "\u{200f}\u{200e}-1,500.00\u{a0}\u{200f}$",
            "\u{200f}1,500\u{a0}\u{200f}¥",
            "\u{200f}1,500.000\u{a0}\u{200f}KWD",
            "יום שני 05 ינואר 2026",
            "ינואר|ינו׳|יום ב׳|PM",
        ],
    ),
    (
        "ja",
        [
            "1,234,567.891",
            "-1,234.50",
            "€1,500.00",
            "-$1,500.00",
            "￥1,500",
            "KWD\u{a0}1,500.000",
            "月曜日 05 1月 2026",
            "1月|1月|月|午後",
        ],
    ),
    (
        "zh",
        [
            "1,234,567.891",
            "-1,234.50",
            "€1,500.00",
            "-US$1,500.00",
            "JP¥1,500",
            "KWD\u{a0}1,500.000",
            "星期一 05 一月 2026",
            "一月|1月|周一|下午",
        ],
    ),
    (
        "ko",
        [
            "1,234,567.891",
            "-1,234.50",
            "€1,500.00",
            "-US$1,500.00",
            "JP¥1,500",
            "KWD\u{a0}1,500.000",
            "월요일 05 1월 2026",
            "1월|1월|월|오후",
        ],
    ),
];

#[test]
fn every_listed_language_formats_as_cldr_does() {
    let source = r#"{{ 1234567.891 | number }}~{{ -1234.5 | number(2) }}~{{ 1500 | currency("EUR") }}~{{ -1500 | currency("USD") }}~{{ 1500 | currency("JPY") }}~{{ 1500 | currency("KWD") }}~{{ d | date("%A %d %B %Y") }}~{{ d | date("%B|%b|%a|%p") }}"#;
    for (lang, want) in LANGUAGES {
        let got = text(source, Some(lang), json!({"d": "2026-01-05T13:00:00"}));
        let got: Vec<&str> = got.split('~').collect();
        assert_eq!(&got, want, "{lang}");
    }
}

#[test]
fn every_language_with_data_matches_icu() {
    let vectors: Value =
        serde_json::from_str(include_str!("fixtures/cldr-vectors.json")).expect("vectors");
    let mut checked = 0;
    for (lang, v) in vectors["locales"].as_object().unwrap() {
        for case in v["number"].as_array().unwrap() {
            let value = &case[0];
            let got = text(
                "{{ v | number }}~{{ v | number(2) }}~{{ v | number(0) }}",
                Some(lang),
                json!({ "v": value }),
            );
            let want = format!(
                "{}~{}~{}",
                case[1].as_str().unwrap(),
                case[2].as_str().unwrap(),
                case[3].as_str().unwrap()
            );
            assert_eq!(got, want, "{lang} number {value}");
            checked += 1;
        }
        for case in v["currency"].as_array().unwrap() {
            let got = text(
                "{{ v | currency(code) }}",
                Some(lang),
                json!({ "v": case[1], "code": case[0] }),
            );
            assert_eq!(
                got,
                case[2].as_str().unwrap(),
                "{lang} {} {}",
                case[0],
                case[1]
            );
            checked += 1;
        }
    }
    assert!(checked > 2000, "{checked}");
}

#[test]
fn unknown_languages_fall_back_to_their_base_then_to_plain() {
    let source = r#"{{ 1234.5 | number(2) }} {{ 1500 | currency("EUR") }} {{ d | date("%B") }}"#;
    let d = json!({"d": "2026-03-01"});
    assert_eq!(
        text(source, Some("fr-LU"), d.clone()),
        text(source, Some("fr"), d.clone())
    );
    assert_eq!(
        text(source, Some("de-AT"), d.clone()),
        "1\u{a0}234,50 €\u{a0}1.500,00 März"
    );
    let plain = "1,234.50 EUR 1,500.00 March";
    assert_eq!(text(source, Some("xx"), d.clone()), plain);
    assert_eq!(text(source, None, d), plain);
}

#[test]
fn without_a_language_output_is_as_it_always_was() {
    // The outputs of `currency` and `date` before languages formatted
    // anything, byte for byte.
    let source = r#"{{ 1234.5 | currency("USD") }}|{{ -1234567.891 | currency("USD") }}|{{ 999.999 | currency("EUR") }}|{{ 0.5 | currency }}|{{ 2 | currency("") }}|{{ "2026-11-06T10:20:30" | date("%A %d %B %Y %p") }}|{{ 1793980800 | date }}"#;
    assert_eq!(
        text(source, None, json!({})),
        "USD 1,234.50|-USD 1,234,567.89|EUR 1,000.00|0.50|2.00|Friday 06 November 2026 AM|2026-11-06"
    );
    assert_eq!(
        text("{{ 1234.5 | number }} {{ 2 | number(2) }}", None, json!({})),
        "1,234.5 2.00"
    );
}

#[test]
fn month_names_read_alone_or_in_a_date() {
    let d = json!({"d": "2026-01-05"});
    assert_eq!(
        text(r#"{{ d | date("%d %B") }}"#, Some("ru"), d.clone()),
        "05 января"
    );
    assert_eq!(
        text(r#"{{ d | date("%B %Y") }}"#, Some("ru"), d.clone()),
        "январь 2026"
    );
    assert_eq!(
        text(r#"{{ d | date("%-d. %B") }}"#, Some("cs"), d.clone()),
        "5. ledna"
    );
    assert_eq!(text(r#"{{ d | date("%P") }}"#, Some("en"), d), "am");
}

#[test]
fn named_zones_follow_daylight_saving() {
    let source = r#"{{ "2026-03-28T12:00:00Z" | date("%d %H:%M") }} {{ "2026-03-30T12:00:00Z" | date("%d %H:%M") }} {{ 1793980800 | date("%H:%M") }}"#;
    let settings = Settings {
        timezone: Some("Europe/Paris".into()),
        ..Default::default()
    };
    // Paris moves from +01:00 to +02:00 on 29 March 2026.
    assert_eq!(
        text_with(source, &settings, &RenderOptions::default(), json!({})),
        "28 13:00 30 14:00 17:00"
    );
    let ny = RenderOptions {
        timezone: Some("America/New_York".into()),
        ..Default::default()
    };
    // New York moves on 8 March 2026, and the request wins over the template.
    assert_eq!(
        text_with(
            r#"{{ "2026-03-07T17:00:00Z" | date("%H:%M") }} {{ "2026-03-09T17:00:00Z" | date("%H:%M") }}"#,
            &settings,
            &ny,
            json!({})
        ),
        "12:00 13:00"
    );
}

#[test]
fn request_settings_win_over_the_template() {
    let doc = docx(&p(
        "{% settings lang=\"de\" currency=\"EUR\" t.total=\"Total\" t.total.de=\"Summe\" t.total.fr=\"Montant (modèle)\" %}{{ \"total\" | t }} {{ 1500 | currency }} {{ \"due\" | t }}",
    ));
    let compiled = compile(Format::Docx, &doc).unwrap();
    let plain = docx_text(&compiled.render(&json!({})).unwrap());
    assert_eq!(plain, ["Summe 1.500,00\u{a0}€ due"]);
    let options = RenderOptions {
        lang: Some("fr-ca".into()),
        currency: Some("CAD".into()),
        translations: parse_translations(&json!({"fr": {"due": "Échéance"}}), "translations")
            .unwrap(),
        ..Default::default()
    };
    let out = docx_text(&compiled.render_with(&json!({}), &options).unwrap());
    assert_eq!(out, ["Montant (modèle) 1\u{a0}500,00\u{a0}$ Échéance"]);
}

#[test]
fn validation_warns_of_missing_translations() {
    let doc = docx(
        &(p("{% settings t.total.fr=\"Montant\" t.note=\"Note\" %}{{ \"total\" | t }}")
            + &p("{{ \"note\" | t }} and {{ (\"tax\" | t) ~ \":\" }}")),
    );
    let options = RenderOptions {
        lang: Some("fr-CA".into()),
        dictionary: parse_translations(&json!({"fr": {"tax": "Taxe"}}), "dictionary").unwrap(),
        ..Default::default()
    };
    let found = validate_with(Format::Docx, &doc, &json!({}), &options);
    let warnings: Vec<&str> = found
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(warnings.len(), 1, "{found:?}");
    assert!(
        warnings[0].starts_with("no fr-ca translation for \"note\""),
        "{warnings:?}"
    );
    assert!(found.iter().all(|d| d.severity == Severity::Warning));
    // No language, no warnings.
    let found = validate_with(Format::Docx, &doc, &json!({}), &RenderOptions::default());
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn translation_keys_cover_tags_and_settings() {
    let doc = docx(
        &(p("{% settings t.title.fr=\"Titre\" %}{{ \"total\" | t }}")
            + &p("{% if x %}{{ ('a' | t) ~ \"b\" | t }}{% endif %}")),
    );
    let compiled = compile(Format::Docx, &doc).unwrap();
    let keys: Vec<String> = compiled.translation_keys().into_iter().collect();
    assert_eq!(keys, ["a", "b", "title", "total"]);
}
