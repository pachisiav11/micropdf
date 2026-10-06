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
