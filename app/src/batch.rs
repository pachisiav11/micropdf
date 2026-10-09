//! Batch: runs one tool over many PDFs on a thread, writing each result under the same name
//! to a folder the user picks. The files themselves are not changed.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use mp_engine::{
    Engine, Error, Export, Optimize, Overlay, Place, Protection, Recognize, Sanitize, Security,
};

use crate::FormField;
use crate::tools::{Done, Form, PDFS, choice, password, pick_files, text, today};
use crate::viewer::{self, App, file_name};

const TOOLS: [&str; 13] = [
    "Reduce file size",
    "Sanitize: metadata, scripts, attached files, hidden text",
    "Recognize text (OCR)",
    "Flatten comments and form fields",
    "Rotate pages right",
    "Add a watermark",
    "Protect with a password",
    "Export to Word",
    "Export to Excel",
    "Export to OpenDocument text",
    "Export to text",
    "Export to Markdown",
    "Export to PNG images",
];

const EXPORTS: [Export; 6] = [
    Export::Word,
    Export::Excel,
    Export::OpenDocument,
    Export::Text,
    Export::Markdown,
    Export::Png,
];

enum Job {
    Optimize,
    Sanitize,
    Ocr,
    Flatten,
    Rotate,
    Watermark(String),
    Protect(String),
    Export(Export),
}

static RUNNING: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);

/// The batch command; choosing it while a batch runs stops it after the current file.
pub fn command(app: &mut App, id: &str) -> bool {
    if id != "batch" {
        return false;
    }
    if RUNNING.load(Ordering::SeqCst) {
        STOP.store(true, Ordering::SeqCst);
        app.status("Stopping the batch\u{2026}".into());
        return true;
    }
    app.show_form(
        Form::Batch,
        "Run a tool on many files",
        "Choose the PDFs next, then the folder the results go to. The files themselves stay as \
         they are.",
        "Choose files\u{2026}",
        vec![
            choice("Tool", &TOOLS, 0),
            text("Watermark text", "CONFIDENTIAL"),
            password("Password to open"),
        ],
    );
    true
}

pub(crate) fn run(app: &mut App, f: &[FormField]) -> Done {
    let job = match f[0].index.clamp(0, TOOLS.len() as i32 - 1) as usize {
        0 => Job::Optimize,
        1 => Job::Sanitize,
        2 => Job::Ocr,
        3 => Job::Flatten,
        4 => Job::Rotate,
        5 if f[1].text.trim().is_empty() => return Err("Type the watermark's text.".into()),
        5 => Job::Watermark(f[1].text.trim().to_owned()),
        6 if f[2].text.is_empty() => return Err("Type the password to open the files.".into()),
        6 => Job::Protect(f[2].text.to_string()),
        n => Job::Export(EXPORTS[n - 7]),
    };
    let engine = app.engine();
    pick_files(
        "Choose the PDFs",
        app.active_path(),
        true,
        PDFS,
        move |files| {
            let Some(out) = rfd::FileDialog::new()
                .set_title("Choose the folder for the results")
                .pick_folder()
            else {
                return;
            };
            if files.iter().any(|f| f.parent() == Some(out.as_path())) {
                tell(|app| {
                    app.message(
                        "Choose another folder",
                        "The results take the files' names, so they go to a folder other than \
                         the one the files are in."
                            .into(),
                    )
                });
                return;
            }
            RUNNING.store(true, Ordering::SeqCst);
            STOP.store(false, Ordering::SeqCst);
            let mut failed = Vec::new();
            let mut done = 0;
            for (i, path) in files.iter().enumerate() {
                if STOP.load(Ordering::SeqCst) {
                    break;
                }
                let line = format!("Batch: {} of {}, {}", i + 1, files.len(), file_name(path));
                tell(move |app| app.status(line));
                match one(&engine, &job, path, &out) {
                    Ok(()) => done += 1,
                    Err(e) => failed.push(format!("{}: {e}", file_name(path))),
                }
            }
            RUNNING.store(false, Ordering::SeqCst);
            let s = if done == 1 { "" } else { "s" };
            let summary = format!("{done} file{s} done, in {}", out.display());
            tell(move |app| {
                if failed.is_empty() {
                    app.status(summary);
                } else {
                    app.message(
                        &format!("{} could not be done", failed.len()),
                        format!("{summary}.\n\n{}", failed.join("\n")),
                    );
                }
            });
        },
    );
    Ok(())
}

fn tell(f: impl FnOnce(&mut App) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || {
        viewer::with(f);
    });
}

/// Opens `path`, does `job` to it and writes the result to `out`.
fn one(engine: &Engine, job: &Job, path: &Path, out: &Path) -> Result<(), Error> {
    let info = engine.open(path)?;
    let done = if info.needs_password {
        Err(Error::Message("it needs a password".into()))
    } else {
        apply(
            engine,
            info.id,
            info.page_count,
            job,
            &out.join(file_name(path)),
        )
    };
    engine.close(info.id);
    done
}

fn apply(
    engine: &Engine,
    doc: mp_engine::DocId,
    count: usize,
    job: &Job,
    target: &Path,
) -> Result<(), Error> {
    let all: Vec<usize> = (0..count).collect();
    match job {
        Job::Optimize => return engine.optimize(doc, target.into(), Optimize::BALANCED),
        Job::Export(format) => {
            return engine
                .export(doc, *format, target.with_extension(format.extension()))
                .map(drop);
        }
        Job::Protect(open) => {
            let protection = Protection {
                open: open.clone(),
                owner: String::new(),
                print: true,
                copy: true,
                edit: false,
                comment: true,
            };
            return engine
                .save_secured(doc, target, false, Security::Protect(protection))
                .map(drop);
        }
        Job::Sanitize => engine.sanitize(
            doc,
            Sanitize {
                metadata: true,
                scripts: true,
                attachments: true,
                hidden_text: true,
                comments: false,
            },
        )?,
        Job::Ocr => {
            let how = Recognize {
                language: String::new(),
                skip_text: true,
                deskew: false,
            };
            engine.recognize_text(doc, &all, &how, |_, _| !STOP.load(Ordering::SeqCst))?;
        }
        Job::Flatten => engine.flatten(doc, true, true)?,
        Job::Rotate => engine.rotate_pages(doc, all, 90)?,
        Job::Watermark(words) => {
            let overlay = Overlay {
                texts: vec![(Place::Center, words.clone())],
                size: 60.0,
                angle: 45.0,
                opacity: 0.3,
                color: [0.5, 0.5, 0.5],
                date: today(),
                ..Overlay::default()
            };
            engine.stamp_pages(doc, all, overlay, "Add watermark")?;
        }
    }
    engine.save(doc, target, false).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_batch_writes_each_result_and_reports_failures() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures");
        let out = std::env::temp_dir().join("micropdf-batch-test");
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).unwrap();
        let engine = Engine::start();
        let hello = fixtures.join("hello.pdf");
        one(&engine, &Job::Rotate, &hello, &out).unwrap();
        let rotated = engine.open(out.join("hello.pdf")).unwrap();
        assert!(rotated.page_count > 0);
        engine.close(rotated.id);
        one(&engine, &Job::Export(Export::Text), &hello, &out).unwrap();
        assert!(std::fs::metadata(out.join("hello.txt")).unwrap().len() > 0);
        let locked = one(
            &engine,
            &Job::Flatten,
            &fixtures.join("encrypted.pdf"),
            &out,
        );
        assert!(locked.is_err());
        assert!(one(&engine, &Job::Rotate, &fixtures.join("not-a-pdf.pdf"), &out).is_err());
        let _ = std::fs::remove_dir_all(&out);
    }
}
