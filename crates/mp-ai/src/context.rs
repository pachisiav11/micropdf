//! What the model reads: the document as page-tagged text (`[p. 12]`), whole when it fits, else
//! the pages that best match the question by BM25, and the citations in its answers as links.

use std::collections::HashMap;

use crate::MAX_DOC_CHARS;

/// The document as one request carries it.
#[derive(Debug, Clone, PartialEq)]
pub struct Context {
    /// Page-tagged text.
    pub text: String,
    /// The pages it carries (1-based, in order): all of them unless the document is too long.
    pub pages: Vec<usize>,
    pub page_count: usize,
}

const K1: f64 = 1.2;
const B: f64 = 0.75;

impl Context {
    /// `pages` holds each page's text; `focus` (0-based) is the page being read, kept first when
    /// only some pages fit. Fails when no page has text.
    pub fn new(pages: &[String], question: &str, focus: Option<usize>) -> Result<Context, String> {
        Context::within(pages, question, focus, MAX_DOC_CHARS)
    }

    fn within(
        pages: &[String],
        question: &str,
        focus: Option<usize>,
        budget: usize,
    ) -> Result<Context, String> {
        if pages.iter().all(|p| p.trim().is_empty()) {
            return Err(
                "This PDF has no text the assistant can read: its pages are images. \
                        Text recognition (OCR) is not available yet."
                    .into(),
            );
        }
        let tagged: Vec<String> = pages
            .iter()
            .enumerate()
            .map(|(i, text)| tag(i, text))
            .collect();

        let total: usize = tagged.iter().map(|t| t.len() + 2).sum();
        let chosen: Vec<usize> = if total <= budget {
            (0..pages.len()).collect()
        } else {
            let mut order = rank(pages, question);
            if let Some(f) = focus.filter(|&f| f < pages.len()) {
                order.retain(|&i| i != f);
                order.insert(0, f);
            }
            let mut used = 0;
            let mut chosen: Vec<usize> = order
                .into_iter()
                .filter(|&i| !pages[i].trim().is_empty())
                .take_while(|&i| {
                    used += tagged[i].len() + 2;
                    used <= budget
                })
                .collect();
            if chosen.is_empty() {
                // A single page longer than the whole budget: it goes, and is truncated.
                chosen.push(focus.unwrap_or(0).min(pages.len() - 1));
            }
            chosen.sort_unstable();
            chosen
        };

        let text = chosen
            .iter()
            .map(|&i| tagged[i].as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        Ok(Context {
            text,
            pages: chosen.iter().map(|i| i + 1).collect(),
            page_count: pages.len(),
        })
    }

    pub fn partial(&self) -> bool {
        self.pages.len() < self.page_count
    }

    /// Tells the model, after the document, that it holds only some of the pages.
    pub(crate) fn note(&self) -> String {
        if !self.partial() {
            return String::new();
        }
        format!(
            "(The document has {} pages and is too long to send whole. Only pages {} are \
             included: the ones that best match the question. If the answer is not in them, \
             say so.)",
            self.page_count,
            ranges(&self.pages)
        )
    }
}

fn tag(index: usize, text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        format!("[p. {}]\n(no text on this page)", index + 1)
    } else {
        format!("[p. {}]\n{text}", index + 1)
    }
}

fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() > 1)
        .map(str::to_lowercase)
}

/// Page indices, best match for the question first (BM25); pages that match nothing keep their
/// order at the end, so a question with no useful words gets the start of the document.
fn rank(pages: &[String], question: &str) -> Vec<usize> {
    let mut terms: Vec<String> = words(question).collect();
    terms.sort();
    terms.dedup();

    let counts: Vec<HashMap<String, usize>> = pages
        .iter()
        .map(|page| {
            let mut tf = HashMap::new();
            for w in words(page) {
                *tf.entry(w).or_insert(0) += 1;
            }
            tf
        })
        .collect();
    let lengths: Vec<usize> = counts.iter().map(|tf| tf.values().sum()).collect();
    let n = pages.len() as f64;
    let average = (lengths.iter().sum::<usize>() as f64 / n).max(1.0);

    let mut scores = vec![0.0; pages.len()];
    for term in &terms {
        let df = counts.iter().filter(|tf| tf.contains_key(term)).count() as f64;
        if df == 0.0 {
            continue;
        }
        let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
        for (i, tf) in counts.iter().enumerate() {
            if let Some(&f) = tf.get(term) {
                let f = f as f64;
                scores[i] +=
                    idf * f * (K1 + 1.0) / (f + K1 * (1.0 - B + B * lengths[i] as f64 / average));
            }
        }
    }

    let mut order: Vec<usize> = (0..pages.len()).collect();
    order.sort_by(|&a, &b| scores[b].total_cmp(&scores[a]).then(a.cmp(&b)));
    order
}

/// "3–5, 7, 12" for pages 3, 4, 5, 7 and 12.
pub fn ranges(pages: &[usize]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < pages.len() {
        let start = pages[i];
        while i + 1 < pages.len() && pages[i + 1] == pages[i] + 1 {
            i += 1;
        }
        out.push(if pages[i] == start {
            start.to_string()
        } else {
            format!("{start}–{}", pages[i])
        });
        i += 1;
    }
    out.join(", ")
}

/// Turns each citation such as `[p. 12]` into a Markdown link to `page:12`.
pub fn link_citations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('[') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        match citation(tail) {
            Some((len, page)) if !tail[len..].starts_with('(') => {
                out.push_str(&format!("[{}](page:{page})", &tail[1..len - 1]));
                rest = &tail[len..];
            }
            _ => {
                out.push('[');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A citation at the start of `s` ("[p. 12]", "[p.3]", "[pp. 3–4]"): its length and first page.
fn citation(s: &str) -> Option<(usize, usize)> {
    let end = s.find(']').filter(|&e| e <= 24)?;
    let inner = s[1..end].trim();
    let rest = inner
        .strip_prefix("pp.")
        .or_else(|| inner.strip_prefix("p."))?
        .trim_start();
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    let page = rest[..digits].parse().ok().filter(|&p: &usize| p > 0)?;
    rest[digits..]
        .chars()
        .all(|c| c.is_ascii_digit() || " ,–-p.".contains(c))
        .then_some((end + 1, page))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages(texts: &[&str]) -> Vec<String> {
        texts.iter().map(|t| t.to_string()).collect()
    }

    #[test]
    fn a_short_document_goes_whole() {
        let c = Context::new(&pages(&["One.", "", "Three."]), "anything", None).unwrap();
        assert_eq!(c.pages, [1, 2, 3]);
        assert!(!c.partial());
        assert_eq!(
            c.text,
            "[p. 1]\nOne.\n\n[p. 2]\n(no text on this page)\n\n[p. 3]\nThree."
        );
        assert_eq!(c.note(), "");
    }

    #[test]
    fn a_long_document_sends_the_pages_that_match() {
        let mut texts = vec!["Filler about weather and seasons. ".repeat(20); 30];
        texts[17] = "The invoice total is 4,200 euros, due in March.".into();
        texts[4] = "An invoice was mentioned here.".into();
        let c = Context::within(&texts, "What is the invoice total?", None, 500).unwrap();
        assert!(c.partial());
        assert_eq!(c.pages, [5, 18]);
        assert!(c.text.contains("[p. 18]\nThe invoice total"));
        assert!(c.note().contains("pages 5, 18"));
        assert!(c.note().contains("30 pages"));
    }

    #[test]
    fn the_page_being_read_comes_first() {
        let texts = vec!["Same words on every page. ".repeat(10); 10];
        let c = Context::within(&texts, "summarize this page", Some(7), 600).unwrap();
        assert!(c.pages.contains(&8));
        // Nothing matches better, so the rest is the start of the document.
        assert_eq!(c.pages[0], 1);
    }

    #[test]
    fn a_document_without_text_cannot_be_read() {
        let err = Context::new(&pages(&["", "  "]), "what?", None).unwrap_err();
        assert!(err.contains("OCR"));
    }

    #[test]
    fn page_lists_read_as_ranges() {
        assert_eq!(ranges(&[3, 4, 5, 7, 12]), "3–5, 7, 12");
        assert_eq!(ranges(&[1]), "1");
        assert_eq!(ranges(&[]), "");
    }

    #[test]
    fn citations_become_page_links() {
        assert_eq!(
            link_citations("See [p. 3] and [pp. 4–5], [p.12]."),
            "See [p. 3](page:3) and [pp. 4–5](page:4), [p.12](page:12)."
        );
        // Links, other brackets and page zero stay as they are.
        for text in [
            "[p. 3](page:3)",
            "[note]",
            "[p. 0]",
            "[p. 3 says the opposite of p. 4]",
            "a [b",
        ] {
            assert_eq!(link_citations(text), text);
        }
    }
}
