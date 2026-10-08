use std::path::PathBuf;

use mp_engine::{AnnotKind, Engine, NewAnnot, Rect, Restyle, Style};

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

    engine
        .restyle(doc, 0, added.id, Restyle::Color([0.2, 0.5, 1.0]))
        .unwrap();
    assert_eq!(
        engine.annotations(doc, 0).unwrap()[0].color,
        Some([0.2, 0.5, 1.0])
    );
    assert_eq!(yellow_pixels(&engine, doc), 0);

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

    assert_eq!(history.position, 1);

    // Saving keeps the history.
    let saved = Scratch::new("history.pdf");
    engine.save(doc, &saved.0, false).unwrap();
    assert_eq!(engine.history(doc).unwrap(), history);

    engine.delete_annotation(doc, 0, first.id).unwrap();
    let history = engine.history(doc).unwrap();
    assert_eq!(history.undo.as_deref(), Some("Delete comment"));
    // A new edit drops the steps that could have been redone.
    assert_eq!(history.redo, None);
    engine.undo(doc).unwrap();
    assert_eq!(engine.annotations(doc, 0).unwrap().len(), 1);
}

#[test]
fn comments_round_trip_through_xfdf() {
    let engine = Engine::start();
    let doc = engine.open(fixture("hello.pdf")).unwrap().id;
    let mut s = style();
    let highlight = NewAnnot::TextMarkup {
        kind: AnnotKind::Highlight,
        rects: vec![TEXT],
    };
    engine.add_annotation(doc, 0, highlight, s.clone()).unwrap();
    let note = NewAnnot::Note {
        x: 250.0,
        y: 20.0,
        text: "Check <this> & that".into(),
    };
    engine.add_annotation(doc, 0, note, s.clone()).unwrap();
    s.color = [0.1, 0.45, 0.9];
    let ink = NewAnnot::Ink {
        strokes: vec![vec![(20.0, 150.0), (60.0, 170.0), (100.0, 150.0)]],
        width: 2.0,
    };
    engine.add_annotation(doc, 0, ink, s.clone()).unwrap();
    let square = NewAnnot::Shape {
        kind: AnnotKind::Square,
        rect: Rect {
            x0: 200.0,
            y0: 140.0,
            x1: 260.0,
            y1: 180.0,
        },
        width: 1.5,
    };
    engine.add_annotation(doc, 0, square, s.clone()).unwrap();
    let line = NewAnnot::Line {
        from: (20.0, 190.0),
        to: (120.0, 190.0),
        width: 1.0,
    };
    engine.add_annotation(doc, 0, line, s).unwrap();
    let before = engine.annotations(doc, 0).unwrap();

    let (xml, count) = engine.export_comments(doc, "hello.pdf".into()).unwrap();
    assert_eq!(count, 5, "{xml}");
    for part in [
        "<highlight page=\"0\"",
        "coords=\"",
        "title=\"Tester\"",
        "<contents>Check &lt;this&gt; &amp; that</contents>",
        "<inklist><gesture>",
        "<f href=\"hello.pdf\"/>",
    ] {
        assert!(xml.contains(part), "{part} missing from {xml}");
    }

    // Into a fresh copy: the same comments, in the same places, drawn.
    let copy = engine.open(fixture("hello.pdf")).unwrap().id;
    assert_eq!(engine.import_comments(copy, xml.clone()).unwrap(), 5);
    let after = engine.annotations(copy, 0).unwrap();
    assert_eq!(after.len(), 5);
    for (a, b) in before.iter().zip(&after) {
        assert_eq!(
            (a.kind, &a.contents, &a.author),
            (b.kind, &b.contents, &b.author)
        );
        let (ca, cb) = (a.color.unwrap(), b.color.unwrap());
        assert!(
            ca.iter().zip(cb).all(|(x, y)| (x - y).abs() < 0.01),
            "{ca:?} {cb:?}"
        );
        let (ra, rb) = (a.rect, b.rect);
        let close = [ra.x0 - rb.x0, ra.y0 - rb.y0, ra.x1 - rb.x1, ra.y1 - rb.y1]
            .iter()
            .all(|d| d.abs() < 2.0);
        assert!(close, "{:?}: {ra:?} became {rb:?}", a.kind);
    }
    assert!(yellow_pixels(&engine, copy) > 1000);
    assert_eq!(
        engine.history(copy).unwrap().undo.as_deref(),
        Some("Import comments")
    );

    // Importing again adds nothing, in the copy or the original: the names match.
    assert_eq!(engine.import_comments(copy, xml.clone()).unwrap(), 0);
    assert_eq!(engine.import_comments(doc, xml).unwrap(), 0);
    assert!(engine.import_comments(copy, "not xml".into()).is_err());
}

/// Yellow pixels inside `rect` (page space, rendered at 1:1).
fn yellow_in(engine: &Engine, doc: mp_engine::DocId, rect: Rect) -> usize {
    let list = engine.display_list(doc, 0).unwrap();
    let image = mp_engine::render(&list, 1.0).unwrap();
    let (w, rgb) = (image.width as usize, image.rgb.as_chunks::<3>().0);
    rgb.iter()
        .enumerate()
        .filter(|&(i, _)| {
            let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
            x > rect.x0 && x < rect.x1 && y > rect.y0 && y < rect.y1
        })
        .filter(|(_, p)| p[0] > 200 && p[1] > 170 && p[2] < 120)
        .count()
}

#[test]
fn comments_move_and_resize() {
    let engine = Engine::start();
    let doc = engine.open(fixture("hello.pdf")).unwrap().id;
    let below = Rect {
        x0: 20.0,
        y0: 130.0,
        x1: 70.0,
        y1: 170.0,
    };
    let square = engine
        .add_annotation(
            doc,
            0,
            NewAnnot::Shape {
                kind: AnnotKind::Square,
                rect: below,
                width: 2.0,
            },
            style(),
        )
        .unwrap();
    assert!(yellow_in(&engine, doc, below) > 50);

    let r = square.rect;
    let above = Rect {
        y0: r.y0 - 110.0,
        y1: r.y1 - 110.0,
        ..r
    };
    engine.reshape(doc, 0, square.id, above).unwrap();
    assert_eq!(
        engine.history(doc).unwrap().undo.as_deref(),
        Some("Move comment")
    );
    let r = engine.annotations(doc, 0).unwrap()[0].rect;
    for (a, b) in [
        (r.x0, above.x0),
        (r.y0, above.y0),
        (r.x1, above.x1),
        (r.y1, above.y1),
    ] {
        assert!((a - b).abs() < 0.5, "{r:?} {above:?}");
    }
    assert_eq!(yellow_in(&engine, doc, below), 0);
    assert!(yellow_in(&engine, doc, above) > 50);

    let ink = engine
        .add_annotation(
            doc,
            0,
            NewAnnot::Ink {
                strokes: vec![vec![(160.0, 140.0), (200.0, 170.0), (240.0, 140.0)]],
                width: 2.0,
            },
            style(),
        )
        .unwrap();
    let r = ink.rect;
    let half = Rect {
        x1: r.x0 + r.width() / 2.0,
        ..r
    };
    engine.reshape(doc, 0, ink.id, half).unwrap();
    assert_eq!(
        engine.history(doc).unwrap().undo.as_deref(),
        Some("Resize comment")
    );
    let shrunk = engine.annotations(doc, 0).unwrap()[1].rect;
    assert!((shrunk.width() - half.width()).abs() < 0.5, "{shrunk:?}");
    let right = Rect {
        x0: half.x1 + 2.0,
        ..r
    };
    assert_eq!(yellow_in(&engine, doc, right), 0);

    engine.undo(doc).unwrap();
    assert!(yellow_in(&engine, doc, right) > 20);
    engine.undo(doc).unwrap();
    engine.undo(doc).unwrap();
    assert!(yellow_in(&engine, doc, below) > 50);
    assert_eq!(yellow_in(&engine, doc, above), 0);
}

#[test]
fn saving_a_repaired_file_over_itself_rewrites_it_whole() {
    let engine = Engine::start();
    let copy = Scratch::new("repaired.pdf");
    std::fs::copy(fixture("truncated.pdf"), &copy.0).unwrap();
    let doc = engine.open(&copy.0).unwrap().id;
    let note = |text: &str| NewAnnot::Note {
        x: 20.0,
        y: 20.0,
        text: text.into(),
    };
    engine
        .add_annotation(doc, 0, note("Kept"), style())
        .unwrap();

    // MuPDF repaired the file on open, so it cannot append; the save swaps in a rewrite.
    assert!(engine.save(doc, &copy.0, true).unwrap());
    let temp = copy.0.with_file_name(format!(
        "{}.micropdf-save.tmp",
        copy.0.file_name().unwrap().to_string_lossy()
    ));
    assert!(!temp.exists());
    assert_eq!(engine.history(doc).unwrap().position, 0);
    let listed = engine.annotations(doc, 0).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].contents, "Kept");

    // The rewritten file is sound, so the next save appends and keeps the document open.
    engine
        .add_annotation(doc, 0, note("Also kept"), style())
        .unwrap();
    assert!(!engine.save(doc, &copy.0, true).unwrap());
    assert_eq!(engine.history(doc).unwrap().position, 1);
    let fresh = engine.open(&copy.0).unwrap().id;
    let texts: Vec<String> = engine
        .annotations(fresh, 0)
        .unwrap()
        .into_iter()
        .map(|a| a.contents)
        .collect();
    assert_eq!(texts, ["Kept", "Also kept"]);
}

#[test]
fn a_full_save_over_the_open_file_keeps_it_readable() {
    let engine = Engine::start();
    let copy = Scratch::new("full-in-place.pdf");
    std::fs::copy(fixture("outline-links.pdf"), &copy.0).unwrap();
    let doc = engine.open(&copy.0).unwrap().id;
    let pages = engine.page_sizes(doc).unwrap().len();
    engine
        .add_annotation(
            doc,
            1,
            NewAnnot::TextMarkup {
                kind: AnnotKind::Highlight,
                rects: vec![TEXT],
            },
            style(),
        )
        .unwrap();
    assert!(engine.save(doc, &copy.0, false).unwrap());
    // Every page still renders from the reopened document, and from a fresh open.
    for d in [doc, engine.open(&copy.0).unwrap().id] {
        assert_eq!(engine.page_sizes(d).unwrap().len(), pages);
        for page in 0..pages {
            engine.display_list(d, page).unwrap();
        }
        assert_eq!(engine.annotations(d, 1).unwrap().len(), 1);
    }
}

#[test]
fn replies_and_review_states_thread_under_their_comment() {
    let engine = Engine::start();
    let doc = engine.open(fixture("hello.pdf")).unwrap().id;
    let highlight = engine
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
    let before = engine.display_list(doc, 0).unwrap();
    let before = mp_engine::render(&before, 1.0).unwrap().rgb;

    let reply = engine
        .reply(doc, 0, highlight.id, "Agreed".into(), "Ada".into())
        .unwrap();
    assert_eq!(reply.reply_to, Some(highlight.id));
    assert_eq!(reply.kind, AnnotKind::Note);
    assert_eq!(engine.history(doc).unwrap().undo.as_deref(), Some("Reply"));
    engine
        .set_state(doc, 0, highlight.id, "Accepted".into(), "Ada".into())
        .unwrap();
    assert_eq!(
        engine.history(doc).unwrap().undo.as_deref(),
        Some("Status: Accepted")
    );
    assert!(
        engine
            .set_state(doc, 0, highlight.id, "Maybe".into(), "Ada".into())
            .is_err()
    );

    let listed = engine.annotations(doc, 0).unwrap();
    assert_eq!(listed.len(), 3);
    assert_eq!(listed[1].contents, "Agreed");
    assert_eq!(listed[1].author, "Ada");
    assert_eq!(listed[1].state, None);
    assert_eq!(listed[2].reply_to, Some(highlight.id));
    assert_eq!(listed[2].state.as_deref(), Some("Accepted"));
    assert_eq!(listed[2].contents, "Accepted set by Ada");

    // Replies stay off the page: it looks as it did with the highlight alone.
    let after = engine.display_list(doc, 0).unwrap();
    assert!(mp_engine::render(&after, 1.0).unwrap().rgb == before);

    // They survive a save, and go with their comment when it is deleted.
    let saved = Scratch::new("threads.pdf");
    engine.save(doc, &saved.0, false).unwrap();
    let copy = engine.open(&saved.0).unwrap().id;
    let reread = engine.annotations(copy, 0).unwrap();
    assert_eq!(reread[1].reply_to, Some(reread[0].id));
    engine.delete_annotation(copy, 0, reread[0].id).unwrap();
    assert!(engine.annotations(copy, 0).unwrap().is_empty());

    // Flattening keeps the highlight's look and drops the thread.
    engine.flatten(doc, true, false).unwrap();
    assert!(engine.annotations(doc, 0).unwrap().is_empty());
    let flat = engine.display_list(doc, 0).unwrap();
    let flat = mp_engine::render(&flat, 1.0).unwrap().rgb;
    let differ = flat
        .iter()
        .zip(&before)
        .filter(|(a, b)| a.abs_diff(**b) > 8)
        .count();
    assert_eq!(differ, 0);
}

#[test]
fn threads_round_trip_through_xfdf() {
    let engine = Engine::start();
    let doc = engine.open(fixture("hello.pdf")).unwrap().id;
    let note = engine
        .add_annotation(
            doc,
            0,
            NewAnnot::Note {
                x: 20.0,
                y: 20.0,
                text: "Question".into(),
            },
            style(),
        )
        .unwrap();
    engine
        .reply(doc, 0, note.id, "Answer".into(), "Ada".into())
        .unwrap();
    engine
        .set_state(doc, 0, note.id, "Completed".into(), "Ada".into())
        .unwrap();
    let (xml, n) = engine.export_comments(doc, "hello.pdf".into()).unwrap();
    assert_eq!(n, 3);
    assert_eq!(xml.matches("inreplyto=").count(), 2, "{xml}");
    assert!(
        xml.contains("state=\"Completed\" statemodel=\"Review\""),
        "{xml}"
    );

    let fresh = engine.open(fixture("hello.pdf")).unwrap().id;
    assert_eq!(engine.import_comments(fresh, xml).unwrap(), 3);
    let listed = engine.annotations(fresh, 0).unwrap();
    assert_eq!(listed[0].reply_to, None);
    assert_eq!(listed[1].reply_to, Some(listed[0].id));
    assert_eq!(listed[1].contents, "Answer");
    assert_eq!(listed[2].reply_to, Some(listed[0].id));
    assert_eq!(listed[2].state.as_deref(), Some("Completed"));
}

#[test]
fn standard_stamps_are_drawn_by_name() {
    let engine = Engine::start();
    let doc = engine.open(fixture("hello.pdf")).unwrap().id;
    let red = Style {
        color: [0.8, 0.1, 0.1],
        author: "Tester".into(),
    };
    let stamp = engine
        .add_annotation(
            doc,
            0,
            NewAnnot::Stamp {
                name: "Approved".into(),
                center: (150.0, 160.0),
                width: 114.0,
            },
            red.clone(),
        )
        .unwrap();
    assert_eq!(stamp.kind, AnnotKind::Stamp);
    let r = stamp.rect;
    assert!((r.width() - 114.0).abs() < 1.0, "{r:?}");
    assert!((r.height() - 30.0).abs() < 1.0, "{r:?}");
    assert!(((r.x0 + r.x1) / 2.0 - 150.0).abs() < 0.5);
    assert_eq!(
        engine.history(doc).unwrap().undo.as_deref(),
        Some("Add stamp")
    );

    // The stamp's red letters are on the page.
    let list = engine.display_list(doc, 0).unwrap();
    let image = mp_engine::render(&list, 1.0).unwrap();
    let w = image.width as usize;
    let reds = image
        .rgb
        .as_chunks::<3>()
        .0
        .iter()
        .enumerate()
        .filter(|&(i, p)| {
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            r.contains(x, y) && p[0] > 150 && p[1] < 90 && p[2] < 90
        })
        .count();
    assert!(reds > 200, "{reds}");

    let bad = engine.add_annotation(
        doc,
        0,
        NewAnnot::Stamp {
            name: "Whatever".into(),
            center: (150.0, 160.0),
            width: 114.0,
        },
        red,
    );
    assert!(bad.is_err());
}

#[test]
fn a_callout_points_at_its_target_and_round_trips() {
    let engine = Engine::start();
    let doc = engine.open(fixture("hello.pdf")).unwrap().id;
    let box_ = Rect {
        x0: 180.0,
        y0: 140.0,
        x1: 280.0,
        y1: 180.0,
    };
    let callout = engine
        .add_annotation(
            doc,
            0,
            NewAnnot::Callout {
                target: (60.0, 40.0),
                rect: box_,
                text: "Look here".into(),
            },
            Style {
                color: [1.0, 1.0, 0.8],
                author: "Tester".into(),
            },
        )
        .unwrap();
    assert_eq!(callout.kind, AnnotKind::Callout);
    assert_eq!(callout.contents, "Look here");
    assert_eq!(
        engine.history(doc).unwrap().undo.as_deref(),
        Some("Add callout")
    );
    // The bounds grew from the box to the target.
    let r = callout.rect;
    assert!(r.x0 <= 61.0 && r.y0 <= 41.0, "{r:?}");
    assert!(r.x1 >= 279.0 && r.y1 >= 179.0, "{r:?}");

    let (xml, _) = engine.export_comments(doc, "hello.pdf".into()).unwrap();
    assert!(xml.contains("intent=\"FreeTextCallout\""), "{xml}");
    assert!(xml.contains("callout=\""), "{xml}");
    let fresh = engine.open(fixture("hello.pdf")).unwrap().id;
    assert_eq!(engine.import_comments(fresh, xml).unwrap(), 1);
    let copy = &engine.annotations(fresh, 0).unwrap()[0];
    assert_eq!(copy.kind, AnnotKind::Callout);
    assert!((copy.rect.x0 - r.x0).abs() < 2.0, "{:?} {r:?}", copy.rect);
}

#[test]
fn files_attach_to_the_document_and_to_pages() {
    let engine = Engine::start();
    let doc = engine.open(fixture("attachment.pdf")).unwrap().id;
    engine
        .add_attachment(doc, "notes.txt".into(), b"second".to_vec())
        .unwrap();
    assert_eq!(
        engine.history(doc).unwrap().undo.as_deref(),
        Some("Attach file")
    );
    let clip = engine
        .add_annotation(
            doc,
            0,
            NewAnnot::File {
                at: (40.0, 40.0),
                name: "data.csv".into(),
                data: b"a,b\n1,2\n".to_vec(),
            },
            Style {
                color: [0.1, 0.3, 0.7],
                author: "Tester".into(),
            },
        )
        .unwrap();
    assert_eq!(clip.kind, AnnotKind::File);
    assert_eq!(clip.contents, "data.csv");

    let files = engine.attachments(doc).unwrap();
    let names: Vec<_> = files.iter().map(|f| (f.name.as_str(), f.page)).collect();
    assert_eq!(
        names,
        [
            ("notes.txt", None),
            ("notes.txt", None),
            ("data.csv", Some(0))
        ]
    );
    let data = |i| engine.attachment_data(doc, i).unwrap();
    let second = (0..2)
        .find(|&i| data(i) == b"second")
        .expect("the added file is listed");
    assert_eq!(data(1 - second), b"Embedded by make_fixtures.py.\n");
    assert_eq!(data(2), b"a,b\n1,2\n");

    // Both survive a save.
    let copy = std::env::temp_dir().join(format!("mp-attach-{}.pdf", std::process::id()));
    engine.save(doc, &copy, false).unwrap();
    let saved = engine.open(&copy).unwrap().id;
    assert_eq!(engine.attachments(saved).unwrap().len(), 3);
    engine.close(saved);
    let _ = std::fs::remove_file(&copy);

    // Deleting the page's file deletes its comment; deleting a document file leaves the other.
    engine.delete_attachment(doc, 2).unwrap();
    assert!(engine.annotations(doc, 0).unwrap().is_empty());
    engine.delete_attachment(doc, second).unwrap();
    assert_eq!(
        engine.history(doc).unwrap().undo.as_deref(),
        Some("Delete attachment")
    );
    let files = engine.attachments(doc).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(data(0), b"Embedded by make_fixtures.py.\n");
    engine.undo(doc).unwrap();
    engine.undo(doc).unwrap();
    assert_eq!(engine.attachments(doc).unwrap().len(), 3);
}

#[test]
fn restyle_changes_fill_opacity_and_width() {
    let engine = Engine::start();
    let doc = engine.open(fixture("hello.pdf")).unwrap().id;
    let style = Style {
        color: [0.85, 0.15, 0.15],
        author: "Tester".into(),
    };
    let rect = engine
        .add_annotation(
            doc,
            0,
            NewAnnot::Shape {
                kind: AnnotKind::Square,
                rect: Rect {
                    x0: 40.0,
                    y0: 40.0,
                    x1: 120.0,
                    y1: 100.0,
                },
                width: 1.5,
            },
            style.clone(),
        )
        .unwrap();
    assert_eq!((rect.fill, rect.width), (None, Some(1.5)));
    assert!((rect.opacity - 1.0).abs() < 1e-6);

    let change = |c| engine.restyle(doc, 0, rect.id, c).unwrap();
    change(Restyle::Fill(Some([0.1, 0.45, 0.9])));
    change(Restyle::Width(4.0));
    change(Restyle::Opacity(0.5));
    let now = engine.annotations(doc, 0).unwrap()[0].clone();
    assert_eq!(now.fill, Some([0.1, 0.45, 0.9]));
    assert_eq!(now.width, Some(4.0));
    assert!((now.opacity - 0.5).abs() < 0.01, "{}", now.opacity);
    assert_eq!(
        engine.history(doc).unwrap().undo.as_deref(),
        Some("Change opacity")
    );
    change(Restyle::Fill(None));
    assert_eq!(engine.annotations(doc, 0).unwrap()[0].fill, None);

    // A note has no line or fill to set.
    let note = engine
        .add_annotation(
            doc,
            0,
            NewAnnot::Note {
                x: 200.0,
                y: 20.0,
                text: "n".into(),
            },
            style,
        )
        .unwrap();
    assert_eq!((note.width, note.fill), (None, None));
}
