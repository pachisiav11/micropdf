//! The document viewer: tabs, page tiles, navigation, selection, search and the sidebar. All
//! state lives on the UI thread in one `App`; render and search work runs on the pool and reports
//! back through `invoke_from_event_loop`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use mp_engine::{
    Annot, AnnotKind, Attachment, DocId, DocInfo, Engine, Field, FieldEdit, FieldKind, History,
    Layer, Link, LinkTarget, Mark, NewAnnot, OutlineItem, PageText, REVIEW_STATES, Rect,
    RenderPool, Restyle, STAMPS, Style, Tile, Xfa,
};
use slint::{
    ComponentHandle, Image, Model, ModelRc, Rgb8Pixel, SharedPixelBuffer, SharedString, Timer,
    TimerMode, VecModel,
};

use crate::layout::{Frame, Layout, PageMode, Params, Zoom};
use crate::palette;
use crate::recolor::ReadingMode;
use crate::settings::{SavedMark, Settings};
use crate::{
    AttachmentRow, CommentRow, InfoRow, LayerRow, MainWindow, MarkItem, OutlineRow, PageItem,
    PaletteItem, StampItem, Swatch, TabItem, Theme, ThumbItem, TileItem,
};

/// Tile edge in device pixels.
const TILE: i32 = 512;
/// Largest thumbnail, in logical pixels.
const THUMB_BOX: (f32, f32) = (140.0, 180.0);
/// Space below each thumbnail for its label; matches the row height in main.slint.
const THUMB_EXTRA: f32 = 34.0;
/// Logical pixels per point at 100%: an inch of paper is 96 logical pixels on screen.
pub const ACTUAL: f32 = 96.0 / 72.0;
const ZOOM_STEPS: &[f32] = &[
    0.1, 0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0, 6.4,
    8.0, 12.0,
];
/// Arrow-key scroll step, in logical pixels.
pub const LINE: f32 = 60.0;
const HISTORY_LIMIT: usize = 100;
const TEXT_CACHE: usize = 24;
const STATUS_TIME: Duration = Duration::from_secs(4);

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` on the app. UI thread only. Returns None (and does nothing) if the app is already
/// borrowed, which happens when a modal dialog pumps events from inside a handler.
pub fn with<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| {
        let mut app = cell.try_borrow_mut().ok()?;
        app.as_mut().map(f)
    })
}

pub fn install(app: App) {
    APP.with(|cell| *cell.borrow_mut() = Some(app));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct TileKey {
    page: usize,
    generation: u64,
    col: i32,
    row: i32,
}

struct TileImage {
    /// Edges as fractions of the page frame: x0, y0, x1, y1. Fractions keep a tile placed
    /// correctly after a zoom, so it can stand in until the sharper tile arrives.
    frac: [f32; 4],
    image: Image,
}

enum Rendered {
    Skipped,
    Failed,
    Done(SharedPixelBuffer<Rgb8Pixel>),
}

/// Read by render workers so they can skip work that scrolled away.
#[derive(Default)]
struct Shared {
    tiles: Mutex<HashSet<(DocId, TileKey)>>,
    thumbs: Mutex<HashSet<(DocId, usize, u64)>>,
    search: AtomicU64,
}

/// A reading position: the page at the top of the view and how far down it the view starts.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Spot {
    page: usize,
    /// Fraction of the page height above the view's top edge (negative in the margin above).
    frac: f32,
    /// Horizontal centre of the view as a fraction of the document width.
    x_frac: f32,
}

#[derive(Default)]
struct Search {
    query: String,
    id: u64,
    hits: Vec<(usize, Rect)>,
    current: Option<(usize, Rect)>,
    searched: usize,
    done: bool,
}

#[derive(Debug, Clone, Copy)]
struct Selection {
    page: usize,
    anchor: usize,
    focus: usize,
}

impl Selection {
    fn range(&self) -> std::ops::Range<usize> {
        self.anchor.min(self.focus)..self.anchor.max(self.focus) + 1
    }
}

struct DocTab {
    path: PathBuf,
    info: DocInfo,
    sizes: Vec<(f32, f32)>,
    mode: PageMode,
    zoom: Zoom,
    rotation: i32,
    layout: Layout,
    view: (f32, f32),
    current: usize,
    /// Applied once the layout exists.
    pending: Option<Spot>,
    saved_scroll: Option<(f32, f32)>,
    /// What tiles were rendered with: device scale, rotation, reading mode.
    signature: (u32, i32, ReadingMode),
    generation: u64,
    tiles: HashMap<TileKey, TileImage>,
    inflight: HashSet<TileKey>,
    thumb_generation: u64,
    thumb_tops: Vec<f32>,
    thumbs: HashMap<usize, Image>,
    thumbs_inflight: HashSet<usize>,
    texts: HashMap<usize, Rc<PageText>>,
    links: HashMap<usize, Rc<Vec<Link>>>,
    /// Form widgets by page; cleared on every edit, since values and calculations change.
    fields: HashMap<usize, Rc<Vec<Field>>>,
    search: Search,
    selection: Option<Selection>,
    history: Vec<Spot>,
    future: Vec<Spot>,
    outline: Vec<OutlineItem>,
    expanded: HashSet<usize>,
    outline_rows: Vec<usize>,
    outline_current: Option<usize>,
    metadata: Vec<(String, String)>,
    attachments: Vec<Attachment>,
    layers: Vec<Layer>,
    /// Edits not yet written to the file.
    dirty: bool,
    /// The file's size and time after this app last wrote it, so the watcher skips own saves.
    saved: Option<(u64, SystemTime)>,
    edits: History,
    /// `edits.position` when the file was last written; usize::MAX once that state is gone.
    saved_position: usize,
    /// Every comment in the document, by page.
    comments: Vec<(usize, Annot)>,
    /// The scan that will replace `comments`, while one runs.
    comments_scan: Option<u64>,
    /// The comment picked on the page or in the list, by page and id.
    picked: Option<(usize, i32)>,
}

impl DocTab {
    fn name(&self) -> String {
        file_name(&self.path)
    }

    /// The name with a dot in front while there are unsaved edits.
    fn title(&self) -> String {
        if self.dirty {
            format!("\u{2022} {}", self.name())
        } else {
            self.name()
        }
    }

    fn page_count(&self) -> usize {
        self.sizes.len()
    }
}

enum Ask {
    Unlock {
        info: DocInfo,
        path: PathBuf,
    },
    OpenUri(String),
    Restore(Vec<PathBuf>),
    /// Close the tab for this file and drop its edits.
    Discard(PathBuf),
    /// Reload the tab for this file and drop its edits.
    Reload(PathBuf),
    Note {
        page: usize,
        x: f32,
        y: f32,
    },
    Reply {
        page: usize,
        parent: i32,
    },
    TextBox {
        page: usize,
        rect: Rect,
    },
    Callout {
        page: usize,
        target: (f32, f32),
        rect: Rect,
    },
    /// New text for a comment.
    Comment {
        page: usize,
        id: i32,
        value: String,
    },
    Quit,
    Message,
}

/// The form field the keyboard is on.
struct FieldFocus {
    doc: DocId,
    page: usize,
    field: Field,
    /// The editor is open on it for typing.
    editing: bool,
}

struct Dialog {
    ask: Ask,
    kind: &'static str,
    title: String,
    text: String,
    ok: &'static str,
    cancel: &'static str,
}

enum PaletteAction {
    Command(&'static str),
    Page(usize),
    Recent(PathBuf),
    Outline(usize),
}

enum Drag {
    Select {
        moved: bool,
        start: (f32, f32),
    },
    Pan {
        start: (f32, f32),
        scroll: (f32, f32),
    },
    /// A shape or stroke being drawn, in page space; shapes keep only start and end.
    Draw {
        page: usize,
        points: Vec<(f32, f32)>,
    },
    Click {
        start: (f32, f32),
    },
    /// A comment being moved, or resized by a corner handle; rectangles are page space.
    Shape {
        page: usize,
        id: i32,
        /// The press, in document space and in page space.
        origin: (f32, f32),
        start: (f32, f32),
        rect: Rect,
        resize: bool,
        keep_aspect: bool,
        current: Rect,
        moved: bool,
    },
}

struct Models {
    tabs: Rc<VecModel<TabItem>>,
    pages: Rc<VecModel<PageItem>>,
    tiles: Rc<VecModel<TileItem>>,
    marks: Rc<VecModel<MarkItem>>,
    thumbs: Rc<VecModel<ThumbItem>>,
    outline: Rc<VecModel<OutlineRow>>,
    info: Rc<VecModel<InfoRow>>,
    palette: Rc<VecModel<PaletteItem>>,
    recent: Rc<VecModel<PaletteItem>>,
    attachments: Rc<VecModel<AttachmentRow>>,
    layers: Rc<VecModel<LayerRow>>,
    comments: Rc<VecModel<CommentRow>>,
}

pub struct App {
    window: slint::Weak<MainWindow>,
    engine: Arc<Engine>,
    pool: RenderPool,
    shared: Arc<Shared>,
    pub settings: Settings,
    /// False for benchmark runs: nothing is written to the settings file.
    persist: bool,
    tabs: Vec<DocTab>,
    active: Option<usize>,
    closed: Vec<PathBuf>,
    models: Models,
    next_generation: u64,
    shown_pages: Vec<PageItem>,
    shown_tiles: Vec<TileItem>,
    shown_marks: Vec<MarkItem>,
    dialogs: VecDeque<Dialog>,
    palette_actions: Vec<PaletteAction>,
    drag: Option<Drag>,
    tool: Tool,
    /// The open signature pad.
    pad: Option<Pad>,
    /// What the Sign tool places.
    placing: Option<Placing>,
    /// The standard stamp the Stamp tool places, by PDF name.
    stamp: Option<&'static str>,
    field_focus: Option<FieldFocus>,
    cursor: i32,
    status_timer: Timer,
    search_timer: Timer,
    reload_timer: Timer,
    reload_paths: HashSet<PathBuf>,
    watcher: Option<notify::RecommendedWatcher>,
    watched: HashSet<PathBuf>,
    /// State to restore when leaving presentation mode: sidebar, page mode, zoom.
    presenting: Option<(bool, PageMode, Zoom)>,
    pub vim_count: String,
    pub vim_pending: Option<char>,
    /// Text the comment list is filtered by.
    comment_filter: String,
    renders: u64,
}

impl App {
    pub fn new(window: &MainWindow, settings: Settings, persist: bool) -> App {
        let workers = std::thread::available_parallelism()
            .map_or(2, |n| n.get().saturating_sub(1).clamp(1, 4));
        let models = Models {
            tabs: Rc::new(VecModel::default()),
            pages: Rc::new(VecModel::default()),
            tiles: Rc::new(VecModel::default()),
            marks: Rc::new(VecModel::default()),
            thumbs: Rc::new(VecModel::default()),
            outline: Rc::new(VecModel::default()),
            info: Rc::new(VecModel::default()),
            palette: Rc::new(VecModel::default()),
            recent: Rc::new(VecModel::default()),
            attachments: Rc::new(VecModel::default()),
            layers: Rc::new(VecModel::default()),
            comments: Rc::new(VecModel::default()),
        };
        window.set_tabs(ModelRc::from(models.tabs.clone()));
        window.set_pages(ModelRc::from(models.pages.clone()));
        window.set_tiles(ModelRc::from(models.tiles.clone()));
        window.set_marks(ModelRc::from(models.marks.clone()));
        window.set_thumbs(ModelRc::from(models.thumbs.clone()));
        window.set_outline(ModelRc::from(models.outline.clone()));
        window.set_info(ModelRc::from(models.info.clone()));
        window.set_palette_items(ModelRc::from(models.palette.clone()));
        window.set_recent_items(ModelRc::from(models.recent.clone()));
        window.set_attachments(ModelRc::from(models.attachments.clone()));
        window.set_layers(ModelRc::from(models.layers.clone()));
        window.set_comments(ModelRc::from(models.comments.clone()));
        let stamps: Vec<StampItem> = STAMPS
            .iter()
            .map(|(name, label)| StampItem {
                name: (*name).into(),
                label: (*label).into(),
            })
            .collect();
        window.set_stamps(ModelRc::new(VecModel::from(stamps)));
        let swatches: Vec<Swatch> = SWATCHES
            .iter()
            .map(|(name, [r, g, b])| Swatch {
                name: (*name).into(),
                color: slint::Color::from_rgb_f32(*r, *g, *b),
            })
            .collect();
        window.set_swatches(ModelRc::new(VecModel::from(swatches)));

        window.global::<Theme>().set_dark(settings.dark_theme);
        window.set_vim_enabled(settings.vim);
        window.set_reading_mode(settings.reading_mode.index());
        window.set_paper(paper_color(settings.reading_mode));
        window.set_sidebar_visible(settings.sidebar);
        window.set_page_mode(mode_index(settings.page_mode));

        let app = App {
            window: window.as_weak(),
            engine: Arc::new(Engine::start()),
            pool: RenderPool::new(workers),
            shared: Arc::new(Shared::default()),
            settings,
            persist,
            tabs: Vec::new(),
            active: None,
            closed: Vec::new(),
            models,
            next_generation: 1,
            shown_pages: Vec::new(),
            shown_tiles: Vec::new(),
            shown_marks: Vec::new(),
            dialogs: VecDeque::new(),
            palette_actions: Vec::new(),
            drag: None,
            tool: Tool::Select,
            pad: None,
            placing: None,
            stamp: None,
            field_focus: None,
            cursor: 0,
            status_timer: Timer::default(),
            search_timer: Timer::default(),
            reload_timer: Timer::default(),
            reload_paths: HashSet::new(),
            watcher: None,
            watched: HashSet::new(),
            presenting: None,
            vim_count: String::new(),
            vim_pending: None,
            comment_filter: String::new(),
            renders: 0,
        };
        app.refresh_recent();
        app.refresh_sign_menu();
        app
    }

    fn window(&self) -> Option<MainWindow> {
        self.window.upgrade()
    }

    fn tab(&self) -> Option<&DocTab> {
        self.active.and_then(|i| self.tabs.get(i))
    }

    fn tab_mut(&mut self) -> Option<&mut DocTab> {
        self.active.and_then(|i| self.tabs.get_mut(i))
    }

    pub fn has_document(&self) -> bool {
        self.tab().is_some()
    }

    pub fn renders(&self) -> u64 {
        self.renders
    }

    pub fn page_count(&self) -> usize {
        self.tab().map_or(0, DocTab::page_count)
    }

    fn save_settings(&self) {
        if self.persist {
            self.settings.save();
        }
    }

    // ---------------------------------------------------------------- opening and closing

    pub fn open_paths(&mut self, paths: Vec<PathBuf>) {
        for path in paths {
            self.open(path);
        }
    }

    pub fn open(&mut self, path: PathBuf) {
        let path = std::path::absolute(&path).unwrap_or(path);
        if let Some(i) = self.tabs.iter().position(|t| same_path(&t.path, &path)) {
            self.select(i);
            return;
        }
        match self.engine.open(&path) {
            Err(e) => self.message("Could not open file", format!("{}\n\n{e}", path.display())),
            Ok(info) if info.needs_password => self.ask_password(info, path, false),
            Ok(info) => self.finish_open(path, info),
        }
    }

    fn ask_password(&mut self, info: DocInfo, path: PathBuf, retry: bool) {
        let name = file_name(&path);
        let text = if retry {
            format!("That password did not unlock {name}. Try again.")
        } else {
            format!("{name} is protected. Enter its password to open it.")
        };
        let dialog = Dialog {
            ask: Ask::Unlock { info, path },
            kind: "password",
            title: "Password required".into(),
            text,
            ok: "Open",
            cancel: "Cancel",
        };
        if retry {
            self.dialogs.push_front(dialog);
            self.show_dialog();
        } else {
            self.push_dialog(dialog);
        }
    }

    fn finish_open(&mut self, path: PathBuf, info: DocInfo) {
        let sizes: Vec<(f32, f32)> = match self.engine.page_sizes(info.id) {
            Ok(s) => s
                .iter()
                .map(|s| (s.width.max(1.0), s.height.max(1.0)))
                .collect(),
            Err(e) => {
                self.engine.close(info.id);
                self.message("Could not open file", format!("{}\n\n{e}", path.display()));
                return;
            }
        };
        if sizes.is_empty() {
            self.engine.close(info.id);
            self.message(
                "Could not open file",
                format!("{} has no pages.", path.display()),
            );
            return;
        }
        let outline = self.engine.outline(info.id).unwrap_or_default();
        let metadata = self.engine.metadata(info.id).unwrap_or_default();
        let attachments = self.engine.attachments(info.id).unwrap_or_default();
        let layers = self.engine.layers(info.id).unwrap_or_default();
        let expanded = if outline.len() <= 30 {
            (0..outline.len()).collect()
        } else {
            HashSet::new()
        };
        let pending = self
            .settings
            .positions
            .get(&path)
            .filter(|&&p| p < sizes.len())
            .map(|&page| Spot {
                page,
                frac: 0.0,
                x_frac: 0.5,
            });
        let xfa = self.engine.xfa(info.id).unwrap_or(Xfa::None);
        let generation = self.bump();
        let thumb_generation = self.bump();
        let tab = DocTab {
            path: path.clone(),
            info,
            sizes,
            mode: self.settings.page_mode,
            zoom: Zoom::FitWidth,
            rotation: 0,
            layout: Layout::default(),
            view: (0.0, 0.0),
            current: pending.map_or(0, |s| s.page),
            pending,
            saved_scroll: None,
            signature: (0, 0, ReadingMode::Normal),
            generation,
            tiles: HashMap::new(),
            inflight: HashSet::new(),
            thumb_generation,
            thumb_tops: Vec::new(),
            thumbs: HashMap::new(),
            thumbs_inflight: HashSet::new(),
            texts: HashMap::new(),
            links: HashMap::new(),
            fields: HashMap::new(),
            search: Search::default(),
            selection: None,
            history: Vec::new(),
            future: Vec::new(),
            outline,
            expanded,
            outline_rows: Vec::new(),
            outline_current: None,
            metadata,
            attachments,
            layers,
            dirty: false,
            saved: None,
            edits: History::default(),
            saved_position: 0,
            comments: Vec::new(),
            comments_scan: None,
            picked: None,
        };
        self.tabs.push(tab);
        self.scan_comments(self.tabs.len() - 1);
        self.settings.add_recent(&path);
        self.watch(&path);
        self.select(self.tabs.len() - 1);
        self.save_session();
        match xfa {
            Xfa::None => {}
            Xfa::Static => self.status(
                "This form also holds XFA data. Filling it in removes the XFA copy, so every                  reader shows your values."
                    .into(),
            ),
            Xfa::Dynamic => self.message(
                "This form needs Adobe Reader",
                format!(
                    "{} is a dynamic XFA form. Its fields exist only as Adobe XML form data,                      which micropdf cannot show, so the page holds the file's placeholder text                      instead of the form.

Open it in Adobe Acrobat Reader to fill it in.",
                    file_name(&path)
                ),
            ),
        }
    }

    fn bump(&mut self) -> u64 {
        self.next_generation += 1;
        self.next_generation
    }

    pub fn close_tab(&mut self, index: usize) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        if tab.dirty {
            let (path, name) = (tab.path.clone(), tab.name());
            self.push_dialog(Dialog {
                ask: Ask::Discard(path),
                kind: "confirm",
                title: "Close without saving?".into(),
                text: format!("{name} has changes that are not saved."),
                ok: "Close without saving",
                cancel: "Keep open",
            });
            return;
        }
        self.discard_tab(index);
    }

    fn discard_tab(&mut self, index: usize) {
        let tab = self.tabs.remove(index);
        self.settings.remember_page(&tab.path, tab.current);
        self.engine.close(tab.info.id);
        self.closed.push(tab.path.clone());
        self.unwatch(&tab.path);
        let next = match self.active {
            _ if self.tabs.is_empty() => None,
            Some(a) if a > index => Some(a - 1),
            Some(a) if a == index => Some(index.min(self.tabs.len() - 1)),
            other => other,
        };
        self.active = None;
        match next {
            Some(i) => self.select(i),
            None => self.show_empty(),
        }
        self.save_session();
    }

    fn show_empty(&mut self) {
        let Some(window) = self.window() else { return };
        self.shared.tiles.lock().unwrap().clear();
        self.shared.thumbs.lock().unwrap().clear();
        window.set_has_document(false);
        window.set_window_title("micropdf".into());
        window.set_dirty(false);
        window.set_undo_name("".into());
        window.set_redo_name("".into());
        window.set_active_tab(-1);
        window.set_page_count(0);
        window.set_page_text("".into());
        window.set_document_width(0.0);
        window.set_document_height(0.0);
        window.set_find_visible(false);
        window.set_status_right("".into());
        self.models.tabs.set_vec(Vec::new());
        self.models.thumbs.set_vec(Vec::new());
        self.models.outline.set_vec(Vec::new());
        self.models.info.set_vec(Vec::new());
        self.models.attachments.set_vec(Vec::new());
        self.models.layers.set_vec(Vec::new());
        self.models.comments.set_vec(Vec::new());
        self.shown_pages.clear();
        self.shown_tiles.clear();
        self.shown_marks.clear();
        self.models.pages.set_vec(Vec::new());
        self.models.tiles.set_vec(Vec::new());
        self.models.marks.set_vec(Vec::new());
        self.refresh_recent();
    }

    pub fn select(&mut self, index: usize) {
        let Some(window) = self.window() else { return };
        if index >= self.tabs.len() {
            return;
        }
        if let Some(old) = self.active
            && old != index
            && let Some(tab) = self.tabs.get_mut(old)
        {
            tab.saved_scroll = Some((-window.get_viewport_x(), -window.get_viewport_y()));
            // Pixels for background tabs are not worth their memory; they re-render quickly.
            tab.tiles.clear();
            tab.inflight.clear();
            tab.thumbs.clear();
            tab.thumbs_inflight.clear();
        }
        self.active = Some(index);
        self.drag = None;
        self.refresh_names();
        let tab = &mut self.tabs[index];
        window.set_has_document(true);
        window.set_window_title(format!("{} — micropdf", tab.title()).into());
        window.set_page_count(tab.page_count() as i32);
        window.set_page_mode(mode_index(tab.mode));
        window.set_find_query(tab.search.query.clone().into());
        // Force a fresh layout so the saved scroll applies to this tab's geometry.
        tab.view = (0.0, 0.0);
        self.rebuild_thumbs();
        self.rebuild_outline();
        self.rebuild_info();
        self.rebuild_files();
        self.refresh_find_status();
        self.refresh_zoom_text();
        self.update_view();
        if let Some(tab) = self.tab_mut()
            && let Some((x, y)) = tab.saved_scroll.take()
        {
            self.set_scroll(x, y);
            self.update_view();
        }
    }

    fn refresh_tabs(&self) {
        let Some(window) = self.window() else { return };
        let items: Vec<TabItem> = self
            .tabs
            .iter()
            .map(|t| TabItem {
                title: t.title().into(),
                tooltip: t.path.display().to_string().into(),
            })
            .collect();
        self.models.tabs.set_vec(items);
        window.set_active_tab(self.active.map_or(-1, |a| a as i32));
    }

    fn refresh_recent(&self) {
        let items: Vec<PaletteItem> = self
            .settings
            .recent
            .iter()
            .map(|p| PaletteItem {
                title: file_name(p).into(),
                detail: p
                    .parent()
                    .map(|d| d.display().to_string())
                    .unwrap_or_default()
                    .into(),
                shortcut: "".into(),
            })
            .collect();
        self.models.recent.set_vec(items);
    }

    pub fn open_recent(&mut self, index: usize) {
        let Some(path) = self.settings.recent.get(index).cloned() else {
            return;
        };
        if !path.exists() {
            self.settings.recent.remove(index);
            self.save_settings();
            self.refresh_recent();
            self.status(format!("{} no longer exists", path.display()));
            return;
        }
        self.open(path);
    }

    pub fn reopen_closed(&mut self) {
        if let Some(path) = self.closed.pop() {
            self.open(path);
        }
    }

    pub fn save_session(&mut self) {
        self.settings.session.files = self.tabs.iter().map(|t| t.path.clone()).collect();
        self.settings.session.active = self.active.unwrap_or(0);
        self.save_settings();
    }

    /// Records a normal exit, with every open file's page.
    pub fn shutdown(&mut self) {
        for i in 0..self.tabs.len() {
            let (path, page) = (self.tabs[i].path.clone(), self.tabs[i].current);
            self.settings.remember_page(&path, page);
        }
        self.settings.session.clean_exit = true;
        self.save_session();
    }

    /// Offers to reopen the files from a run that ended unexpectedly.
    pub fn offer_restore(&mut self, files: Vec<PathBuf>) {
        let files: Vec<PathBuf> = files.into_iter().filter(|p| p.exists()).collect();
        if files.is_empty() {
            return;
        }
        let count = files.len();
        self.push_dialog(Dialog {
            ask: Ask::Restore(files),
            kind: "confirm",
            title: "Reopen your files?".into(),
            text: format!(
                "micropdf closed unexpectedly last time. Reopen the {count} file{} that {} open?",
                if count == 1 { "" } else { "s" },
                if count == 1 { "was" } else { "were" },
            ),
            ok: "Reopen",
            cancel: "Not now",
        });
    }

    // ---------------------------------------------------------------- live reload

    fn watch(&mut self, path: &Path) {
        let Some(dir) = path.parent().map(Path::to_path_buf) else {
            return;
        };
        if self.watched.contains(&dir) {
            return;
        }
        if self.watcher.is_none() {
            self.watcher = notify::recommended_watcher(|event: notify::Result<notify::Event>| {
                let Ok(event) = event else { return };
                if matches!(event.kind, notify::EventKind::Access(_)) {
                    return;
                }
                let paths = event.paths;
                let _ = slint::invoke_from_event_loop(move || {
                    with(|app| app.file_changed(paths));
                });
            })
            .ok();
        }
        if let Some(w) = self.watcher.as_mut() {
            use notify::Watcher;
            if w.watch(&dir, notify::RecursiveMode::NonRecursive).is_ok() {
                self.watched.insert(dir);
            }
        }
    }

    fn unwatch(&mut self, path: &Path) {
        let Some(dir) = path.parent() else { return };
        if self.tabs.iter().any(|t| t.path.parent() == Some(dir)) {
            return;
        }
        if self.watched.remove(dir)
            && let Some(w) = self.watcher.as_mut()
        {
            use notify::Watcher;
            let _ = w.unwatch(dir);
        }
    }

    fn file_changed(&mut self, paths: Vec<PathBuf>) {
        for p in paths {
            if self.tabs.iter().any(|t| same_path(&t.path, &p)) {
                self.reload_paths.insert(p);
            }
        }
        if self.reload_paths.is_empty() {
            return;
        }
        // Writers often save in several steps; wait for the file to settle.
        self.reload_timer
            .start(TimerMode::SingleShot, Duration::from_millis(500), || {
                with(|app| {
                    let paths: Vec<PathBuf> = app.reload_paths.drain().collect();
                    for p in paths {
                        if let Some(i) = app.tabs.iter().position(|t| same_path(&t.path, &p)) {
                            app.reload_changed(i);
                        }
                    }
                });
            });
    }

    /// Reloads a tab whose file changed on disk, unless the change is this app's own save or
    /// a reload would drop unsaved edits.
    fn reload_changed(&mut self, index: usize) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        if tab.saved.is_some() && tab.saved == file_stamp(&tab.path) {
            return;
        }
        if tab.dirty {
            let name = tab.name();
            self.status(format!("{name} changed on disk; your edits are kept"));
            return;
        }
        self.reload(index);
    }

    pub fn reload(&mut self, index: usize) {
        let Some(path) = self.tabs.get(index).map(|t| t.path.clone()) else {
            return;
        };
        let info = match self.engine.open(&path) {
            Ok(info) if !info.needs_password => info,
            Ok(info) => {
                self.engine.close(info.id);
                self.status(format!("{} changed on disk", file_name(&path)));
                return;
            }
            Err(_) => return, // mid-write; the next change event retries
        };
        if self.adopt(index, info) {
            self.status(format!("Reloaded {}", file_name(&path)));
        } else {
            self.engine.close(info.id);
        }
    }

    /// Shows document `info` in tab `index` from scratch: sizes, outline, caches and comments
    /// are read again, and the edit history starts over. The tab's old document closes unless
    /// it is the same one. False if `info` has no pages to show.
    fn adopt(&mut self, index: usize, info: DocInfo) -> bool {
        let Ok(sizes) = self.engine.page_sizes(info.id) else {
            return false;
        };
        if sizes.is_empty() {
            return false;
        }
        let outline = self.engine.outline(info.id).unwrap_or_default();
        let metadata = self.engine.metadata(info.id).unwrap_or_default();
        let attachments = self.engine.attachments(info.id).unwrap_or_default();
        let layers = self.engine.layers(info.id).unwrap_or_default();
        let generation = self.bump();
        let thumb_generation = self.bump();
        let active = self.active == Some(index);
        let spot = if active { self.spot() } else { None };
        let tab = &mut self.tabs[index];
        if tab.info.id != info.id {
            self.engine.close(tab.info.id);
        }
        tab.info = info;
        tab.sizes = sizes
            .iter()
            .map(|s| (s.width.max(1.0), s.height.max(1.0)))
            .collect();
        tab.current = tab.current.min(tab.sizes.len() - 1);
        tab.generation = generation;
        tab.thumb_generation = thumb_generation;
        tab.tiles.clear();
        tab.inflight.clear();
        tab.thumbs.clear();
        tab.thumbs_inflight.clear();
        tab.texts.clear();
        tab.links.clear();
        tab.fields.clear();
        tab.selection = None;
        tab.outline = outline;
        tab.metadata = metadata;
        tab.attachments = attachments;
        tab.layers = layers;
        tab.dirty = false;
        tab.saved = None;
        tab.edits = History::default();
        tab.saved_position = 0;
        tab.comments.clear();
        tab.picked = None;
        tab.view = (0.0, 0.0);
        tab.pending = spot.map(|mut s| {
            s.page = s.page.min(tab.sizes.len() - 1);
            s
        });
        let query = tab.search.query.clone();
        self.refresh_names();
        self.scan_comments(index);
        if active {
            if let Some(window) = self.window() {
                window.set_page_count(self.tabs[index].page_count() as i32);
            }
            self.rebuild_thumbs();
            self.rebuild_outline();
            self.rebuild_info();
            self.rebuild_files();
            self.start_search(query);
            self.update_view();
        }
        true
    }

    // ---------------------------------------------------------------- dialogs

    fn push_dialog(&mut self, dialog: Dialog) {
        self.dialogs.push_back(dialog);
        if self.dialogs.len() == 1 {
            self.show_dialog();
        }
    }

    pub fn message(&mut self, title: &str, text: String) {
        self.push_dialog(Dialog {
            ask: Ask::Message,
            kind: "message",
            title: title.into(),
            text,
            ok: "OK",
            cancel: "",
        });
    }

    fn show_dialog(&self) {
        let Some(window) = self.window() else { return };
        match self.dialogs.front() {
            Some(d) => {
                window.set_dialog_title(d.title.clone().into());
                window.set_dialog_text(d.text.clone().into());
                window.set_dialog_ok(d.ok.into());
                window.set_dialog_cancel_text(d.cancel.into());
                let input = match &d.ask {
                    Ask::Comment { value, .. } => value.clone(),
                    _ => String::new(),
                };
                window.set_dialog_input(input.into());
                window.set_dialog_kind(d.kind.into());
                if d.kind == "password" || d.kind == "input" {
                    window.invoke_focus_dialog();
                } else {
                    window.invoke_focus_view();
                }
            }
            None => {
                window.set_dialog_kind("".into());
                window.invoke_focus_view();
            }
        }
    }

    pub fn dialog_visible(&self) -> bool {
        !self.dialogs.is_empty()
    }

    pub fn dialog_accept(&mut self, input: String) {
        let Some(dialog) = self.dialogs.pop_front() else {
            return;
        };
        match dialog.ask {
            Ask::Unlock { info, path } => match self.engine.authenticate(info.id, &input) {
                Ok(Some(count)) => {
                    let info = DocInfo {
                        page_count: count,
                        needs_password: false,
                        ..info
                    };
                    self.finish_open(path, info);
                }
                _ => {
                    self.ask_password(info, path, true);
                    return;
                }
            },
            Ask::OpenUri(uri) => shell_open(&uri),
            Ask::Restore(files) => self.open_paths(files),
            Ask::Discard(path) => {
                if let Some(i) = self.tabs.iter().position(|t| t.path == path) {
                    self.discard_tab(i);
                }
            }
            Ask::Reload(path) => {
                if let Some(i) = self.tabs.iter().position(|t| t.path == path) {
                    self.reload(i);
                }
            }
            Ask::Note { page, x, y } => {
                let new = NewAnnot::Note { x, y, text: input };
                self.add_comment(page, new, [1.0, 0.85, 0.0]);
            }
            Ask::Reply { page, parent } => {
                if !input.trim().is_empty()
                    && let Some(doc) = self.tab().map(|t| t.info.id)
                {
                    match self.engine.reply(doc, page, parent, input, user_name()) {
                        Ok(_) => self.edited(Some(page)),
                        Err(e) => self.status(format!("Could not reply: {e}")),
                    }
                }
            }
            Ask::Callout { page, target, rect } => {
                if !input.trim().is_empty() {
                    let new = NewAnnot::Callout {
                        target,
                        rect,
                        text: input,
                    };
                    self.add_comment(page, new, [1.0, 1.0, 0.8]);
                }
                self.set_tool(Tool::Select);
            }
            Ask::TextBox { page, rect } => {
                if !input.trim().is_empty() {
                    let new = NewAnnot::FreeText { rect, text: input };
                    self.add_comment(page, new, [1.0, 1.0, 0.8]);
                }
            }
            Ask::Comment { page, id, .. } => {
                if let Some(doc) = self.tab().map(|t| t.info.id) {
                    match self.engine.set_contents(doc, page, id, input) {
                        Ok(()) => self.edited(Some(page)),
                        Err(e) => self.status(format!("Could not change the comment: {e}")),
                    }
                }
            }
            Ask::Quit => {
                for tab in &mut self.tabs {
                    tab.dirty = false;
                }
                let _ = slint::quit_event_loop();
            }
            Ask::Message => {}
        }
        self.show_dialog();
    }

    /// Enter pressed while a dialog shows and its text field does not have focus.
    pub fn dialog_enter(&mut self) {
        let input = self
            .window()
            .map(|w| w.get_dialog_input().to_string())
            .unwrap_or_default();
        self.dialog_accept(input);
    }

    pub fn dialog_cancel(&mut self) {
        let Some(dialog) = self.dialogs.pop_front() else {
            return;
        };
        if let Ask::Unlock { info, .. } = dialog.ask {
            self.engine.close(info.id);
        }
        self.show_dialog();
    }

    pub fn status(&mut self, text: String) {
        let Some(window) = self.window() else { return };
        window.set_status_left(text.into());
        let weak = self.window.clone();
        self.status_timer
            .start(TimerMode::SingleShot, STATUS_TIME, move || {
                if let Some(w) = weak.upgrade() {
                    w.set_status_left("".into());
                }
            });
    }

    // ---------------------------------------------------------------- layout and scrolling

    fn scroll(&self) -> (f32, f32) {
        self.window()
            .map_or((0.0, 0.0), |w| (-w.get_viewport_x(), -w.get_viewport_y()))
    }

    fn set_scroll(&self, x: f32, y: f32) {
        let (Some(window), Some(tab)) = (self.window(), self.tab()) else {
            return;
        };
        let (vw, vh) = (window.get_view_width(), window.get_view_height());
        let dpr = window.window().scale_factor();
        let snap = |v: f32| (v * dpr).round() / dpr;
        let x = snap(x.clamp(0.0, (tab.layout.width - vw).max(0.0)));
        let y = snap(y.clamp(0.0, (tab.layout.height - vh).max(0.0)));
        window.set_viewport_x(-x);
        window.set_viewport_y(-y);
    }

    pub fn scroll_by(&mut self, dx: f32, dy: f32) {
        let (x, y) = self.scroll();
        self.set_scroll(x + dx, y + dy);
        self.update_view();
    }

    pub fn view_size(&self) -> (f32, f32) {
        self.window()
            .map_or((0.0, 0.0), |w| (w.get_view_width(), w.get_view_height()))
    }

    pub fn document_height(&self) -> f32 {
        self.tab().map_or(0.0, |t| t.layout.height)
    }

    pub fn scroll_to_y(&mut self, y: f32) {
        let (x, _) = self.scroll();
        self.set_scroll(x, y);
        self.update_view();
    }

    pub fn at_bottom(&self) -> bool {
        let (_, y) = self.scroll();
        let (_, vh) = self.view_size();
        y + vh >= self.document_height() - 1.0
    }

    pub fn at_top(&self) -> bool {
        self.scroll().1 <= 0.5
    }

    fn spot(&self) -> Option<Spot> {
        let tab = self.tab()?;
        if tab.layout.frames.is_empty() {
            return None;
        }
        let (x, y) = self.scroll();
        let (vw, _) = self.view_size();
        let page = tab.layout.current_page(y, 0.0);
        let f = tab.layout.frame(page)?;
        Some(Spot {
            page,
            frac: (y - f.y) / f.height,
            x_frac: (x + vw / 2.0) / tab.layout.width.max(1.0),
        })
    }

    fn go_spot(&mut self, spot: Spot) {
        let Some(tab) = self.tab_mut() else { return };
        if tab.mode == PageMode::Single && tab.current != spot.page {
            tab.current = spot.page;
            tab.pending = Some(spot);
            tab.view = (0.0, 0.0);
            self.update_view();
            return;
        }
        let Some(f) = tab.layout.frame(spot.page) else {
            return;
        };
        let width = tab.layout.width;
        let (vw, _) = self.view_size();
        self.set_scroll(spot.x_frac * width - vw / 2.0, f.y + spot.frac * f.height);
        self.update_view();
    }

    /// Recomputes the active tab's layout, keeping the reading position.
    fn relayout(&mut self) {
        let Some(window) = self.window() else { return };
        let (vw, vh) = (window.get_view_width(), window.get_view_height());
        let spot = self.spot();
        let Some(tab) = self.tab_mut() else { return };
        if tab.pending.is_none() {
            // A fresh tab with no remembered position starts at its current page's top.
            tab.pending = spot.or(Some(Spot {
                page: tab.current,
                frac: 0.0,
                x_frac: 0.5,
            }));
        }
        tab.view = (vw, vh);
        tab.layout = Layout::compute(&Params {
            sizes: &tab.sizes,
            rotation: tab.rotation,
            mode: tab.mode,
            zoom: tab.zoom,
            view_width: vw,
            view_height: vh,
            current: tab.current,
        });
        window.set_document_width(tab.layout.width);
        window.set_document_height(tab.layout.height);
        if let Some(spot) = tab.pending.take() {
            let f = tab.layout.frame(spot.page).unwrap_or_default();
            let (w, h) = (tab.layout.width, tab.layout.height);
            let x = (spot.x_frac * w - vw / 2.0).clamp(0.0, (w - vw).max(0.0));
            // frac 0 means "this page's top", which leaves the margin above it in view.
            let y = if spot.frac == 0.0 {
                tab.layout.scroll_to(spot.page, None).unwrap_or(0.0)
            } else {
                f.y + spot.frac * f.height
            };
            let y = y.clamp(0.0, (h - vh).max(0.0));
            window.set_viewport_x(-x);
            window.set_viewport_y(-y);
        }
        self.refresh_zoom_text();
    }

    fn refresh_zoom_text(&self) {
        let (Some(window), Some(tab)) = (self.window(), self.tab()) else {
            return;
        };
        window.set_zoom_text(format!("{:.0}%", tab.layout.scale / ACTUAL * 100.0).into());
        window.set_zoom_mode(match tab.zoom {
            Zoom::FitWidth => 0,
            Zoom::FitPage => 1,
            Zoom::Scale(_) => 2,
        });
    }

    /// Brings models and render requests in line with the current scroll position. Called on
    /// every scroll, resize and state change; cheap when nothing moved.
    pub fn update_view(&mut self) {
        let Some(window) = self.window() else { return };
        let Some(ti) = self.active else { return };
        let (vw, vh) = (window.get_view_width(), window.get_view_height());
        if vw < 2.0 || vh < 2.0 {
            return;
        }
        let needs_layout = {
            let tab = &self.tabs[ti];
            tab.view != (vw, vh) || tab.layout.frames.is_empty()
        };
        if needs_layout {
            self.relayout();
        }
        let dpr = window.window().scale_factor();
        let mode = self.settings.reading_mode;
        let (left, top) = (-window.get_viewport_x(), -window.get_viewport_y());

        // Current page.
        let current = {
            let tab = &self.tabs[ti];
            if tab.mode == PageMode::Single {
                tab.current
            } else {
                tab.layout.current_page(top, vh)
            }
        };
        if current != self.tabs[ti].current || needs_layout {
            self.tabs[ti].current = current;
            self.page_changed();
        }

        let tab = &mut self.tabs[ti];
        let signature = ((tab.layout.scale * dpr * 1000.0) as u32, tab.rotation, mode);
        if signature != tab.signature {
            tab.signature = signature;
            self.next_generation += 1;
            tab.generation = self.next_generation;
        }
        let generation = tab.generation;
        let scale = tab.layout.scale * dpr;

        let band = (top - vh * 0.5, top + vh * 1.5);
        let pages = tab.layout.visible(band.0, band.1);
        let mut missing: Vec<(f32, TileKey, Tile, [f32; 4])> = Vec::new();
        let mut page_items = Vec::with_capacity(pages.len());
        let mut tile_items = Vec::new();
        let mut incomplete = HashSet::new();
        for &p in &pages {
            let Some(f) = tab.layout.frame(p) else {
                continue;
            };
            page_items.push(page_item(p, f, dpr));
            let dw = (f.width * dpr).ceil().max(1.0) as i32;
            let dh = (f.height * dpr).ceil().max(1.0) as i32;
            let x0 = (((left - 128.0) - f.x) * dpr).floor().max(0.0) as i32;
            let x1 = ((((left + vw + 128.0) - f.x) * dpr).ceil() as i32).min(dw);
            let y0 = ((band.0 - f.y) * dpr).floor().max(0.0) as i32;
            let y1 = (((band.1 - f.y) * dpr).ceil() as i32).min(dh);
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let mut fresh = Vec::new();
            for row in y0 / TILE..=(y1 - 1) / TILE {
                for col in x0 / TILE..=(x1 - 1) / TILE {
                    let key = TileKey {
                        page: p,
                        generation,
                        col,
                        row,
                    };
                    if let Some(t) = tab.tiles.get(&key) {
                        fresh.push(tile_item(f, t, dpr));
                    } else {
                        let tile = Tile {
                            x: col * TILE,
                            y: row * TILE,
                            width: TILE.min(dw - col * TILE),
                            height: TILE.min(dh - row * TILE),
                        };
                        let frac = [
                            tile.x as f32 / dw as f32,
                            tile.y as f32 / dh as f32,
                            (tile.x + tile.width) as f32 / dw as f32,
                            (tile.y + tile.height) as f32 / dh as f32,
                        ];
                        let centre = f.y + (tile.y as f32 + tile.height as f32 / 2.0) / dpr;
                        missing.push(((centre - (top + vh / 2.0)).abs(), key, tile, frac));
                    }
                }
            }
            if missing.iter().any(|m| m.1.page == p) {
                incomplete.insert(p);
                // Older, blurrier tiles stand in until the new ones arrive.
                let mut old: Vec<(&TileKey, &TileImage)> = tab
                    .tiles
                    .iter()
                    .filter(|(k, _)| k.page == p && k.generation != generation)
                    .collect();
                old.sort_by_key(|(k, _)| k.generation);
                tile_items.extend(old.into_iter().map(|(_, t)| tile_item(f, t, dpr)));
            }
            tile_items.extend(fresh);
        }

        // Forget pixels far from the view.
        let keep: HashSet<usize> = tab
            .layout
            .visible(top - vh * 2.0, top + vh * 3.0)
            .into_iter()
            .collect();
        let band_pages: HashSet<usize> = pages.iter().copied().collect();
        tab.tiles.retain(|k, _| {
            if k.generation == generation {
                keep.contains(&k.page)
            } else {
                band_pages.contains(&k.page) && incomplete.contains(&k.page)
            }
        });

        missing.sort_by(|a, b| a.0.total_cmp(&b.0));
        {
            let mut wanted = self.shared.tiles.lock().unwrap();
            wanted.clear();
            wanted.extend(missing.iter().map(|m| (tab.info.id, m.1)));
        }
        let rotation = tab.rotation;
        for (_, key, tile, frac) in missing {
            if !tab.inflight.insert(key) {
                continue;
            }
            let (engine, shared, doc) = (
                Arc::clone(&self.engine),
                Arc::clone(&self.shared),
                tab.info.id,
            );
            self.pool.spawn(move || {
                let result = if shared.tiles.lock().unwrap().contains(&(doc, key)) {
                    match engine
                        .display_list(doc, key.page)
                        .and_then(|list| mp_engine::render_tile(&list, scale, rotation, tile))
                    {
                        Ok(mut image) => {
                            mode.apply(&mut image.rgb);
                            Rendered::Done(SharedPixelBuffer::clone_from_slice(
                                &image.rgb,
                                image.width,
                                image.height,
                            ))
                        }
                        Err(_) => Rendered::Failed,
                    }
                } else {
                    Rendered::Skipped
                };
                let _ = slint::invoke_from_event_loop(move || {
                    with(|app| app.tile_done(doc, key, frac, result));
                });
            });
        }

        if page_items != self.shown_pages {
            self.models.pages.set_vec(page_items.clone());
            self.shown_pages = page_items;
        }
        if tile_items != self.shown_tiles {
            self.models.tiles.set_vec(tile_items.clone());
            self.shown_tiles = tile_items;
        }
        self.refresh_marks();
        self.update_thumbs();
    }

    fn tile_done(&mut self, doc: DocId, key: TileKey, frac: [f32; 4], result: Rendered) {
        let Some(tab) = self.tabs.iter_mut().find(|t| t.info.id == doc) else {
            return;
        };
        tab.inflight.remove(&key);
        if key.generation != tab.generation {
            return;
        }
        let image = match result {
            Rendered::Skipped => {
                if self.shared.tiles.lock().unwrap().contains(&(doc, key)) {
                    self.update_view();
                }
                return;
            }
            Rendered::Failed => Image::default(),
            Rendered::Done(buffer) => {
                self.renders += 1;
                Image::from_rgb8(buffer)
            }
        };
        tab.tiles.insert(key, TileImage { frac, image });
        if self.tab().is_some_and(|t| t.info.id == doc) {
            self.update_view();
        }
    }

    fn page_changed(&mut self) {
        let (Some(window), Some(tab)) = (self.window(), self.tab()) else {
            return;
        };
        let page = tab.current;
        window.set_page_text((page + 1).to_string().into());
        let (w, h) = tab.sizes[page];
        window.set_status_right(
            format!(
                "{:.2} × {:.2} in  ·  {} page{}",
                w / 72.0,
                h / 72.0,
                tab.page_count(),
                if tab.page_count() == 1 { "" } else { "s" }
            )
            .into(),
        );
        let history = (!tab.history.is_empty(), !tab.future.is_empty());
        window.set_can_back(history.0);
        window.set_can_forward(history.1);
        let thumbs = &self.models.thumbs;
        for row in 0..thumbs.row_count() {
            if let Some(mut item) = thumbs.row_data(row) {
                let current = item.index as usize == page;
                if item.current != current {
                    item.current = current;
                    thumbs.set_row_data(row, item);
                }
            }
        }
        self.reveal_thumb(page);
        self.refresh_outline_current();
    }

    // ---------------------------------------------------------------- navigation

    pub fn go_to(&mut self, page: usize, top: Option<f32>, record: bool) {
        let Some(count) = self.tab().map(DocTab::page_count) else {
            return;
        };
        let page = page.min(count - 1);
        if record {
            self.record_history();
        }
        let tab = self.tab_mut().expect("checked above");
        if tab.mode == PageMode::Single && tab.current != page {
            tab.current = page;
            tab.view = (0.0, 0.0);
            tab.pending = Some(Spot {
                page,
                frac: 0.0,
                x_frac: 0.5,
            });
            self.update_view();
        }
        let Some(tab) = self.tab() else { return };
        if let Some(y) = tab.layout.scroll_to(page, top) {
            let (x, _) = self.scroll();
            self.set_scroll(x, y);
        }
        self.update_view();
    }

    fn record_history(&mut self) {
        let Some(spot) = self.spot() else { return };
        let Some(tab) = self.tab_mut() else { return };
        if tab.history.last() != Some(&spot) {
            tab.history.push(spot);
            if tab.history.len() > HISTORY_LIMIT {
                tab.history.remove(0);
            }
        }
        tab.future.clear();
    }

    pub fn back(&mut self) {
        let Some(here) = self.spot() else { return };
        let Some(tab) = self.tab_mut() else { return };
        let Some(spot) = tab.history.pop() else {
            return;
        };
        tab.future.push(here);
        self.go_spot(spot);
        self.page_changed();
    }

    pub fn forward(&mut self) {
        let Some(here) = self.spot() else { return };
        let Some(tab) = self.tab_mut() else { return };
        let Some(spot) = tab.future.pop() else { return };
        tab.history.push(here);
        self.go_spot(spot);
        self.page_changed();
    }

    /// The first page of the next (or previous) row: a page in continuous mode, a spread in
    /// two-up and book modes.
    fn row_step(&self, forward: bool) -> Option<usize> {
        let tab = self.tab()?;
        let current = tab.current;
        if tab.mode == PageMode::Single {
            return Some(if forward {
                (current + 1).min(tab.page_count() - 1)
            } else {
                current.saturating_sub(1)
            });
        }
        let y = tab.layout.frame(current)?.y;
        let rows: Vec<(usize, f32)> = tab
            .layout
            .frames
            .iter()
            .enumerate()
            .filter_map(|(i, f)| f.map(|f| (i, f.y)))
            .collect();
        if forward {
            rows.iter().find(|(_, fy)| *fy > y + 0.5).map(|r| r.0)
        } else {
            let prev = rows
                .iter()
                .filter(|(_, fy)| *fy < y - 0.5)
                .map(|r| r.1)
                .fold(f32::NEG_INFINITY, f32::max);
            rows.iter()
                .find(|(_, fy)| (*fy - prev).abs() < 0.5)
                .map(|r| r.0)
                .or(Some(0))
        }
    }

    pub fn next_page(&mut self) {
        if let Some(p) = self.row_step(true) {
            self.go_to(p, None, false);
        }
    }

    pub fn prev_page(&mut self) {
        if let Some(p) = self.row_step(false) {
            self.go_to(p, None, false);
        }
    }

    pub fn first_page(&mut self) {
        self.go_to(0, None, true);
    }

    pub fn last_page(&mut self) {
        let n = self.page_count();
        if n > 0 {
            self.go_to(n - 1, None, true);
        }
    }

    pub fn page_entered(&mut self, text: &str) {
        let Some(tab) = self.tab() else { return };
        let current = tab.current;
        match text.trim().parse::<usize>() {
            Ok(n) if n >= 1 => self.go_to(n - 1, None, true),
            _ => {
                if let Some(window) = self.window() {
                    window.set_page_text((current + 1).to_string().into());
                }
            }
        }
    }

    /// Page down or up by most of a screen; in single-page mode, turns the page at the edges.
    pub fn page_scroll(&mut self, forward: bool) {
        let (_, vh) = self.view_size();
        let single = self.tab().is_some_and(|t| t.mode == PageMode::Single);
        if single && forward && self.at_bottom() {
            self.next_page();
        } else if single && !forward && self.at_top() {
            let before = self.tab().map(|t| t.current);
            self.prev_page();
            if self.tab().map(|t| t.current) != before {
                self.scroll_to_y(f32::MAX);
            }
        } else {
            let step = (vh - 40.0).max(vh * 0.5);
            self.scroll_by(0.0, if forward { step } else { -step });
        }
    }

    /// Left and right arrows: scroll sideways when the document is wider than the view,
    /// otherwise turn the page.
    pub fn horizontal(&mut self, forward: bool) {
        let (vw, _) = self.view_size();
        let wide = self.tab().is_some_and(|t| t.layout.width > vw + 1.0);
        if wide {
            self.scroll_by(if forward { LINE } else { -LINE }, 0.0);
        } else if forward {
            self.next_page();
        } else {
            self.prev_page();
        }
    }

    // ---------------------------------------------------------------- zoom, rotation, modes

    pub fn set_zoom(&mut self, zoom: Zoom, anchor: Option<(f32, f32)>) {
        let Some(window) = self.window() else { return };
        let (vw, vh) = (window.get_view_width(), window.get_view_height());
        let (left, top) = self.scroll();
        let (ax, ay) = anchor.unwrap_or((left + vw / 2.0, top + vh / 2.0));
        let Some(tab) = self.tab_mut() else { return };
        let hit = tab.layout.hit(ax, ay);
        tab.zoom = zoom;
        tab.view = (0.0, 0.0);
        if hit.is_some() {
            tab.pending = None;
        }
        self.relayout();
        let Some(tab) = self.tab() else { return };
        if let Some((page, px, py)) = hit
            && let Some(at) = tab.layout.to_view(
                page,
                &Rect {
                    x0: px,
                    y0: py,
                    x1: px,
                    y1: py,
                },
            )
        {
            self.set_scroll(at.x - (ax - left), at.y - (ay - top));
        }
        self.update_view();
    }

    fn scale(&self) -> f32 {
        self.tab().map_or(ACTUAL, |t| t.layout.scale)
    }

    pub fn zoom_step(&mut self, steps: i32, anchor: Option<(f32, f32)>) {
        let factor = self.scale() / ACTUAL;
        let next = if steps > 0 {
            ZOOM_STEPS.iter().copied().find(|&z| z > factor * 1.01)
        } else {
            ZOOM_STEPS
                .iter()
                .rev()
                .copied()
                .find(|&z| z < factor * 0.99)
        };
        if let Some(z) = next {
            self.set_zoom(Zoom::Scale(z * ACTUAL), anchor);
        }
    }

    /// Ctrl+wheel: about 12% per notch, anchored at the pointer.
    pub fn wheel_zoom(&mut self, delta: f32, x: f32, y: f32) {
        let notches = (delta / 60.0).clamp(-3.0, 3.0);
        let factor = (self.scale() * 1.12f32.powf(notches)) / ACTUAL;
        let factor = factor.clamp(ZOOM_STEPS[0], *ZOOM_STEPS.last().unwrap());
        self.set_zoom(Zoom::Scale(factor * ACTUAL), Some((x, y)));
    }

    pub fn rotate(&mut self, degrees: i32) {
        let Some(tab) = self.tab_mut() else { return };
        tab.rotation = (tab.rotation + degrees).rem_euclid(360);
        tab.view = (0.0, 0.0);
        tab.selection = None;
        // Old tiles would stand in sideways; plain paper is better.
        tab.tiles.clear();
        tab.thumbs.clear();
        self.next_generation += 1;
        let g = self.next_generation;
        if let Some(tab) = self.tab_mut() {
            tab.thumb_generation = g;
        }
        self.rebuild_thumbs();
        self.update_view();
    }

    pub fn set_mode(&mut self, mode: PageMode) {
        let Some(window) = self.window() else { return };
        let Some(tab) = self.tab_mut() else { return };
        tab.mode = mode;
        tab.view = (0.0, 0.0);
        self.settings.page_mode = mode;
        self.save_settings();
        window.set_page_mode(mode_index(mode));
        self.update_view();
    }

    pub fn set_reading_mode(&mut self, mode: ReadingMode) {
        let Some(window) = self.window() else { return };
        self.settings.reading_mode = mode;
        self.save_settings();
        window.set_reading_mode(mode.index());
        window.set_paper(paper_color(mode));
        self.next_generation += 1;
        let g = self.next_generation;
        if let Some(tab) = self.tab_mut() {
            tab.thumbs.clear();
            tab.thumb_generation = g;
        }
        self.rebuild_thumbs();
        self.update_view();
    }

    pub fn toggle_theme(&mut self) {
        let Some(window) = self.window() else { return };
        self.settings.dark_theme = !self.settings.dark_theme;
        window.global::<Theme>().set_dark(self.settings.dark_theme);
        self.save_settings();
    }

    pub fn toggle_vim(&mut self) {
        let Some(window) = self.window() else { return };
        self.settings.vim = !self.settings.vim;
        window.set_vim_enabled(self.settings.vim);
        self.save_settings();
        self.status(if self.settings.vim {
            "Vim keys on: j k scroll, J K turn pages, gg G, / search, : commands".into()
        } else {
            "Vim keys off".into()
        });
    }

    pub fn vim(&self) -> bool {
        self.settings.vim
    }

    pub fn set_sidebar(&mut self, visible: bool, tab: Option<i32>) {
        let Some(window) = self.window() else { return };
        window.set_sidebar_visible(visible);
        if let Some(t) = tab {
            window.set_sidebar_tab(t);
        }
        self.settings.sidebar = visible;
        self.save_settings();
        self.update_view();
    }

    pub fn toggle_sidebar(&mut self) {
        let visible = self.window().is_some_and(|w| !w.get_sidebar_visible());
        self.set_sidebar(visible, None);
    }

    pub fn toggle_fullscreen(&mut self) {
        let Some(window) = self.window() else { return };
        let full = !window.window().is_fullscreen();
        window.window().set_fullscreen(full);
    }

    pub fn presenting(&self) -> bool {
        self.presenting.is_some()
    }

    pub fn toggle_present(&mut self) {
        let Some(window) = self.window() else { return };
        if let Some((sidebar, mode, zoom)) = self.presenting.take() {
            window.set_presenting(false);
            window.set_chrome_visible(true);
            window.window().set_fullscreen(false);
            window.set_sidebar_visible(sidebar);
            if let Some(tab) = self.tab_mut() {
                tab.mode = mode;
                tab.zoom = zoom;
                tab.view = (0.0, 0.0);
            }
            window.set_page_mode(mode_index(mode));
            self.update_view();
            return;
        }
        let Some(tab) = self.tab_mut() else { return };
        let saved = (window.get_sidebar_visible(), tab.mode, tab.zoom);
        tab.mode = PageMode::Single;
        tab.zoom = Zoom::FitPage;
        tab.view = (0.0, 0.0);
        tab.pending = Some(Spot {
            page: tab.current,
            frac: 0.0,
            x_frac: 0.5,
        });
        self.presenting = Some(saved);
        window.set_find_visible(false);
        window.set_presenting(true);
        window.set_chrome_visible(false);
        window.window().set_fullscreen(true);
        self.update_view();
        self.status("Presenting: arrows or clicks turn pages, Esc ends".into());
    }

    // ---------------------------------------------------------------- pointer

    fn page_text(&mut self, page: usize) -> Option<Rc<PageText>> {
        let tab = self.tab_mut()?;
        if let Some(t) = tab.texts.get(&page) {
            return Some(Rc::clone(t));
        }
        let doc = tab.info.id;
        let text = self
            .engine
            .display_list(doc, page)
            .and_then(|list| mp_engine::page_text(&list))
            .unwrap_or_default();
        let tab = self.tab_mut()?;
        if tab.texts.len() >= TEXT_CACHE {
            let current = tab.current;
            tab.texts
                .retain(|&p, _| p.abs_diff(current) < TEXT_CACHE / 4);
        }
        let text = Rc::new(text);
        tab.texts.insert(page, Rc::clone(&text));
        Some(text)
    }

    fn page_links(&mut self, page: usize) -> Rc<Vec<Link>> {
        let Some(tab) = self.tab() else {
            return Rc::default();
        };
        if let Some(l) = tab.links.get(&page) {
            return Rc::clone(l);
        }
        let links = Rc::new(self.engine.links(tab.info.id, page).unwrap_or_default());
        if let Some(tab) = self.tab_mut() {
            if tab.links.len() > 64 {
                tab.links.clear();
            }
            tab.links.insert(page, Rc::clone(&links));
        }
        links
    }

    fn hit(&self, x: f32, y: f32) -> Option<(usize, f32, f32)> {
        self.tab()?.layout.hit(x, y)
    }

    /// The page-space point under a document-space point, clamped to `page`, so a drag that
    /// leaves the page stays on its edge.
    fn page_point(&self, page: usize, x: f32, y: f32) -> Option<(f32, f32)> {
        let f = self.tab()?.layout.frame(page)?;
        let cx = x.clamp(f.x, f.x + f.width - 0.01);
        let cy = y.clamp(f.y, f.bottom() - 0.01);
        let (hit, px, py) = self.hit(cx, cy)?;
        (hit == page).then_some((px, py))
    }

    fn link_at(&mut self, x: f32, y: f32) -> Option<LinkTarget> {
        let (page, px, py) = self.hit(x, y)?;
        self.page_links(page)
            .iter()
            .find(|l| l.rect.contains(px, py))
            .map(|l| l.target.clone())
    }

    fn over_text(&mut self, x: f32, y: f32) -> bool {
        let Some((page, px, py)) = self.hit(x, y) else {
            return false;
        };
        self.page_text(page).is_some_and(|t| {
            t.chars.iter().any(|c| {
                px >= c.rect.x0 - 2.0
                    && px <= c.rect.x1 + 2.0
                    && py >= c.rect.y0 - 2.0
                    && py <= c.rect.y1 + 2.0
            })
        })
    }

    pub fn hover(&mut self, x: f32, y: f32) {
        if self.drag.is_some() || self.presenting() {
            return;
        }
        let cursor = if let Some(corner) = self.handle_at(x, y) {
            if corner % 2 == 0 { 4 } else { 5 }
        } else if self.movable_at(x, y).is_some() {
            3
        } else if self.link_at(x, y).is_some()
            || self.comment_at(x, y).is_some()
            || self.field_at(x, y).is_some()
        {
            2
        } else if self.over_text(x, y) {
            1
        } else {
            0
        };
        if cursor != self.cursor {
            self.cursor = cursor;
            if let Some(w) = self.window() {
                w.set_cursor(cursor);
            }
        }
    }

    pub fn pointer_down(&mut self, x: f32, y: f32, button: i32, shift: bool) {
        // A click anywhere but the field editor ends typing, keeping the text.
        self.commit_open_field();
        if self.presenting() {
            match button {
                0 => self.next_page(),
                1 => self.prev_page(),
                _ => {}
            }
            return;
        }
        match button {
            2 => {
                self.drag = Some(Drag::Pan {
                    start: self.to_screen(x, y),
                    scroll: self.scroll(),
                });
            }
            0 if self.tool != Tool::Select => {
                let Some((page, px, py)) = self.hit(x, y) else {
                    return;
                };
                if self.tool == Tool::Sign {
                    self.place_mark(page, px, py);
                } else if self.tool == Tool::Stamp {
                    self.place_stamp(page, px, py);
                } else if self.tool == Tool::Attach {
                    self.attach_pick(Some((page, px, py)));
                } else if self.tool == Tool::Note {
                    self.push_dialog(Dialog {
                        ask: Ask::Note { page, x: px, y: py },
                        kind: "input",
                        title: "Add a note".into(),
                        text: String::new(),
                        ok: "Add",
                        cancel: "Cancel",
                    });
                } else {
                    self.drag = Some(Drag::Draw {
                        page,
                        points: vec![(px, py)],
                    });
                }
            }
            0 => {
                if let Some(drag) = self.shape_drag(x, y) {
                    self.drag = Some(drag);
                    return;
                }
                let hit = self.hit(x, y);
                let text = hit.and_then(|(page, px, py)| {
                    let t = self.page_text(page)?;
                    t.nearest(px, py).map(|i| (page, i))
                });
                match text {
                    Some((page, index)) if !self.link_at(x, y).is_some() || shift => {
                        let tab = self.tab_mut().expect("hit implies a tab");
                        match tab.selection {
                            Some(ref mut s) if shift && s.page == page => s.focus = index,
                            _ => {
                                tab.selection = Some(Selection {
                                    page,
                                    anchor: index,
                                    focus: index,
                                })
                            }
                        }
                        self.drag = Some(Drag::Select {
                            moved: shift,
                            start: (x, y),
                        });
                    }
                    _ => {
                        if let Some(tab) = self.tab_mut() {
                            tab.selection = None;
                        }
                        self.drag = Some(Drag::Click { start: (x, y) });
                    }
                }
                self.refresh_marks();
            }
            _ => {}
        }
    }

    fn to_screen(&self, x: f32, y: f32) -> (f32, f32) {
        let (sx, sy) = self.scroll();
        (x - sx, y - sy)
    }

    pub fn pointer_move(&mut self, x: f32, y: f32) {
        if let Some(Drag::Draw { page, .. }) = self.drag {
            let Some((px, py)) = self.page_point(page, x, y) else {
                return;
            };
            let ink = self.tool == Tool::Ink;
            if let Some(Drag::Draw { points, .. }) = self.drag.as_mut() {
                if ink {
                    points.push((px, py));
                } else {
                    points.truncate(1);
                    points.push((px, py));
                }
            }
            self.show_draft();
            return;
        }
        match self.drag {
            Some(Drag::Shape {
                page,
                origin,
                start,
                rect,
                resize,
                keep_aspect,
                ..
            }) => {
                let Some(to) = self.page_point(page, x, y) else {
                    return;
                };
                let next = if resize {
                    resized(rect, start, to, keep_aspect)
                } else {
                    let (dx, dy) = (to.0 - start.0, to.1 - start.1);
                    Rect {
                        x0: rect.x0 + dx,
                        y0: rect.y0 + dy,
                        x1: rect.x1 + dx,
                        y1: rect.y1 + dy,
                    }
                };
                if let Some(Drag::Shape { current, moved, .. }) = self.drag.as_mut() {
                    *current = next;
                    *moved |= (x - origin.0).abs() + (y - origin.1).abs() > 3.0;
                }
                self.refresh_marks();
            }
            Some(Drag::Pan { start, scroll }) => {
                let (vx, vy) = self.to_screen(x, y);
                self.set_scroll(scroll.0 - (vx - start.0), scroll.1 - (vy - start.1));
                self.update_view();
            }
            Some(Drag::Select {
                ref mut moved,
                start,
            }) => {
                if (x - start.0).abs() + (y - start.1).abs() > 3.0 {
                    *moved = true;
                }
                let Some(sel) = self.tab().and_then(|t| t.selection) else {
                    return;
                };
                let Some(f) = self.tab().and_then(|t| t.layout.frame(sel.page)) else {
                    return;
                };
                // Clamp to the selection's page so dragging past its edge keeps selecting.
                let cx = x.clamp(f.x, f.x + f.width - 0.01);
                let cy = y.clamp(f.y, f.bottom() - 0.01);
                if let Some((page, px, py)) = self.hit(cx, cy)
                    && page == sel.page
                    && let Some(i) = self.page_text(page).and_then(|t| t.nearest(px, py))
                    && let Some(tab) = self.tab_mut()
                    && let Some(s) = tab.selection.as_mut()
                {
                    s.focus = i;
                }
                self.refresh_marks();
                // Scroll when dragging past the view's edge.
                let (_, vy) = self.to_screen(x, y);
                let (_, vh) = self.view_size();
                if vy < 0.0 || vy > vh {
                    self.scroll_by(0.0, if vy < 0.0 { vy } else { vy - vh } / 2.0);
                }
            }
            _ => {}
        }
    }

    pub fn pointer_up(&mut self, x: f32, y: f32) {
        let drag = self.drag.take();
        if let Some(Drag::Draw { page, points }) = drag {
            if let Some(w) = self.window() {
                w.set_draft_path("".into());
            }
            self.finish_draw(page, points);
            return;
        }
        match drag {
            Some(Drag::Shape {
                page,
                id,
                current,
                moved: true,
                ..
            }) => self.reshape(page, id, current),
            Some(Drag::Shape { .. }) => self.refresh_marks(),
            Some(Drag::Select { moved: false, .. }) => {
                if let Some(tab) = self.tab_mut() {
                    tab.selection = None;
                }
                self.refresh_marks();
                self.click(x, y);
            }
            Some(Drag::Click { start }) => {
                if (x - start.0).abs() + (y - start.1).abs() <= 3.0 {
                    self.click(x, y);
                }
            }
            Some(Drag::Select { moved: true, .. }) => {
                let n = self.selected_text().map_or(0, |t| t.chars().count());
                if n > 0 {
                    self.status(format!("{n} characters selected. Ctrl+C copies them."));
                }
            }
            _ => {}
        }
        self.hover(x, y);
    }

    fn click(&mut self, x: f32, y: f32) {
        if let Some((page, field)) = self.field_at(x, y) {
            self.use_field(page, field);
            return;
        }
        let comment = self.comment_at(x, y);
        let had = self.tab_mut().and_then(|t| t.picked.take()).is_some();
        if let Some(index) = comment {
            self.pick_comment(index);
            return;
        }
        if had {
            self.refresh_marks();
        }
        match self.link_at(x, y) {
            Some(LinkTarget::Page { page, top }) => self.go_to(page, top, true),
            Some(LinkTarget::Uri(uri)) => self.confirm_uri(uri),
            None => {}
        }
    }

    pub fn pointer_double(&mut self, x: f32, y: f32) {
        if let Some(index) = self.comment_at(x, y) {
            self.comment_edit(index);
            return;
        }
        let Some((page, px, py)) = self.hit(x, y) else {
            return;
        };
        let Some(text) = self.page_text(page) else {
            return;
        };
        let Some(i) = text.nearest(px, py) else {
            return;
        };
        let word = |c: char| c.is_alphanumeric() || c == '_' || c == '\'' || c == '-';
        if !word(text.chars[i].ch) {
            return;
        }
        let mut a = i;
        while a > 0 && word(text.chars[a - 1].ch) && !text.chars[a - 1].line_end {
            a -= 1;
        }
        let mut b = i;
        while b + 1 < text.chars.len() && word(text.chars[b + 1].ch) && !text.chars[b].line_end {
            b += 1;
        }
        if let Some(tab) = self.tab_mut() {
            tab.selection = Some(Selection {
                page,
                anchor: a,
                focus: b,
            });
        }
        self.drag = None;
        self.refresh_marks();
    }

    fn confirm_uri(&mut self, uri: String) {
        let lower = uri.to_ascii_lowercase();
        if !(lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("mailto:"))
        {
            self.message(
                "Link not opened",
                format!("micropdf only opens web and mail links. This link points to:\n{uri}"),
            );
            return;
        }
        self.push_dialog(Dialog {
            text: format!("This document wants to open:\n{uri}"),
            ask: Ask::OpenUri(uri),
            kind: "confirm",
            title: "Open link?".into(),
            ok: "Open",
            cancel: "Cancel",
        });
    }

    pub fn selected_text(&mut self) -> Option<String> {
        let sel = self.tab()?.selection?;
        let text = self.page_text(sel.page)?;
        let range = sel.range();
        (range.end <= text.chars.len()).then(|| text.text(range))
    }

    pub fn copy(&mut self) {
        let Some(text) = self.selected_text().filter(|t| !t.is_empty()) else {
            self.status("Nothing selected".into());
            return;
        };
        let n = text.chars().count();
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(text)) {
            Ok(()) => self.status(format!("Copied {n} characters")),
            Err(e) => self.status(format!("Could not copy: {e}")),
        }
    }

    pub fn select_all(&mut self) {
        let Some(page) = self.tab().map(|t| t.current) else {
            return;
        };
        let Some(text) = self.page_text(page) else {
            return;
        };
        if text.chars.is_empty() {
            self.status("This page has no text".into());
            return;
        }
        let last = text.chars.len() - 1;
        if let Some(tab) = self.tab_mut() {
            tab.selection = Some(Selection {
                page,
                anchor: 0,
                focus: last,
            });
        }
        self.refresh_marks();
    }

    pub fn clear_selection(&mut self) -> bool {
        let Some(tab) = self.tab_mut() else {
            return false;
        };
        let had = tab.selection.take().is_some() | tab.picked.take().is_some();
        if had {
            self.refresh_marks();
        }
        had
    }

    fn refresh_marks(&mut self) {
        let selection = self.tab().and_then(|t| t.selection);
        let selection_rects = match selection {
            Some(s) => self
                .page_text(s.page)
                .filter(|t| s.range().end <= t.chars.len())
                .map(|t| (s.page, t.line_rects(s.range())))
                .unwrap_or_default(),
            None => (0, Vec::new()),
        };
        let Some(tab) = self.tab() else { return };
        let band: HashSet<usize> = self.shown_pages.iter().map(|p| p.index as usize).collect();
        let mut marks = Vec::new();
        for (page, rect) in &tab.search.hits {
            if !band.contains(page) {
                continue;
            }
            if let Some(f) = tab.layout.to_view(*page, rect) {
                let kind = if tab.search.current == Some((*page, *rect)) {
                    1
                } else {
                    0
                };
                marks.push(mark_item(f, kind));
            }
        }
        for rect in &selection_rects.1 {
            if let Some(f) = tab.layout.to_view(selection_rects.0, rect) {
                marks.push(mark_item(f, 2));
            }
        }
        if let Some((page, id)) = tab.picked
            && let Some((_, a)) = tab.comments.iter().find(|(p, a)| *p == page && a.id == id)
        {
            let rect = match self.drag {
                Some(Drag::Shape {
                    id: dragged,
                    current,
                    ..
                }) if dragged == id => current,
                _ => a.rect,
            };
            if let Some(f) = tab.layout.to_view(page, &rect) {
                marks.push(mark_item(f, 3));
                if resizable(a.kind) {
                    for (x, y) in corners(f) {
                        marks.push(MarkItem {
                            x: x - HANDLE,
                            y: y - HANDLE,
                            width: HANDLE * 2.0,
                            height: HANDLE * 2.0,
                            kind: 4,
                        });
                    }
                }
            }
        }
        if let Some(f) = &self.field_focus
            && f.doc == tab.info.id
            && !f.editing
            && let Some(frame) = tab.layout.to_view(f.page, &f.field.rect)
        {
            marks.push(mark_item(frame, 5));
        }
        if marks != self.shown_marks {
            self.models.marks.set_vec(marks.clone());
            self.shown_marks = marks;
            self.refresh_style_bar();
        }
        self.place_field_editor();
    }

    // ---------------------------------------------------------------- search

    pub fn find_edited(&mut self, query: String) {
        self.search_timer.start(
            TimerMode::SingleShot,
            Duration::from_millis(220),
            move || {
                with(|app| app.start_search(query.clone()));
            },
        );
    }

    fn start_search(&mut self, query: String) {
        let id = self.shared.search.fetch_add(1, Ordering::SeqCst) + 1;
        let Some(tab) = self.tab_mut() else { return };
        tab.search = Search {
            query: query.clone(),
            id,
            ..Search::default()
        };
        let (doc, start, count) = (tab.info.id, tab.current, tab.page_count());
        self.refresh_find_status();
        self.refresh_marks();
        if query.trim().is_empty() {
            return;
        }
        let (engine, shared) = (Arc::clone(&self.engine), Arc::clone(&self.shared));
        self.pool.spawn(move || {
            for k in 0..count {
                if shared.search.load(Ordering::SeqCst) != id {
                    return;
                }
                let page = (start + k) % count;
                let hits = engine
                    .display_list(doc, page)
                    .and_then(|list| mp_engine::search(&list, &query))
                    .unwrap_or_default();
                let last = k + 1 == count;
                if !hits.is_empty() || last || k % 16 == 15 {
                    let _ = slint::invoke_from_event_loop(move || {
                        with(|app| app.search_progress(doc, id, page, hits, k + 1, last));
                    });
                }
            }
        });
    }

    fn search_progress(
        &mut self,
        doc: DocId,
        id: u64,
        page: usize,
        hits: Vec<Rect>,
        searched: usize,
        done: bool,
    ) {
        let Some(tab) = self.tabs.iter_mut().find(|t| t.info.id == doc) else {
            return;
        };
        if tab.search.id != id {
            return;
        }
        tab.search.searched = searched;
        tab.search.done = done;
        let first = tab.search.hits.is_empty() && !hits.is_empty();
        tab.search.hits.extend(hits.into_iter().map(|r| (page, r)));
        tab.search.hits.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then(a.1.y0.total_cmp(&b.1.y0))
                .then(a.1.x0.total_cmp(&b.1.x0))
        });
        let active = self.tab().is_some_and(|t| t.info.id == doc);
        if active && first {
            self.find_step(true);
        }
        if active {
            self.refresh_find_status();
            self.refresh_marks();
        }
    }

    fn refresh_find_status(&self) {
        let (Some(window), Some(tab)) = (self.window(), self.tab()) else {
            return;
        };
        let s = &tab.search;
        let text = if s.query.trim().is_empty() {
            String::new()
        } else if s.hits.is_empty() {
            if s.done {
                "No matches".into()
            } else {
                "Searching…".into()
            }
        } else {
            let total = format!("{}{}", s.hits.len(), if s.done { "" } else { "+" });
            match s.current.and_then(|c| s.hits.iter().position(|h| *h == c)) {
                Some(i) => format!("{} of {total}", i + 1),
                None => format!("{total} matches"),
            }
        };
        window.set_find_status(text.into());
    }

    /// Moves to the next (or previous) search hit after the current one, or after the current
    /// page when no hit is current.
    pub fn find_step(&mut self, forward: bool) {
        let Some(tab) = self.tab_mut() else { return };
        let hits = &tab.search.hits;
        if hits.is_empty() {
            return;
        }
        let index = match tab
            .search
            .current
            .and_then(|c| hits.iter().position(|h| *h == c))
        {
            Some(i) if forward => (i + 1) % hits.len(),
            Some(i) => (i + hits.len() - 1) % hits.len(),
            None if forward => hits.iter().position(|h| h.0 >= tab.current).unwrap_or(0),
            None => hits
                .iter()
                .rposition(|h| h.0 <= tab.current)
                .unwrap_or(hits.len() - 1),
        };
        let (page, rect) = hits[index];
        tab.search.current = Some((page, rect));
        self.reveal(page, rect);
        self.refresh_find_status();
        self.refresh_marks();
    }

    /// Scrolls so `rect` on `page` is in view, if it is not already.
    fn reveal(&mut self, page: usize, rect: Rect) {
        if self
            .tab()
            .is_some_and(|t| t.mode == PageMode::Single && t.current != page)
        {
            self.go_to(page, None, false);
        }
        let Some(f) = self.tab().and_then(|t| t.layout.to_view(page, &rect)) else {
            return;
        };
        let (x, y) = self.scroll();
        let (vw, vh) = self.view_size();
        let nx = if f.x < x || f.x + f.width > x + vw {
            f.x - vw / 3.0
        } else {
            x
        };
        let ny = if f.y < y || f.bottom() > y + vh {
            f.y - vh / 3.0
        } else {
            y
        };
        self.set_scroll(nx, ny);
        self.update_view();
    }

    pub fn open_find(&mut self) {
        let Some(window) = self.window() else { return };
        if !self.has_document() {
            return;
        }
        window.set_find_visible(true);
        window.invoke_focus_find();
    }

    pub fn close_find(&mut self) {
        let Some(window) = self.window() else { return };
        window.set_find_visible(false);
        window.invoke_focus_view();
        self.shared.search.fetch_add(1, Ordering::SeqCst);
        if let Some(tab) = self.tab_mut() {
            tab.search = Search::default();
        }
        window.set_find_query("".into());
        self.refresh_find_status();
        self.refresh_marks();
    }

    // ---------------------------------------------------------------- sidebar

    fn rebuild_thumbs(&mut self) {
        let Some(tab) = self.tab_mut() else {
            return;
        };
        let rotated = tab.rotation % 180 != 0;
        let mut y = 0.0;
        let mut tops = Vec::with_capacity(tab.sizes.len());
        let items: Vec<ThumbItem> = tab
            .sizes
            .iter()
            .enumerate()
            .map(|(i, &(w, h))| {
                let (w, h) = if rotated { (h, w) } else { (w, h) };
                let s = (THUMB_BOX.0 / w).min(THUMB_BOX.1 / h);
                tops.push(y);
                y += (h * s).round() + THUMB_EXTRA;
                ThumbItem {
                    index: i as i32,
                    image: tab.thumbs.get(&i).cloned().unwrap_or_default(),
                    label: (i + 1).to_string().into(),
                    width: (w * s).round(),
                    height: (h * s).round(),
                    current: i == tab.current,
                }
            })
            .collect();
        tab.thumb_tops = tops;
        self.models.thumbs.set_vec(items);
    }

    pub fn update_thumbs(&mut self) {
        let Some(window) = self.window() else { return };
        let Some(ti) = self.active else { return };
        if !window.get_sidebar_visible() || window.get_sidebar_tab() != 0 {
            self.shared.thumbs.lock().unwrap().clear();
            return;
        }
        let dpr = window.window().scale_factor();
        let top = -window.get_thumbs_y();
        let height = window.get_thumbs_view_height().max(200.0);
        let tab = &mut self.tabs[ti];
        let first = tab
            .thumb_tops
            .partition_point(|&t| t < top - 300.0)
            .saturating_sub(1);
        let last = tab
            .thumb_tops
            .partition_point(|&t| t < top + height + 300.0);
        let (doc, generation, rotation) = (tab.info.id, tab.thumb_generation, tab.rotation);

        // Drop thumbnails far from the visible range.
        let far: Vec<usize> = tab
            .thumbs
            .keys()
            .copied()
            .filter(|&p| p + 40 < first || p > last + 40)
            .collect();
        for p in far {
            tab.thumbs.remove(&p);
            if let Some(mut item) = self.models.thumbs.row_data(p) {
                item.image = Image::default();
                self.models.thumbs.set_row_data(p, item);
            }
        }

        let wanted: Vec<usize> = (first..last)
            .filter(|p| !tab.thumbs.contains_key(p))
            .collect();
        {
            let mut set = self.shared.thumbs.lock().unwrap();
            set.clear();
            set.extend(wanted.iter().map(|&p| (doc, p, generation)));
        }
        for p in wanted {
            if !tab.thumbs_inflight.insert(p) {
                continue;
            }
            let Some(item) = self.models.thumbs.row_data(p) else {
                continue;
            };
            let (w, h) = tab.sizes[p];
            let (w, h) = if rotation % 180 != 0 { (h, w) } else { (w, h) };
            let scale = item.width / w * dpr;
            let (dw, dh) = ((w * scale).ceil() as i32, (h * scale).ceil() as i32);
            let (engine, shared) = (Arc::clone(&self.engine), Arc::clone(&self.shared));
            let mode = self.settings.reading_mode;
            self.pool.spawn(move || {
                let result = if shared
                    .thumbs
                    .lock()
                    .unwrap()
                    .contains(&(doc, p, generation))
                {
                    let tile = Tile {
                        x: 0,
                        y: 0,
                        width: dw.max(1),
                        height: dh.max(1),
                    };
                    match engine
                        .display_list(doc, p)
                        .and_then(|list| mp_engine::render_tile(&list, scale, rotation, tile))
                    {
                        Ok(mut image) => {
                            mode.apply(&mut image.rgb);
                            Rendered::Done(SharedPixelBuffer::clone_from_slice(
                                &image.rgb,
                                image.width,
                                image.height,
                            ))
                        }
                        Err(_) => Rendered::Failed,
                    }
                } else {
                    Rendered::Skipped
                };
                let _ = slint::invoke_from_event_loop(move || {
                    with(|app| app.thumb_done(doc, p, generation, result));
                });
            });
        }
    }

    fn thumb_done(&mut self, doc: DocId, page: usize, generation: u64, result: Rendered) {
        let active = self.tab().is_some_and(|t| t.info.id == doc);
        let Some(tab) = self.tabs.iter_mut().find(|t| t.info.id == doc) else {
            return;
        };
        tab.thumbs_inflight.remove(&page);
        if generation != tab.thumb_generation {
            return;
        }
        let image = match result {
            Rendered::Skipped => return,
            Rendered::Failed => Image::default(),
            Rendered::Done(buffer) => Image::from_rgb8(buffer),
        };
        tab.thumbs.insert(page, image.clone());
        if active && let Some(mut item) = self.models.thumbs.row_data(page) {
            item.image = image;
            self.models.thumbs.set_row_data(page, item);
        }
    }

    /// Scrolls the thumbnail list so `page` is visible.
    fn reveal_thumb(&self, page: usize) {
        let (Some(window), Some(tab)) = (self.window(), self.tab()) else {
            return;
        };
        if !window.get_sidebar_visible() || window.get_sidebar_tab() != 0 {
            return;
        }
        let Some(&t) = tab.thumb_tops.get(page) else {
            return;
        };
        let next = tab.thumb_tops.get(page + 1).copied().unwrap_or(t + 200.0);
        let top = -window.get_thumbs_y();
        let height = window.get_thumbs_view_height();
        if t < top || next > top + height {
            window.set_thumbs_y(-(t - (height - (next - t)) / 2.0).max(0.0));
        }
    }

    fn rebuild_outline(&mut self) {
        let Some(tab) = self.tab_mut() else { return };
        let mut rows = Vec::new();
        let mut hide_below: Option<usize> = None;
        for (i, item) in tab.outline.iter().enumerate() {
            if let Some(d) = hide_below {
                if item.depth > d {
                    continue;
                }
                hide_below = None;
            }
            rows.push(i);
            let has_children = tab.outline.get(i + 1).is_some_and(|n| n.depth > item.depth);
            if has_children && !tab.expanded.contains(&i) {
                hide_below = Some(item.depth);
            }
        }
        tab.outline_rows = rows;
        tab.outline_current = None;
        self.refresh_outline_current();
        self.fill_outline();
    }

    fn fill_outline(&self) {
        let Some(tab) = self.tab() else { return };
        let items: Vec<OutlineRow> = tab
            .outline_rows
            .iter()
            .map(|&i| {
                let item = &tab.outline[i];
                OutlineRow {
                    title: item.title.clone().into(),
                    depth: item.depth as i32,
                    has_children: tab.outline.get(i + 1).is_some_and(|n| n.depth > item.depth),
                    expanded: tab.expanded.contains(&i),
                    current: tab.outline_current == Some(i),
                }
            })
            .collect();
        self.models.outline.set_vec(items);
    }

    fn refresh_outline_current(&mut self) {
        let Some(tab) = self.active.and_then(|i| self.tabs.get_mut(i)) else {
            return;
        };
        let current = tab.current;
        let best = tab
            .outline_rows
            .iter()
            .copied()
            .filter(|&i| {
                matches!(tab.outline[i].target, Some(LinkTarget::Page { page, .. }) if page <= current)
            })
            .max_by_key(|&i| match tab.outline[i].target {
                Some(LinkTarget::Page { page, .. }) => (page, i),
                _ => (0, i),
            });
        if best != tab.outline_current {
            let old = tab.outline_current;
            tab.outline_current = best;
            for (row, i) in tab.outline_rows.iter().enumerate() {
                if (Some(*i) == old || Some(*i) == best)
                    && let Some(mut item) = self.models.outline.row_data(row)
                {
                    item.current = Some(*i) == best;
                    self.models.outline.set_row_data(row, item);
                }
            }
        }
    }

    pub fn outline_clicked(&mut self, row: usize) {
        let Some(target) = self.tab().and_then(|t| {
            t.outline_rows
                .get(row)
                .map(|&i| t.outline[i].target.clone())
        }) else {
            return;
        };
        match target {
            Some(LinkTarget::Page { page, top }) => self.go_to(page, top, true),
            Some(LinkTarget::Uri(uri)) => self.confirm_uri(uri),
            None => self.outline_toggle(row),
        }
    }

    pub fn outline_toggle(&mut self, row: usize) {
        let Some(tab) = self.tab_mut() else { return };
        let Some(&i) = tab.outline_rows.get(row) else {
            return;
        };
        if !tab.expanded.remove(&i) {
            tab.expanded.insert(i);
        }
        self.rebuild_outline();
    }

    fn rebuild_info(&self) {
        let Some(tab) = self.tab() else { return };
        let mut rows = vec![
            ("File".to_string(), tab.name()),
            (
                "Folder".into(),
                tab.path
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            ),
        ];
        if let Ok(meta) = std::fs::metadata(&tab.path) {
            rows.push(("Size".into(), format_size(meta.len())));
        }
        rows.push(("Pages".into(), tab.page_count().to_string()));
        let (w, h) = tab.sizes[0];
        rows.push((
            "First page".into(),
            format!(
                "{:.2} × {:.2} in ({:.0} × {:.0} mm)",
                w / 72.0,
                h / 72.0,
                w / 72.0 * 25.4,
                h / 72.0 * 25.4
            ),
        ));
        rows.extend(
            tab.metadata
                .iter()
                .filter(|(_, v)| !v.trim().is_empty())
                .cloned(),
        );
        let items: Vec<InfoRow> = rows
            .into_iter()
            .map(|(label, value)| InfoRow {
                label: label.into(),
                value: value.into(),
            })
            .collect();
        self.models.info.set_vec(items);
    }

    /// Fills the attachments and layers panels, and leaves a panel the new tab lacks.
    fn rebuild_files(&self) {
        let (Some(window), Some(tab)) = (self.window(), self.tab()) else {
            return;
        };
        self.fill_files();
        self.fill_layers();
        self.fill_comments();
        if window.get_sidebar_tab() == 4 && tab.layers.is_empty() {
            window.set_sidebar_tab(0);
        }
    }

    /// Fills the attachments panel, and leaves it when the tab has no files.
    fn fill_files(&self) {
        let (Some(window), Some(tab)) = (self.window(), self.tab()) else {
            return;
        };
        let files: Vec<AttachmentRow> = tab
            .attachments
            .iter()
            .map(|a| {
                let size = a.size.map(|n| format_size(n as u64));
                let detail = match (size, a.page) {
                    (Some(size), Some(p)) => format!("{size}, page {}", p + 1),
                    (Some(size), None) => size,
                    (None, Some(p)) => format!("page {}", p + 1),
                    (None, None) => String::new(),
                };
                AttachmentRow {
                    name: a.name.clone().into(),
                    detail: detail.into(),
                }
            })
            .collect();
        self.models.attachments.set_vec(files);
        if window.get_sidebar_tab() == 3 && tab.attachments.is_empty() {
            window.set_sidebar_tab(0);
        }
    }

    /// Asks for a file to embed: in the document, or at `at` (page, x, y) as a comment.
    pub fn attach_pick(&mut self, at: Option<(usize, f32, f32)>) {
        let Some(tab) = self.tab() else { return };
        let dir = tab.path.parent().map(Path::to_path_buf);
        std::thread::spawn(move || {
            let mut dialog = rfd::FileDialog::new().set_title("Attach a file");
            if let Some(dir) = dir {
                dialog = dialog.set_directory(dir);
            }
            let Some(file) = dialog.pick_file() else {
                return;
            };
            let _ = slint::invoke_from_event_loop(move || {
                with(|app| app.attach(&file, at));
            });
        });
    }

    /// Embeds the file at `path`: in the document, or at `at` (page, x, y) as a comment.
    pub fn attach(&mut self, path: &Path, at: Option<(usize, f32, f32)>) {
        let Some(doc) = self.tab().map(|t| t.info.id) else {
            return;
        };
        let name = file_name(path);
        let data = match std::fs::read(path) {
            Ok(data) => data,
            Err(e) => {
                self.status(format!("Could not read {name}: {e}"));
                return;
            }
        };
        if let Some((page, x, y)) = at {
            let new = NewAnnot::File {
                at: (x, y),
                name,
                data,
            };
            self.add_comment(page, new, [0.1, 0.3, 0.7]);
            self.set_tool(Tool::Select);
            return;
        }
        match self.engine.add_attachment(doc, name.clone(), data) {
            Ok(()) => {
                self.edited(None);
                if let Some(w) = self.window() {
                    w.set_sidebar_tab(3);
                }
                self.status(format!("Attached {name}"));
            }
            Err(e) => self.status(format!("Could not attach {name}: {e}")),
        }
    }

    /// Removes attachment `index`; a file attached to a page goes with its comment.
    pub fn attachment_delete(&mut self, index: usize) {
        let Some(tab) = self.tab() else { return };
        let Some(file) = tab.attachments.get(index) else {
            return;
        };
        let (doc, page, name) = (tab.info.id, file.page, file.name.clone());
        match self.engine.delete_attachment(doc, index) {
            Ok(()) => {
                self.edited(page);
                self.status(format!("Deleted {name}"));
            }
            Err(e) => self.status(format!("Could not delete {name}: {e}")),
        }
    }

    fn fill_layers(&self) {
        let Some(tab) = self.tab() else { return };
        let rows: Vec<LayerRow> = tab
            .layers
            .iter()
            .map(|l| LayerRow {
                name: l.name.clone().into(),
                depth: l.depth as i32,
                toggle: l.toggle,
                visible: l.visible,
                locked: l.locked,
            })
            .collect();
        self.models.layers.set_vec(rows);
    }

    pub fn layer_toggle(&mut self, index: usize) {
        let Some(tab) = self.tab() else { return };
        let Ok(layers) = self.engine.toggle_layer(tab.info.id, index) else {
            return;
        };
        let thumb_generation = self.bump();
        let Some(tab) = self.tab_mut() else { return };
        tab.layers = layers;
        // Every rendered pixel may change: force new tiles, thumbnails and text.
        tab.signature = (0, 0, ReadingMode::Normal);
        tab.tiles.clear();
        tab.thumbs.clear();
        tab.thumb_generation = thumb_generation;
        tab.texts.clear();
        self.fill_layers();
        self.rebuild_thumbs();
        self.update_view();
    }

    /// Asks where to save attachment `index`, then writes it, off the UI thread.
    pub fn attachment_save(&mut self, index: usize) {
        let Some(tab) = self.tab() else { return };
        let Some(file) = tab.attachments.get(index) else {
            return;
        };
        let (engine, doc) = (Arc::clone(&self.engine), tab.info.id);
        let name = file.name.clone();
        let dir = tab.path.parent().map(Path::to_path_buf);
        std::thread::spawn(move || {
            let mut dialog = rfd::FileDialog::new().set_file_name(&name);
            if let Some(dir) = dir {
                dialog = dialog.set_directory(dir);
            }
            let Some(target) = dialog.save_file() else {
                return;
            };
            let message = match engine
                .attachment_data(doc, index)
                .map_err(|e| e.to_string())
                .and_then(|data| std::fs::write(&target, data).map_err(|e| e.to_string()))
            {
                Ok(()) => format!("Saved {}", target.display()),
                Err(e) => format!("Could not save {name}: {e}"),
            };
            let _ = slint::invoke_from_event_loop(move || {
                with(|app| app.status(message));
            });
        });
    }

    // ---------------------------------------------------------------- palette

    pub fn open_palette(&mut self) {
        let Some(window) = self.window() else { return };
        window.set_palette_visible(true);
        window.invoke_focus_palette();
        self.palette_edited("");
    }

    pub fn close_palette(&mut self) {
        let Some(window) = self.window() else { return };
        window.set_palette_visible(false);
        window.invoke_focus_view();
    }

    pub fn palette_edited(&mut self, query: &str) {
        let query = query.trim();
        let mut scored: Vec<(i32, PaletteItem, PaletteAction)> = Vec::new();
        let count = self.page_count();
        if let Ok(n) = query.parse::<usize>()
            && n >= 1
            && n <= count
        {
            scored.push((
                i32::MAX,
                PaletteItem {
                    title: format!("Go to page {n}").into(),
                    detail: "".into(),
                    shortcut: "Ctrl+G".into(),
                },
                PaletteAction::Page(n - 1),
            ));
        }
        for c in palette::COMMANDS {
            if let Some(s) = palette::score(query, c.title) {
                scored.push((
                    s + 2,
                    PaletteItem {
                        title: c.title.into(),
                        detail: "".into(),
                        shortcut: c.shortcut.into(),
                    },
                    PaletteAction::Command(c.id),
                ));
            }
        }
        if !query.is_empty() {
            for p in &self.settings.recent {
                if let Some(s) = palette::score(query, &file_name(p)) {
                    scored.push((
                        s,
                        PaletteItem {
                            title: file_name(p).into(),
                            detail: "Recent file".into(),
                            shortcut: "".into(),
                        },
                        PaletteAction::Recent(p.clone()),
                    ));
                }
            }
            if let Some(tab) = self.tab() {
                for (i, item) in tab.outline.iter().enumerate() {
                    if let Some(s) = palette::score(query, &item.title) {
                        let detail = match item.target {
                            Some(LinkTarget::Page { page, .. }) => {
                                format!("Outline · page {}", page + 1)
                            }
                            _ => "Outline".into(),
                        };
                        scored.push((
                            s - 1,
                            PaletteItem {
                                title: item.title.clone().into(),
                                detail: detail.into(),
                                shortcut: "".into(),
                            },
                            PaletteAction::Outline(i),
                        ));
                    }
                }
            }
        }
        if !query.is_empty() {
            scored.sort_by_key(|a| std::cmp::Reverse(a.0));
        }
        scored.truncate(60);
        let (items, actions): (Vec<_>, Vec<_>) = scored.into_iter().map(|(_, i, a)| (i, a)).unzip();
        self.models.palette.set_vec(items);
        self.palette_actions = actions;
        if let Some(w) = self.window() {
            w.set_palette_selected(0);
        }
    }

    /// Returns the command to run, if the chosen entry is one; commands run outside this
    /// borrow because some open modal dialogs.
    pub fn palette_accept(&mut self, index: usize) -> Option<&'static str> {
        let action = self.palette_actions.get(index)?;
        let mut command = None;
        let mut page = None;
        let mut recent = None;
        let mut outline = None;
        match action {
            PaletteAction::Command(id) => command = Some(*id),
            PaletteAction::Page(p) => page = Some(*p),
            PaletteAction::Recent(p) => recent = Some(p.clone()),
            PaletteAction::Outline(i) => outline = Some(*i),
        }
        self.close_palette();
        if let Some(p) = page {
            self.go_to(p, None, true);
        }
        if let Some(p) = recent {
            self.open(p);
        }
        if let Some(i) = outline
            && let Some(target) = self.tab().and_then(|t| t.outline.get(i)?.target.clone())
        {
            match target {
                LinkTarget::Page { page, top } => self.go_to(page, top, true),
                LinkTarget::Uri(uri) => self.confirm_uri(uri),
            }
        }
        command
    }

    pub fn palette_visible(&self) -> bool {
        self.window().is_some_and(|w| w.get_palette_visible())
    }

    pub fn focus_page_field(&self) {
        if let Some(w) = self.window()
            && self.has_document()
        {
            w.invoke_focus_page_field();
        }
    }

    pub fn next_tab(&mut self, forward: bool) {
        let n = self.tabs.len();
        if n < 2 {
            return;
        }
        let a = self.active.unwrap_or(0);
        self.select(if forward {
            (a + 1) % n
        } else {
            (a + n - 1) % n
        });
    }

    // ---------------------------------------------------------------- editing

    /// Window title and tab strip, after a tab's name or unsaved state changed.
    fn refresh_names(&self) {
        self.refresh_tabs();
        if let (Some(window), Some(tab)) = (self.window(), self.tab()) {
            window.set_window_title(format!("{} — micropdf", tab.title()).into());
            window.set_dirty(tab.dirty);
            window.set_undo_name(tab.edits.undo.clone().unwrap_or_default().into());
            window.set_redo_name(tab.edits.redo.clone().unwrap_or_default().into());
        }
    }

    /// Redraws the active tab after an edit changed `page`, or any page when None.
    fn edited(&mut self, page: Option<usize>) {
        self.changed(page, false);
    }

    /// Redraws after an edit, or after undo or redo when `undo` is set; those can return to
    /// the saved state.
    fn changed(&mut self, page: Option<usize>, undo: bool) {
        let thumb_generation = self.bump();
        let Some(tab) = self.tab() else { return };
        let edits = self.engine.history(tab.info.id).unwrap_or_default();
        let attachments = self.engine.attachments(tab.info.id).unwrap_or_default();
        let Some(tab) = self.tab_mut() else { return };
        tab.attachments = attachments;
        if !undo && edits.position <= tab.saved_position {
            // The new step replaced the saved state's steps; no undo reaches it again.
            tab.saved_position = usize::MAX;
        }
        tab.dirty = edits.position != tab.saved_position;
        tab.edits = edits;
        // A new tile generation; the old tiles show until the new ones arrive.
        tab.signature = (0, 0, ReadingMode::Normal);
        tab.thumbs.clear();
        tab.thumbs_inflight.clear();
        tab.thumb_generation = thumb_generation;
        tab.fields.clear();
        match page {
            Some(p) => {
                tab.texts.remove(&p);
                tab.links.remove(&p);
            }
            None => {
                tab.texts.clear();
                tab.links.clear();
                tab.selection = None;
            }
        }
        self.refresh_names();
        self.rebuild_thumbs();
        self.refresh_marks();
        self.update_view();
        self.fill_files();
        self.refresh_comments(page);
    }

    /// Lists every page's comments on a worker thread; the result replaces the tab's list.
    fn scan_comments(&mut self, index: usize) {
        let generation = self.bump();
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        tab.comments_scan = Some(generation);
        let (doc, pages) = (tab.info.id, tab.page_count());
        let engine = Arc::clone(&self.engine);
        std::thread::spawn(move || {
            let mut all = Vec::new();
            for page in 0..pages {
                match engine.annotations(doc, page) {
                    Ok(list) => all.extend(list.into_iter().map(|a| (page, a))),
                    Err(_) => return, // closed meanwhile
                }
            }
            let _ = slint::invoke_from_event_loop(move || {
                with(|app| app.comments_scanned(doc, generation, all));
            });
        });
    }

    fn comments_scanned(&mut self, doc: DocId, generation: u64, all: Vec<(usize, Annot)>) {
        let Some(index) = self
            .tabs
            .iter()
            .position(|t| t.info.id == doc && t.comments_scan == Some(generation))
        else {
            return;
        };
        let tab = &mut self.tabs[index];
        tab.comments = all;
        tab.comments_scan = None;
        if self.active == Some(index) {
            self.fill_comments();
        }
    }

    /// Updates the comment list after an edit on `page`, or on any page when None.
    fn refresh_comments(&mut self, page: Option<usize>) {
        let Some(index) = self.active else { return };
        let tab = &self.tabs[index];
        let (Some(page), None) = (page, tab.comments_scan) else {
            self.scan_comments(index);
            return;
        };
        let list = self
            .engine
            .annotations(tab.info.id, page)
            .unwrap_or_default();
        let tab = &mut self.tabs[index];
        tab.comments.retain(|(p, _)| *p != page);
        let at = tab.comments.partition_point(|(p, _)| *p < page);
        tab.comments
            .splice(at..at, list.into_iter().map(|a| (page, a)));
        self.fill_comments();
    }

    fn fill_comments(&self) {
        let (Some(window), Some(tab)) = (self.window(), self.tab()) else {
            return;
        };
        let comments = &tab.comments;
        let by_id: HashMap<(usize, i32), usize> = comments
            .iter()
            .enumerate()
            .map(|(i, (p, a))| ((*p, a.id), i))
            .collect();
        // The comment a thread starts from: follow replies up. A reply whose parent is gone
        // starts its own thread.
        let root = |mut i: usize| {
            for _ in 0..comments.len() {
                let (p, a) = &comments[i];
                match a.reply_to.and_then(|r| by_id.get(&(*p, r))) {
                    Some(&parent) if parent != i => i = parent,
                    _ => break,
                }
            }
            i
        };
        struct Thread<'a> {
            root: usize,
            replies: Vec<usize>,
            status: Option<&'a str>,
        }
        let mut threads: Vec<Thread> = Vec::new();
        let mut at = HashMap::new();
        for i in 0..comments.len() {
            if root(i) == i {
                at.insert(i, threads.len());
                threads.push(Thread {
                    root: i,
                    replies: Vec::new(),
                    status: None,
                });
            }
        }
        for (i, (_, a)) in comments.iter().enumerate() {
            let r = root(i);
            if r == i {
                continue;
            }
            let thread = &mut threads[at[&r]];
            match a.state.as_deref() {
                // Later states replace earlier ones; "None" clears the status.
                Some(s) if REVIEW_STATES.contains(&s) => {
                    thread.status = (s != "None").then_some(s);
                }
                Some(_) => {} // a private check mark, not shown
                None => thread.replies.push(i),
            }
        }
        let needle = self.comment_filter.trim().to_lowercase();
        let matches = |i: usize, status: Option<&str>| {
            let (page, a) = &comments[i];
            [
                kind_name(a.kind),
                &a.contents,
                &a.author,
                status.unwrap_or_default(),
                &format!("page {}", page + 1),
            ]
            .iter()
            .any(|s| s.to_lowercase().contains(&needle))
        };
        let mut rows = Vec::new();
        for t in &threads {
            if !needle.is_empty()
                && !matches(t.root, t.status)
                && !t.replies.iter().any(|&i| matches(i, None))
            {
                continue;
            }
            let (page, a) = &comments[t.root];
            rows.push(CommentRow {
                index: t.root as i32,
                kind: kind_name(a.kind).into(),
                text: a.contents.lines().next().unwrap_or_default().into(),
                detail: if a.author.is_empty() {
                    format!("page {}", page + 1)
                } else {
                    format!("page {}, {}", page + 1, a.author)
                }
                .into(),
                reply: false,
                status: t.status.unwrap_or_default().into(),
            });
            for &i in &t.replies {
                let a = &comments[i].1;
                let when = pdf_date(&a.modified);
                let detail = match (a.author.is_empty(), when) {
                    (false, Some(when)) => format!("{}, {when}", a.author),
                    (false, None) => a.author.clone(),
                    (true, Some(when)) => when,
                    (true, None) => String::new(),
                };
                rows.push(CommentRow {
                    index: i as i32,
                    kind: "Reply".into(),
                    text: a.contents.lines().next().unwrap_or_default().into(),
                    detail: detail.into(),
                    reply: true,
                    status: SharedString::new(),
                });
            }
        }
        self.models.comments.set_vec(rows);
        window.set_has_comments(!comments.is_empty());
        if window.get_sidebar_tab() == 5 && tab.comments.is_empty() {
            window.set_sidebar_tab(0);
        }
        self.refresh_style_bar();
    }

    pub fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        if tool != Tool::Sign {
            self.placing = None;
        }
        if tool != Tool::Stamp {
            self.stamp = None;
        }
        if let Some(w) = self.window() {
            w.set_tool(tool as i32);
        }
        if tool != Tool::Select {
            self.clear_selection();
        }
        self.refresh_style_bar();
    }

    pub fn tool(&self) -> Tool {
        self.tool
    }

    fn add_comment(&mut self, page: usize, new: NewAnnot, color: [f32; 3]) {
        let Some(doc) = self.tab().map(|t| t.info.id) else {
            return;
        };
        let color = self.chosen_color(new_key(&new)).unwrap_or(color);
        let style = Style {
            color,
            author: user_name(),
        };
        match self.engine.add_annotation(doc, page, new, style) {
            Ok(_) => self.edited(Some(page)),
            Err(e) => self.status(format!("Could not add the comment: {e}")),
        }
    }

    /// Draws the shape being dragged out, in document space.
    fn show_draft(&self) {
        let (Some(window), Some(tab)) = (self.window(), self.tab()) else {
            return;
        };
        let Some(Drag::Draw { page, points }) = &self.drag else {
            return;
        };
        let view = |&(x, y): &(f32, f32)| {
            let r = Rect {
                x0: x,
                y0: y,
                x1: x,
                y1: y,
            };
            tab.layout.to_view(*page, &r).map(|f| (f.x, f.y))
        };
        let points: Vec<(f32, f32)> = points.iter().filter_map(view).collect();
        let (Some(&(ax, ay)), Some(&(bx, by))) = (points.first(), points.last()) else {
            return;
        };
        let path = match self.tool {
            Tool::Ink | Tool::Line => {
                let mut path = format!("M {ax} {ay}");
                for (x, y) in &points[1..] {
                    path += &format!(" L {x} {y}");
                }
                path
            }
            Tool::Callout => format!("M {ax} {ay} L {bx} {by}"),
            Tool::Ellipse => {
                let (rx, ry) = ((bx - ax).abs() / 2.0, (by - ay).abs() / 2.0);
                let (cx, cy) = ((ax + bx) / 2.0, (ay + by) / 2.0);
                format!(
                    "M {} {cy} A {rx} {ry} 0 1 0 {} {cy} A {rx} {ry} 0 1 0 {} {cy} Z",
                    cx - rx,
                    cx + rx,
                    cx - rx
                )
            }
            _ => format!("M {ax} {ay} L {bx} {ay} L {bx} {by} L {ax} {by} Z"),
        };
        window.set_draft_path(path.into());
    }

    fn finish_draw(&mut self, page: usize, points: Vec<(f32, f32)>) {
        let (Some(&a), Some(&b)) = (points.first(), points.last()) else {
            return;
        };
        let rect = Rect {
            x0: a.0.min(b.0),
            y0: a.1.min(b.1),
            x1: a.0.max(b.0),
            y1: a.1.max(b.1),
        };
        let small = rect.width() < 4.0 && rect.height() < 4.0;
        let red = [0.85, 0.15, 0.15];
        match self.tool {
            Tool::Ink if points.len() > 1 => {
                let new = NewAnnot::Ink {
                    strokes: vec![points],
                    width: 2.0,
                };
                self.add_comment(page, new, [0.1, 0.35, 0.9]);
            }
            Tool::Callout => {
                // Pressed on the target, released where the box goes; a click puts the box
                // up and to the right.
                let center = if small { (a.0 + 90.0, a.1 - 50.0) } else { b };
                let (w, h) = (CALLOUT_SIZE.0 / 2.0, CALLOUT_SIZE.1 / 2.0);
                let (cx, cy) = self.inside_page(page, center, (w, h));
                let rect = Rect {
                    x0: cx - w,
                    y0: cy - h,
                    x1: cx + w,
                    y1: cy + h,
                };
                self.push_dialog(Dialog {
                    ask: Ask::Callout {
                        page,
                        target: a,
                        rect,
                    },
                    kind: "input",
                    title: "Add a callout".into(),
                    text: String::new(),
                    ok: "Add",
                    cancel: "Cancel",
                });
            }
            _ if small => self.status("Drag to draw the shape".into()),
            Tool::Line => {
                let new = NewAnnot::Line {
                    from: a,
                    to: b,
                    width: 1.5,
                };
                self.add_comment(page, new, red);
            }
            Tool::Rect | Tool::Ellipse => {
                let kind = if self.tool == Tool::Rect {
                    AnnotKind::Square
                } else {
                    AnnotKind::Circle
                };
                let new = NewAnnot::Shape {
                    kind,
                    rect,
                    width: 1.5,
                };
                self.add_comment(page, new, red);
            }
            Tool::TextBox => self.push_dialog(Dialog {
                ask: Ask::TextBox { page, rect },
                kind: "input",
                title: "Add a text box".into(),
                text: String::new(),
                ok: "Add",
                cancel: "Cancel",
            }),
            _ => {}
        }
    }

    fn field_at(&mut self, x: f32, y: f32) -> Option<(usize, Field)> {
        let (page, px, py) = self.hit(x, y)?;
        self.page_fields(page)
            .iter()
            .find(|f| f.rect.contains(px, py))
            .map(|f| (page, f.clone()))
    }

    /// The form widgets of `page`, loaded once per edit.
    fn page_fields(&mut self, page: usize) -> Rc<Vec<Field>> {
        let Some(tab) = self.tab() else {
            return Rc::default();
        };
        if let Some(f) = tab.fields.get(&page) {
            return Rc::clone(f);
        }
        let f = Rc::new(self.engine.fields(tab.info.id, page).unwrap_or_default());
        if let Some(tab) = self.tab_mut() {
            tab.fields.insert(page, Rc::clone(&f));
        }
        f
    }

    fn use_field(&mut self, page: usize, field: Field) {
        if field.read_only {
            self.status("This field is read-only".into());
            return;
        }
        match field.kind {
            FieldKind::Checkbox | FieldKind::Radio => {
                self.edit_field(page, field.id, FieldEdit::Toggle);
                self.focus_field(page, field, false);
            }
            FieldKind::Text | FieldKind::Choice => self.focus_field(page, field, true),
            FieldKind::Signature => self.status("Signing is not supported yet".into()),
            FieldKind::Button | FieldKind::Other => {}
        }
    }

    /// Puts the keyboard on `field`: a text field opens for typing over the page, a list or
    /// combo box lists its options when `open` is set, and other fields get a focus ring.
    fn focus_field(&mut self, page: usize, field: Field, open: bool) {
        let Some(doc) = self.tab().map(|t| t.info.id) else {
            return;
        };
        let Some(w) = self.window() else { return };
        let kind = field.kind;
        let editing = kind == FieldKind::Text;
        w.set_field_text(field.value.clone().into());
        w.set_field_multiline(field.multiline);
        w.set_field_name(field.name.clone().into());
        let options: Vec<SharedString> = field.options.iter().map(Into::into).collect();
        w.set_field_options(ModelRc::new(VecModel::from(options)));
        self.field_focus = Some(FieldFocus {
            doc,
            page,
            field,
            editing,
        });
        self.reveal_field();
        self.refresh_marks();
        if editing {
            w.invoke_focus_field_editor();
        } else {
            w.invoke_focus_view();
            if kind == FieldKind::Choice && open {
                w.invoke_show_field_menu();
            }
        }
    }

    /// Scrolls the focused field into view.
    fn reveal_field(&mut self) {
        let Some((page, rect)) = self.field_focus.as_ref().map(|f| (f.page, f.field.rect)) else {
            return;
        };
        let Some(tab) = self.tab() else { return };
        if tab.mode == PageMode::Single && tab.current != page {
            self.go_to(page, Some((rect.y0 - 40.0).max(0.0)), false);
            return;
        }
        let Some(frame) = tab.layout.to_view(page, &rect) else {
            return;
        };
        let (x, y) = self.scroll();
        let (_, vh) = self.view_size();
        if frame.y < y || frame.y + frame.height > y + vh {
            self.set_scroll(x, frame.y - vh / 3.0);
            self.update_view();
        }
    }

    /// Lays the field editor over the focused text field, or hides it.
    fn place_field_editor(&self) {
        let Some(w) = self.window() else { return };
        let placed = self
            .field_focus
            .as_ref()
            .filter(|f| f.editing)
            .and_then(|f| {
                let tab = self.tab().filter(|t| t.info.id == f.doc)?;
                Some((f, tab.layout.to_view(f.page, &f.field.rect)?))
            });
        let Some((f, frame)) = placed else {
            w.set_field_editing(false);
            return;
        };
        let rect = f.field.rect;
        let scale = frame.height / rect.height().max(1.0);
        let points = if f.field.multiline {
            10.0
        } else {
            (rect.height() * 0.6).clamp(6.0, 14.0)
        };
        w.set_field_x(frame.x);
        w.set_field_y(frame.y);
        w.set_field_width(frame.width);
        w.set_field_height(frame.height);
        w.set_field_font_size(points * scale);
        w.set_field_editing(true);
    }

    /// Ends typing in the focused field and keeps the text. `step` 1 or -1 moves on to the
    /// next or previous field; 0 and 2 (the editor lost the keyboard) leave the form.
    pub fn field_commit(&mut self, step: i32) {
        let Some(f) = self.field_focus.as_mut().filter(|f| f.editing) else {
            return;
        };
        f.editing = false;
        let (page, id, old) = (f.page, f.field.id, f.field.value.clone());
        let text = self
            .window()
            .map(|w| w.get_field_text().to_string())
            .unwrap_or_default();
        if !matches!(step, 1 | -1) {
            self.field_focus = None;
        }
        self.place_field_editor();
        if text != old {
            self.edit_field(page, id, FieldEdit::Value(text));
        }
        match step {
            1 | -1 => {
                self.field_tab(step > 0);
            }
            0 => {
                self.refresh_marks();
                if let Some(w) = self.window() {
                    w.invoke_focus_view();
                }
            }
            _ => self.refresh_marks(),
        }
    }

    /// Ends typing in the focused field and drops the text.
    pub fn field_cancel(&mut self) {
        if self.field_focus.take().is_some() {
            self.refresh_marks();
        }
        if let Some(w) = self.window() {
            w.invoke_focus_view();
        }
    }

    /// Option `index` of the focused list or combo box was picked.
    pub fn field_choose(&mut self, index: usize) {
        let Some(f) = &self.field_focus else { return };
        let Some(option) = f.field.options.get(index).cloned() else {
            return;
        };
        let (page, id, old) = (f.page, f.field.id, f.field.value.clone());
        if option != old {
            self.edit_field(page, id, FieldEdit::Value(option.clone()));
        }
        if let Some(f) = self.field_focus.as_mut() {
            f.field.value = option;
        }
    }

    /// Commits the field being typed into, if any.
    fn commit_open_field(&mut self) {
        if self.field_focus.as_ref().is_some_and(|f| f.editing) {
            self.field_commit(2);
        }
    }

    /// Takes the keyboard off the form. False if no field had it.
    pub fn clear_field_focus(&mut self) -> bool {
        if self.field_focus.is_none() {
            return false;
        }
        self.field_cancel();
        true
    }

    /// Moves the keyboard to the next (or previous) field that takes input, across pages and
    /// round to the start. False when the document has no fields.
    pub fn field_tab(&mut self, forward: bool) -> bool {
        let Some(tab) = self.tab() else { return false };
        let (doc, count, current) = (tab.info.id, tab.page_count(), tab.current);
        if count == 0 || !self.engine.has_fields(doc).unwrap_or(false) {
            return false;
        }
        let (mut page, mut after) = match &self.field_focus {
            Some(f) if f.doc == doc => (f.page, Some(f.field.id)),
            _ => (current.min(count - 1), None),
        };
        for _ in 0..=count {
            let mut usable: Vec<Field> = self
                .page_fields(page)
                .iter()
                .filter(|f| takes_input(f))
                .cloned()
                .collect();
            if !forward {
                usable.reverse();
            }
            let next = match after {
                Some(id) => usable
                    .iter()
                    .position(|f| f.id == id)
                    .and_then(|i| usable.get(i + 1)),
                None => usable.first(),
            };
            if let Some(field) = next.cloned() {
                self.focus_field(page, field, false);
                return true;
            }
            after = None;
            page = if forward {
                (page + 1) % count
            } else {
                (page + count - 1) % count
            };
        }
        false
    }

    /// Space or Enter on the focused field: checks a box, lists a choice's options or opens
    /// a text field for typing. False if no field has the keyboard.
    pub fn use_focused_field(&mut self) -> bool {
        let doc = self.tab().map(|t| t.info.id);
        let Some(f) = self
            .field_focus
            .as_ref()
            .filter(|f| !f.editing && Some(f.doc) == doc)
        else {
            return false;
        };
        let (page, field) = (f.page, f.field.clone());
        match field.kind {
            FieldKind::Checkbox | FieldKind::Radio => {
                self.edit_field(page, field.id, FieldEdit::Toggle)
            }
            FieldKind::Choice | FieldKind::Text => self.focus_field(page, field, true),
            _ => return false,
        }
        true
    }

    fn edit_field(&mut self, page: usize, id: i32, edit: FieldEdit) {
        let Some(doc) = self.tab().map(|t| t.info.id) else {
            return;
        };
        match self.engine.edit_field(doc, page, id, edit) {
            // Calculated fields may change other pages.
            Ok(()) => self.edited(None),
            Err(e) => self.status(format!("Could not fill in the field: {e}")),
        }
    }

    pub fn flatten(&mut self, comments: bool, fields: bool) {
        let Some(doc) = self.tab().map(|t| t.info.id) else {
            return;
        };
        match self.engine.flatten(doc, comments, fields) {
            Ok(()) => self.edited(None),
            Err(e) => self.status(format!("Could not flatten: {e}")),
        }
    }

    pub fn comment_filter_edited(&mut self, text: String) {
        self.comment_filter = text;
        self.fill_comments();
    }

    /// Asks for a reply to comment `index`.
    pub fn comment_reply(&mut self, index: usize) {
        let Some((page, parent, kind)) = self
            .tab()
            .and_then(|t| t.comments.get(index))
            .map(|(p, a)| (*p, a.id, a.kind))
        else {
            return;
        };
        self.push_dialog(Dialog {
            ask: Ask::Reply { page, parent },
            kind: "input",
            title: format!("Reply to {}", kind_name(kind).to_lowercase()),
            text: String::new(),
            ok: "Reply",
            cancel: "Cancel",
        });
    }

    /// Gives comment `index` a review state, one of [`REVIEW_STATES`].
    pub fn comment_status(&mut self, index: usize, state: &str) {
        let Some((doc, page, id)) = self
            .tab()
            .and_then(|t| t.comments.get(index).map(|(p, a)| (t.info.id, *p, a.id)))
        else {
            return;
        };
        match self
            .engine
            .set_state(doc, page, id, state.to_owned(), user_name())
        {
            Ok(()) => self.edited(Some(page)),
            Err(e) => self.status(format!("Could not set the status: {e}")),
        }
    }

    pub fn reply_picked(&mut self) {
        match self.picked_index() {
            Some(index) => self.comment_reply(index),
            None => self.status("Select a comment first".into()),
        }
    }

    pub fn status_picked(&mut self, state: &str) {
        match self.picked_index() {
            Some(index) => self.comment_status(index, state),
            None => self.status("Select a comment first".into()),
        }
    }

    pub fn comment_edit(&mut self, index: usize) {
        let Some((page, id, value, kind)) = self.tab().and_then(|t| {
            let (p, a) = t.comments.get(index)?;
            Some((*p, a.id, a.contents.clone(), a.kind))
        }) else {
            return;
        };
        self.push_dialog(Dialog {
            ask: Ask::Comment { page, id, value },
            kind: "input",
            title: format!("Edit {}", kind_name(kind).to_lowercase()),
            text: String::new(),
            ok: "Save",
            cancel: "Cancel",
        });
    }

    // ---------------------------------------------------------------- signatures

    fn refresh_sign_menu(&self) {
        if let Some(w) = self.window() {
            w.set_has_signature(self.settings.signature.is_some());
            w.set_has_initials(self.settings.initials.is_some());
        }
    }

    /// Starts the Sign tool with the saved signature or initials, or opens the pad to make one.
    pub fn sign(&mut self, initials: bool) {
        if !self.has_document() {
            return;
        }
        let saved = if initials {
            &self.settings.initials
        } else {
            &self.settings.signature
        };
        let Some(saved) = saved.clone() else {
            self.open_pad(initials);
            return;
        };
        match saved.to_mark() {
            Ok(mark) => self.start_placing(mark, initials),
            Err(e) => self.status(format!(
                "Could not read the saved {}: {e}",
                mark_name(initials)
            )),
        }
    }

    pub fn open_pad(&mut self, initials: bool) {
        let Some(w) = self.window() else { return };
        if w.get_sign_fonts().row_count() == 0 {
            let mut fonts: Vec<SharedString> = SIGN_FONTS
                .iter()
                .filter(|f| mp_engine::has_font(f))
                .take(4)
                .map(|&f| f.into())
                .collect();
            if fonts.is_empty() {
                fonts.push("Arial".into());
            }
            w.set_sign_fonts(ModelRc::new(VecModel::from(fonts)));
        }
        let saved = if initials {
            &self.settings.initials
        } else {
            &self.settings.signature
        };
        let text = match saved {
            Some(SavedMark::Typed { text, .. }) => text.clone(),
            _ => String::new(),
        };
        w.set_sign_text(text.into());
        w.set_sign_path("".into());
        w.set_sign_image(Image::default());
        w.set_sign_kind(if initials { "initials" } else { "signature" }.into());
        w.invoke_focus_sign();
        self.pad = Some(Pad {
            initials,
            strokes: Vec::new(),
            image: None,
        });
    }

    pub fn pad_visible(&self) -> bool {
        self.pad.is_some()
    }

    pub fn close_pad(&mut self) {
        self.pad = None;
        if let Some(w) = self.window() {
            w.set_sign_kind("".into());
            w.invoke_focus_view();
        }
    }

    pub fn pad_down(&mut self, x: f32, y: f32) {
        if let Some(pad) = &mut self.pad {
            pad.strokes.push(vec![(x, y)]);
            self.show_pad();
        }
    }

    pub fn pad_move(&mut self, x: f32, y: f32) {
        let Some(stroke) = self.pad.as_mut().and_then(|p| p.strokes.last_mut()) else {
            return;
        };
        if stroke
            .last()
            .is_some_and(|&(lx, ly)| (lx - x).hypot(ly - y) < 1.0)
        {
            return;
        }
        stroke.push((x, y));
        self.show_pad();
    }

    pub fn pad_clear(&mut self) {
        if let Some(pad) = &mut self.pad {
            pad.strokes.clear();
            self.show_pad();
        }
    }

    fn show_pad(&self) {
        let (Some(w), Some(pad)) = (self.window(), &self.pad) else {
            return;
        };
        let mut path = String::new();
        for stroke in &pad.strokes {
            let Some(&(x, y)) = stroke.first() else {
                continue;
            };
            path += &format!("M {x:.1} {y:.1} ");
            // A lone point shows as a dot.
            if stroke.len() == 1 {
                path += &format!("L {:.1} {y:.1} ", x + 0.5);
            }
            for &(x, y) in &stroke[1..] {
                path += &format!("L {x:.1} {y:.1} ");
            }
        }
        w.set_sign_path(path.into());
    }

    pub fn pad_image(&mut self, path: PathBuf) {
        let Some(w) = self.window() else { return };
        match Image::load_from_path(&path) {
            Ok(image) => {
                w.set_sign_image(image);
                if let Some(pad) = &mut self.pad {
                    pad.image = Some(path);
                }
            }
            Err(_) => self.status(format!("Could not open {}", file_name(&path))),
        }
    }

    /// Keeps the pad's mark in the settings and starts the Sign tool with it.
    pub fn pad_done(&mut self) {
        let (Some(w), Some(pad)) = (self.window(), self.pad.as_ref()) else {
            return;
        };
        let initials = pad.initials;
        let (mark, saved) = match w.get_sign_mode() {
            0 => {
                let text = w.get_sign_text().trim().to_owned();
                if text.is_empty() {
                    self.status(format!("Type your {} first", mark_name(initials)));
                    return;
                }
                let font = w
                    .get_sign_fonts()
                    .row_data(w.get_sign_font().max(0) as usize)
                    .map_or_else(|| "Arial".to_owned(), |f| f.to_string());
                let saved = SavedMark::Typed { text, font };
                (saved.to_mark().expect("typed marks need no file"), saved)
            }
            1 => {
                if pad.strokes.is_empty() {
                    self.status(format!("Draw your {} first", mark_name(initials)));
                    return;
                }
                let saved = SavedMark::Ink {
                    strokes: pad.strokes.clone(),
                    width: PEN_WIDTH,
                };
                (saved.to_mark().expect("drawn marks need no file"), saved)
            }
            _ => {
                let Some(path) = pad.image.clone() else {
                    self.status("Choose an image first".into());
                    return;
                };
                let mark = match std::fs::read(&path) {
                    Ok(bytes) => Mark::Image(bytes),
                    Err(e) => {
                        self.status(format!("Could not read {}: {e}", file_name(&path)));
                        return;
                    }
                };
                (mark, SavedMark::Image { file: path })
            }
        };
        // An image MuPDF cannot read, or a font that went away, fails here, before it is kept.
        let aspect = match self.engine.mark_aspect(mark.clone()) {
            Ok(a) => a,
            Err(e) => {
                self.status(format!("Could not use that {}: {e}", mark_name(initials)));
                return;
            }
        };
        let saved = match saved {
            SavedMark::Image { file } => match self.keep_image(&file, initials) {
                Ok(file) => SavedMark::Image { file },
                Err(e) => {
                    self.status(format!("Could not keep the image: {e}"));
                    return;
                }
            },
            other => other,
        };
        if initials {
            self.settings.initials = Some(saved);
        } else {
            self.settings.signature = Some(saved);
        }
        self.save_settings();
        self.refresh_sign_menu();
        self.close_pad();
        self.placing = Some(Placing {
            mark,
            initials,
            aspect,
        });
        self.set_tool(Tool::Sign);
        self.sign_hint(initials);
    }

    /// Copies a signature image beside the settings file, so the mark outlives the original.
    fn keep_image(&self, file: &Path, initials: bool) -> std::io::Result<PathBuf> {
        let Some(dir) = crate::settings::dir().filter(|_| self.persist) else {
            return Ok(file.to_path_buf());
        };
        std::fs::create_dir_all(&dir)?;
        let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("png");
        let target = dir.join(format!("{}.{ext}", mark_name(initials)));
        if target != file {
            self.forget_image(initials);
            std::fs::copy(file, &target)?;
        }
        Ok(target)
    }

    /// Removes a kept signature image, if the settings point at one in the settings folder.
    fn forget_image(&self, initials: bool) {
        let saved = if initials {
            &self.settings.initials
        } else {
            &self.settings.signature
        };
        if let (Some(SavedMark::Image { file }), Some(dir)) = (saved, crate::settings::dir())
            && self.persist
            && file.parent() == Some(dir.as_path())
        {
            let _ = std::fs::remove_file(file);
        }
    }

    pub fn forget_marks(&mut self) {
        self.forget_image(false);
        self.forget_image(true);
        self.settings.signature = None;
        self.settings.initials = None;
        self.save_settings();
        self.refresh_sign_menu();
        if self.tool == Tool::Sign {
            self.set_tool(Tool::Select);
        }
        self.status("Forgot the saved signature and initials".into());
    }

    fn start_placing(&mut self, mark: Mark, initials: bool) {
        match self.engine.mark_aspect(mark.clone()) {
            Ok(aspect) => {
                self.placing = Some(Placing {
                    mark,
                    initials,
                    aspect,
                });
                self.set_tool(Tool::Sign);
                self.sign_hint(initials);
            }
            Err(e) => self.status(format!(
                "Could not use the saved {}: {e}",
                mark_name(initials)
            )),
        }
    }

    fn sign_hint(&mut self, initials: bool) {
        self.status(format!(
            "Click where your {} goes. Esc cancels.",
            mark_name(initials)
        ));
    }

    pub fn show_stamp_menu(&mut self) {
        if self.has_document()
            && let Some(w) = self.window()
        {
            w.invoke_show_stamp_menu();
        }
    }

    /// Starts the Stamp tool with the standard stamp `name`.
    pub fn start_stamp(&mut self, name: &str) {
        let Some(&(name, label)) = STAMPS.iter().find(|(n, _)| *n == name) else {
            return;
        };
        if !self.has_document() {
            return;
        }
        self.set_tool(Tool::Stamp);
        self.stamp = Some(name);
        self.status(format!(
            "Click the page to place the {label} stamp. Esc cancels."
        ));
    }

    fn place_stamp(&mut self, page: usize, x: f32, y: f32) {
        let Some(name) = self.stamp else { return };
        let half = (STAMP_WIDTH / 2.0, STAMP_WIDTH / 2.0 * 50.0 / 190.0);
        let new = NewAnnot::Stamp {
            name: name.to_owned(),
            center: self.inside_page(page, (x, y), half),
            width: STAMP_WIDTH,
        };
        self.add_comment(page, new, stamp_color(name));
        self.set_tool(Tool::Select);
    }

    /// Moves `center` so a box `half` its size each way stays on `page`.
    fn inside_page(&self, page: usize, center: (f32, f32), half: (f32, f32)) -> (f32, f32) {
        let Some(&(pw, ph)) = self.tab().and_then(|t| t.sizes.get(page)) else {
            return center;
        };
        (
            center.0.clamp(half.0, (pw - half.0).max(half.0)),
            center.1.clamp(half.1, (ph - half.1).max(half.1)),
        )
    }

    fn place_mark(&mut self, page: usize, x: f32, y: f32) {
        let (Some(doc), Some(placing)) = (self.tab().map(|t| t.info.id), &self.placing) else {
            return;
        };
        let height = if placing.initials {
            INITIALS_HEIGHT
        } else {
            SIGNATURE_HEIGHT
        };
        let width = height * placing.aspect.min(MAX_ASPECT);
        let mark = placing.mark.clone();
        match self
            .engine
            .place_mark(doc, page, mark, (x, y), width, [0.0, 0.0, 0.0])
        {
            Ok(_) => {
                self.set_tool(Tool::Select);
                self.edited(Some(page));
            }
            Err(e) => self.status(format!("Could not sign: {e}")),
        }
    }

    /// Writes the active tab's form values to `target` as XFDF.
    pub fn export_form(&mut self, target: PathBuf) {
        let Some(tab) = self.tab() else { return };
        let (doc, name) = (tab.info.id, tab.name());
        let data = if is_fdf(&target) {
            self.engine.export_fdf(doc, name)
        } else {
            self.engine.export_xfdf(doc, name).map(String::into_bytes)
        };
        let result = data
            .map_err(|e| e.to_string())
            .and_then(|data| std::fs::write(&target, data).map_err(|e| e.to_string()));
        match result {
            Ok(()) => self.status(format!("Exported form data to {}", file_name(&target))),
            Err(e) => self.message("Could not export form data", e),
        }
    }

    /// Fills the active tab's form from the XFDF or FDF file at `path`.
    pub fn import_form(&mut self, path: PathBuf) {
        let Some(doc) = self.tab().map(|t| t.info.id) else {
            return;
        };
        let result = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|data| {
                if data.starts_with(b"%FDF") {
                    self.engine.import_fdf(doc, data)
                } else {
                    let xml = String::from_utf8_lossy(&data).into_owned();
                    self.engine.import_xfdf(doc, xml)
                }
                .map_err(|e| e.to_string())
            });
        match result {
            Ok(n) => {
                self.edited(None);
                self.status(format!("Filled in {n} fields from {}", file_name(&path)));
            }
            Err(e) => self.message("Could not import form data", e),
        }
    }

    /// Writes the active tab's comments to `target` as XFDF.
    pub fn export_comments(&mut self, target: PathBuf) {
        let Some(tab) = self.tab() else { return };
        let (doc, name) = (tab.info.id, tab.name());
        let result = self
            .engine
            .export_comments(doc, name)
            .map_err(|e| e.to_string())
            .and_then(|(xml, n)| {
                std::fs::write(&target, xml)
                    .map(|()| n)
                    .map_err(|e| e.to_string())
            });
        match result {
            Ok(n) => self.status(format!(
                "Exported {} to {}",
                plural(n, "comment"),
                file_name(&target)
            )),
            Err(e) => self.message("Could not export comments", e),
        }
    }

    /// Adds the comments in the XFDF file at `path` to the active tab. Comments it already
    /// holds (by name) are skipped, so importing a file twice adds nothing.
    pub fn import_comments(&mut self, path: PathBuf) {
        let Some(doc) = self.tab().map(|t| t.info.id) else {
            return;
        };
        let result = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|xml| {
                self.engine
                    .import_comments(doc, xml)
                    .map_err(|e| e.to_string())
            });
        match result {
            Ok(0) => self.status(format!("{} holds no new comments", file_name(&path))),
            Ok(n) => {
                self.edited(None);
                self.status(format!(
                    "Imported {} from {}",
                    plural(n, "comment"),
                    file_name(&path)
                ));
            }
            Err(e) => self.message("Could not import comments", e),
        }
    }

    pub fn reset_form(&mut self) {
        let Some(doc) = self.tab().map(|t| t.info.id) else {
            return;
        };
        match self.engine.reset_form(doc) {
            Ok(()) => self.edited(None),
            Err(e) => self.status(format!("Could not reset the form: {e}")),
        }
    }

    pub fn comment_clicked(&mut self, index: usize) {
        let Some((page, top)) = self
            .tab()
            .and_then(|t| t.comments.get(index))
            .map(|(p, a)| (*p, (a.rect.y0 - 36.0).max(0.0)))
        else {
            return;
        };
        self.go_to(page, Some(top), true);
        self.pick_comment(index);
    }

    /// The comment under a document-space point; the topmost if several overlap.
    fn comment_at(&self, x: f32, y: f32) -> Option<usize> {
        let (page, px, py) = self.hit(x, y)?;
        self.tab()?
            .comments
            .iter()
            .rposition(|(p, a)| *p == page && a.reply_to.is_none() && a.rect.contains(px, py))
    }

    /// The movable comment under a document-space point; form fields come first.
    fn movable_at(&mut self, x: f32, y: f32) -> Option<usize> {
        let index = self.comment_at(x, y)?;
        let (_, a) = self.tab()?.comments.get(index)?;
        (movable(a.kind) && self.field_at(x, y).is_none()).then_some(index)
    }

    fn picked_index(&self) -> Option<usize> {
        let tab = self.tab()?;
        let (page, id) = tab.picked?;
        tab.comments
            .iter()
            .position(|(p, a)| *p == page && a.id == id)
    }

    /// The picked comment's corner handle under a document-space point: 0 top left, then
    /// clockwise.
    fn handle_at(&self, x: f32, y: f32) -> Option<usize> {
        let tab = self.tab()?;
        let (page, a) = tab.comments.get(self.picked_index()?)?;
        if !resizable(a.kind) {
            return None;
        }
        let f = tab.layout.to_view(*page, &a.rect)?;
        corners(f)
            .iter()
            .position(|&(cx, cy)| (x - cx).abs() <= HANDLE + 1.0 && (y - cy).abs() <= HANDLE + 1.0)
    }

    /// Starts moving the comment under a point, or resizing the picked one by a corner.
    fn shape_drag(&mut self, x: f32, y: f32) -> Option<Drag> {
        let handle = self.handle_at(x, y);
        let index = match handle {
            Some(_) => self.picked_index()?,
            None => self.movable_at(x, y)?,
        };
        let (page, a) = self.tab()?.comments.get(index)?.clone();
        let start = self.page_point(page, x, y)?;
        self.pick_comment(index);
        Some(Drag::Shape {
            page,
            id: a.id,
            origin: (x, y),
            start,
            rect: a.rect,
            resize: handle.is_some(),
            keep_aspect: a.kind == AnnotKind::Stamp,
            current: a.rect,
            moved: false,
        })
    }

    /// Moves the picked comment `step` points in the screen direction (`dx`, `dy`). False if
    /// no comment that can move is picked.
    pub fn nudge_picked(&mut self, dx: f32, dy: f32, step: f32) -> bool {
        let Some((page, a)) = self
            .picked_index()
            .and_then(|i| self.tab()?.comments.get(i).cloned())
        else {
            return false;
        };
        if !movable(a.kind) || a.reply_to.is_some() {
            return false;
        }
        // The view may be turned; find the page-space direction through the layout.
        let Some(f) = self.tab().and_then(|t| t.layout.to_view(page, &a.rect)) else {
            return false;
        };
        let (cx, cy) = (f.x + f.width / 2.0, f.y + f.height / 2.0);
        let (Some(from), Some(to)) = (
            self.page_point(page, cx, cy),
            self.page_point(page, cx + dx * 10.0, cy + dy * 10.0),
        ) else {
            return false;
        };
        let (ex, ey) = (to.0 - from.0, to.1 - from.1);
        let len = (ex * ex + ey * ey).sqrt();
        if len > 0.001 {
            let (mx, my) = (ex / len * step, ey / len * step);
            let r = a.rect;
            self.reshape(
                page,
                a.id,
                Rect {
                    x0: r.x0 + mx,
                    y0: r.y0 + my,
                    x1: r.x1 + mx,
                    y1: r.y1 + my,
                },
            );
        }
        true
    }

    fn reshape(&mut self, page: usize, id: i32, rect: Rect) {
        let Some(doc) = self.tab().map(|t| t.info.id) else {
            return;
        };
        match self.engine.reshape(doc, page, id, rect) {
            Ok(()) => self.edited(Some(page)),
            Err(e) => self.status(format!("Could not move the comment: {e}")),
        }
    }

    fn pick_comment(&mut self, index: usize) {
        let Some(tab) = self.tab_mut() else { return };
        let Some((page, a)) = tab.comments.get(index) else {
            return;
        };
        tab.picked = Some((*page, a.id));
        tab.selection = None;
        let (kind, can_move) = (kind_name(a.kind), movable(a.kind));
        self.refresh_marks();
        let message = if can_move {
            format!("{kind} selected. Drag to move it; Delete removes it; double-click edits it.")
        } else {
            format!("{kind} selected. Delete removes it; double-click edits its text.")
        };
        self.status(message);
    }

    /// Changes the picked comment's colour, fill, line width or opacity.
    fn restyle_picked(&mut self, change: Restyle) {
        let Some((doc, page, id)) = self
            .tab()
            .and_then(|t| t.picked.map(|(page, id)| (t.info.id, page, id)))
        else {
            self.status("Select a comment first".into());
            return;
        };
        match self.engine.restyle(doc, page, id, change) {
            Ok(()) => self.edited(Some(page)),
            Err(e) => self.status(format!("Could not change the comment: {e}")),
        }
    }

    /// The colour the reader chose for new comments of `key`, if any.
    fn chosen_color(&self, key: Option<&str>) -> Option<[f32; 3]> {
        self.settings.comment_colors.get(key?).copied()
    }

    /// The picked comment, unless a comment tool is active: then the style bar is the tool's.
    fn styled_comment(&self) -> Option<&Annot> {
        if tool_key(self.tool).is_some() {
            return None;
        }
        let (_, a) = self.tab()?.comments.get(self.picked_index()?)?;
        (a.reply_to.is_none() && kind_key(a.kind).is_some()).then_some(a)
    }

    /// Shows the style bar for the active comment tool or the picked comment, else hides it.
    fn refresh_style_bar(&self) {
        let Some(w) = self.window() else { return };
        let swatch = |c: Option<[f32; 3]>| {
            c.and_then(|c| {
                SWATCHES
                    .iter()
                    .position(|(_, s)| s.iter().zip(c).all(|(a, b)| (a - b).abs() < 0.02))
            })
            .map_or(-1, |i| i as i32)
        };
        let (visible, color, fill, width, opacity) = if let Some(key) = tool_key(self.tool) {
            (true, swatch(self.chosen_color(Some(key))), -1, 0.0, -1)
        } else if let Some(a) = self.styled_comment() {
            let shape = matches!(a.kind, AnnotKind::Square | AnnotKind::Circle);
            (
                true,
                swatch(a.color),
                if shape {
                    i32::from(a.fill.is_some())
                } else {
                    -1
                },
                a.width.unwrap_or(0.0),
                (a.opacity * 100.0).round() as i32,
            )
        } else {
            (false, -1, -1, 0.0, -1)
        };
        w.set_style_visible(visible);
        w.set_style_color(color);
        w.set_style_fill(fill);
        w.set_style_width(width);
        w.set_style_opacity(opacity);
    }

    /// Swatch `index` was picked: the active comment tool draws with it from now on, or the
    /// picked comment takes it.
    pub fn style_color(&mut self, index: usize) {
        let Some(&(_, color)) = SWATCHES.get(index) else {
            return;
        };
        if let Some(key) = tool_key(self.tool) {
            self.settings.comment_colors.insert(key.to_owned(), color);
            self.save_settings();
            self.refresh_style_bar();
        } else {
            self.restyle_picked(Restyle::Color(color));
        }
    }

    /// Fills the picked rectangle or ellipse with its line colour, or clears its fill.
    pub fn style_fill(&mut self) {
        let Some(a) = self.styled_comment() else {
            return;
        };
        let fill = match a.fill {
            Some(_) => None,
            None => Some(a.color.unwrap_or([0.0; 3])),
        };
        self.restyle_picked(Restyle::Fill(fill));
    }

    pub fn style_width(&mut self, width: f32) {
        self.restyle_picked(Restyle::Width(width));
    }

    pub fn style_opacity(&mut self, percent: i32) {
        self.restyle_picked(Restyle::Opacity(percent as f32 / 100.0));
    }

    /// Deletes the picked comment. Returns false if none is picked.
    pub fn delete_picked(&mut self) -> bool {
        let Some(index) = self.picked_index() else {
            return false;
        };
        if let Some(tab) = self.tab_mut() {
            tab.picked = None;
        }
        self.comment_delete(index);
        true
    }

    pub fn comment_delete(&mut self, index: usize) {
        let Some((doc, page, id)) = self
            .tab()
            .and_then(|t| t.comments.get(index).map(|(p, a)| (t.info.id, *p, a.id)))
        else {
            return;
        };
        match self.engine.delete_annotation(doc, page, id) {
            Ok(()) => self.edited(Some(page)),
            Err(e) => self.status(format!("Could not delete the comment: {e}")),
        }
    }

    /// Highlights, underlines or strikes out the selected text.
    pub fn markup(&mut self, kind: AnnotKind) {
        let Some(sel) = self.tab().and_then(|t| t.selection) else {
            self.status("Select text first".into());
            return;
        };
        let Some(text) = self.page_text(sel.page) else {
            return;
        };
        let range = sel.range();
        if range.end > text.chars.len() {
            return;
        }
        let rects = text.line_rects(range);
        let Some(doc) = self.tab().map(|t| t.info.id) else {
            return;
        };
        let style = Style {
            color: self
                .chosen_color(kind_key(kind))
                .unwrap_or(markup_color(kind)),
            author: user_name(),
        };
        let new = NewAnnot::TextMarkup { kind, rects };
        match self.engine.add_annotation(doc, sel.page, new, style) {
            Ok(_) => {
                if let Some(tab) = self.tab_mut() {
                    tab.selection = None;
                }
                self.edited(Some(sel.page));
            }
            Err(e) => self.status(format!("Could not add the comment: {e}")),
        }
    }

    pub fn undo(&mut self, redo: bool) {
        let Some(tab) = self.tab() else { return };
        let (doc, step) = if redo {
            (tab.info.id, tab.edits.redo.clone())
        } else {
            (tab.info.id, tab.edits.undo.clone())
        };
        let Some(step) = step else {
            self.status(
                if redo {
                    "Nothing to redo"
                } else {
                    "Nothing to undo"
                }
                .into(),
            );
            return;
        };
        let result = if redo {
            self.engine.redo(doc)
        } else {
            self.engine.undo(doc)
        };
        match result {
            Ok(_) => {
                self.changed(None, true);
                let verb = if redo { "Redid" } else { "Undid" };
                self.status(format!("{verb}: {step}"));
            }
            Err(e) => self.status(format!("Could not undo: {e}")),
        }
    }

    /// Writes the active tab's edits into its file, appended after the original bytes.
    pub fn save(&mut self) {
        let Some(tab) = self.tab() else { return };
        if !tab.dirty {
            self.status("No changes to save".into());
            return;
        }
        let (info, path) = (tab.info, tab.path.clone());
        match self.engine.save(info.id, &path, true) {
            Ok(reopened) => {
                if reopened && let Some(index) = self.active {
                    // The file was rewritten whole and opened again; start the tab afresh.
                    self.adopt(index, info);
                }
                if let Some(tab) = self.tab_mut() {
                    tab.dirty = false;
                    tab.saved = file_stamp(&path);
                    tab.saved_position = tab.edits.position;
                }
                self.refresh_names();
                self.status(format!("Saved {}", file_name(&path)));
            }
            Err(e) => self.message("Could not save", format!("{}\n\n{e}", path.display())),
        }
    }

    /// Writes the active tab, edits included, to a new file and switches the tab to it.
    pub fn save_as(&mut self, target: PathBuf) {
        let Some(index) = self.active else { return };
        let (doc, old) = (self.tabs[index].info.id, self.tabs[index].path.clone());
        if same_path(&old, &target) {
            self.save();
            return;
        }
        if let Err(e) = self.engine.save(doc, &target, false) {
            self.message("Could not save", format!("{}\n\n{e}", target.display()));
            return;
        }
        self.tabs[index].path = target.clone();
        self.unwatch(&old);
        self.watch(&target);
        self.settings.add_recent(&target);
        self.reload(index);
        self.tabs[index].saved = file_stamp(&target);
        self.save_session();
        self.status(format!("Saved {}", file_name(&target)));
    }

    /// Whether the window may close now; if edits would be lost, asks first and says no.
    pub fn confirm_quit(&mut self) -> bool {
        let dirty = self.tabs.iter().filter(|t| t.dirty).count();
        if dirty == 0 {
            return true;
        }
        let text = if dirty == 1 {
            "One file has changes that are not saved.".to_owned()
        } else {
            format!("{dirty} files have changes that are not saved.")
        };
        self.push_dialog(Dialog {
            ask: Ask::Quit,
            kind: "confirm",
            title: "Quit without saving?".into(),
            text,
            ok: "Quit without saving",
            cancel: "Keep open",
        });
        false
    }

    pub fn close_active(&mut self) {
        if let Some(a) = self.active {
            self.close_tab(a);
        }
    }

    pub fn reload_active(&mut self) {
        let Some(tab) = self.tab() else { return };
        if tab.dirty {
            let (path, name) = (tab.path.clone(), tab.name());
            self.push_dialog(Dialog {
                ask: Ask::Reload(path),
                kind: "confirm",
                title: "Reload and lose your changes?".into(),
                text: format!("{name} has changes that are not saved."),
                ok: "Reload",
                cancel: "Keep editing",
            });
            return;
        }
        if let Some(a) = self.active {
            self.reload(a);
        }
    }

    pub fn show_properties(&mut self) {
        if self.has_document() {
            self.set_sidebar(true, Some(2));
        }
    }

    pub fn active_path(&self) -> Option<PathBuf> {
        self.tab().map(|t| t.path.clone())
    }

    pub fn active_doc(&self) -> Option<(DocId, Vec<(f32, f32)>)> {
        self.tab().map(|t| (t.info.id, t.sizes.clone()))
    }

    pub fn engine(&self) -> Arc<Engine> {
        Arc::clone(&self.engine)
    }
}

/// Page sheet snapped to physical pixels, so tiles line up with it exactly.
fn page_item(index: usize, f: Frame, dpr: f32) -> PageItem {
    let (x, y) = ((f.x * dpr).round(), (f.y * dpr).round());
    PageItem {
        index: index as i32,
        x: x / dpr,
        y: y / dpr,
        width: (f.width * dpr).ceil() / dpr,
        height: (f.height * dpr).ceil() / dpr,
    }
}

/// Tile edges are computed in physical pixels: a tile must cover exactly as many physical pixels
/// as it has, or the renderer resamples it and a seam shows in the middle.
fn tile_item(f: Frame, t: &TileImage, dpr: f32) -> TileItem {
    let (px, py) = ((f.x * dpr).round(), (f.y * dpr).round());
    let (dw, dh) = ((f.width * dpr).ceil(), (f.height * dpr).ceil());
    let x0 = px + (t.frac[0] * dw).round();
    let y0 = py + (t.frac[1] * dh).round();
    let x1 = px + (t.frac[2] * dw).round();
    let y1 = py + (t.frac[3] * dh).round();
    TileItem {
        x: x0 / dpr,
        y: y0 / dpr,
        width: (x1 - x0) / dpr,
        height: (y1 - y0) / dpr,
        image: t.image.clone(),
    }
}

/// Half the side of a resize handle, in document space.
const HANDLE: f32 = 4.0;
/// The smallest side a resize leaves, in points.
const MIN_SIDE: f32 = 6.0;

/// Text markup follows its text; the other kinds can move.
fn movable(kind: AnnotKind) -> bool {
    !matches!(
        kind,
        AnnotKind::Highlight | AnnotKind::Underline | AnnotKind::StrikeOut | AnnotKind::Squiggly
    )
}

/// Fields Tab stops at: those that take a value.
fn takes_input(f: &Field) -> bool {
    !f.read_only
        && matches!(
            f.kind,
            FieldKind::Text | FieldKind::Choice | FieldKind::Checkbox | FieldKind::Radio
        )
}

/// Notes and attached files keep their icon size.
fn resizable(kind: AnnotKind) -> bool {
    movable(kind) && !matches!(kind, AnnotKind::Note | AnnotKind::File)
}

/// A frame's corners: top left, then clockwise.
fn corners(f: Frame) -> [(f32, f32); 4] {
    let (x1, y1) = (f.x + f.width, f.bottom());
    [(f.x, f.y), (x1, f.y), (x1, y1), (f.x, y1)]
}

/// `rect` with the corner nearest `start` dragged to `to`; the opposite corner stays put.
fn resized(rect: Rect, start: (f32, f32), to: (f32, f32), keep_aspect: bool) -> Rect {
    let nearer = |v: f32, a: f32, b: f32| (v - a).abs() < (v - b).abs();
    let fx = if nearer(start.0, rect.x0, rect.x1) {
        rect.x1
    } else {
        rect.x0
    };
    let fy = if nearer(start.1, rect.y0, rect.y1) {
        rect.y1
    } else {
        rect.y0
    };
    let mut w = (to.0 - fx).abs().max(MIN_SIDE);
    let mut h = (to.1 - fy).abs().max(MIN_SIDE);
    if keep_aspect && rect.width() > 0.0 && rect.height() > 0.0 {
        let s = (w / rect.width()).max(h / rect.height());
        (w, h) = (rect.width() * s, rect.height() * s);
    }
    let (x0, x1) = if to.0 < fx {
        (fx - w, fx)
    } else {
        (fx, fx + w)
    };
    let (y0, y1) = if to.1 < fy {
        (fy - h, fy)
    } else {
        (fy, fy + h)
    };
    Rect { x0, y0, x1, y1 }
}

fn mark_item(f: Frame, kind: i32) -> MarkItem {
    MarkItem {
        x: f.x - 1.0,
        y: f.y - 1.0,
        width: f.width + 2.0,
        height: f.height + 2.0,
        kind,
    }
}

fn mode_index(mode: PageMode) -> i32 {
    match mode {
        PageMode::Single => 0,
        PageMode::Continuous => 1,
        PageMode::TwoUp => 2,
        PageMode::Book => 3,
    }
}

fn paper_color(mode: ReadingMode) -> slint::Color {
    let [r, g, b] = mode.paper();
    slint::Color::from_rgb_u8(r, g, b)
}

pub fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// Windows paths compare case-insensitively.
fn file_stamp(path: &Path) -> Option<(u64, SystemTime)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

/// What a left drag on a page does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Note,
    TextBox,
    Rect,
    Ellipse,
    Line,
    Ink,
    Sign,
    Stamp,
    Callout,
    Attach,
}

/// The signature pad's state that the window does not hold.
struct Pad {
    initials: bool,
    strokes: Vec<Vec<(f32, f32)>>,
    image: Option<PathBuf>,
}

struct Placing {
    mark: Mark,
    initials: bool,
    /// Width over height.
    aspect: f32,
}

/// Handwriting fonts offered for a typed signature, in order, when installed. The first four
/// ship with Windows; Lucida Handwriting comes with Office.
const SIGN_FONTS: [&str; 5] = [
    "Segoe Script",
    "Ink Free",
    "Segoe Print",
    "Lucida Handwriting",
    "Gabriola",
];
/// Pen width on the drawing pad, in pad pixels.
const PEN_WIDTH: f32 = 3.0;
/// Placed heights in points; the width follows from the mark's shape.
const SIGNATURE_HEIGHT: f32 = 36.0;
const INITIALS_HEIGHT: f32 = 28.0;
/// Placed marks are at most this many times as wide as they are tall.
const MAX_ASPECT: f32 = 7.0;

fn mark_name(initials: bool) -> &'static str {
    if initials { "initials" } else { "signature" }
}

/// How wide a new stamp is, in points.
const STAMP_WIDTH: f32 = 150.0;
/// A new callout's text box, in points.
const CALLOUT_SIZE: (f32, f32) = (150.0, 44.0);

/// Green for approval, red for warnings, blue for the rest, as Acrobat colours them.
fn stamp_color(name: &str) -> [f32; 3] {
    match name {
        "Approved" | "Final" | "Completed" => [0.13, 0.55, 0.13],
        "NotApproved" | "Confidential" | "TopSecret" | "Expired" | "NotForPublicRelease" => {
            [0.8, 0.1, 0.1]
        }
        _ => [0.1, 0.3, 0.7],
    }
}

/// Who new comments are by: the Windows user name.
fn user_name() -> String {
    std::env::var("USERNAME").unwrap_or_default()
}

/// A PDF date ("D:20261008165500+05'30'") as "2026-10-08 16:55".
fn pdf_date(s: &str) -> Option<String> {
    let digits: String = s
        .strip_prefix("D:")
        .unwrap_or(s)
        .chars()
        .take_while(char::is_ascii_digit)
        .take(12)
        .collect();
    if digits.len() < 8 {
        return None;
    }
    let mut out = format!("{}-{}-{}", &digits[..4], &digits[4..6], &digits[6..8]);
    if digits.len() == 12 {
        out += &format!(" {}:{}", &digits[8..10], &digits[10..12]);
    }
    Some(out)
}

fn kind_name(kind: AnnotKind) -> &'static str {
    match kind {
        AnnotKind::Highlight => "Highlight",
        AnnotKind::Underline => "Underline",
        AnnotKind::StrikeOut => "Strike-out",
        AnnotKind::Squiggly => "Squiggly",
        AnnotKind::Note => "Note",
        AnnotKind::FreeText => "Text box",
        AnnotKind::Ink => "Drawing",
        AnnotKind::Square => "Rectangle",
        AnnotKind::Circle => "Ellipse",
        AnnotKind::Line => "Line",
        AnnotKind::Stamp => "Stamp",
        AnnotKind::Callout => "Callout",
        AnnotKind::File => "Attachment",
        AnnotKind::Other => "Comment",
    }
}

/// The colours the style bar offers.
const SWATCHES: [(&str, [f32; 3]); 8] = [
    ("Yellow", [1.0, 0.85, 0.0]),
    ("Orange", [1.0, 0.55, 0.1]),
    ("Red", [0.85, 0.15, 0.15]),
    ("Pink", [0.95, 0.4, 0.7]),
    ("Purple", [0.55, 0.3, 0.85]),
    ("Blue", [0.1, 0.45, 0.9]),
    ("Green", [0.2, 0.7, 0.3]),
    ("Black", [0.0, 0.0, 0.0]),
];

/// The settings key of the colour a comment tool draws with.
fn tool_key(tool: Tool) -> Option<&'static str> {
    Some(match tool {
        Tool::Note => "note",
        Tool::TextBox => "text",
        Tool::Rect => "rect",
        Tool::Ellipse => "ellipse",
        Tool::Line => "line",
        Tool::Ink => "ink",
        Tool::Callout => "callout",
        _ => return None,
    })
}

/// The settings key of the colour for comments of `kind`; None for kinds the style bar
/// leaves alone (stamps, signatures and attached files keep their own look).
fn kind_key(kind: AnnotKind) -> Option<&'static str> {
    Some(match kind {
        AnnotKind::Note => "note",
        AnnotKind::FreeText => "text",
        AnnotKind::Square => "rect",
        AnnotKind::Circle => "ellipse",
        AnnotKind::Line => "line",
        AnnotKind::Ink => "ink",
        AnnotKind::Callout => "callout",
        AnnotKind::Highlight => "highlight",
        AnnotKind::Underline => "underline",
        AnnotKind::StrikeOut => "strikeout",
        AnnotKind::Squiggly => "squiggly",
        _ => return None,
    })
}

fn new_key(new: &NewAnnot) -> Option<&'static str> {
    match new {
        NewAnnot::Note { .. } => Some("note"),
        NewAnnot::FreeText { .. } => Some("text"),
        NewAnnot::Shape { kind, .. } | NewAnnot::TextMarkup { kind, .. } => kind_key(*kind),
        NewAnnot::Line { .. } => Some("line"),
        NewAnnot::Ink { .. } => Some("ink"),
        NewAnnot::Callout { .. } => Some("callout"),
        _ => None,
    }
}

fn is_fdf(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("fdf"))
}

fn markup_color(kind: AnnotKind) -> [f32; 3] {
    match kind {
        AnnotKind::Underline => [0.1, 0.45, 0.9],
        AnnotKind::StrikeOut => [0.85, 0.15, 0.15],
        _ => [1.0, 0.85, 0.0],
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.as_os_str().to_string_lossy().to_lowercase() == b.as_os_str().to_string_lossy().to_lowercase()
}

fn format_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.2} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{b} bytes"),
    }
}

/// Opens a web or mail link with the user's default handler.
pub fn shell_open(uri: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (verb, file) = (wide("open"), wide(uri));
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}
