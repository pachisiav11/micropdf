use std::path::PathBuf;

use mp_engine::{AnnotKind, Engine, NewAnnot, Rect, Style};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

/// A scratch path in the system temp folder, removed again when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        Scratch(std::env::temp_dir().join(format!("mp-engine-{}-{name}", std::process::id())))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn style() -> Style {
    Style {
        color: [1.0, 0.85, 0.0],
        author: "Tester".into(),
    }
}

/// "Hello micropdf" on hello.pdf (300x200 pt): baseline at y = 110 in page space, 24 pt type.
const TEXT: Rect = Rect {
    x0: 38.0,
    y0: 88.0,
    x1: 210.0,
    y1: 116.0,
};

fn yellow_pixels(engine: &Engine, doc: mp_engine::DocId) -> usize {
    let list = engine.display_list(doc, 0).unwrap();
    let image = mp_engine::render(&list, 1.0).unwrap();
    image
        .rgb
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|p| p[0] > 200 && p[1] > 170 && p[2] < 120)
        .count()
}

#[test]
fn highlight_round_trips_through_full_and_incremental_saves() {
    let engine = Engine::start();
    let doc = engine.open(fixture("hello.pdf")).unwrap().id;
    assert!(engine.annotations(doc, 0).unwrap().is_empty());
    assert_eq!(yellow_pixels(&engine, doc), 0);

    let added = engine
        .add_annotation(
            doc,
            0,
            NewAnnot::TextMarkup {
                kind: AnnotKind::Highlight,
                rects: vec![TEXT],
            },
            style(),
        )
        .unwrap();
    assert_eq!(added.kind, AnnotKind::Highlight);
    assert_eq!(added.author, "Tester");
    assert!(added.rect.x0 <= TEXT.x0 + 1.0 && added.rect.x1 >= TEXT.x1 - 1.0);
    assert_eq!(engine.annotations(doc, 0).unwrap(), vec![added.clone()]);
    // The new display list draws it.
    assert!(yellow_pixels(&engine, doc) > 1000);

    let full = Scratch::new("full.pdf");
    let incremental = Scratch::new("incremental.pdf");
    engine.save(doc, &full.0, false).unwrap();
    engine.save(doc, &incremental.0, true).unwrap();

    let original = std::fs::read(fixture("hello.pdf")).unwrap();
    let appended = std::fs::read(&incremental.0).unwrap();
    assert!(
        appended.starts_with(&original),
        "incremental save keeps the original bytes"
    );
    assert!(appended.len() > original.len());

    for path in [&full.0, &incremental.0] {
        let copy = engine.open(path).unwrap().id;
        let annots = engine.annotations(copy, 0).unwrap();
        assert_eq!(annots.len(), 1, "{}", path.display());
        assert_eq!(annots[0].kind, AnnotKind::Highlight);
        assert_eq!(annots[0].author, "Tester");
        assert!(yellow_pixels(&engine, copy) > 1000);
        engine.close(copy);
    }

    engine.delete_annotation(doc, 0, added.id).unwrap();
    assert!(engine.annotations(doc, 0).unwrap().is_empty());
    assert_eq!(yellow_pixels(&engine, doc), 0);
}

#[test]
fn adds_every_kind_the_toolbar_offers() {
    let engine = Engine::start();
    let doc = engine.open(fixture("outline-links.pdf")).unwrap().id;
    let box_ = Rect {
        x0: 100.0,
        y0: 300.0,
        x1: 250.0,
        y1: 380.0,
    };
    let new = [
        NewAnnot::TextMarkup {
            kind: AnnotKind::Underline,
            rects: vec![box_],
        },
        NewAnnot::TextMarkup {
            kind: AnnotKind::StrikeOut,
            rects: vec![box_],
        },
        NewAnnot::TextMarkup {
            kind: AnnotKind::Squiggly,
            rects: vec![box_],
        },
        NewAnnot::Note {
            x: 400.0,
            y: 100.0,
            text: "Check this".into(),
        },
        NewAnnot::FreeText {
            rect: box_,
            text: "Typed comment".into(),
        },
        NewAnnot::Ink {
            strokes: vec![vec![(100.0, 500.0), (150.0, 520.0), (200.0, 500.0)]],
            width: 2.0,
        },
        NewAnnot::Shape {
            kind: AnnotKind::Square,
            rect: box_,
            width: 1.5,
        },
        NewAnnot::Shape {
            kind: AnnotKind::Circle,
            rect: box_,
            width: 1.5,
        },
        NewAnnot::Line {
            from: (100.0, 600.0),
            to: (300.0, 650.0),
            width: 1.0,
        },
    ];
    let kinds = [
        AnnotKind::Underline,
        AnnotKind::StrikeOut,
        AnnotKind::Squiggly,
        AnnotKind::Note,
        AnnotKind::FreeText,
        AnnotKind::Ink,
        AnnotKind::Square,
        AnnotKind::Circle,
        AnnotKind::Line,
    ];
    for (n, k) in new.into_iter().zip(kinds) {
        let a = engine.add_annotation(doc, 1, n, style()).unwrap();
        assert_eq!(a.kind, k);
        assert!(
            a.rect.width() > 0.0 && a.rect.height() > 0.0,
            "{k:?} has a box"
        );
    }
    let listed = engine.annotations(doc, 1).unwrap();
    assert_eq!(listed.iter().map(|a| a.kind).collect::<Vec<_>>(), kinds);
    assert_eq!(listed[3].contents, "Check this");
    // Page 1 keeps its two links, which are not listed as comments.
    assert!(engine.annotations(doc, 0).unwrap().is_empty());
    assert_eq!(engine.links(doc, 0).unwrap().len(), 2);

    let bad = engine.add_annotation(
        doc,
        1,
        NewAnnot::Shape {
            kind: AnnotKind::Ink,
            rect: box_,
            width: 1.0,
        },
        style(),
    );
    assert!(bad.is_err());
}

#[test]
fn undo_and_redo_step_through_edits() {
    let engine = Engine::start();
    let doc = engine.open(fixture("hello.pdf")).unwrap().id;
    assert_eq!(engine.history(doc).unwrap(), mp_engine::History::default());

    let highlight = NewAnnot::TextMarkup {
        kind: AnnotKind::Highlight,
        rects: vec![TEXT],
    };
    let first = engine.add_annotation(doc, 0, highlight, style()).unwrap();
    let note = NewAnnot::Note {
        x: 250.0,
        y: 20.0,
        text: "Second".into(),
    };
    engine.add_annotation(doc, 0, note, style()).unwrap();
    let history = engine.history(doc).unwrap();
    assert_eq!(history.undo.as_deref(), Some("Add note"));
    assert_eq!(history.redo, None);

    let history = engine.undo(doc).unwrap();
    assert_eq!(engine.annotations(doc, 0).unwrap(), vec![first.clone()]);
    assert_eq!(history.undo.as_deref(), Some("Highlight"));
    assert_eq!(history.redo.as_deref(), Some("Add note"));

    engine.undo(doc).unwrap();
    assert!(engine.annotations(doc, 0).unwrap().is_empty());
    assert_eq!(yellow_pixels(&engine, doc), 0);

    let history = engine.redo(doc).unwrap();
    assert_eq!(engine.annotations(doc, 0).unwrap(), vec![first.clone()]);
    assert!(yellow_pixels(&engine, doc) > 1000);
    assert_eq!(history.redo.as_deref(), Some("Add note"));

    engine.delete_annotation(doc, 0, first.id).unwrap();
    let history = engine.history(doc).unwrap();
    assert_eq!(history.undo.as_deref(), Some("Delete comment"));
    // A new edit drops the steps that could have been redone.
    assert_eq!(history.redo, None);
    engine.undo(doc).unwrap();
    assert_eq!(engine.annotations(doc, 0).unwrap().len(), 1);
}
