//! Annotations: list, add and delete markup on PDF pages, and save the result.

use std::path::Path;

use mupdf::color::AnnotationColor;
use mupdf::pdf::{
    AnnotationBorderEffect, AnnotationBorderStyle, AnnotationFlags, AnnotationQuadPoints,
    EmbeddedFileOptions, LineEndingStyle, PdfAnnotation, PdfAnnotationType, PdfDocument, PdfObject,
    PdfPage,
};
use mupdf::{Document, Point, Quad};

use crate::measure::{Measure, Scale};
use crate::render::{self, Preview};
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
    /// A text box with a line pointing at something (a FreeText annotation of intent
    /// FreeTextCallout).
    Callout,
    /// A file attached to the page (a FileAttachment annotation).
    File,
    /// Marked for redaction (a Redact annotation), until redactions are applied.
    Redact,
    /// A distance, perimeter or area: a Line, PolyLine or Polygon with a Measure dictionary.
    Measure,
    Other,
}

impl AnnotKind {
    /// What readers call this kind of comment, such as "Text box".
    pub fn name(self) -> &'static str {
        match self {
            AnnotKind::Highlight => "Highlight",
            AnnotKind::Underline => "Underline",
            AnnotKind::StrikeOut => "Strike-out",
            AnnotKind::Squiggly => "Squiggly",
            AnnotKind::Note => "Note",
            AnnotKind::FreeText => "Text box",
            AnnotKind::Ink => "Drawing",
            AnnotKind::Square => "Rectangle",
            AnnotKind::Circle => "Ellipse",
            AnnotKind::Line => "Line",
            AnnotKind::Stamp => "Stamp",
            AnnotKind::Callout => "Callout",
            AnnotKind::File => "Attachment",
            AnnotKind::Redact => "Redaction",
            AnnotKind::Measure => "Measurement",
            AnnotKind::Other => "Comment",
        }
    }
}

/// A PDF date ("D:20261008165500+05'30'") as "2026-10-08 16:55".
pub fn readable_date(s: &str) -> Option<String> {
    let digits: String = s
        .strip_prefix("D:")
        .unwrap_or(s)
        .chars()
        .take_while(char::is_ascii_digit)
        .take(12)
        .collect();
    if digits.len() < 8 {
        return None;
    }
    let mut out = format!("{}-{}-{}", &digits[..4], &digits[4..6], &digits[6..8]);
    if digits.len() == 12 {
        out += &format!(" {}:{}", &digits[8..10], &digits[10..12]);
    }
    Some(out)
}

/// An annotation as the comment list shows it. `id` is its object number, stable while the
/// document is open.
#[derive(Debug, Clone, PartialEq)]
pub struct Annot {
    pub id: i32,
    pub kind: AnnotKind,
    pub rect: Rect,
    /// The colour it is drawn in; for text boxes and callouts, the text's colour.
    pub color: Option<[f32; 3]>,
    /// The inside of a rectangle or ellipse, or the background of a text box or callout, when
    /// filled.
    pub fill: Option<[f32; 3]>,
    /// 1.0 is opaque.
    pub opacity: f32,
    /// Line width in points, for the kinds drawn with a line: shapes, lines and ink.
    pub width: Option<f32>,
    pub contents: String,
    pub author: String,
    /// The comment this one answers, by id: set for replies and for review states. Readers
    /// show these only in the comment thread, never on the page.
    pub reply_to: Option<i32>,
    /// The review state this annotation sets on `reply_to` ("Accepted", "Rejected",
    /// "Cancelled", "Completed" or "None"), or "Marked"/"Unmarked" for a private check mark.
    pub state: Option<String>,
    /// When it last changed, as a PDF date ("D:20261008165500+05'30'"), or empty.
    pub modified: String,
    /// How the outline is drawn, for rectangles, ellipses and lines.
    pub border: Option<Border>,
    /// A line's start and end, as PDF names from [`LINE_ENDS`].
    pub line_ends: Option<(String, String)>,
    /// A text box's or callout's text size in points.
    pub font_size: Option<f32>,
    pub subject: String,
    /// Readers keep a locked comment from being moved, resized, restyled or deleted.
    pub locked: bool,
    /// Whether it appears when the page is printed.
    pub printed: bool,
}

/// How a rectangle, ellipse or line outline is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Border {
    Solid,
    Dashed,
    /// Scalloped like a cloud; rectangles and ellipses only.
    Cloudy,
}

/// The line ends readers draw, as PDF names, and what to call them.
pub const LINE_ENDS: [(&str, &str); 8] = [
    ("None", "None"),
    ("OpenArrow", "Open arrow"),
    ("ClosedArrow", "Closed arrow"),
    ("Circle", "Circle"),
    ("Square", "Square"),
    ("Diamond", "Diamond"),
    ("Butt", "Bar"),
    ("Slash", "Slash"),
];

fn line_end(name: &str) -> Option<LineEndingStyle> {
    Some(match name {
        "None" => LineEndingStyle::None,
        "OpenArrow" => LineEndingStyle::OpenArrow,
        "ClosedArrow" => LineEndingStyle::ClosedArrow,
        "Circle" => LineEndingStyle::Circle,
        "Square" => LineEndingStyle::Square,
        "Diamond" => LineEndingStyle::Diamond,
        "Butt" => LineEndingStyle::Butt,
        "Slash" => LineEndingStyle::Slash,
        _ => return None,
    })
}

/// Who wrote a comment and what it is about, whether it is locked and whether it prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Properties {
    pub author: String,
    pub subject: String,
    pub locked: bool,
    pub printed: bool,
}

/// The review states a comment can be given, as PDF names.
pub const REVIEW_STATES: [&str; 5] = ["None", "Accepted", "Rejected", "Cancelled", "Completed"];

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
    /// A text box at `rect` with a line from the nearest point of its edge to `target`.
    Callout {
        target: (f32, f32),
        rect: Rect,
        text: String,
    },
    /// A rubber stamp, one of [`STAMPS`] by name, centred on `center`, `width` points wide.
    /// MuPDF draws it, so other readers redraw it the same way from the name.
    Stamp {
        name: String,
        center: (f32, f32),
        width: f32,
    },
    /// `data` embedded as the file `name`, shown as a paperclip with its top left at `at`.
    File {
        at: (f32, f32),
        name: String,
        data: Vec<u8>,
    },
    /// The distance between two points, the length of a path or the area inside an outline,
    /// at `scale`; its text says how much.
    Measure {
        kind: Measure,
        points: Vec<(f32, f32)>,
        scale: Scale,
    },
}

/// The standard stamps every PDF reader knows, by PDF name and the words they show.
pub const STAMPS: [(&str, &str); 14] = [
    ("Approved", "Approved"),
    ("NotApproved", "Not approved"),
    ("Draft", "Draft"),
    ("Final", "Final"),
    ("Confidential", "Confidential"),
    ("ForComment", "For comment"),
    ("ForPublicRelease", "For public release"),
    ("NotForPublicRelease", "Not for public release"),
    ("Experimental", "Experimental"),
    ("Expired", "Expired"),
    ("AsIs", "As is"),
    ("Departmental", "Departmental"),
    ("Sold", "Sold"),
    ("TopSecret", "Top secret"),
];

/// MuPDF draws stamps on a 190 x 50 box.
const STAMP_ASPECT: f32 = 190.0 / 50.0;

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
        PdfAnnotationType::FileAttachment => AnnotKind::File,
        PdfAnnotationType::Redact => AnnotKind::Redact,
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

/// Text boxes and callouts draw their text, border and callout line in the colour of their
/// default appearance; their /C fills the box behind the text.
fn set_text_color(annot: &mut PdfAnnotation, color: AnnotationColor) -> Result<(), Error> {
    let (font, size) = match annot.default_appearance()? {
        Some(da) => (da.font_name, da.size),
        None => ("Helv".to_owned(), 12.0),
    };
    annot.set_default_appearance(&font, size, Some(color))?;
    Ok(())
}

pub(crate) fn describe(annot: &PdfAnnotation) -> Result<Option<Annot>, Error> {
    let subtype = annot.r#type()?;
    let Some(mut kind) = kind_of(subtype) else {
        return Ok(None);
    };
    let obj = annot.object();
    if kind == AnnotKind::FreeText && name(&obj, "IT")?.as_deref() == Some("FreeTextCallout") {
        kind = AnnotKind::Callout;
    }
    let measured = matches!(
        subtype,
        PdfAnnotationType::Line | PdfAnnotationType::PolyLine | PdfAnnotationType::Polygon
    );
    if measured && obj.get_dict("Measure")?.is_some() {
        kind = AnnotKind::Measure;
    }
    let state = name(&obj, "State")?;
    let shape = matches!(kind, AnnotKind::Square | AnnotKind::Circle);
    let lined = shape || matches!(kind, AnnotKind::Line | AnnotKind::Ink);
    let border = if shape || kind == AnnotKind::Line {
        let cloudy = obj
            .get_dict("BE")?
            .map(|be| name(&be, "S"))
            .transpose()?
            .flatten()
            .as_deref()
            == Some("C");
        let dashed = obj
            .get_dict("BS")?
            .map(|bs| name(&bs, "S"))
            .transpose()?
            .flatten()
            .as_deref()
            == Some("D");
        Some(if cloudy && shape {
            Border::Cloudy
        } else if dashed {
            Border::Dashed
        } else {
            Border::Solid
        })
    } else {
        None
    };
    let line_ends = if kind == AnnotKind::Line {
        let mut ends = (String::from("None"), String::from("None"));
        if let Some(le) = obj.get_dict("LE")?
            && le.is_array()?
        {
            let at = |i| -> Result<String, Error> {
                Ok(match le.get_array(i)? {
                    Some(n) if n.is_name()? => String::from_utf8_lossy(&n.as_name()?).into_owned(),
                    _ => "None".into(),
                })
            };
            ends = (at(0)?, at(1)?);
        }
        Some(ends)
    } else {
        None
    };
    let text_box = matches!(kind, AnnotKind::FreeText | AnnotKind::Callout);
    let font_size = if text_box {
        annot.default_appearance()?.map(|da| da.size)
    } else {
        None
    };
    Ok(Some(Annot {
        id: annot.xref()?,
        kind,
        rect: annot.bounds()?.into(),
        color: if text_box {
            annot.default_appearance()?.and_then(|da| da.color).map(rgb)
        } else {
            annot.color()?.map(rgb)
        },
        fill: if shape {
            annot.interior_color()?.map(rgb)
        } else if text_box {
            annot.color()?.map(rgb)
        } else {
            None
        },
        opacity: annot.opacity()?,
        width: if lined {
            Some(annot.border_width()?)
        } else {
            None
        },
        contents: annot.contents()?.unwrap_or_default().to_owned(),
        author: annot.author()?.unwrap_or_default().to_owned(),
        reply_to: reply_to(&obj)?,
        state,
        modified: string(&obj, "M")?.unwrap_or_default(),
        border,
        line_ends,
        font_size,
        subject: string(&obj, "Subj")?.unwrap_or_default(),
        locked: annot.flags()?.contains(AnnotationFlags::IS_LOCKED),
        printed: annot.flags()?.contains(AnnotationFlags::IS_PRINT),
    }))
}

pub(crate) fn string(obj: &PdfObject, key: &str) -> Result<Option<String>, Error> {
    match obj.get_dict(key)? {
        Some(v) if v.is_string()? => Ok(Some(v.as_string()?)),
        _ => Ok(None),
    }
}

pub(crate) fn name(obj: &PdfObject, key: &str) -> Result<Option<String>, Error> {
    match obj.get_dict(key)? {
        Some(v) if v.is_name()? => Ok(Some(String::from_utf8_lossy(&v.as_name()?).into_owned())),
        _ => Ok(None),
    }
}

/// The object number of the comment `obj` replies to. A grouped annotation (/RT /Group) is
/// part of its parent, not a reply, and gets None.
fn reply_to(obj: &PdfObject) -> Result<Option<i32>, Error> {
    let Some(irt) = obj.get_dict("IRT")? else {
        return Ok(None);
    };
    if name(obj, "RT")?.as_deref() == Some("Group") || !irt.is_indirect()? {
        return Ok(None);
    }
    Ok(Some(irt.as_indirect()?))
}

/// Leaves replies and review states out of the page's drawing; readers show them only in the
/// comment thread. The flag lasts while `page` lives.
pub(crate) fn hide_replies(page: &mupdf::Page) -> Result<(), Error> {
    let Ok(page) = PdfPage::try_from(page.clone()) else {
        return Ok(());
    };
    for mut annot in page.annotations() {
        if reply_to(&annot.object())?.is_some() {
            annot.set_hidden_for_editing(true);
        }
    }
    Ok(())
}

/// A Text annotation on the same spot as `parent` that points back at it.
fn new_reply(
    page: &mut PdfPage,
    parent: &PdfAnnotation,
    contents: &str,
    author: &str,
) -> Result<PdfAnnotation, Error> {
    let mut annot = page.create_annotation(PdfAnnotationType::Text)?;
    let mut obj = annot.object();
    obj.dict_put("NM", PdfObject::new_string(&unique_name())?)?;
    obj.dict_put("IRT", parent.object())?;
    annot.set_rect(parent.bounds()?)?;
    annot.set_contents(contents)?;
    if !author.is_empty() {
        annot.set_author(author)?;
    }
    Ok(annot)
}

fn find(page: &PdfPage, id: i32) -> Result<PdfAnnotation, Error> {
    page.annotations()
        .find(|a| a.xref().ok() == Some(id))
        .ok_or(Error::NotFound)
}

/// Answers comment `parent` with `text`.
pub fn reply(
    doc: &Document,
    page: usize,
    parent: i32,
    text: &str,
    author: &str,
) -> Result<Annot, Error> {
    operation(doc, "Reply", || {
        let mut page = pdf_page(doc, page)?;
        let parent = find(&page, parent)?;
        let mut annot = new_reply(&mut page, &parent, text, author)?;
        annot.update()?;
        page.update()?;
        describe(&annot)?.ok_or(Error::Invalid("annotation type has no entry"))
    })
}

/// Gives comment `parent` a review state, one of [`REVIEW_STATES`]. Like Acrobat, it adds a
/// hidden reply that holds the state, so earlier states stay in the record.
pub fn set_state(
    doc: &Document,
    page: usize,
    parent: i32,
    state: &str,
    author: &str,
) -> Result<(), Error> {
    if !REVIEW_STATES.contains(&state) {
        return Err(Error::Invalid("not a review state"));
    }
    let label = if state == "None" {
        "Status cleared"
    } else {
        state
    };
    operation(doc, &format!("Status: {label}"), || {
        let mut page = pdf_page(doc, page)?;
        let parent = find(&page, parent)?;
        let who = if author.is_empty() { "" } else { " by " };
        let text = format!("{state} set{who}{author}");
        let mut annot = new_reply(&mut page, &parent, &text, author)?;
        let mut obj = annot.object();
        obj.dict_put("State", PdfObject::new_name(state)?)?;
        obj.dict_put("StateModel", PdfObject::new_name("Review")?)?;
        let flags = annot.flags()? | AnnotationFlags::IS_HIDDEN;
        annot.set_flags(flags)?;
        annot.update()?;
        page.update()?;
        Ok(())
    })
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

/// A random annotation name (/NM). Review tools match comments by it, so importing an exported
/// comment back into its document does not add it twice.
pub(crate) fn unique_name() -> String {
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let half = || {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(nanos);
        h.finish()
    };
    format!("{:016x}{:016x}", half(), half())
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
        NewAnnot::Stamp { .. } => "Add stamp",
        NewAnnot::Callout { .. } => "Add callout",
        NewAnnot::File { .. } => "Attach file",
        NewAnnot::Measure { .. } => "Measure",
    }
}

/// The standard stamp `name` in `color`, `height` pixels tall, drawn as a placed one would be.
pub fn stamp_preview(name: &str, color: [f32; 3], height: u32) -> Result<Preview, Error> {
    let (width, tall) = (190.0, 190.0 / STAMP_ASPECT);
    let mut pdf = PdfDocument::new();
    pdf.new_page(mupdf::Size::new(width, tall))?;
    let new = NewAnnot::Stamp {
        name: name.to_owned(),
        center: (width / 2.0, tall / 2.0),
        width,
    };
    let style = Style {
        color,
        author: String::new(),
    };
    add_now(&pdf, 0, &new, &style)?;
    render::preview(&pdf.load_page(0)?.to_display_list(true)?, height)
}

pub fn add(doc: &Document, page: usize, new: &NewAnnot, style: &Style) -> Result<Annot, Error> {
    operation(doc, label(new), || add_now(doc, page, new, style))
}

fn add_now(doc: &Document, page: usize, new: &NewAnnot, style: &Style) -> Result<Annot, Error> {
    let mut page = pdf_page(doc, page)?;
    let subtype = match new {
        NewAnnot::TextMarkup { kind, .. } => markup_type(*kind)?,
        NewAnnot::Note { .. } => PdfAnnotationType::Text,
        NewAnnot::FreeText { .. } | NewAnnot::Callout { .. } => PdfAnnotationType::FreeText,
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
        NewAnnot::Stamp { name, .. } => {
            if !STAMPS.iter().any(|(n, _)| n == name) {
                return Err(Error::Invalid("not a standard stamp"));
            }
            PdfAnnotationType::Stamp
        }
        NewAnnot::File { .. } => PdfAnnotationType::FileAttachment,
        NewAnnot::Measure { kind, points, .. } => match kind {
            _ if points.len() < 2 => return Err(Error::Invalid("too few points to measure")),
            Measure::Distance => PdfAnnotationType::Line,
            Measure::Perimeter => PdfAnnotationType::PolyLine,
            Measure::Area if points.len() < 3 => {
                return Err(Error::Invalid("too few points to measure"));
            }
            Measure::Area => PdfAnnotationType::Polygon,
        },
    };
    let mut annot = page.create_annotation(subtype)?;
    annot
        .object()
        .dict_put("NM", PdfObject::new_string(&unique_name())?)?;
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
        NewAnnot::Callout { target, rect, text } => {
            annot.set_rect(to_rect(rect))?;
            annot.set_contents(text)?;
            // MuPDF draws the line from /CL (PDF space) and grows the Rect to hold it.
            let knee = (
                target.0.clamp(rect.x0, rect.x1),
                target.1.clamp(rect.y0, rect.y1),
            );
            let inverse = page
                .ctm()?
                .invert()
                .ok_or(Error::Invalid("page has no inverse transform"))?;
            let pdf = PdfDocument::try_from(doc.clone()).map_err(|_| Error::NotPdf)?;
            let mut line = pdf.new_array()?;
            for p in [*target, knee] {
                let p = point(p).transform(&inverse);
                line.array_push(PdfObject::new_real(p.x)?)?;
                line.array_push(PdfObject::new_real(p.y)?)?;
            }
            let mut obj = annot.object();
            obj.dict_put("CL", line)?;
            obj.dict_put("IT", PdfObject::new_name("FreeTextCallout")?)?;
            obj.dict_put("LE", PdfObject::new_name("OpenArrow")?)?;
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
        NewAnnot::Stamp {
            name,
            center: (x, y),
            width,
        } => {
            annot.set_icon_name(name)?;
            let (w, h) = (width / 2.0, width / STAMP_ASPECT / 2.0);
            annot.set_rect(mupdf::Rect::new(x - w, y - h, x + w, y + h))?;
        }
        NewAnnot::File {
            at: (x, y),
            name,
            data,
        } => {
            let mut pdf = PdfDocument::try_from(doc.clone()).map_err(|_| Error::NotPdf)?;
            let spec = pdf.new_embedded_file(data, EmbeddedFileOptions::new(name))?;
            annot.object().dict_put("FS", spec)?;
            annot.set_icon_name("Paperclip")?;
            annot.set_rect(mupdf::Rect::new(*x, *y, x + 20.0, y + 20.0))?;
            annot.set_contents(name)?;
        }
        NewAnnot::Measure {
            kind,
            points,
            scale,
        } => {
            let pdf = PdfDocument::try_from(doc.clone()).map_err(|_| Error::NotPdf)?;
            let (intent, subject) = match kind {
                Measure::Distance => ("LineDimension", "Length Measurement"),
                Measure::Perimeter => ("PolyLineDimension", "Perimeter Measurement"),
                Measure::Area => ("PolygonDimension", "Area Measurement"),
            };
            if *kind == Measure::Distance {
                annot.set_line(point(points[0]), point(points[points.len() - 1]))?;
                annot.set_line_ending_styles(
                    LineEndingStyle::OpenArrow,
                    LineEndingStyle::OpenArrow,
                )?;
                // Readers write the measure on the line.
                annot.object().dict_put("Cap", PdfObject::new_bool(true))?;
            } else {
                annot.set_vertices(points.iter().copied().map(point))?;
            }
            annot.set_border_width(1.0)?;
            annot.set_contents(&scale.label(*kind, points))?;
            let mut obj = annot.object();
            obj.dict_put("IT", PdfObject::new_name(intent)?)?;
            obj.dict_put("Subj", PdfObject::new_string(subject)?)?;
            obj.dict_put("Measure", scale.dictionary(&pdf)?)?;
        }
    }
    let [red, green, blue] = style.color;
    let color = AnnotationColor::Rgb { red, green, blue };
    if subtype == PdfAnnotationType::FreeText {
        set_text_color(&mut annot, color)?;
    } else {
        annot.set_color(color)?;
    }
    if !style.author.is_empty() {
        annot.set_author(&style.author)?;
    }
    page.update()?;
    describe(&annot)?.ok_or(Error::Invalid("annotation type has no entry"))
}

/// Deletes a comment with its replies and review states.
pub fn delete(doc: &Document, page: usize, id: i32) -> Result<(), Error> {
    operation(doc, "Delete comment", || {
        let mut page = pdf_page(doc, page)?;
        find(&page, id)?;
        let mut doomed = vec![id];
        // Replies can have replies; take each generation in turn.
        let mut i = 0;
        while i < doomed.len() {
            for annot in page.annotations() {
                let n = annot.xref()?;
                if reply_to(&annot.object())? == Some(doomed[i]) && !doomed.contains(&n) {
                    doomed.push(n);
                }
            }
            i += 1;
        }
        for n in doomed {
            let annot = find(&page, n)?;
            page.delete_annotation(annot)?;
        }
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

/// One change to a comment's look.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Restyle {
    Color([f32; 3]),
    /// The inside of a rectangle or ellipse; None leaves it clear.
    Fill(Option<[f32; 3]>),
    /// 1.0 is opaque.
    Opacity(f32),
    /// Line width in points, for shapes, lines and ink.
    Width(f32),
    Border(Border),
    /// A line's start and end, as PDF names from [`LINE_ENDS`].
    LineEnds(&'static str, &'static str),
    /// A text box's or callout's text size in points.
    FontSize(f32),
}

/// Changes one property of a comment's look, as one undoable step.
pub fn restyle(doc: &Document, page: usize, id: i32, change: Restyle) -> Result<(), Error> {
    let name = match change {
        Restyle::Color(_) => "Change colour",
        Restyle::Fill(_) => "Change fill",
        Restyle::Opacity(_) => "Change opacity",
        Restyle::Width(_) => "Change line width",
        Restyle::Border(_) => "Change border",
        Restyle::LineEnds(..) => "Change line ends",
        Restyle::FontSize(_) => "Change text size",
    };
    operation(doc, name, || {
        let mut page = pdf_page(doc, page)?;
        let mut annot = page
            .annotations()
            .find(|a| a.xref().ok() == Some(id))
            .ok_or(Error::NotFound)?;
        let color = |[red, green, blue]: [f32; 3]| AnnotationColor::Rgb { red, green, blue };
        let text_box = annot.r#type()? == PdfAnnotationType::FreeText;
        match change {
            Restyle::Color(c) if text_box => set_text_color(&mut annot, color(c))?,
            Restyle::Color(c) => annot.set_color(color(c))?,
            Restyle::Fill(Some(c)) if text_box => annot.set_color(color(c))?,
            Restyle::Fill(Some(c)) => annot.set_interior_color(color(c))?,
            Restyle::Fill(None) => {
                annot
                    .object()
                    .dict_delete(if text_box { "C" } else { "IC" })?;
                // Setting the flags to what they are marks the annotation changed.
                let flags = annot.flags()?;
                annot.set_flags(flags)?;
            }
            Restyle::Opacity(o) => annot.set_opacity(o.clamp(0.0, 1.0))?,
            Restyle::Width(w) => annot.set_border_width(w.max(0.0))?,
            Restyle::Border(border) => {
                let (style, effect) = match border {
                    Border::Solid => (AnnotationBorderStyle::Solid, AnnotationBorderEffect::None),
                    Border::Dashed => (AnnotationBorderStyle::Dashed, AnnotationBorderEffect::None),
                    Border::Cloudy => {
                        (AnnotationBorderStyle::Solid, AnnotationBorderEffect::Cloudy)
                    }
                };
                // Only rectangles and ellipses (of the kinds with a border) take an effect.
                let shape = matches!(
                    annot.r#type()?,
                    PdfAnnotationType::Square | PdfAnnotationType::Circle
                );
                if border == Border::Cloudy && !shape {
                    return Err(Error::Invalid("only rectangles and ellipses can be cloudy"));
                }
                annot.set_border_style(style)?;
                if border == Border::Dashed {
                    annot.set_border_dash_pattern(&[4.0, 3.0])?;
                }
                if shape {
                    annot.set_border_effect(effect)?;
                }
                if border == Border::Cloudy {
                    annot.set_border_effect_intensity(1.0)?;
                }
            }
            Restyle::LineEnds(start, end) => {
                let (Some(start), Some(end)) = (line_end(start), line_end(end)) else {
                    return Err(Error::Invalid("not a line end"));
                };
                annot.set_line_ending_styles(start, end)?;
            }
            Restyle::FontSize(size) => {
                let (font, color) = match annot.default_appearance()? {
                    Some(da) => (da.font_name, da.color),
                    None => ("Helv".to_owned(), None),
                };
                annot.set_default_appearance(&font, size.clamp(4.0, 144.0), color)?;
            }
        }
        page.update()?;
        Ok(())
    })
}

/// Sets comment `id`'s author, subject, lock and print flag, as one undoable step.
pub fn set_properties(
    doc: &Document,
    page: usize,
    id: i32,
    props: &Properties,
) -> Result<(), Error> {
    operation(doc, "Change properties", || {
        let mut page = pdf_page(doc, page)?;
        let mut annot = page
            .annotations()
            .find(|a| a.xref().ok() == Some(id))
            .ok_or(Error::NotFound)?;
        annot.set_author(&props.author)?;
        let mut obj = annot.object();
        if props.subject.is_empty() {
            obj.dict_delete("Subj")?;
        } else {
            obj.dict_put("Subj", PdfObject::new_string(&props.subject)?)?;
        }
        let mut flags = annot.flags()?;
        flags.set(AnnotationFlags::IS_LOCKED, props.locked);
        flags.set(AnnotationFlags::IS_PRINT, props.printed);
        annot.set_flags(flags)?;
        page.update()?;
        Ok(())
    })
}

/// Moves or resizes a comment so its bounds become `rect` (page space). The points inside it
/// (ink strokes, line ends, vertices, text quads) move and scale with it.
pub fn reshape(doc: &Document, page: usize, id: i32, rect: Rect) -> Result<(), Error> {
    let mut page = pdf_page(doc, page)?;
    let mut annot = page
        .annotations()
        .find(|a| a.xref().ok() == Some(id))
        .ok_or(Error::NotFound)?;
    let old: Rect = annot.bounds()?.into();
    let moved_only =
        (old.width() - rect.width()).abs() < 0.01 && (old.height() - rect.height()).abs() < 0.01;
    let name = if moved_only {
        "Move comment"
    } else {
        "Resize comment"
    };
    operation(doc, name, || {
        let inverse = page
            .ctm()?
            .invert()
            .ok_or(Error::Invalid("page has no inverse transform"))?;
        // Pages turn in quarter turns only, so boxes stay axis-aligned in PDF space.
        let to_pdf = |r: Rect| mupdf::Rect::new(r.x0, r.y0, r.x1, r.y1).transform(&inverse);
        let target = to_pdf(rect);
        let stamp = annot.r#type()? == PdfAnnotationType::Stamp;
        let inner = reshape_now(&mut page, &mut annot, to_pdf(old), target, stamp)?;
        // MuPDF gives ink, lines and polygons new bounds: their points plus a margin for the
        // stroke. Fit the points inside `rect` less that margin, so the bounds land on `rect`.
        if let Some(inner) = inner {
            let got = to_pdf(annot.bounds()?.into());
            let fit = mupdf::Rect::new(
                target.x0 + (inner.x0 - got.x0),
                target.y0 + (inner.y0 - got.y0),
                target.x1 - (got.x1 - inner.x1),
                target.y1 - (got.y1 - inner.y1),
            );
            let off = [
                fit.x0 - inner.x0,
                fit.y0 - inner.y0,
                fit.x1 - inner.x1,
                fit.y1 - inner.y1,
            ];
            if fit.x1 >= fit.x0 && fit.y1 >= fit.y0 && off.iter().any(|d| d.abs() > 0.1) {
                reshape_now(&mut page, &mut annot, inner, fit, stamp)?;
            }
        }
        Ok(())
    })
}

/// Maps the annotation's box `from` onto `to` (PDF space). Returns the bounds of the points
/// inside it after the move, if it has any.
fn reshape_now(
    page: &mut PdfPage,
    annot: &mut PdfAnnotation,
    from: mupdf::Rect,
    to: mupdf::Rect,
    stamp: bool,
) -> Result<Option<mupdf::Rect>, Error> {
    let scale = |to: f32, from: f32| if from < 0.01 { 1.0 } else { to / from };
    let sx = scale(to.x1 - to.x0, from.x1 - from.x0);
    let sy = scale(to.y1 - to.y0, from.y1 - from.y0);
    let map = |x: f32, y: f32| (to.x0 + (x - from.x0) * sx, to.y0 + (y - from.y0) * sy);
    let obj = annot.object();
    if let Some(mut points) = obj.get_dict("Rect")? {
        map_points(&mut points, map, &mut None)?;
    }
    let mut inner = None;
    for key in ["L", "Vertices", "QuadPoints"] {
        if let Some(mut points) = obj.get_dict(key)? {
            map_points(&mut points, map, &mut inner)?;
        }
    }
    // A callout's line lies inside its Rect, which MuPDF keeps; it needs no second fit.
    if let Some(mut points) = obj.get_dict("CL")? {
        map_points(&mut points, map, &mut None)?;
    }
    if let Some(strokes) = obj.get_dict("InkList")? {
        for stroke in strokes.array_iter()? {
            map_points(&mut stroke?, map, &mut inner)?;
        }
    }
    // A stamp keeps its own appearance, which MuPDF fits to the new Rect. Other kinds are
    // redrawn: setting the flags to what they are marks the annotation changed.
    if !stamp {
        let flags = annot.flags()?;
        annot.set_flags(flags)?;
    }
    page.update()?;
    Ok(inner)
}

/// Maps each x, y pair of a number array in place, growing `bounds` to hold the results.
fn map_points(
    points: &mut PdfObject,
    map: impl Fn(f32, f32) -> (f32, f32),
    bounds: &mut Option<mupdf::Rect>,
) -> Result<(), Error> {
    if !points.is_array()? {
        return Ok(());
    }
    let n = points.len()? as i32;
    for i in (0..n - 1).step_by(2) {
        let value = |i| -> Result<f32, Error> {
            Ok(points
                .get_array(i)?
                .map(|v| v.as_float())
                .transpose()?
                .unwrap_or(0.0))
        };
        let (x, y) = map(value(i)?, value(i + 1)?);
        points.array_put(i, PdfObject::new_real(x)?)?;
        points.array_put(i + 1, PdfObject::new_real(y)?)?;
        let b = bounds.get_or_insert(mupdf::Rect::new(x, y, x, y));
        *b = mupdf::Rect::new(b.x0.min(x), b.y0.min(y), b.x1.max(x), b.y1.max(y));
    }
    Ok(())
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
        if comments {
            // Replies would bake in as note icons; they go with the discussion.
            for n in 0..doc.page_count()? {
                let mut page = pdf_page(doc, n as usize)?;
                let replies: Vec<PdfAnnotation> = page
                    .annotations()
                    .filter(|a| reply_to(&a.object()).ok().flatten().is_some())
                    .collect();
                for annot in replies {
                    page.delete_annotation(annot)?;
                }
            }
        }
        if fields {
            lend_form_resources(&mut pdf, doc)?;
        }
        Ok(pdf.bake(comments, fields)?)
    })
}

/// Field appearances may leave out their resources and lean on the form's default resources
/// (/DR) and the standard form fonts, Helv and ZaDb, as readers allow. Baked into the page they
/// would lose them, and a check mark drawn in ZapfDingbats would become a "4"; so each such
/// appearance gets /DR, with the standard fonts added where it lacks them.
fn lend_form_resources(pdf: &mut PdfDocument, doc: &Document) -> Result<(), Error> {
    let mut bare = Vec::new();
    for n in 0..doc.page_count()? {
        let page = pdf_page(doc, n as usize)?;
        for widget in page.widgets() {
            let Some(normal) = widget
                .annotation()
                .object()
                .get_dict("AP")?
                .map(|ap| ap.get_dict("N"))
                .transpose()?
                .flatten()
            else {
                continue;
            };
            // One look, or one per state of a checkbox or radio button.
            let looks: Vec<PdfObject> = if normal.is_stream()? {
                vec![normal]
            } else {
                (0..normal.dict_len()? as i32)
                    .filter_map(|i| normal.get_dict_val(i).transpose())
                    .collect::<Result<_, _>>()?
            };
            for look in looks {
                if look.is_stream()? && look.get_dict("Resources")?.is_none() {
                    bare.push(look);
                }
            }
        }
    }
    if bare.is_empty() {
        return Ok(());
    }
    let dr = match pdf.catalog()?.get_dict("AcroForm")? {
        Some(form) => form.get_dict("DR")?,
        None => None,
    };
    let mut resources = match dr {
        Some(dr) => dr.copy_dict()?,
        None => pdf.new_dict()?,
    };
    let mut fonts = match resources.get_dict("Font")? {
        Some(fonts) => fonts.copy_dict()?,
        None => pdf.new_dict()?,
    };
    for (alias, base) in [("Helv", "Helvetica"), ("ZaDb", "ZapfDingbats")] {
        if fonts.get_dict(alias)?.is_none() {
            let font = format!("<</Type /Font /Subtype /Type1 /BaseFont /{base}>>");
            fonts.dict_put(alias, pdf.new_object_from_str(&font)?)?;
        }
    }
    resources.dict_put("Font", fonts)?;
    let resources = pdf.add_object(&resources)?;
    for mut look in bare {
        look.dict_put("Resources", resources.try_clone()?)?;
    }
    Ok(())
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

/// Whether a save can append to the file instead of rewriting it. MuPDF rewrites files it
/// had to repair on open, and redacted ones.
pub(crate) fn can_append(doc: &Document) -> bool {
    PdfDocument::try_from(doc.clone()).is_ok_and(|pdf| pdf.can_be_saved_incrementally())
}

pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Where a full save of `path` is written before it replaces the file.
pub(crate) fn sibling(path: &Path) -> std::path::PathBuf {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!("{name}.micropdf-save.tmp"))
}

/// Puts `replacement` in place of `original` in one step, keeping the original's attributes,
/// permissions and creation time. On failure both files stay as they were.
pub(crate) fn replace(original: &Path, replacement: &Path) -> Result<(), Error> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        REPLACEFILE_IGNORE_ACL_ERRORS, REPLACEFILE_IGNORE_MERGE_ERRORS, ReplaceFileW,
    };
    let wide = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain([0]).collect() };
    let (to, from) = (wide(original), wide(replacement));
    let flags = REPLACEFILE_IGNORE_MERGE_ERRORS | REPLACEFILE_IGNORE_ACL_ERRORS;
    // SAFETY: both names are NUL-terminated and outlive the call; the rest are optional.
    let ok = unsafe {
        ReplaceFileW(
            to.as_ptr(),
            from.as_ptr(),
            std::ptr::null(),
            flags,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok == 0 {
        let e = std::io::Error::last_os_error();
        return Err(Error::Io(std::io::Error::new(
            e.kind(),
            format!(
                "could not replace the file ({e}); the changes are saved in {}",
                replacement.display()
            ),
        )));
    }
    Ok(())
}
