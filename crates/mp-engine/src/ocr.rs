//! Text recognition with the OCR engine built into Windows (Windows.Media.Ocr), and the
//! invisible text layer that makes scanned pages searchable, selectable and copyable.

use mupdf::pdf::{PdfDocument, PdfObject, PdfPage};
use mupdf::{Buffer, Document, Matrix};
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::core::HSTRING;

use crate::pages::pdf;
use crate::render::{PageImage, render};
use crate::text::page_text;
use crate::{DocId, Error, Rect};

/// MuPDF's GlyphLessFont (source/fitz/output-pdfocr.c): a TrueType font whose one glyph is
/// blank, and a CIDToGIDMap (zlib) that sends every 16-bit code to it. With an identity
/// ToUnicode map, codes are UTF-16 units, so any text can be laid down without a real font.
const GLYPHLESS_TTF: &[u8] = include_bytes!("../assets/glyphless.ttf");
const GLYPHLESS_CID_TO_GID: &[u8] = include_bytes!("../assets/glyphless-cidtogid.zz");
const IDENTITY_UCS: &str = "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n\
1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n\
1 beginbfrange\n<0000> <FFFF> <0000>\nendbfrange\n\
endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n";

/// Pages with at least this many letters are taken to have text already, besides those with a
/// text layer from OCR.
const HAS_TEXT: usize = 20;

/// How to recognize text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recognize {
    /// BCP 47 tag of the language, or empty for the user's own.
    pub language: String,
    /// Leave pages that hold text already.
    pub skip_text: bool,
    /// Turn pages whose text runs at a slant until it is level.
    pub deskew: bool,
}

/// A language Windows can recognize text in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrLanguage {
    /// BCP 47 tag, such as "en-US".
    pub tag: String,
    pub name: String,
}

/// One line of recognized words, with boxes in page space (points from the top left of the
/// page as it shows).
#[derive(Debug, Clone, PartialEq)]
struct Line {
    words: Vec<(String, Rect)>,
    /// The script puts spaces between words.
    spaced: bool,
}

fn win(e: windows::core::Error) -> Error {
    Error::Message(format!("Windows OCR: {}", e.message()))
}

/// The languages Windows has OCR for, the user's own first. Empty when none is installed.
pub fn ocr_languages() -> Vec<OcrLanguage> {
    let Ok(list) = OcrEngine::AvailableRecognizerLanguages() else {
        return Vec::new();
    };
    let mut out: Vec<OcrLanguage> = list
        .into_iter()
        .filter_map(|l| {
            Some(OcrLanguage {
                tag: l.LanguageTag().ok()?.to_string(),
                name: l.DisplayName().ok()?.to_string(),
            })
        })
        .collect();
    let own = OcrEngine::TryCreateFromUserProfileLanguages()
        .and_then(|e| e.RecognizerLanguage())
        .and_then(|l| l.LanguageTag());
    if let Ok(own) = own
        && let Some(i) = out.iter().position(|l| own == l.tag.as_str())
    {
        let l = out.remove(i);
        out.insert(0, l);
    }
    out
}

fn ocr_engine(tag: &str) -> Result<OcrEngine, Error> {
    let engine = if tag.is_empty() {
        OcrEngine::TryCreateFromUserProfileLanguages()
    } else {
        Language::CreateLanguage(&HSTRING::from(tag))
            .and_then(|l| OcrEngine::TryCreateFromLanguage(&l))
    };
    engine.map_err(|_| {
        Error::Message(
            "Windows has no text recognition for this language. Add it in Settings, under \
             Time & language > Language & region."
                .into(),
        )
    })
}

/// Recognizes the text of `image`, a render at `scale` of a page whose bounds start at
/// `origin`.
fn recognize(
    engine: &OcrEngine,
    image: &PageImage,
    scale: f32,
    origin: (f32, f32),
) -> Result<Vec<Line>, Error> {
    let mut bgra = Vec::with_capacity(image.rgb.len() / 3 * 4);
    for [r, g, b] in image.rgb.as_chunks::<3>().0 {
        bgra.extend_from_slice(&[*b, *g, *r, 255]);
    }
    let writer = DataWriter::new().map_err(win)?;
    writer.WriteBytes(&bgra).map_err(win)?;
    let bitmap = SoftwareBitmap::Create(
        BitmapPixelFormat::Bgra8,
        image.width as i32,
        image.height as i32,
    )
    .map_err(win)?;
    bitmap
        .CopyFromBuffer(&writer.DetachBuffer().map_err(win)?)
        .map_err(win)?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(win)?
        .join()
        .map_err(win)?;
    let at = |x: f32, y: f32| (origin.0 + x / scale, origin.1 + y / scale);
    let mut lines = Vec::new();
    for line in result.Lines().map_err(win)? {
        let mut words = Vec::new();
        for word in line.Words().map_err(win)? {
            let r = word.BoundingRect().map_err(win)?;
            let (x0, y0) = at(r.X, r.Y);
            let (x1, y1) = at(r.X + r.Width, r.Y + r.Height);
            words.push((
                word.Text().map_err(win)?.to_string(),
                Rect { x0, y0, x1, y1 },
            ));
        }
        let spaced = line.Text().map_err(win)?.to_string().contains(' ');
        lines.push(Line { words, spaced });
    }
    Ok(lines)
}

/// How far the lines of `image` are turned anticlockwise, in degrees, up to 15: the turn that
/// makes its rows of dark pixels sharpest. Windows' own estimate is too rough to level by.
fn skew(image: &PageImage) -> f32 {
    const STEP: usize = 3;
    let (w, h) = (image.width as usize, image.height as usize);
    let dark: Vec<(f32, f32)> = (0..h)
        .step_by(STEP)
        .flat_map(|y| (0..w).step_by(STEP).map(move |x| (x, y)))
        .filter(|&(x, y)| image.rgb[(y * w + x) * 3 + 1] < 128)
        .map(|(x, y)| (x as f32 - w as f32 / 2.0, y as f32 - h as f32 / 2.0))
        .collect();
    if dark.len() < 50 {
        return 0.0;
    }
    let bins = (w + h) / STEP + 2;
    let sharpness = |degrees: f32| {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let mut rows = vec![0u32; bins];
        for (dx, dy) in &dark {
            // The row of the point once the page is turned back clockwise.
            let row = (dx * sin + dy * cos) / STEP as f32 + bins as f32 / 2.0;
            rows[(row as usize).min(bins - 1)] += 1;
        }
        rows.iter().map(|&n| (n as f64).powi(2)).sum::<f64>()
    };
    let best = |candidates: Vec<f32>| {
        candidates
            .into_iter()
            .map(|d| (d, sharpness(d)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map_or(0.0, |(d, _)| d)
    };
    let coarse = best((-30..=30).map(|i| i as f32 * 0.5).collect());
    best((-10..=10).map(|i| coarse + i as f32 * 0.05).collect())
}

/// `image` turned anticlockwise by `degrees` about its middle, on white.
fn straighten(image: &PageImage, degrees: f32) -> PageImage {
    let (w, h) = (image.width as usize, image.height as usize);
    let (sin, cos) = degrees.to_radians().sin_cos();
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let mut rgb = vec![255; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            // The source of each pixel: its place turned clockwise back.
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let sx = cx + dx * cos - dy * sin - 0.5;
            let sy = cy + dx * sin + dy * cos - 0.5;
            if sx < 0.0 || sy < 0.0 || sx >= (w - 1) as f32 || sy >= (h - 1) as f32 {
                continue;
            }
            let (x0, y0) = (sx as usize, sy as usize);
            let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
            let at = |x: usize, y: usize, c: usize| image.rgb[(y * w + x) * 3 + c] as f32;
            for c in 0..3 {
                let top = at(x0, y0, c) * (1.0 - fx) + at(x0 + 1, y0, c) * fx;
                let bottom = at(x0, y0 + 1, c) * (1.0 - fx) + at(x0 + 1, y0 + 1, c) * fx;
                rgb[(y * w + x) * 3 + c] = (top * (1.0 - fy) + bottom * fy).round() as u8;
            }
        }
    }
    PageImage {
        width: image.width,
        height: image.height,
        rgb,
    }
}

/// Adds the Type0 GlyphLessFont to the document; returns its reference.
fn glyphless_font(pdf: &mut PdfDocument) -> Result<PdfObject, Error> {
    let name = || PdfObject::new_name("GlyphLessFont");
    let mut file = pdf.new_dict()?;
    file.dict_put("Length1", PdfObject::new_int(GLYPHLESS_TTF.len() as i32)?)?;
    let file = pdf.add_stream(&Buffer::from_bytes(GLYPHLESS_TTF)?, Some(&file), false)?;
    let mut map = pdf.new_dict()?;
    map.dict_put("Filter", PdfObject::new_name("FlateDecode")?)?;
    let map = pdf.add_stream(&Buffer::from_bytes(GLYPHLESS_CID_TO_GID)?, Some(&map), true)?;
    let to_unicode = pdf.add_stream(&Buffer::from_bytes(IDENTITY_UCS.as_bytes())?, None, false)?;

    let mut bbox = pdf.new_array()?;
    for v in [0, 0, 500, 1000] {
        bbox.array_push(PdfObject::new_int(v)?)?;
    }
    let mut descriptor = pdf.new_dict()?;
    descriptor.dict_put("Type", PdfObject::new_name("FontDescriptor")?)?;
    descriptor.dict_put("FontName", name()?)?;
    for (key, v) in [
        ("Flags", 5),
        ("ItalicAngle", 0),
        ("Ascent", 1000),
        ("Descent", -1),
        ("CapHeight", 1000),
        ("StemV", 80),
    ] {
        descriptor.dict_put(key, PdfObject::new_int(v)?)?;
    }
    descriptor.dict_put("FontBBox", bbox)?;
    descriptor.dict_put("FontFile2", file)?;

    let mut system = pdf.new_dict()?;
    system.dict_put("Registry", PdfObject::new_string("Adobe")?)?;
    system.dict_put("Ordering", PdfObject::new_string("Identity")?)?;
    system.dict_put("Supplement", PdfObject::new_int(0)?)?;
    let mut cid = pdf.new_dict()?;
    cid.dict_put("Type", PdfObject::new_name("Font")?)?;
    cid.dict_put("Subtype", PdfObject::new_name("CIDFontType2")?)?;
    cid.dict_put("BaseFont", name()?)?;
    cid.dict_put("CIDSystemInfo", system)?;
    cid.dict_put("FontDescriptor", pdf.add_object(&descriptor)?)?;
    cid.dict_put("DW", PdfObject::new_int(500)?)?;
    cid.dict_put("CIDToGIDMap", map)?;
    let mut descendants = pdf.new_array()?;
    descendants.array_push(pdf.add_object(&cid)?)?;

    let mut font = pdf.new_dict()?;
    font.dict_put("Type", PdfObject::new_name("Font")?)?;
    font.dict_put("Subtype", PdfObject::new_name("Type0")?)?;
    font.dict_put("BaseFont", name()?)?;
    font.dict_put("Encoding", PdfObject::new_name("Identity-H")?)?;
    font.dict_put("DescendantFonts", descendants)?;
    font.dict_put("ToUnicode", to_unicode)?;
    Ok(pdf.add_object(&font)?)
}

/// Lists `font` in the page's font resources under a free name, and returns the name.
fn font_resource(pdf: &PdfDocument, page: &PdfPage, font: &PdfObject) -> Result<String, Error> {
    let mut resources = page.resources()?;
    let mut fonts = match resources.get_dict("Font")? {
        Some(f) if f.is_dict()? => f.copy_dict()?,
        _ => pdf.new_dict()?,
    };
    let mut name = String::new();
    for n in 0.. {
        name = format!("OCR{n}");
        if fonts.get_dict(name.as_str())?.is_none() {
            break;
        }
    }
    fonts.dict_put(name.as_str(), font.try_clone()?)?;
    resources.dict_put("Font", fonts)?;
    Ok(name)
}

/// Whether the page uses a GlyphLessFont, as text layers from OCR tools do.
fn has_text_layer(doc: &Document, page: usize) -> Result<bool, Error> {
    let page = pdf(doc)?.load_pdf_page(page as i32)?;
    let fonts = match page.object().get_dict_inheritable("Resources")? {
        Some(resources) => resources.get_dict("Font")?,
        None => None,
    };
    let Some(fonts) = fonts else {
        return Ok(false);
    };
    for i in 0..fonts.dict_len()? as i32 {
        if let Some(font) = fonts.get_dict_val(i)?
            && let Some(base) = font.get_dict("BaseFont")?
            && base.as_name()?.ends_with(b"GlyphLessFont")
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// What recognition found on one page: the lines, and the turn (degrees anticlockwise about
/// `center`, a point of the page as it shows) that levels its content first.
struct Found {
    page: usize,
    lines: Vec<Line>,
    turn: f32,
    center: (f32, f32),
}

/// Lays each recognized word over its place on the page in invisible text (render mode 3),
/// stretched to the word's width, after turning skewed pages level.
fn add_text_layer(doc: &Document, found: &[Found]) -> Result<(), Error> {
    let mut pdf = pdf(doc)?;
    let font = glyphless_font(&mut pdf)?;
    for Found {
        page,
        lines,
        turn,
        center: (cx, cy),
    } in found
    {
        let mut p = pdf.load_pdf_page(*page as i32)?;
        let name = font_resource(&pdf, &p, &font)?;
        let ctm = p.ctm()?;
        // Page space from the page as it shows (y down, from its top left).
        let to_page = ctm.invert().ok_or(Error::Invalid("the page has no area"))?;
        if *turn != 0.0 {
            // Anticlockwise as it shows, which is clockwise with y down.
            let (sin, cos) = turn.to_radians().sin_cos();
            let level = Matrix::new(
                cos,
                -sin,
                sin,
                cos,
                cx - (cos * cx + sin * cy),
                cy - (-sin * cx + cos * cy),
            );
            let mut m = ctm.clone();
            m.concat(level);
            m.concat(to_page.clone());
            let cm = format!(
                "q {:.6} {:.6} {:.6} {:.6} {:.3} {:.3} cm\n",
                m.a, m.b, m.c, m.d, m.e, m.f
            );
            p.insert_contents(&mut pdf, cm.as_bytes(), false)?;
            p.insert_contents(&mut pdf, b"Q\n", true)?;
        }
        let mut ops = String::from("q\nBT\n3 Tr\n");
        for line in lines {
            for (i, (text, r)) in line.words.iter().enumerate() {
                let units: Vec<u16> = text.encode_utf16().collect();
                if units.is_empty() {
                    continue;
                }
                let size = (r.y1 - r.y0).max(1.0);
                // Each code is 500 units wide; stretch the word over its box.
                let stretch = 100.0 * (r.x1 - r.x0) / (units.len() as f32 * 0.5 * size);
                let mut hex: String = units.iter().map(|u| format!("{u:04X}")).collect();
                if line.spaced && i + 1 < line.words.len() {
                    hex += "0020";
                }
                let mut tm = Matrix::new(1.0, 0.0, 0.0, -1.0, r.x0, r.y1);
                tm.concat(to_page.clone());
                ops += &format!(
                    "/{name} {size:.2} Tf {:.1} Tz {:.4} {:.4} {:.4} {:.4} {:.3} {:.3} Tm <{hex}> Tj\n",
                    stretch.clamp(1.0, 10000.0),
                    tm.a,
                    tm.b,
                    tm.c,
                    tm.d,
                    tm.e,
                    tm.f
                );
            }
        }
        ops += "ET\nQ\n";
        p.insert_contents(&mut pdf, ops.as_bytes(), false)?;
    }
    Ok(())
}

impl crate::Engine {
    /// Recognizes the text of `pages` with Windows' OCR and lays it over them as invisible
    /// text, in one undoable step. `progress(done, total)` runs before each page; when it
    /// returns false, nothing is added. Returns how many pages got text.
    ///
    /// Rendering and recognition run on the calling thread; only the last step uses the
    /// engine's thread, so the document stays readable meanwhile.
    pub fn recognize_text(
        &self,
        doc: DocId,
        pages: &[usize],
        how: &Recognize,
        mut progress: impl FnMut(usize, usize) -> bool,
    ) -> Result<usize, Error> {
        let engine = ocr_engine(&how.language)?;
        let max = OcrEngine::MaxImageDimension().map_err(win)? as f32;
        let start = self.history(doc)?.position;
        let mut found = Vec::new();
        for (i, &page) in pages.iter().enumerate() {
            if !progress(i, pages.len()) {
                return Ok(0);
            }
            let list = self.display_list(doc, page)?;
            if how.skip_text
                && (page_text(&list)?
                    .chars
                    .iter()
                    .filter(|c| !c.ch.is_whitespace())
                    .count()
                    >= HAS_TEXT
                    || self.read(doc, move |d, _| has_text_layer(d, page))?)
            {
                continue;
            }
            let b = list.bounds();
            let longest = (b.x1 - b.x0).max(b.y1 - b.y0).max(1.0);
            let scale = (300.0 / 72.0f32).min(max / longest);
            let image = render(&list, scale)?;
            let origin = (b.x0, b.y0);
            let turn = if how.deskew { -skew(&image) } else { 0.0 };
            let lines = if turn.abs() >= 0.3 {
                recognize(&engine, &straighten(&image, turn), scale, origin)?
            } else {
                recognize(&engine, &image, scale, origin)?
            };
            let turn = if turn.abs() >= 0.3 { turn } else { 0.0 };
            if lines.iter().any(|l| !l.words.is_empty()) {
                let center = (
                    b.x0 + image.width as f32 / scale / 2.0,
                    b.y0 + image.height as f32 / scale / 2.0,
                );
                found.push(Found {
                    page,
                    lines,
                    turn,
                    center,
                });
            }
        }
        if !progress(pages.len(), pages.len()) || found.is_empty() {
            return Ok(0);
        }
        if self.history(doc)?.position != start {
            return Err(Error::Invalid(
                "the document changed while its text was being recognized",
            ));
        }
        let n = found.len();
        self.edit(doc, "Recognize text", move |d| add_text_layer(d, &found))?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The row of the dark pixels in column `x`.
    fn dark_row(image: &PageImage, x: usize) -> f32 {
        let w = image.width as usize;
        let rows: Vec<usize> = (0..image.height as usize)
            .filter(|&y| image.rgb[(y * w + x) * 3] < 128)
            .collect();
        rows.iter().sum::<usize>() as f32 / rows.len() as f32
    }

    #[test]
    fn straighten_levels_a_line_rising_to_the_right() {
        let (w, h) = (200usize, 100usize);
        let mut rgb = vec![255u8; w * h * 3];
        for x in 20..180 {
            let y = 60.0 - (x - 20) as f32 * 0.05;
            for dy in 0..3 {
                let i = ((y as usize + dy) * w + x) * 3;
                rgb[i..i + 3].fill(0);
            }
        }
        let image = PageImage {
            width: w as u32,
            height: h as u32,
            rgb,
        };
        assert!(dark_row(&image, 30) - dark_row(&image, 170) > 6.0);
        // The line is turned anticlockwise by atan(0.05); turning it back is clockwise.
        let tilt = (0.05f32).atan().to_degrees();
        assert!((skew(&image) - tilt).abs() < 0.2, "{}", skew(&image));
        let level = straighten(&image, -tilt);
        let (left, right) = (dark_row(&level, 40), dark_row(&level, 160));
        assert!((left - right).abs() < 1.0, "{left} {right}");
    }
}
