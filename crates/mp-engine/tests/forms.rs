use std::path::PathBuf;

use mp_engine::{Engine, FieldAction, FieldEdit, FieldKind, NewField, Rect, Xfa};

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
    let fields = engine.fields(doc, 0).unwrap();
    engine
        .edit_field(
            doc,
            0,
            fields[0].id,
            FieldEdit::Value("Grace Hopper".into()),
        )
        .unwrap();
    engine
        .edit_field(doc, 0, fields[1].id, FieldEdit::Toggle)
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
    let text = text.text(0..text.chars.len());
    assert!(text.contains("Grace Hopper"));
    // The check mark keeps its ZapfDingbats font; in Helvetica it would be a "4".
    assert!(!text.contains('4'), "{text:?}");

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

#[test]
fn form_data_round_trips_through_fdf() {
    let engine = Engine::start();
    let doc = engine.open(fixture("form.pdf")).unwrap().id;
    let fields = engine.fields(doc, 0).unwrap();
    let name = "Grace (n\u{e9}e) \\ Hopper \u{2014} \u{fc}";
    engine
        .edit_field(doc, 0, fields[0].id, FieldEdit::Value(name.into()))
        .unwrap();
    engine
        .edit_field(doc, 0, fields[1].id, FieldEdit::Toggle)
        .unwrap();

    let fdf = engine.export_fdf(doc, "form (1).pdf".into()).unwrap();
    let text = String::from_utf8_lossy(&fdf);
    assert!(text.starts_with("%FDF-1.2"), "{text}");
    assert!(text.contains("/F (form \\(1\\).pdf)"), "{text}");
    assert!(text.contains("<< /T (agree) /V /Yes >>"), "{text}");
    assert!(text.contains("<< /T (colour) /V (Green) >>"), "{text}");

    engine.reset_form(doc).unwrap();
    assert_eq!(engine.import_fdf(doc, fdf).unwrap(), 3);
    let fields = engine.fields(doc, 0).unwrap();
    assert_eq!(fields[0].value, name);
    assert_eq!(fields[1].value, "Yes");
    assert_eq!(fields[2].value, "Green");
    // A checkbox's value is the name of its on state, not the text "Yes".
    let saved = std::env::temp_dir().join(format!("mp-engine-{}-fdf.pdf", std::process::id()));
    engine.save(doc, &saved, false).unwrap();
    let bytes = std::fs::read(&saved).unwrap();
    let _ = std::fs::remove_file(&saved);
    let has = |s: &[u8]| bytes.windows(s.len()).any(|w| w == s);
    assert!(has(b"/V/Yes") || has(b"/V /Yes"));
    assert!(!has(b"/V(Yes)") && !has(b"/V (Yes)"));
    assert_eq!(
        engine.history(doc).unwrap().undo.as_deref(),
        Some("Import form data")
    );

    // Kids name their fields relative to the parent.
    let nested = b"%FDF-1.2\n1 0 obj\n<< /FDF << /Fields [<< /T (name) /V (Nested) >> \
        << /T (group) /Kids [<< /T (unknown) /V (x) >>] >>] >> >>\nendobj\n\
        trailer\n<< /Root 1 0 R >>\n%EOF\n";
    assert_eq!(engine.import_fdf(doc, nested.to_vec()).unwrap(), 1);
    assert_eq!(engine.fields(doc, 0).unwrap()[0].value, "Nested");
    assert!(engine.import_fdf(doc, b"not fdf".to_vec()).is_err());
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

#[test]
fn prepare_form_finds_blanks_adds_fields_and_sets_their_properties() {
    let engine = Engine::start();
    let dir = std::env::temp_dir().join(format!("mp-prepare-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("blank-form.pdf");
    std::fs::copy(fixture("blank-form.pdf"), &path).unwrap();
    let doc = engine.open(&path).unwrap().id;
    let count = |kind| {
        let fields = engine.fields(doc, 0).unwrap();
        fields.iter().filter(|f| f.kind == kind).count()
    };

    // The underscores, the three empty cells and the square.
    assert_eq!(engine.detect_fields(doc, vec![0]).unwrap(), 5);
    assert_eq!((count(FieldKind::Text), count(FieldKind::Checkbox)), (4, 1));
    assert_eq!(engine.detect_fields(doc, vec![0]).unwrap(), 0);

    let at = |n: usize| Rect {
        x0: 300.0,
        y0: 300.0 + n as f32 * 40.0,
        x1: 420.0,
        y1: 320.0 + n as f32 * 40.0,
    };
    let kinds = [
        NewField::Text,
        NewField::Checkbox,
        NewField::Radio,
        NewField::Dropdown,
        NewField::List,
        NewField::Button,
        NewField::Signature,
    ];
    let ids: Vec<i32> = kinds
        .iter()
        .enumerate()
        .map(|(n, &k)| engine.add_field(doc, 0, k, at(n), None).unwrap())
        .collect();
    // A second button in the radio group, with a value of its own.
    let second = Rect {
        x0: 440.0,
        x1: 454.0,
        ..at(2)
    };
    let other = engine
        .add_field(doc, 0, NewField::Radio, second, Some(ids[2]))
        .unwrap();
    assert_eq!(engine.field_props(doc, 0, other).unwrap().export, "Choice2");
    let fields = engine.fields(doc, 0).unwrap();
    assert_eq!(fields.len(), 13);
    assert_eq!(
        fields.iter().filter(|f| f.name == "Group1").count(),
        2,
        "{fields:?}"
    );
    for (kind, name) in [
        (FieldKind::Radio, "Group1"),
        (FieldKind::Choice, "Dropdown1"),
        (FieldKind::Choice, "List Box1"),
        (FieldKind::Button, "Button1"),
        (FieldKind::Signature, "Signature1"),
    ] {
        assert!(
            fields.iter().any(|f| f.kind == kind && f.name == name),
            "{kind:?} {name}: {fields:?}"
        );
    }

    let dropdown = ids[3];
    let mut props = engine.field_props(doc, 0, dropdown).unwrap();
    props.name = "Size".into();
    props.tooltip = "Pick a size".into();
    props.required = true;
    props.options = vec!["S".into(), "M".into(), "L".into()];
    engine
        .set_field_props(doc, 0, dropdown, props.clone())
        .unwrap();
    assert_eq!(engine.field_props(doc, 0, dropdown).unwrap(), props);
    engine
        .edit_field(doc, 0, dropdown, FieldEdit::Value("M".into()))
        .unwrap();

    let check = ids[1];
    let mut props = engine.field_props(doc, 0, check).unwrap();
    assert_eq!(props.export, "Yes");
    props.export = "Agreed".into();
    engine.set_field_props(doc, 0, check, props).unwrap();
    engine.edit_field(doc, 0, check, FieldEdit::Toggle).unwrap();
    let radio = ids[2];
    engine.edit_field(doc, 0, radio, FieldEdit::Toggle).unwrap();
    let value = |id| {
        let fields = engine.fields(doc, 0).unwrap();
        fields.into_iter().find(|f| f.id == id).unwrap().value
    };
    assert_eq!(value(check), "Agreed");
    assert_eq!(value(radio), "Choice1");
    engine.edit_field(doc, 0, other, FieldEdit::Toggle).unwrap();
    assert_eq!(value(radio), "Choice2");

    let button = ids[5];
    let mut props = engine.field_props(doc, 0, button).unwrap();
    props.label = "Visit".into();
    props.action = FieldAction::Uri("https://example.com/".into());
    engine
        .set_field_props(doc, 0, button, props.clone())
        .unwrap();
    assert_eq!(engine.field_props(doc, 0, button).unwrap(), props);

    let text = ids[0];
    let mut props = engine.field_props(doc, 0, text).unwrap();
    props.multiline = true;
    props.calculate = "event.value = 1 + 2;".into();
    engine.set_field_props(doc, 0, text, props.clone()).unwrap();
    assert_eq!(engine.field_props(doc, 0, text).unwrap(), props);

    let moved = Rect {
        x0: 450.0,
        y0: 300.0,
        x1: 560.0,
        y1: 330.0,
    };
    engine.move_field(doc, 0, text, moved).unwrap();
    let r = engine
        .fields(doc, 0)
        .unwrap()
        .into_iter()
        .find(|f| f.id == text)
        .unwrap()
        .rect;
    assert!(
        (r.x0 - moved.x0).abs() < 0.5 && (r.y1 - moved.y1).abs() < 0.5,
        "{r:?}"
    );
    engine.delete_field(doc, 0, ids[4]).unwrap();
    assert_eq!(engine.fields(doc, 0).unwrap().len(), 12);

    // Tab goes along the rows, top to bottom.
    engine.order_fields(doc, vec![0], false).unwrap();
    let tops: Vec<f32> = engine
        .fields(doc, 0)
        .unwrap()
        .iter()
        .map(|f| (f.rect.y0 / 6.0).round())
        .collect();
    assert!(tops.is_sorted(), "{tops:?}");

    let saved = dir.join("prepared.pdf");
    engine.save(doc, &saved, false).unwrap();
    let again = engine.open(&saved).unwrap().id;
    let fields = engine.fields(again, 0).unwrap();
    assert_eq!(fields.len(), 12);
    assert!(fields.iter().any(|f| f.name == "Size" && f.value == "M"));
    assert!(
        fields
            .iter()
            .any(|f| f.name == "Check Box2" && f.value == "Agreed")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
