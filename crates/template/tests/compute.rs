//! The calculation and condition helpers inside Office templates.

mod common;

use common::*;
use rusticdoc_template::{compile, Format};
use serde_json::json;

fn invoice() -> serde_json::Value {
    json!({
        "number": 42,
        "issued": "2024-01-31",
        "note": "",
        "lines": [
            {"name": "Crème brûlée", "group": "Food", "qty": 2, "price": 4.5},
            {"name": "Espresso", "group": "Drinks", "qty": 3, "price": 2.25},
            {"name": "Tarte Tatin", "group": "Food", "qty": 1, "price": 6.0}
        ],
        "ctx": {"now": "2024-02-29T12:00:00Z"}
    })
}

#[test]
fn docx_calculations_and_conditions() {
    let doc = docx(
        &(p("{% settings lang=\"fr\" %}Invoice {{ number | pad_left(6, \"0\") }}")
            + &p("Due {{ issued | date_add(1, \"month\") | date(\"%d/%m/%Y\") }}, {{ now() | date_diff(issued, \"days\") }} days after issue")
            + &p("{% for g in lines | groupby(\"group\") %}{{ g.grouper | upper }}: {{ g.list | sum(attribute=\"qty\") }}{% if not loop.last %}, {% endif %}{% endfor %}")
            + &p("{% set ns = namespace(balance=0) %}{% for l in lines %}{% set ns.balance = ns.balance + l.qty * l.price %}{{ l.name | unaccent | truncate(8, end=\".\") }} {{ ns.balance }}; {% endfor %}")
            + &p("Average {{ lines | avg(attribute=\"price\") | round(2) }}, at least {{ lines | map(attribute=\"price\") | min | ceil }}")
            + &p("{{ 81 | words }} / {{ 81 | words(lang=\"en\") }}")
            + &p("{% if note is empty %}No note{% else %}{{ note }}{% endif %}")
            + &p("{% if lines is not empty %}Lines follow")
            + &p("{{ lines | length }} lines{% endif %}")
            + &p("{% if note is not empty %}Note: {{ note }}")
            + &p("Hidden{% endif %}End")),
    );
    let compiled = compile(Format::Docx, &doc).expect("compiles");
    let out = compiled.render(&invoice()).expect("renders");
    let text = docx_text(&out);
    assert_eq!(text[0], "Invoice 000042");
    assert_eq!(text[1], "Due 29/02/2024, 29 days after issue");
    assert_eq!(text[2], "DRINKS: 3, FOOD: 3");
    assert_eq!(text[3], "Creme b. 9.0; Espresso 15.75; Tarte T. 21.75; ");
    assert_eq!(text[4], "Average 4.25, at least 3");
    assert_eq!(text[5], "quatre-vingt-un / eighty-one");
    assert_eq!(text[6], "No note");
    assert_eq!(text[7], "Lines follow");
    assert_eq!(text[8], "3 lines");
    // The closing paragraph belongs to the false branch, text after the tag too.
    assert_eq!(text.len(), 9, "{text:?}");
}

#[test]
fn xlsx_cells_get_numbers_from_the_helpers() {
    let sheet = xlsx(
        &[row(
            1,
            &[
                cell("A1", "s", "0"),
                cell("B1", "s", "1"),
                cell("C1", "s", "2"),
            ],
        )],
        &[
            "{{ prices | avg }}",
            "{{ 7.2 | floor }}",
            "{{ 5400000 | duration(\"hours\") }}",
        ],
    );
    let out = compile(Format::Xlsx, &sheet)
        .expect("compiles")
        .render(&json!({"prices": [1, 2, 6]}))
        .expect("renders");
    let cells = cells_resolved(&out);
    assert!(cells.contains(&("A1".into(), "3".into())), "{cells:?}");
    assert!(cells.contains(&("B1".into(), "7".into())), "{cells:?}");
    assert!(cells.contains(&("C1".into(), "1.5".into())), "{cells:?}");
}
