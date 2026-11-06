mod common;

use common::*;
use rusticdoc_template::{compile, validate, Format, Severity};
use serde_json::{json, Value};

fn render(format: Format, bytes: &[u8], data: Value) -> Vec<u8> {
    let compiled = compile(format, bytes).expect("compiles");
    compiled.render(&data).expect("renders")
}

fn items() -> Value {
    json!({
        "customer": {"name": "Ada & Co", "vip": true},
        "items": [
            {"name": "Lamp", "qty": 2, "price": 19.5},
            {"name": "Plug", "qty": 1, "price": 0.5},
            {"name": "Desk", "qty": 1, "price": 1200.0}
        ],
        "ctx": {"company": "Crafted"}
    })
}

// --- DOCX -----------------------------------------------------------------

#[test]
fn docx_substitution_and_filters() {
    let doc = docx(&(p("Dear {{ customer.name }},")
        + &p("Total {{ items | sum(attribute=\"price\") | currency(\"EUR\") }} from {{ ctx.company }}")
        + &p("Count {{ items | length }} <escaped> & {{ \"a < b\" }} {{ nul }}")));
    let mut data = items();
    // NUL separates parts internally; a value cannot forge a part boundary.
    data["nul"] = json!("x\u{0}word/evil.xml\u{0}y\u{1}z");
    let out = render(Format::Docx, &doc, data);
    let text = docx_text(&out);
    assert_eq!(text[0], "Dear Ada & Co,");
    assert_eq!(text[1], "Total EUR 1,220.00 from Crafted");
    assert_eq!(text[2], "Count 3 <escaped> & a < b xword/evil.xmlyz");
    assert!(entries(&out).iter().all(|(n, _)| !n.contains("evil")));
    let xml = part(&out, "word/document.xml");
    assert!(xml.contains("Ada &amp; Co"), "output must stay well-formed");
    assert!(xml.contains("a &lt; b"));
    let header = paragraphs(&part(&out, "word/header1.xml"), "w:p", "w:t");
    assert_eq!(header[0], "Header text");
}

#[test]
fn tag_split_across_three_runs_with_proof_err() {
    let doc = docx(
        &(p("Hello {{ cust§!omer.na§me }} and {{ items[0].qty }§} more") + &p("{§{ \"a\" }}")),
    );
    let compiled = compile(Format::Docx, &doc).unwrap();
    assert_eq!(
        compiled.tags(),
        ["{{ customer.name }}", "{{ items[0].qty }}", "{{ \"a\" }}"]
    );
    let out = compiled.render(&items()).unwrap();
    let text = docx_text(&out);
    assert_eq!(text[0], "Hello Ada & Co and 2 more");
    assert_eq!(text[1], "a");
    let xml = part(&out, "word/document.xml");
    // The merged text took the first run's properties; the tail kept its run.
    assert!(
        xml.contains(
            "<w:rsid w:val=\"000\"/></w:rPr><w:t xml:space=\"preserve\">Hello Ada &amp; Co</w:t>"
        ),
        "{xml}"
    );
    assert!(
        xml.contains("<w:rsid w:val=\"003\"/></w:rPr><w:t xml:space=\"preserve\"> more</w:t>"),
        "{xml}"
    );
    assert!(
        xml.contains("<w:rsid w:val=\"002\"/></w:rPr><w:t xml:space=\"preserve\"> and 2</w:t>"),
        "{xml}"
    );
}

#[test]
fn entities_and_curly_quotes_inside_tags() {
    // The builder escapes `"` and `<` as Word does; the engine decodes them.
    let doc = docx(&p(
        "{{ missing | default(\"n/a\") }} {{ “quoted” }} {{ 1 < 2 }}",
    ));
    assert!(part(&doc, "word/document.xml").contains("default(&quot;n/a&quot;)"));
    let out = render(Format::Docx, &doc, json!({}));
    assert_eq!(docx_text(&out)[0], "n/a quoted True");
}

fn row_loop_doc() -> Vec<u8> {
    docx(
        &(p("Items:")
            + &tbl(&[
                &["Name", "Qty"],
                &[
                    "{% for i in items %}{{ i.name }}",
                    "{{ i.qty }}{% endfor %}",
                ],
                &["Total", "{{ items | sum(attribute=\"qty\") }}"],
            ])
            + &p("End")),
    )
}

#[test]
fn docx_row_loop_with_zero_one_and_three_items() {
    let doc = row_loop_doc();
    let compiled = compile(Format::Docx, &doc).unwrap();
    for (n, data) in [
        (0, json!({"items": []})),
        (1, json!({"items": [{"name": "Lamp", "qty": 2}]})),
        (3, items()),
    ] {
        let out = compiled.render(&data).unwrap();
        let xml = part(&out, "word/document.xml");
        let rows = xml.matches("<w:tr>").count();
        assert_eq!(rows, 2 + n, "{n} items give {} rows: {xml}", 2 + n);
        let text = docx_text(&out);
        assert_eq!(text[0], "Items:");
        assert_eq!(text[1], "Name");
        if n == 3 {
            assert_eq!(&text[3..9], ["Lamp", "2", "Plug", "1", "Desk", "1"]);
            assert_eq!(text[10], "4");
        }
        assert_eq!(text.last().unwrap(), "End");
    }
}

#[test]
fn docx_rows_between_for_and_endfor_repeat() {
    let doc = docx(&tbl(&[
        &["{% for i in items %}{{ i.name }}", ""],
        &["note", "{{ i.qty }}{% endfor %}"],
    ]));
    let out = render(Format::Docx, &doc, items());
    let xml = part(&out, "word/document.xml");
    assert_eq!(xml.matches("<w:tr>").count(), 6);
    let text = docx_text(&out);
    assert_eq!(&text[..4], ["Lamp", "", "note", "2"]);
}

#[test]
fn docx_paragraph_loop() {
    let doc = docx(
        &(p("Before")
            + &p("{% for i in items %}Item {{ loop.index }}")
            + &p("{{ i.name }}: {{ i.price | currency }}{% endfor %}")
            + &p("After")),
    );
    let out = render(Format::Docx, &doc, items());
    let text = docx_text(&out);
    assert_eq!(
        text,
        [
            "Before",
            "Item 1",
            "Lamp: 19.50",
            "Item 2",
            "Plug: 0.50",
            "Item 3",
            "Desk: 1,200.00",
            "After"
        ]
    );
    let out = render(Format::Docx, &doc, json!({"items": []}));
    assert_eq!(docx_text(&out), ["Before", "After"]);
}

#[test]
fn docx_inline_loop() {
    let doc = docx(&(p("Names: {% for i in items %}{{ i.name }}{% if not loop.last %}, {% endif %}{% endfor %}.")
        + &p("Split: {% for i in items %}§{{ i.name }} §{% endfor %}!")));
    let out = render(Format::Docx, &doc, items());
    let text = docx_text(&out);
    assert_eq!(text[0], "Names: Lamp, Plug, Desk.");
    assert_eq!(text[1], "Split: Lamp Plug Desk !");
    let xml = part(&out, "word/document.xml");
    assert_eq!(xml.matches("<w:p>").count(), 2);
}

#[test]
fn nested_loops() {
    // A row loop inside a paragraph loop, and an inline loop inside the row.
    let doc = docx(
        &(p("{% for o in orders %}Order {{ o.id }}")
            + &tbl(&[&[
                "{% for l in o.lines %}{{ l.sku }}",
                "{% for t in l.tags %}[{{ t }}]{% endfor %}{% endfor %}",
            ]])
            + &p("End of {{ o.id }}{% endfor %}")),
    );
    let data = json!({"orders": [
        {"id": "A", "lines": [{"sku": "x", "tags": ["1", "2"]}, {"sku": "y", "tags": []}]},
        {"id": "B", "lines": []}
    ]});
    let out = render(Format::Docx, &doc, data);
    let text = docx_text(&out);
    assert_eq!(
        text,
        ["Order A", "x", "[1][2]", "y", "", "End of A", "Order B", "End of B"]
    );
    assert_eq!(
        part(&out, "word/document.xml").matches("<w:tbl>").count(),
        2
    );
}

#[test]
fn if_else_inline_and_on_rows() {
    let doc = docx(
        &(p("{% if customer.vip %}VIP{% else %}Regular{% endif %} customer")
            + &tbl(&[
                &["{% if customer.vip %}Gold row", ""],
                &["{% else %}Plain row", ""],
                &["{% endif %}Also plain", ""],
                &["Always", ""],
            ])),
    );
    let out = render(Format::Docx, &doc, items());
    let text = docx_text(&out);
    assert_eq!(text, ["VIP customer", "Gold row", "", "Always", ""]);
    let out = render(Format::Docx, &doc, json!({"customer": {"vip": false}}));
    let text = docx_text(&out);
    assert_eq!(
        text,
        [
            "Regular customer",
            "Plain row",
            "",
            "Also plain",
            "",
            "Always",
            ""
        ]
    );
}

#[test]
fn settings_tag_is_removed_and_applies() {
    let doc = docx(&(p("{% settings lang=\"fr\" currency=\"EUR\" timezone=\"+02:00\" t.greeting=\"Bonjour\" t.bye.fr=\"Au revoir\" %}Start")
        + &p("{{ \"greeting\" | t }} {{ \"bye\" | t }} {{ \"other\" | t }} {{ 5 | currency }} {{ \"2024-01-01T23:00:00Z\" | date(\"%d/%m/%Y %H:%M\") }}")));
    let compiled = compile(Format::Docx, &doc).unwrap();
    assert_eq!(compiled.settings().lang.as_deref(), Some("fr"));
    assert_eq!(compiled.settings().currency.as_deref(), Some("EUR"));
    assert_eq!(compiled.tags().len(), 6);
    let out = compiled.render(&json!({})).unwrap();
    let text = docx_text(&out);
    assert_eq!(text[0], "Start");
    assert_eq!(text[1], "Bonjour Au revoir other EUR 5.00 02/01/2024 01:00");
}

#[test]
fn date_currency_and_t_filters() {
    let doc = docx(&p("{{ d | date(\"%d/%m/%Y\") }} {{ 86400 | date }} {{ 1234.5 | currency(\"USD\") }} {{ \"k\" | t }}"));
    let out = render(Format::Docx, &doc, json!({"d": "2024-03-05"}));
    assert_eq!(docx_text(&out)[0], "05/03/2024 1970-01-02 USD 1,234.50 k");
}

#[test]
fn undefined_variable_error_carries_a_location() {
    let doc = docx(&(p("fine") + &tbl(&[&["x", "{{ customer.city }}"]]) + &p("{{ nobody }}")));
    let err = compile(Format::Docx, &doc)
        .unwrap()
        .render(&items())
        .unwrap_err();
    let loc = err.location.clone().expect("location");
    assert_eq!(loc.part, "word/document.xml");
    assert_eq!(loc.place, "table 1 row 1 paragraph 3");
    assert_eq!(loc.snippet, "{{ customer.city }}");
    assert!(err.message.contains("undefined"), "{err}");

    let doc = docx(&(p("fine") + &p("{{ nobody.home }}")));
    let err = compile(Format::Docx, &doc)
        .unwrap()
        .render(&items())
        .unwrap_err();
    assert_eq!(err.location.unwrap().place, "paragraph 2");
}

#[test]
fn validate_reports_several_problems_at_once() {
    let doc = docx(
        &(p("{% for i in items %}{{ i.name }}")
            + &p("{{ customer.phone }} {{ 1 + }}")
            + &p("{% for x in customer.name %}{{ x }}{% endfor %}")
            + &p("{% endif %}")),
    );
    let diags = validate(Format::Docx, &doc, &items());
    let messages: Vec<String> = diags
        .iter()
        .map(|d| format!("{:?} {}", d.severity, d.message))
        .collect();
    let has = |needle: &str| messages.iter().any(|m| m.contains(needle));
    assert!(has("is never closed"), "{messages:?}");
    assert!(has("has no matching"), "{messages:?}");
    assert!(has("cannot parse tag"), "{messages:?}");
    assert!(diags.len() >= 3, "{messages:?}");

    // Without syntax problems the test render reports undefined values and odd loops.
    let doc = docx(
        &(p("{{ customer.phone }}")
            + &p("{{ customer.name }} {{ ctx.nothing }}")
            + &p("{% for x in customer.name %}{{ x }}{% endfor %}")),
    );
    let diags = validate(Format::Docx, &doc, &items());
    let messages: Vec<String> = diags
        .iter()
        .map(|d| format!("{:?} {}", d.severity, d.message))
        .collect();
    assert!(
        messages.iter().filter(|m| m.contains("undefined")).count() >= 2,
        "{messages:?}"
    );
    assert!(
        diags
            .iter()
            .any(|d| d.severity == Severity::Warning && d.message.contains("rather than an array")),
        "{messages:?}"
    );
    let loc = diags
        .iter()
        .find(|d| d.message.contains("undefined"))
        .unwrap()
        .location
        .clone()
        .unwrap();
    assert_eq!(loc.place, "paragraph 1");
    assert!(validate(Format::Docx, &docx(&p("{{ customer.name }}")), &items()).is_empty());
}

#[test]
fn misplaced_block_tags_are_reported_clearly() {
    let doc = docx_with(&p("{{ x }}{% for i in items %}"), "{% endfor %}");
    let err = compile(Format::Docx, &doc).unwrap_err();
    assert!(err.message.contains("different parts"), "{err}");
    assert!(
        err.message.contains("word/header1.xml") && err.message.contains("word/document.xml"),
        "{err}"
    );

    let doc = docx(&tbl(&[&["{% if a %}x", "{% else %}y", "{% endif %}"]]));
    let err = compile(Format::Docx, &doc).unwrap_err();
    assert!(err.message.contains("needs its own row"), "{err}");
}

#[test]
fn untouched_entries_are_byte_identical() {
    let doc = docx(&p("Hello {{ name }}"));
    let out = render(Format::Docx, &doc, json!({"name": "x"}));
    let before = entries(&doc);
    let after = entries(&out);
    assert_eq!(
        before.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
        after.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>()
    );
    for ((name, a), (_, b)) in before.iter().zip(&after) {
        if name == "word/document.xml" {
            assert_ne!(a, b);
        } else {
            assert_eq!(a, b, "{name} changed");
        }
    }
}

#[test]
fn compile_hash_and_tags() {
    let doc = docx(&p("{{ a }} {% if b %}{{ c }}{% endif %}"));
    let compiled = compile(Format::Docx, &doc).unwrap();
    assert_eq!(compiled.hash().len(), 64);
    assert_eq!(
        compiled.tags(),
        ["{{ a }}", "{% if b %}", "{{ c }}", "{% endif %}"]
    );
    assert_eq!(compiled.format(), Format::Docx);
    assert!(compile(Format::Docx, b"not a zip").is_err());
}

// --- ODT ------------------------------------------------------------------

#[test]
fn odt_substitution_spans_and_loops() {
    let body = concat!(
        "<text:p text:style-name=\"P1\">Dear <text:span text:style-name=\"T1\">{{ customer.</text:span>name<text:s/>}},</text:p>",
        "<text:h text:outline-level=\"1\">{% for i in items %}{{ i.name }}</text:h>",
        "<text:p>{{ i.qty }}<text:tab/>{{ i.price | currency(\"EUR\") }}{% endfor %}</text:p>",
        "<table:table table:name=\"T\"><table:table-column/><table:table-row><table:table-cell><text:p>{% for i in items %}{{ i.name }}{% endfor %}</text:p></table:table-cell></table:table-row></table:table>",
        "<text:p>Bye {{ \"x\\ny\" }}</text:p>"
    );
    let doc = odt(body);
    let out = render(Format::Odt, &doc, items());
    let xml = part(&out, "content.xml");
    assert!(xml.starts_with("<?xml"));
    let text = odf_paragraphs(&xml);
    assert_eq!(
        text,
        [
            "Dear Ada & Co,",
            "Lamp",
            "2\tEUR 19.50",
            "Plug",
            "1\tEUR 0.50",
            "Desk",
            "1\tEUR 1,200.00",
            "LampPlugDesk",
            "Bye x\ny"
        ]
    );
    assert_eq!(xml.matches("<text:h ").count(), 3);
    assert_eq!(xml.matches("<table:table-row>").count(), 1);
    let out_entries = entries(&out);
    assert_eq!(out_entries[0].0, "mimetype");
}

// --- XLSX -----------------------------------------------------------------

fn sales_sheet() -> Vec<u8> {
    xlsx(
        &[
            row(1, &[cell("A1", "s", "0"), cell("B1", "s", "1")]),
            row(
                2,
                &[
                    cell("A2", "s", "2"),
                    cell(
                        "B2",
                        "inlineStr",
                        "{{ customer.name }} ({{ items | length }})",
                    ),
                ],
            ),
            row(
                4,
                &[
                    cell("A4", "s", "3"),
                    cell("B4", "s", "4"),
                    cell("C4", "s", "5"),
                    cell("D4", "f", "B4*C4"),
                ],
            ),
            row(
                6,
                &[
                    cell("A6", "inlineStr", "Total"),
                    cell("D6", "f", "SUM(D4:D4)"),
                    cell("B6", "f", "COUNT(B4:B4)"),
                ],
            ),
            row(7, &[cell("A7", "s", "7")]),
        ],
        &[
            "Invoice",
            "{{ ctx.company }}",
            "Customer",
            "<r><rPr><b/></rPr><t>{% for i in </t></r><r><t>items %}{{ i.name }}</t></r>",
            "{{ i.qty }}",
            "{{ i.price }}{% endfor %}",
            "unused",
            "{{ customer.vip }}",
        ],
    )
}

#[test]
fn xlsx_substitution_and_row_loop_with_formulas() {
    let doc = sales_sheet();
    let compiled = compile(Format::Xlsx, &doc).unwrap();
    assert_eq!(compiled.tags()[0], "{{ ctx.company }}");

    let out = compiled.render(&items()).unwrap();
    let sheet = part(&out, "xl/worksheets/sheet1.xml");
    let c = cells(&sheet);
    let get = |r: &str| {
        c.iter()
            .find(|(k, _)| k == r)
            .map(|(_, v)| v.as_str())
            .unwrap_or("<none>")
    };
    assert_eq!(get("B1"), "Crafted");
    assert_eq!(get("B2"), "Ada & Co (3)");
    assert_eq!(get("A4"), "Lamp");
    assert_eq!(get("B4"), "2");
    assert_eq!(get("C4"), "19.5");
    assert_eq!(get("D4"), "=B4*C4");
    assert_eq!(get("A5"), "Plug");
    assert_eq!(get("D5"), "=B5*C5");
    assert_eq!(get("A6"), "Desk");
    assert_eq!(get("D6"), "=B6*C6");
    assert_eq!(get("A8"), "Total");
    assert_eq!(get("D8"), "=SUM(D4:D6)");
    assert_eq!(get("B8"), "=COUNT(B4:B6)");
    assert_eq!(get("A9"), "bool:1");
    assert!(
        sheet.contains("<c r=\"A9\" t=\"b\"><v>1</v></c>"),
        "{sheet}"
    );
    assert!(
        sheet.contains("<row r=\"8\">") && !sheet.contains("<row r=\"7\">"),
        "{sheet}"
    );
    assert!(
        sheet.contains("<c r=\"B4\"><v>2</v></c>"),
        "numbers stay numbers: {sheet}"
    );
    assert!(sheet.contains("<dimension ref=\"A1:D9\"/>"), "{sheet}");
    assert!(part(&out, "xl/workbook.xml").contains("fullCalcOnLoad=\"1\""));
    assert_eq!(
        part(&out, "xl/sharedStrings.xml"),
        part(&doc, "xl/sharedStrings.xml")
    );

    let out = compiled.render(&json!({"customer": {"name": "N", "vip": false}, "items": [{"name": "One", "qty": 1, "price": 2}], "ctx": {"company": "C"}})).unwrap();
    let c = cells(&part(&out, "xl/worksheets/sheet1.xml"));
    let get = |r: &str| {
        c.iter()
            .find(|(k, _)| k == r)
            .map(|(_, v)| v.as_str())
            .unwrap_or("<none>")
    };
    assert_eq!(get("A4"), "One");
    assert_eq!(get("A5"), "<none>");
    assert_eq!(get("D6"), "=SUM(D4:D4)");
    assert_eq!(get("A7"), "bool:0");

    let out = compiled
        .render(
            &json!({"customer": {"name": "N", "vip": null}, "items": [], "ctx": {"company": "C"}}),
        )
        .unwrap();
    let sheet = part(&out, "xl/worksheets/sheet1.xml");
    let c = cells(&sheet);
    let get = |r: &str| {
        c.iter()
            .find(|(k, _)| k == r)
            .map(|(_, v)| v.as_str())
            .unwrap_or("<none>")
    };
    assert_eq!(get("A4"), "<none>");
    assert_eq!(get("A5"), "Total");
    assert_eq!(get("D5"), "=SUM(D4:D3)");
    assert!(
        sheet.contains("<c r=\"A6\"/>"),
        "null gives an empty cell: {sheet}"
    );
}

#[test]
fn xlsx_errors_name_the_sheet_and_row() {
    let doc = xlsx(&[row(3, &[cell("B3", "inlineStr", "{{ nope }}")])], &[]);
    let err = compile(Format::Xlsx, &doc)
        .unwrap()
        .render(&json!({}))
        .unwrap_err();
    let loc = err.location.unwrap();
    assert_eq!(loc.part, "xl/worksheets/sheet1.xml");
    assert_eq!(loc.place, "sheet \"Sales\" row 3 (cell B3)");
}

// --- PPTX -----------------------------------------------------------------

#[test]
fn pptx_substitution_table_rows_and_slide_loop() {
    let doc = pptx(&[
        &shape(&["{{ ctx.company }} for {{ cust§omer.name }}"]),
        &(shape(&["{% for i in items %}{{ i.name }}"]) + &ptbl(&[&["Qty", "{{ i.qty }}"]])),
        &shape(&["Price {{ i.price | currency(\"EUR\") }}{% endfor %}"]),
        &(shape(&["Summary"])
            + &ptbl(&[&[
                "{% for i in items %}{{ i.name }}",
                "{{ i.qty }}{% endfor %}",
            ]])),
    ]);
    let compiled = compile(Format::Pptx, &doc).unwrap();
    let out = compiled.render(&items()).unwrap();
    let order = slide_order(&out);
    assert_eq!(order.len(), 8, "{order:?}");
    assert_eq!(order[0], "ppt/slides/slide1.xml");
    assert_eq!(order[1], "ppt/slides/slide2.xml");
    assert_eq!(order[2], "ppt/slides/slide3.xml");
    assert_eq!(order[7], "ppt/slides/slide4.xml");
    assert_eq!(slide_text(&out, &order[0])[0], "Crafted for Ada & Co");
    assert_eq!(slide_text(&out, &order[1])[0], "Lamp");
    assert_eq!(slide_text(&out, &order[2])[0], "Price EUR 19.50");
    assert_eq!(slide_text(&out, &order[3])[0], "Plug");
    assert_eq!(slide_text(&out, &order[5])[0], "Desk");
    assert_eq!(slide_text(&out, &order[6])[0], "Price EUR 1,200.00");
    let summary = part(&out, "ppt/slides/slide4.xml");
    assert_eq!(summary.matches("<a:tr ").count(), 3);
    for copy in &order[3..7] {
        let rels_name = copy.replace("slides/", "slides/_rels/") + ".rels";
        assert!(
            part(&out, &rels_name).contains("slideLayout"),
            "copied rels for {copy}"
        );
    }
    let ct = part(&out, "[Content_Types].xml");
    assert_eq!(ct.matches("presentationml.slide+xml").count(), 8);
    let pres = part(&out, "ppt/presentation.xml");
    assert_eq!(pres.matches("<p:sldId ").count(), 8);

    let out = compiled
        .render(&json!({"customer": {"name": "N"}, "items": [], "ctx": {"company": "C"}}))
        .unwrap();
    let order = slide_order(&out);
    assert_eq!(order, ["ppt/slides/slide1.xml", "ppt/slides/slide4.xml"]);
    assert!(entries(&out)
        .iter()
        .all(|(n, _)| !n.contains("slide2") && !n.contains("slide3")));
    assert_eq!(
        part(&out, "[Content_Types].xml")
            .matches("presentationml.slide+xml")
            .count(),
        2
    );
    assert!(!part(&out, "ppt/_rels/presentation.xml.rels").contains("slide2.xml"));
}

#[test]
fn pptx_errors_name_the_slide() {
    let doc = pptx(&[&shape(&["ok"]), &shape(&["a", "{{ nope }}"])]);
    let err = compile(Format::Pptx, &doc)
        .unwrap()
        .render(&json!({}))
        .unwrap_err();
    let loc = err.location.unwrap();
    assert_eq!(loc.part, "ppt/slides/slide2.xml");
    assert_eq!(loc.place, "slide 2 paragraph 2");
}

// --- ODS / ODP ------------------------------------------------------------

#[test]
fn ods_row_loop_and_odp_slide_loop() {
    let body = "<table:table table:name=\"Sheet1\"><table:table-column/><table:table-row><table:table-cell office:value-type=\"string\"><text:p>{% for i in items %}{{ i.name }}</text:p></table:table-cell><table:table-cell office:value-type=\"string\"><text:p>{{ i.qty }}{% endfor %}</text:p></table:table-cell></table:table-row></table:table>";
    let out = render(Format::Ods, &ods(body), items());
    let xml = part(&out, "content.xml");
    assert_eq!(xml.matches("<table:table-row>").count(), 3);
    assert_eq!(
        odf_paragraphs(&xml),
        ["Lamp", "2", "Plug", "1", "Desk", "1"]
    );

    let body = "<draw:page draw:name=\"page1\"><draw:frame><draw:text-box><text:p>{% for i in items %}{{ i.name }}</text:p></draw:text-box></draw:frame></draw:page><draw:page draw:name=\"page2\"><draw:frame><draw:text-box><text:p>{{ i.qty }}{% endfor %}</text:p></draw:text-box></draw:frame></draw:page><draw:page draw:name=\"page3\"><draw:frame><draw:text-box><text:p>{{ nope }}</text:p></draw:text-box></draw:frame></draw:page>";
    let doc = odp(body);
    let err = compile(Format::Odp, &doc)
        .unwrap()
        .render(&items())
        .unwrap_err();
    assert_eq!(err.location.unwrap().place, "slide 3 paragraph 3");
    let mut data = items();
    data["nope"] = json!("end");
    let out = render(Format::Odp, &doc, data);
    let xml = part(&out, "content.xml");
    assert_eq!(xml.matches("<draw:page ").count(), 7);
    assert_eq!(
        odf_paragraphs(&xml),
        ["Lamp", "2", "Plug", "1", "Desk", "1", "end"]
    );
}

// --- Files LibreOffice wrote (see tests/fixtures/gen.sh) --------------------

const REAL_DOCX: &[u8] = include_bytes!("fixtures/invoice.docx");
const REAL_XLSX: &[u8] = include_bytes!("fixtures/invoice.xlsx");

fn invoice_data() -> Value {
    json!({
        "customer": {"name": "Ada Lovelace", "vip": true},
        "issued": "2024-03-05",
        "items": [
            {"name": "Lamp", "qty": 2, "price": 19.5},
            {"name": "Plug", "qty": 1, "price": 0.5},
            {"name": "Desk", "qty": 1, "price": 1200.0}
        ]
    })
}

#[test]
fn libreoffice_docx_renders() {
    let compiled = compile(Format::Docx, REAL_DOCX).unwrap();
    assert_eq!(compiled.settings().currency.as_deref(), Some("EUR"));
    let out = compiled.render(&invoice_data()).unwrap();
    let text = docx_text(&out);
    assert_eq!(text[0], "Invoice for Ada Lovelace");
    assert_eq!(text[1], "Issued 05 March 2024");
    assert_eq!(&text[2..5], ["Item", "Qty", "Price"]);
    assert_eq!(
        &text[5..14],
        [
            "Lamp",
            "2",
            "EUR 19.50",
            "Plug",
            "1",
            "EUR 0.50",
            "Desk",
            "1",
            "EUR 1,200.00"
        ]
    );
    assert_eq!(text[14], "Thank you for your loyalty.");
    assert_eq!(text[15], "Total EUR 1,220.00");
    let before = entries(REAL_DOCX);
    let after = entries(&out);
    for ((name, a), (_, b)) in before.iter().zip(&after) {
        if name != "word/document.xml" {
            assert_eq!(a, b, "{name} changed");
        }
    }
}

#[test]
fn libreoffice_xlsx_renders_with_rebased_formulas() {
    let compiled = compile(Format::Xlsx, REAL_XLSX).unwrap();
    let out = compiled.render(&invoice_data()).unwrap();
    let sheet = part(&out, "xl/worksheets/sheet1.xml");
    let c = cells_resolved(&out);
    let get = |r: &str| {
        c.iter()
            .find(|(k, _)| k == r)
            .map(|(_, v)| v.as_str())
            .unwrap_or("<none>")
    };
    assert_eq!(get("B1"), "Ada Lovelace");
    assert_eq!(get("A3"), "Lamp");
    assert_eq!(get("B3"), "2");
    assert_eq!(get("C3"), "19.5");
    assert_eq!(get("D3"), "=B3*C3");
    assert_eq!(get("A4"), "Plug");
    assert_eq!(get("D4"), "=B4*C4");
    assert_eq!(get("A5"), "Desk");
    assert_eq!(get("D5"), "=B5*C5");
    assert_eq!(get("A6"), "Total");
    assert_eq!(get("D6"), "=SUM(D5)");
    assert!(
        !sheet.contains("t=\"e\""),
        "error cells lose their stale type: {sheet}"
    );
    assert!(part(&out, "xl/workbook.xml").contains("fullCalcOnLoad=\"1\""));
}

// --- LibreOffice ----------------------------------------------------------

/// Renders a DOCX and an XLSX, converts them to PDF with LibreOffice in the
/// office container and checks the text, which for the sheet proves the
/// re-based formulas recalculate. Skips when the images are not present.
#[test]
fn rendered_files_convert_to_pdf_with_the_expected_text() {
    if !libreoffice_available() {
        eprintln!("skipping: docker images not available");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let dir_s = dir.path().to_str().unwrap();

    let out = render(Format::Docx, &row_loop_doc(), items());
    std::fs::write(dir.path().join("out.docx"), &out).unwrap();
    assert!(soffice(dir_s, "pdf", "out.docx"), "DOCX conversion failed");
    let text = pdf_text(dir_s, "out.pdf");
    for expected in ["Items:", "Lamp", "Plug", "Desk", "Total", "End"] {
        assert!(text.contains(expected), "missing {expected} in {text}");
    }
    assert_eq!(text.matches("Desk").count(), 1);

    let out = compile(Format::Docx, REAL_DOCX)
        .unwrap()
        .render(&invoice_data())
        .unwrap();
    std::fs::write(dir.path().join("invoice.docx"), &out).unwrap();
    assert!(
        soffice(dir_s, "pdf", "invoice.docx"),
        "real DOCX conversion failed"
    );
    let text = pdf_text(dir_s, "invoice.pdf");
    for expected in [
        "Invoice for Ada Lovelace",
        "Issued 05 March 2024",
        "EUR 1,200.00",
        "loyalty",
        "Total EUR 1,220.00",
    ] {
        assert!(text.contains(expected), "missing {expected} in {text}");
    }

    let out = compile(Format::Xlsx, REAL_XLSX)
        .unwrap()
        .render(&invoice_data())
        .unwrap();
    std::fs::write(dir.path().join("invoice.xlsx"), &out).unwrap();
    assert!(
        soffice(dir_s, "pdf", "invoice.xlsx"),
        "XLSX conversion failed"
    );
    let text = pdf_text(dir_s, "invoice.pdf");
    for expected in ["Ada Lovelace", "Lamp", "39", "0.5", "1200", "Total"] {
        assert!(text.contains(expected), "missing {expected} in {text}");
    }
}

#[test]
fn validation_keeps_going_past_filters_on_missing_values() {
    let doc = docx(
        &(p("{{ issued | date(\"%d\") }}")
            + &p("{{ total | currency(\"EUR\") }}")
            + &p("{{ share | round(2) }}")
            + &p("{{ who }}")),
    );
    let found = validate(Format::Docx, &doc, &json!({}));
    let errors: Vec<_> = found
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert_eq!(errors.len(), 4, "{found:?}");
}
