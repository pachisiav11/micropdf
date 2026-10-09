//! Organize, stamps, protection, Optimize, Sanitize and redaction.

use std::path::{Path, PathBuf};

use mp_engine::{
    Bates, DocId, Engine, Find, LabelStyle, Optimize, Overlay, PATTERNS, Place, Protection, Rect,
    Sanitize, Security,
};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

/// A scratch folder in the system temp folder, removed again when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("mp-tools-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn texts(engine: &Engine, doc: DocId) -> Vec<String> {
    let pages = engine.page_sizes(doc).unwrap().len();
    (0..pages)
        .map(|p| {
            let text = mp_engine::page_text(&engine.display_list(doc, p).unwrap()).unwrap();
            text.text(0..text.chars.len())
        })
        .collect()
}

fn first_lines(engine: &Engine, doc: DocId) -> Vec<String> {
    texts(engine, doc)
        .iter()
        .map(|t| t.lines().next().unwrap_or_default().to_owned())
        .collect()
}

fn open(engine: &Engine, path: &Path) -> DocId {
    engine.open(path).unwrap().id
}

#[test]
fn pages_move_rotate_copy_insert_and_delete() {
    let engine = Engine::start();
    let doc = open(&engine, &fixture("outline-links.pdf"));
    assert_eq!(
        first_lines(&engine, doc),
        ["Chapter 1", "Chapter 2", "Chapter 3"]
    );

    assert_eq!(engine.move_pages(doc, vec![2], 0).unwrap(), 0);
    assert_eq!(
        first_lines(&engine, doc),
        ["Chapter 3", "Chapter 1", "Chapter 2"]
    );
    assert_eq!(engine.move_pages(doc, vec![0], 3).unwrap(), 2);
    assert_eq!(
        first_lines(&engine, doc),
        ["Chapter 1", "Chapter 2", "Chapter 3"]
    );
    engine.undo(doc).unwrap();
    assert_eq!(
        first_lines(&engine, doc),
        ["Chapter 3", "Chapter 1", "Chapter 2"]
    );
    engine.redo(doc).unwrap();

    engine.rotate_pages(doc, vec![0], 90).unwrap();
    let size = engine.page_sizes(doc).unwrap()[0];
    assert_eq!((size.width, size.height), (792.0, 612.0));
    engine.rotate_pages(doc, vec![0], -90).unwrap();

    engine.duplicate_pages(doc, vec![1]).unwrap();
    assert_eq!(
        first_lines(&engine, doc),
        ["Chapter 1", "Chapter 2", "Chapter 2", "Chapter 3"]
    );
    engine.delete_pages(doc, vec![1]).unwrap();
    assert!(engine.delete_pages(doc, vec![0, 1, 2]).is_err());

    assert_eq!(
        engine
            .insert_file(doc, 1, fixture("hello.pdf"), String::new())
            .unwrap(),
        1
    );
    engine.insert_blank_page(doc, 0).unwrap();
    assert_eq!(
        first_lines(&engine, doc),
        ["", "Chapter 1", "Hello micropdf", "Chapter 2", "Chapter 3"]
    );

    assert_eq!(
        engine
            .replace_pages(doc, vec![0], fixture("hello.pdf"), String::new())
            .unwrap(),
        1
    );
    assert_eq!(first_lines(&engine, doc)[0], "Hello micropdf");

    // Saved and opened again, the pages keep their new order.
    let scratch = Scratch::new("organize");
    engine.save(doc, &scratch.file("out.pdf"), false).unwrap();
    let again = open(&engine, &scratch.file("out.pdf"));
    assert_eq!(first_lines(&engine, again)[3], "Chapter 2");
}

#[test]
fn pages_go_to_new_files_and_files_combine() {
    let engine = Engine::start();
    let doc = open(&engine, &fixture("outline-links.pdf"));
    let scratch = Scratch::new("files");

    engine
        .extract_pages(doc, vec![2, 0], scratch.file("two.pdf"))
        .unwrap();
    let two = open(&engine, &scratch.file("two.pdf"));
    assert_eq!(first_lines(&engine, two), ["Chapter 1", "Chapter 3"]);

    let groups = mp_engine::parse_ranges("1, 2-", 3).unwrap();
    let written = engine.split(doc, groups, scratch.file("part.pdf")).unwrap();
    assert_eq!(
        written,
        [scratch.file("part-1.pdf"), scratch.file("part-2.pdf")]
    );
    let second = open(&engine, &written[1]);
    assert_eq!(first_lines(&engine, second), ["Chapter 2", "Chapter 3"]);

    let sources = [fixture("hello.pdf"), fixture("outline-links.pdf")];
    mp_engine::combine(&sources, &scratch.file("all.pdf")).unwrap();
    let all = open(&engine, &scratch.file("all.pdf"));
    assert_eq!(
        first_lines(&engine, all),
        ["Hello micropdf", "Chapter 1", "Chapter 2", "Chapter 3"]
    );
    assert!(mp_engine::combine(&[fixture("encrypted.pdf")], &scratch.file("x.pdf")).is_err());
}

#[test]
fn pages_crop_and_take_labels() {
    let engine = Engine::start();
    let doc = open(&engine, &fixture("outline-links.pdf"));
    engine
        .crop_pages(doc, vec![0], [72.0, 36.0, 72.0, 36.0])
        .unwrap();
    let size = engine.page_sizes(doc).unwrap()[0];
    assert_eq!((size.width, size.height), (468.0, 720.0));
    assert!(
        engine
            .crop_pages(doc, vec![1], [400.0, 0.0, 400.0, 0.0])
            .is_err()
    );

    engine
        .label_pages(doc, 0, LabelStyle::LowerRoman, String::new(), 1)
        .unwrap();
    engine
        .label_pages(doc, 2, LabelStyle::Decimal, "A-".into(), 1)
        .unwrap();
    assert_eq!(engine.page_labels(doc).unwrap(), ["i", "ii", "A-1"]);
}

#[test]
fn stamps_number_and_mark_pages() {
    let engine = Engine::start();
    let doc = open(&engine, &fixture("outline-links.pdf"));
    let footer = Overlay {
        texts: vec![
            (Place::Bottom, "Page {page} of {pages}".into()),
            (Place::BottomRight, "{bates}".into()),
            (Place::TopLeft, "Printed {date}".into()),
        ],
        bates: Some(Bates {
            prefix: "ACME-".into(),
            suffix: String::new(),
            start: 41,
            digits: 4,
        }),
        date: "2026-10-09".into(),
        ..Overlay::default()
    };
    assert_eq!(
        engine
            .stamp_pages(doc, vec![0, 1, 2], footer, "Add header and footer")
            .unwrap(),
        44
    );
    let watermark = Overlay {
        texts: vec![(Place::Center, "DRAFT – é €".into())],
        size: 72.0,
        color: [1.0, 0.0, 0.0],
        opacity: 0.3,
        angle: 45.0,
        behind: true,
        ..Overlay::default()
    };
    engine
        .stamp_pages(doc, vec![1], watermark, "Add watermark")
        .unwrap();

    let all = texts(&engine, doc);
    assert!(
        all[0].contains("Page 1 of 3") && all[0].contains("ACME-0041"),
        "{}",
        all[0]
    );
    assert!(all[0].contains("Printed 2026-10-09"));
    assert!(all[2].contains("Page 3 of 3") && all[2].contains("ACME-0043"));
    assert!(all[1].contains("DRAFT – é €"), "{}", all[1]);
    assert!(!all[0].contains("DRAFT"));
    // The page's own text is still there, and the footer sits at the bottom.
    assert!(all[1].contains("Chapter 2"));
    let text = mp_engine::page_text(&engine.display_list(doc, 0).unwrap()).unwrap();
    let page = text.chars.iter().find(|c| c.ch == 'P').unwrap();
    assert!(
        page.rect.y1 > 740.0 && page.rect.y1 < 760.0,
        "{:?}",
        page.rect
    );
}

#[test]
fn passwords_go_on_and_come_off() {
    let engine = Engine::start();
    let scratch = Scratch::new("protect");
    let path = scratch.file("doc.pdf");
    std::fs::copy(fixture("hello.pdf"), &path).unwrap();
    let doc = open(&engine, &path);
    let protect = Protection {
        open: "open sesame".into(),
        owner: "owner".into(),
        print: true,
        ..Protection::default()
    };
    // Over the open file: rewritten, and open again with the new password.
    assert!(
        engine
            .save_secured(doc, &path, true, Security::Protect(protect))
            .unwrap()
    );
    assert_eq!(first_lines(&engine, doc), ["Hello micropdf"]);

    let other = Engine::start();
    let info = other.open(&path).unwrap();
    assert!(info.needs_password);
    assert_eq!(other.authenticate(info.id, "wrong").unwrap(), None);
    assert_eq!(other.authenticate(info.id, "open sesame").unwrap(), Some(1));
    let encryption = other.metadata(info.id).unwrap();
    assert!(
        encryption
            .iter()
            .any(|(k, v)| k == "Encryption" && v.contains("AES")),
        "{encryption:?}"
    );
    drop(other);

    assert!(
        engine
            .save_secured(doc, &path, true, Security::Remove)
            .unwrap()
    );
    let other = Engine::start();
    assert!(!other.open(&path).unwrap().needs_password);
}

#[test]
fn optimize_writes_a_copy_and_properties_change() {
    let engine = Engine::start();
    let scratch = Scratch::new("optimize");
    let doc = open(&engine, &fixture("form.pdf"));
    for (i, how) in [Optimize::LOSSLESS, Optimize::SMALLEST]
        .into_iter()
        .enumerate()
    {
        let target = scratch.file(&format!("small-{i}.pdf"));
        engine.optimize(doc, target.clone(), how).unwrap();
        let copy = open(&engine, &target);
        assert_eq!(engine.fields(copy, 0).unwrap().len(), 3);
    }
    let encrypted = Engine::start();
    let info = encrypted.open(fixture("encrypted.pdf")).unwrap();
    encrypted.authenticate(info.id, "user").unwrap();
    encrypted
        .optimize(info.id, scratch.file("enc.pdf"), Optimize::BALANCED)
        .unwrap();
    assert!(
        encrypted
            .open(scratch.file("enc.pdf"))
            .unwrap()
            .needs_password
    );

    let fields = vec![
        ("Title".into(), "Quarterly".into()),
        ("Author".into(), String::new()),
    ];
    let doc = open(&engine, &fixture("redact.pdf"));
    engine.set_info(doc, fields).unwrap();
    let meta = engine.metadata(doc).unwrap();
    assert!(meta.contains(&("Title".into(), "Quarterly".into())));
    assert!(!meta.iter().any(|(k, _)| k == "Author"));
    assert!(
        engine
            .set_info(doc, vec![("Producer".into(), "x".into())])
            .is_err()
    );
}

/// Whether `needle` is anywhere in the file: in its raw bytes or in any stream, decompressed.
fn anywhere(path: &Path, needle: &str) -> bool {
    let bytes = std::fs::read(path).unwrap();
    if bytes.windows(needle.len()).any(|w| w == needle.as_bytes()) {
        return true;
    }
    let pdf = mupdf::pdf::PdfDocument::open(path.to_str().unwrap()).unwrap();
    (1..pdf.xref_len().unwrap() as i32).any(|i| {
        pdf.xref_stream(i)
            .is_ok_and(|s| s.windows(needle.len()).any(|w| w == needle.as_bytes()))
    })
}

#[test]
fn sanitize_leaves_nothing_behind_in_the_file() {
    let engine = Engine::start();
    let scratch = Scratch::new("sanitize");
    let path = scratch.file("doc.pdf");
    std::fs::copy(fixture("redact.pdf"), &path).unwrap();
    let doc = open(&engine, &path);
    // Compressed, the fixture's secrets are still found.
    engine
        .save(doc, &scratch.file("compressed.pdf"), false)
        .unwrap();
    for secret in [
        "xmp-secret",
        "Jane Doe",
        "attachment-secret",
        "app.alert",
        "HIDDEN-OCR-LAYER",
    ] {
        assert!(
            anywhere(&scratch.file("compressed.pdf"), secret),
            "{secret} is in the fixture"
        );
    }
    let all = Sanitize {
        metadata: true,
        scripts: true,
        attachments: true,
        hidden_text: true,
        comments: true,
    };
    engine.sanitize(doc, all).unwrap();
    // Saving over the file would append; Sanitize makes it a rewrite.
    assert!(engine.save(doc, &path, true).unwrap());
    for secret in [
        "xmp-secret",
        "Jane Doe",
        "attachment-secret",
        "app.alert",
        "HIDDEN-OCR-LAYER",
    ] {
        assert!(!anywhere(&path, secret), "{secret} is still in the file");
    }
    let left = engine.attachments(doc).unwrap();
    assert!(left.is_empty(), "{left:?}");
    assert!(texts(&engine, doc)[0].contains("Project SECRET-PLAN starts on Monday."));
}

#[test]
fn redaction_removes_text_from_the_page_and_the_file() {
    let engine = Engine::start();
    let scratch = Scratch::new("redact");
    let doc = open(&engine, &fixture("redact.pdf"));
    let all = vec![0, 1];
    let email = Find::Pattern(PATTERNS[0].1.into());
    let phone = Find::Pattern(PATTERNS[1].1.into());
    assert_eq!(
        engine
            .mark_redactions(doc, all.clone(), Find::Text("secret-plan".into()))
            .unwrap(),
        1
    );
    assert_eq!(engine.mark_redactions(doc, all.clone(), email).unwrap(), 1);
    assert_eq!(engine.mark_redactions(doc, all.clone(), phone).unwrap(), 1);
    assert!(
        engine
            .mark_redactions(doc, all, Find::Pattern("(".into()))
            .is_err()
    );
    let marks = engine.annotations(doc, 0).unwrap();
    assert_eq!(marks.len(), 3);
    assert!(marks.iter().all(|a| a.kind == mp_engine::AnnotKind::Redact));
    // Marks alone remove nothing.
    assert!(texts(&engine, doc)[0].contains("SECRET-PLAN"));
    engine
        .mark_redaction(
            doc,
            1,
            Rect {
                x0: 0.0,
                y0: 0.0,
                x1: 1.0,
                y1: 1.0,
            },
        )
        .unwrap();

    assert_eq!(engine.apply_redactions(doc).unwrap(), 2);
    let page = &texts(&engine, doc)[0];
    for secret in ["SECRET-PLAN", "jane.doe", "example.com", "123-4567"] {
        assert!(!page.contains(secret), "{secret} in {page}");
    }
    assert!(
        page.contains("Contact") && page.contains("starts on Monday."),
        "{page}"
    );
    assert!(engine.annotations(doc, 0).unwrap().is_empty());

    let out = scratch.file("redacted.pdf");
    engine.save(doc, &out, true).unwrap();
    for secret in ["SECRET-PLAN", "jane.doe@example.com", "123-4567"] {
        assert!(!anywhere(&out, secret), "{secret} is still in the file");
    }
    let again = open(&engine, &out);
    assert_eq!(texts(&engine, again)[1], "Page two keeps its text.");
}
