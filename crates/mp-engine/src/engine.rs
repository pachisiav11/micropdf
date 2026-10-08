use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};

use mupdf::link::LinkDestination;
use mupdf::pdf::{PdfDocument, PdfObject};
use mupdf::{DestinationKind, DisplayList, Document, MetadataName, Outline};

use crate::annots::{self, Annot, History, NewAnnot, Style};
use crate::forms::{self, Field, FieldEdit, Xfa};
use crate::marks::{self, Mark};
use crate::xfdf;
use crate::{Attachment, Error, Layer, Link, LinkTarget, OutlineItem, Rect};

/// Display lists kept per engine. Re-rendering a page at a new zoom reuses its list.
const LIST_CACHE_CAPACITY: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DocId(u32);

#[derive(Debug, Clone, Copy)]
pub struct DocInfo {
    pub id: DocId,
    /// 0 until a password-protected document is unlocked with [`Engine::authenticate`].
    pub page_count: usize,
    pub needs_password: bool,
}

/// Page size in PDF points (1/72 inch), after the page's own rotation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSize {
    pub width: f32,
    pub height: f32,
}

type Reply<T> = Sender<Result<T, Error>>;

enum Command {
    Open {
        path: PathBuf,
        reply: Reply<DocInfo>,
    },
    Authenticate {
        doc: DocId,
        password: String,
        reply: Reply<Option<usize>>,
    },
    PageSizes {
        doc: DocId,
        reply: Reply<Vec<PageSize>>,
    },
    DisplayList {
        doc: DocId,
        page: usize,
        reply: Reply<Arc<DisplayList>>,
    },
    Links {
        doc: DocId,
        page: usize,
        reply: Reply<Vec<Link>>,
    },
    Outline {
        doc: DocId,
        reply: Reply<Vec<OutlineItem>>,
    },
    Metadata {
        doc: DocId,
        reply: Reply<Vec<(String, String)>>,
    },
    Attachments {
        doc: DocId,
        reply: Reply<Vec<Attachment>>,
    },
    AttachmentData {
        doc: DocId,
        index: usize,
        reply: Reply<Vec<u8>>,
    },
    Layers {
        doc: DocId,
        reply: Reply<Vec<Layer>>,
    },
    ToggleLayer {
        doc: DocId,
        index: usize,
        reply: Reply<Vec<Layer>>,
    },
    Annotations {
        doc: DocId,
        page: usize,
        reply: Reply<Vec<Annot>>,
    },
    AddAnnotation {
        doc: DocId,
        page: usize,
        new: NewAnnot,
        style: Style,
        reply: Reply<Annot>,
    },
    DeleteAnnotation {
        doc: DocId,
        page: usize,
        id: i32,
        reply: Reply<()>,
    },
    Fields {
        doc: DocId,
        page: usize,
        reply: Reply<Vec<Field>>,
    },
    SetContents {
        doc: DocId,
        page: usize,
        id: i32,
        text: String,
        reply: Reply<()>,
    },
    Flatten {
        doc: DocId,
        comments: bool,
        fields: bool,
        reply: Reply<()>,
    },
    SetColor {
        doc: DocId,
        page: usize,
        id: i32,
        color: [f32; 3],
        reply: Reply<()>,
    },
    Reply {
        doc: DocId,
        page: usize,
        parent: i32,
        text: String,
        author: String,
        reply: Reply<Annot>,
    },
    SetState {
        doc: DocId,
        page: usize,
        parent: i32,
        state: String,
        author: String,
        reply: Reply<()>,
    },
    Reshape {
        doc: DocId,
        page: usize,
        id: i32,
        rect: Rect,
        reply: Reply<()>,
    },
    PlaceMark {
        doc: DocId,
        page: usize,
        mark: Mark,
        center: (f32, f32),
        width: f32,
        color: [f32; 3],
        reply: Reply<Annot>,
    },
    MarkAspect {
        mark: Mark,
        reply: Reply<f32>,
    },
    ExportComments {
        doc: DocId,
        file: String,
        reply: Reply<(String, usize)>,
    },
    ImportComments {
        doc: DocId,
        xml: String,
        reply: Reply<usize>,
    },
    EditField {
        doc: DocId,
        page: usize,
        id: i32,
        edit: FieldEdit,
        reply: Reply<()>,
    },
    ResetForm {
        doc: DocId,
        reply: Reply<()>,
    },
    Xfa {
        doc: DocId,
        reply: Reply<Xfa>,
    },
    ExportXfdf {
        doc: DocId,
        file: String,
        reply: Reply<String>,
    },
    ImportXfdf {
        doc: DocId,
        xml: String,
        reply: Reply<usize>,
    },
    Save {
        doc: DocId,
        target: PathBuf,
        incremental: bool,
        reply: Reply<bool>,
    },
    History {
        doc: DocId,
        reply: Reply<History>,
    },
    Undo {
        doc: DocId,
        redo: bool,
        reply: Reply<History>,
    },
    Close {
        doc: DocId,
    },
}

/// Handle to the engine thread. Cheap to share by reference across threads.
pub struct Engine {
    tx: Option<Sender<Command>>,
    thread: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn start() -> Self {
        crate::fonts::install();
        let (tx, rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("mp-engine".into())
            .spawn(move || run(rx))
            .expect("failed to spawn engine thread");
        Engine {
            tx: Some(tx),
            thread: Some(thread),
        }
    }

    pub fn open(&self, path: impl Into<PathBuf>) -> Result<DocInfo, Error> {
        let path = path.into();
        self.call(|reply| Command::Open { path, reply })
    }

    /// Unlocks a password-protected document. Returns the page count, or None for a wrong
    /// password.
    pub fn authenticate(&self, doc: DocId, password: &str) -> Result<Option<usize>, Error> {
        let password = password.to_owned();
        self.call(|reply| Command::Authenticate {
            doc,
            password,
            reply,
        })
    }

    pub fn page_sizes(&self, doc: DocId) -> Result<Vec<PageSize>, Error> {
        self.call(|reply| Command::PageSizes { doc, reply })
    }

    /// Display list for one page, annotations included.
    pub fn display_list(&self, doc: DocId, page: usize) -> Result<Arc<DisplayList>, Error> {
        self.call(|reply| Command::DisplayList { doc, page, reply })
    }

    pub fn links(&self, doc: DocId, page: usize) -> Result<Vec<Link>, Error> {
        self.call(|reply| Command::Links { doc, page, reply })
    }

    pub fn outline(&self, doc: DocId) -> Result<Vec<OutlineItem>, Error> {
        self.call(|reply| Command::Outline { doc, reply })
    }

    /// Non-empty document information entries as (label, value).
    pub fn metadata(&self, doc: DocId) -> Result<Vec<(String, String)>, Error> {
        self.call(|reply| Command::Metadata { doc, reply })
    }

    /// Files embedded in the document (the EmbeddedFiles name tree), in tree order.
    pub fn attachments(&self, doc: DocId) -> Result<Vec<Attachment>, Error> {
        self.call(|reply| Command::Attachments { doc, reply })
    }

    /// The contents of attachment `index` from [`Engine::attachments`].
    pub fn attachment_data(&self, doc: DocId, index: usize) -> Result<Vec<u8>, Error> {
        self.call(|reply| Command::AttachmentData { doc, index, reply })
    }

    /// Rows of the document's layers (optional content) panel. Empty when it has none.
    pub fn layers(&self, doc: DocId) -> Result<Vec<Layer>, Error> {
        self.call(|reply| Command::Layers { doc, reply })
    }

    /// Shows or hides layer `index` and returns the updated rows. Display lists made before
    /// the change still show the old state; ask for new ones.
    pub fn toggle_layer(&self, doc: DocId, index: usize) -> Result<Vec<Layer>, Error> {
        self.call(|reply| Command::ToggleLayer { doc, index, reply })
    }

    /// Annotations on `page` (not links, form fields or popups).
    pub fn annotations(&self, doc: DocId, page: usize) -> Result<Vec<Annot>, Error> {
        self.call(|reply| Command::Annotations { doc, page, reply })
    }

    /// Adds an annotation. Ask for a new display list of the page afterwards.
    pub fn add_annotation(
        &self,
        doc: DocId,
        page: usize,
        new: NewAnnot,
        style: Style,
    ) -> Result<Annot, Error> {
        self.call(|reply| Command::AddAnnotation {
            doc,
            page,
            new,
            style,
            reply,
        })
    }

    pub fn delete_annotation(&self, doc: DocId, page: usize, id: i32) -> Result<(), Error> {
        self.call(|reply| Command::DeleteAnnotation {
            doc,
            page,
            id,
            reply,
        })
    }

    /// Replaces a comment's text.
    pub fn set_contents(
        &self,
        doc: DocId,
        page: usize,
        id: i32,
        text: String,
    ) -> Result<(), Error> {
        self.call(|reply| Command::SetContents {
            doc,
            page,
            id,
            text,
            reply,
        })
    }

    /// Answers comment `parent` on `page` with `text`.
    pub fn reply(
        &self,
        doc: DocId,
        page: usize,
        parent: i32,
        text: String,
        author: String,
    ) -> Result<Annot, Error> {
        self.call(|reply| Command::Reply {
            doc,
            page,
            parent,
            text,
            author,
            reply,
        })
    }

    /// Gives comment `parent` a review state, one of [`crate::REVIEW_STATES`].
    pub fn set_state(
        &self,
        doc: DocId,
        page: usize,
        parent: i32,
        state: String,
        author: String,
    ) -> Result<(), Error> {
        self.call(|reply| Command::SetState {
            doc,
            page,
            parent,
            state,
            author,
            reply,
        })
    }

    /// Moves or resizes comment `id` so its bounds become `rect` (page space).
    pub fn reshape(&self, doc: DocId, page: usize, id: i32, rect: Rect) -> Result<(), Error> {
        self.call(|reply| Command::Reshape {
            doc,
            page,
            id,
            rect,
            reply,
        })
    }

    pub fn set_color(
        &self,
        doc: DocId,
        page: usize,
        id: i32,
        color: [f32; 3],
    ) -> Result<(), Error> {
        self.call(|reply| Command::SetColor {
            doc,
            page,
            id,
            color,
            reply,
        })
    }

    /// Places a signature or initials, `width` points wide and centred on `center`.
    pub fn place_mark(
        &self,
        doc: DocId,
        page: usize,
        mark: Mark,
        center: (f32, f32),
        width: f32,
        color: [f32; 3],
    ) -> Result<Annot, Error> {
        self.call(|reply| Command::PlaceMark {
            doc,
            page,
            mark,
            center,
            width,
            color,
            reply,
        })
    }

    /// The comments as XFDF, and how many; `file` is the PDF's name, recorded in the XFDF.
    pub fn export_comments(&self, doc: DocId, file: String) -> Result<(String, usize), Error> {
        self.call(|reply| Command::ExportComments { doc, file, reply })
    }

    /// Adds the comments in an XFDF file. Returns how many were added.
    pub fn import_comments(&self, doc: DocId, xml: String) -> Result<usize, Error> {
        self.call(|reply| Command::ImportComments { doc, xml, reply })
    }

    /// A mark's width over its height.
    pub fn mark_aspect(&self, mark: Mark) -> Result<f32, Error> {
        self.call(|reply| Command::MarkAspect { mark, reply })
    }

    /// Draws comments and/or form fields into the page content; see [`crate::annots::flatten`].
    pub fn flatten(&self, doc: DocId, comments: bool, fields: bool) -> Result<(), Error> {
        self.call(|reply| Command::Flatten {
            doc,
            comments,
            fields,
            reply,
        })
    }

    /// The page's form widgets.
    pub fn fields(&self, doc: DocId, page: usize) -> Result<Vec<Field>, Error> {
        self.call(|reply| Command::Fields { doc, page, reply })
    }

    /// Fills in or toggles a field. Ask for new display lists afterwards; calculated fields
    /// on other pages may change too.
    pub fn edit_field(
        &self,
        doc: DocId,
        page: usize,
        id: i32,
        edit: FieldEdit,
    ) -> Result<(), Error> {
        self.call(|reply| Command::EditField {
            doc,
            page,
            id,
            edit,
            reply,
        })
    }

    pub fn reset_form(&self, doc: DocId) -> Result<(), Error> {
        self.call(|reply| Command::ResetForm { doc, reply })
    }

    /// Whether the form carries XFA; see [`Xfa`].
    pub fn xfa(&self, doc: DocId) -> Result<Xfa, Error> {
        self.call(|reply| Command::Xfa { doc, reply })
    }

    /// The form's values as XFDF; `file` is the PDF's name, recorded in the XFDF.
    pub fn export_xfdf(&self, doc: DocId, file: String) -> Result<String, Error> {
        self.call(|reply| Command::ExportXfdf { doc, file, reply })
    }

    /// Fills the form from XFDF. Returns how many fields took a value.
    pub fn import_xfdf(&self, doc: DocId, xml: String) -> Result<usize, Error> {
        self.call(|reply| Command::ImportXfdf { doc, xml, reply })
    }

    /// Writes the document to `target`; see [`crate::annots::save`] for `incremental`.
    ///
    /// Saving over the open file appends to it when it can. Otherwise the whole file is
    /// rewritten beside it and swapped in, and the document opens again: it returns true, the
    /// edit history is gone and comment ids may differ.
    pub fn save(&self, doc: DocId, target: &Path, incremental: bool) -> Result<bool, Error> {
        let target = target.to_path_buf();
        self.call(|reply| Command::Save {
            doc,
            target,
            incremental,
            reply,
        })
    }

    /// What Undo and Redo would do now.
    pub fn history(&self, doc: DocId) -> Result<History, Error> {
        self.call(|reply| Command::History { doc, reply })
    }

    /// Takes back the last edit. Any page may change; ask for new display lists.
    pub fn undo(&self, doc: DocId) -> Result<History, Error> {
        self.call(|reply| Command::Undo {
            doc,
            redo: false,
            reply,
        })
    }

    pub fn redo(&self, doc: DocId) -> Result<History, Error> {
        self.call(|reply| Command::Undo {
            doc,
            redo: true,
            reply,
        })
    }

    /// Display lists already handed out stay valid after the document closes.
    pub fn close(&self, doc: DocId) {
        let _ = self.sender().send(Command::Close { doc });
    }

    fn call<T>(&self, command: impl FnOnce(Reply<T>) -> Command) -> Result<T, Error> {
        let (reply, rx) = mpsc::channel();
        self.sender()
            .send(command(reply))
            .map_err(|_| Error::Stopped)?;
        rx.recv().map_err(|_| Error::Stopped)?
    }

    fn sender(&self) -> &Sender<Command> {
        self.tx
            .as_ref()
            .expect("engine sender is only taken in drop")
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(rx: mpsc::Receiver<Command>) {
    let mut docs: HashMap<DocId, Document> = HashMap::new();
    let mut paths: HashMap<DocId, PathBuf> = HashMap::new();
    // Kept to open an encrypted document again after a save rewrites it.
    let mut passwords: HashMap<DocId, String> = HashMap::new();
    let mut lists = ListCache::default();
    let mut next_id = 0;

    for command in rx {
        match command {
            Command::Open { path, reply } => {
                let result = (|| {
                    // On Windows mupdf only accepts UTF-8 string paths.
                    let path = path.to_str().ok_or(mupdf::Error::InvalidUtf8)?;
                    let doc = Document::open(path)?;
                    annots::enable_journal(&doc)?;
                    let needs_password = doc.needs_password()?;
                    let page_count = if needs_password {
                        0
                    } else {
                        doc.page_count()? as usize
                    };
                    next_id += 1;
                    let id = DocId(next_id);
                    docs.insert(id, doc);
                    paths.insert(id, path.into());
                    Ok(DocInfo {
                        id,
                        page_count,
                        needs_password,
                    })
                })();
                let _ = reply.send(result);
            }
            Command::Authenticate {
                doc,
                password,
                reply,
            } => {
                let result = match docs.get_mut(&doc) {
                    None => Err(Error::UnknownDocument),
                    Some(d) => (|| {
                        if !d.authenticate(&password)? {
                            return Ok(None);
                        }
                        Ok(Some(d.page_count()? as usize))
                    })(),
                };
                if let Ok(Some(_)) = result {
                    passwords.insert(doc, password);
                }
                let _ = reply.send(result);
            }
            Command::PageSizes { doc, reply } => {
                let result = with_doc(&docs, doc, |d| {
                    (0..d.page_count()?)
                        .map(|i| {
                            let b = d.load_page(i)?.bounds()?;
                            Ok(PageSize {
                                width: b.x1 - b.x0,
                                height: b.y1 - b.y0,
                            })
                        })
                        .collect()
                });
                let _ = reply.send(result);
            }
            Command::DisplayList { doc, page, reply } => {
                let result = match (lists.get(doc, page), docs.get(&doc)) {
                    (_, None) => Err(Error::UnknownDocument),
                    (Some(list), _) => Ok(list),
                    (None, Some(d)) => (|| {
                        let loaded = d.load_page(page as i32)?;
                        annots::hide_replies(&loaded)?;
                        let list = Arc::new(loaded.to_display_list(true)?);
                        lists.insert(doc, page, Arc::clone(&list));
                        Ok(list)
                    })(),
                };
                let _ = reply.send(result);
            }
            Command::Links { doc, page, reply } => {
                let result = with_doc(&docs, doc, |d| {
                    Ok(d.load_page(page as i32)?
                        .links()?
                        .filter_map(|l| {
                            Some(Link {
                                rect: l.bounds.into(),
                                target: link_target(l.dest, Some(&l.uri))?,
                            })
                        })
                        .collect())
                });
                let _ = reply.send(result);
            }
            Command::Outline { doc, reply } => {
                let result = with_doc(&docs, doc, |d| {
                    let mut items = Vec::new();
                    flatten_outline(&d.outlines()?, 0, &mut items);
                    Ok(items)
                });
                let _ = reply.send(result);
            }
            Command::Metadata { doc, reply } => {
                let result = with_doc(&docs, doc, |d| {
                    let fields = [
                        ("Title", MetadataName::Title),
                        ("Author", MetadataName::Author),
                        ("Subject", MetadataName::Subject),
                        ("Keywords", MetadataName::Keywords),
                        ("Creator", MetadataName::Creator),
                        ("Producer", MetadataName::Producer),
                        ("Created", MetadataName::CreationDate),
                        ("Modified", MetadataName::ModDate),
                        ("Format", MetadataName::Format),
                        ("Encryption", MetadataName::Encryption),
                    ];
                    let mut out = Vec::new();
                    for (label, name) in fields {
                        let value = d.metadata(name)?;
                        if !value.trim().is_empty() {
                            out.push((label.to_owned(), value));
                        }
                    }
                    Ok(out)
                });
                let _ = reply.send(result);
            }
            Command::Attachments { doc, reply } => {
                let result = with_doc(&docs, doc, |d| {
                    Ok(embedded_files(d)?
                        .into_iter()
                        .map(|(name, spec)| Attachment {
                            size: file_size(&spec),
                            name,
                        })
                        .collect())
                });
                let _ = reply.send(result);
            }
            Command::AttachmentData { doc, index, reply } => {
                let result = with_doc(&docs, doc, |d| {
                    let files = embedded_files(d)?;
                    let (_, spec) = files.get(index).ok_or(Error::NotFound)?;
                    let stream = spec
                        .get_dict("EF")?
                        .and_then(|ef| {
                            ef.get_dict("UF").ok().flatten().or(ef.get_dict("F").ok()?)
                        })
                        .ok_or(Error::NotFound)?;
                    Ok(stream.read_stream()?)
                });
                let _ = reply.send(result);
            }
            Command::Layers { doc, reply } => {
                let _ = reply.send(with_doc(&docs, doc, |d| Ok(layers(d))));
            }
            Command::ToggleLayer { doc, index, reply } => {
                let result = with_doc(&docs, doc, |d| {
                    if let Ok(mut pdf) = PdfDocument::try_from(d.clone()) {
                        pdf.toggle_layer_ui(index as i32);
                    }
                    Ok(layers(d))
                });
                lists.remove_doc(doc);
                let _ = reply.send(result);
            }
            Command::SetContents {
                doc,
                page,
                id,
                text,
                reply,
            } => {
                let result = with_doc(&docs, doc, |d| annots::set_contents(d, page, id, &text));
                lists.remove_page(doc, page);
                let _ = reply.send(result);
            }
            Command::Reply {
                doc,
                page,
                parent,
                text,
                author,
                reply,
            } => {
                let result = with_doc(&docs, doc, |d| {
                    annots::reply(d, page, parent, &text, &author)
                });
                lists.remove_page(doc, page);
                let _ = reply.send(result);
            }
            Command::SetState {
                doc,
                page,
                parent,
                state,
                author,
                reply,
            } => {
                let result = with_doc(&docs, doc, |d| {
                    annots::set_state(d, page, parent, &state, &author)
                });
                lists.remove_page(doc, page);
                let _ = reply.send(result);
            }
            Command::Reshape {
                doc,
                page,
                id,
                rect,
                reply,
            } => {
                let result = with_doc(&docs, doc, |d| annots::reshape(d, page, id, rect));
                lists.remove_page(doc, page);
                let _ = reply.send(result);
            }
            Command::SetColor {
                doc,
                page,
                id,
                color,
                reply,
            } => {
                let result = with_doc(&docs, doc, |d| annots::set_color(d, page, id, color));
                lists.remove_page(doc, page);
                let _ = reply.send(result);
            }
            Command::PlaceMark {
                doc,
                page,
                mark,
                center,
                width,
                color,
                reply,
            } => {
                let result = with_doc(&docs, doc, |d| {
                    marks::place(d, page, &mark, center, width, color)
                });
                lists.remove_page(doc, page);
                let _ = reply.send(result);
            }
            Command::MarkAspect { mark, reply } => {
                let _ = reply.send(marks::aspect(&mark));
            }
            Command::ExportComments { doc, file, reply } => {
                let _ = reply.send(with_doc(&docs, doc, |d| xfdf::export(d, &file)));
            }
            Command::ImportComments { doc, xml, reply } => {
                let result = with_doc(&docs, doc, |d| xfdf::import(d, &xml));
                lists.remove_doc(doc);
                let _ = reply.send(result);
            }
            Command::Flatten {
                doc,
                comments,
                fields,
                reply,
            } => {
                let result = with_doc(&docs, doc, |d| annots::flatten(d, comments, fields));
                lists.remove_doc(doc);
                let _ = reply.send(result);
            }
            Command::Fields { doc, page, reply } => {
                let _ = reply.send(with_doc(&docs, doc, |d| forms::list(d, page)));
            }
            Command::EditField {
                doc,
                page,
                id,
                edit,
                reply,
            } => {
                let result = with_doc(&docs, doc, |d| forms::edit(d, page, id, &edit));
                lists.remove_doc(doc);
                let _ = reply.send(result);
            }
            Command::ExportXfdf { doc, file, reply } => {
                let _ = reply.send(with_doc(&docs, doc, |d| forms::export_xfdf(d, &file)));
            }
            Command::ImportXfdf { doc, xml, reply } => {
                let result = with_doc(&docs, doc, |d| forms::import_xfdf(d, &xml));
                lists.remove_doc(doc);
                let _ = reply.send(result);
            }
            Command::Xfa { doc, reply } => {
                let _ = reply.send(with_doc(&docs, doc, forms::xfa));
            }
            Command::ResetForm { doc, reply } => {
                let result = with_doc(&docs, doc, forms::reset);
                lists.remove_doc(doc);
                let _ = reply.send(result);
            }
            Command::Annotations { doc, page, reply } => {
                let _ = reply.send(with_doc(&docs, doc, |d| annots::list(d, page)));
            }
            Command::AddAnnotation {
                doc,
                page,
                new,
                style,
                reply,
            } => {
                let result = with_doc(&docs, doc, |d| annots::add(d, page, &new, &style));
                lists.remove_page(doc, page);
                let _ = reply.send(result);
            }
            Command::DeleteAnnotation {
                doc,
                page,
                id,
                reply,
            } => {
                let result = with_doc(&docs, doc, |d| annots::delete(d, page, id));
                lists.remove_page(doc, page);
                let _ = reply.send(result);
            }
            Command::Save {
                doc,
                target,
                incremental,
                reply,
            } => {
                let result = (|| {
                    let original = paths.get(&doc).ok_or(Error::UnknownDocument)?.clone();
                    let d = docs.get(&doc).ok_or(Error::UnknownDocument)?;
                    if !annots::same_file(&original, &target)
                        || (incremental && annots::can_append(d))
                    {
                        annots::save(d, &original, &target, incremental)?;
                        return Ok(false);
                    }
                    // MuPDF reads the open file as it goes, so a full rewrite cannot go over
                    // it. Write a sibling, close the document, swap the files, open again.
                    let temp = annots::sibling(&original);
                    if let Err(e) = annots::save(d, &original, &temp, false) {
                        let _ = std::fs::remove_file(&temp);
                        return Err(e);
                    }
                    docs.remove(&doc);
                    lists.remove_doc(doc);
                    let replaced = annots::replace(&original, &temp);
                    match reopen(&original, passwords.get(&doc)) {
                        Ok(d) => {
                            docs.insert(doc, d);
                        }
                        Err(e) => {
                            paths.remove(&doc);
                            passwords.remove(&doc);
                            return Err(e);
                        }
                    }
                    replaced?;
                    Ok(true)
                })();
                let _ = reply.send(result);
            }
            Command::History { doc, reply } => {
                let _ = reply.send(with_doc(&docs, doc, annots::history));
            }
            Command::Undo { doc, redo, reply } => {
                let result = with_doc(&docs, doc, |d| {
                    if redo {
                        annots::redo(d)?;
                    } else {
                        annots::undo(d)?;
                    }
                    annots::history(d)
                });
                lists.remove_doc(doc);
                let _ = reply.send(result);
            }
            Command::Close { doc } => {
                paths.remove(&doc);
                passwords.remove(&doc);
                docs.remove(&doc);
                lists.remove_doc(doc);
            }
        }
    }
}

/// Opens `path` again after a save rewrote it, unlocked with the password it had.
fn reopen(path: &Path, password: Option<&String>) -> Result<Document, Error> {
    let path = path.to_str().ok_or(mupdf::Error::InvalidUtf8)?;
    let mut doc = Document::open(path)?;
    annots::enable_journal(&doc)?;
    if let Some(password) = password
        && !doc.authenticate(password)?
    {
        return Err(Error::Invalid(
            "the saved file no longer takes the password",
        ));
    }
    Ok(doc)
}

/// (file name, file specification) for each embedded file. Empty for non-PDF documents.
fn embedded_files(doc: &Document) -> Result<Vec<(String, PdfObject)>, Error> {
    let Ok(pdf) = PdfDocument::try_from(doc.clone()) else {
        return Ok(Vec::new());
    };
    // MuPDF takes the tree's name and finds it under the catalog's /Names itself.
    let map = pdf.load_name_tree(PdfObject::new_name("EmbeddedFiles")?)?;
    let mut out = Vec::new();
    for i in 0..map.dict_len()? as i32 {
        let (Some(key), Some(spec)) = (map.get_dict_key(i)?, map.get_dict_val(i)?) else {
            continue;
        };
        let fallback = String::from_utf8_lossy(&key.as_name().unwrap_or_default()).into_owned();
        let name = ["UF", "F"]
            .iter()
            .find_map(|k| spec.get_dict(*k).ok().flatten()?.as_string().ok())
            .filter(|n| !n.is_empty())
            .unwrap_or(fallback);
        out.push((name, spec));
    }
    Ok(out)
}

fn layers(doc: &Document) -> Vec<Layer> {
    PdfDocument::try_from(doc.clone())
        .map(|pdf| {
            pdf.layer_ui()
                .into_iter()
                .map(|l| Layer {
                    name: l.text,
                    depth: l.depth,
                    toggle: l.toggle,
                    visible: l.selected,
                    locked: l.locked,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn file_size(spec: &PdfObject) -> Option<usize> {
    let ef = spec.get_dict("EF").ok()??;
    let stream = ef.get_dict("F").ok()??;
    let size = stream.get_dict("Params").ok()??.get_dict("Size").ok()??;
    size.as_int().ok().map(|s| s.max(0) as usize)
}

fn with_doc<T>(
    docs: &HashMap<DocId, Document>,
    doc: DocId,
    f: impl FnOnce(&Document) -> Result<T, Error>,
) -> Result<T, Error> {
    docs.get(&doc).map_or(Err(Error::UnknownDocument), f)
}

fn link_target(dest: Option<LinkDestination>, uri: Option<&str>) -> Option<LinkTarget> {
    match (dest, uri) {
        (Some(d), _) => Some(LinkTarget::Page {
            page: d.loc.page_number as usize,
            top: match d.kind {
                DestinationKind::XYZ { top, .. }
                | DestinationKind::FitH { top }
                | DestinationKind::FitBH { top } => top,
                DestinationKind::FitR { top, .. } => Some(top),
                _ => None,
            },
        }),
        (None, Some(uri)) if !uri.is_empty() && !uri.starts_with('#') => {
            Some(LinkTarget::Uri(uri.to_owned()))
        }
        _ => None,
    }
}

fn flatten_outline(items: &[Outline], depth: usize, out: &mut Vec<OutlineItem>) {
    for item in items {
        out.push(OutlineItem {
            title: item.title.trim().to_owned(),
            depth,
            target: link_target(item.dest, item.uri.as_deref()),
        });
        flatten_outline(&item.down, depth + 1, out);
    }
}

#[derive(Default)]
struct ListCache {
    lists: HashMap<(DocId, usize), Arc<DisplayList>>,
    /// Least recently used first.
    order: VecDeque<(DocId, usize)>,
}

impl ListCache {
    fn get(&mut self, doc: DocId, page: usize) -> Option<Arc<DisplayList>> {
        let list = self.lists.get(&(doc, page))?.clone();
        self.touch((doc, page));
        Some(list)
    }

    fn insert(&mut self, doc: DocId, page: usize, list: Arc<DisplayList>) {
        if self.lists.insert((doc, page), list).is_none() {
            self.order.push_back((doc, page));
        } else {
            self.touch((doc, page));
        }
        while self.order.len() > LIST_CACHE_CAPACITY {
            if let Some(key) = self.order.pop_front() {
                self.lists.remove(&key);
            }
        }
    }

    fn remove_page(&mut self, doc: DocId, page: usize) {
        self.lists.remove(&(doc, page));
        self.order.retain(|k| *k != (doc, page));
    }

    fn remove_doc(&mut self, doc: DocId) {
        self.lists.retain(|(d, _), _| *d != doc);
        self.order.retain(|(d, _)| *d != doc);
    }

    fn touch(&mut self, key: (DocId, usize)) {
        if let Some(pos) = self.order.iter().position(|k| *k == key) {
            self.order.remove(pos);
            self.order.push_back(key);
        }
    }
}
