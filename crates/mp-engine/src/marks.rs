//! Signatures and initials: the reader's own mark, drawn, typed or imported, placed on a page as
//! a stamp whose appearance is the mark. This is a visual signature, not a digital one.

use mupdf::pdf::{PdfAnnotationType, PdfObject, PdfPage};
use mupdf::{
    ColorParams, Colorspace, Device, DisplayList, Document, Image, LineCap, LineJoin, Matrix, Path,
    StrokeState,
};

use crate::annots::{Annot, describe, operation, unique_name};
use crate::render::{self, Preview};
use crate::{Error, Rect};

/// A signature or initials.
#[derive(Debug, Clone, PartialEq)]
pub enum Mark {
    /// Pen strokes in any units, y pointing down, drawn `width` units wide.
    Ink {
        strokes: Vec<Vec<(f32, f32)>>,
        width: f32,
    },
    /// Text in an installed font family, such as "Segoe Script".
    Typed { text: String, font: String },
    /// A PNG or JPEG image.
    Image(Vec<u8>),
}

/// The stamp's icon name. It is not one of the standard stamp names, so MuPDF keeps the mark as
/// the appearance instead of drawing a "Draft" stamp over it when the stamp changes.
const ICON: &str = "Signature";

/// Text is laid out at this size; the stamp scales it to the placed width.
const TEXT_SIZE: f32 = 48.0;

fn round_stroke(width: f32) -> Result<StrokeState, Error> {
    Ok(StrokeState::new(
        LineCap::Round,
        LineCap::Round,
        LineCap::Round,
        LineJoin::Round,
        width,
        10.0,
        0.0,
        &[],
    )?)
}

fn union(a: Option<mupdf::Rect>, b: mupdf::Rect) -> Option<mupdf::Rect> {
    Some(match a {
        None => b,
        Some(a) => mupdf::Rect::new(
            a.x0.min(b.x0),
            a.y0.min(b.y0),
            a.x1.max(b.x1),
            a.y1.max(b.y1),
        ),
    })
}

/// Records the mark in its own units, bounded by what it draws.
fn draw(mark: &Mark, color: [f32; 3]) -> Result<DisplayList, Error> {
    let rgb = Colorspace::device_rgb();
    let id = Matrix::IDENTITY;
    match mark {
        Mark::Ink { strokes, width } => {
            let mut path = Path::new()?;
            for stroke in strokes {
                let Some(&(x, y)) = stroke.first() else {
                    continue;
                };
                path.move_to(x, y)?;
                // A lone point still draws a dot with round caps.
                if stroke.len() == 1 {
                    path.line_to(x, y)?;
                }
                for &(x, y) in &stroke[1..] {
                    path.line_to(x, y)?;
                }
            }
            let stroke = round_stroke(*width)?;
            let bounds = path.bounds(&stroke, &id)?;
            if bounds.is_empty() {
                return Err(Error::Invalid("the signature is empty"));
            }
            let mut list = DisplayList::new(bounds)?;
            let dev = Device::from_display_list(&mut list)?;
            dev.stroke_path(
                &path,
                &stroke,
                &id,
                &rgb,
                &color,
                1.0,
                ColorParams::default(),
            )?;

            drop(dev);
            Ok(list)
        }
        Mark::Typed { text, font } => {
            let font =
                crate::fonts::family(font).ok_or(Error::Invalid("the font is not installed"))?;
            let mut glyphs = Vec::new();
            let mut bounds = None;
            let no_stroke = round_stroke(0.0)?;
            let mut x = 0.0;
            for c in text.trim().chars() {
                let gid = font.encode_character(c as i32)?;
                let ctm = Matrix::new(TEXT_SIZE, 0.0, 0.0, -TEXT_SIZE, x, 0.0);
                if let Some(glyph) = font.outline_glyph_with_ctm(gid, &ctm)? {
                    let b = glyph.bounds(&no_stroke, &id)?;
                    if !b.is_empty() {
                        bounds = union(bounds, b);
                        glyphs.push(glyph);
                    }
                }
                x += font.advance_glyph(gid)? * TEXT_SIZE;
            }
            let bounds = bounds.ok_or(Error::Invalid("the signature is empty"))?;
            let mut list = DisplayList::new(bounds)?;
            let dev = Device::from_display_list(&mut list)?;
            for glyph in &glyphs {
                dev.fill_path(glyph, false, &id, &rgb, &color, 1.0, ColorParams::default())?;
            }

            drop(dev);
            Ok(list)
        }
        Mark::Image(bytes) => {
            let image = Image::from_bytes(bytes)?;
            let (w, h) = (image.width() as f32, image.height() as f32);
            if w < 1.0 || h < 1.0 {
                return Err(Error::Invalid("the image is empty"));
            }
            let mut list = DisplayList::new(mupdf::Rect::new(0.0, 0.0, w, h))?;
            let dev = Device::from_display_list(&mut list)?;
            dev.fill_image(
                &image,
                &Matrix::new(w, 0.0, 0.0, h, 0.0, 0.0),
                1.0,
                ColorParams::default(),
            )?;

            drop(dev);
            Ok(list)
        }
    }
}

/// Width over height, so a caller can show where the mark will go before placing it.
pub fn aspect(mark: &Mark) -> Result<f32, Error> {
    let b = draw(mark, [0.0; 3])?.bounds();
    Ok((b.x1 - b.x0) / (b.y1 - b.y0))
}

/// The mark drawn black, `height` pixels tall, to show under the pointer before it is placed.
pub fn preview(mark: &Mark, height: u32) -> Result<Preview, Error> {
    render::preview(&draw(mark, [0.0; 3])?, height)
}

/// Places `mark` on `page`, `width` points wide and centred on `center`, as one undoable step.
pub fn place(
    doc: &Document,
    page: usize,
    mark: &Mark,
    center: (f32, f32),
    width: f32,
    color: [f32; 3],
) -> Result<Annot, Error> {
    let list = draw(mark, color)?;
    let b = list.bounds();
    let height = width * (b.y1 - b.y0) / (b.x1 - b.x0);
    let rect = Rect {
        x0: center.0 - width / 2.0,
        y0: center.1 - height / 2.0,
        x1: center.0 + width / 2.0,
        y1: center.1 + height / 2.0,
    };
    operation(doc, "Sign", || {
        let mut page = PdfPage::try_from(doc.load_page(page as i32)?).map_err(|_| Error::NotPdf)?;
        let mut annot = page.create_annotation(PdfAnnotationType::Stamp)?;
        annot.set_icon_name(ICON)?;
        annot
            .object()
            .dict_put("NM", PdfObject::new_string(&unique_name())?)?;
        annot.set_rect(mupdf::Rect::new(rect.x0, rect.y0, rect.x1, rect.y1))?;
        annot.set_appearance(&list)?;
        page.update()?;
        describe(&annot)?.ok_or(Error::Invalid("annotation type has no entry"))
    })
}
