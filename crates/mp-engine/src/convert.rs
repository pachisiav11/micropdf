//! Conversion: a document out to Word, OpenDocument, Excel, PowerPoint, images, text, HTML
//! and Markdown, and Office files, images, web pages and text files in as PDF.
//!
//! Word, OpenDocument, text, HTML and the images come from MuPDF's document writers. Excel is
//! the tables MuPDF's CSV writer finds (it segments each page and hunts for tables in the text
//! and the rules around it), one sheet a table, written as XLSX here. Markdown is set from the
//! page's text blocks, with headings by type size. Office files become PDFs in Word, Excel or
//! PowerPoint when installed, driven over COM from PowerShell, else in LibreOffice, installed
//! or as micropdf's add-on; PowerPoint files come from LibreOffice's PDF import.

use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use base64::prelude::{BASE64_STANDARD, Engine as _};
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
    PowerPoint,
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
            Export::PowerPoint => "pptx",
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

/// The Office and OpenDocument files a PDF is made of through Office or LibreOffice.
pub const OFFICE: [&str; 10] = [
    "doc", "docx", "rtf", "odt", "xls", "xlsx", "ods", "ppt", "pptx", "odp",
];

/// Starts a process without a console window.
const NO_WINDOW: u32 = 0x0800_0000;

const NEEDS_OFFICE: &str = "This needs Microsoft Office or LibreOffice. Install the LibreOffice \
                            add-on from the Convert menu, or LibreOffice itself.";

/// Where micropdf keeps add-ons: %LOCALAPPDATA%\micropdf\addons.
pub fn addons_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("micropdf")
        .join("addons")
}

/// Where the LibreOffice add-on is unpacked.
pub fn libreoffice_dir() -> PathBuf {
    addons_dir().join("LibreOffice")
}

/// The LibreOffice add-on's program, when the add-on is installed.
pub fn addon_soffice() -> Option<PathBuf> {
    fn find(dir: &Path, depth: usize) -> Option<PathBuf> {
        let exe = dir.join("program").join("soffice.exe");
        if exe.is_file() {
            return Some(exe);
        }
        let entries = std::fs::read_dir(dir).ok().filter(|_| depth > 0)?;
        entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .find_map(|e| find(&e.path(), depth - 1))
    }
    find(&libreoffice_dir(), 3)
}

/// LibreOffice's program: the add-on's, else an installed one.
pub fn soffice() -> Option<PathBuf> {
    addon_soffice().or_else(|| {
        ["ProgramFiles", "ProgramFiles(x86)"]
            .iter()
            .filter_map(std::env::var_os)
            .map(|d| PathBuf::from(d).join("LibreOffice/program/soffice.exe"))
            .find(|p| p.is_file())
    })
}

/// Converts `path` to `format` in LibreOffice, with `filter` reading it when given; returns
/// the temporary folder it wrote to, and the file.
fn libreoffice(path: &Path, format: &str, filter: Option<&str>) -> Result<(Temp, PathBuf), Error> {
    let soffice = soffice().ok_or(Error::Message(NEEDS_OFFICE.into()))?;
    let out = Temp::new("dir");
    std::fs::create_dir_all(&out.0)?;
    // A profile of its own, so a LibreOffice the user has open does not take the job.
    let profile = addons_dir().join("profile");
    let url = format!(
        "file:///{}",
        profile
            .to_string_lossy()
            .replace('\\', "/")
            .replace(' ', "%20")
    );
    let mut command = Command::new(soffice);
    command.args([
        "--headless",
        "--norestore",
        "--nolockcheck",
        &format!("-env:UserInstallation={url}"),
    ]);
    if let Some(filter) = filter {
        command.arg(format!("--infilter={filter}"));
    }
    command
        .args(["--convert-to", format, "--outdir"])
        .arg(&out.0)
        .arg(path);
    finish(&mut command)?;
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let made = out.0.join(format!("{stem}.{format}"));
    if !made.is_file() {
        return Err(Error::Message(format!(
            "LibreOffice could not convert {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        )));
    }
    Ok((out, made))
}

/// Makes a PDF of `path` in Word, Excel or PowerPoint; false if that Office program is not
/// installed or failed.
fn microsoft_office(path: &Path, target: &Path, ext: &str) -> bool {
    let quote = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', "''"));
    let (input, output) = (quote(path), quote(target));
    let work = match ext {
        "doc" | "docx" | "rtf" | "odt" => format!(
            "$a = New-Object -ComObject Word.Application; try {{ $d = $a.Documents.Open({input}, \
             $false, $true); $d.ExportAsFixedFormat({output}, 17); $d.Close($false) }} finally \
             {{ $a.Quit() }}"
        ),
        "xls" | "xlsx" | "ods" => format!(
            "$a = New-Object -ComObject Excel.Application; $a.DisplayAlerts = $false; try {{ \
             $b = $a.Workbooks.Open({input}, 0, $true); $b.ExportAsFixedFormat(0, {output}); \
             $b.Close($false) }} finally {{ $a.Quit() }}"
        ),
        _ => format!(
            "$a = New-Object -ComObject PowerPoint.Application; try {{ $p = \
             $a.Presentations.Open({input}, -1, 0, 0); $p.SaveAs({output}, 32); $p.Close() }} \
             finally {{ $a.Quit() }}"
        ),
    };
    let script = format!("$ErrorActionPreference = 'Stop'; {work}");
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut command = Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-NonInteractive", "-EncodedCommand"])
        .arg(BASE64_STANDARD.encode(utf16));
    finish(&mut command).unwrap_or(false) && target.is_file()
}

/// Runs `command` without a window, and stops it after three minutes: Office or LibreOffice
/// waiting on a dialog no one can see would otherwise hold the conversion for ever. Returns
/// whether it succeeded.
fn finish(command: &mut Command) -> Result<bool, Error> {
    let mut child = command.creation_flags(NO_WINDOW).spawn()?;
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status.success());
        }
        if start.elapsed() > std::time::Duration::from_secs(180) {
            let _ = child.kill();
            return Err(Error::Message(
                "The conversion took too long and was stopped.".into(),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// A file, or a folder, removed when dropped.
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
        let _ = std::fs::remove_dir_all(&self.0);
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
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if ext == "pdf" {
        return Ok(None);
    }
    if OFFICE.contains(&ext.as_str()) {
        let temp = Temp::new("pdf");
        if microsoft_office(path, &temp.0, &ext) {
            return Ok(Some(temp));
        }
        let (_out, made) = libreoffice(path, "pdf", None)?;
        std::fs::rename(made, &temp.0)?;
        return Ok(Some(temp));
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
        if format != Export::PowerPoint {
            return self.read(doc, move |d, _| export(d, format, &target));
        }
        // LibreOffice reads the document from a file, saved as it is now; the engine is free
        // while it works.
        let saved = Temp::new("pdf");
        let path = saved.0.clone();
        self.read(doc, move |d, _| Ok(pdf(d)?.save(utf8(&path)?)?))?;
        let (_out, made) = libreoffice(&saved.0, "pptx", Some("impress_pdf_import"))?;
        std::fs::copy(made, &target)?;
        Ok(vec![target])
    }
}
