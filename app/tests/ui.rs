//! Golden-path UI test: the real window, controller, engine and render pool on Slint's headless
//! testing backend. The backend's event loop can start only once per process, so this file holds
//! a single test that runs its steps in order from a timer.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use i_slint_backend_testing::{AccessibleRole, ElementQuery};
use micropdf::settings::Settings;
use micropdf::{MainWindow, viewer, wire};
use slint::platform::Key;
use slint::{ComponentHandle, Model};

const STEP_TIMEOUT: Duration = Duration::from_secs(15);

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures")
        .join(name)
}

struct Step {
    name: &'static str,
    act: Box<dyn Fn(&MainWindow)>,
    done: Box<dyn Fn(&MainWindow) -> bool>,
}

fn step(
    name: &'static str,
    act: impl Fn(&MainWindow) + 'static,
    done: impl Fn(&MainWindow) -> bool + 'static,
) -> Step {
    Step {
        name,
        act: Box::new(act),
        done: Box::new(done),
    }
}

fn command(id: &'static str) -> impl Fn(&MainWindow) {
    move |w: &MainWindow| w.invoke_command(id.into())
}

fn key(text: impl Into<String>) -> impl Fn(&MainWindow) {
    let text: String = text.into();
    move |w: &MainWindow| {
        w.invoke_key_input(text.as_str().into(), false, false, false);
    }
}

fn open(name: &'static str) -> impl Fn(&MainWindow) {
    move |_: &MainWindow| {
        viewer::with(|app| app.open(fixture(name)));
    }
}

/// A scratch copy of hello.pdf to edit, and the target of Save As.
fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("micropdf-ui-{}-{name}", std::process::id()))
}

fn active_title(w: &MainWindow) -> String {
    w.get_tabs()
        .row_data(w.get_active_tab() as usize)
        .map(|t| t.title.to_string())
        .unwrap_or_default()
}

/// Comments on page 1 of the active tab, as the engine has them now.
fn comments() -> usize {
    viewer::with(|app| {
        let (doc, _) = app.active_doc()?;
        app.engine().annotations(doc, 0).ok().map(|a| a.len())
    })
    .flatten()
    .unwrap_or(usize::MAX)
}

/// Comments on page 1 of the file at `path`, read from disk.
fn comments_in(path: &std::path::Path) -> usize {
    let engine = viewer::with(|app| app.engine()).unwrap();
    let Ok(info) = engine.open(path) else {
        return usize::MAX;
    };
    let n = engine
        .annotations(info.id, 0)
        .map_or(usize::MAX, |a| a.len());
    engine.close(info.id);
    n
}

/// The value of form field `index` on page 1 of the active tab.
fn field_value(index: usize) -> String {
    viewer::with(|app| {
        let (doc, _) = app.active_doc()?;
        let fields = app.engine().fields(doc, 0).ok()?;
        fields.get(index).map(|f| f.value.clone())
    })
    .flatten()
    .unwrap_or_else(|| "?".into())
}

fn tab_count(w: &MainWindow) -> usize {
    w.get_tabs().row_count()
}

fn page(w: &MainWindow) -> String {
    w.get_page_text().to_string()
}

fn marks(w: &MainWindow, kind: i32) -> usize {
    w.get_marks().iter().filter(|m| m.kind == kind).count()
}

/// Clicks at (x, y) points from the top-left of page `index`, unrotated, in document space.
fn click_page(w: &MainWindow, index: i32, x: f32, y: f32) {
    let p = w
        .get_pages()
        .iter()
        .find(|p| p.index == index)
        .expect("page is laid out");
    let (dx, dy) = (p.x + x / 612.0 * p.width, p.y + y / 792.0 * p.height);
    w.invoke_pointer_down(dx, dy, 0, false);
    w.invoke_pointer_up(dx, dy);
}

/// Drags through `points` (points from the top-left of hello.pdf's 300x200 page 1).
fn drag_hello(w: &MainWindow, points: &[(f32, f32)]) {
    let p = w
        .get_pages()
        .iter()
        .find(|p| p.index == 0)
        .expect("page is laid out");
    let at = |(x, y): (f32, f32)| (p.x + x / 300.0 * p.width, p.y + y / 200.0 * p.height);
    let (x, y) = at(points[0]);
    w.invoke_pointer_down(x, y, 0, false);
    for &point in &points[1..] {
        let (x, y) = at(point);
        w.invoke_pointer_move(x, y);
    }
    let (x, y) = at(*points.last().unwrap());
    w.invoke_pointer_up(x, y);
}

/// The comment list's last row, as "kind: text".
fn last_comment(w: &MainWindow) -> String {
    let rows = w.get_comments();
    rows.row_data(rows.row_count().wrapping_sub(1))
        .map(|r| format!("{}: {}", r.kind, r.text))
        .unwrap_or_default()
}

fn steps() -> Vec<Step> {
    let zoom_before = Rc::new(RefCell::new(String::new()));
    let zoom_after = Rc::clone(&zoom_before);
    let scroll_before = Rc::new(RefCell::new(0.0f32));
    let scroll_after = Rc::clone(&scroll_before);
    let palette_zoom = Rc::new(RefCell::new(String::new()));
    let palette_zoom_after = Rc::clone(&palette_zoom);
    let tabs_before = Rc::new(Cell::new(0usize));
    let tabs_after = Rc::clone(&tabs_before);
    let tabs_reopened = Rc::clone(&tabs_before);
    let tabs_edited = Rc::new(Cell::new(0usize));
    let tabs_edited_after = Rc::clone(&tabs_edited);
    vec![
        step("empty window", |_| {}, |w| !w.get_has_document()),
        step("open hello.pdf", open("hello.pdf"), |w| {
            w.get_has_document() && w.get_page_count() == 1 && page(w) == "1"
        }),
        step("tiles render", |_| {}, |w| w.get_tiles().row_count() > 0),
        step("find opens", command("find"), |w| w.get_find_visible()),
        step(
            "find highlights the match",
            |w| w.invoke_find_edited("micropdf".into()),
            |w| w.get_find_status() == "1 of 1" && marks(w, 1) == 1,
        ),
        step("find closes and clears", command("find-close"), |w| {
            !w.get_find_visible() && w.get_marks().row_count() == 0
        }),
        step("select all marks the text", command("select-all"), |w| {
            marks(w, 2) > 0
        }),
        step(
            "selection holds the page text",
            |_| {},
            |_| {
                viewer::with(|app| app.selected_text()).flatten().as_deref()
                    == Some("Hello micropdf")
            },
        ),
        step(
            "zoom in changes the zoom",
            move |w| {
                *zoom_before.borrow_mut() = w.get_zoom_text().to_string();
                w.invoke_command("zoom-in".into());
            },
            move |w| w.get_zoom_text() != *zoom_after.borrow(),
        ),
        step("actual size is 100%", command("zoom-100"), |w| {
            w.get_zoom_text() == "100%"
        }),
        step(
            "open outline-links.pdf in a second tab",
            open("outline-links.pdf"),
            |w| tab_count(w) == 2 && w.get_active_tab() == 1 && w.get_page_count() == 3,
        ),
        step(
            "outline lists three chapters",
            command("show-outline"),
            |w| w.get_outline().row_count() == 3 && w.get_sidebar_tab() == 1,
        ),
        step(
            "outline entry jumps to its page",
            |w| w.invoke_outline_clicked(2),
            |w| page(w) == "3" && w.get_can_back(),
        ),
        step("back returns", command("back"), |w| page(w) == "1"),
        step("next page", command("next-page"), |w| page(w) == "2"),
        step("last page", command("last-page"), |w| page(w) == "3"),
        step("first page", command("first-page"), |w| page(w) == "1"),
        step(
            "page field jumps",
            |w| w.invoke_page_entered("2".into()),
            |w| page(w) == "2",
        ),
        step(
            "vim keys scroll",
            move |w| {
                w.invoke_command("toggle-vim".into());
                *scroll_before.borrow_mut() = w.get_viewport_y();
                w.invoke_key_input("j".into(), false, false, false);
            },
            move |w| w.get_vim_enabled() && w.get_viewport_y() < *scroll_after.borrow(),
        ),
        step("vim off", command("toggle-vim"), |w| !w.get_vim_enabled()),
        step("dark pages", command("read-dark"), |w| {
            w.get_reading_mode() != 0
        }),
        step("normal pages", command("read-normal"), |w| {
            w.get_reading_mode() == 0
        }),
        step("two-up layout", command("mode-two-up"), |w| {
            w.get_pages().row_count() >= 2
        }),
        step("continuous layout", command("mode-continuous"), |w| {
            w.get_page_mode() == 1
        }),
        step(
            "palette filters commands",
            move |w| {
                *palette_zoom.borrow_mut() = w.get_zoom_text().to_string();
                w.invoke_command("palette".into());
                w.invoke_palette_edited("zoom in".into());
            },
            |w| {
                w.get_palette_visible()
                    && w.get_palette_items()
                        .row_data(0)
                        .is_some_and(|i| i.title == "Zoom in")
            },
        ),
        step(
            "palette runs a command",
            |w| w.invoke_palette_accept(0),
            move |w| !w.get_palette_visible() && w.get_zoom_text() != *palette_zoom_after.borrow(),
        ),
        step("presentation starts", command("present"), |w| {
            w.get_presenting() && !w.get_chrome_visible()
        }),
        step(
            "Escape ends presentation",
            key(char::from(Key::Escape)),
            |w| !w.get_presenting() && w.get_chrome_visible(),
        ),
        step(
            "encrypted file asks for a password",
            open("encrypted.pdf"),
            |w| w.get_dialog_kind() == "password",
        ),
        step(
            "wrong password asks again",
            |w| w.invoke_dialog_accept("nope".into()),
            |w| w.get_dialog_kind() == "password" && w.get_dialog_text().contains("did not unlock"),
        ),
        step(
            "right password opens it",
            |w| w.invoke_dialog_accept("user".into()),
            |w| w.get_dialog_kind().is_empty() && tab_count(w) == 3 && w.get_page_count() == 1,
        ),
        step("attachments panel", open("attachment.pdf"), |w| {
            tab_count(w) == 4
                && w.get_attachments()
                    .row_data(0)
                    .is_some_and(|a| a.name == "notes.txt")
        }),
        step("layers panel", open("layers.pdf"), |w| {
            tab_count(w) == 5 && w.get_layers().row_count() == 2
        }),
        step(
            "a hidden layer can be shown",
            |w| {
                let notes = w
                    .get_layers()
                    .iter()
                    .position(|l| l.name == "Notes")
                    .unwrap();
                w.invoke_layer_toggle(notes as i32);
            },
            |w| w.get_layers().iter().all(|l| l.visible),
        ),
        step(
            "switching tabs restores that tab's sidebar data",
            |w| w.invoke_select_tab(3),
            |w| w.get_active_tab() == 3 && w.get_layers().row_count() == 0,
        ),
        step("broken file shows a message", open("truncated.pdf"), |w| {
            // MuPDF repairs some truncated files; either outcome is fine, a crash is not.
            w.get_dialog_kind() == "message" || tab_count(w) == 6
        }),
        step(
            "message closes",
            |w| {
                if w.get_dialog_kind() == "message" {
                    w.invoke_dialog_accept("".into());
                }
            },
            |w| w.get_dialog_kind().is_empty(),
        ),
        step("not a PDF shows a message", open("not-a-pdf.pdf"), |w| {
            w.get_dialog_kind() == "message"
        }),
        step(
            "message closes again",
            |w| w.invoke_dialog_accept("".into()),
            |w| w.get_dialog_kind().is_empty(),
        ),
        step(
            "close tab",
            move |w| {
                tabs_before.set(tab_count(w));
                w.invoke_command("close-tab".into());
            },
            move |w| tab_count(w) + 1 == tabs_after.get(),
        ),
        step("reopen closed tab", command("reopen-tab"), move |w| {
            tab_count(w) == tabs_reopened.get()
        }),
        step(
            "restore offers the last session",
            |_| {
                viewer::with(|app| app.offer_restore(vec![fixture("hello.pdf")]));
            },
            |w| w.get_dialog_kind() == "confirm" && w.get_dialog_title() == "Reopen your files?",
        ),
        step(
            "restore reopens; an open file is selected, not opened twice",
            |w| w.invoke_dialog_accept("".into()),
            |w| w.get_dialog_kind().is_empty() && w.get_active_tab() == 0,
        ),
        step(
            "outline-links.pdf, page 1",
            |w| {
                w.invoke_select_tab(1);
                w.invoke_command("first-page".into());
            },
            |w| w.get_active_tab() == 1 && page(w) == "1",
        ),
        step(
            "an internal link jumps",
            |w| click_page(w, 0, 145.0, 792.0 - 645.0),
            |w| page(w) == "3",
        ),
        step(
            "a web link asks first",
            |w| {
                w.invoke_command("back".into());
                click_page(w, 0, 145.0, 792.0 - 615.0);
            },
            |w| {
                w.get_dialog_kind() == "confirm"
                    && w.get_dialog_text().contains("https://example.com/")
            },
        ),
        step(
            "declining keeps the page",
            |w| w.invoke_dialog_cancel(),
            |w| w.get_dialog_kind().is_empty() && page(w) == "1",
        ),
        step(
            "open an editable copy",
            |_| {
                std::fs::copy(fixture("hello.pdf"), scratch("edit.pdf")).unwrap();
                viewer::with(|app| app.open(scratch("edit.pdf")));
            },
            |w| active_title(w) == viewer::file_name(&scratch("edit.pdf")) && comments() == 0,
        ),
        step(
            "highlight the selection",
            |w| {
                w.invoke_command("select-all".into());
                w.invoke_command("highlight".into());
            },
            |w| comments() == 1 && active_title(w).starts_with('\u{2022}') && marks(w, 2) == 0,
        ),
        step(
            "the toolbar offers to undo it",
            |_| {},
            |w| w.get_dirty() && w.get_undo_name() == "Highlight" && w.get_redo_name().is_empty(),
        ),
        step("undo takes it back", command("undo"), |w| {
            comments() == 0
                && w.get_status_left() == "Undid: Highlight"
                && w.get_redo_name() == "Highlight"
        }),
        step("redo puts it back", command("redo"), |_| comments() == 1),
        step("the comment list shows it", command("show-comments"), |w| {
            w.get_sidebar_tab() == 5
                && w.get_comments().row_count() == 1
                && w.get_comments().row_data(0).unwrap().kind == "Highlight"
        }),
        step("save writes it into the file", command("save"), |w| {
            !active_title(w).starts_with('\u{2022}')
                && !w.get_dirty()
                && comments_in(&scratch("edit.pdf")) == 1
        }),
        step(
            "save as writes a new file and switches to it",
            |w| {
                w.invoke_command("select-all".into());
                w.invoke_command("underline".into());
                viewer::with(|app| app.save_as(scratch("copy.pdf")));
            },
            |w| {
                active_title(w) == viewer::file_name(&scratch("copy.pdf"))
                    && comments() == 2
                    && comments_in(&scratch("edit.pdf")) == 1
            },
        ),
        step(
            "closing an edited tab asks first",
            |w| {
                w.invoke_command("select-all".into());
                w.invoke_command("strikeout".into());
                w.invoke_command("close-tab".into());
            },
            |w| w.get_dialog_kind() == "confirm" && w.get_dialog_title() == "Close without saving?",
        ),
        step(
            "keep open keeps the tab",
            |w| w.invoke_dialog_cancel(),
            |w| {
                w.get_dialog_kind().is_empty()
                    && active_title(w).starts_with('\u{2022}')
                    && w.get_comments().row_count() == 3
            },
        ),
        step(
            "the list deletes a comment",
            |w| w.invoke_comment_delete(2),
            |w| comments() == 2 && w.get_comments().row_count() == 2,
        ),
        step(
            "the rectangle tool draws a rectangle",
            |w| {
                w.invoke_command("tool-rect".into());
                drag_hello(w, &[(20.0, 20.0), (80.0, 50.0), (120.0, 70.0)]);
            },
            |w| w.get_tool() == 3 && comments() == 3 && last_comment(w) == "Rectangle: ",
        ),
        step(
            "the drawing tool draws a stroke",
            |w| {
                w.invoke_command("tool-ink".into());
                drag_hello(w, &[(20.0, 150.0), (60.0, 170.0), (100.0, 150.0)]);
            },
            |w| comments() == 4 && last_comment(w) == "Drawing: " && w.get_draft_path().is_empty(),
        ),
        step(
            "the note tool asks for the text",
            |w| {
                w.invoke_command("tool-note".into());
                drag_hello(w, &[(250.0, 20.0)]);
            },
            |w| w.get_dialog_kind() == "input" && w.get_dialog_title() == "Add a note",
        ),
        step(
            "the note is added",
            |w| w.invoke_dialog_accept("Check this".into()),
            |w| comments() == 5 && last_comment(w) == "Note: Check this",
        ),
        step(
            "Escape returns to selecting text",
            key(char::from(Key::Escape)),
            |w| w.get_tool() == 0,
        ),
        step(
            "close without saving drops the edits",
            move |w| {
                tabs_edited.set(tab_count(w));
                w.invoke_command("close-tab".into());
                w.invoke_dialog_accept("".into());
            },
            move |w| {
                let closed = tab_count(w) + 1 == tabs_edited_after.get();
                if closed {
                    assert_eq!(comments_in(&scratch("copy.pdf")), 2);
                    let _ = std::fs::remove_file(scratch("edit.pdf"));
                    let _ = std::fs::remove_file(scratch("copy.pdf"));
                }
                closed
            },
        ),
        step("open a form", open("form.pdf"), |w| {
            active_title(w) == "form.pdf" && w.get_pages().row_count() > 0
        }),
        step(
            "clicking a text field asks for its value",
            |w| click_page(w, 0, 200.0, 91.0),
            |w| w.get_dialog_kind() == "input" && w.get_dialog_title().contains("name"),
        ),
        step(
            "the field takes the value",
            |w| w.invoke_dialog_accept("Ada Lovelace".into()),
            |_| field_value(0) == "Ada Lovelace",
        ),
        step(
            "clicking a checkbox checks it",
            |w| click_page(w, 0, 158.0, 132.0),
            |w| field_value(1) == "Yes" && w.get_undo_name() == "Check box",
        ),
        step(
            "export the form data",
            |_| viewer::with(|app| app.export_form(scratch("form.xfdf"))).unwrap(),
            |_| {
                std::fs::read_to_string(scratch("form.xfdf"))
                    .is_ok_and(|x| x.contains("<value>Ada Lovelace</value>"))
            },
        ),
        step("reset clears the form", command("reset-form"), |_| {
            field_value(0).is_empty() && field_value(1) == "Off"
        }),
        step(
            "import fills it in again",
            |_| viewer::with(|app| app.import_form(scratch("form.xfdf"))).unwrap(),
            |w| {
                let done = field_value(0) == "Ada Lovelace" && field_value(1) == "Yes";
                if done {
                    let _ = std::fs::remove_file(scratch("form.xfdf"));
                }
                done && w.get_status_left().starts_with("Filled in 2 fields")
            },
        ),
        step("reset again", command("reset-form"), |_| {
            field_value(0).is_empty()
        }),
        step(
            "a note on the form",
            |w| {
                w.invoke_command("tool-note".into());
                click_page(w, 0, 450.0, 100.0);
                w.invoke_dialog_accept("First".into());
                w.invoke_command("tool-select".into());
            },
            |w| last_comment(w) == "Note: First",
        ),
        step(
            "editing a comment shows its text",
            |w| w.invoke_comment_edit(0),
            |w| w.get_dialog_kind() == "input" && w.get_dialog_input() == "First",
        ),
        step(
            "the comment takes the new text",
            |w| w.invoke_dialog_accept("Second".into()),
            |w| last_comment(w) == "Note: Second",
        ),
        step(
            "clicking a comment selects it",
            |w| click_page(w, 0, 460.0, 110.0),
            |w| marks(w, 3) == 1 && w.get_status_left().starts_with("Note selected"),
        ),
        step(
            "a selected comment takes a new colour",
            command("color-blue"),
            |w| {
                let color = viewer::with(|app| {
                    let (doc, _) = app.active_doc()?;
                    app.engine().annotations(doc, 0).ok()?.first()?.color
                })
                .flatten();
                color == Some([0.1, 0.45, 0.9]) && w.get_undo_name() == "Change colour"
            },
        ),
        step(
            "Delete removes the selected comment",
            key(char::from(Key::Delete)),
            |w| w.get_comments().row_count() == 0 && marks(w, 3) == 0,
        ),
        step(
            "flattening removes the fields",
            command("flatten-form"),
            |w| field_value(0) == "?" && w.get_undo_name() == "Flatten form fields",
        ),
        step(
            "every button has an accessible name",
            |_| {},
            |w| {
                let buttons = ElementQuery::from_root(w)
                    .match_descendants()
                    .match_accessible_role(AccessibleRole::Button)
                    .find_all();
                assert!(buttons.len() > 10, "found only {} buttons", buttons.len());
                let unnamed: Vec<String> = buttons
                    .into_iter()
                    .filter(|e| e.accessible_label().is_none_or(|l| l.trim().is_empty()))
                    .map(|e| format!("{:?} {:?}", e.id(), e.type_name()))
                    .collect();
                assert!(unnamed.is_empty(), "buttons without names: {unnamed:?}");
                true
            },
        ),
    ]
}

struct Runner {
    window: slint::Weak<MainWindow>,
    steps: Vec<Step>,
    index: usize,
    started: Option<Instant>,
    failure: Option<String>,
}

thread_local! {
    static RUNNER: RefCell<Option<Runner>> = const { RefCell::new(None) };
}

static FINISHED: AtomicBool = AtomicBool::new(false);

/// Runs the current step's action once, then checks it until it is done or times out.
fn tick() {
    RUNNER.with_borrow_mut(|runner| {
        let r = runner.as_mut().expect("runner installed");
        let Some(w) = r.window.upgrade() else { return };
        let Some(s) = r.steps.get(r.index) else {
            FINISHED.store(true, Ordering::SeqCst);
            let _ = slint::quit_event_loop();
            return;
        };
        let started = *r.started.get_or_insert_with(|| {
            (s.act)(&w);
            Instant::now()
        });
        if (s.done)(&w) {
            r.index += 1;
            r.started = None;
        } else if started.elapsed() > STEP_TIMEOUT {
            r.failure = Some(format!(
                "step {} \"{}\" timed out (page {:?}, zoom {:?}, tabs {}, dialog {:?}: {:?}, status {:?})",
                r.index + 1,
                s.name,
                w.get_page_text(),
                w.get_zoom_text(),
                tab_count(&w),
                w.get_dialog_kind(),
                w.get_dialog_text(),
                w.get_status_left(),
            ));
            FINISHED.store(true, Ordering::SeqCst);
            let _ = slint::quit_event_loop();
        }
    });
}

#[test]
fn golden_path() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let window = MainWindow::new().unwrap();
    window
        .window()
        .set_size(slint::LogicalSize::new(1100.0, 760.0));
    viewer::install(viewer::App::new(&window, Settings::default(), false));
    wire(&window);
    window.show().unwrap();

    RUNNER.set(Some(Runner {
        window: window.as_weak(),
        steps: steps(),
        index: 0,
        started: None,
        failure: None,
    }));
    // The testing backend's timers do not wake its event loop, so a thread posts the ticks.
    let ticker = std::thread::spawn(|| {
        while !FINISHED.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(2));
            let _ = slint::invoke_from_event_loop(tick);
        }
    });
    slint::run_event_loop().unwrap();
    ticker.join().unwrap();
    if let Some(message) = RUNNER.with_borrow_mut(|r| r.as_mut().and_then(|r| r.failure.take())) {
        panic!("{message}");
    }
}
