//! MuPDF behind two thread boundaries.
//!
//! MuPDF documents are not thread-safe, so one engine thread owns every open document and
//! answers commands over a channel. Display lists are safe to share, so the engine hands them
//! out as `Arc<DisplayList>` and a pool of render workers (each with its own cloned
//! `fz_context`, managed by the `mupdf` crate) turns them into pixels.

mod engine;
mod error;
mod render;

pub use engine::{DocId, DocInfo, Engine, PageSize};
pub use error::Error;
pub use mupdf::DisplayList;
pub use render::{PageImage, RenderPool, render};
