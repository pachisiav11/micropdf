use std::path::PathBuf;

use mp_engine::{Engine, Error, RenderPool};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

#[test]
fn opens_and_counts_pages() {
    let engine = Engine::start();
    let info = engine.open(fixture("hello.pdf")).unwrap();
    assert_eq!(info.page_count, 1);
}

#[test]
fn reports_page_sizes_in_points() {
    let engine = Engine::start();
    let doc = engine.open(fixture("hello.pdf")).unwrap();
    let sizes = engine.page_sizes(doc.id).unwrap();
    assert_eq!(sizes.len(), 1);
    assert_eq!((sizes[0].width, sizes[0].height), (300.0, 200.0));
}

#[test]
fn display_lists_are_cached() {
    let engine = Engine::start();
    let doc = engine.open(fixture("hello.pdf")).unwrap();
    let first = engine.display_list(doc.id, 0).unwrap();
    let second = engine.display_list(doc.id, 0).unwrap();
    assert!(std::sync::Arc::ptr_eq(&first, &second));
}

#[test]
fn missing_file_is_an_error() {
    let engine = Engine::start();
    assert!(matches!(
        engine.open(fixture("does-not-exist.pdf")),
        Err(Error::MuPdf(_))
    ));
}

#[test]
fn renders_page_at_requested_scale() {
    let engine = Engine::start();
    let pool = RenderPool::new(2);
    let doc = engine.open(fixture("hello.pdf")).unwrap();
    let list = engine.display_list(doc.id, 0).unwrap();
    let image = pool.render(list, 2.0).recv().unwrap().unwrap();

    // 300x200 pt page at 2x.
    assert_eq!((image.width, image.height), (600, 400));
    assert_eq!(image.rgb.len(), 600 * 400 * 3);

    // White paper with dark text: both must be present.
    let dark = image
        .rgb
        .chunks(3)
        .filter(|p| p.iter().all(|&c| c < 64))
        .count();
    let white = image
        .rgb
        .chunks(3)
        .filter(|p| p.iter().all(|&c| c > 250))
        .count();
    assert!(
        dark > 500,
        "expected rendered text, found {dark} dark pixels"
    );
    assert!(
        white > 200_000,
        "expected white paper, found {white} white pixels"
    );
}

#[test]
fn closed_document_rejects_new_requests_but_old_lists_still_render() {
    let engine = Engine::start();
    let pool = RenderPool::new(1);
    let doc = engine.open(fixture("hello.pdf")).unwrap();
    let list = engine.display_list(doc.id, 0).unwrap();

    engine.close(doc.id);

    assert!(matches!(
        engine.display_list(doc.id, 0),
        Err(Error::UnknownDocument)
    ));
    assert!(pool.render(list, 1.0).recv().unwrap().is_ok());
}

#[test]
fn repairs_a_file_without_xref() {
    let engine = Engine::start();
    assert_eq!(engine.open(fixture("truncated.pdf")).unwrap().page_count, 1);
}

#[test]
fn rejects_a_file_that_is_not_a_pdf() {
    let engine = Engine::start();
    assert!(engine.open(fixture("not-a-pdf.pdf")).is_err());
}

#[test]
fn opens_multi_page_fixture() {
    let engine = Engine::start();
    assert_eq!(
        engine
            .open(fixture("outline-links.pdf"))
            .unwrap()
            .page_count,
        3
    );
}

/// Fuzzing found a page 10^20 points wide, which took minutes and gigabytes to draw whole.
#[test]
fn a_huge_page_box_is_read_quickly() {
    let data = std::fs::read(fixture("cjk.pdf")).unwrap();
    let text = String::from_utf8_lossy(&data).replace(
        "/MediaBox [0 0 300 360]",
        "/MediaBox [99999999999999999999 0 300 360]",
    );
    let path = std::env::temp_dir().join("micropdf-huge-box.pdf");
    std::fs::write(&path, text.as_bytes()).unwrap();
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let engine = Engine::start();
        let doc = engine.open(&path).unwrap().id;
        let list = engine.display_list(doc, 0).unwrap();
        assert!(mp_engine::render(&list, 0.5).is_err());
        let tile = mp_engine::Tile {
            x: 0,
            y: 0,
            width: 256,
            height: 256,
        };
        mp_engine::render_tile(&list, 0.5, 0, tile).unwrap();
        engine.words(doc).unwrap();
        done.send(()).unwrap();
    });
    finished
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the page took too long");
}
