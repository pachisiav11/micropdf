//! The library: folders of PDFs the reader adds, read again on a thread as files come, change
//! and go, and searched in full text from the palette (Ctrl+Shift+F). A hit opens its file at
//! the page, with the words found marked.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mp_engine::{Hit, Library, library_dir};
use slint::{Timer, TimerMode};

use crate::tools::{Done, Form, choice};
use crate::viewer::{self, App, file_name};

#[derive(Default)]
pub struct Shelf {
    pub(crate) library: Library,
    /// The palette searches the library rather than listing commands.
    pub(crate) searching: bool,
    watcher: Option<notify::RecommendedWatcher>,
    /// Waits for a burst of file changes to end before reading the folders again.
    settle: Timer,
}

static UPDATING: AtomicBool = AtomicBool::new(false);
static AGAIN: AtomicBool = AtomicBool::new(false);

/// Opens the library kept on disk, watches its folders, and catches up with what changed in
/// them while the app was closed.
pub fn start(app: &mut App) {
    app.shelf.library = Library::open(&library_dir());
    shown(app);
    watch(app);
    if !app.settings.library.is_empty() {
        changed(app);
    }
}

/// Library command `id`; false if it is not one.
pub fn command(app: &mut App, id: &str) -> bool {
    match id {
        "library-search" => {
            if app.settings.library.is_empty() {
                app.status("Add a folder to the library first".into());
            } else {
                app.shelf.searching = true;
                app.open_palette();
            }
        }
        "library-add" => add_folder(),
        "library-remove" => {
            let folders: Vec<String> = app
                .settings
                .library
                .iter()
                .map(|f| f.display().to_string())
                .collect();
            if folders.is_empty() {
                app.status("The library has no folders".into());
                return true;
            }
            let names: Vec<&str> = folders.iter().map(String::as_str).collect();
            app.show_form(
                Form::LibraryRemove,
                "Remove a folder from the library",
                "Its files stay where they are.",
                "Remove",
                vec![choice("Folder", &names, 0)],
            );
        }
        "library-update" => update(app),
        _ => return false,
    }
    true
}

fn add_folder() {
    std::thread::spawn(|| {
        let Some(folder) = rfd::FileDialog::new()
            .set_title("Add a folder to the library")
            .pick_folder()
        else {
            return;
        };
        let _ = slint::invoke_from_event_loop(move || {
            viewer::with(|app| {
                if !app.settings.library.contains(&folder) {
                    app.settings.library.push(folder);
                    app.save_settings();
                    shown(app);
                    watch(app);
                }
                update(app);
            });
        });
    });
}

pub(crate) fn remove_folder(app: &mut App, index: usize) -> Done {
    if index < app.settings.library.len() {
        app.settings.library.remove(index);
        app.save_settings();
        shown(app);
        app.shelf.watcher = None;
        watch(app);
        update(app);
    }
    Ok(())
}

/// Lets the start page offer to search the library once it has folders.
fn shown(app: &App) {
    if let Some(w) = app.window() {
        w.set_has_library(!app.settings.library.is_empty());
    }
}

/// Watches the library's folders, and their subfolders, for changes.
fn watch(app: &mut App) {
    use notify::Watcher;
    if app.shelf.watcher.is_none() {
        app.shelf.watcher = notify::recommended_watcher(|event: notify::Result<notify::Event>| {
            if event.is_ok_and(|e| !matches!(e.kind, notify::EventKind::Access(_))) {
                let _ = slint::invoke_from_event_loop(|| {
                    viewer::with(changed);
                });
            }
        })
        .ok();
    }
    if let Some(w) = app.shelf.watcher.as_mut() {
        for folder in &app.settings.library {
            let _ = w.watch(folder, notify::RecursiveMode::Recursive);
        }
    }
}

/// Something changed in the folders: reads them again once it has been quiet a moment.
fn changed(app: &mut App) {
    app.shelf
        .settle
        .start(TimerMode::SingleShot, Duration::from_secs(3), || {
            viewer::with(update);
        });
}

/// Brings the library in line with its folders on a thread.
fn update(app: &mut App) {
    if UPDATING.swap(true, Ordering::SeqCst) {
        AGAIN.store(true, Ordering::SeqCst);
        return;
    }
    let folders: Vec<PathBuf> = app.settings.library.clone();
    std::thread::spawn(move || {
        let mut library = Library::open(&library_dir());
        let result = library.update(&folders, |done, total| {
            if done % 20 == 0 && done < total {
                let text = format!("Reading the library: {done} of {total} files");
                let _ = slint::invoke_from_event_loop(move || {
                    viewer::with(|app| app.status(text));
                });
            }
        });
        UPDATING.store(false, Ordering::SeqCst);
        let _ = slint::invoke_from_event_loop(move || {
            viewer::with(|app| {
                app.shelf.library = library;
                match result {
                    Ok(0) => {}
                    Ok(_) => {
                        let n = app.shelf.library.len();
                        let s = if n == 1 { "" } else { "s" };
                        app.status(format!("The library has {n} PDF{s}, ready to search"));
                    }
                    Err(e) => app.message("Could not read the library", e.to_string()),
                }
                if AGAIN.swap(false, Ordering::SeqCst) {
                    update(app);
                }
            });
        });
    });
}

/// The pages of the library that match `query`, for the palette.
pub fn search(app: &App, query: &str) -> Vec<Hit> {
    if query.chars().count() < 2 {
        return Vec::new();
    }
    app.shelf.library.search(query, 60).unwrap_or_default()
}

/// The palette's line for `hit`.
pub fn title(hit: &Hit) -> String {
    format!("{} \u{b7} page {}", file_name(&hit.path), hit.page + 1)
}
