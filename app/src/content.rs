//! Editing page content. Edit text opens a block of text in the editor that form fields use
//! and sets what was typed in its place; Add text writes new text the same way. Edit images
//! picks an image to move, resize, replace or delete, and places new ones. The Link tool draws
//! links as boxes, and the outline panel's menu adds, renames, moves and deletes bookmarks.

use std::path::PathBuf;

use mp_engine::{DocId, ImageSource, LinkTarget, OutlineItem, Rect};

use crate::FormField;
use crate::tools::{Done, Form, pick_files, text};
use crate::viewer::{self, App, Drag, HANDLE, Tool};

/// The id of a picked image, and of a `Drag::Shape` that drags one.
pub const IMAGE: i32 = i32::MIN;

/// The type size of added text.
const SIZE: f32 = 12.0;

const IMAGES: (&str, &[&str]) = (
    "Images",
    &["png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff"],
);

#[derive(Default)]
pub struct Content {
    pub(crate) text: Option<TextEdit>,
    /// The picked image or form field: its document, page, id (`IMAGE`, or the field's
    /// widget) and box.
    pub(crate) picked: Option<(DocId, usize, i32, Rect)>,
    /// An image file waiting for the box it goes in.
    file: Option<PathBuf>,
    /// The bookmark the outline panel's menu is for.
    bookmark: Option<usize>,
}

/// The block of text open in the editor, or the place new text goes.
pub struct TextEdit {
    doc: DocId,
    page: usize,
    rect: Rect,
    size: f32,
    original: String,
    new: bool,
}

/// Content command `id`; false if it is not one.
pub fn command(app: &mut App, id: &str) -> bool {
    let (tool, hint) = match id {
        "tool-edit-text" => (Tool::EditText, "Click a block of text to edit it"),
        "tool-add-text" => (Tool::AddText, "Click where the text goes"),
        "tool-edit-image" => (
            Tool::EditImage,
            "Click an image to pick it; drag it, or a corner to resize it",
        ),
        "tool-link" => (
            Tool::Link,
            "Drag a box to make a link; click a link to delete it",
        ),
        "image-add" => {
            add_image(app);
            return true;
        }
        "image-replace" => {
            replace_image(app);
            return true;
        }
        "image-delete" => {
            if !(app.content.picked.is_some_and(|p| p.2 == IMAGE) && delete_picked(app)) {
                app.status("Pick an image with Edit images first".into());
            }
            return true;
        }
        _ if id.starts_with("bookmark-") => {
            bookmarks(app, id);
            return true;
        }
        _ => return false,
    };
    app.set_tool(tool);
    app.status(hint.into());
    true
}

/// A click with Edit text or Add text at `(x, y)` on `page`: opens the block of text there, or
/// an empty editor for new text.
pub fn click(app: &mut App, page: usize, (x, y): (f32, f32)) {
    let Some((doc, sizes)) = app.active_doc() else {
        return;
    };
    let edit = if app.tool() == Tool::AddText {
        let width = sizes.get(page).map_or(612.0, |s| s.0);
        TextEdit {
            doc,
            page,
            rect: Rect {
                x0: x,
                y0: y,
                x1: (width - 36.0).max(x + 72.0),
                y1: y + SIZE * 1.2,
            },
            size: SIZE,
            original: String::new(),
            new: true,
        }
    } else {
        let blocks = app.engine().text_blocks(doc, page).unwrap_or_default();
        let near =
            |r: &Rect| x >= r.x0 - 2.0 && x <= r.x1 + 2.0 && y >= r.y0 - 2.0 && y <= r.y1 + 2.0;
        let Some(block) = blocks.into_iter().find(|b| near(&b.rect)) else {
            app.status("There is no text there to edit".into());
            return;
        };
        TextEdit {
            doc,
            page,
            rect: block.rect,
            size: block.size,
            original: block.text,
            new: false,
        }
    };
    if let Some(w) = app.window() {
        w.set_field_text(edit.original.clone().into());
        w.set_field_name("Text".into());
        w.set_field_multiline(true);
    }
    app.content.text = Some(edit);
    place(app);
    if let Some(w) = app.window() {
        w.invoke_focus_field_editor();
    }
}

/// Puts the editor over the open block, a little roomier than it, for typing.
pub fn place(app: &App) {
    let (Some(w), Some(e)) = (app.window(), app.content.text.as_ref()) else {
        return;
    };
    if app.reading().is_none_or(|r| r.0 != e.doc) {
        w.set_field_editing(false);
        return;
    }
    let room = Rect {
        x0: e.rect.x0 - 4.0,
        y0: e.rect.y0 - 4.0,
        x1: e.rect.x1 + 4.0,
        y1: e.rect.y1 + e.size * 1.5,
    };
    let Some((x, y, width, height)) = app.view_rect(e.page, &room) else {
        w.set_field_editing(false);
        return;
    };
    w.set_field_x(x);
    w.set_field_y(y);
    w.set_field_width(width);
    w.set_field_height(height);
    w.set_field_font_size(e.size * height / room.height());
    w.set_field_editing(true);
}

/// Sets the text typed in place of the block, or adds it.
pub fn commit(app: &mut App) {
    let Some(e) = app.content.text.take() else {
        return;
    };
    let text = app
        .window()
        .map(|w| {
            w.set_field_editing(false);
            w.invoke_focus_view();
            w.get_field_text().to_string()
        })
        .unwrap_or_default();
    if text == e.original || app.reading().is_none_or(|r| r.0 != e.doc) {
        return;
    }
    let engine = app.engine();
    let done = if e.new {
        engine.add_text(e.doc, e.page, e.rect, text, e.size)
    } else {
        engine.replace_text(e.doc, e.page, e.rect, text)
    };
    let verb = if e.new { "Added" } else { "Edited" };
    match done {
        Ok(r) => {
            app.edited(Some(e.page));
            app.status(if r.own || r.font.is_empty() || r.font == "Helvetica" {
                format!("{verb} the text")
            } else if e.new {
                format!("{verb} the text, set in {}", r.font)
            } else {
                format!(
                    "{verb} the text, set in {}: the document's font lacks some of these letters",
                    r.font
                )
            });
        }
        Err(err) => app.message("Could not set the text", err.to_string()),
    }
}

/// Closes the editor and drops what was typed, and lets go of the picked image or field and
/// of a file waiting to be placed; false if the editor was not open.
pub fn cancel(app: &mut App) -> bool {
    app.content.file = None;
    if app.content.picked.take().is_some() {
        app.refresh_marks();
    }
    if app.content.text.take().is_none() {
        return false;
    }
    if let Some(w) = app.window() {
        w.set_field_editing(false);
        w.invoke_focus_view();
    }
    true
}

/// Starts a drag with Edit images at page point `p`, document point `at`: a corner of the
/// picked image resizes it, an image moves, and with a file waiting a box is drawn for it.
pub(crate) fn image_down(
    app: &mut App,
    page: usize,
    p: (f32, f32),
    at: (f32, f32),
) -> Option<Drag> {
    let (doc, ..) = app.reading()?;
    if let Some(drag) = corner(app, doc, page, p, at) {
        return Some(drag);
    }
    if app.content.file.is_some() {
        return Some(Drag::Draw {
            page,
            points: vec![p],
        });
    }
    let images = app.engine().page_images(doc, page).unwrap_or_default();
    // The image drawn last is the one on top.
    let Some(rect) = images.into_iter().rev().find(|r| r.contains(p.0, p.1)) else {
        app.content.picked = None;
        app.refresh_marks();
        app.status("There is no image there".into());
        return None;
    };
    app.content.picked = Some((doc, page, IMAGE, rect));
    app.refresh_marks();
    app.status(
        "Image picked. Drag it to move it, or a corner to resize it; Delete removes it".into(),
    );
    Some(shape(page, IMAGE, at, p, rect, false))
}

/// A drag that resizes the picked image or field, when document point `at` is on one of its
/// corners.
pub(crate) fn corner(
    app: &App,
    doc: DocId,
    page: usize,
    p: (f32, f32),
    at: (f32, f32),
) -> Option<Drag> {
    let (d, pg, id, rect) = app.content.picked?;
    ((d, pg) == (doc, page) && on_corner(app, page, rect, at))
        .then(|| shape(page, id, at, p, rect, true))
}

pub(crate) fn shape(
    page: usize,
    id: i32,
    origin: (f32, f32),
    start: (f32, f32),
    rect: Rect,
    resize: bool,
) -> Drag {
    Drag::Shape {
        page,
        id,
        origin,
        start,
        rect,
        resize,
        keep_aspect: id == IMAGE,
        current: rect,
        moved: false,
    }
}

fn on_corner(app: &App, page: usize, rect: Rect, (x, y): (f32, f32)) -> bool {
    app.view_rect(page, &rect).is_some_and(|(fx, fy, w, h)| {
        [(fx, fy), (fx + w, fy), (fx + w, fy + h), (fx, fy + h)]
            .iter()
            .any(|&(cx, cy)| (x - cx).abs() <= HANDLE + 1.0 && (y - cy).abs() <= HANDLE + 1.0)
    })
}

/// The picked image was dragged to `to`.
pub fn image_moved(app: &mut App, page: usize, to: Rect) {
    if let Some((doc, _, IMAGE, from)) = app.content.picked {
        change_image(app, doc, page, Some(from), Some(to), ImageSource::Same);
    }
}

/// A box drawn with Edit images while a file waits for one; a click puts it in a square.
pub fn image_drawn(app: &mut App, page: usize, rect: Rect, click: bool) {
    let (Some(file), Some((doc, ..))) = (app.content.file.take(), app.reading()) else {
        return;
    };
    let to = if click {
        Rect {
            x1: rect.x0 + 200.0,
            y1: rect.y0 + 200.0,
            ..rect
        }
    } else {
        rect
    };
    change_image(app, doc, page, None, Some(to), ImageSource::File(file));
}

fn change_image(
    app: &mut App,
    doc: DocId,
    page: usize,
    from: Option<Rect>,
    to: Option<Rect>,
    source: ImageSource,
) {
    let done = match (&from, &to, &source) {
        (None, ..) => "Added the image",
        (_, None, _) => "Deleted the image",
        (_, _, ImageSource::File(_)) => "Replaced the image",
        _ => "Moved the image",
    };
    let engine = app.engine();
    match engine.place_image(doc, page, from, to, source) {
        Ok(()) => {
            // A file is fitted in the box, so find where it went.
            let center = |r: &Rect| ((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0);
            app.content.picked = to.and_then(|to| {
                let (cx, cy) = center(&to);
                let images = engine.page_images(doc, page).unwrap_or_default();
                let d = |r: &Rect| {
                    let (x, y) = center(r);
                    (x - cx).hypot(y - cy)
                };
                let r = images.into_iter().min_by(|a, b| d(a).total_cmp(&d(b)))?;
                Some((doc, page, IMAGE, r))
            });
            app.edited(Some(page));
            app.status(done.into());
        }
        Err(e) => {
            app.refresh_marks();
            app.message("Could not change the image", e.to_string());
        }
    }
}

fn add_image(app: &mut App) {
    let Some((_, path, ..)) = app.reading() else {
        return;
    };
    app.set_tool(Tool::EditImage);
    pick_files("Add an image", Some(path), false, IMAGES, move |files| {
        let file = files[0].clone();
        let _ = slint::invoke_from_event_loop(move || {
            viewer::with(|app| {
                if app.tool() == Tool::EditImage {
                    app.content.file = Some(file);
                    app.status("Drag a box for the image, or click where it goes".into());
                }
            });
        });
    });
}

fn replace_image(app: &mut App) {
    let picked = app.content.picked.filter(|p| p.2 == IMAGE);
    let (Some((doc, page, _, rect)), Some((_, path, ..))) = (picked, app.reading()) else {
        app.status("Pick an image with Edit images first".into());
        return;
    };
    pick_files(
        "Replace the image with",
        Some(path),
        false,
        IMAGES,
        move |files| {
            let file = files[0].clone();
            let _ = slint::invoke_from_event_loop(move || {
                viewer::with(|app| {
                    let source = ImageSource::File(file);
                    change_image(app, doc, page, Some(rect), Some(rect), source);
                });
            });
        },
    );
}

/// Deletes the picked image or field; false if none is picked in the active document.
pub fn delete_picked(app: &mut App) -> bool {
    let Some((doc, page, id, rect)) = app.content.picked else {
        return false;
    };
    if app.reading().is_none_or(|r| r.0 != doc) {
        return false;
    }
    if id == IMAGE {
        change_image(app, doc, page, Some(rect), None, ImageSource::Same);
    } else {
        crate::prepare::delete(app, doc, page, id);
    }
    true
}

/// A box drawn with the Link tool: asks where the link goes. A click on a link offers to
/// delete it.
pub fn link_drawn(app: &mut App, page: usize, rect: Rect, click: bool) {
    let Some((doc, ..)) = app.reading() else {
        return;
    };
    if !click {
        app.show_form(
            Form::Link { page, rect },
            "Add a link",
            "Type the number of the page it goes to, or a web address.",
            "Add",
            vec![text("Goes to", "")],
        );
        return;
    }
    let links = app.engine().links(doc, page).unwrap_or_default();
    let Some(link) = links
        .into_iter()
        .find(|l| l.rect.contains(rect.x0, rect.y0))
    else {
        app.status("Drag a box to make a link".into());
        return;
    };
    let to = match &link.target {
        LinkTarget::Page { page, .. } => format!("page {}", page + 1),
        LinkTarget::Uri(uri) => uri.clone(),
    };
    app.show_form(
        Form::DeleteLink {
            page,
            rect: link.rect,
        },
        "Delete the link?",
        &format!("It goes to {to}."),
        "Delete",
        Vec::new(),
    );
}

pub(crate) fn add_link(app: &mut App, page: usize, rect: Rect, f: &[FormField]) -> Done {
    let Some((doc, _, _, count)) = app.reading() else {
        return Ok(());
    };
    let to = f[0].text.trim();
    let target = match to.parse::<usize>() {
        Ok(n) if (1..=count).contains(&n) => LinkTarget::Page {
            page: n - 1,
            top: Some(0.0),
        },
        Ok(_) => return Err(format!("The document's pages are 1 to {count}.").into()),
        Err(_) if to.contains(':') => LinkTarget::Uri(to.into()),
        Err(_) if to.contains('.') && !to.contains(' ') => LinkTarget::Uri(format!("https://{to}")),
        Err(_) => return Err("Type a page number or a web address.".into()),
    };
    app.engine().add_link(doc, page, rect, target)?;
    app.edited(Some(page));
    app.status("Added the link".into());
    Ok(())
}

pub(crate) fn delete_link(app: &mut App, page: usize, rect: Rect) -> Done {
    let Some((doc, ..)) = app.reading() else {
        return Ok(());
    };
    app.engine().delete_link(doc, page, rect)?;
    app.edited(Some(page));
    app.status("Deleted the link".into());
    Ok(())
}

/// The outline panel's menu opens on row `row`.
pub fn outline_menu(app: &mut App, row: usize) {
    app.content.bookmark = app.bookmark_at_row(row);
}

/// Where bookmark `i`'s children end: the index after its last one.
fn end(items: &[OutlineItem], i: usize) -> usize {
    let depth = items[i].depth;
    (i + 1..items.len())
        .find(|&j| items[j].depth <= depth)
        .unwrap_or(items.len())
}

/// The bookmark before `i` at its level, under the same parent.
fn previous(items: &[OutlineItem], i: usize) -> Option<usize> {
    let depth = items[i].depth;
    (0..i)
        .rev()
        .find(|&j| items[j].depth <= depth)
        .filter(|&j| items[j].depth == depth)
}

/// Moves bookmark `i`, with its children, as `id` says; false if it cannot go that way.
fn rearrange(items: &mut [OutlineItem], i: usize, id: &str) -> bool {
    let e = end(items, i);
    match id {
        "bookmark-up" => {
            let Some(p) = previous(items, i) else {
                return false;
            };
            items[p..e].rotate_left(i - p);
        }
        "bookmark-down" => {
            if items.get(e).is_none_or(|n| n.depth != items[i].depth) {
                return false;
            }
            let next_end = end(items, e);
            items[i..next_end].rotate_left(e - i);
        }
        "bookmark-indent" => {
            if previous(items, i).is_none() {
                return false;
            }
            items[i..e].iter_mut().for_each(|b| b.depth += 1);
        }
        "bookmark-outdent" => {
            let depth = items[i].depth;
            let Some(parent) = (0..i).rev().find(|&j| items[j].depth < depth) else {
                return false;
            };
            // After the parent's last child, a level up.
            let parent_end = end(items, parent);
            items[i..parent_end].rotate_left(e - i);
            let from = parent_end - (e - i);
            items[from..parent_end]
                .iter_mut()
                .for_each(|b| b.depth -= 1);
        }
        _ => return false,
    }
    true
}

fn bookmarks(app: &mut App, id: &str) {
    let Some((items, current)) = app.bookmarks() else {
        return;
    };
    let picked = app.content.bookmark.filter(|&i| i < items.len());
    if id == "bookmark-add" {
        let (Some((_, sizes)), Some((page, frac))) = (app.active_doc(), app.reading_fraction())
        else {
            return;
        };
        let after = picked.or(current);
        let title = app
            .selected_text()
            .and_then(|t| {
                t.lines()
                    .next()
                    .map(|l| l.trim().chars().take(80).collect())
            })
            .filter(|t: &String| !t.is_empty())
            .unwrap_or_else(|| format!("Page {}", page + 1));
        let form = Form::NewBookmark {
            at: after.map_or(items.len(), |i| end(&items, i)),
            depth: after.map_or(0, |i| items[i].depth),
            page,
            top: frac * sizes.get(page).map_or(0.0, |s| s.1),
        };
        app.show_form(
            form,
            "Add a bookmark",
            "",
            "Add",
            vec![text("Title", &title)],
        );
        return;
    }
    let Some(i) = picked else {
        app.status("Right-click a bookmark first".into());
        return;
    };
    let mut items = items;
    let done = match id {
        "bookmark-rename" => {
            let title = items[i].title.clone();
            app.show_form(
                Form::Bookmark(i),
                "Rename the bookmark",
                "",
                "Rename",
                vec![text("Title", &title)],
            );
            return;
        }
        "bookmark-delete" => {
            items.drain(i..end(&items, i));
            "Deleted the bookmark"
        }
        _ if rearrange(&mut items, i, id) => "Moved the bookmark",
        _ => {
            app.status("The bookmark cannot go that way".into());
            return;
        }
    };
    save(app, items, done);
}

/// The title typed for a new or renamed bookmark.
pub(crate) fn bookmark_named(app: &mut App, form: Form, f: &[FormField]) -> Done {
    let title = f[0].text.trim().to_owned();
    if title.is_empty() {
        return Err("Type a title for the bookmark.".into());
    }
    let Some((mut items, _)) = app.bookmarks() else {
        return Ok(());
    };
    match form {
        Form::NewBookmark {
            at,
            depth,
            page,
            top,
        } => {
            let item = OutlineItem {
                title,
                depth,
                target: Some(LinkTarget::Page {
                    page,
                    top: Some(top),
                }),
                source: None,
            };
            items.insert(at.min(items.len()), item);
            save(app, items, "Added a bookmark");
        }
        Form::Bookmark(i) if i < items.len() => {
            items[i].title = title;
            save(app, items, "Renamed the bookmark");
        }
        _ => {}
    }
    Ok(())
}

fn save(app: &mut App, items: Vec<OutlineItem>, done: &str) {
    let Some((doc, ..)) = app.reading() else {
        return;
    };
    match app.engine().set_outline(doc, items) {
        Ok(()) => {
            app.edited(None);
            app.status(done.into());
        }
        Err(e) => app.message("Could not change the bookmarks", e.to_string()),
    }
}
