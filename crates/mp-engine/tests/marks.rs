use std::path::PathBuf;

use mp_engine::{AnnotKind, DocId, Engine, Mark, Rect};

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

/// hello.pdf is 300 x 200 pt with its text between y = 88 and 116; below it is blank.
const CENTER: (f32, f32) = (150.0, 160.0);

/// Pixels inside `rect` (page space, rendered at 1:1) that `test` accepts.
fn pixels(engine: &Engine, doc: DocId, rect: Rect, test: impl Fn(&[u8; 3]) -> bool) -> usize {
    let list = engine.display_list(doc, 0).unwrap();
    let image = mp_engine::render(&list, 1.0).unwrap();
    let (w, rgb) = (image.width as usize, image.rgb.as_chunks::<3>().0);
    rgb.iter()
        .enumerate()
        .filter(|&(i, p)| {
            let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
            x > rect.x0 && x < rect.x1 && y > rect.y0 && y < rect.y1 && test(p)
        })
        .count()
}

fn dark(p: &[u8; 3]) -> bool {
    p.iter().all(|&c| c < 100)
}

/// A 4 x 2 PNG, all one red.
const RED_PNG: [u8; 73] = [
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 4, 0, 0, 0, 2, 8, 2, 0,
    0, 0, 240, 202, 234, 52, 0, 0, 0, 16, 73, 68, 65, 84, 120, 156, 99, 184, 43, 40, 8, 71, 12,
    200, 28, 0, 110, 18, 7, 249, 139, 18, 99, 207, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

#[test]
fn a_drawn_signature_is_a_stamp_at_the_placed_width() {
    let engine = Engine::start();
    let doc = engine.open(fixture("hello.pdf")).unwrap().id;
    // A "V" 200 units wide and 100 tall; round caps add half the pen width on every side.
    let mark = Mark::Ink {
        strokes: vec![vec![(0.0, 0.0), (100.0, 100.0), (200.0, 0.0)]],
        width: 8.0,
    };
    let aspect = engine.mark_aspect(mark.clone()).unwrap();
    assert!((aspect - 208.0 / 108.0).abs() < 0.05, "aspect {aspect}");

    let placed = engine
        .place_mark(doc, 0, mark, CENTER, 100.0, [0.0, 0.0, 0.0])
        .unwrap();
    assert_eq!(placed.kind, AnnotKind::Stamp);
    let r = placed.rect;
    assert!((r.x1 - r.x0 - 100.0).abs() < 0.5, "{r:?}");
    assert!((r.y1 - r.y0 - 100.0 / aspect).abs() < 0.5, "{r:?}");
    assert!(((r.x0 + r.x1) / 2.0 - CENTER.0).abs() < 0.5);
    assert!(((r.y0 + r.y1) / 2.0 - CENTER.1).abs() < 0.5);
    let ink = pixels(&engine, doc, r, dark);
    assert!(ink > 100, "{ink} dark pixels");
    assert_eq!(engine.history(doc).unwrap().undo.as_deref(), Some("Sign"));

    // A new colour keeps the mark: MuPDF does not redraw it as a standard stamp.
    engine
        .set_color(doc, 0, placed.id, [1.0, 0.0, 0.0])
        .unwrap();
    assert!(pixels(&engine, doc, r, dark) > ink / 2);

    // The mark survives a save.
    let saved = Scratch::new("signed.pdf");
    engine.save(doc, &saved.0, false).unwrap();
    let reopened = engine.open(saved.0.clone()).unwrap().id;
    assert_eq!(engine.annotations(reopened, 0).unwrap().len(), 1);
    assert!(pixels(&engine, reopened, r, dark) > ink / 2);

    engine.undo(doc).unwrap();
    engine.undo(doc).unwrap();
    assert!(engine.annotations(doc, 0).unwrap().is_empty());
    assert_eq!(pixels(&engine, doc, r, dark), 0);
}

#[test]
fn a_typed_signature_is_drawn_in_its_font() {
    let engine = Engine::start();
    let doc = engine.open(fixture("hello.pdf")).unwrap().id;
    let mark = Mark::Typed {
        text: "Ada".into(),
        font: "Arial".into(),
    };
    let aspect = engine.mark_aspect(mark.clone()).unwrap();
    assert!(aspect > 1.5 && aspect < 3.5, "aspect {aspect}");
    let placed = engine
        .place_mark(doc, 0, mark, CENTER, 90.0, [0.0, 0.0, 0.0])
        .unwrap();
    assert!(pixels(&engine, doc, placed.rect, dark) > 100);

    let missing = Mark::Typed {
        text: "Ada".into(),
        font: "No Such Font".into(),
    };
    assert!(engine.mark_aspect(missing).is_err());
    let blank = Mark::Typed {
        text: "  ".into(),
        font: "Arial".into(),
    };
    assert!(engine.mark_aspect(blank).is_err());
}

#[test]
fn an_image_signature_keeps_its_pixels() {
    let engine = Engine::start();
    let doc = engine.open(fixture("hello.pdf")).unwrap().id;
    let mark = Mark::Image(RED_PNG.to_vec());
    assert_eq!(engine.mark_aspect(mark.clone()).unwrap(), 2.0);
    let placed = engine
        .place_mark(doc, 0, mark, CENTER, 60.0, [0.0, 0.0, 0.0])
        .unwrap();
    let r = placed.rect;
    assert!((r.y1 - r.y0 - 30.0).abs() < 0.5, "{r:?}");
    let red = pixels(&engine, doc, r, |p| p[0] > 180 && p[1] < 60 && p[2] < 60);
    // Nearly all of the 60 x 30 stamp.
    assert!(red > 1500, "{red} red pixels");

    assert!(
        engine
            .mark_aspect(Mark::Image(b"not an image".to_vec()))
            .is_err()
    );
}
