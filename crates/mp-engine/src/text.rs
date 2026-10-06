use mupdf::DisplayList;
use mupdf::text_page::{TextBlockType, TextPageFlags};

use crate::{Error, Rect};

/// One character with its box, in reading order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextChar {
    pub ch: char,
    pub rect: Rect,
    /// Last character of its line; copying inserts a line break after it.
    pub line_end: bool,
}

/// The text of one page, for selection and copying.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageText {
    pub chars: Vec<TextChar>,
}

impl PageText {
    /// Plain text of `chars[range]`, with line breaks between lines.
    pub fn text(&self, range: std::ops::Range<usize>) -> String {
        let mut out = String::new();
        for c in &self.chars[range] {
            out.push(c.ch);
            if c.line_end {
                out.push('\n');
            }
        }
        out.trim_end_matches('\n').to_owned()
    }

    /// Index of the character nearest to (x, y): the closest line first, then the closest
    /// character on it. None when the page has no text.
    pub fn nearest(&self, x: f32, y: f32) -> Option<usize> {
        let line_distance = |r: &Rect| {
            if y < r.y0 {
                r.y0 - y
            } else if y > r.y1 {
                y - r.y1
            } else {
                0.0
            }
        };
        let char_distance = |r: &Rect| {
            if x < r.x0 {
                r.x0 - x
            } else if x > r.x1 {
                x - r.x1
            } else {
                0.0
            }
        };
        self.chars
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let (la, lb) = (line_distance(&a.rect), line_distance(&b.rect));
                la.total_cmp(&lb)
                    .then(char_distance(&a.rect).total_cmp(&char_distance(&b.rect)))
            })
            .map(|(i, _)| i)
    }

    /// Highlight boxes for `chars[range]`, merged per line.
    pub fn line_rects(&self, range: std::ops::Range<usize>) -> Vec<Rect> {
        let mut rects: Vec<Rect> = Vec::new();
        let mut open = false;
        for c in &self.chars[range] {
            match rects.last_mut() {
                Some(last) if open => *last = last.union(&c.rect),
                _ => rects.push(c.rect),
            }
            open = !c.line_end;
        }
        rects
    }
}

pub fn page_text(list: &DisplayList) -> Result<PageText, Error> {
    let page = list.to_text_page(TextPageFlags::empty())?;
    let mut chars = Vec::new();
    for block in page.blocks() {
        if block.r#type() != TextBlockType::Text {
            continue;
        }
        for line in block.lines() {
            let start = chars.len();
            for c in line.chars() {
                if let Some(ch) = c.char() {
                    chars.push(TextChar {
                        ch,
                        rect: c.quad().into(),
                        line_end: false,
                    });
                }
            }
            if chars.len() > start {
                chars.last_mut().expect("line has chars").line_end = true;
            }
        }
    }
    Ok(PageText { chars })
}

/// Case-insensitive search; one box per hit (multi-line hits give one box per line).
pub fn search(list: &DisplayList, needle: &str) -> Result<Vec<Rect>, Error> {
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let page = list.to_text_page(TextPageFlags::empty())?;
    Ok(page.search(needle)?.into_iter().map(Rect::from).collect())
}
