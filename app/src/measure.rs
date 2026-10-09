//! The Measure tools: a distance dragged out, or a perimeter or area clicked out point by
//! point, at the scale the user set, else the one the page gives, else 1 in = 1 in.

use mp_engine::{Measure, NewAnnot, PAGE_UNITS, REAL_UNITS, Scale};

use crate::FormField;
use crate::tools::{Done, Form, choice, number, text};
use crate::viewer::{App, Tool};

const RED: [f32; 3] = [0.85, 0.15, 0.15];

#[derive(Default)]
pub struct Measuring {
    pub(crate) kind: Measure,
    /// The scale the user set, for every page.
    scale: Option<Scale>,
    /// The page and points of a perimeter or area being clicked out.
    pub(crate) outline: Option<(usize, Vec<(f32, f32)>)>,
}

/// Measure command `id`; false if it is not one.
pub fn command(app: &mut App, id: &str) -> bool {
    let kind = match id {
        "measure-distance" => Measure::Distance,
        "measure-perimeter" => Measure::Perimeter,
        "measure-area" => Measure::Area,
        "measure-scale" => {
            open_scale(app);
            return true;
        }
        _ => return false,
    };
    app.measuring.kind = kind;
    app.set_tool(Tool::Measure);
    if let Some(w) = app.window() {
        w.set_measure_kind(kind as i32);
    }
    app.status(
        match kind {
            Measure::Distance => "Drag from one point to the other to measure the distance",
            Measure::Perimeter => {
                "Click each point of the path; double-click the last one or press Enter"
            }
            Measure::Area => {
                "Click each corner of the area; double-click the last one or press Enter"
            }
        }
        .into(),
    );
    true
}

fn what(kind: Measure) -> &'static str {
    match kind {
        Measure::Distance => "Distance",
        Measure::Perimeter => "Perimeter",
        Measure::Area => "Area",
    }
}

/// The scale to measure on `page` at.
fn scale(app: &App, page: usize) -> Scale {
    if let Some(s) = &app.measuring.scale {
        return s.clone();
    }
    app.reading()
        .and_then(|(doc, ..)| app.engine().page_scale(doc, page).ok().flatten())
        .unwrap_or_default()
}

/// Shows the measure of `points` on `page` while they are being placed.
pub fn live(app: &mut App, page: usize, points: &[(f32, f32)]) {
    let kind = app.measuring.kind;
    let label = scale(app, page).label(kind, points);
    app.status(format!("{}: {label}", what(kind)));
}

/// Adds the measurement of `points` on `page` to the document.
pub fn finish(app: &mut App, page: usize, points: Vec<(f32, f32)>) {
    let kind = app.measuring.kind;
    let scale = scale(app, page);
    let label = scale.label(kind, &points);
    let new = NewAnnot::Measure {
        kind,
        points,
        scale,
    };
    if app.add_comment(page, new, RED) {
        app.status(format!("{}: {label}", what(kind)));
    }
}

/// A click with the perimeter or area tool: the next point of the outline.
pub fn click(app: &mut App, page: usize, point: (f32, f32)) {
    match &mut app.measuring.outline {
        Some((p, points)) if *p == page => {
            // A double-click's second press lands on the point the first one placed.
            if points
                .last()
                .is_some_and(|l| (l.0 - point.0).hypot(l.1 - point.1) < 2.0)
            {
                return;
            }
            points.push(point);
        }
        outline => *outline = Some((page, vec![point])),
    }
    draw(app, None);
}

/// The pointer moved while an outline is being clicked out.
pub fn hover(app: &mut App, x: f32, y: f32) {
    let Some(page) = app.measuring.outline.as_ref().map(|o| o.0) else {
        return;
    };
    let point = app.page_point(page, x, y);
    draw(app, point);
}

/// Draws the outline so far, on to `pointer` when given, and says what it measures.
fn draw(app: &mut App, pointer: Option<(f32, f32)>) {
    let Some((page, mut points)) = app.measuring.outline.clone() else {
        return;
    };
    points.extend(pointer);
    let mut path = String::new();
    for (i, &p) in points.iter().enumerate() {
        let Some((x, y)) = app.view_point(page, p) else {
            return;
        };
        path += &format!("{} {x} {y} ", if i == 0 { "M" } else { "L" });
    }
    let area = app.measuring.kind == Measure::Area;
    if area && points.len() > 2 {
        path += "Z";
    }
    if let Some(w) = app.window() {
        w.set_draft_path(path.into());
    }
    if points.len() > 1 {
        live(app, page, &points);
    }
}

/// Measures the outline clicked out so far; false if none is.
pub fn finish_outline(app: &mut App) -> bool {
    let Some((page, points)) = app.measuring.outline.take() else {
        return false;
    };
    if let Some(w) = app.window() {
        w.set_draft_path("".into());
    }
    let least = if app.measuring.kind == Measure::Area {
        3
    } else {
        2
    };
    if points.len() < least {
        app.status(format!("Click at least {least} points to measure"));
    } else {
        finish(app, page, points);
    }
    true
}

/// Drops the outline being clicked out; false if none is.
pub fn cancel(app: &mut App) -> bool {
    if app.measuring.outline.take().is_none() {
        return false;
    }
    if let Some(w) = app.window() {
        w.set_draft_path("".into());
    }
    true
}

fn open_scale(app: &mut App) {
    let current = app.measuring.scale.as_ref().map(|s| s.ratio.clone());
    let unit = app
        .measuring
        .scale
        .as_ref()
        .map_or("in", |s| s.unit.as_str());
    let index = REAL_UNITS.iter().position(|&u| u == unit).unwrap_or(0) as i32;
    let page_units: Vec<&str> = PAGE_UNITS.iter().map(|u| u.0).collect();
    let note = format!(
        "Measurements on every page use this scale. {}",
        match current {
            Some(ratio) => format!("It is now {ratio}."),
            None => "Until you set one, they use the scale the page gives, or 1 in = 1 in.".into(),
        }
    );
    app.show_form(
        Form::Scale,
        "Set the scale",
        &note,
        "Set",
        vec![
            text("Length on the page", "1"),
            choice("Unit on the page", &page_units, 0),
            text("Stands for", "1"),
            choice("Unit", &REAL_UNITS, index),
        ],
    );
}

pub(crate) fn set_scale(app: &mut App, f: &[FormField]) -> Done {
    let page_unit = PAGE_UNITS
        .get(f[1].index.max(0) as usize)
        .map_or("in", |u| u.0);
    let unit = REAL_UNITS
        .get(f[3].index.max(0) as usize)
        .copied()
        .unwrap_or("in");
    let scale = Scale::new(number(&f[0])?, page_unit, number(&f[2])?, unit)
        .ok_or("Both lengths must be more than 0.")?;
    app.status(format!("Measuring at {}", scale.ratio));
    app.measuring.scale = Some(scale);
    Ok(())
}
