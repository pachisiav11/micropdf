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
