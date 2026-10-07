//! Commands by id, shared by the toolbar, menus, palette and keyboard, and the key map that
//! includes the optional Vim layer.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use mp_engine::AnnotKind;
use slint::platform::Key;

use crate::layout::{PageMode, Zoom};
use crate::recolor::ReadingMode;
use crate::viewer::{self, ACTUAL, App, LINE};

/// Runs a command. Commands that show a system dialog run outside the app borrow.
pub fn run(id: &str) {
    match id {
        "open" => open_dialog(),
        "save-as" => save_as_dialog(),
        "print" => crate::print::start(),
        "register-pdf" => {
            let message = match crate::assoc::register() {
                Ok(()) => {
                    viewer::shell_open(crate::assoc::DEFAULT_APPS_URI);
                    "micropdf is now a PDF app. Pick it in Default apps to open PDFs with it."
                        .to_owned()
                }
                Err(e) => format!("Could not register micropdf: {e}"),
            };
            viewer::with(|app| app.status(message));
        }
        "unregister-pdf" => {
            let message = match crate::assoc::unregister() {
                Ok(()) => "micropdf is no longer listed as a PDF app.".to_owned(),
                Err(e) => format!("Could not remove the registration: {e}"),
            };
            viewer::with(|app| app.status(message));
        }
        _ => {
            let follow_up = viewer::with(|app| command(app, id)).flatten();
            if let Some(next) = follow_up {
                run(next);
            }
        }
    }
}

/// Runs `id` on the app. Returns a command that must run outside the borrow, if any.
fn command(app: &mut App, id: &str) -> Option<&'static str> {
    if let Some(percent) = id.strip_prefix("zoom-").and_then(|z| z.parse::<f32>().ok()) {
        app.set_zoom(Zoom::Scale(percent / 100.0 * ACTUAL), None);
        return None;
    }
    match id {
        "close-tab" => app.close_active(),
        "reopen-tab" => app.reopen_closed(),
        "next-tab" => app.next_tab(true),
        "prev-tab" => app.next_tab(false),
        "find" => app.open_find(),
        "find-close" => app.close_find(),
        "goto" => app.focus_page_field(),
        "first-page" => app.first_page(),
        "last-page" => app.last_page(),
        "next-page" => app.next_page(),
        "prev-page" => app.prev_page(),
        "back" => app.back(),
        "forward" => app.forward(),
        "zoom-in" => app.zoom_step(1, None),
        "zoom-out" => app.zoom_step(-1, None),
        "fit-width" => app.set_zoom(Zoom::FitWidth, None),
        "fit-page" => app.set_zoom(Zoom::FitPage, None),
        "rotate-cw" => app.rotate(90),
        "rotate-ccw" => app.rotate(-90),
        "mode-single" => app.set_mode(PageMode::Single),
        "mode-continuous" => app.set_mode(PageMode::Continuous),
        "mode-two-up" => app.set_mode(PageMode::TwoUp),
        "mode-book" => app.set_mode(PageMode::Book),
        "read-normal" => app.set_reading_mode(ReadingMode::Normal),
        "read-dark" => app.set_reading_mode(ReadingMode::Dark),
        "read-sepia" => app.set_reading_mode(ReadingMode::Sepia),
        "read-invert" => app.set_reading_mode(ReadingMode::Invert),
        "toggle-theme" => app.toggle_theme(),
        "toggle-vim" => app.toggle_vim(),
        "sidebar" => app.toggle_sidebar(),
        "show-thumbs" => app.set_sidebar(true, Some(0)),
        "show-outline" => app.set_sidebar(true, Some(1)),
        "properties" => app.show_properties(),
        "copy" => app.copy(),
        "undo" => app.undo(false),
        "redo" => app.undo(true),
        "save" => app.save(),
        "highlight" => app.markup(AnnotKind::Highlight),
        "underline" => app.markup(AnnotKind::Underline),
        "strikeout" => app.markup(AnnotKind::StrikeOut),
        "select-all" => app.select_all(),
        "fullscreen" => app.toggle_fullscreen(),
        "present" => {
            if app.has_document() || app.presenting() {
                app.toggle_present();
            }
        }
        "reload" => app.reload_active(),
        "palette" => app.open_palette(),
        "palette-close" => app.close_palette(),
        "open" => return Some("open"),
        "print" => return Some("print"),
        "save-as" => return Some("save-as"),
        _ => {}
    }
    None
}

pub fn palette_accept(index: usize) {
    if let Some(id) = viewer::with(|app| app.palette_accept(index)).flatten() {
        run(id);
    }
}

static OPEN_DIALOG: AtomicBool = AtomicBool::new(false);

/// Shows the system file picker on its own thread, so the window keeps painting.
fn open_dialog() {
    if OPEN_DIALOG.swap(true, Ordering::SeqCst) {
        return;
    }
    let start = viewer::with(|app| app.active_path())
        .flatten()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    std::thread::spawn(move || {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Open")
            .add_filter("PDF documents", &["pdf"])
            .add_filter("All files", &["*"]);
        if let Some(dir) = start {
            dialog = dialog.set_directory(dir);
        }
        let files = dialog.pick_files();
        OPEN_DIALOG.store(false, Ordering::SeqCst);
        if let Some(files) = files {
            let _ = slint::invoke_from_event_loop(move || {
                viewer::with(|app| app.open_paths(files));
            });
        }
    });
}

static SAVE_DIALOG: AtomicBool = AtomicBool::new(false);

fn save_as_dialog() {
    let Some(path) = viewer::with(|app| app.active_path()).flatten() else {
        return;
    };
    if SAVE_DIALOG.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Save as")
            .add_filter("PDF documents", &["pdf"])
            .set_file_name(viewer::file_name(&path));
        if let Some(dir) = path.parent() {
            dialog = dialog.set_directory(dir);
        }
        let target = dialog.save_file();
        SAVE_DIALOG.store(false, Ordering::SeqCst);
        if let Some(target) = target {
            let _ = slint::invoke_from_event_loop(move || {
                viewer::with(|app| app.save_as(target));
            });
        }
    });
}

enum KeyResult {
    Ignored,
    Handled,
    Run(&'static str),
}

/// Handles a key the focused control did not take. Returns whether it was used.
pub fn key_input(text: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
    match viewer::with(|app| map_key(app, text, ctrl, shift, alt)) {
        Some(KeyResult::Handled) => true,
        Some(KeyResult::Run(id)) => {
            run(id);
            true
        }
        _ => false,
    }
}

fn map_key(app: &mut App, text: &str, ctrl: bool, shift: bool, alt: bool) -> KeyResult {
    use KeyResult::*;
    let Some(c) = text.chars().next() else {
        return Ignored;
    };
    let is = |key: Key| c == char::from(key);

    if app.dialog_visible() {
        if is(Key::Escape) {
            app.dialog_cancel();
        } else if is(Key::Return) {
            app.dialog_enter();
        }
        return Handled;
    }
    if app.palette_visible() {
        return Ignored;
    }

    if ctrl {
        let lower = text.to_lowercase();
        let id = match lower.as_str() {
            "o" => "open",
            "w" | "\u{4}" => "close-tab",
            "t" if shift => "reopen-tab",
            "f" => "find",
            "g" => "goto",
            "k" => "palette",
            "p" if shift => "palette",
            "p" => "print",
            "c" => "copy",
            "z" if shift => "redo",
            "z" => "undo",
            "y" => "redo",
            "s" if shift => "save-as",
            "s" => "save",
            "a" => "select-all",
            "d" => "properties",
            "l" => "present",
            "r" => "reload",
            "=" => "zoom-in",
            "+" if shift => "rotate-cw",
            "+" => "zoom-in",
            "_" => "rotate-ccw",
            "-" => "zoom-out",
            "0" => "fit-page",
            "1" => "zoom-100",
            "2" => "fit-width",
            "\t" if shift => "prev-tab",
            "\t" => "next-tab",
            _ if is(Key::Backtab) => "prev-tab",
            _ if is(Key::PageDown) => "next-tab",
            _ if is(Key::PageUp) => "prev-tab",
            _ if is(Key::Home) => "first-page",
            _ if is(Key::End) => "last-page",
            _ => return Ignored,
        };
        return Run(id);
    }
    if alt {
        return if is(Key::LeftArrow) {
            app.back();
            Handled
        } else if is(Key::RightArrow) {
            app.forward();
            Handled
        } else {
            Ignored
        };
    }

    if is(Key::Escape) {
        app.vim_count.clear();
        app.vim_pending = None;
        if app.presenting() {
            app.toggle_present();
        } else if !app.clear_selection() {
            app.close_find();
        }
        return Handled;
    }
    if is(Key::F3) {
        app.find_step(!shift);
        return Handled;
    }
    if is(Key::F4) {
        return Run("sidebar");
    }
    if is(Key::F5) {
        return Run("reload");
    }
    if is(Key::F11) {
        return Run("fullscreen");
    }
    if !app.has_document() {
        return Ignored;
    }

    if app.presenting() {
        if is(Key::RightArrow)
            || is(Key::DownArrow)
            || is(Key::PageDown)
            || is(Key::Return)
            || (c == ' ' && !shift)
        {
            app.next_page();
            return Handled;
        }
        if is(Key::LeftArrow) || is(Key::UpArrow) || is(Key::PageUp) || c == ' ' {
            app.prev_page();
            return Handled;
        }
    }

    if is(Key::DownArrow) {
        app.scroll_by(0.0, LINE);
    } else if is(Key::UpArrow) {
        app.scroll_by(0.0, -LINE);
    } else if is(Key::PageDown) || (c == ' ' && !shift) {
        app.page_scroll(true);
    } else if is(Key::PageUp) || c == ' ' {
        app.page_scroll(false);
    } else if is(Key::Home) {
        app.first_page();
    } else if is(Key::End) {
        app.last_page();
    } else if is(Key::LeftArrow) {
        app.horizontal(false);
    } else if is(Key::RightArrow) {
        app.horizontal(true);
    } else if app.vim() && text.chars().count() == 1 {
        return vim(app, c);
    } else {
        return Ignored;
    }
    Handled
}

fn vim(app: &mut App, c: char) -> KeyResult {
    use KeyResult::*;
    if c.is_ascii_digit() && (c != '0' || !app.vim_count.is_empty()) {
        if app.vim_count.len() < 6 {
            app.vim_count.push(c);
        }
        return Handled;
    }
    let count: Option<usize> = app.vim_count.parse().ok();
    app.vim_count.clear();
    let n = count.unwrap_or(1).max(1);
    if let Some(prefix) = app.vim_pending.take() {
        match (prefix, c) {
            ('g', 'g') => app.go_to(count.map_or(0, |n| n.saturating_sub(1)), None, true),
            ('g', 't') => app.next_tab(true),
            ('g', 'T') => app.next_tab(false),
            _ => {}
        }
        return Handled;
    }
    let (_, vh) = app.view_size();
    match c {
        'j' => app.scroll_by(0.0, LINE * n as f32),
        'k' => app.scroll_by(0.0, -LINE * n as f32),
        'h' => app.scroll_by(-LINE * n as f32, 0.0),
        'l' => app.scroll_by(LINE * n as f32, 0.0),
        'd' => app.scroll_by(0.0, vh / 2.0 * n as f32),
        'u' => app.scroll_by(0.0, -vh / 2.0 * n as f32),
        'J' => (0..n).for_each(|_| app.next_page()),
        'K' => (0..n).for_each(|_| app.prev_page()),
        'H' => app.back(),
        'L' => app.forward(),
        'G' => match count {
            Some(page) => app.go_to(page.saturating_sub(1), None, true),
            None => app.last_page(),
        },
        'g' => app.vim_pending = Some('g'),
        'n' => app.find_step(true),
        'N' => app.find_step(false),
        '/' => return Run("find"),
        ':' => return Run("palette"),
        'o' => return Run("open"),
        '+' | '=' => app.zoom_step(1, None),
        '-' => app.zoom_step(-1, None),
        's' => app.set_zoom(Zoom::FitWidth, None),
        'a' => app.set_zoom(Zoom::FitPage, None),
        'r' => app.rotate(90),
        'R' => app.rotate(-90),
        'y' => app.copy(),
        _ => return Ignored,
    }
    Handled
}
