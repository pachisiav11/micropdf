use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};

use mupdf::link::LinkDestination;
use mupdf::pdf::PdfDocument;
use mupdf::{DestinationKind, DisplayList, Document, MetadataName, Outline};

use crate::annots::{self, Annot, History, NewAnnot, Properties, Restyle, Style};
use crate::attachments;
use crate::forms::{self, Field, FieldEdit, Xfa};
use crate::marks::{self, Mark};
use crate::render::Preview;
use crate::secure::{self, Security};
use crate::summary;
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

/// Work for the engine thread on one document, given the password that opened it; see
/// [`Engine::edit`].
type Job = Box<dyn FnOnce(&Document, Option<&str>) -> Result<Box<dyn Any + Send>, Error> + Send>;

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
    AddAttachment {
        doc: DocId,
        name: String,
        data: Vec<u8>,
        reply: Reply<()>,
    },
    DeleteAttachment {
        doc: DocId,
        index: usize,
        reply: Reply<()>,
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
    HasFields {
        doc: DocId,
        reply: Reply<bool>,
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
    Restyle {
        doc: DocId,
        page: usize,
        id: i32,
        change: Restyle,
        reply: Reply<()>,
    },
    SetProperties {
        doc: DocId,
        page: usize,
        id: i32,
        props: Properties,
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
    MarkPreview {
        mark: Mark,
        height: u32,
        reply: Reply<Preview>,
    },
    StampPreview {
        name: String,
        color: [f32; 3],
        height: u32,
        reply: Reply<Preview>,
    },
    SummarizeComments {
        doc: DocId,
        file: String,
        target: PathBuf,
        reply: Reply<usize>,
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
    ExportFdf {
        doc: DocId,
        file: String,
        reply: Reply<Vec<u8>>,
    },
    ImportFdf {
        doc: DocId,
        data: Vec<u8>,
        reply: Reply<usize>,
    },
    Save {
        doc: DocId,
        target: PathBuf,
        incremental: bool,
        security: Security,
        reply: Reply<bool>,
    },
    Edit {
        doc: DocId,
        /// The undo step's name; empty for work that changes nothing.
        name: &'static str,
        /// The next save must rewrite the file, so that removed content leaves it.
        rewrite: bool,
        job: Job,
        reply: Reply<Box<dyn Any + Send>>,
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

    /// Files embedded in the document: its own (the EmbeddedFiles name tree) in tree order,
    /// then files attached to pages as comments, page by page.
    pub fn attachments(&self, doc: DocId) -> Result<Vec<Attachment>, Error> {
        self.call(|reply| Command::Attachments { doc, reply })
    }

    /// The contents of attachment `index` from [`Engine::attachments`].
    pub fn attachment_data(&self, doc: DocId, index: usize) -> Result<Vec<u8>, Error> {
        self.call(|reply| Command::AttachmentData { doc, index, reply })
    }

    /// Embeds `data` in the document as a file called `name`, as one undoable step.
    pub fn add_attachment(&self, doc: DocId, name: String, data: Vec<u8>) -> Result<(), Error> {
        self.call(|reply| Command::AddAttachment {
            doc,
            name,
            data,
            reply,
        })
    }

    /// Removes attachment `index` from [`Engine::attachments`]; a file attached to a page goes
    /// with its comment.
    pub fn delete_attachment(&self, doc: DocId, index: usize) -> Result<(), Error> {
        self.call(|reply| Command::DeleteAttachment { doc, index, reply })
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

    /// Changes one part of comment `id`'s look, such as its colour or border.
    pub fn restyle(&self, doc: DocId, page: usize, id: i32, change: Restyle) -> Result<(), Error> {
        self.call(|reply| Command::Restyle {
            doc,
            page,
            id,
            change,
            reply,
        })
    }

    /// Sets comment `id`'s author and subject, whether it is locked and whether it prints.
    pub fn set_properties(
        &self,
        doc: DocId,
        page: usize,
        id: i32,
        props: Properties,
    ) -> Result<(), Error> {
        self.call(|reply| Command::SetProperties {
            doc,
            page,
            id,
            props,
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

    /// Writes a PDF that lists the comments beside their pages to `target`; `file` is the
    /// document's name, for the title. Returns how many comments it lists, without replies.
    pub fn summarize_comments(
        &self,
        doc: DocId,
        file: String,
        target: PathBuf,
    ) -> Result<usize, Error> {
        self.call(|reply| Command::SummarizeComments {
            doc,
            file,
            target,
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

    /// A mark drawn black, `height` pixels tall, to show where it will go.
    pub fn mark_preview(&self, mark: Mark, height: u32) -> Result<Preview, Error> {
        self.call(|reply| Command::MarkPreview {
            mark,
            height,
            reply,
        })
    }

    /// The standard stamp `name` in `color`, `height` pixels tall.
    pub fn stamp_preview(
        &self,
        name: String,
        color: [f32; 3],
        height: u32,
    ) -> Result<Preview, Error> {
        self.call(|reply| Command::StampPreview {
            name,
            color,
            height,
            reply,
        })
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

    /// Whether the document has form fields, without loading its pages.
    pub fn has_fields(&self, doc: DocId) -> Result<bool, Error> {
        self.call(|reply| Command::HasFields { doc, reply })
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

    /// The form's values as FDF; `file` is the PDF's name, recorded in the FDF.
    pub fn export_fdf(&self, doc: DocId, file: String) -> Result<Vec<u8>, Error> {
        self.call(|reply| Command::ExportFdf { doc, file, reply })
    }

    /// Fills the form from FDF. Returns how many fields took a value.
    pub fn import_fdf(&self, doc: DocId, data: Vec<u8>) -> Result<usize, Error> {
        self.call(|reply| Command::ImportFdf { doc, data, reply })
    }

    /// Writes the document to `target`; see [`crate::annots::save`] for `incremental`.
    ///
    /// Saving over the open file appends to it when it can. Otherwise the whole file is
    /// rewritten beside it and swapped in, and the document opens again: it returns true, the
    /// edit history is gone and comment ids may differ.
    pub fn save(&self, doc: DocId, target: &Path, incremental: bool) -> Result<bool, Error> {
        self.save_secured(doc, target, incremental, Security::Keep)
    }

    /// [`Engine::save`] that also adds, keeps or removes the file's password and permissions.
    /// Any change to them rewrites the whole file.
    pub fn save_secured(
        &self,
        doc: DocId,
        target: &Path,
        incremental: bool,
        security: Security,
    ) -> Result<bool, Error> {
        let target = target.to_path_buf();
        self.call(|reply| Command::Save {
            doc,
            target,
            incremental: incremental && matches!(security, Security::Keep),
            security,
            reply,
        })
    }

    /// Runs `f` on the engine thread as one undoable step called `name`, and forgets the
    /// document's display lists: any page may have changed, or moved.
    pub(crate) fn edit<T: Send + 'static>(
        &self,
        doc: DocId,
        name: &'static str,
        f: impl FnOnce(&Document) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        self.job(doc, name, false, move |d, _| f(d))
    }

    /// [`Engine::edit`] for edits that remove content: the next save rewrites the file.
    pub(crate) fn remove<T: Send + 'static>(
        &self,
        doc: DocId,
        name: &'static str,
        f: impl FnOnce(&Document) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        self.job(doc, name, true, move |d, _| f(d))
    }

    /// Runs `f` on the engine thread, for work that leaves the document as it is.
    pub(crate) fn read<T: Send + 'static>(
        &self,
        doc: DocId,
        f: impl FnOnce(&Document, Option<&str>) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        self.job(doc, "", false, f)
    }

    fn job<T: Send + 'static>(
        &self,
        doc: DocId,
        name: &'static str,
        rewrite: bool,
        f: impl FnOnce(&Document, Option<&str>) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        let job: Job =
            Box::new(move |d, password| f(d, password).map(|v| Box::new(v) as Box<dyn Any + Send>));
        self.call(|reply| Command::Edit {
            doc,
            name,
            rewrite,
            job,
            reply,
        })
        .map(|v| *v.downcast::<T>().expect("a job replies with its own type"))
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
    // Documents whose next save must rewrite the file; see Command::Edit.
    let mut rewrite_next = std::collections::HashSet::new();
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
                    Ok(attachments::list(d)?
                        .into_iter()
                        .map(|f| Attachment {
                            size: f.size(),
                            page: f.page(),
                            name: f.name,
                        })
                        .collect())
                });
                let _ = reply.send(result);
            }
            Command::AttachmentData { doc, index, reply } => {
                let result = with_doc(&docs, doc, |d| attachments::data(d, index));
                let _ = reply.send(result);
            }
            Command::AddAttachment {
                doc,
                name,
                data,
                reply,
            } => {
                let result = with_doc(&docs, doc, |d| attachments::add(d, &name, &data));
                let _ = reply.send(result);
            }
            Command::DeleteAttachment { doc, index, reply } => {
                let result = with_doc(&docs, doc, |d| attachments::delete(d, index));
                if let Ok(Some(page)) = result {
                    lists.remove_page(doc, page);
                }
                let _ = reply.send(result.map(|_| ()));
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
            Command::Restyle {
                doc,
                page,
                id,
                change,
                reply,
            } => {
                let result = with_doc(&docs, doc, |d| annots::restyle(d, page, id, change));
                lists.remove_page(doc, page);
                let _ = reply.send(result);
            }
            Command::SetProperties {
                doc,
                page,
                id,
                props,
                reply,
            } => {
                let result = with_doc(&docs, doc, |d| annots::set_properties(d, page, id, &props));
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
            Command::MarkPreview {
                mark,
                height,
                reply,
            } => {
                let _ = reply.send(marks::preview(&mark, height));
            }
            Command::StampPreview {
                name,
                color,
                height,
                reply,
            } => {
                let _ = reply.send(annots::stamp_preview(&name, color, height));
            }
            Command::SummarizeComments {
                doc,
                file,
                target,
                reply,
            } => {
                let result = with_doc(&docs, doc, |d| summary::write(d, &file, &target));
                let _ = reply.send(result);
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
            Command::HasFields { doc, reply } => {
                let _ = reply.send(with_doc(&docs, doc, forms::has_fields));
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
            Command::ExportFdf { doc, file, reply } => {
                let _ = reply.send(with_doc(&docs, doc, |d| forms::export_fdf(d, &file)));
            }
            Command::ImportFdf { doc, data, reply } => {
                let result = with_doc(&docs, doc, |d| forms::import_fdf(d, &data));
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
                security,
                reply,
            } => {
                let result = (|| {
                    let original = paths.get(&doc).ok_or(Error::UnknownDocument)?.clone();
                    let d = docs.get(&doc).ok_or(Error::UnknownDocument)?;
                    let same = annots::same_file(&original, &target);
                    let incremental = incremental && !rewrite_next.contains(&doc);
                    if !same || (incremental && annots::can_append(d)) {
                        secure::save(d, &original, &target, incremental, &security)?;
                        return Ok(false);
                    }
                    // MuPDF reads the open file as it goes, so a full rewrite cannot go over
                    // it. Write a sibling, close the document, swap the files, open again.
                    let temp = annots::sibling(&original);
                    if let Err(e) = secure::save(d, &original, &temp, false, &security) {
                        let _ = std::fs::remove_file(&temp);
                        return Err(e);
                    }
                    docs.remove(&doc);
                    lists.remove_doc(doc);
                    match &security {
                        Security::Keep => {}
                        Security::Remove => {
                            passwords.remove(&doc);
                        }
                        Security::Protect(p) => {
                            passwords.insert(doc, p.password().to_owned());
                        }
                    }
                    rewrite_next.remove(&doc);
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
            Command::Edit {
                doc,
                name,
                rewrite,
                job,
                reply,
            } => {
                let password = passwords.get(&doc).map(String::as_str);
                let result = with_doc(&docs, doc, |d| {
                    if name.is_empty() {
                        job(d, password)
                    } else {
                        annots::operation(d, name, || job(d, password))
                    }
                });
                if !name.is_empty() {
                    lists.remove_doc(doc);
                }
                if rewrite && result.is_ok() {
                    rewrite_next.insert(doc);
                }
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
                rewrite_next.remove(&doc);
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
            source: Some(out.len()),
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
