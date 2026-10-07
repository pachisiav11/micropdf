use std::path::PathBuf;

use mp_engine::{Engine, FieldEdit, FieldKind};

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
        [(FieldKind::Text, "name"), (FieldKind::Checkbox, "agree")]
    );
    let (name, agree) = (fields[0].id, fields[1].id);
    assert_eq!(fields[0].value, "");
    assert!(!fields[0].read_only);

    engine
        .edit_field(doc, 0, name, FieldEdit::Value("Ada Lovelace".into()))
        .unwrap();
    engine.edit_field(doc, 0, agree, FieldEdit::Toggle).unwrap();
    let fields = engine.fields(doc, 0).unwrap();
    assert_eq!(fields[0].value, "Ada Lovelace");
    assert_eq!(fields[1].value, "Yes");
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
