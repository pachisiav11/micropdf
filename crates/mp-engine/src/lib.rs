//! MuPDF behind two thread boundaries.
//!
//! MuPDF documents are not thread-safe, so one engine thread owns every open document and
//! answers commands over a channel. Display lists are safe to share, so the engine hands them
//! out as `Arc<DisplayList>` and a pool of render workers (each with its own cloned
//! `fz_context`, managed by the `mupdf` crate) turns them into pixels and text.

mod annots;
mod attachments;
mod compare;
mod engine;
mod error;
mod fonts;
mod forms;
mod marks;
mod measure;
mod ocr;
mod pages;
mod redact;
mod render;
mod secure;
mod sign;
mod stamp;
mod summary;
mod text;
mod types;
mod xfdf;

pub use annots::{
    Annot, AnnotKind, Border, History, LINE_ENDS, NewAnnot, Properties, REVIEW_STATES, Restyle,
    STAMPS, Style, readable_date,
};
pub use compare::{Change, Comparison, Word, compare_words};
pub use engine::{DocId, DocInfo, Engine, PageSize};
pub use error::Error;
pub use fonts::{has_cjk, has_font};
pub use forms::{Field, FieldEdit, FieldKind, Xfa};
pub use marks::Mark;
pub use measure::{Measure, PAGE_UNITS, REAL_UNITS, Scale};
pub use mupdf::CjkFontOrdering;
pub use mupdf::DisplayList;
pub use ocr::{OcrLanguage, Recognize, ocr_languages};
pub use pages::{LabelStyle, combine, parse_ranges};
pub use redact::{Find, PATTERNS};
pub use render::{PageImage, Preview, RenderPool, Tile, render, render_tile, rendered_size};
pub use secure::{INFO_FIELDS, Optimize, Protection, Sanitize, Security};
pub use sign::{Certificate, SignField, SignWith, Signature, Signing, Trust, certificates};
pub use stamp::{Bates, Overlay, Place};
pub use text::{PageText, TextChar, page_text, search};
pub use types::{Attachment, Layer, Link, LinkTarget, OutlineItem, Rect};
