use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};

use mupdf::link::LinkDestination;
use mupdf::{DestinationKind, DisplayList, Document, MetadataName, Outline};

use crate::{Error, Link, LinkTarget, OutlineItem};

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
    let mut lists = ListCache::default();
    let mut next_id = 0;

    for command in rx {
        match command {
            Command::Open { path, reply } => {
                let result = (|| {
                    // On Windows mupdf only accepts UTF-8 string paths.
                    let path = path.to_str().ok_or(mupdf::Error::InvalidUtf8)?;
                    let doc = Document::open(path)?;
                    let needs_password = doc.needs_password()?;
                    let page_count = if needs_password {
                        0
                    } else {
                        doc.page_count()? as usize
                    };
                    next_id += 1;
                    let id = DocId(next_id);
                    docs.insert(id, doc);
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
                        let list = Arc::new(d.load_page(page as i32)?.to_display_list(true)?);
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
            Command::Close { doc } => {
                docs.remove(&doc);
                lists.remove_doc(doc);
            }
        }
    }
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
