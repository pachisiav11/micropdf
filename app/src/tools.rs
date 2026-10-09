//! The page, stamp, protection and redaction tools: the forms they ask with, and what runs
//! when a form is accepted. Tools that write new files ask for the file name on a thread and
//! do the writing there, so the window keeps painting.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use mp_engine::{
    Bates, Find, INFO_FIELDS, LabelStyle, Optimize, Overlay, PATTERNS, Place, Protection, Sanitize,
    Security, parse_ranges,
};
use slint::{ModelRc, SharedString, VecModel};
use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

use crate::FormField;
use crate::viewer::{self, App, Tool, file_name};

type Done = Result<(), Box<dyn Error>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    Rotate,
    Delete,
    Move,
    Extract,
    Split,
    Crop,
    Labels,
    HeaderFooter,
    Watermark,
    Bates,
    Protect,
    Optimize,
    Properties,
    Sanitize,
    Redact,
    ApplyRedactions,
}

/// Undo steps after which the pages are read again: they change how many there are, their
/// order, size or labels.
const RESHAPING: [&str; 9] = [
    "Rotate pages",
    "Delete pages",
    "Move pages",
    "Duplicate pages",
    "Insert blank page",
    "Insert pages",
    "Replace pages",
    "Crop pages",
    "Number pages",
];

pub fn reshapes(step: &str) -> bool {
    RESHAPING.contains(&step)
}

/// Runs tool command `id`; false if it is not one.
pub fn command(app: &mut App, id: &str) -> bool {
    let form = match id {
        "pages-rotate" => Form::Rotate,
        "pages-delete-some" => Form::Delete,
        "pages-move" => Form::Move,
        "pages-extract" => Form::Extract,
        "split" => Form::Split,
        "pages-crop" => Form::Crop,
        "page-labels" => Form::Labels,
        "header-footer" => Form::HeaderFooter,
        "watermark" => Form::Watermark,
        "bates" => Form::Bates,
        "protect" => Form::Protect,
        "optimize" => Form::Optimize,
        "doc-properties" => Form::Properties,
        "sanitize" => Form::Sanitize,
        "redact-search" => Form::Redact,
        "redact-apply" => Form::ApplyRedactions,
        _ => {
            let done = match id {
                "pages-rotate-cw" => turn(app, 90),
                "pages-rotate-ccw" => turn(app, -90),
                "pages-duplicate" => duplicate(app),
                "pages-insert-blank" => insert_blank(app),
                "pages-delete" => delete(app, None),
                "pages-insert-file" => pick_pdf(app, true),
                "pages-replace" => pick_pdf(app, false),
                "combine" => combine(app),
                "unprotect" => unprotect(app),
                "redact-selection" => {
                    app.redact_selection();
                    Ok(())
                }
                "tool-redact" => {
                    app.set_tool(Tool::Redact);
                    Ok(())
                }
                _ => return false,
            };
            if let Err(e) = done {
                app.message("Could not finish", e.to_string());
            }
            return true;
        }
    };
    open(app, form);
    true
}

fn text(label: &str, value: &str) -> FormField {
    FormField {
        label: label.into(),
        kind: 0,
        text: value.into(),
        ..Default::default()
    }
}

fn password(label: &str) -> FormField {
    FormField {
        kind: 1,
        ..text(label, "")
    }
}

fn check(label: &str, on: bool) -> FormField {
    FormField {
        kind: 2,
        checked: on,
        ..text(label, "")
    }
}

fn choice(label: &str, options: &[&str], index: i32) -> FormField {
    let options: Vec<SharedString> = options.iter().map(|&o| o.into()).collect();
    FormField {
        kind: 3,
        options: ModelRc::new(VecModel::from(options)),
        index,
        ..text(label, "")
    }
}

/// Pages from 0 as the user writes them, from 1: "1-3, 5".
fn ranges_text(pages: &[usize]) -> String {
    let mut parts = Vec::new();
    let mut i = 0;
    while i < pages.len() {
        let mut j = i;
        while j + 1 < pages.len() && pages[j + 1] == pages[j] + 1 {
            j += 1;
        }
        parts.push(if i == j {
            (pages[i] + 1).to_string()
        } else {
            format!("{}-{}", pages[i] + 1, pages[j] + 1)
        });
        i = j + 1;
    }
    parts.join(", ")
}

const PLACES: [(&str, Place); 6] = [
    ("Bottom right", Place::BottomRight),
    ("Bottom centre", Place::Bottom),
    ("Bottom left", Place::BottomLeft),
    ("Top right", Place::TopRight),
    ("Top centre", Place::Top),
    ("Top left", Place::TopLeft),
];

const COLORS: [(&str, [f32; 3]); 4] = [
    ("Grey", [0.5, 0.5, 0.5]),
    ("Red", [0.8, 0.1, 0.1]),
    ("Blue", [0.1, 0.3, 0.8]),
    ("Black", [0.0, 0.0, 0.0]),
];

const LABEL_STYLES: [(&str, LabelStyle); 6] = [
    ("1, 2, 3", LabelStyle::Decimal),
    ("i, ii, iii", LabelStyle::LowerRoman),
    ("I, II, III", LabelStyle::UpperRoman),
    ("a, b, c", LabelStyle::LowerAlpha),
    ("A, B, C", LabelStyle::UpperAlpha),
    ("The prefix alone", LabelStyle::None),
];

fn names<T>(list: &[(&'static str, T)]) -> Vec<&'static str> {
    list.iter().map(|(n, _)| *n).collect()
}

fn open(app: &mut App, form: Form) {
    let Some((_, _, current, _)) = app.reading() else {
        return;
    };
    let chosen = ranges_text(&app.chosen_pages());
    let all = format!("1-{}", app.page_count());
    let (title, note, ok, fields): (&str, &str, &'static str, Vec<FormField>) = match form {
        Form::Rotate => (
            "Rotate pages",
            "",
            "Rotate",
            vec![
                text("Pages", &chosen),
                choice("Turn", &["Right", "Left", "Upside down"], 0),
            ],
        ),
        Form::Delete => (
            "Delete pages",
            "You can undo this until you save.",
            "Delete",
            vec![text("Pages", &chosen)],
        ),
        Form::Move => (
            "Move pages",
            "Type end to move them after the last page.",
            "Move",
            vec![text("Pages", &chosen), text("Before page", "1")],
        ),
        Form::Extract => (
            "Extract pages",
            "The pages are copied to a new PDF.",
            "Extract\u{2026}",
            vec![text("Pages", &chosen)],
        ),
        Form::Split => (
            "Split document",
            "Each part is saved as a PDF of its own, numbered after the name you choose.",
            "Split\u{2026}",
            vec![
                choice(
                    "Split",
                    &[
                        "Every few pages",
                        "At these page ranges",
                        "At each top-level bookmark",
                    ],
                    0,
                ),
                text("Pages per file", "1"),
                text("Page ranges", "1-3, 4-"),
            ],
        ),
        Form::Crop => (
            "Crop pages",
            "How much to cut off each edge, in millimetres.",
            "Crop",
            vec![
                text("Pages", &chosen),
                text("Left", "0"),
                text("Top", "0"),
                text("Right", "0"),
                text("Bottom", "0"),
            ],
        ),
        Form::Labels => (
            "Number pages",
            "Labels show in place of page numbers, such as i, ii, iii for a preface. They \
             apply until the next labelled page.",
            "Apply",
            vec![
                text("From page", &(current + 1).to_string()),
                choice("Style", &names(&LABEL_STYLES), 0),
                text("Prefix", ""),
                text("Start at", "1"),
            ],
        ),
        Form::HeaderFooter => (
            "Header and footer",
            "{page} is the page number, {pages} the page count and {date} today's date.",
            "Add",
            vec![
                text("Pages", &all),
                text("Top left", ""),
                text("Top centre", ""),
                text("Top right", ""),
                text("Bottom left", ""),
                text("Bottom centre", "Page {page} of {pages}"),
                text("Bottom right", ""),
                text("Size (pt)", "10"),
            ],
        ),
        Form::Watermark => (
            "Watermark",
            "",
            "Add",
            vec![
                text("Text", "CONFIDENTIAL"),
                text("Pages", &all),
                text("Size (pt)", "60"),
                text("Angle (degrees)", "45"),
                text("Opacity (%)", "30"),
                choice("Colour", &names(&COLORS), 0),
                check("Behind the page's content", false),
            ],
        ),
        Form::Bates => (
            "Bates numbering",
            "Each page gets the next number.",
            "Number",
            vec![
                text("Prefix", ""),
                text("Start at", "1"),
                text("Digits", "6"),
                text("Suffix", ""),
                choice("Place", &names(&PLACES), 0),
                text("Pages", &all),
            ],
        ),
        Form::Protect => (
            "Protect with a password",
            "The file is saved now, encrypted with AES-256. Without the owner password, \
             readers get only what you allow below.",
            "Protect and save",
            vec![
                password("Password to open"),
                password("Owner password"),
                check("Allow printing", true),
                check("Allow copying text", true),
                check("Allow changing pages and content", false),
                check("Allow comments and form filling", true),
            ],
        ),
        Form::Optimize => (
            "Reduce file size",
            "A smaller copy is saved under a new name; this file stays as it is.",
            "Save copy\u{2026}",
            vec![
                choice(
                    "Images",
                    &[
                        "Keep as they are",
                        "150 dpi, good quality",
                        "96 dpi, smallest",
                    ],
                    1,
                ),
                check("Keep only the font glyphs in use", true),
            ],
        ),
        Form::Properties => (
            "Document properties",
            "",
            "Apply",
            INFO_FIELDS
                .iter()
                .map(|&key| text(key, &app.info_value(key)))
                .collect(),
        ),
        Form::Sanitize => (
            "Sanitize document",
            "Removes what the file holds besides its pages. You can undo this until you save.",
            "Remove",
            vec![
                check("Metadata", true),
                check("Scripts and actions", true),
                check("Attached files", true),
                check("Hidden text", true),
                check("Comments", false),
            ],
        ),
        Form::Redact => {
            let mut finds = vec!["Words"];
            finds.extend(PATTERNS.iter().map(|(name, _)| *name));
            finds.push("Regular expression");
            (
                "Search and redact",
                "Matches are marked for redaction. Check the marks, then apply them.",
                "Mark",
                vec![
                    choice("Find", &finds, 0),
                    text("Words or expression", ""),
                    text("Pages", &all),
                ],
            )
        }
        Form::ApplyRedactions => {
            let marks = app.redaction_count();
            if marks == 0 {
                app.status(
                    "There are no redaction marks. Mark text or areas for redaction first.".into(),
                );
                return;
            }
            (
                "Apply redactions?",
                "What lies under the marks is removed: text, images and drawings. Once you \
                 save, it is gone from the file for good.",
                "Apply",
                Vec::new(),
            )
        }
    };
    app.show_form(form, title, note, ok, fields);
}

/// Runs `form` with the values the user gave; problems show in a message.
pub fn accept(app: &mut App, form: Form, fields: Vec<FormField>) {
    if let Err(e) = run(app, form, &fields) {
        app.message("Could not finish", e.to_string());
    }
}

fn number<T: std::str::FromStr>(field: &FormField) -> Result<T, Box<dyn Error>> {
    field
        .text
        .trim()
        .replace(',', ".")
        .parse()
        .map_err(|_| format!("{} must be a number.", field.label).into())
}

fn run(app: &mut App, form: Form, f: &[FormField]) -> Done {
    let Some((doc, path, _, count)) = app.reading() else {
        return Ok(());
    };
    let engine = app.engine();
    let pages = |field: &FormField| -> Result<Vec<usize>, Box<dyn Error>> {
        let mut pages: Vec<usize> = parse_ranges(&field.text, count)?.concat();
        pages.sort_unstable();
        pages.dedup();
        Ok(pages)
    };
    match form {
        Form::Rotate => {
            let by = [90, -90, 180][f[1].index.clamp(0, 2) as usize];
            engine.rotate_pages(doc, pages(&f[0])?, by)?;
            app.restructure(None, false);
        }
        Form::Delete => delete(app, Some(pages(&f[0])?))?,
        Form::Move => {
            let pages = pages(&f[0])?;
            let before = match f[1].text.trim() {
                "end" | "End" => count,
                _ => {
                    let n: usize = number(&f[1])?;
                    if !(1..=count).contains(&n) {
                        return Err(format!("There is no page {n}.").into());
                    }
                    n - 1
                }
            };
            let n = pages.len();
            let at = engine.move_pages(doc, pages, before)?;
            app.restructure(Some(at), false);
            app.choose(at..at + n);
        }
        Form::Extract => {
            let pages = pages(&f[0])?;
            let name = format!("{} pages {}.pdf", stem(&path), f[0].text.trim());
            save_dialog("Extract pages", &path, name, move |target| {
                engine
                    .extract_pages(doc, pages, target.clone())
                    .map(|()| format!("Saved {}", file_name(&target)))
            });
        }
        Form::Split => {
            let groups = match f[0].index {
                0 => {
                    let n: usize = number(&f[1])?;
                    if n == 0 {
                        return Err("Pages per file must be 1 or more.".into());
                    }
                    (0..count)
                        .collect::<Vec<_>>()
                        .chunks(n)
                        .map(<[usize]>::to_vec)
                        .collect()
                }
                1 => parse_ranges(&f[2].text, count)?,
                _ => {
                    let mut starts = app.chapter_starts();
                    if starts.len() < 2 {
                        return Err("The document has fewer than two top-level bookmarks.".into());
                    }
                    starts[0] = 0;
                    starts.push(count);
                    starts.windows(2).map(|w| (w[0]..w[1]).collect()).collect()
                }
            };
            let name = format!("{}.pdf", stem(&path));
            save_dialog("Split: name the parts", &path, name, move |first| {
                let parts = engine.split(doc, groups, first)?;
                Ok(format!("Saved {} files", parts.len()))
            });
        }
        Form::Crop => {
            let mut margins = [0.0f32; 4];
            for (m, field) in margins.iter_mut().zip(&f[1..5]) {
                *m = number::<f32>(field)?.max(0.0) * 72.0 / 25.4;
            }
            engine.crop_pages(doc, pages(&f[0])?, margins)?;
            app.restructure(None, false);
        }
        Form::Labels => {
            let from: usize = number(&f[0])?;
            if !(1..=count).contains(&from) {
                return Err(format!("There is no page {from}.").into());
            }
            let style = LABEL_STYLES[f[1].index.clamp(0, 5) as usize].1;
            let start: i32 = number(&f[3])?;
            engine.label_pages(doc, from - 1, style, f[2].text.to_string(), start)?;
            app.restructure(None, false);
        }
        Form::HeaderFooter => {
            let places = [
                Place::TopLeft,
                Place::Top,
                Place::TopRight,
                Place::BottomLeft,
                Place::Bottom,
                Place::BottomRight,
            ];
            let overlay = Overlay {
                texts: places
                    .into_iter()
                    .zip(&f[1..7])
                    .map(|(place, field)| (place, field.text.to_string()))
                    .filter(|(_, t)| !t.trim().is_empty())
                    .collect(),
                size: number(&f[7])?,
                date: today(),
                ..Overlay::default()
            };
            if overlay.texts.is_empty() {
                return Err("Type the text for at least one place.".into());
            }
            engine.stamp_pages(doc, pages(&f[0])?, overlay, "Add header and footer")?;
            app.edited(None);
        }
        Form::Watermark => {
            if f[0].text.trim().is_empty() {
                return Err("Type the watermark's text.".into());
            }
            let overlay = Overlay {
                texts: vec![(Place::Center, f[0].text.to_string())],
                size: number(&f[2])?,
                angle: number(&f[3])?,
                opacity: number::<f32>(&f[4])? / 100.0,
                color: COLORS[f[5].index.clamp(0, 3) as usize].1,
                behind: f[6].checked,
                date: today(),
                ..Overlay::default()
            };
            engine.stamp_pages(doc, pages(&f[1])?, overlay, "Add watermark")?;
            app.edited(None);
        }
        Form::Bates => {
            let bates = Bates {
                prefix: f[0].text.to_string(),
                start: number(&f[1])?,
                digits: number(&f[2])?,
                suffix: f[3].text.to_string(),
            };
            let overlay = Overlay {
                texts: vec![(PLACES[f[4].index.clamp(0, 5) as usize].1, "{bates}".into())],
                bates: Some(bates.clone()),
                ..Overlay::default()
            };
            let next = engine.stamp_pages(doc, pages(&f[5])?, overlay, "Add Bates numbers")?;
            app.edited(None);
            app.status(format!(
                "Numbered to {}; the next number is {}",
                bates.number(next.saturating_sub(1)),
                bates.number(next)
            ));
        }
        Form::Protect => {
            let protection = Protection {
                open: f[0].text.to_string(),
                owner: f[1].text.to_string(),
                print: f[2].checked,
                copy: f[3].checked,
                edit: f[4].checked,
                comment: f[5].checked,
            };
            if protection.open.is_empty() && protection.owner.is_empty() {
                return Err("Type a password to open the file, an owner password, or both.".into());
            }
            app.save_secured(Security::Protect(protection));
        }
        Form::Optimize => {
            let how = Optimize {
                fonts: f[1].checked,
                ..[Optimize::LOSSLESS, Optimize::BALANCED, Optimize::SMALLEST]
                    [f[0].index.clamp(0, 2) as usize]
            };
            let before = std::fs::metadata(&path).map_or(0, |m| m.len());
            let name = format!("{} (smaller).pdf", stem(&path));
            save_dialog("Reduce file size", &path, name, move |target| {
                engine.optimize(doc, target.clone(), how)?;
                let after = std::fs::metadata(&target).map_or(0, |m| m.len());
                Ok(format!(
                    "Saved {}: {} instead of {}",
                    file_name(&target),
                    viewer::format_size(after),
                    viewer::format_size(before)
                ))
            });
        }
        Form::Properties => {
            let values = INFO_FIELDS
                .iter()
                .zip(f)
                .map(|(key, field)| ((*key).to_owned(), field.text.trim().to_owned()))
                .collect();
            engine.set_info(doc, values)?;
            app.edited(None);
        }
        Form::Sanitize => {
            let what = Sanitize {
                metadata: f[0].checked,
                scripts: f[1].checked,
                attachments: f[2].checked,
                hidden_text: f[3].checked,
                comments: f[4].checked,
            };
            engine.sanitize(doc, what)?;
            app.edited(None);
            app.status("Sanitized. Save to write the result.".into());
        }
        Form::Redact => {
            let text = f[1].text.to_string();
            let find = match f[0].index as usize {
                0 => Find::Text(text),
                i if i <= PATTERNS.len() => Find::Pattern(PATTERNS[i - 1].1.into()),
                _ => Find::Pattern(text),
            };
            let marked = engine.mark_redactions(doc, pages(&f[2])?, find)?;
            app.edited(None);
            app.status(match marked {
                0 => "No matches".into(),
                1 => "Marked 1 match for redaction".into(),
                n => format!("Marked {n} matches for redaction"),
            });
        }
        Form::ApplyRedactions => {
            let changed = engine.apply_redactions(doc)?;
            app.edited(None);
            app.status(format!(
                "Redacted {changed} page{}. Save to remove the content from the file.",
                if changed == 1 { "" } else { "s" }
            ));
        }
    }
    Ok(())
}

fn turn(app: &mut App, by: i32) -> Done {
    let Some((doc, ..)) = app.reading() else {
        return Ok(());
    };
    app.engine().rotate_pages(doc, app.chosen_pages(), by)?;
    app.restructure(None, false);
    Ok(())
}

fn duplicate(app: &mut App) -> Done {
    let Some((doc, ..)) = app.reading() else {
        return Ok(());
    };
    app.engine().duplicate_pages(doc, app.chosen_pages())?;
    app.restructure(None, false);
    Ok(())
}

fn insert_blank(app: &mut App) -> Done {
    let Some((doc, ..)) = app.reading() else {
        return Ok(());
    };
    let at = app.chosen_pages().last().map_or(0, |p| p + 1);
    app.engine().insert_blank_page(doc, at)?;
    app.restructure(Some(at), false);
    app.choose([at]);
    Ok(())
}

/// Deletes `pages`, or the picked ones.
fn delete(app: &mut App, pages: Option<Vec<usize>>) -> Done {
    let Some((doc, ..)) = app.reading() else {
        return Ok(());
    };
    let pages = pages.unwrap_or_else(|| app.chosen_pages());
    let first = pages.first().copied().unwrap_or(0);
    app.engine().delete_pages(doc, pages)?;
    app.choose([]);
    app.restructure(Some(first), false);
    Ok(())
}

fn unprotect(app: &mut App) -> Done {
    if app.reading().is_none() {
        return Ok(());
    }
    match app.info_value("Encryption").as_str() {
        "" | "None" => app.status("This file has no password".into()),
        _ => app.save_secured(Security::Remove),
    }
    Ok(())
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn today() -> String {
    let mut t = SYSTEMTIME::default();
    unsafe { GetLocalTime(&mut t) };
    format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay)
}

/// One system file dialog at a time.
static PICKING: AtomicBool = AtomicBool::new(false);

/// Asks on a thread where to save, next to `path`; then runs `write` there and shows what it
/// says, or why it failed.
fn save_dialog(
    title: &'static str,
    path: &Path,
    name: String,
    write: impl FnOnce(PathBuf) -> Result<String, mp_engine::Error> + Send + 'static,
) {
    if PICKING.swap(true, Ordering::SeqCst) {
        return;
    }
    let dir = path.parent().map(Path::to_path_buf);
    std::thread::spawn(move || {
        let mut dialog = rfd::FileDialog::new()
            .set_title(title)
            .add_filter("PDF documents", &["pdf"])
            .set_file_name(name);
        if let Some(dir) = dir {
            dialog = dialog.set_directory(dir);
        }
        let target = dialog.save_file();
        PICKING.store(false, Ordering::SeqCst);
        let Some(target) = target else { return };
        let result = write(target);
        let _ = slint::invoke_from_event_loop(move || {
            viewer::with(|app| match result {
                Ok(done) => app.status(done),
                Err(e) => app.message("Could not save", e.to_string()),
            });
        });
    });
}

/// Asks for PDFs on a thread, next to `path` when given.
fn pick_files(
    title: &'static str,
    path: Option<PathBuf>,
    many: bool,
    then: impl FnOnce(Vec<PathBuf>) + Send + 'static,
) {
    if PICKING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let mut dialog = rfd::FileDialog::new()
            .set_title(title)
            .add_filter("PDF documents", &["pdf"]);
        if let Some(dir) = path.as_deref().and_then(Path::parent) {
            dialog = dialog.set_directory(dir);
        }
        let files = if many {
            dialog.pick_files()
        } else {
            dialog.pick_file().map(|f| vec![f])
        };
        PICKING.store(false, Ordering::SeqCst);
        if let Some(files) = files.filter(|f| !f.is_empty()) {
            then(files);
        }
    });
}

/// Inserts another PDF's pages after the picked ones, or puts its first pages in their place.
fn pick_pdf(app: &mut App, insert: bool) -> Done {
    let Some((doc, path, ..)) = app.reading() else {
        return Ok(());
    };
    let pages = app.chosen_pages();
    let title = if insert {
        "Insert pages from"
    } else {
        "Replace pages with"
    };
    pick_files(title, Some(path), false, move |files| {
        let file = files[0].clone();
        let _ = slint::invoke_from_event_loop(move || {
            viewer::with(|app| {
                let engine = app.engine();
                let at = pages.last().map_or(0, |p| p + 1);
                let done = if insert {
                    engine
                        .insert_file(doc, at, file, String::new())
                        .map(|n| (at, n))
                } else {
                    engine
                        .replace_pages(doc, pages.clone(), file, String::new())
                        .map(|n| (pages[0], n))
                };
                match done {
                    Ok((at, n)) => {
                        app.restructure(Some(at), false);
                        app.choose(at..at + n);
                    }
                    Err(e) => app.message("Could not add the pages", e.to_string()),
                }
            });
        });
    });
    Ok(())
}

/// Combines the active document's file (if any) with PDFs the user picks into a new file,
/// and opens it.
fn combine(app: &mut App) -> Done {
    let first = app.active_path();
    let title = if first.is_some() {
        "Combine: choose the files to add after this one"
    } else {
        "Combine: choose the files"
    };
    pick_files(title, first.clone(), true, move |mut files| {
        files.sort();
        let sources: Vec<PathBuf> = first.into_iter().chain(files).collect();
        if sources.len() < 2 {
            return;
        }
        let name = format!("{} combined.pdf", stem(&sources[0]));
        let start = sources[0].clone();
        let _ = slint::invoke_from_event_loop(move || {
            save_dialog("Save the combined PDF", &start, name, move |target| {
                mp_engine::combine(&sources, &target)?;
                let shown = target.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    viewer::with(|app| app.open_paths(vec![shown]));
                });
                Ok(format!("Combined {} files", sources.len()))
            });
        });
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ranges_text;

    #[test]
    fn ranges_read_back() {
        assert_eq!(ranges_text(&[0, 1, 2, 4, 7, 8]), "1-3, 5, 8-9");
        assert_eq!(ranges_text(&[3]), "4");
    }
}
