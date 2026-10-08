/// A rectangle in page space: PDF points, origin at the page's top-left, y growing down.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }

    pub fn union(&self, other: &Rect) -> Rect {
        Rect {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }
}

impl From<mupdf::Rect> for Rect {
    fn from(r: mupdf::Rect) -> Self {
        Rect {
            x0: r.x0,
            y0: r.y0,
            x1: r.x1,
            y1: r.y1,
        }
    }
}

impl From<mupdf::Quad> for Rect {
    fn from(q: mupdf::Quad) -> Self {
        let xs = [q.ul.x, q.ur.x, q.ll.x, q.lr.x];
        let ys = [q.ul.y, q.ur.y, q.ll.y, q.lr.y];
        Rect {
            x0: xs.iter().copied().fold(f32::INFINITY, f32::min),
            y0: ys.iter().copied().fold(f32::INFINITY, f32::min),
            x1: xs.iter().copied().fold(f32::NEG_INFINITY, f32::max),
            y1: ys.iter().copied().fold(f32::NEG_INFINITY, f32::max),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LinkTarget {
    /// A page in this document, optionally scrolled so `top` (page space) is at the top.
    Page { page: usize, top: Option<f32> },
    /// Anything outside the document: web URLs, mail, files.
    Uri(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub rect: Rect,
    pub target: LinkTarget,
}

/// One outline (bookmark) entry, flattened: children follow their parent with `depth + 1`.
#[derive(Debug, Clone, PartialEq)]
pub struct OutlineItem {
    pub title: String,
    pub depth: usize,
    pub target: Option<LinkTarget>,
}

/// A file embedded in the document.
#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    pub name: String,
    /// Uncompressed size, when the document records it.
    pub size: Option<usize>,
    /// The page of a file attached to a page as a comment; None for the document's own files.
    pub page: Option<usize>,
}

/// One row of a layers (optional content) panel.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub name: String,
    pub depth: usize,
    /// False for labels, which only group the rows below them.
    pub toggle: bool,
    pub visible: bool,
    pub locked: bool,
}
