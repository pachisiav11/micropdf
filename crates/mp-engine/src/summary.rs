//! A comment summary: a new PDF in which each commented page sits beside a numbered list of its
//! comments, with their replies and review states, as Acrobat's Summarize Comments lays it out.
//! Numbered badges on the page show where each comment is.

use std::path::Path;

use mupdf::pdf::{PdfDocument, PdfWriteOptions};
use mupdf::{
    ColorParams, Colorspace, Device, Document, DocumentWriter, Font, Matrix, Path as Shape,
    Rect as Box, Text,
};

use crate::annots::{self, Annot, readable_date};
use crate::{Error, fonts};

const TITLE: f32 = 13.0;
const HEADING: f32 = 11.0;
const BODY: f32 = 9.0;
/// Line height over text size.
const LEADING: f32 = 1.35;
/// Space around the comment column's text, and the indent of contents and replies.
const MARGIN: f32 = 24.0;
const INDENT: f32 = 16.0;
/// Radius of the numbered badges.
const BADGE: f32 = 6.5;
/// Summary pages are at least this tall, so a short page still leaves room for its comments.
const MIN_HEIGHT: f32 = 400.0;

/// Installed fonts for the characters Arial lacks, tried in order.
const FALLBACKS: [&str; 6] = [
    "Segoe UI",
    "Segoe UI Symbol",
    "Nirmala UI",
    "Microsoft YaHei",
    "Yu Gothic",
    "Malgun Gothic",
];

const GREY: [f32; 3] = [0.4, 0.4, 0.4];
const INK: [f32; 3] = [0.1, 0.1, 0.1];
/// Badges of comments without a colour.
const BLUE: [f32; 3] = [0.1, 0.45, 0.9];

/// Fonts by preference: Arial, then the [`FALLBACKS`] that are installed, then Helvetica.
struct Fonts {
    regular: Vec<Font>,
    bold: Vec<Font>,
}

impl Fonts {
    fn load() -> Result<Fonts, Error> {
        let fallbacks: Vec<Font> = FALLBACKS.iter().filter_map(|n| fonts::family(n)).collect();
        let mut regular: Vec<Font> = fonts::family("Arial").into_iter().collect();
        let mut bold: Vec<Font> = fonts::bold_family("Arial").into_iter().collect();
        regular.extend(fallbacks.iter().cloned());
        bold.extend(fallbacks);
        // Helvetica is built in, so there is always a font. MuPDF writes it with the character
        // map of its Standard encoding, so copied text beyond ASCII comes out wrong.
        regular.push(Font::new("Helvetica")?);
        bold.push(Font::new("Helvetica-Bold")?);
        Ok(Fonts { regular, bold })
    }

    fn chain(&self, bold: bool) -> &[Font] {
        if bold { &self.bold } else { &self.regular }
    }
}

/// The first font in `chain` that has `c`, and the glyph; "?" when none does.
fn glyph(chain: &[Font], c: char) -> Result<(&Font, i32, char), Error> {
    for font in chain {
        let gid = font.encode_character(c as i32)?;
        if gid > 0 {
            return Ok((font, gid, c));
        }
    }
    Ok((&chain[0], chain[0].encode_character('?' as i32)?, '?'))
}

fn measure(chain: &[Font], s: &str, size: f32) -> Result<f32, Error> {
    let mut width = 0.0;
    for c in s.chars() {
        let (font, gid, _) = glyph(chain, c)?;
        width += font.advance_glyph(gid)? * size;
    }
    Ok(width)
}

/// Breaks `text` into lines no wider than `width`: at spaces, and inside a word that is wider
/// than a whole line.
fn wrap(chain: &[Font], text: &str, size: f32, width: f32) -> Result<Vec<String>, Error> {
    let space = measure(chain, " ", size)?;
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let (mut line, mut used) = (String::new(), 0.0);
        for word in paragraph.split(' ') {
            let w = measure(chain, word, size)?;
            if !line.is_empty() && used + space + w > width {
                lines.push(std::mem::take(&mut line));
                used = 0.0;
            }
            if !line.is_empty() {
                line.push(' ');
                used += space;
            }
            if w <= width {
                line += word;
                used += w;
                continue;
            }
            for c in word.chars() {
                let cw = measure(chain, c.encode_utf8(&mut [0; 4]), size)?;
                if !line.is_empty() && used + cw > width {
                    lines.push(std::mem::take(&mut line));
                    used = 0.0;
                }
                line.push(c);
                used += cw;
            }
        }
        lines.push(line);
    }
    Ok(lines)
}

/// One line of the comment column.
struct Line {
    text: String,
    size: f32,
    bold: bool,
    color: [f32; 3],
    indent: f32,
    /// Extra space above the line.
    gap: f32,
    /// The numbered badge before a comment's first line.
    badge: Option<(usize, [f32; 3])>,
}

impl Line {
    fn height(&self) -> f32 {
        self.gap + self.size * LEADING
    }
}

/// Adds `text`, wrapped to the column, as lines that share a style. Only the first gets `gap`
/// and `badge`.
#[allow(clippy::too_many_arguments)]
fn push(
    out: &mut Vec<Line>,
    fonts: &Fonts,
    width: f32,
    text: &str,
    size: f32,
    bold: bool,
    color: [f32; 3],
    indent: f32,
    gap: f32,
    badge: Option<(usize, [f32; 3])>,
) -> Result<(), Error> {
    for (i, text) in wrap(fonts.chain(bold), text, size, width - indent)?
        .into_iter()
        .enumerate()
    {
        out.push(Line {
            text,
            size,
            bold,
            color,
            indent,
            gap: if i == 0 { gap } else { 0.0 },
            badge: if i == 0 { badge } else { None },
        });
    }
    Ok(())
}

/// `text`, then when `a` last changed: "Note by Ada, 2026-10-08 16:55". Plain words keep the
/// text right when copied: MuPDF maps a glyph back to one character, and Arial's middle dot
/// also stands for U+A78F.
fn dated(text: String, a: &Annot) -> String {
    match readable_date(&a.modified) {
        Some(date) => format!("{text}, {date}"),
        None => text,
    }
}

/// The page's comments with their replies, numbered from `first`, as column lines.
fn comment_lines(
    fonts: &Fonts,
    width: f32,
    top: &[&Annot],
    all: &[Annot],
    first: usize,
) -> Result<Vec<Line>, Error> {
    let mut out = Vec::new();
    for (i, a) in top.iter().enumerate() {
        let badge = (first + i, a.color.unwrap_or(BLUE));
        let head = if a.author.is_empty() {
            a.kind.name().to_owned()
        } else {
            format!("{} by {}", a.kind.name(), a.author)
        };
        let head = dated(head, a);
        push(
            &mut out,
            fonts,
            width,
            &head,
            BODY,
            true,
            INK,
            INDENT,
            BODY * 0.6,
            Some(badge),
        )?;
        if !a.contents.trim().is_empty() {
            push(
                &mut out,
                fonts,
                width,
                &a.contents,
                BODY,
                false,
                INK,
                INDENT,
                0.0,
                None,
            )?;
        }
        for r in all.iter().filter(|r| r.reply_to == Some(a.id)) {
            match &r.state {
                Some(state) => {
                    let line = format!("{} set the status to {state}", fallback(&r.author));
                    push(
                        &mut out,
                        fonts,
                        width,
                        &dated(line, r),
                        BODY,
                        false,
                        GREY,
                        INDENT * 2.0,
                        BODY * 0.3,
                        None,
                    )?;
                }
                None => {
                    let head = dated(fallback(&r.author).to_owned(), r);
                    push(
                        &mut out,
                        fonts,
                        width,
                        &head,
                        BODY,
                        true,
                        GREY,
                        INDENT * 2.0,
                        BODY * 0.3,
                        None,
                    )?;
                    push(
                        &mut out,
                        fonts,
                        width,
                        &r.contents,
                        BODY,
                        false,
                        INK,
                        INDENT * 2.0,
                        0.0,
                        None,
                    )?;
                }
            }
        }
    }
    Ok(out)
}

fn fallback(author: &str) -> &str {
    if author.is_empty() { "Someone" } else { author }
}

/// Shows `s` with its baseline at (x, y).
fn show(
    text: &mut Text,
    chain: &[Font],
    s: &str,
    size: f32,
    (mut x, y): (f32, f32),
) -> Result<(), Error> {
    for c in s.chars() {
        let (font, gid, c) = glyph(chain, c)?;
        let trm = Matrix::new(size, 0.0, 0.0, -size, x, y);
        text.show_glyph(font, &trm, gid, c as i32)?;
        x += font.advance_glyph(gid)? * size;
    }
    Ok(())
}

fn fill_text(dev: &Device, text: &Text, color: [f32; 3]) -> Result<(), Error> {
    let rgb = Colorspace::device_rgb();
    dev.fill_text(
        text,
        &Matrix::IDENTITY,
        &rgb,
        &color,
        1.0,
        ColorParams::default(),
    )?;
    Ok(())
}

/// A filled circle with `number` in it, centred on (x, y).
fn badge(
    dev: &Device,
    fonts: &Fonts,
    number: usize,
    color: [f32; 3],
    (x, y): (f32, f32),
) -> Result<(), Error> {
    // Four Bézier arcs; 0.5523 puts their middles on the circle.
    let (r, k) = (BADGE, BADGE * 0.5523);
    let mut circle = Shape::new()?;
    circle.move_to(x + r, y)?;
    circle.curve_to(x + r, y + k, x + k, y + r, x, y + r)?;
    circle.curve_to(x - k, y + r, x - r, y + k, x - r, y)?;
    circle.curve_to(x - r, y - k, x - k, y - r, x, y - r)?;
    circle.curve_to(x + k, y - r, x + r, y - k, x + r, y)?;
    circle.close()?;
    let rgb = Colorspace::device_rgb();
    dev.fill_path(
        &circle,
        false,
        &Matrix::IDENTITY,
        &rgb,
        &color,
        1.0,
        ColorParams::default(),
    )?;
    // White on light colours is hard to read; those get dark numbers.
    let light = color[0] * 0.3 + color[1] * 0.59 + color[2] * 0.11 > 0.6;
    let ink = if light { INK } else { [1.0; 3] };
    let label = number.to_string();
    let size = if number < 100 { 7.5 } else { 6.0 };
    let width = measure(fonts.chain(true), &label, size)?;
    let mut text = Text::new()?;
    show(
        &mut text,
        fonts.chain(true),
        &label,
        size,
        (x - width / 2.0, y + size * 0.35),
    )?;
    fill_text(dev, &text, ink)
}

/// Writes the summary of `doc`'s comments to `target`; `file` names the document in the title.
/// Returns how many comments it lists, not counting replies.
pub fn write(doc: &Document, file: &str, target: &Path) -> Result<usize, Error> {
    let fonts = Fonts::load()?;
    let mut pages = Vec::new();
    let mut count = 0;
    for page_no in 0..doc.page_count()? {
        let all = annots::list(doc, page_no as usize)?;
        let mut top: Vec<&Annot> = all.iter().filter(|a| a.reply_to.is_none()).collect();
        if top.is_empty() {
            continue;
        }
        top.sort_by(|a, b| {
            a.rect
                .y0
                .total_cmp(&b.rect.y0)
                .then(a.rect.x0.total_cmp(&b.rect.x0))
        });
        count += top.len();
        let ids: Vec<i32> = top.iter().map(|a| a.id).collect();
        pages.push((page_no, all, ids));
    }
    if count == 0 {
        return Err(Error::Invalid("the document has no comments"));
    }

    let name = target
        .file_name()
        .ok_or(Error::Invalid("no file name"))?
        .to_string_lossy();
    // MuPDF writes the pages first, then the subset copy goes to `target`.
    let draft = target.with_file_name(format!("{name}.part"));
    let draft_str = draft.to_str().ok_or(Error::Invalid("path is not UTF-8"))?;
    let target_str = target.to_str().ok_or(Error::Invalid("path is not UTF-8"))?;
    let result = (|| {
        {
            let mut writer = DocumentWriter::new(draft_str, "pdf", "")?;
            let mut number = 1;
            for (index, (page_no, all, ids)) in pages.iter().enumerate() {
                let top: Vec<&Annot> = ids
                    .iter()
                    .filter_map(|id| all.iter().find(|a| a.id == *id))
                    .collect();
                let title = (index == 0).then(|| {
                    let s = if count == 1 { "" } else { "s" };
                    (
                        format!("Comments on {file}"),
                        format!("{count} comment{s} on {} page{}", pages.len(), {
                            if pages.len() == 1 { "" } else { "s" }
                        }),
                    )
                });
                summary_page(&mut writer, doc, &fonts, *page_no, &top, all, number, title)?;
                number += top.len();
            }
        }
        let mut out = PdfDocument::open(draft_str)?;
        out.subset_fonts()?;
        let mut options = PdfWriteOptions::default();
        options.set_garbage_level(1).set_compress(true);
        out.save_with_options(target_str, options)?;
        Ok(count)
    })();
    let _ = std::fs::remove_file(&draft);
    result
}

/// Writes one commented page and its column, and more pages if the comments run over.
#[allow(clippy::too_many_arguments)]
fn summary_page(
    writer: &mut DocumentWriter,
    doc: &Document,
    fonts: &Fonts,
    page_no: i32,
    top: &[&Annot],
    all: &[Annot],
    first: usize,
    title: Option<(String, String)>,
) -> Result<(), Error> {
    let page = doc.load_page(page_no)?;
    let bounds = page.bounds()?;
    let list = page.to_display_list(true)?;
    let (w, h) = (bounds.x1 - bounds.x0, bounds.y1 - bounds.y0);
    let column = (w * 0.75).clamp(260.0, 420.0);
    let height = h.max(MIN_HEIGHT);
    let media = Box::new(0.0, 0.0, w + column, height);
    let text_width = column - MARGIN * 2.0;

    let mut lines = Vec::new();
    if let Some((title, subtitle)) = &title {
        push(
            &mut lines, fonts, text_width, title, TITLE, true, INK, 0.0, 0.0, None,
        )?;
        push(
            &mut lines, fonts, text_width, subtitle, BODY, false, GREY, 0.0, 0.0, None,
        )?;
    }
    let heading_gap = if title.is_some() { HEADING } else { 0.0 };
    let heading = format!("Page {}", page_no + 1);
    let comments = comment_lines(fonts, text_width, top, all, first)?;

    let bottom = height - MARGIN;
    let mut rest = comments.as_slice();
    let mut continued = false;
    loop {
        let dev = writer.begin_page(media)?;
        let shift = Matrix::new_translate(-bounds.x0, -bounds.y0);
        list.run(&dev, &shift, media)?;
        let rgb = Colorspace::device_rgb();
        let mut panel = Shape::new()?;
        panel.rect(w, 0.0, w + column, height)?;
        // Below a page shorter than the summary page.
        if height > h {
            panel.rect(0.0, h, w, height)?;
        }
        dev.fill_path(
            &panel,
            false,
            &Matrix::IDENTITY,
            &rgb,
            &[0.965, 0.965, 0.97],
            1.0,
            ColorParams::default(),
        )?;
        for (i, a) in top.iter().enumerate() {
            let at = (
                (a.rect.x0 - bounds.x0).max(BADGE),
                (a.rect.y0 - bounds.y0).max(BADGE),
            );
            badge(&dev, fonts, first + i, a.color.unwrap_or(BLUE), at)?;
        }

        let x = w + MARGIN;
        let mut y = MARGIN;
        let mut head = Vec::new();
        if !continued {
            head.extend(lines.iter());
        }
        let label = if continued {
            format!("{heading} (continued)")
        } else {
            heading.clone()
        };
        let mut heading_lines = Vec::new();
        push(
            &mut heading_lines,
            fonts,
            text_width,
            &label,
            HEADING,
            true,
            INK,
            0.0,
            if continued { 0.0 } else { heading_gap },
            None,
        )?;
        head.extend(heading_lines.iter());
        for line in head {
            y = draw_line(&dev, fonts, line, x, y)?;
        }
        // Always at least one comment line per page, so a page that is too short still ends.
        let mut taken = 0;
        for line in rest {
            if taken > 0 && y + line.height() > bottom {
                break;
            }
            y = draw_line(&dev, fonts, line, x, y)?;
            taken += 1;
        }
        writer.end_page(dev)?;
        rest = &rest[taken..];
        if rest.is_empty() {
            return Ok(());
        }
        continued = true;
    }
}

/// Draws `line` below `y`, its left edge at `x`, and returns the y below it.
fn draw_line(dev: &Device, fonts: &Fonts, line: &Line, x: f32, y: f32) -> Result<f32, Error> {
    let top = y + line.gap;
    let baseline = top + line.size;
    if let Some((number, color)) = line.badge {
        badge(
            dev,
            fonts,
            number,
            color,
            (x + BADGE, baseline - line.size * 0.35),
        )?;
    }
    if !line.text.is_empty() {
        let mut text = Text::new()?;
        show(
            &mut text,
            fonts.chain(line.bold),
            &line.text,
            line.size,
            (x + line.indent, baseline),
        )?;
        fill_text(dev, &text, line.color)?;
    }
    Ok(top + line.size * LEADING)
}
