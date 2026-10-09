//! Editing page content in place: a block of text set again with new words. The block's glyphs
//! go by a text-only redaction, so images and drawings under them stay; the new text is set
//! in the block's own font when it has every glyph, else in the closest standard font.

use std::path::PathBuf;

use mupdf::pdf::{
    InsertFontOptions, PdfAnnotationType, PdfDocument, PdfObject, PdfPage, PdfRedactImageMethod,
    PdfRedactLineArtMethod, PdfRedactOptions, PdfRedactTextMethod,
};
use mupdf::text_page::{TextBlockType, TextPageFlags};
use mupdf::{Document, Font, Matrix};

use crate::ocr::font_resource;
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

/// A block with what setting it again needs.
struct Block {
    shown: TextBlock,
    /// Each line's box, for the redaction.
    lines: Vec<Rect>,
    /// The first character's origin: where the first line starts, on its baseline.
    origin: (f32, f32),
    /// From one baseline to the next.
    leading: f32,
    color: [f32; 3],
    font: Option<Font>,
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
            origin: lines[0].2,
            leading: leading.max(size * 0.8),
            color: [r, g, b].map(|v| v as f32 / 255.0),
            font,
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

/// Chooses the font for `text` in `block` and adds it to the page's resources.
fn setter(
    pdf: &mut PdfDocument,
    page: &mut PdfPage,
    block: &Block,
    text: &str,
) -> Result<(Setter, Replaced), Error> {
    let own_name = block.font.as_ref().map_or("", |f| f.name());
    let base = own_name.rsplit('+').next().unwrap_or(own_name);
    let standard_own = STANDARD.iter().flatten().any(|&n| n == base);
    // An embedded font with every glyph is set again as itself; a standard one, which readers
    // supply, is named again rather than embedded.
    if !standard_own
        && let Some(font) = &block.font
        && has_glyphs(font, text)
        && let Ok(obj) = pdf.add_font(font)
    {
        let resource = font_resource(pdf, page, &obj, "Ed")?;
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
        let name = standard(block.font.as_ref());
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
            let resource = font_resource(pdf, page, &obj, "Ed")?;
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
    let (setter, replaced) = setter(&mut pdf, &mut page, &block, text)?;
    let size = block.shown.size;
    let limit = (block.shown.rect.x1 - block.origin.0) / size;
    let lines = wrap(&setter, text, limit * 1.02)?;
    let to_pdf = page
        .ctm()?
        .invert()
        .ok_or(Error::Invalid("the page has no area"))?;
    let [r, g, b] = block.color;
    let mut ops = format!(
        "q\n{r:.3} {g:.3} {b:.3} rg\nBT\n/{} {size:.2} Tf\n",
        setter.resource
    );
    for (i, line) in lines.iter().enumerate() {
        let y = block.origin.1 + i as f32 * block.leading;
        for (x, hex) in line {
            // Text space is y up; the page space it is placed in, y down.
            let mut tm = Matrix::new(1.0, 0.0, 0.0, -1.0, block.origin.0 + x * size, y);
            tm.concat(to_pdf.clone());
            ops += &format!(
                "{:.4} {:.4} {:.4} {:.4} {:.3} {:.3} Tm <{hex}> Tj\n",
                tm.a, tm.b, tm.c, tm.d, tm.e, tm.f
            );
        }
    }
    ops += "ET\nQ\n";
    page.wrap_contents(&mut pdf)?;
    page.insert_contents(&mut pdf, ops.as_bytes(), true)?;
    Ok(replaced)
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
}
