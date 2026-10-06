use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};

use mupdf::{DisplayList, Document};

use crate::Error;

/// Display lists kept per engine. Re-rendering a page at a new zoom reuses its list.
const LIST_CACHE_CAPACITY: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DocId(u32);

#[derive(Debug, Clone, Copy)]
pub struct DocInfo {
    pub id: DocId,
    pub page_count: usize,
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
    PageSizes {
        doc: DocId,
        reply: Reply<Vec<PageSize>>,
    },
    DisplayList {
        doc: DocId,
        page: usize,
        reply: Reply<Arc<DisplayList>>,
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

    pub fn page_sizes(&self, doc: DocId) -> Result<Vec<PageSize>, Error> {
        self.call(|reply| Command::PageSizes { doc, reply })
    }

    /// Display list for one page, annotations included.
    pub fn display_list(&self, doc: DocId, page: usize) -> Result<Arc<DisplayList>, Error> {
        self.call(|reply| Command::DisplayList { doc, page, reply })
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
                    let page_count = doc.page_count()? as usize;
                    next_id += 1;
                    let id = DocId(next_id);
                    docs.insert(id, doc);
                    Ok(DocInfo { id, page_count })
                })();
                let _ = reply.send(result);
            }
            Command::PageSizes { doc, reply } => {
                let result = match docs.get(&doc) {
                    None => Err(Error::UnknownDocument),
                    Some(d) => (|| {
                        (0..d.page_count()?)
                            .map(|i| {
                                let b = d.load_page(i)?.bounds()?;
                                Ok(PageSize {
                                    width: b.x1 - b.x0,
                                    height: b.y1 - b.y0,
                                })
                            })
                            .collect()
                    })(),
                };
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
            Command::Close { doc } => {
                docs.remove(&doc);
                lists.remove_doc(doc);
            }
        }
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
