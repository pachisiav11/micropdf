//! Convert: exports the document to Word, Excel, OpenDocument, images, text, a web page or
//! Markdown. Making a PDF of other files is Combine without the open document (tools.rs).

use mp_engine::Export;

use crate::tools::{save_dialog, stem};
use crate::viewer::{App, file_name};

type Kind = (&'static str, &'static [&'static str]);

const FORMATS: [(&str, Export, Kind); 8] = [
    ("export-word", Export::Word, ("Word documents", &["docx"])),
    (
        "export-excel",
        Export::Excel,
        ("Excel workbooks", &["xlsx"]),
    ),
    (
        "export-odt",
        Export::OpenDocument,
        ("OpenDocument text", &["odt"]),
    ),
    ("export-png", Export::Png, ("PNG images", &["png"])),
    ("export-jpeg", Export::Jpeg, ("JPEG images", &["jpg"])),
    ("export-text", Export::Text, ("Text files", &["txt"])),
    ("export-html", Export::Html, ("Web pages", &["html"])),
    (
        "export-markdown",
        Export::Markdown,
        ("Markdown files", &["md"]),
    ),
];

/// Convert command `id`; false if it is not one.
pub fn command(app: &mut App, id: &str) -> bool {
    let Some(&(_, format, kind)) = FORMATS.iter().find(|f| f.0 == id) else {
        return false;
    };
    let Some((doc, path, ..)) = app.reading() else {
        return true;
    };
    let name = format!("{}.{}", stem(&path), format.extension());
    let engine = app.engine();
    save_dialog("Export", &path, name, kind, move |target| {
        let files = engine.export(doc, format, target)?;
        Ok(match files.as_slice() {
            [one] => format!("Saved {}", file_name(one)),
            [first, .., last] => format!(
                "Saved {} images, {} to {}",
                files.len(),
                file_name(first),
                file_name(last)
            ),
            [] => String::new(),
        })
    });
    true
}
