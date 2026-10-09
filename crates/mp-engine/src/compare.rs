//! Compare: the words in which two documents differ, found by Myers' diff over their text.

use std::ops::Range;

use crate::text::{PageText, page_text};
use crate::{DocId, Error, Rect};

/// Diffs that need more edits than this give up and report the rest as one change; page by
/// page, a long document still gets a useful answer.
const MAX_EDITS: i64 = 2000;

/// A word of a page, with its box in page space.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub page: usize,
    pub text: String,
    pub rect: Rect,
}

/// A run of differing words: `old` words of the first document stand where `new` words of the
/// second do. One of the two is empty for words only removed or only added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub old: Range<usize>,
    pub new: Range<usize>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Comparison {
    pub old: Vec<Word>,
    pub new: Vec<Word>,
    pub changes: Vec<Change>,
}

fn union(a: Rect, b: Rect) -> Rect {
    Rect {
        x0: a.x0.min(b.x0),
        y0: a.y0.min(b.y0),
        x1: a.x1.max(b.x1),
        y1: a.y1.max(b.y1),
    }
}

/// The words of a page's text, in reading order, added to `out`.
fn words(page: usize, text: PageText, out: &mut Vec<Word>) {
    let mut word: Option<Word> = None;
    for c in text.chars {
        if c.ch.is_whitespace() {
            out.extend(word.take());
        } else {
            match &mut word {
                Some(w) => {
                    w.text.push(c.ch);
                    w.rect = union(w.rect, c.rect);
                }
                None => {
                    word = Some(Word {
                        page,
                        text: c.ch.to_string(),
                        rect: c.rect,
                    })
                }
            }
        }
        if c.line_end {
            out.extend(word.take());
        }
    }
    out.extend(word);
}

/// The runs in which `a` and `b` differ, by Myers' O(ND) algorithm; None when they are more
/// than [`MAX_EDITS`] edits apart.
fn myers<T: PartialEq>(a: &[T], b: &[T]) -> Option<Vec<Change>> {
    let (n, m) = (a.len() as i64, b.len() as i64);
    let limit = (n + m).min(MAX_EDITS);
    let off = limit + 1;
    let at = |k: i64| (k + off) as usize;
    let mut v = vec![0i64; (2 * off + 1) as usize];
    // trace[d]: the furthest x on each diagonal -d..=d after d edits.
    let mut trace: Vec<Vec<i64>> = Vec::new();
    let mut end = None;
    'search: for d in 0..=limit {
        for k in (-d..=d).step_by(2) {
            let mut x = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) {
                v[at(k + 1)]
            } else {
                v[at(k - 1)] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[at(k)] = x;
            if x >= n && y >= m {
                end = Some(d);
                break 'search;
            }
        }
        trace.push(v[at(-d)..=at(d)].to_vec());
    }
    // Walk back from the end; each step back is one deletion or insertion, at (x, y).
    let mut edits = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (1..=end?).rev() {
        let before = &trace[(d - 1) as usize];
        let x_at = |k: i64| before[(k + d - 1) as usize];
        let k = x - y;
        let prev = if k == -d || (k != d && x_at(k - 1) < x_at(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let (px, py) = (x_at(prev), x_at(prev) - prev);
        while x > px && y > py {
            x -= 1;
            y -= 1;
        }
        // Down a row is an insertion of b[py]; across a column, a deletion of a[px].
        edits.push(if x == px {
            (px, py, false)
        } else {
            (px, y, true)
        });
        (x, y) = (px, py);
    }
    let mut changes: Vec<Change> = Vec::new();
    for (x, y, deleted) in edits.into_iter().rev() {
        let (x, y) = (x as usize, y as usize);
        let joins = changes
            .last()
            .is_some_and(|c| c.old.end == x && c.new.end == y);
        if !joins {
            changes.push(Change {
                old: x..x,
                new: y..y,
            });
        }
        let c = changes.last_mut().expect("just pushed");
        if deleted {
            c.old.end += 1;
        } else {
            c.new.end += 1;
        }
    }
    Some(changes)
}

/// The runs in which `a` and `b` differ, with common ends set aside first; None when they are
/// too far apart.
fn diff<T: PartialEq>(a: &[T], b: &[T]) -> Option<Vec<Change>> {
    let head = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let tail = a[head..]
        .iter()
        .rev()
        .zip(b[head..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (a, b) = (&a[head..a.len() - tail], &b[head..b.len() - tail]);
    let changes = myers(a, b)?;
    Some(
        changes
            .into_iter()
            .map(|c| Change {
                old: c.old.start + head..c.old.end + head,
                new: c.new.start + head..c.new.end + head,
            })
            .collect(),
    )
}

/// The word changes from `old` to `new`: over the whole text when it is close enough, else
/// page against page.
pub fn compare_words(old: &[Word], new: &[Word]) -> Vec<Change> {
    fn text(w: &[Word]) -> Vec<&str> {
        w.iter().map(|w| w.text.as_str()).collect()
    }
    let (a, b) = (text(old), text(new));
    if let Some(changes) = diff(&a, &b) {
        return changes;
    }
    let pages = old
        .last()
        .map_or(0, |w| w.page + 1)
        .max(new.last().map_or(0, |w| w.page + 1));
    let range = |words: &[Word], page: usize| {
        words.partition_point(|w| w.page < page)..words.partition_point(|w| w.page <= page)
    };
    let mut changes = Vec::new();
    for page in 0..pages {
        let (ra, rb) = (range(old, page), range(new, page));
        let page = diff(&a[ra.clone()], &b[rb.clone()]).unwrap_or_else(|| {
            vec![Change {
                old: 0..ra.len(),
                new: 0..rb.len(),
            }]
        });
        changes.extend(page.into_iter().map(|c| Change {
            old: c.old.start + ra.start..c.old.end + ra.start,
            new: c.new.start + rb.start..c.new.end + rb.start,
        }));
    }
    changes
}

impl crate::Engine {
    /// Every word of the document, in reading order.
    pub fn words(&self, doc: DocId) -> Result<Vec<Word>, Error> {
        let mut out = Vec::new();
        for page in 0..self.page_sizes(doc)?.len() {
            let list = self.display_list(doc, page)?;
            words(page, page_text(&list)?, &mut out);
        }
        Ok(out)
    }

    /// The words of `old` and `new` and how they differ. Runs on the calling thread but for
    /// reading the pages.
    pub fn compare(&self, old: DocId, new: DocId) -> Result<Comparison, Error> {
        let (old, new) = (self.words(old)?, self.words(new)?);
        let changes = compare_words(&old, &new);
        Ok(Comparison { old, new, changes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changes(a: &str, b: &str) -> Vec<(String, String)> {
        let (a, b): (Vec<&str>, Vec<&str>) = (a.split(' ').collect(), b.split(' ').collect());
        diff(&a, &b)
            .unwrap()
            .into_iter()
            .map(|c| (a[c.old].join(" "), b[c.new].join(" ")))
            .collect()
    }

    #[test]
    fn diff_finds_removed_added_and_replaced_runs() {
        assert_eq!(changes("a b c", "a b c"), []);
        assert_eq!(
            changes("the quick brown fox jumps", "the slow brown fox leaps high"),
            [
                ("quick".into(), "slow".into()),
                ("jumps".into(), "leaps high".into())
            ]
        );
        assert_eq!(changes("a b c d", "a d"), [("b c".into(), String::new())]);
        assert_eq!(changes("a d", "a b c d"), [(String::new(), "b c".into())]);
        assert_eq!(changes("x", "y"), [("x".into(), "y".into())]);
    }

    #[test]
    fn far_apart_texts_diff_page_by_page() {
        let word = |page, text: String| Word {
            page,
            text,
            rect: Rect::default(),
        };
        let a: Vec<Word> = (0..3000).map(|i| word(i / 1000, format!("a{i}"))).collect();
        let mut b: Vec<Word> = (0..3000).map(|i| word(i / 1000, format!("b{i}"))).collect();
        assert!(diff(&a, &b).is_none());
        b[1000..2000].clone_from_slice(&a[1000..2000]);
        assert_eq!(
            compare_words(&a, &b),
            [
                Change {
                    old: 0..1000,
                    new: 0..1000
                },
                Change {
                    old: 2000..3000,
                    new: 2000..3000
                }
            ]
        );
    }
}
