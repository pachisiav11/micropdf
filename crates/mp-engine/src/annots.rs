//! Annotations: list, add and delete markup on PDF pages, and save the result.

use std::path::Path;

use mupdf::color::AnnotationColor;
use mupdf::pdf::{
    AnnotationQuadPoints, PdfAnnotation, PdfAnnotationType, PdfDocument, PdfPage, PdfWriteOptions,
};
use mupdf::{Document, Point, Quad};

use crate::{Error, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotKind {
    Highlight,
    Underline,
    StrikeOut,
    Squiggly,
    Note,
    FreeText,
    Ink,
    Square,
    Circle,
    Line,
    Stamp,
    Other,
}

/// An annotation as the comment list shows it. `id` is its object number, stable while the
/// document is open.
#[derive(Debug, Clone, PartialEq)]
pub struct Annot {
    pub id: i32,
    pub kind: AnnotKind,
    pub rect: Rect,
    pub color: Option<[f32; 3]>,
    pub contents: String,
    pub author: String,
}

/// What to add. Coordinates are page space (points, origin top-left).
#[derive(Debug, Clone, PartialEq)]
pub enum NewAnnot {
    /// Highlight, underline, strike-out or squiggly over text, one rectangle per line.
    TextMarkup {
        kind: AnnotKind,
        rects: Vec<Rect>,
    },
    Note {
        x: f32,
        y: f32,
        text: String,
    },
    FreeText {
        rect: Rect,
        text: String,
    },
    Ink {
        strokes: Vec<Vec<(f32, f32)>>,
        width: f32,
    },
    /// A rectangle or ellipse.
    Shape {
        kind: AnnotKind,
        rect: Rect,
        width: f32,
    },
    Line {
        from: (f32, f32),
        to: (f32, f32),
        width: f32,
    },
}

/// Colour and author applied to a new annotation.
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub color: [f32; 3],
    pub author: String,
}

fn kind_of(t: PdfAnnotationType) -> Option<AnnotKind> {
    Some(match t {
        PdfAnnotationType::Highlight => AnnotKind::Highlight,
        PdfAnnotationType::Underline => AnnotKind::Underline,
        PdfAnnotationType::StrikeOut => AnnotKind::StrikeOut,
        PdfAnnotationType::Squiggly => AnnotKind::Squiggly,
        PdfAnnotationType::Text => AnnotKind::Note,
        PdfAnnotationType::FreeText => AnnotKind::FreeText,
        PdfAnnotationType::Ink => AnnotKind::Ink,
        PdfAnnotationType::Square => AnnotKind::Square,
        PdfAnnotationType::Circle => AnnotKind::Circle,
        PdfAnnotationType::Line => AnnotKind::Line,
        PdfAnnotationType::Stamp => AnnotKind::Stamp,
        // Links, form widgets and popups have their own panels or none.
        PdfAnnotationType::Link | PdfAnnotationType::Widget | PdfAnnotationType::Popup => {
            return None;
        }
        _ => AnnotKind::Other,
    })
}

fn markup_type(kind: AnnotKind) -> Result<PdfAnnotationType, Error> {
    match kind {
        AnnotKind::Highlight => Ok(PdfAnnotationType::Highlight),
        AnnotKind::Underline => Ok(PdfAnnotationType::Underline),
        AnnotKind::StrikeOut => Ok(PdfAnnotationType::StrikeOut),
        AnnotKind::Squiggly => Ok(PdfAnnotationType::Squiggly),
        _ => Err(Error::Invalid("not a text markup kind")),
    }
}

fn pdf_page(doc: &Document, page: usize) -> Result<PdfPage, Error> {
    let page = doc.load_page(page as i32)?;
    PdfPage::try_from(page).map_err(|_| Error::NotPdf)
}

fn rgb(color: AnnotationColor) -> [f32; 3] {
    match color {
        AnnotationColor::Gray(g) => [g; 3],
        AnnotationColor::Rgb { red, green, blue } => [red, green, blue],
        AnnotationColor::Cmyk {
            cyan,
            magenta,
            yellow,
            key,
        } => [
            (1.0 - cyan) * (1.0 - key),
            (1.0 - magenta) * (1.0 - key),
            (1.0 - yellow) * (1.0 - key),
        ],
    }
}

pub(crate) fn describe(annot: &PdfAnnotation) -> Result<Option<Annot>, Error> {
    let Some(kind) = kind_of(annot.r#type()?) else {
        return Ok(None);
    };
    Ok(Some(Annot {
        id: annot.xref()?,
        kind,
        rect: annot.bounds()?.into(),
        color: annot.color()?.map(rgb),
        contents: annot.contents()?.unwrap_or_default().to_owned(),
        author: annot.author()?.unwrap_or_default().to_owned(),
    }))
}

/// The page's annotations, in drawing order. Empty for documents that are not PDF.
pub fn list(doc: &Document, page: usize) -> Result<Vec<Annot>, Error> {
    let Ok(page) = pdf_page(doc, page) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for annot in page.annotations() {
        out.extend(describe(&annot)?);
    }
    Ok(out)
}

fn point((x, y): (f32, f32)) -> Point {
    Point::new(x, y)
}

/// Runs `f` as one undoable step named `name`; a failed step is rolled back.
pub(crate) fn operation<T>(
    doc: &Document,
    name: &str,
    f: impl FnOnce() -> Result<T, Error>,
) -> Result<T, Error> {
    let pdf = PdfDocument::try_from(doc.clone()).map_err(|_| Error::NotPdf)?;
    pdf.begin_operation(name)?;
    match f() {
        Ok(value) => {
            pdf.end_operation()?;
            Ok(value)
        }
        Err(e) => {
            let _ = pdf.abandon_operation();
            Err(e)
        }
    }
}

fn label(new: &NewAnnot) -> &'static str {
    match new {
        NewAnnot::TextMarkup {
            kind: AnnotKind::Highlight,
            ..
        } => "Highlight",
        NewAnnot::TextMarkup {
            kind: AnnotKind::Underline,
            ..
        } => "Underline",
        NewAnnot::TextMarkup {
            kind: AnnotKind::StrikeOut,
            ..
        } => "Strike out",
        NewAnnot::TextMarkup { .. } => "Squiggly underline",
        NewAnnot::Note { .. } => "Add note",
        NewAnnot::FreeText { .. } => "Add text box",
        NewAnnot::Ink { .. } => "Draw",
        NewAnnot::Shape {
            kind: AnnotKind::Circle,
            ..
        } => "Add ellipse",
        NewAnnot::Shape { .. } => "Add rectangle",
        NewAnnot::Line { .. } => "Add line",
    }
}

pub fn add(doc: &Document, page: usize, new: &NewAnnot, style: &Style) -> Result<Annot, Error> {
    operation(doc, label(new), || add_now(doc, page, new, style))
}

fn add_now(doc: &Document, page: usize, new: &NewAnnot, style: &Style) -> Result<Annot, Error> {
    let mut page = pdf_page(doc, page)?;
    let subtype = match new {
        NewAnnot::TextMarkup { kind, .. } => markup_type(*kind)?,
        NewAnnot::Note { .. } => PdfAnnotationType::Text,
        NewAnnot::FreeText { .. } => PdfAnnotationType::FreeText,
        NewAnnot::Ink { .. } => PdfAnnotationType::Ink,
        NewAnnot::Shape {
            kind: AnnotKind::Circle,
            ..
        } => PdfAnnotationType::Circle,
        NewAnnot::Shape {
            kind: AnnotKind::Square,
            ..
        } => PdfAnnotationType::Square,
        NewAnnot::Shape { .. } => return Err(Error::Invalid("not a shape kind")),
        NewAnnot::Line { .. } => PdfAnnotationType::Line,
    };
    let mut annot = page.create_annotation(subtype)?;
    let to_rect = |r: &Rect| mupdf::Rect::new(r.x0, r.y0, r.x1, r.y1);
    match new {
        NewAnnot::TextMarkup { rects, .. } => {
            let quads = rects.iter().map(|r| Quad {
                ul: Point::new(r.x0, r.y0),
                ur: Point::new(r.x1, r.y0),
                ll: Point::new(r.x0, r.y1),
                lr: Point::new(r.x1, r.y1),
            });
            annot.set_quad_points(AnnotationQuadPoints::new(quads))?;
        }
        NewAnnot::Note { x, y, text } => {
            annot.set_rect(mupdf::Rect::new(*x, *y, x + 20.0, y + 20.0))?;
            annot.set_contents(text)?;
        }
        NewAnnot::FreeText { rect, text } => {
            annot.set_rect(to_rect(rect))?;
            annot.set_contents(text)?;
        }
        NewAnnot::Ink { strokes, width } => {
            annot.set_border_width(*width)?;
            annot.set_ink_list(strokes.iter().map(|s| s.iter().copied().map(point)))?;
        }
        NewAnnot::Shape { rect, width, .. } => {
            annot.set_rect(to_rect(rect))?;
            annot.set_border_width(*width)?;
        }
        NewAnnot::Line { from, to, width } => {
            annot.set_line(point(*from), point(*to))?;
            annot.set_border_width(*width)?;
        }
    }
    let [red, green, blue] = style.color;
    annot.set_color(AnnotationColor::Rgb { red, green, blue })?;
    if !style.author.is_empty() {
        annot.set_author(&style.author)?;
    }
    page.update()?;
    describe(&annot)?.ok_or(Error::Invalid("annotation type has no entry"))
}

pub fn delete(doc: &Document, page: usize, id: i32) -> Result<(), Error> {
    operation(doc, "Delete comment", || {
        let mut page = pdf_page(doc, page)?;
        let annot = page
            .annotations()
            .find(|a| a.xref().ok() == Some(id))
            .ok_or(Error::NotFound)?;
        page.delete_annotation(annot)?;
        page.update()?;
        Ok(())
    })
}

/// Replaces the text of a comment.
pub fn set_contents(doc: &Document, page: usize, id: i32, text: &str) -> Result<(), Error> {
    operation(doc, "Edit comment", || {
        let mut page = pdf_page(doc, page)?;
        let mut annot = page
            .annotations()
            .find(|a| a.xref().ok() == Some(id))
            .ok_or(Error::NotFound)?;
        annot.set_contents(text)?;
        page.update()?;
        Ok(())
    })
}

pub fn set_color(doc: &Document, page: usize, id: i32, color: [f32; 3]) -> Result<(), Error> {
    operation(doc, "Change colour", || {
        let mut page = pdf_page(doc, page)?;
        let mut annot = page
            .annotations()
            .find(|a| a.xref().ok() == Some(id))
            .ok_or(Error::NotFound)?;
        let [red, green, blue] = color;
        annot.set_color(AnnotationColor::Rgb { red, green, blue })?;
        page.update()?;
        Ok(())
    })
}

/// Draws comments, form fields or both into the page content, where they can no longer change.
pub fn flatten(doc: &Document, comments: bool, fields: bool) -> Result<(), Error> {
    let name = match (comments, fields) {
        (true, false) => "Flatten comments",
        (false, true) => "Flatten form fields",
        _ => "Flatten",
    };
    operation(doc, name, || {
        let mut pdf = PdfDocument::try_from(doc.clone()).map_err(|_| Error::NotPdf)?;
        Ok(pdf.bake(comments, fields)?)
    })
}

/// The names of the steps Undo and Redo would take back or redo, if any.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    pub undo: Option<String>,
    pub redo: Option<String>,
    /// How many steps are applied; equal positions mean equal content unless a new edit
    /// replaced steps that had been undone.
    pub position: usize,
}

/// Turns on undo recording. Every later edit must go through [`operation`].
pub(crate) fn enable_journal(doc: &Document) -> Result<(), Error> {
    match PdfDocument::try_from(doc.clone()) {
        Ok(pdf) => Ok(pdf.enable_journal()?),
        Err(_) => Ok(()),
    }
}

pub fn history(doc: &Document) -> Result<History, Error> {
    let Ok(pdf) = PdfDocument::try_from(doc.clone()) else {
        return Ok(History::default());
    };
    let (current, steps) = pdf.undo_redo_state()?;
    let name = |step: i32| -> Result<Option<String>, Error> {
        Ok(Some(pdf.undo_redo_step(step)?.unwrap_or_default()))
    };
    Ok(History {
        undo: if current > 0 {
            name(current - 1)?
        } else {
            None
        },
        redo: if current < steps {
            name(current)?
        } else {
            None
        },
        position: current.max(0) as usize,
    })
}

pub fn undo(doc: &Document) -> Result<(), Error> {
    let pdf = PdfDocument::try_from(doc.clone()).map_err(|_| Error::NotPdf)?;
    Ok(pdf.undo()?)
}

pub fn redo(doc: &Document) -> Result<(), Error> {
    let pdf = PdfDocument::try_from(doc.clone()).map_err(|_| Error::NotPdf)?;
    Ok(pdf.redo()?)
}

/// Writes the document to `target`. Incremental saves append the changes to a copy of
/// `original`, which keeps existing signatures valid; full saves rewrite and compact it.
pub fn save(
    doc: &Document,
    original: &Path,
    target: &Path,
    incremental: bool,
) -> Result<(), Error> {
    let pdf = PdfDocument::try_from(doc.clone()).map_err(|_| Error::NotPdf)?;
    let target_str = target.to_str().ok_or(Error::Invalid("path is not UTF-8"))?;
    let mut options = PdfWriteOptions::default();
    if incremental && pdf.can_be_saved_incrementally() {
        // MuPDF appends to the file at `target`, so it must start as the original bytes.
        if target != original {
            std::fs::copy(original, target)?;
        }
        options.set_incremental(true);
    } else {
        options.set_garbage_level(1).set_compress(true);
    }
    pdf.save_with_options(target_str, options)?;
    Ok(())
}
