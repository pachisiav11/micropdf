//! Redaction: marking areas, text and patterns, then removing what lies under the marks.
//! Marks are Redact annotations; applying them removes the text, the image pixels and the
//! line art under them for good, and draws black boxes in their place. Only a full save
//! drops the removed content from the file, and the engine makes the next save one.

use mupdf::pdf::{
    PdfAnnotationType, PdfPage, PdfRedactImageMethod, PdfRedactLineArtMethod, PdfRedactOptions,
    PdfRedactTextMethod,
};
use mupdf::{Document, Quad};
use regex::RegexBuilder;

use crate::pages::pdf;
use crate::{Error, Rect, text};

/// Patterns that find common personal data, by name.
pub const PATTERNS: [(&str, &str); 4] = [
    ("Email addresses", r"[\w.+-]+@[\w-]+(?:\.[\w-]+)+"),
    (
        "Phone numbers",
        r"(?:\+\d{1,3}[\s.-]?)?(?:\(\d{2,5}\)[\s.-]?|\b\d{2,5}[\s.-])\d{3,4}[\s.-]?\d{3,4}\b",
    ),
    ("Card numbers", r"\b(?:\d[ -]?){12,18}\d\b"),
    ("US Social Security numbers", r"\b\d{3}-\d{2}-\d{4}\b"),
];

/// What to look for: words as typed (any case), or a regular expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Find {
    Text(String),
    Pattern(String),
}

/// Marks every match of `find` on `pages` for redaction; returns how many.
pub(crate) fn mark_matches(doc: &Document, pages: &[usize], find: &Find) -> Result<usize, Error> {
    let source = match find {
        Find::Text(text) if text.trim().is_empty() => {
            return Err(Error::Invalid("type something to find"));
        }
        Find::Text(text) => regex::escape(text.trim()),
        Find::Pattern(p) => p.clone(),
    };
    let re = RegexBuilder::new(&source)
        .case_insensitive(matches!(find, Find::Text(_)))
        .build()
        .map_err(|e| Error::Message(format!("the pattern does not read: {e}")))?;
    let pdf = pdf(doc)?;
    let count = pdf.page_count()? as usize;
    let mut marked = 0;
    for &p in pages.iter().filter(|&&p| p < count) {
        let page = doc.load_page(p as i32)?;
        let text = text::page_text(&page.to_display_list(false)?)?;
        // The page's text with a newline after each line, and the character at each byte.
        let mut flat = String::new();
        let mut at = Vec::new();
        for (i, c) in text.chars.iter().enumerate() {
            flat.push(c.ch);
            at.resize(flat.len(), i);
            if c.line_end {
                flat.push('\n');
                at.resize(flat.len(), i);
            }
        }
        let mut page = pdf.load_pdf_page(p as i32)?;
        for m in re
            .find_iter(&flat)
            .filter(|m| !m.as_str().trim().is_empty())
        {
            let rects = text.line_rects(at[m.start()]..at[m.end() - 1] + 1);
            mark(&mut page, &rects)?;
            marked += 1;
        }
    }
    Ok(marked)
}

/// Marks `rects` (page space) as one redaction.
pub(crate) fn mark(page: &mut PdfPage, rects: &[Rect]) -> Result<(), Error> {
    let quads: Vec<Quad> = rects
        .iter()
        .map(|r| Quad::from(mupdf::Rect::new(r.x0, r.y0, r.x1, r.y1)))
        .collect();
    let mut annot = page.add_redact_annotation(quads)?;
    annot.update()?;
    Ok(())
}

/// Applies every redaction mark; returns how many pages changed.
pub(crate) fn apply(doc: &Document) -> Result<usize, Error> {
    let pdf = pdf(doc)?;
    let mut changed = 0;
    for p in 0..pdf.page_count()? {
        let mut page = pdf.load_pdf_page(p)?;
        if !page
            .annotations()
            .any(|a| a.r#type().ok() == Some(PdfAnnotationType::Redact))
        {
            continue;
        }
        page.apply_redactions_with_options(PdfRedactOptions {
            black_boxes: true,
            image_method: PdfRedactImageMethod::Pixels,
            line_art: PdfRedactLineArtMethod::RemoveIfCovered,
            text: PdfRedactTextMethod::Remove,
        })?;
        changed += 1;
    }
    Ok(changed)
}

impl crate::Engine {
    /// Marks every match of `find` on `pages` for redaction; returns how many.
    pub fn mark_redactions(
        &self,
        doc: crate::DocId,
        pages: Vec<usize>,
        find: Find,
    ) -> Result<usize, Error> {
        self.edit(doc, "Mark for redaction", move |d| {
            mark_matches(d, &pages, &find)
        })
    }

    /// Marks `rects` on a page (page space) as one redaction: an area, or selected lines.
    pub fn mark_redaction(
        &self,
        doc: crate::DocId,
        page: usize,
        rects: Vec<Rect>,
    ) -> Result<(), Error> {
        self.edit(doc, "Mark for redaction", move |d| {
            mark(&mut pdf(d)?.load_pdf_page(page as i32)?, &rects)
        })
    }

    /// Removes what lies under every redaction mark; returns how many pages changed.
    pub fn apply_redactions(&self, doc: crate::DocId) -> Result<usize, Error> {
        self.remove(doc, "Apply redactions", apply)
    }
}

#[cfg(test)]
mod tests {
    use regex::Regex;

    use super::PATTERNS;

    fn hits(pattern: usize, text: &str) -> Vec<String> {
        Regex::new(PATTERNS[pattern].1)
            .unwrap()
            .find_iter(text)
            .map(|m| m.as_str().to_owned())
            .collect()
    }

    #[test]
    fn patterns_find_personal_data() {
        assert_eq!(
            hits(0, "mail jane.doe+x@mail.example.co.uk now"),
            ["jane.doe+x@mail.example.co.uk"]
        );
        assert_eq!(
            hits(1, "call (555) 123-4567 or +44 20 7946 0958"),
            ["(555) 123-4567", "+44 20 7946 0958"]
        );
        assert_eq!(
            hits(2, "card 4111 1111 1111 1111."),
            ["4111 1111 1111 1111"]
        );
        assert_eq!(hits(3, "SSN 078-05-1120"), ["078-05-1120"]);
        assert!(hits(1, "page 12 of 30").is_empty());
    }
}
