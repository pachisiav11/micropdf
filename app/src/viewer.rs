//! Continuous-scroll page view. Layout lives here, not in Slint: the model only ever holds the
//! pages near the viewport, and renders are requested for those at the current zoom.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mp_engine::{DocInfo, Engine, PageSize, RenderPool};
use slint::{ComponentHandle, Image, Rgb8Pixel, SharedPixelBuffer, VecModel};

use crate::{MainWindow, PageItem};

/// Space around the document and between pages, in logical pixels.
const MARGIN: f32 = 24.0;
const GAP: f32 = 16.0;

thread_local! {
    static VIEWER: RefCell<Option<Viewer>> = const { RefCell::new(None) };
}

/// Runs `f` on the open viewer, if any. UI thread only.
pub fn with<R>(f: impl FnOnce(&mut Viewer) -> R) -> Option<R> {
    VIEWER.with(|v| v.borrow_mut().as_mut().map(f))
}

pub fn open(window: &MainWindow, path: &Path) -> Result<(), mp_engine::Error> {
    let engine = Arc::new(Engine::start());
    let doc = engine.open(path)?;
    let sizes = engine.page_sizes(doc.id)?;
    let workers =
        std::thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(1).clamp(1, 4));

    let model = Rc::new(VecModel::default());
    window.set_pages(model.clone().into());
    window.set_title_text(
        format!(
            "{} — micropdf",
            path.file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
        )
        .into(),
    );
    window.on_view_changed(|| {
        with(Viewer::update);
    });

    let viewer = Viewer {
        window: window.as_weak(),
        engine,
        pool: RenderPool::new(workers),
        doc,
        sizes,
        zoom: 0.0,
        tops: Vec::new(),
        generation: 0,
        shared: Arc::new(Shared {
            generation: AtomicU64::new(0),
            wanted: Mutex::new(HashSet::new()),
        }),
        cache: HashMap::new(),
        inflight: HashSet::new(),
        model,
        shown: Vec::new(),
        renders: 0,
    };
    VIEWER.with(|v| *v.borrow_mut() = Some(viewer));
    with(Viewer::update);
    Ok(())
}

pub struct Viewer {
    window: slint::Weak<MainWindow>,
    engine: Arc<Engine>,
    pool: RenderPool,
    doc: DocInfo,
    sizes: Vec<PageSize>,
    /// Logical pixels per PDF point.
    zoom: f32,
    /// Top edge of each page in logical pixels.
    tops: Vec<f32>,
    /// Bumped whenever the zoom changes; renders from an older generation are discarded.
    generation: u64,
    shared: Arc<Shared>,
    cache: HashMap<usize, Rendered>,
    inflight: HashSet<usize>,
    model: Rc<VecModel<PageItem>>,
    /// What the model currently shows: (page, generation of its image), plus the zoom.
    shown: Vec<(usize, Option<u64>)>,
    renders: u64,
}

struct Rendered {
    image: Image,
    generation: u64,
}

/// Read by render workers so they can skip pages that scrolled away or were re-zoomed.
struct Shared {
    generation: AtomicU64,
    wanted: Mutex<HashSet<usize>>,
}

impl Viewer {
    pub fn page_count(&self) -> usize {
        self.doc.page_count
    }

    pub fn renders(&self) -> u64 {
        self.renders
    }

    pub fn update(&mut self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let (view_w, view_h) = (window.get_view_width(), window.get_view_height());
        if view_w <= 0.0 || view_h <= 0.0 {
            return;
        }
        let layout_changed = self.fit_width(&window, view_w);

        let top = -window.get_viewport_y();
        let visible = self.pages_between(top - view_h / 2.0, top + view_h * 1.5);
        *self.shared.wanted.lock().unwrap() = visible.clone().collect();

        let keep = self.pages_between(top - view_h * 2.0, top + view_h * 3.0);
        self.cache.retain(|page, _| keep.contains(page));

        let shown: Vec<_> = visible
            .clone()
            .map(|i| (i, self.cache.get(&i).map(|r| r.generation)))
            .collect();
        if layout_changed || shown != self.shown {
            let items: Vec<PageItem> = visible.clone().map(|i| self.item(i, view_w)).collect();
            self.model.set_vec(items);
            self.shown = shown;
        }

        for page in visible {
            self.request(page, &window);
        }

        let current = self
            .pages_between(top + view_h / 3.0, top + view_h / 3.0)
            .start;
        window.set_status_text(
            format!(
                "Page {} of {}  ·  {:.0}%",
                (current + 1).min(self.doc.page_count),
                self.doc.page_count,
                self.zoom * 100.0
            )
            .into(),
        );
    }

    /// Fits the widest page to the view. Returns true if the layout changed.
    fn fit_width(&mut self, window: &MainWindow, view_w: f32) -> bool {
        let widest = self.sizes.iter().map(|s| s.width).fold(1.0, f32::max);
        let zoom = ((view_w - 2.0 * MARGIN) / widest).clamp(0.05, 8.0);
        if self.zoom > 0.0 && (zoom - self.zoom).abs() / self.zoom < 0.002 {
            return false;
        }

        // Keep the page under the top edge in place across the re-layout.
        let top = -window.get_viewport_y();
        let anchor = (!self.tops.is_empty()).then(|| {
            let page = self.pages_between(top, top).start.min(self.tops.len() - 1);
            let height = self.sizes[page].height * self.zoom;
            (page, (top - self.tops[page]) / height)
        });

        self.zoom = zoom;
        let mut y = MARGIN;
        self.tops = self
            .sizes
            .iter()
            .map(|s| {
                let page_top = y;
                y += s.height * zoom + GAP;
                page_top
            })
            .collect();
        window.set_document_height(y - GAP + MARGIN);
        window.set_document_width(widest * zoom + 2.0 * MARGIN);
        if let Some((page, fraction)) = anchor {
            let height = self.sizes[page].height * zoom;
            window.set_viewport_y(-(self.tops[page] + fraction * height).max(0.0));
        }

        self.generation += 1;
        self.shared
            .generation
            .store(self.generation, Ordering::Relaxed);
        self.inflight.clear();
        true
    }

    /// Pages that overlap the vertical band [from, to].
    fn pages_between(&self, from: f32, to: f32) -> Range<usize> {
        let mut first = self.tops.partition_point(|&t| t < from);
        if first > 0 {
            let prev = first - 1;
            if self.tops[prev] + self.sizes[prev].height * self.zoom >= from {
                first = prev;
            }
        }
        let last = self.tops.partition_point(|&t| t <= to);
        first..last.max(first)
    }

    fn item(&self, page: usize, view_w: f32) -> PageItem {
        let size = self.sizes[page];
        let (width, height) = (size.width * self.zoom, size.height * self.zoom);
        let content_w = view_w
            .max(self.sizes.iter().map(|s| s.width).fold(0.0, f32::max) * self.zoom + 2.0 * MARGIN);
        PageItem {
            index: page as i32,
            x: (content_w - width) / 2.0,
            y: self.tops[page],
            width,
            height,
            image: self
                .cache
                .get(&page)
                .map(|r| r.image.clone())
                .unwrap_or_default(),
        }
    }

    fn request(&mut self, page: usize, window: &MainWindow) {
        let current = self
            .cache
            .get(&page)
            .is_some_and(|r| r.generation == self.generation);
        if current || !self.inflight.insert(page) {
            return;
        }

        let scale = self.zoom * window.window().scale_factor();
        let generation = self.generation;
        let (engine, shared, doc) = (
            Arc::clone(&self.engine),
            Arc::clone(&self.shared),
            self.doc.id,
        );
        self.pool.spawn(move || {
            let stale = shared.generation.load(Ordering::Relaxed) != generation
                || !shared.wanted.lock().unwrap().contains(&page);
            if stale {
                let _ = slint::invoke_from_event_loop(move || {
                    with(|v| v.skipped(page, generation));
                });
                return;
            }
            match engine
                .display_list(doc, page)
                .and_then(|list| mp_engine::render(&list, scale))
            {
                Ok(image) => {
                    let buffer = SharedPixelBuffer::<Rgb8Pixel>::clone_from_slice(
                        &image.rgb,
                        image.width,
                        image.height,
                    );
                    let _ = slint::invoke_from_event_loop(move || {
                        with(|v| v.rendered(page, generation, buffer));
                    });
                }
                Err(e) => {
                    eprintln!("page {}: {e}", page + 1);
                    let _ = slint::invoke_from_event_loop(move || {
                        with(|v| v.skipped(page, generation));
                    });
                }
            }
        });
    }

    fn rendered(&mut self, page: usize, generation: u64, buffer: SharedPixelBuffer<Rgb8Pixel>) {
        if generation != self.generation {
            return;
        }
        self.inflight.remove(&page);
        self.renders += 1;
        self.cache.insert(
            page,
            Rendered {
                image: Image::from_rgb8(buffer),
                generation,
            },
        );
        self.update();
    }

    fn skipped(&mut self, page: usize, generation: u64) {
        if generation != self.generation {
            return;
        }
        self.inflight.remove(&page);
        // It may have become wanted again between the check and now.
        if self.shared.wanted.lock().unwrap().contains(&page) {
            self.update();
        }
    }
}
