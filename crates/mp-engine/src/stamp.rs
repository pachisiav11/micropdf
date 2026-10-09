//! Text set on pages: headers and footers, watermarks, backgrounds and Bates numbers. The
//! text is Helvetica in WinAnsiEncoding, so it shows Western European text (Latin-1, plus
//! dashes, curly quotes, the euro sign and a few more); other characters come out as "?".

use mupdf::pdf::{InsertFontOptions, PdfDocument, PdfObject, PdfPage};
use mupdf::{Document, Font, Matrix};

use crate::Error;
use crate::pages::pdf;

/// Where a text goes on the page as it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    TopLeft,
    Top,
    TopRight,
    Center,
    BottomLeft,
    Bottom,
    BottomRight,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bates {
    pub prefix: String,
    pub suffix: String,
    pub start: u64,
    /// The number is padded with zeros to this many digits.
    pub digits: usize,
}

impl Bates {
    pub fn number(&self, n: u64) -> String {
        format!(
            "{}{:0width$}{}",
            self.prefix,
            n,
            self.suffix,
            width = self.digits
        )
    }
}

/// Texts to set on pages. In the texts, {page} is the page number, {pages} the page count,
/// {date} today's date and {bates} the Bates number.
#[derive(Debug, Clone, PartialEq)]
pub struct Overlay {
    pub texts: Vec<(Place, String)>,
    /// Points.
    pub size: f32,
    pub color: [f32; 3],
    /// 0 (invisible) to 1.
    pub opacity: f32,
    /// Degrees, counterclockwise as the page shows; for Center only.
    pub angle: f32,
    /// Under the page's content, as a background, instead of over it.
    pub behind: bool,
    /// From the page's edge to the text, in points.
    pub margin: f32,
    pub bates: Option<Bates>,
    pub date: String,
}

impl Default for Overlay {
    fn default() -> Self {
        Overlay {
            texts: Vec::new(),
            size: 10.0,
            color: [0.0, 0.0, 0.0],
            opacity: 1.0,
            angle: 0.0,
            behind: false,
            margin: 36.0,
            bates: None,
            date: String::new(),
        }
    }
}

/// Sets `overlay` on `pages`; returns the Bates number after the last one used.
pub(crate) fn apply(doc: &Document, pages: &[usize], overlay: &Overlay) -> Result<u64, Error> {
    let mut pdf = pdf(doc)?;
    let count = pdf.page_count()? as usize;
    let helvetica = Font::new("Helvetica")?;
    let mut bates = overlay.bates.as_ref().map_or(0, |b| b.start);
    for &p in pages.iter().filter(|&&p| p < count) {
        let fill = |text: &str| {
            text.replace("{page}", &(p + 1).to_string())
                .replace("{pages}", &count.to_string())
                .replace("{date}", &overlay.date)
                .replace(
                    "{bates}",
                    &overlay
                        .bates
                        .as_ref()
                        .map(|b| b.number(bates))
                        .unwrap_or_default(),
                )
        };
        let texts: Vec<(Place, String)> = overlay
            .texts
            .iter()
            .map(|(place, text)| (*place, fill(text)))
            .filter(|(_, text)| !text.trim().is_empty())
            .collect();
        bates += 1;
        if texts.is_empty() {
            continue;
        }
        let mut page = pdf.load_pdf_page(p as i32)?;
        stamp(&mut pdf, &mut page, &helvetica, overlay, &texts)?;
    }
    Ok(bates)
}

fn stamp(
    pdf: &mut PdfDocument,
    page: &mut PdfPage,
    helvetica: &Font,
    overlay: &Overlay,
    texts: &[(Place, String)],
) -> Result<(), Error> {
    let (font, xref, _) = page.insert_font(pdf, &InsertFontOptions::new("Helvetica"))?;
    // MuPDF leaves Helvetica in its StandardEncoding, which has no accented letters.
    if let Some(mut dict) = pdf.xref_object(xref)? {
        dict.dict_put("Encoding", PdfObject::new_name("WinAnsiEncoding")?)?;
    }
    let opacity = overlay.opacity.clamp(0.0, 1.0);
    let state = page.register_ext_gstate(pdf, None, (opacity < 1.0).then_some(opacity))?;
    let bounds = page.bounds()?;
    let (w, h) = (bounds.width(), bounds.height());
    // Page space from the page as it shows (y down, from its top left).
    let to_page = page
        .ctm()?
        .invert()
        .ok_or(Error::Invalid("the page has no area"))?;
    let (size, m) = (overlay.size.max(1.0), overlay.margin.max(0.0));
    let [r, g, b] = overlay.color;
    let mut ops = String::from("q\n");
    if let Some(state) = state {
        ops += &format!("{state} gs\n");
    }
    ops += &format!("{r:.3} {g:.3} {b:.3} rg\nBT\n{font} {size:.2} Tf\n");
    for (place, text) in texts {
        let bytes = win_ansi(text);
        let width = text
            .chars()
            .map(|c| {
                helvetica
                    .encode_character(c as i32)
                    .and_then(|g| helvetica.advance_glyph(g))
            })
            .sum::<Result<f32, _>>()?
            * size;
        let angle = if *place == Place::Center {
            overlay.angle.to_radians()
        } else {
            0.0
        };
        let (cos, sin) = (angle.cos(), angle.sin());
        // Text space (y up) to the shown page: x along the angle, y up the page.
        let (a, b, c, d) = (cos, -sin, -sin, -cos);
        let left = |x: f32, y: f32| (x, y);
        let (x, y) = match place {
            Place::TopLeft => left(m, m + size * 0.75),
            Place::Top => ((w - width) / 2.0, m + size * 0.75),
            Place::TopRight => (w - m - width, m + size * 0.75),
            Place::BottomLeft => left(m, h - m),
            Place::Bottom => ((w - width) / 2.0, h - m),
            Place::BottomRight => (w - m - width, h - m),
            Place::Center => {
                // The text's middle, half its width along and a third of its size up.
                let (u, v) = (width / 2.0, size * 0.35);
                (w / 2.0 - (a * u + c * v), h / 2.0 - (b * u + d * v))
            }
        };
        let mut tm = Matrix::new(a, b, c, d, bounds.x0 + x, bounds.y0 + y);
        tm.concat(to_page.clone());
        let hex: String = bytes.iter().map(|c| format!("{c:02X}")).collect();
        ops += &format!(
            "{:.4} {:.4} {:.4} {:.4} {:.3} {:.3} Tm <{hex}> Tj\n",
            tm.a, tm.b, tm.c, tm.d, tm.e, tm.f
        );
    }
    ops += "ET\nQ\n";
    if !overlay.behind {
        // The page's own content may leave its graphics state changed; set it back first.
        page.wrap_contents(pdf)?;
    }
    page.insert_contents(pdf, ops.as_bytes(), !overlay.behind)?;
    Ok(())
}

/// WinAnsiEncoding's characters from 0x80 to 0x9F; the rest of its range is Latin-1.
const WIN_ANSI_HIGH: [(u8, char); 27] = [
    (0x80, '€'),
    (0x82, '‚'),
    (0x83, 'ƒ'),
    (0x84, '„'),
    (0x85, '…'),
    (0x86, '†'),
    (0x87, '‡'),
    (0x88, 'ˆ'),
    (0x89, '‰'),
    (0x8A, 'Š'),
    (0x8B, '‹'),
    (0x8C, 'Œ'),
    (0x8E, 'Ž'),
    (0x91, '‘'),
    (0x92, '’'),
    (0x93, '“'),
    (0x94, '”'),
    (0x95, '•'),
    (0x96, '–'),
    (0x97, '—'),
    (0x98, '˜'),
    (0x99, '™'),
    (0x9A, 'š'),
    (0x9B, '›'),
    (0x9C, 'œ'),
    (0x9E, 'ž'),
    (0x9F, 'Ÿ'),
];

fn win_ansi(text: &str) -> Vec<u8> {
    text.chars()
        .map(|ch| match ch as u32 {
            0x20..=0x7E | 0xA0..=0xFF => ch as u8,
            _ => WIN_ANSI_HIGH
                .iter()
                .find(|(_, c)| *c == ch)
                .map_or(b'?', |(b, _)| *b),
        })
        .collect()
}

impl crate::Engine {
    /// Sets `overlay` on `pages` as one undo step called `name`; returns the Bates number
    /// after the last one used.
    pub fn stamp_pages(
        &self,
        doc: crate::DocId,
        pages: Vec<usize>,
        overlay: Overlay,
        name: &'static str,
    ) -> Result<u64, Error> {
        self.edit(doc, name, move |d| apply(d, &pages, &overlay))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_win_ansi() {
        assert_eq!(win_ansi("A€–é中"), b"A\x80\x96\xE9?");
    }

    #[test]
    fn bates_numbers_are_padded() {
        let b = Bates {
            prefix: "ACME-".into(),
            suffix: "".into(),
            start: 7,
            digits: 6,
        };
        assert_eq!(b.number(7), "ACME-000007");
    }
}
