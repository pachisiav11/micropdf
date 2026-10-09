//! Organize, stamps, protection, Optimize, Sanitize, redaction and digital signatures.

use std::path::{Path, PathBuf};

use mp_engine::{
    Bates, DocId, Engine, Find, LabelStyle, Optimize, Overlay, PATTERNS, Place, Protection,
    Recognize, Rect, Sanitize, Security, SignField, SignWith, Signing, Trust,
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

    assert_eq!(engine.size_groups(doc, 1).unwrap(), [[0], [1], [2]]);
    assert_eq!(engine.size_groups(doc, u64::MAX).unwrap(), [[0, 1, 2]]);

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
            vec![Rect {
                x0: 0.0,
                y0: 0.0,
                x1: 1.0,
                y1: 1.0,
            }],
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

fn signing(field: SignField, certify: bool, password: &str) -> Signing {
    Signing {
        with: SignWith::Pfx {
            data: std::fs::read(fixture("signer.pfx")).unwrap(),
            password: password.into(),
        },
        field,
        reason: "Approved".into(),
        location: "Test bench".into(),
        tsa: None,
        certify,
    }
}

fn new_field(y: f32) -> SignField {
    SignField::New {
        page: 0,
        rect: Rect {
            x0: 20.0,
            y0: y,
            x1: 200.0,
            y1: y + 40.0,
        },
    }
}

/// Signs a copy of hello.pdf at `path`, and opens it again from the saved bytes.
fn signed_copy(engine: &Engine, path: &Path, certify: bool) -> DocId {
    std::fs::copy(fixture("hello.pdf"), path).unwrap();
    let doc = open(engine, path);
    engine
        .sign(doc, signing(new_field(100.0), certify, "test"))
        .unwrap();
    engine.save(doc, path, true).unwrap();
    engine.close(doc);
    open(engine, path)
}

/// The signature checks that a tampered copy of a signed file is flagged rest on. Run with
/// `cargo test -p mp-engine --test tools -- --ignored` to write them again.
#[test]
#[ignore]
fn write_signed_fixtures() {
    let engine = Engine::start();
    let doc = signed_copy(&engine, &fixture("signed.pdf"), false);
    engine.close(doc);
    let mut bytes = std::fs::read(fixture("signed.pdf")).unwrap();
    let at = bytes.windows(5).position(|w| w == b"Hello").unwrap();
    bytes[at] = b'J';
    std::fs::write(fixture("signed-tampered.pdf"), bytes).unwrap();
}

#[test]
fn signed_fixtures_check_out() {
    let engine = Engine::start();
    let signed = open(&engine, &fixture("signed.pdf"));
    let s = engine.signatures(signed).unwrap();
    assert_eq!(s.len(), 1);
    assert!(s[0].signed && s[0].intact && !s[0].changed_after, "{s:?}");
    // Self-signed: the identity is unknown, not untrusted.
    assert_eq!(s[0].trust, Trust::Unknown);
    assert_eq!(s[0].signer, "micropdf test signer");
    assert_eq!(
        (s[0].reason.as_str(), s[0].location.as_str()),
        ("Approved", "Test bench")
    );

    let tampered = open(&engine, &fixture("signed-tampered.pdf"));
    let s = engine.signatures(tampered).unwrap();
    assert!(s[0].signed && !s[0].intact, "{s:?}");
}

#[test]
fn signatures_sign_count_later_changes_and_certify() {
    let engine = Engine::start();
    let scratch = Scratch::new("sign");
    let path = scratch.file("signed.pdf");
    std::fs::copy(fixture("hello.pdf"), &path).unwrap();
    let doc = open(&engine, &path);
    let wrong = engine.sign(doc, signing(new_field(100.0), false, "nope"));
    assert!(wrong.unwrap_err().to_string().contains("password"));
    engine.close(doc);

    let doc = signed_copy(&engine, &path, false);
    // A second signature, appended: the first stays intact and shows the change after it.
    engine
        .sign(doc, signing(new_field(150.0), false, "test"))
        .unwrap();
    engine.save(doc, &path, true).unwrap();
    engine.close(doc);
    let doc = open(&engine, &path);
    let s = engine.signatures(doc).unwrap();
    assert_eq!(s.len(), 2);
    assert!(s.iter().all(|s| s.signed && s.intact), "{s:?}");
    assert_eq!(
        s.iter().map(|s| s.changed_after).collect::<Vec<_>>(),
        [true, false]
    );
    assert!(s.iter().all(|s| !s.certifies));

    let certified = signed_copy(&engine, &scratch.file("certified.pdf"), true);
    let s = engine.signatures(certified).unwrap();
    assert!(s[0].certifies && s[0].intact, "{s:?}");
}

/// Writes fixtures/scanned.pdf: hello.pdf's page as a 150 dpi picture with no text, as a
/// scanner makes it, and on page 2 the same picture fed in 3 degrees askew.
#[test]
#[ignore]
fn write_scanned_fixture() {
    use mupdf::pdf::{PdfDocument, PdfWriteOptions};
    use mupdf::{Colorspace, Document, Image, Matrix};
    let hello = Document::open(fixture("hello.pdf").to_str().unwrap()).unwrap();
    let scale = 150.0 / 72.0;
    let pixmap = hello
        .load_page(0)
        .unwrap()
        .to_pixmap(
            &Matrix::new_scale(scale, scale),
            &Colorspace::device_gray(),
            false,
            false,
        )
        .unwrap();
    let mut pdf = PdfDocument::new();
    let image = pdf
        .add_image(&Image::from_pixmap(&pixmap).unwrap())
        .unwrap();
    let (sin, cos) = 3f32.to_radians().sin_cos();
    let askew = format!(
        "q {cos} {sin} {} {cos} {} {} cm ",
        -sin,
        150.0 - (150.0 * cos - 100.0 * sin),
        100.0 - (150.0 * sin + 100.0 * cos)
    );
    for turn in ["", askew.as_str()] {
        let mut page = pdf.new_page((300.0, 200.0)).unwrap();
        let mut xobjects = pdf.new_dict().unwrap();
        xobjects.dict_put("Scan", image.clone()).unwrap();
        page.resources()
            .unwrap()
            .dict_put("XObject", xobjects)
            .unwrap();
        let ops = format!(
            "{turn}q 300 0 0 200 0 0 cm /Scan Do Q\n{}",
            if turn.is_empty() { "" } else { "Q\n" }
        );
        page.insert_contents(&mut pdf, ops.as_bytes(), true)
            .unwrap();
    }
    let mut options = PdfWriteOptions::default();
    options.set_compress(true).set_garbage_level(1);
    pdf.save_with_options(fixture("scanned.pdf").to_str().unwrap(), options)
        .unwrap();
}

/// The box around the letters of a document's first page.
fn ink(engine: &Engine, doc: DocId) -> Rect {
    let text = mp_engine::page_text(&engine.display_list(doc, 0).unwrap()).unwrap();
    text.chars
        .iter()
        .filter(|c| !c.ch.is_whitespace())
        .map(|c| c.rect)
        .reduce(|a, b| Rect {
            x0: a.x0.min(b.x0),
            y0: a.y0.min(b.y0),
            x1: a.x1.max(b.x1),
            y1: a.y1.max(b.y1),
        })
        .unwrap()
}

#[test]
fn ocr_makes_a_scanned_page_searchable() {
    if mp_engine::ocr_languages().is_empty() {
        eprintln!("skipped: Windows has no text recognition language installed");
        return;
    }
    let engine = Engine::start();
    let scratch = Scratch::new("ocr");
    let path = scratch.file("scanned.pdf");
    std::fs::copy(fixture("scanned.pdf"), &path).unwrap();
    let doc = open(&engine, &path);
    assert!(texts(&engine, doc)[0].trim().is_empty());

    let how = Recognize {
        skip_text: true,
        ..Recognize::default()
    };
    let mut seen = Vec::new();
    let pages = engine
        .recognize_text(doc, &[0], &how, |done, total| {
            seen.push((done, total));
            true
        })
        .unwrap();
    assert_eq!((pages, seen), (1, vec![(0, 1), (1, 1)]));
    let text = texts(&engine, doc)[0].to_lowercase();
    assert!(text.contains("hello micropdf"), "{text}");
    // The words lie where the picture shows them: across, give or take a few points, and up
    // and down within the font's boxes, which reach above the letters.
    let hello = open(&engine, &fixture("hello.pdf"));
    let (want, got) = (ink(&engine, hello), ink(&engine, doc));
    assert!(
        (want.x0 - got.x0).abs() < 5.0
            && (want.x1 - got.x1).abs() < 5.0
            && got.y0 > want.y0 - 3.0
            && got.y1 < want.y1 + 3.0,
        "{want:?} {got:?}"
    );

    // The page has text now, so it is skipped; undo takes the layer away.
    assert_eq!(
        engine.recognize_text(doc, &[0], &how, |_, _| true).unwrap(),
        0
    );
    engine.save(doc, &path, true).unwrap();
    engine.undo(doc).unwrap();
    assert!(texts(&engine, doc)[0].trim().is_empty());
    engine.close(doc);
    let doc = open(&engine, &path);
    assert!(
        texts(&engine, doc)[0]
            .to_lowercase()
            .contains("hello micropdf")
    );

    // Page 2 is askew: straightening turns it level, so its ink is as tall as page 1's.
    let (level, before) = (ink_height(&engine, doc, 0), ink_height(&engine, doc, 1));
    assert!(before > level + 2.0, "{level} {before}");
    let how = Recognize {
        deskew: true,
        ..Recognize::default()
    };
    assert_eq!(
        engine.recognize_text(doc, &[1], &how, |_, _| true).unwrap(),
        1
    );
    let after = ink_height(&engine, doc, 1);
    assert!((after - level).abs() < 1.0, "{level} {after}");
    let text = mp_engine::page_text(&engine.display_list(doc, 1).unwrap()).unwrap();
    assert!(
        text.text(0..text.chars.len())
            .to_lowercase()
            .contains("hello micropdf")
    );
}

/// How tall, in points, the dark pixels of a page reach.
fn ink_height(engine: &Engine, doc: DocId, page: usize) -> f32 {
    let image = mp_engine::render(&engine.display_list(doc, page).unwrap(), 2.0).unwrap();
    let rows: Vec<usize> = image
        .rgb
        .chunks(image.width as usize * 3)
        .enumerate()
        .filter(|(_, row)| row.iter().any(|&v| v < 160))
        .map(|(y, _)| y)
        .collect();
    (rows.last().unwrap() - rows[0]) as f32 / 2.0
}

#[test]
fn compare_finds_the_words_added() {
    let engine = Engine::start();
    let scratch = Scratch::new("compare");
    let path = scratch.file("draft.pdf");
    std::fs::copy(fixture("hello.pdf"), &path).unwrap();
    let new = open(&engine, &path);
    let header = Overlay {
        texts: vec![(Place::TopLeft, "Second draft".into())],
        ..Overlay::default()
    };
    engine
        .stamp_pages(new, vec![0], header, "Add header")
        .unwrap();
    let old = open(&engine, &fixture("hello.pdf"));
    let c = engine.compare(old, new).unwrap();
    assert_eq!(c.changes.len(), 1, "{:?}", c.changes);
    let change = &c.changes[0];
    assert!(change.old.is_empty());
    let added: Vec<&str> = c.new[change.new.clone()]
        .iter()
        .map(|w| w.text.as_str())
        .collect();
    assert_eq!(added, ["Second", "draft"]);
    assert!(c.new[change.new.start].rect.y1 < 50.0);
    assert!(engine.compare(old, old).unwrap().changes.is_empty());
}

#[test]
fn measurements_take_the_page_scale_and_keep_it() {
    use mp_engine::{AnnotKind, Measure, NewAnnot, Style};
    use mupdf::pdf::{PdfDocument, PdfObject, PdfWriteOptions};
    let engine = Engine::start();
    let scratch = Scratch::new("measure");
    let path = scratch.file("plan.pdf");
    // hello.pdf, drawn at 1 in = 10 ft as its page's viewport says.
    {
        let pdf = PdfDocument::open(fixture("hello.pdf").to_str().unwrap()).unwrap();
        let dict = || pdf.new_dict().unwrap();
        let list = |item| {
            let mut a = pdf.new_array().unwrap();
            a.array_push(item).unwrap();
            a
        };
        let mut x = dict();
        x.dict_put("U", PdfObject::new_string("ft").unwrap())
            .unwrap();
        x.dict_put("C", PdfObject::new_real(10.0 / 72.0).unwrap())
            .unwrap();
        let mut m = dict();
        m.dict_put("Subtype", PdfObject::new_name("RL").unwrap())
            .unwrap();
        m.dict_put("R", PdfObject::new_string("1 in = 10 ft").unwrap())
            .unwrap();
        m.dict_put("X", list(x)).unwrap();
        let mut view = dict();
        view.dict_put("Measure", m).unwrap();
        pdf.find_page(0)
            .unwrap()
            .dict_put("VP", list(view))
            .unwrap();
        pdf.save_with_options(path.to_str().unwrap(), PdfWriteOptions::default())
            .unwrap();
    }
    let doc = open(&engine, &path);
    let scale = engine.page_scale(doc, 0).unwrap().unwrap();
    assert_eq!(
        (scale.ratio.as_str(), scale.unit.as_str()),
        ("1 in = 10 ft", "ft")
    );
    let square = [(20.0, 20.0), (92.0, 20.0), (92.0, 164.0), (20.0, 164.0)];
    let style = Style {
        color: [0.85, 0.15, 0.15],
        author: String::new(),
    };
    for (kind, points, label) in [
        (Measure::Distance, &square[..2], "10 ft"),
        (Measure::Perimeter, &square[..], "40 ft"),
        (Measure::Area, &square[..], "200 sq ft"),
    ] {
        let new = NewAnnot::Measure {
            kind,
            points: points.to_vec(),
            scale: scale.clone(),
        };
        let a = engine.add_annotation(doc, 0, new, style.clone()).unwrap();
        assert_eq!((a.kind, a.contents.as_str()), (AnnotKind::Measure, label));
    }
    let saved = scratch.file("measured.pdf");
    engine.save(doc, &saved, false).unwrap();
    for needle in [
        "/LineDimension",
        "/PolyLineDimension",
        "/PolygonDimension",
        "sq ft",
    ] {
        assert!(anywhere(&saved, needle), "{needle} is not in the file");
    }
    let again = open(&engine, &saved);
    let kinds: Vec<AnnotKind> = engine
        .annotations(again, 0)
        .unwrap()
        .iter()
        .map(|a| a.kind)
        .collect();
    assert_eq!(kinds, [AnnotKind::Measure; 3]);
}

#[test]
fn edited_text_reads_back_and_changes_only_its_box() {
    let engine = Engine::start();
    let scratch = Scratch::new("edit");
    let path = scratch.file("hello.pdf");
    std::fs::copy(fixture("hello.pdf"), &path).unwrap();
    let doc = open(&engine, &path);
    let blocks = engine.text_blocks(doc, 0).unwrap();
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    assert_eq!(blocks[0].text, "Hello micropdf");
    let r = blocks[0].rect;
    let shot = || mp_engine::render(&engine.display_list(doc, 0).unwrap(), 1.0).unwrap();
    let before = shot();
    let replaced = engine
        .replace_text(doc, 0, r, "Hello world".into())
        .unwrap();
    assert!(replaced.own, "{replaced:?}");
    assert_eq!(texts(&engine, doc)[0].trim(), "Hello world");
    let after = shot();
    let pixels = |image: &mp_engine::PageImage| image.rgb.as_chunks::<3>().0.to_vec();
    for (i, (a, b)) in pixels(&before).iter().zip(pixels(&after)).enumerate() {
        let (x, y) = (
            (i as u32 % before.width) as f32,
            (i as u32 / before.width) as f32,
        );
        let inside = x >= r.x0 - 2.0 && x <= r.x1 + 2.0 && y >= r.y0 - 2.0 && y <= r.y1 + 2.0;
        assert!(*a == b || inside, "pixel {x}, {y} changed outside {r:?}");
    }

    // Longer text wraps to the block's width, line under line.
    engine.undo(doc).unwrap();
    let long = "Hello micropdf, set again in place and wrapped";
    engine.replace_text(doc, 0, r, long.into()).unwrap();
    let lines: Vec<String> = texts(&engine, doc)[0].lines().map(str::to_owned).collect();
    assert!(lines.len() > 1, "{lines:?}");
    assert_eq!(lines.join(" "), long);
    let saved = scratch.file("edited.pdf");
    engine.save(doc, &saved, false).unwrap();
    let again = open(&engine, &saved);
    assert_eq!(
        texts(&engine, again)[0]
            .lines()
            .collect::<Vec<_>>()
            .join(" "),
        long
    );
}
