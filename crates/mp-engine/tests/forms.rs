use std::path::PathBuf;

use mp_engine::{Engine, FieldEdit, FieldKind, Xfa};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

#[test]
fn fills_in_toggles_and_resets_fields() {
    let engine = Engine::start();
    let doc = engine.open(fixture("form.pdf")).unwrap().id;
    let fields = engine.fields(doc, 0).unwrap();
    let kinds: Vec<_> = fields.iter().map(|f| (f.kind, f.name.as_str())).collect();
    assert_eq!(
        kinds,
        [
            (FieldKind::Text, "name"),
            (FieldKind::Checkbox, "agree"),
            (FieldKind::Choice, "colour")
        ]
    );
    let (name, agree, colour) = (fields[0].id, fields[1].id, fields[2].id);
    assert_eq!(fields[0].value, "");
    assert!(!fields[0].read_only);
    assert_eq!(fields[2].options, ["Red", "Green", "Blue"]);
    assert_eq!(fields[2].value, "Green");

    engine
        .edit_field(doc, 0, name, FieldEdit::Value("Ada Lovelace".into()))
        .unwrap();
    engine
        .edit_field(doc, 0, colour, FieldEdit::Value("Blue".into()))
        .unwrap();
    engine.edit_field(doc, 0, agree, FieldEdit::Toggle).unwrap();
    let fields = engine.fields(doc, 0).unwrap();
    assert_eq!(fields[0].value, "Ada Lovelace");
    assert_eq!(fields[1].value, "Yes");
    assert_eq!(fields[2].value, "Blue");
    // The filled-in text is drawn.
    let list = engine.display_list(doc, 0).unwrap();
    let text = mp_engine::page_text(&list).unwrap();
    assert!(text.text(0..text.chars.len()).contains("Ada Lovelace"));

    assert_eq!(
        engine.history(doc).unwrap().undo.as_deref(),
        Some("Check box")
    );
    engine.undo(doc).unwrap();
    assert_eq!(engine.fields(doc, 0).unwrap()[1].value, "Off");

    engine.reset_form(doc).unwrap();
    let fields = engine.fields(doc, 0).unwrap();
    assert_eq!(fields[0].value, "");
    assert_eq!(fields[1].value, "Off");
}

#[test]
fn flattening_keeps_the_look_and_drops_the_fields() {
    let engine = Engine::start();
    let doc = engine.open(fixture("form.pdf")).unwrap().id;
    let name = engine.fields(doc, 0).unwrap()[0].id;
    engine
        .edit_field(doc, 0, name, FieldEdit::Value("Grace Hopper".into()))
        .unwrap();
    let note = mp_engine::NewAnnot::Note {
        x: 400.0,
        y: 100.0,
        text: "First".into(),
    };
    let style = mp_engine::Style {
        color: [1.0, 0.85, 0.0],
        author: String::new(),
    };
    let added = engine.add_annotation(doc, 0, note, style).unwrap();
    engine
        .set_contents(doc, 0, added.id, "Second".into())
        .unwrap();
    assert_eq!(engine.annotations(doc, 0).unwrap()[0].contents, "Second");

    engine.flatten(doc, false, true).unwrap();
    assert!(engine.fields(doc, 0).unwrap().is_empty());
    assert_eq!(engine.annotations(doc, 0).unwrap().len(), 1);
    let list = engine.display_list(doc, 0).unwrap();
    let text = mp_engine::page_text(&list).unwrap();
    assert!(text.text(0..text.chars.len()).contains("Grace Hopper"));

    engine.flatten(doc, true, false).unwrap();
    assert!(engine.annotations(doc, 0).unwrap().is_empty());
    engine.undo(doc).unwrap();
    assert_eq!(engine.annotations(doc, 0).unwrap().len(), 1);
}

#[test]
fn form_data_round_trips_through_xfdf() {
    let engine = Engine::start();
    let doc = engine.open(fixture("form.pdf")).unwrap().id;
    let fields = engine.fields(doc, 0).unwrap();
    engine
        .edit_field(doc, 0, fields[0].id, FieldEdit::Value("A & <B>".into()))
        .unwrap();
    engine
        .edit_field(doc, 0, fields[1].id, FieldEdit::Toggle)
        .unwrap();

    let xfdf = engine.export_xfdf(doc, "form.pdf".into()).unwrap();
    assert!(xfdf.contains(r#"<f href="form.pdf"/>"#), "{xfdf}");
    assert!(xfdf.contains(r#"<field name="name"><value>A &amp; &lt;B&gt;</value></field>"#));
    assert!(xfdf.contains(r#"<field name="agree"><value>Yes</value></field>"#));

    engine.reset_form(doc).unwrap();
    assert_eq!(engine.import_xfdf(doc, xfdf).unwrap(), 3);
    let fields = engine.fields(doc, 0).unwrap();
    assert_eq!(fields[0].value, "A & <B>");
    assert_eq!(fields[1].value, "Yes");
    assert_eq!(
        engine.history(doc).unwrap().undo.as_deref(),
        Some("Import form data")
    );

    // Nested XFDF fields name their children relative to the parent.
    let nested = r#"<xfdf xmlns="http://ns.adobe.com/xfdf/"><fields>
        <field name="name"><value>Nested</value></field>
        <field name="group"><field name="unknown"><value>x</value></field></field>
        </fields></xfdf>"#;
    assert_eq!(engine.import_xfdf(doc, nested.into()).unwrap(), 1);
    assert_eq!(engine.fields(doc, 0).unwrap()[0].value, "Nested");
    assert!(engine.import_xfdf(doc, "not xml".into()).is_err());
}

fn field<'a>(fields: &'a [mp_engine::Field], name: &str) -> &'a mp_engine::Field {
    fields.iter().find(|f| f.name == name).unwrap()
}

#[test]
fn form_javascript_calculates_formats_and_validates() {
    let engine = Engine::start();
    let doc = engine.open(fixture("calc.pdf")).unwrap().id;
    let fields = engine.fields(doc, 0).unwrap();
    let (a, b) = (field(&fields, "a").id, field(&fields, "b").id);
    engine
        .edit_field(doc, 0, a, FieldEdit::Value("2".into()))
        .unwrap();
    engine
        .edit_field(doc, 0, b, FieldEdit::Value("3.5".into()))
        .unwrap();
    let fields = engine.fields(doc, 0).unwrap();
    assert_eq!(field(&fields, "total").value, "5.5");
    // The format action shows two decimals.
    let list = engine.display_list(doc, 0).unwrap();
    let text = mp_engine::page_text(&list).unwrap();
    assert!(text.text(0..text.chars.len()).contains("5.50"));

    // a rejects values over 100 and keeps its old value.
    let rejected = engine.edit_field(doc, 0, a, FieldEdit::Value("200".into()));
    assert!(rejected.is_err());
    let fields = engine.fields(doc, 0).unwrap();
    assert_eq!(field(&fields, "a").value, "2");
    assert_eq!(field(&fields, "total").value, "5.5");
}

#[test]
fn xfa_forms_are_detected_and_dropped_on_fill() {
    let engine = Engine::start();
    let plain = engine.open(fixture("form.pdf")).unwrap().id;
    assert_eq!(engine.xfa(plain).unwrap(), Xfa::None);
    let dynamic = engine.open(fixture("xfa-dynamic.pdf")).unwrap().id;
    assert_eq!(engine.xfa(dynamic).unwrap(), Xfa::Dynamic);

    let doc = engine.open(fixture("xfa-static.pdf")).unwrap().id;
    assert_eq!(engine.xfa(doc).unwrap(), Xfa::Static);
    let name = engine.fields(doc, 0).unwrap()[0].id;
    engine
        .edit_field(doc, 0, name, FieldEdit::Value("Ada".into()))
        .unwrap();
    // Filling in removes the XFA packet, so every reader shows the AcroForm value.
    assert_eq!(engine.xfa(doc).unwrap(), Xfa::None);
    engine.undo(doc).unwrap();
    assert_eq!(engine.xfa(doc).unwrap(), Xfa::Static);
}
