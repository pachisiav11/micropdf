//! Organize: rotate, delete, move, copy, insert, replace, extract, split, combine, crop and
//! page labels. Pages count from 0.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use mupdf::pdf::{
    InsertPdfOptions, InsertPosition, PageLabelRule, PageLabelStyle, PageSelection, PdfDocument,
    PdfWriteOptions,
};
use mupdf::{Document, Rect};

use crate::{DocId, Error};

pub(crate) fn pdf(doc: &Document) -> Result<PdfDocument, Error> {
    PdfDocument::try_from(doc.clone()).map_err(|_| Error::NotPdf)
}

fn count(pdf: &PdfDocument) -> Result<usize, Error> {
    Ok(pdf.page_count()? as usize)
}

/// The pages of `pages` that exist, each once, in order.
fn valid(pdf: &PdfDocument, pages: &[usize]) -> Result<Vec<usize>, Error> {
    let n = count(pdf)?;
    let set: BTreeSet<usize> = pages.iter().copied().filter(|&p| p < n).collect();
    if set.is_empty() {
        return Err(Error::Invalid("no such pages"));
    }
    Ok(set.into_iter().collect())
}

pub(crate) fn rotate(doc: &Document, pages: &[usize], by: i32) -> Result<(), Error> {
    let pdf = pdf(doc)?;
    for p in valid(&pdf, pages)? {
        let mut page = pdf.load_pdf_page(p as i32)?;
        let now = page.rotation()?;
        page.set_rotation((now + by).rem_euclid(360))?;
    }
    Ok(())
}

pub(crate) fn delete(doc: &Document, pages: &[usize]) -> Result<(), Error> {
    let mut pdf = pdf(doc)?;
    let pages = valid(&pdf, pages)?;
    if pages.len() == count(&pdf)? {
        return Err(Error::Invalid("a document needs at least one page"));
    }
    Ok(pdf.delete_pages(PageSelection::Pages(pages))?)
}

/// Moves `pages`, keeping their order, to just before page `before` (`before` past the last
/// page moves them to the end). Returns where the first of them is now.
pub(crate) fn move_to(doc: &Document, pages: &[usize], before: usize) -> Result<usize, Error> {
    let mut pdf = pdf(doc)?;
    let moving = valid(&pdf, pages)?;
    let n = count(&pdf)?;
    let mut order: Vec<usize> = (0..n).filter(|p| !moving.contains(p)).collect();
    let at = (0..before.min(n)).filter(|p| !moving.contains(p)).count();
    order.splice(at..at, moving);
    rearrange(&mut pdf, &order)?;
    Ok(at)
}

/// Puts the pages in `order`, a permutation of the page numbers.
fn rearrange(pdf: &mut PdfDocument, order: &[usize]) -> Result<(), Error> {
    // `now[i]` is the original number of the page at i; pages before `target` are done.
    let mut now: Vec<usize> = (0..order.len()).collect();
    for (target, page) in order.iter().enumerate() {
        let from = now.iter().position(|p| p == page).expect("a permutation");
        if from != target {
            pdf.move_page(from, target)?;
            let p = now.remove(from);
            now.insert(target, p);
        }
    }
    Ok(())
}

/// Copies each page right after itself.
pub(crate) fn duplicate(doc: &Document, pages: &[usize]) -> Result<(), Error> {
    let mut pdf = pdf(doc)?;
    for p in valid(&pdf, pages)?.into_iter().rev() {
        pdf.duplicate_page(p)?;
    }
    Ok(())
}

/// Inserts a blank page before page `at`, the size of the page there (or of the last one).
pub(crate) fn insert_blank(doc: &Document, at: usize) -> Result<(), Error> {
    let mut pdf = pdf(doc)?;
    let n = count(&pdf)?;
    let like = pdf
        .load_pdf_page(at.min(n.saturating_sub(1)) as i32)?
        .bounds()?;
    pdf.new_page_at(at.min(n) as i32, (like.width(), like.height()))?;
    Ok(())
}

fn open(path: &Path, password: &str) -> Result<PdfDocument, Error> {
    let name = path.to_str().ok_or(Error::Invalid("path is not UTF-8"))?;
    let mut src = PdfDocument::open(name)?;
    if src.needs_password()? && !src.authenticate(password)? {
        return Err(Error::Message(format!(
            "{} needs a password",
            path.file_name().unwrap_or_default().to_string_lossy()
        )));
    }
    Ok(src)
}

fn insert(
    pdf: &mut PdfDocument,
    src: &PdfDocument,
    pages: PageSelection,
    at: usize,
) -> Result<usize, Error> {
    let target = if at >= count(pdf)? {
        InsertPosition::Append
    } else {
        InsertPosition::Before(at)
    };
    let options = InsertPdfOptions {
        source_pages: pages,
        target,
        ..Default::default()
    };
    Ok(pdf.insert_pdf(src, options)?.page_count)
}

/// Inserts every page of the PDF at `path` before page `at`; returns how many.
pub(crate) fn insert_file(
    doc: &Document,
    at: usize,
    path: &Path,
    password: &str,
) -> Result<usize, Error> {
    let mut pdf = pdf(doc)?;
    let src = open(path, password)?;
    insert(&mut pdf, &src, PageSelection::All, at)
}

/// Replaces `pages` with the first pages of the PDF at `path`, in order; returns how many
/// were replaced.
pub(crate) fn replace(
    doc: &Document,
    pages: &[usize],
    path: &Path,
    password: &str,
) -> Result<usize, Error> {
    let mut pdf = pdf(doc)?;
    let src = open(path, password)?;
    let pages = valid(&pdf, pages)?;
    let n = pages.len().min(count(&src)?);
    for (i, &p) in pages.iter().take(n).enumerate() {
        insert(&mut pdf, &src, PageSelection::Pages(vec![i]), p)?;
        pdf.delete_pages(PageSelection::Pages(vec![p + 1]))?;
    }
    Ok(n)
}

fn write(out: &PdfDocument, target: &Path) -> Result<(), Error> {
    let mut options = PdfWriteOptions::default();
    options.set_garbage_level(3).set_compress(true);
    let target = target.to_str().ok_or(Error::Invalid("path is not UTF-8"))?;
    Ok(out.save_with_options(target, options)?)
}

/// Writes `pages` to a new PDF at `target`.
pub(crate) fn extract(doc: &Document, pages: &[usize], target: &Path) -> Result<(), Error> {
    let pdf = pdf(doc)?;
    let pages = valid(&pdf, pages)?;
    let mut out = PdfDocument::new();
    insert(&mut out, &pdf, PageSelection::Pages(pages), 0)?;
    write(&out, target)
}

/// Writes each group of pages to its own file: `<stem>-1.pdf`, `<stem>-2.pdf` and so on
/// beside `first`. Returns the files written.
pub(crate) fn split(
    doc: &Document,
    groups: &[Vec<usize>],
    first: &Path,
) -> Result<Vec<PathBuf>, Error> {
    let stem = first
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut written = Vec::new();
    for (i, group) in groups.iter().enumerate() {
        let target = first.with_file_name(format!("{stem}-{}.pdf", i + 1));
        extract(doc, group, &target)?;
        written.push(target);
    }
    Ok(written)
}

/// Writes the PDFs at `sources`, one after another, to a new PDF at `target`.
pub fn combine(sources: &[PathBuf], target: &Path) -> Result<(), Error> {
    let mut out = PdfDocument::new();
    for path in sources {
        let src = open(path, "")?;
        let at = count(&out)?;
        insert(&mut out, &src, PageSelection::All, at)?;
    }
    write(&out, target)
}

/// Trims `margins` (left, top, right, bottom, in points, as the page shows) off each page.
pub(crate) fn crop(doc: &Document, pages: &[usize], margins: [f32; 4]) -> Result<(), Error> {
    let pdf = pdf(doc)?;
    for p in valid(&pdf, pages)? {
        let mut page = pdf.load_pdf_page(p as i32)?;
        // The page's own sides, from the sides as shown: a page turned 90 degrees shows its
        // left side at the top.
        let turns = (page.rotation()?.rem_euclid(360) / 90) as usize;
        let m: [f32; 4] = std::array::from_fn(|i| margins[(i + turns) % 4].max(0.0));
        let b = page.crop_box()?;
        let r = Rect::new(b.x0 + m[0], b.y0 + m[1], b.x1 - m[2], b.y1 - m[3]);
        if r.x1 - r.x0 < 9.0 || r.y1 - r.y0 < 9.0 {
            return Err(Error::Invalid("the margins leave nothing of the page"));
        }
        page.set_crop_box(r)?;
    }
    Ok(())
}

/// How page labels count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelStyle {
    /// The prefix alone.
    None,
    Decimal,
    UpperRoman,
    LowerRoman,
    UpperAlpha,
    LowerAlpha,
}

/// Labels the pages from `from` on: `prefix` and then `start`, `start + 1`... in `style`,
/// until the next labelled range.
pub(crate) fn label(
    doc: &Document,
    from: usize,
    style: LabelStyle,
    prefix: &str,
    start: i32,
) -> Result<(), Error> {
    let mut pdf = pdf(doc)?;
    let style = match style {
        LabelStyle::None => PageLabelStyle::None,
        LabelStyle::Decimal => PageLabelStyle::Decimal,
        LabelStyle::UpperRoman => PageLabelStyle::UpperRoman,
        LabelStyle::LowerRoman => PageLabelStyle::LowerRoman,
        LabelStyle::UpperAlpha => PageLabelStyle::UpperAlpha,
        LabelStyle::LowerAlpha => PageLabelStyle::LowerAlpha,
    };
    let mut rule = PageLabelRule::new(from, style);
    rule.prefix = prefix.to_owned();
    rule.start = start.max(1);
    Ok(pdf.set_page_label_rule(rule)?)
}

/// Each page's label, or its number when the document has no labels.
pub(crate) fn labels(doc: &Document) -> Result<Vec<String>, Error> {
    let pdf = pdf(doc)?;
    (0..count(&pdf)?).map(|i| Ok(pdf.page_label(i)?)).collect()
}

/// Reads page ranges such as "1-3, 5, 8-" (pages from 1) into groups of pages from 0, one
/// group per comma.
pub fn parse_ranges(text: &str, pages: usize) -> Result<Vec<Vec<usize>>, Error> {
    let bad = || {
        Error::Message(format!(
            "\"{text}\" is not a list of pages such as 1-3, 5, 8-"
        ))
    };
    let number = |s: &str, default: usize| -> Result<usize, Error> {
        let s = s.trim();
        if s.is_empty() {
            return Ok(default);
        }
        match s.parse::<usize>() {
            Ok(n) if (1..=pages).contains(&n) => Ok(n),
            Ok(n) => Err(Error::Message(format!(
                "there is no page {n}; the last is {pages}"
            ))),
            Err(_) => Err(bad()),
        }
    };
    let mut groups = Vec::new();
    for part in text.split([',', ';']).filter(|p| !p.trim().is_empty()) {
        let (a, b) = match part.split_once(['-', '–']) {
            Some((a, b)) => (number(a, 1)?, number(b, pages)?),
            None => {
                let n = number(part, 0)?;
                (n, n)
            }
        };
        if a == 0 || a > b {
            return Err(bad());
        }
        groups.push((a - 1..b).collect());
    }
    if groups.is_empty() {
        return Err(bad());
    }
    Ok(groups)
}

impl crate::Engine {
    pub fn rotate_pages(&self, doc: DocId, pages: Vec<usize>, by: i32) -> Result<(), Error> {
        self.edit(doc, "Rotate pages", move |d| rotate(d, &pages, by))
    }

    pub fn delete_pages(&self, doc: DocId, pages: Vec<usize>) -> Result<(), Error> {
        self.remove(doc, "Delete pages", move |d| delete(d, &pages))
    }

    /// See [`move_to`]; returns where the first moved page is now.
    pub fn move_pages(&self, doc: DocId, pages: Vec<usize>, before: usize) -> Result<usize, Error> {
        self.edit(doc, "Move pages", move |d| move_to(d, &pages, before))
    }

    pub fn duplicate_pages(&self, doc: DocId, pages: Vec<usize>) -> Result<(), Error> {
        self.edit(doc, "Duplicate pages", move |d| duplicate(d, &pages))
    }

    pub fn insert_blank_page(&self, doc: DocId, at: usize) -> Result<(), Error> {
        self.edit(doc, "Insert blank page", move |d| insert_blank(d, at))
    }

    /// Inserts the pages of another PDF before page `at`; returns how many.
    pub fn insert_file(
        &self,
        doc: DocId,
        at: usize,
        path: PathBuf,
        password: String,
    ) -> Result<usize, Error> {
        self.edit(doc, "Insert pages", move |d| {
            insert_file(d, at, &path, &password)
        })
    }

    /// Replaces `pages` with the first pages of another PDF; returns how many.
    pub fn replace_pages(
        &self,
        doc: DocId,
        pages: Vec<usize>,
        path: PathBuf,
        password: String,
    ) -> Result<usize, Error> {
        self.remove(doc, "Replace pages", move |d| {
            replace(d, &pages, &path, &password)
        })
    }

    pub fn extract_pages(
        &self,
        doc: DocId,
        pages: Vec<usize>,
        target: PathBuf,
    ) -> Result<(), Error> {
        self.read(doc, move |d, _| extract(d, &pages, &target))
    }

    /// See [`split`].
    pub fn split(
        &self,
        doc: DocId,
        groups: Vec<Vec<usize>>,
        first: PathBuf,
    ) -> Result<Vec<PathBuf>, Error> {
        self.read(doc, move |d, _| split(d, &groups, &first))
    }

    pub fn crop_pages(
        &self,
        doc: DocId,
        pages: Vec<usize>,
        margins: [f32; 4],
    ) -> Result<(), Error> {
        self.edit(doc, "Crop pages", move |d| crop(d, &pages, margins))
    }

    pub fn label_pages(
        &self,
        doc: DocId,
        from: usize,
        style: LabelStyle,
        prefix: String,
        start: i32,
    ) -> Result<(), Error> {
        self.edit(doc, "Number pages", move |d| {
            label(d, from, style, &prefix, start)
        })
    }

    pub fn page_labels(&self, doc: DocId) -> Result<Vec<String>, Error> {
        self.read(doc, |d, _| labels(d))
    }
}

#[cfg(test)]
mod tests {
    use super::parse_ranges;

    #[test]
    fn ranges_read_as_groups() {
        assert_eq!(
            parse_ranges("1-3, 5, 8-", 9).unwrap(),
            vec![vec![0, 1, 2], vec![4], vec![7, 8]]
        );
        assert_eq!(
            parse_ranges(" -2 ;4–4", 5).unwrap(),
            vec![vec![0, 1], vec![3]]
        );
        assert!(parse_ranges("3-1", 5).is_err());
        assert!(
            parse_ranges("7", 5)
                .unwrap_err()
                .to_string()
                .contains("no page 7")
        );
        assert!(parse_ranges("a", 5).is_err());
        assert!(parse_ranges("", 5).is_err());
    }
}
