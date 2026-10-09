//! Editing page content: Edit text opens a block of text in the editor that form fields use,
//! and sets what was typed in its place.

use mp_engine::{DocId, Rect};

use crate::viewer::{App, Tool};

/// The block of text open in the editor.
pub struct TextEdit {
    doc: DocId,
    page: usize,
    rect: Rect,
    size: f32,
    original: String,
}

/// Content command `id`; false if it is not one.
pub fn command(app: &mut App, id: &str) -> bool {
    match id {
        "tool-edit-text" => {
            app.set_tool(Tool::EditText);
            app.status("Click a block of text to edit it".into());
        }
        _ => return false,
    }
    true
}

/// A click with Edit text at `(x, y)` on `page`: opens the block of text there.
pub fn click(app: &mut App, page: usize, (x, y): (f32, f32)) {
    let Some((doc, ..)) = app.reading() else {
        return;
    };
    let blocks = app.engine().text_blocks(doc, page).unwrap_or_default();
    let near = |r: &Rect| x >= r.x0 - 2.0 && x <= r.x1 + 2.0 && y >= r.y0 - 2.0 && y <= r.y1 + 2.0;
    let Some(block) = blocks.into_iter().find(|b| near(&b.rect)) else {
        app.status("There is no text there to edit".into());
        return;
    };
    app.text_edit = Some(TextEdit {
        doc,
        page,
        rect: block.rect,
        size: block.size,
        original: block.text.clone(),
    });
    if let Some(w) = app.window() {
        w.set_field_text(block.text.into());
        w.set_field_name("Text".into());
        w.set_field_multiline(true);
    }
    place(app);
    if let Some(w) = app.window() {
        w.invoke_focus_field_editor();
    }
}

/// Puts the editor over the open block, a little roomier than it, for typing.
pub fn place(app: &App) {
    let (Some(w), Some(e)) = (app.window(), app.text_edit.as_ref()) else {
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

/// Sets the text typed in place of the block.
pub fn commit(app: &mut App) {
    let Some(e) = app.text_edit.take() else {
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
    match app.engine().replace_text(e.doc, e.page, e.rect, text) {
        Ok(r) => {
            app.edited(Some(e.page));
            app.status(if r.own || r.font.is_empty() {
                "Edited the text".into()
            } else {
                format!(
                    "Edited the text, set in {}: the document's font lacks some of these letters",
                    r.font
                )
            });
        }
        Err(err) => app.message("Could not edit the text", err.to_string()),
    }
}

/// Closes the editor and drops what was typed; false if it was not open.
pub fn cancel(app: &mut App) -> bool {
    if app.text_edit.take().is_none() {
        return false;
    }
    if let Some(w) = app.window() {
        w.set_field_editing(false);
        w.invoke_focus_view();
    }
    true
}
