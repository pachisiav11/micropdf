//! The library: the PDFs in the folders the reader picks, their text kept on disk, and
//! full-text search over it.
//!
//! Two files hold it. `library.tsv` lists each PDF with its size and time (so an unchanged
//! file is not read again) and where its text lies in `library.txt`, which holds every
//! document's text, pages split by form feeds. A search reads one document's text at a time,
//! so the library costs memory only while it searches.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use mupdf::Document;
use mupdf::text_page::TextPageFlags;

use crate::Error;

/// A PDF in the library.
#[derive(Debug, Clone, PartialEq)]
struct Entry {
    path: PathBuf,
    modified: u64,
    size: u64,
    /// Where its text starts in `library.txt`, and how long it is.
    offset: u64,
    len: u64,
}

/// A page whose text matches a search.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub path: PathBuf,
    /// The page, from 0.
    pub page: usize,
    /// The text around the first match, on one line.
    pub snippet: String,
}

#[derive(Debug, Default)]
pub struct Library {
    dir: PathBuf,
    entries: Vec<Entry>,
}

/// Where the library is kept: %LOCALAPPDATA%\micropdf\library.
pub fn library_dir() -> PathBuf {
    crate::convert::addons_dir().with_file_name("library")
}

impl Library {
    /// The library kept in `dir`; empty if there is none yet.
    pub fn open(dir: &Path) -> Library {
        let mut entries = Vec::new();
        if let Ok(file) = File::open(dir.join("library.tsv")) {
            for line in BufReader::new(file).lines().map_while(Result::ok) {
                let mut f = line.splitn(5, '\t');
                let mut n = || f.next().and_then(|v| v.parse::<u64>().ok());
                if let (Some(modified), Some(size), Some(offset), Some(len)) = (n(), n(), n(), n())
                    && let Some(path) = f.next()
                {
                    entries.push(Entry {
                        path: path.into(),
                        modified,
                        size,
                        offset,
                        len,
                    });
                }
            }
        }
        Library {
            dir: dir.to_path_buf(),
            entries,
        }
    }

    /// The PDFs in the library.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.entries.iter().map(|e| e.path.as_path())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Brings the library in line with the PDFs now in `folders` and their subfolders: reads
    /// the text of new and changed files, and drops the files that are gone. `progress` hears
    /// how many files of how many are done. Returns how many files it read.
    pub fn update(
        &mut self,
        folders: &[PathBuf],
        mut progress: impl FnMut(usize, usize),
    ) -> Result<usize, Error> {
        let mut found = Vec::new();
        for folder in folders {
            pdfs(folder, 16, &mut found);
        }
        found.sort();
        found.dedup_by(|a, b| a.0 == b.0);
        let same = found.len() == self.entries.len()
            && found
                .iter()
                .zip(&self.entries)
                .all(|(f, e)| (&f.0, f.1, f.2) == (&e.path, e.modified, e.size));
        if same {
            return Ok(0);
        }
        std::fs::create_dir_all(&self.dir)?;
        let text_path = self.dir.join("library.txt");
        let mut old = File::open(&text_path).ok();
        let part = self.dir.join("library.txt.part");
        let mut out = std::io::BufWriter::new(File::create(&part)?);
        let mut entries = Vec::with_capacity(found.len());
        let (mut offset, mut read) = (0u64, 0);
        let total = found.len();
        for (i, (path, modified, size)) in found.into_iter().enumerate() {
            let kept = self
                .entries
                .iter()
                .find(|e| e.path == path && e.modified == modified && e.size == size);
            let text = match (kept, old.as_mut()) {
                (Some(e), Some(old)) => {
                    let mut buf = vec![0; e.len as usize];
                    old.seek(SeekFrom::Start(e.offset))?;
                    old.read_exact(&mut buf)?;
                    buf
                }
                _ => {
                    read += 1;
                    // A file that does not open, or needs a password, is kept with no text so
                    // it is not tried again until it changes.
                    text(&path).unwrap_or_default().into_bytes()
                }
            };
            out.write_all(&text)?;
            entries.push(Entry {
                path,
                modified,
                size,
                offset,
                len: text.len() as u64,
            });
            offset += text.len() as u64;
            progress(i + 1, total);
        }
        out.flush()?;
        drop(out);
        drop(old);
        std::fs::rename(&part, &text_path)?;
        let mut list = String::new();
        for e in &entries {
            list += &format!(
                "{}\t{}\t{}\t{}\t{}\n",
                e.modified,
                e.size,
                e.offset,
                e.len,
                e.path.display()
            );
        }
        std::fs::write(self.dir.join("library.tsv"), list)?;
        self.entries = entries;
        Ok(read)
    }

    /// The pages holding every word of `query` (a part in quotes counts as one phrase),
    /// regardless of case, with the most matches first; at most `limit`.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Hit>, Error> {
        let terms: Vec<String> = query
            .split('"')
            .enumerate()
            .flat_map(|(i, part)| {
                if i % 2 == 1 {
                    vec![part.trim().to_lowercase()]
                } else {
                    part.split_whitespace().map(str::to_lowercase).collect()
                }
            })
            .filter(|t| !t.is_empty())
            .collect();
        let Some(first) = terms.first() else {
            return Ok(Vec::new());
        };
        let Ok(mut file) = File::open(self.dir.join("library.txt")) else {
            return Ok(Vec::new());
        };
        let mut hits = Vec::new();
        let mut buf = Vec::new();
        for e in self.entries.iter().filter(|e| e.len > 0) {
            buf.resize(e.len as usize, 0);
            file.seek(SeekFrom::Start(e.offset))?;
            file.read_exact(&mut buf)?;
            let text = String::from_utf8_lossy(&buf);
            let lower = text.to_lowercase();
            if !terms.iter().all(|t| lower.contains(t.as_str())) {
                continue;
            }
            for (page, (shown, low)) in text.split('\u{c}').zip(lower.split('\u{c}')).enumerate() {
                if terms.iter().all(|t| low.contains(t.as_str())) {
                    let count: usize = terms.iter().map(|t| low.matches(t.as_str()).count()).sum();
                    let snippet = snippet(shown, low, first);
                    hits.push((count, e.path.clone(), page, snippet));
                }
            }
        }
        hits.sort_by_key(|h| std::cmp::Reverse(h.0));
        Ok(hits
            .into_iter()
            .take(limit)
            .map(|(_, path, page, snippet)| Hit {
                path,
                page,
                snippet,
            })
            .collect())
    }
}

/// The PDFs in `dir` and its subfolders, `depth` levels down, with their times and sizes.
fn pdfs(dir: &Path, depth: usize, out: &mut Vec<(PathBuf, u64, u64)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() && depth > 0 {
            pdfs(&path, depth - 1, out);
        } else if kind.is_file()
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
            && let Ok(meta) = entry.metadata()
        {
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            out.push((path, modified, meta.len()));
        }
    }
}

/// The text of the PDF at `path`, pages split by form feeds.
fn text(path: &Path) -> Result<String, Error> {
    let doc = Document::open(path.to_str().ok_or(Error::Invalid("path is not UTF-8"))?)?;
    if doc.needs_password()? {
        return Err(Error::Invalid("the file needs a password"));
    }
    let mut text = String::new();
    for i in 0..doc.page_count()? {
        if i > 0 {
            text.push('\u{c}');
        }
        let page = doc.load_page(i)?.to_text_page(TextPageFlags::empty())?;
        text += &page.to_text()?.replace('\u{c}', " ");
    }
    Ok(text)
}

/// A line of `shown` around the first place `term` is in `low`, its lowercase copy.
fn snippet(shown: &str, low: &str, term: &str) -> String {
    // Lowercasing keeps the byte offsets of almost all text; where it does not, show the
    // page's start.
    let at = low
        .find(term)
        .filter(|_| shown.len() == low.len())
        .unwrap_or(0);
    let floor = |mut i: usize| {
        while !shown.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let start = floor(at.saturating_sub(60));
    let end = floor((at + term.len() + 100).min(shown.len()));
    let mut s: String = shown[start..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if start > 0 {
        s.insert(0, '\u{2026}');
    }
    if end < shown.len() {
        s.push('\u{2026}');
    }
    s
}
