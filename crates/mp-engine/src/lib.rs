//! MuPDF behind two thread boundaries.
//!
//! MuPDF documents are not thread-safe, so one engine thread owns every open document and
//! answers commands over a channel. Display lists are safe to share, so the engine hands them
//! out as `Arc<DisplayList>` and a pool of render workers (each with its own cloned
//! `fz_context`, managed by the `mupdf` crate) turns them into pixels and text.

mod annots;
mod engine;
mod error;
mod fonts;
mod forms;
mod marks;
mod render;
mod text;
mod types;

pub use annots::{Annot, AnnotKind, History, NewAnnot, Style};
pub use engine::{DocId, DocInfo, Engine, PageSize};
pub use error::Error;
pub use fonts::has_cjk;
pub use forms::{Field, FieldEdit, FieldKind, Xfa};
pub use marks::Mark;
pub use mupdf::CjkFontOrdering;
pub use mupdf::DisplayList;
pub use render::{PageImage, RenderPool, Tile, render, render_tile, rendered_size};
pub use text::{PageText, TextChar, page_text, search};
pub use types::{Attachment, Layer, Link, LinkTarget, OutlineItem, Rect};
