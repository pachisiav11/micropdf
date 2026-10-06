//! Page geometry for the document view, in logical pixels. Pure: no Slint, no MuPDF, so it can
//! be unit-tested.
//!
//! "Page space" is MuPDF's: PDF points, origin at the unrotated page's top-left, y down.
//! "Document space" is the scrollable area: logical pixels, origin at its top-left.

use mp_engine::Rect;
use serde::{Deserialize, Serialize};

/// Space around the document and between pages, in logical pixels.
pub const MARGIN: f32 = 24.0;
pub const GAP: f32 = 16.0;
const MIN_SCALE: f32 = 0.05;
const MAX_SCALE: f32 = 16.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PageMode {
    /// One page at a time.
    Single,
    #[default]
    Continuous,
    /// Two pages side by side, scrolling.
    TwoUp,
    /// Like TwoUp, but the first page stands alone (a cover).
    Book,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum Zoom {
    #[default]
    FitWidth,
    FitPage,
    /// Logical pixels per PDF point; 1.0 is 100%.
    Scale(f32),
}

/// Where one page sits in document space.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Frame {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Frame {
    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }
}

pub struct Params<'a> {
    /// Unrotated page sizes in points.
    pub sizes: &'a [(f32, f32)],
    pub rotation: i32,
    pub mode: PageMode,
    pub zoom: Zoom,
    pub view_width: f32,
    pub view_height: f32,
    /// The page shown in Single mode.
    pub current: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layout {
    /// Logical pixels per point.
    pub scale: f32,
    pub rotation: i32,
    /// Placement per page; None for pages not shown (Single mode).
    pub frames: Vec<Option<Frame>>,
    /// Unrotated page sizes in points, kept for coordinate conversion.
    sizes: Vec<(f32, f32)>,
    /// Page indices per row, top to bottom.
    rows: Vec<Vec<usize>>,
    pub width: f32,
    pub height: f32,
}

impl Layout {
    pub fn compute(p: &Params) -> Layout {
        let rotation = p.rotation.rem_euclid(360);
        let rotated = |i: usize| {
            let (w, h) = p.sizes[i];
            if rotation % 180 == 0 { (w, h) } else { (h, w) }
        };
        let count = p.sizes.len();
        let rows: Vec<Vec<usize>> = match p.mode {
            PageMode::Single if count > 0 => vec![vec![p.current.min(count - 1)]],
            PageMode::Single => Vec::new(),
            PageMode::Continuous => (0..count).map(|i| vec![i]).collect(),
            PageMode::TwoUp => (0..count)
                .step_by(2)
                .map(|i| (i..(i + 2).min(count)).collect())
                .collect(),
            PageMode::Book => std::iter::once(vec![0])
                .filter(|_| count > 0)
                .chain(
                    (1..count)
                        .step_by(2)
                        .map(|i| (i..(i + 2).min(count)).collect()),
                )
                .collect(),
        };

        // Row sizes in points; pages in a row sit side by side with GAP between (GAP is in
        // pixels, so it is added after scaling).
        let row_points = |row: &Vec<usize>| {
            let w: f32 = row.iter().map(|&i| rotated(i).0).sum();
            let h = row.iter().map(|&i| rotated(i).1).fold(0.0, f32::max);
            (w, h, row.len().saturating_sub(1) as f32 * GAP)
        };
        let widest = rows
            .iter()
            .map(row_points)
            .fold((1.0f32, 1.0f32, 0.0f32), |a, b| {
                (a.0.max(b.0), a.1.max(b.1), a.2.max(b.2))
            });
        let avail_w = (p.view_width - 2.0 * MARGIN - widest.2).max(1.0);
        let avail_h = (p.view_height - 2.0 * MARGIN).max(1.0);
        let scale = match p.zoom {
            Zoom::FitWidth => avail_w / widest.0,
            Zoom::FitPage => (avail_w / widest.0).min(avail_h / widest.1),
            Zoom::Scale(s) => s,
        }
        .clamp(MIN_SCALE, MAX_SCALE);

        let content_w = rows
            .iter()
            .map(|r| {
                let (w, _, gaps) = row_points(r);
                w * scale + gaps
            })
            .fold(0.0, f32::max)
            + 2.0 * MARGIN;
        let width = content_w.max(p.view_width);

        let mut frames = vec![None; count];
        let mut y = MARGIN;
        for row in &rows {
            let (w, h, gaps) = row_points(row);
            let mut x = (width - (w * scale + gaps)) / 2.0;
            for &i in row {
                let (pw, ph) = rotated(i);
                frames[i] = Some(Frame {
                    x,
                    y,
                    width: pw * scale,
                    height: ph * scale,
                });
                x += pw * scale + GAP;
            }
            y += h * scale + GAP;
        }
        let height = if rows.is_empty() {
            0.0
        } else {
            y - GAP + MARGIN
        };

        Layout {
            scale,
            rotation,
            frames,
            sizes: p.sizes.to_vec(),
            rows,
            width,
            height,
        }
    }

    pub fn frame(&self, page: usize) -> Option<Frame> {
        self.frames.get(page).copied().flatten()
    }

    /// Pages whose frames overlap the vertical band [top, bottom], in order.
    pub fn visible(&self, top: f32, bottom: f32) -> Vec<usize> {
        let first_row = self.rows.partition_point(|row| self.row_bottom(row) < top);
        self.rows[first_row..]
            .iter()
            .take_while(|row| self.row_top(row) <= bottom)
            .flatten()
            .copied()
            .collect()
    }

    /// The page the reader is looking at: the one crossing a line a third of the way down the
    /// view, else the nearest one.
    pub fn current_page(&self, top: f32, view_height: f32) -> usize {
        let line = top + view_height / 3.0;
        let mut best = (f32::INFINITY, 0);
        for (i, frame) in self.frames.iter().enumerate() {
            let Some(f) = frame else { continue };
            let distance = if line < f.y {
                f.y - line
            } else if line > f.bottom() {
                line - f.bottom()
            } else {
                0.0
            };
            if distance < best.0 {
                best = (distance, i);
            }
        }
        best.1
    }

    /// The page under a document-space point, with the point in that page's page space.
    pub fn hit(&self, x: f32, y: f32) -> Option<(usize, f32, f32)> {
        self.frames.iter().enumerate().find_map(|(i, frame)| {
            let f = (*frame)?;
            (x >= f.x && x <= f.x + f.width && y >= f.y && y <= f.bottom()).then(|| {
                let (px, py) = self.frame_to_page(i, x - f.x, y - f.y);
                (i, px, py)
            })
        })
    }

    /// Converts a page-space rectangle to document space.
    pub fn to_view(&self, page: usize, r: &Rect) -> Option<Frame> {
        let f = self.frame(page)?;
        let (ax, ay) = self.page_to_frame(page, r.x0, r.y0);
        let (bx, by) = self.page_to_frame(page, r.x1, r.y1);
        Some(Frame {
            x: f.x + ax.min(bx),
            y: f.y + ay.min(by),
            width: (ax - bx).abs(),
            height: (ay - by).abs(),
        })
    }

    /// Document-space y that puts `page` (optionally its page-space `top`) at the view's top.
    pub fn scroll_to(&self, page: usize, top: Option<f32>) -> Option<f32> {
        let f = self.frame(page)?;
        let y = match top {
            Some(t) => {
                let (w, _) = self.sizes[page];
                self.to_view(
                    page,
                    &Rect {
                        x0: 0.0,
                        y0: t,
                        x1: w,
                        y1: t,
                    },
                )?
                .y
            }
            None => f.y,
        };
        Some((y - MARGIN / 2.0).max(0.0))
    }

    /// Frame-relative logical pixels -> page space.
    fn frame_to_page(&self, page: usize, fx: f32, fy: f32) -> (f32, f32) {
        let (w, h) = self.sizes[page];
        let (rx, ry) = (fx / self.scale, fy / self.scale);
        match self.rotation {
            90 => (ry, h - rx),
            180 => (w - rx, h - ry),
            270 => (w - ry, rx),
            _ => (rx, ry),
        }
    }

    /// Page space -> frame-relative logical pixels. Matches MuPDF's rotate-then-translate.
    fn page_to_frame(&self, page: usize, x: f32, y: f32) -> (f32, f32) {
        let (w, h) = self.sizes[page];
        let (rx, ry) = match self.rotation {
            90 => (h - y, x),
            180 => (w - x, h - y),
            270 => (y, w - x),
            _ => (x, y),
        };
        (rx * self.scale, ry * self.scale)
    }

    fn row_top(&self, row: &[usize]) -> f32 {
        row.iter()
            .filter_map(|&i| self.frame(i))
            .map(|f| f.y)
            .fold(f32::INFINITY, f32::min)
    }

    fn row_bottom(&self, row: &[usize]) -> f32 {
        row.iter()
            .filter_map(|&i| self.frame(i))
            .map(|f| f.bottom())
            .fold(0.0, f32::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(sizes: &[(f32, f32)], mode: PageMode, zoom: Zoom, rotation: i32) -> Params<'_> {
        Params {
            sizes,
            rotation,
            mode,
            zoom,
            view_width: 848.0,
            view_height: 600.0,
            current: 0,
        }
    }

    const LETTER: (f32, f32) = (612.0, 792.0);

    #[test]
    fn fit_width_fills_the_view_minus_margins() {
        let sizes = [LETTER; 3];
        let l = Layout::compute(&params(&sizes, PageMode::Continuous, Zoom::FitWidth, 0));
        assert!((l.scale - 800.0 / 612.0).abs() < 1e-4);
        let f0 = l.frame(0).unwrap();
        assert_eq!((f0.x, f0.y), (MARGIN, MARGIN));
        let f1 = l.frame(1).unwrap();
        assert!((f1.y - (f0.bottom() + GAP)).abs() < 1e-3);
        assert!((l.height - (l.frame(2).unwrap().bottom() + MARGIN)).abs() < 1e-3);
    }

    #[test]
    fn fit_page_fits_the_tallest_page() {
        let sizes = [LETTER];
        let l = Layout::compute(&params(&sizes, PageMode::Continuous, Zoom::FitPage, 0));
        assert!((l.frame(0).unwrap().height - 552.0).abs() < 1e-3);
    }

    #[test]
    fn two_up_and_book_pair_pages() {
        let sizes = [LETTER; 5];
        let two = Layout::compute(&params(&sizes, PageMode::TwoUp, Zoom::Scale(0.5), 0));
        assert_eq!(two.frame(0).unwrap().y, two.frame(1).unwrap().y);
        assert!(two.frame(2).unwrap().y > two.frame(1).unwrap().y);

        let book = Layout::compute(&params(&sizes, PageMode::Book, Zoom::Scale(0.5), 0));
        assert!(book.frame(1).unwrap().y > book.frame(0).unwrap().y);
        assert_eq!(book.frame(1).unwrap().y, book.frame(2).unwrap().y);
        assert_eq!(book.frame(3).unwrap().y, book.frame(4).unwrap().y);
    }

    #[test]
    fn single_mode_places_only_the_current_page() {
        let sizes = [LETTER; 4];
        let mut p = params(&sizes, PageMode::Single, Zoom::FitPage, 0);
        p.current = 2;
        let l = Layout::compute(&p);
        assert!(l.frame(2).is_some());
        assert!(l.frame(0).is_none() && l.frame(3).is_none());
        assert_eq!(l.visible(0.0, 10_000.0), vec![2]);
    }

    #[test]
    fn rotation_swaps_frame_size() {
        let sizes = [(300.0, 200.0)];
        let l = Layout::compute(&params(&sizes, PageMode::Continuous, Zoom::Scale(1.0), 90));
        let f = l.frame(0).unwrap();
        assert_eq!((f.width, f.height), (200.0, 300.0));
    }

    #[test]
    fn page_and_view_coordinates_round_trip_for_every_rotation() {
        let sizes = [(300.0, 200.0)];
        for rotation in [0, 90, 180, 270] {
            let l = Layout::compute(&params(
                &sizes,
                PageMode::Continuous,
                Zoom::Scale(1.5),
                rotation,
            ));
            let r = Rect {
                x0: 40.0,
                y0: 30.0,
                x1: 41.0,
                y1: 31.0,
            };
            let v = l.to_view(0, &r).unwrap();
            let (page, x, y) = l.hit(v.x + v.width / 2.0, v.y + v.height / 2.0).unwrap();
            assert_eq!(page, 0);
            assert!(
                (x - 40.5).abs() < 0.01 && (y - 30.5).abs() < 0.01,
                "rotation {rotation}: {x},{y}"
            );
        }
    }

    #[test]
    fn visible_and_current_page_follow_the_scroll_position() {
        let sizes = [LETTER; 10];
        let l = Layout::compute(&params(&sizes, PageMode::Continuous, Zoom::Scale(1.0), 0));
        let f3 = l.frame(3).unwrap();
        assert_eq!(l.visible(f3.y + 10.0, f3.y + 20.0), vec![3]);
        assert_eq!(
            l.visible(f3.bottom() - 1.0, f3.bottom() + GAP + 1.0),
            vec![3, 4]
        );
        assert_eq!(l.current_page(f3.y, 600.0), 3);
        assert_eq!(l.scroll_to(3, None), Some(f3.y - MARGIN / 2.0));
    }

    #[test]
    fn empty_document_has_no_pages() {
        let l = Layout::compute(&params(&[], PageMode::Continuous, Zoom::FitWidth, 0));
        assert!(l.visible(0.0, 1000.0).is_empty());
        assert_eq!(l.height, 0.0);
    }
}
