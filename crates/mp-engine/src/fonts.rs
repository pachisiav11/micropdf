//! Installed Windows fonts for documents that use fonts they do not embed.
//!
//! MuPDF asks the system for a font before it falls back to its built-in Base 14 fonts. The
//! stock `font-kit` loader scanned every installed font on each request, which cost about a
//! second per document. This loader reads the font names from DirectWrite once, answers from
//! that index, and shares each loaded font between documents.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use dwrote::{FontCollection, FontSimulations, FontStyle, InformationalStringId};
use mupdf::{CjkFontOrdering, Font, FontHints, FontLoader};

/// One installed font face, found again by its position in the system collection.
#[derive(Debug, Clone)]
struct Face {
    family: u32,
    font: u32,
    bold: bool,
    italic: bool,
}

#[derive(Default)]
struct Index {
    faces: Vec<Face>,
    /// Normalised PostScript and full names.
    names: HashMap<String, usize>,
    /// Normalised family names.
    families: HashMap<String, Vec<usize>>,
}

static INDEX: OnceLock<Index> = OnceLock::new();
static LOADED: Mutex<Vec<(usize, Font)>> = Mutex::new(Vec::new());

/// Registers the loader with MuPDF and starts reading the font names in the background, so
/// the first document that needs a system font does not wait for it.
pub(crate) fn install() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        mupdf::set_font_loader(SystemFonts);
        std::thread::spawn(index);
    });
}

fn index() -> &'static Index {
    INDEX.get_or_init(build)
}

fn build() -> Index {
    let mut index = Index::default();
    let collection = FontCollection::system();
    for f in 0..collection.get_font_family_count() {
        let Ok(family) = collection.font_family(f) else {
            continue;
        };
        let family_key = family.family_name().map(|n| key(&n)).unwrap_or_default();
        for i in 0..family.get_font_count() {
            let Ok(font) = family.font(i) else { continue };
            // Synthesised bold and oblique faces share a file with the real face.
            if font.simulations() != FontSimulations::None {
                continue;
            }
            let id = index.faces.len();
            index.faces.push(Face {
                family: f,
                font: i,
                bold: font.weight().to_u32() >= 600,
                italic: font.style() != FontStyle::Normal,
            });
            for which in [
                InformationalStringId::PostscriptName,
                InformationalStringId::FullName,
            ] {
                if let Some(name) = font.informational_string(which) {
                    index.names.entry(key(&name)).or_insert(id);
                }
            }
            if !family_key.is_empty() {
                index
                    .families
                    .entry(family_key.clone())
                    .or_default()
                    .push(id);
            }
        }
    }
    index
}

/// Lower-case letters and digits only, without a subset prefix such as `ABCDEF+`.
fn key(name: &str) -> String {
    let b = name.as_bytes();
    let name = if b.len() > 7 && b[6] == b'+' && b[..6].iter().all(u8::is_ascii_uppercase) {
        &name[7..]
    } else {
        name
    };
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// MuPDF draws these with its own metric-compatible fonts; Windows does not have them.
fn is_base14(key: &str) -> bool {
    matches!(
        key,
        "helvetica"
            | "helveticabold"
            | "helveticaoblique"
            | "helveticaboldoblique"
            | "timesroman"
            | "timesbold"
            | "timesitalic"
            | "timesbolditalic"
            | "courier"
            | "courierbold"
            | "courieroblique"
            | "courierboldoblique"
            | "symbol"
            | "zapfdingbats"
    )
}

/// Splits trailing style words from a name key: `arialboldmt` → (`arial`, bold, not italic).
fn split_style(mut key: &str) -> (&str, bool, bool) {
    let (mut bold, mut italic) = (false, false);
    loop {
        let before = key;
        for suffix in ["identityh", "psmt", "mt", "ps", "regular", "normal"] {
            key = key.strip_suffix(suffix).unwrap_or(key);
        }
        for suffix in ["italic", "oblique"] {
            if let Some(k) = key.strip_suffix(suffix) {
                key = k;
                italic = true;
            }
        }
        for suffix in ["bold", "black", "heavy", "semibold"] {
            if let Some(k) = key.strip_suffix(suffix) {
                key = k;
                bold = true;
            }
        }
        if key == before {
            return (key, bold, italic);
        }
    }
}

/// Finds the face for a font name as PDFs write it: `Arial,Bold`, `Arial-BoldMT`,
/// `TimesNewRomanPSMT`, `ABCDEF+Calibri`, `Segoe UI`.
fn find(name: &str, hints: FontHints) -> Option<usize> {
    let k = key(name);
    if k.is_empty() || is_base14(&k) {
        return None;
    }
    let index = index();
    let found = index.names.get(&k).copied().or_else(|| {
        let (family, bold, italic) = split_style(&k);
        family_face(index, family, bold || hints.bold, italic || hints.italic)
    })?;
    let face = &index.faces[found];
    if hints.needs_exact_metrics && ((hints.bold && !face.bold) || (hints.italic && !face.italic)) {
        return None;
    }
    Some(found)
}

/// The face of `family` closest to the wanted style.
fn family_face(index: &Index, family: &str, bold: bool, italic: bool) -> Option<usize> {
    let faces = index.families.get(family)?;
    faces.iter().copied().min_by_key(|&i| {
        let f = &index.faces[i];
        u8::from(f.bold != bold) * 2 + u8::from(f.italic != italic)
    })
}

/// The first installed family from `names`, in its regular style.
fn first_family(names: &[&str]) -> Option<usize> {
    let index = index();
    names
        .iter()
        .find_map(|n| family_face(index, &key(n), false, false))
}

/// Whether a font family is installed, such as "Segoe Script".
pub fn has_font(family: &str) -> bool {
    family_face(index(), &key(family), false, false).is_some()
}

/// An installed family's regular face, such as "Segoe Script".
pub(crate) fn family(name: &str) -> Option<Font> {
    family_face(index(), &key(name), false, false).and_then(load)
}

fn load(id: usize) -> Option<Font> {
    let mut loaded = LOADED.lock().ok()?;
    if let Some((_, font)) = loaded.iter().find(|(i, _)| *i == id) {
        return Some(font.clone());
    }
    let face = index().faces.get(id)?;
    let font = FontCollection::system()
        .font_family(face.family)
        .ok()?
        .font(face.font)
        .ok()?;
    let name = font
        .informational_string(InformationalStringId::PostscriptName)
        .unwrap_or_else(|| font.family_name());
    let face = font.create_font_face();
    let path: PathBuf = face.files().ok()?.first()?.font_file_path().ok()?;
    let data = std::fs::read(path).ok()?;
    let font = Font::from_bytes_with_index(&name, face.get_index() as i32, &data).ok()?;
    loaded.push((id, font.clone()));
    Some(font)
}

fn cjk_families(ordering: CjkFontOrdering, serif: bool) -> &'static [&'static str] {
    match (ordering, serif) {
        (CjkFontOrdering::AdobeGb, true) => &["SimSun", "NSimSun", "Microsoft YaHei"],
        (CjkFontOrdering::AdobeGb, false) => &["Microsoft YaHei", "DengXian", "SimHei", "SimSun"],
        (CjkFontOrdering::AdobeCns, true) => &["MingLiU", "PMingLiU", "Microsoft JhengHei"],
        (CjkFontOrdering::AdobeCns, false) => &["Microsoft JhengHei", "MingLiU", "PMingLiU"],
        (CjkFontOrdering::AdobeJapan, true) => &["Yu Mincho", "MS Mincho", "Yu Gothic", "Meiryo"],
        (CjkFontOrdering::AdobeJapan, false) => &["Yu Gothic", "Meiryo", "MS Gothic"],
        (CjkFontOrdering::AdobeKorea, true) => &["Batang", "Malgun Gothic", "Gulim"],
        (CjkFontOrdering::AdobeKorea, false) => &["Malgun Gothic", "Gulim", "Dotum"],
    }
}

// Script and language codes MuPDF passes for fallback fonts (ucdn.h, fitz/text.h).
const SCRIPT_HAN: u32 = 35;
const LANG_JA: u32 = lang(b"ja");
const LANG_KO: u32 = lang(b"ko");
const LANG_ZH_HANT: u32 = lang(b"zht");

/// `FZ_LANG_TAG2` / `FZ_LANG_TAG3`: base-27 digits, first letter lowest.
const fn lang(tag: &[u8]) -> u32 {
    let (mut value, mut scale, mut i) = (0, 1, 0);
    while i < tag.len() {
        value += (tag[i] - b'a' + 1) as u32 * scale;
        scale *= 27;
        i += 1;
    }
    value
}

fn fallback_families(script: u32, language: u32) -> &'static [&'static str] {
    match script {
        SCRIPT_HAN => match language {
            LANG_JA => cjk_families(CjkFontOrdering::AdobeJapan, false),
            LANG_KO => cjk_families(CjkFontOrdering::AdobeKorea, false),
            LANG_ZH_HANT => cjk_families(CjkFontOrdering::AdobeCns, false),
            _ => cjk_families(CjkFontOrdering::AdobeGb, false),
        },
        32 | 33 => cjk_families(CjkFontOrdering::AdobeJapan, false),
        24 => cjk_families(CjkFontOrdering::AdobeKorea, false),
        34 => cjk_families(CjkFontOrdering::AdobeCns, false),
        5..=7 => &["Segoe UI", "Arial", "Times New Roman"],
        8 => &["MV Boli"],
        9..=18 => &["Nirmala UI", "Mangal"],
        19 | 20 | 30 => &["Leelawadee UI", "Tahoma"],
        21 => &["Microsoft Himalaya"],
        22 => &["Myanmar Text"],
        4 | 23 => &["Segoe UI", "Sylfaen"],
        25 => &["Ebrima", "Nyala"],
        26 | 27 => &["Gadugi"],
        31 => &["Mongolian Baiti"],
        36 => &["Microsoft Yi Baiti"],
        _ => &["Segoe UI", "Segoe UI Symbol", "Segoe UI Emoji", "Arial"],
    }
}

struct SystemFonts;

impl FontLoader for SystemFonts {
    fn load_font(&self, name: &str, hints: FontHints) -> Option<Font> {
        load(find(name, hints)?)
    }

    fn load_cjk_font(&self, _name: &str, ordering: CjkFontOrdering, serif: bool) -> Option<Font> {
        load(first_family(cjk_families(ordering, serif))?)
    }

    fn load_fallback_font(&self, script: u32, language: u32, _hints: FontHints) -> Option<Font> {
        load(first_family(fallback_families(script, language))?)
    }
}

/// Whether an installed font covers this CJK ordering; tests use it to skip on bare systems.
pub fn has_cjk(ordering: CjkFontOrdering) -> bool {
    first_family(cjk_families(ordering, false)).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_drop_subset_prefix_and_punctuation() {
        assert_eq!(key("ABCDEF+Calibri-Bold"), "calibribold");
        assert_eq!(key("Arial,BoldItalic"), "arialbolditalic");
        assert_eq!(key("Segoe UI"), "segoeui");
        assert_eq!(key("abcdef+Foo"), "abcdeffoo");
    }

    #[test]
    fn styles_split_from_family() {
        assert_eq!(split_style("arialboldmt"), ("arial", true, false));
        assert_eq!(
            split_style("timesnewromanpsbolditalicmt"),
            ("timesnewroman", true, true)
        );
        assert_eq!(split_style("couriernewpsmt"), ("couriernew", false, false));
        assert_eq!(split_style("verdana"), ("verdana", false, false));
    }

    #[test]
    fn language_codes_match_mupdf() {
        assert_eq!((LANG_JA, LANG_KO, LANG_ZH_HANT), (37, 416, 14822));
    }

    #[test]
    fn base14_names_are_left_to_mupdf() {
        assert_eq!(find("Helvetica", FontHints::default()), None);
        assert_eq!(find("Times-Roman", FontHints::default()), None);
    }

    #[test]
    fn common_windows_fonts_resolve() {
        // Arial and Times New Roman ship with every Windows edition.
        for name in [
            "Arial",
            "ArialMT",
            "Arial,Bold",
            "Arial-BoldMT",
            "TimesNewRomanPSMT",
        ] {
            let id = find(name, FontHints::default()).unwrap_or_else(|| panic!("{name}"));
            assert!(load(id).is_some(), "{name} loads");
        }
        let bold = find("Arial,Bold", FontHints::default()).unwrap();
        assert!(index().faces[bold].bold);
    }
}
