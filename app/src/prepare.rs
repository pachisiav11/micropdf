//! Prepare Form: adds fields of every kind with a click or a dragged box, picks fields to move,
//! resize, delete or change, sets the order Tab takes through them, and adds fields where the
//! pages ask to be filled in.

use mp_engine::{DocId, FieldAction, FieldKind, FieldProps, NewField, Rect};

use crate::FormField;
use crate::content::{self, IMAGE};
use crate::tools::{Done, Form, check, choice, text};
use crate::viewer::{App, Drag, Tool};

const ACTIONS: [&str; 5] = [
    "Nothing",
    "Open a web address",
    "Reset the form",
    "Submit the form to a web address",
    "Run JavaScript",
];

#[derive(Default)]
pub struct Prepare {
    /// The kind of field a click or a box adds; with none, a click only picks fields.
    adding: Option<NewField>,
    /// The radio button that new radio buttons on its page join.
    group: Option<(DocId, usize, i32)>,
}

/// Prepare Form command `id`; false if it is not one.
pub fn command(app: &mut App, id: &str) -> bool {
    let kind = match id {
        "field-text" => NewField::Text,
        "field-checkbox" => NewField::Checkbox,
        "field-radio" => NewField::Radio,
        "field-dropdown" => NewField::Dropdown,
        "field-list" => NewField::List,
        "field-button" => NewField::Button,
        "field-signature" => NewField::Signature,
        "tool-prepare" => {
            app.set_tool(Tool::Field);
            app.prepare.adding = None;
            app.status(
                "Click a field to pick it; drag it, or a corner to resize it. Double-click for \
                 its properties"
                    .into(),
            );
            return true;
        }
        "field-properties" => {
            properties(app);
            return true;
        }
        "field-detect" => {
            detect(app);
            return true;
        }
        "field-order-rows" | "field-order-columns" => {
            order(app, id == "field-order-columns");
            return true;
        }
        _ => return false,
    };
    app.set_tool(Tool::Field);
    app.prepare.adding = Some(kind);
    if kind == NewField::Radio {
        app.prepare.group = None;
    }
    app.status("Click where the field goes, or drag a box for it".into());
    true
}

/// Starts a drag with Prepare Form at page point `p`, document point `at`: a corner of the
/// picked field resizes it, a field moves, and elsewhere a box is drawn for a new field.
pub(crate) fn down(app: &mut App, page: usize, p: (f32, f32), at: (f32, f32)) -> Option<Drag> {
    let (doc, ..) = app.reading()?;
    if let Some(drag) = content::corner(app, doc, page, p, at) {
        return Some(drag);
    }
    let fields = app.engine().fields(doc, page).unwrap_or_default();
    if let Some(f) = fields.into_iter().rev().find(|f| f.rect.contains(p.0, p.1)) {
        pick(app, doc, page, f.id, f.rect);
        return Some(content::shape(page, f.id, at, p, f.rect, false));
    }
    if app.content.picked.take().is_some() {
        app.refresh_marks();
    }
    if app.prepare.adding.is_none() {
        app.status("There is no field there. Add fields from the Tools menu".into());
        return None;
    }
    Some(Drag::Draw {
        page,
        points: vec![p],
    })
}

fn pick(app: &mut App, doc: DocId, page: usize, id: i32, rect: Rect) {
    app.content.picked = Some((doc, page, id, rect));
    app.refresh_marks();
    app.status(
        "Field picked. Drag it, or a corner to resize it; double-click for its properties, \
         Delete removes it"
            .into(),
    );
}

/// A box drawn with Prepare Form; a click puts the field there at its usual size.
pub fn drawn(app: &mut App, page: usize, rect: Rect, click: bool) {
    let (Some(kind), Some((doc, ..))) = (app.prepare.adding, app.reading()) else {
        return;
    };
    let rect = if click {
        let (w, h) = match kind {
            NewField::Text | NewField::Dropdown => (150.0, 22.0),
            NewField::Checkbox | NewField::Radio => (14.0, 14.0),
            NewField::List => (150.0, 60.0),
            NewField::Button => (80.0, 24.0),
            NewField::Signature => (180.0, 50.0),
        };
        Rect {
            x1: rect.x0 + w,
            y1: rect.y0 + h,
            ..rect
        }
    } else {
        rect
    };
    let group = app
        .prepare
        .group
        .filter(|g| kind == NewField::Radio && (g.0, g.1) == (doc, page))
        .map(|g| g.2);
    match app.engine().add_field(doc, page, kind, rect, group) {
        Ok(id) => {
            if kind == NewField::Radio {
                app.prepare.group = Some((doc, page, id));
            }
            app.edited(Some(page));
            pick(app, doc, page, id, rect);
            app.status("Added the field. Double-click it for its properties".into());
        }
        Err(e) => app.message("Could not add the field", e.to_string()),
    }
}

/// The picked field was dragged to `to`.
pub fn moved(app: &mut App, page: usize, id: i32, to: Rect) {
    let Some((doc, ..)) = app.reading() else {
        return;
    };
    match app.engine().move_field(doc, page, id, to) {
        Ok(()) => {
            app.content.picked = Some((doc, page, id, to));
            app.edited(Some(page));
            app.status("Moved the field".into());
        }
        Err(e) => {
            app.refresh_marks();
            app.message("Could not move the field", e.to_string());
        }
    }
}

pub(crate) fn delete(app: &mut App, doc: DocId, page: usize, id: i32) {
    app.content.picked = None;
    match app.engine().delete_field(doc, page, id) {
        Ok(()) => {
            app.edited(Some(page));
            app.status("Deleted the field".into());
        }
        Err(e) => {
            app.refresh_marks();
            app.message("Could not delete the field", e.to_string());
        }
    }
}

/// Opens the picked field's properties.
pub fn properties(app: &mut App) {
    let picked = app.content.picked.filter(|p| p.2 != IMAGE);
    let (Some((doc, page, id, _)), Some((active, ..))) = (picked, app.reading()) else {
        app.status("Pick a field with Prepare form first".into());
        return;
    };
    if doc != active {
        return;
    }
    let p = match app.engine().field_props(doc, page, id) {
        Ok(p) => p,
        Err(e) => return app.message("Could not read the field", e.to_string()),
    };
    let mut f = vec![
        text("Name", &p.name),
        text("Tooltip", &p.tooltip),
        check("Required", p.required),
        check("Read only", p.read_only),
    ];
    match p.kind {
        FieldKind::Text => f.extend([
            check("Several lines", p.multiline),
            text("Calculated by (JavaScript)", &p.calculate),
        ]),
        FieldKind::Choice => f.extend([
            text("Options, split by ;", &p.options.join("; ")),
            check("List box", p.list),
        ]),
        FieldKind::Checkbox | FieldKind::Radio => f.push(text("Value when on", &p.export)),
        FieldKind::Button => f.push(text("Label", &p.label)),
        _ => {}
    }
    let (index, value) = match &p.action {
        FieldAction::None => (0, ""),
        FieldAction::Uri(u) => (1, u.as_str()),
        FieldAction::Reset => (2, ""),
        FieldAction::Submit(u) => (3, u.as_str()),
        FieldAction::Script(s) => (4, s.as_str()),
    };
    f.extend([
        choice("When clicked", &ACTIONS, index),
        text("Web address or script", value),
    ]);
    app.show_form(
        Form::FieldProps { page, id },
        "Field properties",
        "",
        "Save",
        f,
    );
}

pub(crate) fn set_props(app: &mut App, page: usize, id: i32, f: &[FormField]) -> Done {
    let Some((doc, ..)) = app.reading() else {
        return Ok(());
    };
    let engine = app.engine();
    let mut p: FieldProps = engine.field_props(doc, page, id)?;
    let trim = |i: usize| f[i].text.trim().to_owned();
    p.name = trim(0);
    if p.name.is_empty() {
        return Err("Type a name for the field.".into());
    }
    p.tooltip = trim(1);
    p.required = f[2].checked;
    p.read_only = f[3].checked;
    let at = match p.kind {
        FieldKind::Text => {
            p.multiline = f[4].checked;
            p.calculate = trim(5);
            6
        }
        FieldKind::Choice => {
            p.options = f[4]
                .text
                .split(';')
                .map(str::trim)
                .filter(|o| !o.is_empty())
                .map(Into::into)
                .collect();
            p.list = f[5].checked;
            6
        }
        FieldKind::Checkbox | FieldKind::Radio => {
            p.export = trim(4);
            if p.export.is_empty() || p.export == "Off" {
                return Err("Type the value the field has when on.".into());
            }
            5
        }
        FieldKind::Button => {
            p.label = trim(4);
            5
        }
        _ => 4,
    };
    let value = trim(at + 1);
    p.action = match f[at].index {
        0 => FieldAction::None,
        2 => FieldAction::Reset,
        _ if value.is_empty() => return Err("Type the web address or the script.".into()),
        1 => FieldAction::Uri(value),
        3 => FieldAction::Submit(value),
        _ => FieldAction::Script(value),
    };
    engine.set_field_props(doc, page, id, p)?;
    app.edited(Some(page));
    app.status("Changed the field".into());
    Ok(())
}

fn detect(app: &mut App) {
    let Some((doc, _, _, count)) = app.reading() else {
        return;
    };
    app.set_tool(Tool::Field);
    app.prepare.adding = None;
    match app.engine().detect_fields(doc, (0..count).collect()) {
        Ok(0) => app.status("Found no blanks to make fields of".into()),
        Ok(n) => {
            app.edited(None);
            let s = if n == 1 { "" } else { "s" };
            app.status(format!("Added {n} field{s}. Click one to change it"));
        }
        Err(e) => app.message("Could not find fields", e.to_string()),
    }
}

fn order(app: &mut App, columns: bool) {
    let Some((doc, _, _, count)) = app.reading() else {
        return;
    };
    match app
        .engine()
        .order_fields(doc, (0..count).collect(), columns)
    {
        Ok(()) => {
            app.edited(None);
            app.status(
                if columns {
                    "Tab now goes down each column of fields"
                } else {
                    "Tab now goes along each row of fields"
                }
                .into(),
            );
        }
        Err(e) => app.message("Could not order the fields", e.to_string()),
    }
}
