//! Conversion: a document out to Word, OpenDocument, Excel, images, text, HTML and Markdown,
//! and images, web pages and text files in as PDF.
//!
//! Word, OpenDocument, text, HTML and the images come from MuPDF's document writers. Excel is
//! the tables MuPDF's CSV writer finds (it segments each page and hunts for tables in the text
//! and the rules around it), one sheet a table, written as XLSX here. Markdown is set from the
//! page's text blocks, with headings by type size.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use mupdf::{Document, DocumentWriter, Matrix};

use crate::edit::blocks;
use crate::pages::pdf;
use crate::{DocId, Error, TextBlock};

/// A format a document is exported to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Export {
    Word,
    OpenDocument,
    Excel,
    Png,
    Jpeg,
    Text,
    Html,
    Markdown,
}

impl Export {
    pub fn extension(self) -> &'static str {
        match self {
            Export::Word => "docx",
            Export::OpenDocument => "odt",
            Export::Excel => "xlsx",
            Export::Png => "png",
            Export::Jpeg => "jpg",
            Export::Text => "txt",
            Export::Html => "html",
            Export::Markdown => "md",
        }
    }
}

/// A sheet of a workbook: its name and its rows of cells.
type Sheet = (String, Vec<Vec<String>>);

/// The page reflowing files (web pages, text) are laid out on: A4, in points.
const A4: (f32, f32) = (595.0, 842.0);

/// The resolution pages are exported as images at.
const DPI: u32 = 150;

/// A file removed when dropped.
pub(crate) struct Temp(pub(crate) PathBuf);

impl Temp {
    pub(crate) fn new(extension: &str) -> Temp {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let name = format!("micropdf-{}-{n}.{extension}", std::process::id());
        Temp(std::env::temp_dir().join(name))
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn utf8(path: &Path) -> Result<&str, Error> {
    path.to_str().ok_or(Error::Invalid("path is not UTF-8"))
}

/// Runs every page of `doc` through a MuPDF document writer of `format` at `path`.
fn write_pages(doc: &Document, path: &str, format: &str, options: &str) -> Result<(), Error> {
    let mut writer = DocumentWriter::new(path, format, options)?;
    for i in 0..doc.page_count()? {
        let page = doc.load_page(i)?;
        let device = writer.begin_page(page.bounds()?)?;
        page.run(&device, &Matrix::IDENTITY)?;
        writer.end_page(device)?;
    }
    Ok(())
}

/// When `path` is not a PDF, writes it as one to a temporary file and returns that.
pub(crate) fn as_pdf(path: &Path) -> Result<Option<Temp>, Error> {
    let pdf = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    if pdf {
        return Ok(None);
    }
    let mut doc = Document::open(utf8(path)?)?;
    if doc.is_reflowable()? {
        doc.layout(A4.0, A4.1, 11.0)?;
    }
    let temp = Temp::new("pdf");
    write_pages(&doc, utf8(&temp.0)?, "pdf", "compress")?;
    Ok(Some(temp))
}

fn export(doc: &Document, format: Export, target: &Path) -> Result<Vec<PathBuf>, Error> {
    let ext = format.extension();
    match format {
        Export::Excel => std::fs::write(target, xlsx(&tables(doc)?))?,
        Export::Markdown => std::fs::write(target, markdown(doc)?)?,
        Export::Png | Export::Jpeg => {
            // One file a page: "name-1.png", "name-2.png" and so on.
            let stem = target.with_extension("");
            let stem = utf8(&stem)?;
            let options = format!("resolution={DPI}");
            write_pages(doc, &format!("{stem}-%d.{ext}"), ext, &options)?;
            let n = doc.page_count()?;
            return Ok((1..=n)
                .map(|i| format!("{stem}-{i}.{ext}").into())
                .collect());
        }
        _ => write_pages(doc, utf8(target)?, ext, "")?,
    }
    Ok(vec![target.to_path_buf()])
}

/// The tables MuPDF finds on the pages, each named after its page, as rows of cells. With no
/// tables, a sheet a page of its paragraphs.
fn tables(doc: &Document) -> Result<Vec<Sheet>, Error> {
    let csv = Temp::new("csv");
    write_pages(doc, utf8(&csv.0)?, "csv", "")?;
    let text = std::fs::read_to_string(&csv.0)?.replace('\u{feff}', "");
    let mut sheets: Vec<Sheet> = Vec::new();
    let mut on_page = HashMap::new();
    for line in text.lines() {
        // "Table <n>,<rows>,<page>,<box>" starts a table; cells are always quoted.
        if let Some(head) = line.strip_prefix("Table ") {
            let page: usize = head
                .split(',')
                .nth(2)
                .and_then(|p| p.parse().ok())
                .unwrap_or(0);
            let k = on_page.entry(page).or_insert(0);
            *k += 1;
            sheets.push((format!("Page {} table {k}", page + 1), Vec::new()));
        } else if let Some((_, rows)) = sheets.last_mut() {
            rows.push(cells(line));
        }
    }
    if sheets.is_empty() {
        let pdf = pdf(doc)?;
        for p in 0..pdf.page_count()? {
            let rows = blocks(&pdf.load_pdf_page(p)?)?
                .into_iter()
                .flat_map(|b| {
                    b.shown
                        .text
                        .lines()
                        .map(|l| vec![l.to_owned()])
                        .collect::<Vec<_>>()
                })
                .collect();
            sheets.push((format!("Page {}", p + 1), rows));
        }
    }
    Ok(sheets)
}

/// The cells of one CSV row: quoted, with quotes doubled, and empty between bare commas.
fn cells(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                out.last_mut().expect("a cell").push('"');
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(String::new()),
            c => out.last_mut().expect("a cell").push(c),
        }
    }
    out
}

/// A workbook of `sheets`, numbers as numbers and the rest as text.
fn xlsx(sheets: &[Sheet]) -> Vec<u8> {
    let mut files = vec![
        (
            "[Content_Types].xml".to_owned(),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>{}</Types>"#,
                (1..=sheets.len())
                    .map(|i| format!(r#"<Override PartName="/xl/worksheets/sheet{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#))
                    .collect::<String>()
            ),
        ),
        (
            "_rels/.rels".to_owned(),
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#
                .to_owned(),
        ),
        (
            "xl/workbook.xml".to_owned(),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>{}</sheets></workbook>"#,
                sheets
                    .iter()
                    .enumerate()
                    .map(|(i, (name, _))| format!(
                        r#"<sheet name="{}" sheetId="{}" r:id="rId{}"/>"#,
                        escape(name),
                        i + 1,
                        i + 1
                    ))
                    .collect::<String>()
            ),
        ),
        (
            "xl/_rels/workbook.xml.rels".to_owned(),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{}</Relationships>"#,
                (1..=sheets.len())
                    .map(|i| format!(r#"<Relationship Id="rId{i}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{i}.xml"/>"#))
                    .collect::<String>()
            ),
        ),
    ];
    for (i, (_, rows)) in sheets.iter().enumerate() {
        let mut data = String::new();
        for (r, row) in rows.iter().enumerate() {
            data += &format!(r#"<row r="{}">"#, r + 1);
            for (c, cell) in row.iter().enumerate() {
                let cell = cell.trim();
                if cell.is_empty() {
                    continue;
                }
                let at = format!("{}{}", column(c), r + 1);
                let plain = cell.replace(',', "");
                let number = plain.parse::<f64>().is_ok()
                    && plain
                        .chars()
                        .all(|c| c.is_ascii_digit() || c == '.' || c == '-');
                data += &if number {
                    format!(r#"<c r="{at}"><v>{plain}</v></c>"#)
                } else {
                    format!(
                        r#"<c r="{at}" t="inlineStr"><is><t xml:space="preserve">{}</t></is></c>"#,
                        escape(cell)
                    )
                };
            }
            data += "</row>";
        }
        files.push((
            format!("xl/worksheets/sheet{}.xml", i + 1),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{data}</sheetData></worksheet>"#
            ),
        ));
    }
    zip(&files)
}

/// Column `c` from 0 as a spreadsheet names it: A, B, ... Z, AA.
fn column(mut c: usize) -> String {
    let mut name = Vec::new();
    c += 1;
    while c > 0 {
        c -= 1;
        name.push(b'A' + (c % 26) as u8);
        c /= 26;
    }
    name.reverse();
    String::from_utf8(name).expect("ASCII")
}

/// `s` as XML text, without the control characters XML cannot hold.
fn escape(s: &str) -> String {
    s.chars()
        .filter(|&c| c >= ' ' || c == '\t' || c == '\n')
        .fold(String::new(), |mut out, c| {
            match c {
                '&' => out += "&amp;",
                '<' => out += "&lt;",
                '>' => out += "&gt;",
                '"' => out += "&quot;",
                c => out.push(c),
            }
            out
        })
}

/// A ZIP archive of `files`, stored without compression.
fn zip(files: &[(String, String)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let (name, data) = (name.as_bytes(), data.as_bytes());
        let offset = out.len() as u32;
        // Version 2.0, no flags, stored, 1 January 1980, then the checksum and sizes.
        let mut common = Vec::new();
        for v in [20u16, 0, 0, 0, 0x21] {
            common.extend(v.to_le_bytes());
        }
        common.extend(crc32(data).to_le_bytes());
        common.extend((data.len() as u32).to_le_bytes());
        common.extend((data.len() as u32).to_le_bytes());
        common.extend((name.len() as u16).to_le_bytes());
        common.extend(0u16.to_le_bytes());
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend(&common);
        out.extend(name);
        out.extend(data);
        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend(20u16.to_le_bytes());
        central.extend(&common);
        // No comment, disk 0, no attributes, then where the entry starts.
        central.extend([0u8; 10]);
        central.extend(offset.to_le_bytes());
        central.extend(name);
    }
    let at = out.len() as u32;
    let n = files.len() as u16;
    out.extend(&central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    for v in [n, n] {
        out.extend(v.to_le_bytes());
    }
    out.extend((central.len() as u32).to_le_bytes());
    out.extend(at.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

/// The text as Markdown: paragraphs, and headings where the type is well above the size most
/// of the text is set in.
fn markdown(doc: &Document) -> Result<String, Error> {
    let pdf = pdf(doc)?;
    let mut all: Vec<TextBlock> = Vec::new();
    for p in 0..pdf.page_count()? {
        all.extend(blocks(&pdf.load_pdf_page(p)?)?.into_iter().map(|b| b.shown));
    }
    let mut weight: HashMap<i32, usize> = HashMap::new();
    for b in &all {
        *weight.entry((b.size * 2.0).round() as i32).or_default() += b.text.len();
    }
    let body = weight
        .into_iter()
        .max_by_key(|&(_, n)| n)
        .map_or(10.0, |(s, _)| s as f32 / 2.0);
    let mut out = String::new();
    for b in all {
        let heading = if b.size >= body * 1.6 {
            "# "
        } else if b.size >= body * 1.25 {
            "## "
        } else if b.size >= body * 1.1 && b.text.len() < 120 {
            "### "
        } else {
            ""
        };
        if heading.is_empty() {
            for line in b.text.lines().map(str::trim).filter(|l| !l.is_empty()) {
                match line.strip_prefix(['•', '◦', '▪', '–']) {
                    Some(item) => out += &format!("- {}\n\n", item.trim()),
                    None => out += &format!("{line}\n\n"),
                }
            }
        } else {
            out += &format!("{heading}{}\n\n", b.text.replace('\n', " ").trim());
        }
    }
    Ok(out)
}

impl crate::Engine {
    /// Writes the document as `format` to `target`; images are a file a page, named after
    /// `target` with the page number. Returns the files written.
    pub fn export(
        &self,
        doc: DocId,
        format: Export,
        target: PathBuf,
    ) -> Result<Vec<PathBuf>, Error> {
        self.read(doc, move |d, _| export(d, format, &target))
    }
}
