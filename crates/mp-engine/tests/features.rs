use std::path::PathBuf;

use mp_engine::{Engine, LinkTarget, Tile};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

#[test]
fn tiles_reassemble_into_the_whole_page() {
    let engine = Engine::start();
    let doc = engine.open(fixture("hello.pdf")).unwrap();
    let list = engine.display_list(doc.id, 0).unwrap();
    let whole = mp_engine::render(&list, 2.0).unwrap();
    assert_eq!(mp_engine::rendered_size(&list, 2.0, 0), (600, 400));

    // Four uneven tiles covering the 600x400 page.
    let tiles = [
        Tile {
            x: 0,
            y: 0,
            width: 256,
            height: 256,
        },
        Tile {
            x: 256,
            y: 0,
            width: 344,
            height: 256,
        },
        Tile {
            x: 0,
            y: 256,
            width: 256,
            height: 144,
        },
        Tile {
            x: 256,
            y: 256,
            width: 344,
            height: 144,
        },
    ];
    for tile in tiles {
        let image = mp_engine::render_tile(&list, 2.0, 0, tile).unwrap();
        assert_eq!(
            (image.width, image.height),
            (tile.width as u32, tile.height as u32)
        );
        for row in 0..tile.height as usize {
            let src = ((tile.y as usize + row) * 600 + tile.x as usize) * 3;
            let dst = row * tile.width as usize * 3;
            let len = tile.width as usize * 3;
            assert_eq!(
                &image.rgb[dst..dst + len],
                &whole.rgb[src..src + len],
                "tile {tile:?} row {row} differs"
            );
        }
    }
}

#[test]
fn rotation_swaps_rendered_size() {
    let engine = Engine::start();
    let doc = engine.open(fixture("hello.pdf")).unwrap();
    let list = engine.display_list(doc.id, 0).unwrap();
    assert_eq!(mp_engine::rendered_size(&list, 1.0, 90), (200, 300));
    assert_eq!(mp_engine::rendered_size(&list, 1.0, 270), (200, 300));
    let tile = Tile {
        x: 0,
        y: 0,
        width: 200,
        height: 300,
    };
    let image = mp_engine::render_tile(&list, 1.0, 90, tile).unwrap();
    let dark = image
        .rgb
        .chunks(3)
        .filter(|p| p.iter().all(|&c| c < 64))
        .count();
    assert!(dark > 100, "rotated page should still show its text");
}

#[test]
fn extracts_text_with_boxes_inside_the_page() {
    let engine = Engine::start();
    let doc = engine.open(fixture("hello.pdf")).unwrap();
    let list = engine.display_list(doc.id, 0).unwrap();
    let text = mp_engine::page_text(&list).unwrap();
    assert_eq!(text.text(0..text.chars.len()), "Hello micropdf");
    for c in &text.chars {
        assert!(c.rect.x0 >= 0.0 && c.rect.x1 <= 300.0 && c.rect.y0 >= 0.0 && c.rect.y1 <= 200.0);
    }
    // "micropdf" starts after "Hello ".
    let m = text.chars.iter().position(|c| c.ch == 'm').unwrap();
    let near_m = text.nearest(text.chars[m].rect.x0 + 1.0, text.chars[m].rect.y0 + 1.0);
    assert_eq!(near_m, Some(m));
    assert_eq!(text.line_rects(0..text.chars.len()).len(), 1);
}

#[test]
fn searches_case_insensitively() {
    let engine = Engine::start();
    let doc = engine.open(fixture("hello.pdf")).unwrap();
    let list = engine.display_list(doc.id, 0).unwrap();
    assert_eq!(mp_engine::search(&list, "MICROPDF").unwrap().len(), 1);
    assert!(mp_engine::search(&list, "absent").unwrap().is_empty());
}

#[test]
fn reads_outline_and_links() {
    let engine = Engine::start();
    let doc = engine.open(fixture("outline-links.pdf")).unwrap();

    let outline = engine.outline(doc.id).unwrap();
    let titles: Vec<_> = outline.iter().map(|o| o.title.as_str()).collect();
    assert_eq!(titles, ["Chapter 1", "Chapter 2", "Chapter 3"]);
    let pages: Vec<_> = outline
        .iter()
        .map(|o| match o.target {
            Some(LinkTarget::Page { page, .. }) => page,
            ref other => panic!("unexpected target {other:?}"),
        })
        .collect();
    assert_eq!(pages, [0, 1, 2]);

    let links = engine.links(doc.id, 0).unwrap();
    assert_eq!(links.len(), 2);
    assert!(
        links
            .iter()
            .any(|l| matches!(l.target, LinkTarget::Page { page: 2, .. }))
    );
    assert!(
        links
            .iter()
            .any(|l| l.target == LinkTarget::Uri("https://example.com/".into()))
    );
}

#[test]
fn unlocks_encrypted_documents() {
    let engine = Engine::start();
    let doc = engine.open(fixture("encrypted.pdf")).unwrap();
    assert!(doc.needs_password);
    assert_eq!(doc.page_count, 0);
    assert_eq!(engine.authenticate(doc.id, "wrong").unwrap(), None);
    assert_eq!(engine.authenticate(doc.id, "user").unwrap(), Some(1));
    assert!(engine.display_list(doc.id, 0).is_ok());
}

#[test]
fn reports_metadata() {
    let engine = Engine::start();
    let doc = engine.open(fixture("hello.pdf")).unwrap();
    let meta = engine.metadata(doc.id).unwrap();
    let format = meta
        .iter()
        .find(|(k, _)| k == "Format")
        .map(|(_, v)| v.as_str());
    assert_eq!(format, Some("PDF 1.7"));
}

#[test]
fn lists_and_extracts_attachments() {
    let engine = Engine::start();
    let doc = engine.open(fixture("attachment.pdf")).unwrap();
    let files = engine.attachments(doc.id).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].name, "notes.txt");
    assert_eq!(files[0].size, Some(30));
    let data = engine.attachment_data(doc.id, 0).unwrap();
    assert_eq!(data, b"Embedded by make_fixtures.py.\n");
    assert!(engine.attachment_data(doc.id, 1).is_err());

    let plain = engine.open(fixture("hello.pdf")).unwrap();
    assert!(engine.attachments(plain.id).unwrap().is_empty());
}

#[test]
fn lists_and_toggles_layers() {
    let engine = Engine::start();
    let doc = engine.open(fixture("layers.pdf")).unwrap();
    let layers = engine.layers(doc.id).unwrap();
    let state: Vec<(&str, bool)> = layers
        .iter()
        .map(|l| (l.name.as_str(), l.visible))
        .collect();
    assert_eq!(state, [("Grid", true), ("Notes", false)]);

    let has_notes = |engine: &Engine| {
        let list = engine.display_list(doc.id, 0).unwrap();
        let text = mp_engine::page_text(&list).unwrap();
        text.text(0..text.chars.len()).contains("Hidden notes")
    };
    assert!(!has_notes(&engine));
    let layers = engine.toggle_layer(doc.id, 1).unwrap();
    assert!(layers[1].visible);
    assert!(has_notes(&engine));

    let plain = engine.open(fixture("hello.pdf")).unwrap();
    assert!(engine.layers(plain.id).unwrap().is_empty());
}

#[test]
fn cjk_text_without_embedded_fonts_draws_with_system_fonts() {
    use mp_engine::CjkFontOrdering::{AdobeGb, AdobeJapan, AdobeKorea};

    let engine = Engine::start();
    let doc = engine.open(fixture("cjk.pdf")).unwrap();
    let list = engine.display_list(doc.id, 0).unwrap();
    let text = mp_engine::page_text(&list).unwrap();
    let text = text.text(0..text.chars.len());
    let lines = [
        "\u{4e2d}\u{6587}\u{6587}\u{672c}",
        "\u{65e5}\u{672c}\u{8a9e}",
        "\u{d55c}\u{ad6d}\u{c5b4}",
    ];
    for line in lines {
        assert!(text.contains(line), "{line} in {text:?}");
    }

    // The page is 300x360 pt; line n sits on baseline 300 - 90n in 36 pt type.
    let image = mp_engine::render(&list, 1.0).unwrap();
    assert_eq!((image.width, image.height), (300, 360));
    for (n, ordering) in [AdobeGb, AdobeJapan, AdobeKorea].into_iter().enumerate() {
        if !mp_engine::has_cjk(ordering) {
            eprintln!("no installed font for {ordering:?}; skipping its line");
            continue;
        }
        let baseline = 360 - (300 - 90 * n);
        let dark = (baseline - 30..baseline)
            .flat_map(|y| (40..180).map(move |x| (y * 300 + x) * 3))
            .filter(|&i| image.rgb[i] < 128)
            .count();
        assert!(dark > 300, "line {} drew only {dark} dark pixels", lines[n]);
    }
}
