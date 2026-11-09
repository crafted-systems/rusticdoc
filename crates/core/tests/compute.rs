//! The calculation and condition helpers in text templates and in the
//! environment PDF form tags and `batchName` use.

use rusticdoc_core::Document;
use rusticdoc_core::template;
use serde_json::json;

const LIMIT: usize = 1 << 20;

fn render(name: &str, source: &str, data: serde_json::Value, ctx: serde_json::Value) -> String {
    let doc = Document::new(name, source.as_bytes().to_vec());
    let out = template::render(&doc, &data, &ctx, LIMIT, &Default::default()).expect("renders");
    String::from_utf8(out.bytes.to_vec()).unwrap()
}

#[test]
fn text_template_calculations_and_conditions() {
    let source = "\
{{ ref | pad_right(8, \".\") }}|{{ total | words }}|{{ total | words(currency=\"EUR\") }}
{{ now() }}|{{ now() | end_of(\"month\") }}|{{ due | date_diff(now(), \"days\") }}
{{ (90 * 60 * 1000) | duration }}|{{ \"P1DT12H\" | duration(\"hours\") }}
{% if tags is empty %}no tags{% elif tags | length > 1 %}tags{% else %}one tag{% endif %}|{{ title | truncate(12, words=true) }}";
    let out = render(
        "letter.txt",
        source,
        json!({"ref": "A7", "total": 1234, "due": "2026-03-31", "tags": [], "title": "Annual report for the board"}),
        json!({"now": "2026-02-14T09:30:00Z"}),
    );
    assert_eq!(
        out,
        "A7......|one thousand two hundred thirty-four|one thousand two hundred thirty-four euros\n\
         2026-02-14T09:30:00Z|2026-02-28T23:59:59Z|44\n\
         2 hours|36\n\
         no tags|Annual…"
    );
}

#[test]
fn html_templates_escape_what_the_helpers_return() {
    let out = render(
        "page.html",
        "<p>{{ name | truncate(6) }}</p>",
        json!({"name": "<b>bold</b>"}),
        json!({}),
    );
    assert_eq!(out, "<p>&lt;b&gt;bo…</p>");
}

#[test]
fn the_shared_environment_has_the_helpers() {
    // PDF form tags and batchName render with this environment.
    let env = template::environment();
    let out = template::render_str(
        &env,
        "{{ name | unaccent | upper }}-{{ n | pad_left(3, \"0\") }}-{{ d | start_of(\"quarter\") }}",
        minijinja::Value::from(minijinja::value::Serde(json!({"name": "Zoë", "n": 7, "d": "2026-11-09"}))),
        LIMIT,
    )
    .unwrap();
    assert_eq!(out, "ZOE-007-2026-10-01");
    let err = template::render_str(
        &env,
        "{{ 'x' | pad_left(1000000) }}",
        minijinja::Value::from(()),
        LIMIT,
    )
    .unwrap_err();
    assert!(err.to_string().contains("between 0 and 10000"), "{err}");
}
