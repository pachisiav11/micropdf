//! The second pane: another document, or the same one, beside the main view, scrolling on its
//! own or with it; and Compare, which marks the words that differ between the two.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use mp_engine::{Comparison, DocId, PageImage, Tile};
use slint::{ComponentHandle, Image, Model, ModelRc, Rgb8Pixel, SharedPixelBuffer, VecModel};

use crate::viewer::{self, App, file_name};
use crate::{MainWindow, SplitMark, SplitPage};

/// Space around and between the pages, in logical pixels.
const GAP: f32 = 8.0;

pub struct Split {
    pub doc: DocId,
    /// Opened for the pane and closed with it; false when it shows a tab's document.
    own: bool,
    sizes: Vec<(f32, f32)>,
    synced: bool,
    overlay: bool,
    /// Each page's top and height in the pane, laid out for `width`.
    frames: Vec<(f32, f32)>,
    width: f32,
    shown: Vec<usize>,
    images: HashMap<usize, Image>,
    inflight: HashSet<usize>,
    generation: u64,
    /// The pane's last scroll position, the program's moves included, so a scroll event that
    /// does not move it is told apart from the user's scrolling.
    seen: f32,
    /// Where the pane last scrolled the main view, so the main view does not lead it back.
    led: Option<(usize, f32)>,
    pages: Rc<VecModel<SplitPage>>,
    marks: Rc<VecModel<SplitMark>>,
    pub compare: Option<Compared>,
}

pub struct Compared {
    /// The document compared against: the main view's when Compare ran.
    pub old: DocId,
    pub result: Comparison,
    pub current: Option<usize>,
}

impl Split {
    /// The pane's scale for `page`: points to logical pixels.
    fn scale(&self, page: usize) -> f32 {
        (self.width - 2.0 * GAP).max(1.0) / self.sizes[page].0.max(1.0)
    }
}

/// Second-pane and Compare command `id`; false if it is not one.
pub fn command(app: &mut App, id: &str) -> bool {
    match id {
        "split-same" => {
            if let Some((doc, path, ..)) = app.reading() {
                show(app, doc, false, file_name(&path));
            }
        }
        "split-file" => pick(app, false),
        "compare" => pick(app, true),
        "split-close" => close(app),
        "split-sync" => {
            if let Some(s) = app.split.as_mut() {
                s.synced = !s.synced;
                let synced = s.synced;
                if let Some(w) = app.window() {
                    w.set_split_synced(synced);
                }
                follow(app);
            }
        }
        "compare-overlay" => {
            if let Some(s) = app.split.as_mut() {
                s.overlay = !s.overlay;
                let overlay = s.overlay;
                restart(s);
                if let Some(w) = app.window() {
                    w.set_split_overlay(overlay);
                }
                view(app);
            }
        }
        "compare-next" => step(app, 1),
        "compare-prev" => step(app, -1),
        _ => return false,
    }
    true
}

/// Asks for a PDF to show in the pane, and to compare with the main view's when `compare`.
fn pick(app: &mut App, compare: bool) {
    let Some((old, path, ..)) = app.reading() else {
        return;
    };
    let title = if compare {
        "Compare with"
    } else {
        "Show beside this document"
    };
    crate::tools::pick_files(title, Some(path), false, crate::tools::PDFS, move |files| {
        let file = files[0].clone();
        let _ = slint::invoke_from_event_loop(move || {
            viewer::with(|app| open_file(app, file, compare.then_some(old)));
        });
    });
}

/// Opens `path` in the second pane, and compares it with `compare_with` when given.
pub fn open_file(app: &mut App, path: PathBuf, compare_with: Option<DocId>) {
    let engine = app.engine();
    let info = match engine.open(&path) {
        Ok(info) if info.needs_password => {
            engine.close(info.id);
            app.message(
                "Could not open the file",
                format!(
                    "{} needs a password. Open it in a tab first.",
                    file_name(&path)
                ),
            );
            return;
        }
        Ok(info) => info,
        Err(e) => {
            app.message(
                "Could not open the file",
                format!("{}\n\n{e}", path.display()),
            );
            return;
        }
    };
    let new = info.id;
    show(app, new, true, file_name(&path));
    let Some(old) = compare_with else { return };
    app.status("Comparing\u{2026}".into());
    std::thread::spawn(move || {
        let result = engine.compare(old, new);
        let _ = slint::invoke_from_event_loop(move || {
            viewer::with(|app| match result {
                Ok(result) => compared(app, old, new, result),
                Err(_) if !app.split.as_ref().is_some_and(|s| s.doc == new) => {}
                Err(e) => app.message("Could not compare", e.to_string()),
            });
        });
    });
}

/// Shows `doc` in the second pane: a tab's document, or one opened for the pane when `own`.
pub fn show(app: &mut App, doc: DocId, own: bool, title: String) {
    close(app);
    let Ok(sizes) = app.engine().page_sizes(doc) else {
        return;
    };
    let Some(window) = app.window() else { return };
    let split = Split {
        doc,
        own,
        sizes: sizes.iter().map(|s| (s.width, s.height)).collect(),
        synced: false,
        overlay: false,
        frames: Vec::new(),
        width: 0.0,
        shown: Vec::new(),
        images: HashMap::new(),
        inflight: HashSet::new(),
        generation: app.bump(),
        seen: 0.0,
        led: None,
        pages: Rc::new(VecModel::default()),
        marks: Rc::new(VecModel::default()),
        compare: None,
    };
    window.set_split_pages(ModelRc::from(split.pages.clone()));
    window.set_split_marks(ModelRc::from(split.marks.clone()));
    window.set_split_title(title.into());
    window.set_split_synced(false);
    window.set_split_overlay(false);
    window.set_split_comparing(false);
    window.set_split_viewport_y(0.0);
    window.set_split_open(true);
    app.split = Some(split);
    view(app);
}

pub fn close(app: &mut App) {
    let Some(split) = app.split.take() else {
        return;
    };
    if split.own {
        app.engine().close(split.doc);
    }
    if let Some(w) = app.window() {
        w.set_split_open(false);
        w.set_split_comparing(false);
        w.set_split_pages(ModelRc::default());
        w.set_split_marks(ModelRc::default());
    }
    app.refresh_marks();
}

/// Closes the pane when `doc` goes away from under it.
pub fn closing(app: &mut App, doc: DocId) {
    let gone = app.split.as_ref().is_some_and(|s| {
        (!s.own && s.doc == doc) || s.compare.as_ref().is_some_and(|c| c.old == doc)
    });
    if gone {
        close(app);
    }
}

/// Draws the pane again after `doc` was edited, if it shows it.
pub fn edited(app: &mut App, doc: DocId) {
    let engine = app.engine();
    let Some(s) = app.split.as_mut() else { return };
    let compared = s.compare.as_ref().is_some_and(|c| c.old == doc);
    if s.doc != doc && !(compared && s.overlay) {
        return;
    }
    if let Ok(sizes) = engine.page_sizes(s.doc) {
        s.sizes = sizes.iter().map(|s| (s.width, s.height)).collect();
    }
    s.width = 0.0;
    view(app);
}

/// Forgets what was rendered, for a new look.
fn restart(s: &mut Split) {
    s.images.clear();
    s.inflight.clear();
    s.generation += 1;
}

/// Lays the pane out for its width and renders the pages in view.
pub fn view(app: &mut App) {
    let Some(window) = app.window() else { return };
    let Some(s) = app.split.as_mut() else { return };
    let (vw, vh) = (
        window.get_split_view_width(),
        window.get_split_view_height(),
    );
    if vw < 40.0 || vh < 20.0 {
        return;
    }
    if (vw - s.width).abs() > 0.5 {
        s.width = vw;
        let mut y = GAP;
        s.frames = (0..s.sizes.len())
            .map(|p| {
                let h = s.sizes[p].1 * s.scale(p);
                let frame = (y, h);
                y += h + GAP;
                (frame.0, frame.1)
            })
            .collect();
        window.set_split_content_height(y);
        restart(s);
    }
    let top = -window.get_split_viewport_y();
    let (from, to) = (top - vh, top + 2.0 * vh);
    s.shown = (0..s.frames.len())
        .filter(|&p| s.frames[p].0 + s.frames[p].1 >= from && s.frames[p].0 <= to)
        .collect();
    let keep: HashSet<usize> = s.shown.iter().copied().collect();
    s.images.retain(|p, _| keep.contains(p));

    let dpr = window.window().scale_factor();
    let old = s.compare.as_ref().filter(|_| s.overlay).map(|c| c.old);
    let wanted: Vec<usize> = s
        .shown
        .iter()
        .copied()
        .filter(|p| !s.images.contains_key(p) && !s.inflight.contains(p))
        .collect();
    let (doc, generation) = (s.doc, s.generation);
    let width = ((s.width - 2.0 * GAP) * dpr).round().max(1.0) as i32;
    let jobs: Vec<(usize, i32, f32)> = wanted
        .into_iter()
        .map(|p| {
            s.inflight.insert(p);
            let height = (s.frames[p].1 * dpr).round().max(1.0) as i32;
            (p, height, s.scale(p) * dpr)
        })
        .collect();
    let engine = app.engine();
    for (p, height, scale) in jobs {
        let engine = engine.clone();
        app.pool().spawn(move || {
            let render = |doc: DocId, page: usize| -> Option<PageImage> {
                let list = engine.display_list(doc, page).ok()?;
                let tile = Tile {
                    x: 0,
                    y: 0,
                    width,
                    height,
                };
                mp_engine::render_tile(&list, scale, 0, tile).ok()
            };
            let image = render(doc, p).map(|new| match old.and_then(|old| render(old, p)) {
                Some(old) => overlay(&old, &new),
                None => new.rgb,
            });
            let buffer = image.map(|rgb| {
                SharedPixelBuffer::<Rgb8Pixel>::clone_from_slice(&rgb, width as u32, height as u32)
            });
            let _ = slint::invoke_from_event_loop(move || {
                viewer::with(|app| done(app, generation, p, buffer));
            });
        });
    }
    refresh(app);
}

/// Both versions of a page on one: unchanged ink faded, ink taken out red, ink put in green.
fn overlay(old: &PageImage, new: &PageImage) -> Vec<u8> {
    let w = new.width as usize;
    let light = |img: &PageImage, x: usize, y: usize| -> i32 {
        if x >= img.width as usize || y >= img.height as usize {
            return 255;
        }
        let i = (y * img.width as usize + x) * 3;
        (img.rgb[i] as i32 * 3 + img.rgb[i + 1] as i32 * 6 + img.rgb[i + 2] as i32) / 10
    };
    let mut out = Vec::with_capacity(new.rgb.len());
    for y in 0..new.height as usize {
        for x in 0..w {
            let (a, b) = (light(old, x, y), light(new, x, y));
            out.extend_from_slice(&if (a - b).abs() < 48 {
                let v = (255 - (255 - b) / 3) as u8;
                [v, v, v]
            } else if a < b {
                [214, 48, 49]
            } else {
                [32, 150, 72]
            });
        }
    }
    out
}

fn done(app: &mut App, generation: u64, page: usize, buffer: Option<SharedPixelBuffer<Rgb8Pixel>>) {
    let Some(s) = app.split.as_mut() else { return };
    if s.generation != generation {
        return;
    }
    s.inflight.remove(&page);
    if let Some(buffer) = buffer
        && s.shown.contains(&page)
    {
        s.images.insert(page, Image::from_rgb8(buffer));
    }
    refresh(app);
}

/// Fills the pane's page and mark models for the pages in view.
fn refresh(app: &mut App) {
    let Some(s) = app.split.as_ref() else { return };
    let items: Vec<SplitPage> = s
        .shown
        .iter()
        .map(|&p| SplitPage {
            y: s.frames[p].0,
            width: s.width - 2.0 * GAP,
            height: s.frames[p].1,
            image: s.images.get(&p).cloned().unwrap_or_default(),
        })
        .collect();
    set_vec(&s.pages, items);
    let mut marks = Vec::new();
    if let Some(c) = &s.compare {
        let shown: HashSet<usize> = s.shown.iter().copied().collect();
        for (i, change) in c.result.changes.iter().enumerate() {
            for w in &c.result.new[change.new.clone()] {
                if !shown.contains(&w.page) {
                    continue;
                }
                let k = s.scale(w.page);
                marks.push(SplitMark {
                    x: GAP + w.rect.x0 * k - 1.0,
                    y: s.frames[w.page].0 + w.rect.y0 * k - 1.0,
                    width: (w.rect.x1 - w.rect.x0) * k + 2.0,
                    height: (w.rect.y1 - w.rect.y0) * k + 2.0,
                    kind: if c.current == Some(i) { 1 } else { 0 },
                });
            }
        }
    }
    set_vec(&s.marks, marks);
}

/// Replaces a model's rows, row by row where the count holds, so Slint keeps its items.
fn set_vec<T: Clone + 'static>(model: &VecModel<T>, items: Vec<T>) {
    if model.row_count() == items.len() {
        for (i, item) in items.into_iter().enumerate() {
            model.set_row_data(i, item);
        }
    } else {
        model.set_vec(items);
    }
}

/// The pane was scrolled or resized: follow the user's scrolling in the main view when synced.
pub fn scrolled(app: &mut App) {
    let Some(window) = app.window() else { return };
    let Some(s) = app.split.as_mut() else { return };
    let y = -window.get_split_viewport_y();
    let moved = (y - s.seen).abs() >= 0.5;
    s.seen = y;
    if !moved {
        // Resized: the pages moved under the view, so it finds the main view's place again.
        s.led = None;
        view(app);
        follow(app);
        return;
    }
    let target = (s.synced && !s.frames.is_empty()).then(|| {
        let page = s
            .frames
            .partition_point(|f| f.0 + f.1 < y)
            .min(s.frames.len() - 1);
        (page, (y - s.frames[page].0) / s.frames[page].1.max(1.0))
    });
    view(app);
    if let Some((page, frac)) = target {
        lead(app, |app| app.go_to_fraction(page, frac));
    }
}

/// Moves the main view with `go` without the pane following it back.
fn lead(app: &mut App, go: impl FnOnce(&mut App)) {
    let synced = app.split.as_ref().is_some_and(|s| s.synced);
    if let Some(s) = app.split.as_mut() {
        s.synced = false;
    }
    go(app);
    let led = app.reading_fraction();
    if let Some(s) = app.split.as_mut() {
        s.synced = synced;
        s.led = led;
    }
}

/// Scrolls the pane to where the main view is, when synced.
pub fn follow(app: &mut App) {
    if !app.split.as_ref().is_some_and(|s| s.synced) {
        return;
    }
    let Some((page, frac)) = app.reading_fraction() else {
        return;
    };
    let Some(window) = app.window() else { return };
    let Some(s) = app.split.as_mut() else { return };
    if s.led
        .is_some_and(|l| l.0 == page && (l.1 - frac).abs() < 1e-3)
    {
        return;
    }
    s.led = None;
    let Some(&(top, height)) = s.frames.get(page.min(s.frames.len().saturating_sub(1))) else {
        return;
    };
    place(&window, s, top + frac * height);
}

/// Scrolls the pane to `y`, as far as it goes.
fn place(window: &MainWindow, s: &mut Split, y: f32) {
    let end = (window.get_split_content_height() - window.get_split_view_height()).max(0.0);
    let y = y.clamp(0.0, end);
    if (y - s.seen).abs() < 0.5 {
        return;
    }
    s.seen = y;
    window.set_split_viewport_y(-y);
}

fn compared(app: &mut App, old: DocId, new: DocId, result: Comparison) {
    let Some(s) = app.split.as_mut().filter(|s| s.doc == new) else {
        return;
    };
    let (mut added, mut removed, mut replaced) = (0, 0, 0);
    for c in &result.changes {
        match (c.old.is_empty(), c.new.is_empty()) {
            (true, _) => added += 1,
            (_, true) => removed += 1,
            _ => replaced += 1,
        }
    }
    let n = result.changes.len();
    s.compare = Some(Compared {
        old,
        result,
        current: None,
    });
    s.synced = true;
    if let Some(w) = app.window() {
        w.set_split_comparing(true);
        w.set_split_synced(true);
        let title = w.get_split_title();
        w.set_split_title(format!("{title}: {n} change{}", if n == 1 { "" } else { "s" }).into());
    }
    if n == 0 {
        app.status("The two documents have the same text".into());
        refresh(app);
        return;
    }
    step(app, 1);
    app.status(format!(
        "{n} change{}: {replaced} replaced, {added} added, {removed} taken out. Step through \
         them with the arrows above the second pane.",
        if n == 1 { "" } else { "s" }
    ));
}

/// Goes `by` changes on, in both panes.
fn step(app: &mut App, by: i32) {
    let Some(c) = app.split.as_mut().and_then(|s| s.compare.as_mut()) else {
        return;
    };
    let n = c.result.changes.len() as i32;
    if n == 0 {
        return;
    }
    let i = match c.current {
        Some(i) => (i as i32 + by).rem_euclid(n),
        None if by > 0 => 0,
        None => n - 1,
    } as usize;
    c.current = Some(i);
    let change = c.result.changes[i].clone();
    let at = |words: &[mp_engine::Word], range: &std::ops::Range<usize>| {
        words
            .get(range.start)
            .or_else(|| words.get(range.start.saturating_sub(1)))
            .map(|w| (w.page, w.rect.y0))
    };
    let (old_at, new_at) = (
        at(&c.result.old, &change.old),
        at(&c.result.new, &change.new),
    );
    let words = |words: &[mp_engine::Word], range: std::ops::Range<usize>| {
        let text = words[range]
            .iter()
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        if text.chars().count() > 60 {
            format!("{}\u{2026}", text.chars().take(60).collect::<String>())
        } else {
            text
        }
    };
    let what = match (change.old.is_empty(), change.new.is_empty()) {
        (true, _) => format!("added \u{201c}{}\u{201d}", words(&c.result.new, change.new)),
        (_, true) => format!(
            "took out \u{201c}{}\u{201d}",
            words(&c.result.old, change.old)
        ),
        _ => format!(
            "\u{201c}{}\u{201d} became \u{201c}{}\u{201d}",
            words(&c.result.old, change.old),
            words(&c.result.new, change.new)
        ),
    };
    let old = c.old;
    if let Some((page, y)) = old_at
        && app.reading().is_some_and(|r| r.0 == old)
    {
        lead(app, |app| app.go_to(page, Some((y - 60.0).max(0.0)), true));
    }
    if let (Some((page, y)), Some(window), Some(s)) = (new_at, app.window(), app.split.as_mut())
        && let Some(&(top, _)) = s.frames.get(page)
    {
        let y = top + (y - 60.0).max(0.0) * s.scale(page);
        place(&window, s, y);
    }
    app.refresh_marks();
    view(app);
    app.status(format!("Change {} of {n}: {what}", i + 1));
}
