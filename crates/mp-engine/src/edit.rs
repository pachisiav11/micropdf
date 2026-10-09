//! Editing page content in place. A block of text is set again with new words: its glyphs go
//! by a text-only redaction, so images and drawings under them stay, and the new text is set in
//! the block's own font when it has every glyph, else in the closest standard font. New text
//! goes in the same way. Images are added, moved, resized, replaced and deleted.

use std::path::PathBuf;

use mupdf::pdf::{
    InsertFontOptions, PdfAnnotationType, PdfDocument, PdfObject, PdfPage, PdfRedactImageMethod,
    PdfRedactLineArtMethod, PdfRedactOptions, PdfRedactTextMethod,
};
use mupdf::text_page::{TextBlockType, TextPageFlags};
use mupdf::{Document, Font, Image, Matrix};

use crate::ocr::resource;
use crate::pages::pdf;
use crate::stamp::win_ansi_code;
use crate::{DocId, Error, Rect};

/// A block of text as Edit text picks it: its box, its text with a line break between
/// paragraphs, and its type size in points.
#[derive(Debug, Clone, PartialEq)]
pub struct TextBlock {
    pub rect: Rect,
    pub text: String,
    pub size: f32,
}

/// The font replaced text was set in, and whether it is the block's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replaced {
    pub font: String,
    pub own: bool,
}

/// Where text is set: the first baseline's start, the right edge lines wrap at, the size,
/// from one baseline to the next, the colour, and the font it should look like.
struct Spot {
    origin: (f32, f32),
    right: f32,
    size: f32,
    leading: f32,
    color: [f32; 3],
    font: Option<Font>,
}

/// A block with what setting it again needs.
struct Block {
    shown: TextBlock,
    /// Each line's box, for the redaction.
    lines: Vec<Rect>,
    spot: Spot,
}

fn blocks(page: &PdfPage) -> Result<Vec<Block>, Error> {
    let text = page.to_text_page(TextPageFlags::empty())?;
    let mut out = Vec::new();
    for block in text.blocks() {
        if block.r#type() != TextBlockType::Text {
            continue;
        }
        let rect: Rect = block.bounds().into();
        // Each line's text, box and first origin.
        let mut lines: Vec<(String, Rect, (f32, f32))> = Vec::new();
        let mut first = None;
        for line in block.lines() {
            let mut s = String::new();
            let mut origin = None;
            for c in line.chars() {
                if origin.is_none() {
                    let o = c.origin();
                    origin = Some((o.x, o.y));
                    first.get_or_insert_with(|| (c.size(), c.argb(), c.font()));
                }
                s.extend(c.char());
            }
            if let Some(origin) = origin {
                lines.push((s.trim_end().to_owned(), line.bounds().into(), origin));
            }
        }
        let Some((size, argb, font)) = first else {
            continue;
        };
        let width = rect.width();
        let mut joined = String::new();
        for (i, (s, r, _)) in lines.iter().enumerate() {
            joined += s;
            if i + 1 < lines.len() {
                // A line that stops well short of the block's right edge ends its paragraph.
                joined.push(if rect.x1 - r.x1 > width * 0.2 {
                    '\n'
                } else {
                    ' '
                });
            }
        }
        let n = lines.len();
        let leading = if n > 1 {
            (lines[n - 1].2.1 - lines[0].2.1) / (n - 1) as f32
        } else {
            size * 1.2
        };
        let [_, r, g, b] = argb.to_be_bytes();
        out.push(Block {
            shown: TextBlock {
                rect,
                text: joined,
                size,
            },
            lines: lines.iter().map(|l| l.1).collect(),
            spot: Spot {
                origin: lines[0].2,
                right: rect.x1,
                size,
                leading: leading.max(size * 0.8),
                color: [r, g, b].map(|v| v as f32 / 255.0),
                font,
            },
        });
    }
    Ok(out)
}

/// The standard fonts, by family (Courier, Times, Helvetica) and by plain, bold, italic and
/// bold italic.
const STANDARD: [[&str; 4]; 3] = [
    [
        "Courier",
        "Courier-Bold",
        "Courier-Oblique",
        "Courier-BoldOblique",
    ],
    [
        "Times-Roman",
        "Times-Bold",
        "Times-Italic",
        "Times-BoldItalic",
    ],
    [
        "Helvetica",
        "Helvetica-Bold",
        "Helvetica-Oblique",
        "Helvetica-BoldOblique",
    ],
];

/// The standard font closest to `font`: its family by name or look, its weight and slant.
fn standard(font: Option<&Font>) -> &'static str {
    let name = font.map_or(String::new(), |f| f.name().to_lowercase());
    let has = |f: fn(&Font) -> bool| font.is_some_and(f);
    let family = if name.contains("courier") || name.contains("mono") || has(Font::is_monospaced) {
        0
    } else if name.contains("times")
        || (name.contains("serif") && !name.contains("sans"))
        || has(Font::is_serif)
    {
        1
    } else {
        2
    };
    let bold = name.contains("bold") || has(Font::is_bold);
    let italic = name.contains("italic") || name.contains("oblique") || has(Font::is_italic);
    STANDARD[family][bold as usize + 2 * italic as usize]
}

/// Windows fonts to set text in that no standard font can show, in order of preference.
const SYSTEM_FONTS: [&str; 6] = [
    "arial.ttf",
    "segoeui.ttf",
    "Nirmala.ttf",
    "msyh.ttc",
    "malgun.ttf",
    "msgothic.ttc",
];

fn system_font_path(file: &str) -> PathBuf {
    let windows = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
    PathBuf::from(windows).join("Fonts").join(file)
}

/// Whether `font` has a glyph for every character of `text` but its spaces.
fn has_glyphs(font: &Font, text: &str) -> bool {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .all(|c| font.encode_character(c as i32).is_ok_and(|g| g > 0))
}

/// How the new text is written: a font resource of the page, and each character's code.
struct Setter {
    resource: String,
    font: Font,
    /// Two-byte glyph ids (an Identity-H font) rather than one-byte WinAnsi codes.
    cid: bool,
}

impl Setter {
    /// A word as a hex string for Tj, and its advance at size 1.
    fn word(&self, word: &str) -> Option<(String, f32)> {
        let mut hex = String::new();
        let mut advance = 0.0;
        for c in word.chars() {
            let gid = self
                .font
                .encode_character(c as i32)
                .ok()
                .filter(|&g| g > 0)?;
            advance += self.font.advance_glyph(gid).ok()?;
            if self.cid {
                hex += &format!("{gid:04X}");
            } else {
                hex += &format!("{:02X}", win_ansi_code(c)?);
            }
        }
        Some((hex, advance))
    }

    fn space(&self) -> f32 {
        self.font
            .encode_character(' ' as i32)
            .ok()
            .filter(|&g| g > 0)
            .and_then(|g| self.font.advance_glyph(g).ok())
            .unwrap_or(0.25)
    }
}

/// Chooses the font for `text` to look like `own` and adds it to the page's resources.
fn setter(
    pdf: &mut PdfDocument,
    page: &mut PdfPage,
    own: Option<&Font>,
    text: &str,
) -> Result<(Setter, Replaced), Error> {
    let own_name = own.map_or("", |f| f.name());
    let base = own_name.rsplit('+').next().unwrap_or(own_name);
    let standard_own = STANDARD.iter().flatten().any(|&n| n == base);
    // An embedded font with every glyph is set again as itself; a standard one, which readers
    // supply, is named again rather than embedded.
    if !standard_own
        && let Some(font) = own
        && has_glyphs(font, text)
        && let Ok(obj) = pdf.add_font(font)
    {
        let resource = resource(pdf, page, "Font", &obj, "Ed")?;
        let replaced = Replaced {
            font: base.to_owned(),
            own: true,
        };
        let font = font.clone();
        return Ok((
            Setter {
                resource,
                font,
                cid: true,
            },
            replaced,
        ));
    }
    if text
        .chars()
        .all(|c| c.is_whitespace() || win_ansi_code(c).is_some())
    {
        let name = standard(own);
        let (resource, xref, _) = page.insert_font(pdf, &InsertFontOptions::new(name))?;
        // MuPDF leaves standard fonts in StandardEncoding, which has no accented letters.
        if let Some(mut dict) = pdf.xref_object(xref)? {
            dict.dict_put("Encoding", PdfObject::new_name("WinAnsiEncoding")?)?;
        }
        let replaced = Replaced {
            font: name.to_owned(),
            own: standard_own && name == base,
        };
        let setter = Setter {
            resource: resource.trim_start_matches('/').to_owned(),
            font: Font::new(name)?,
            cid: false,
        };
        return Ok((setter, replaced));
    }
    for file in SYSTEM_FONTS {
        let Ok(data) = std::fs::read(system_font_path(file)) else {
            continue;
        };
        let Ok(font) = Font::from_bytes(file, &data) else {
            continue;
        };
        if has_glyphs(&font, text) {
            let obj = pdf.add_font(&font)?;
            let resource = resource(pdf, page, "Font", &obj, "Ed")?;
            let replaced = Replaced {
                font: font.name().to_owned(),
                own: false,
            };
            let setter = Setter {
                resource,
                font,
                cid: true,
            };
            return Ok((setter, replaced));
        }
    }
    Err(Error::Invalid(
        "no installed font has all of these characters",
    ))
}

/// The block on `page` nearest `rect`, the box Edit text picked.
fn find(page: &PdfPage, rect: Rect) -> Result<Block, Error> {
    let center = |r: &Rect| ((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0);
    let (cx, cy) = center(&rect);
    blocks(page)?
        .into_iter()
        .min_by(|a, b| {
            let d = |r: &Rect| {
                let (x, y) = center(r);
                (x - cx).hypot(y - cy)
            };
            d(&a.shown.rect).total_cmp(&d(&b.shown.rect))
        })
        .ok_or(Error::NotFound)
}

/// Lines of words at most `limit` wide at size 1: each word's offset along its line and code.
fn wrap(setter: &Setter, text: &str, limit: f32) -> Result<Vec<Vec<(f32, String)>>, Error> {
    let space = setter.space();
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line: Vec<(f32, String)> = Vec::new();
        let mut x = 0.0;
        for word in paragraph.split_whitespace() {
            let (hex, width) = setter
                .word(word)
                .ok_or(Error::Invalid("the font has no glyph for a character"))?;
            let at = if line.is_empty() { 0.0 } else { x + space };
            if !line.is_empty() && at + width > limit {
                lines.push(std::mem::take(&mut line));
                x = width;
                line.push((0.0, hex));
            } else {
                line.push((at, hex));
                x = at + width;
            }
        }
        lines.push(line);
    }
    Ok(lines)
}

/// The page's inverse transform: page space to PDF user space.
fn to_pdf(page: &PdfPage) -> Result<Matrix, Error> {
    page.ctm()?
        .invert()
        .ok_or(Error::Invalid("the page has no area"))
}

/// Adds `ops` over the page's contents.
fn draw(pdf: &mut PdfDocument, page: &mut PdfPage, ops: &str) -> Result<(), Error> {
    page.wrap_contents(pdf)?;
    page.insert_contents(pdf, ops.as_bytes(), true)?;
    Ok(())
}

/// Writes `text` at `spot`, wrapped at its right edge.
fn set(
    pdf: &mut PdfDocument,
    page: &mut PdfPage,
    spot: &Spot,
    text: &str,
) -> Result<Replaced, Error> {
    let (setter, replaced) = setter(pdf, page, spot.font.as_ref(), text)?;
    let size = spot.size;
    let lines = wrap(&setter, text, (spot.right - spot.origin.0) / size * 1.02)?;
    let to_pdf = to_pdf(page)?;
    let [r, g, b] = spot.color;
    let mut ops = format!(
        "q\n{r:.3} {g:.3} {b:.3} rg\nBT\n/{} {size:.2} Tf\n",
        setter.resource
    );
    for (i, line) in lines.iter().enumerate() {
        let y = spot.origin.1 + i as f32 * spot.leading;
        for (x, hex) in line {
            // Text space is y up; the page space it is placed in, y down.
            let mut tm = Matrix::new(1.0, 0.0, 0.0, -1.0, spot.origin.0 + x * size, y);
            tm.concat(to_pdf.clone());
            ops += &format!(
                "{:.4} {:.4} {:.4} {:.4} {:.3} {:.3} Tm <{hex}> Tj\n",
                tm.a, tm.b, tm.c, tm.d, tm.e, tm.f
            );
        }
    }
    ops += "ET\nQ\n";
    draw(pdf, page, &ops)?;
    Ok(replaced)
}

fn replace(doc: &Document, page_no: usize, rect: Rect, text: &str) -> Result<Replaced, Error> {
    let mut pdf = pdf(doc)?;
    let mut page = pdf.load_pdf_page(page_no as i32)?;
    if page
        .annotations()
        .any(|a| a.r#type().ok() == Some(PdfAnnotationType::Redact))
    {
        return Err(Error::Invalid(
            "the page has redaction marks; apply or remove them first",
        ));
    }
    let block = find(&page, rect)?;
    // Each line's middle, so glyphs of the lines above and below stay.
    let marks: Vec<Rect> = block
        .lines
        .iter()
        .map(|r| {
            let inset = r.height() * 0.2;
            Rect {
                y0: r.y0 + inset,
                y1: r.y1 - inset,
                ..*r
            }
        })
        .collect();
    crate::redact::mark(&mut page, &marks)?;
    page.apply_redactions_with_options(PdfRedactOptions {
        black_boxes: false,
        image_method: PdfRedactImageMethod::None,
        line_art: PdfRedactLineArtMethod::None,
        text: PdfRedactTextMethod::Remove,
    })?;
    if text.trim().is_empty() {
        return Ok(Replaced {
            font: String::new(),
            own: true,
        });
    }
    set(&mut pdf, &mut page, &block.spot, text)
}

/// The image blocks of `page`: each one's box, the image, and how it is drawn (from the unit
/// square to page space).
fn images(page: &PdfPage) -> Result<Vec<(Rect, Image, Matrix)>, Error> {
    let text = page.to_text_page(TextPageFlags::PRESERVE_IMAGES)?;
    Ok(text
        .blocks()
        .filter(|b| b.r#type() == TextBlockType::Image)
        .filter_map(|b| Some((b.bounds().into(), b.image()?, b.ctm()?)))
        .collect())
}

/// What [`Engine::place_image`] puts where the image was.
pub enum ImageSource {
    /// The image already at the old box.
    Same,
    /// An image file.
    File(PathBuf),
}

fn place(
    doc: &Document,
    page_no: usize,
    from: Option<Rect>,
    to: Option<Rect>,
    source: ImageSource,
) -> Result<(), Error> {
    let mut pdf = pdf(doc)?;
    let mut page = pdf.load_pdf_page(page_no as i32)?;
    let to_pdf = to_pdf(&page)?;
    let pdf_rect = |r: &Rect| mupdf::Rect::new(r.x0, r.y0, r.x1, r.y1).transform(&to_pdf);
    let old = match from {
        Some(from) => {
            let near = |r: &Rect| (r.x0 - from.x0).abs() + (r.y1 - from.y1).abs();
            let found = images(&page)?
                .into_iter()
                .min_by(|a, b| near(&a.0).total_cmp(&near(&b.0)))
                .filter(|(r, ..)| near(r) < 2.0)
                .ok_or(Error::NotFound)?;
            if page.remove_images_at(pdf_rect(&found.0), 1.0)? == 0 {
                return Err(Error::Invalid(
                    "the image is drawn in a way micropdf cannot change",
                ));
            }
            Some(found)
        }
        None => None,
    };
    let Some(to) = to else {
        return Ok(());
    };
    let (image, mut m) = match (source, old) {
        (ImageSource::Same, Some((r, image, mut m))) => {
            // As it was drawn, then from the old box to the new one.
            let (sx, sy) = (to.width() / r.width(), to.height() / r.height());
            m.concat(Matrix::new(
                sx,
                0.0,
                0.0,
                sy,
                to.x0 - r.x0 * sx,
                to.y0 - r.y0 * sy,
            ));
            (image, m)
        }
        (ImageSource::Same, None) => return Err(Error::NotFound),
        (ImageSource::File(path), _) => {
            let image = Image::from_file(&path.to_string_lossy())?;
            // Fitted in the box, keeping its shape, and centred.
            let (w, h) = (image.width() as f32, image.height() as f32);
            let k = (to.width() / w).min(to.height() / h);
            let (w, h) = (w * k, h * k);
            let x = to.x0 + (to.width() - w) / 2.0;
            let y = to.y0 + (to.height() - h) / 2.0;
            (image, Matrix::new(w, 0.0, 0.0, -h, x, y + h))
        }
    };
    m.concat(to_pdf);
    let obj = pdf.add_image(&image)?;
    let name = resource(&pdf, &page, "XObject", &obj, "Img")?;
    let ops = format!(
        "q\n{:.4} {:.4} {:.4} {:.4} {:.3} {:.3} cm\n/{name} Do\nQ\n",
        m.a, m.b, m.c, m.d, m.e, m.f
    );
    draw(&mut pdf, &mut page, &ops)
}

impl crate::Engine {
    /// The blocks of text on `page`, for picking one to edit.
    pub fn text_blocks(&self, doc: DocId, page: usize) -> Result<Vec<TextBlock>, Error> {
        self.read(doc, move |d, _| {
            let page = pdf(d)?.load_pdf_page(page as i32)?;
            Ok(blocks(&page)?.into_iter().map(|b| b.shown).collect())
        })
    }

    /// Sets `text` in place of the block at `rect` on `page`, as one undo step; paragraphs
    /// are split by line breaks and wrapped to the block's width.
    pub fn replace_text(
        &self,
        doc: DocId,
        page: usize,
        rect: Rect,
        text: String,
    ) -> Result<Replaced, Error> {
        self.remove(doc, "Edit text", move |d| replace(d, page, rect, &text))
    }

    /// Writes `text` on `page` in black Helvetica of `size` (or an installed font that has
    /// its letters), from the top left of `rect` and wrapped at its right edge.
    pub fn add_text(
        &self,
        doc: DocId,
        page: usize,
        rect: Rect,
        text: String,
        size: f32,
    ) -> Result<Replaced, Error> {
        self.edit(doc, "Add text", move |d| {
            let mut pdf = pdf(d)?;
            let mut page = pdf.load_pdf_page(page as i32)?;
            let spot = Spot {
                origin: (rect.x0, rect.y0 + size * 0.9),
                right: rect.x1,
                size,
                leading: size * 1.2,
                color: [0.0; 3],
                font: None,
            };
            set(&mut pdf, &mut page, &spot, &text)
        })
    }

    /// The boxes of the images on `page`, in drawing order.
    pub fn page_images(&self, doc: DocId, page: usize) -> Result<Vec<Rect>, Error> {
        self.read(doc, move |d, _| {
            let page = pdf(d)?.load_pdf_page(page as i32)?;
            Ok(images(&page)?.into_iter().map(|i| i.0).collect())
        })
    }

    /// Changes an image on `page` as one undo step: takes away the image at `from`, if any,
    /// and draws `source` in `to`, if any. The same image is stretched to the new box; a file
    /// is fitted in it.
    pub fn place_image(
        &self,
        doc: DocId,
        page: usize,
        from: Option<Rect>,
        to: Option<Rect>,
        source: ImageSource,
    ) -> Result<(), Error> {
        let name = match (&from, &to, &source) {
            (None, ..) => "Add image",
            (_, None, _) => "Delete image",
            (_, _, ImageSource::File(_)) => "Replace image",
            _ => "Move image",
        };
        self.remove(doc, name, move |d| place(d, page, from, to, source))
    }
}
