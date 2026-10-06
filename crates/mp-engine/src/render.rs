use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use mupdf::{Colorspace, DisplayList, Matrix, Pixmap};

use crate::Error;

/// An RGB render, rows packed with no padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageImage {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

type Job = Box<dyn FnOnce() + Send>;

/// Worker threads for rendering. Each worker gets its own cloned MuPDF context the first time
/// it touches MuPDF.
pub struct RenderPool {
    tx: Option<Sender<Job>>,
    workers: Vec<JoinHandle<()>>,
}

impl RenderPool {
    pub fn new(workers: usize) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        let workers = (0..workers.max(1))
            .map(|i| {
                let rx = Arc::clone(&rx);
                thread::Builder::new()
                    .name(format!("mp-render-{i}"))
                    .spawn(move || work(&rx))
                    .expect("failed to spawn render worker")
            })
            .collect();
        RenderPool {
            tx: Some(tx),
            workers,
        }
    }

    /// Runs `job` on a worker. Jobs run in the order they were queued.
    pub fn spawn(&self, job: impl FnOnce() + Send + 'static) {
        let _ = self
            .tx
            .as_ref()
            .expect("taken only in drop")
            .send(Box::new(job));
    }

    /// Queues a whole-page render at `scale` (1.0 = 72 dpi). The result arrives on the receiver.
    pub fn render(&self, list: Arc<DisplayList>, scale: f32) -> Receiver<Result<PageImage, Error>> {
        let (reply, rx) = mpsc::channel();
        self.spawn(move || {
            let _ = reply.send(render(&list, scale));
        });
        rx
    }
}

impl Drop for RenderPool {
    fn drop(&mut self) {
        drop(self.tx.take());
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

/// Renders a display list at `scale` (1.0 = 72 dpi) on the calling thread.
pub fn render(list: &DisplayList, scale: f32) -> Result<PageImage, Error> {
    let pixmap = list.to_pixmap(
        &Matrix::new_scale(scale, scale),
        &Colorspace::device_rgb(),
        false,
    )?;
    Ok(page_image(&pixmap))
}

fn work(rx: &Mutex<Receiver<Job>>) {
    loop {
        // Hold the lock only while taking a job, never while running it.
        let job = match rx.lock() {
            Ok(rx) => rx.recv(),
            Err(_) => return,
        };
        let Ok(job) = job else { return };
        job();
    }
}

fn page_image(pixmap: &Pixmap) -> PageImage {
    let (width, height) = (pixmap.width(), pixmap.height());
    let row = width as usize * pixmap.n() as usize;
    let stride = pixmap.stride() as usize;
    let rgb = pixmap
        .samples()
        .chunks(stride)
        .take(height as usize)
        .flat_map(|line| &line[..row])
        .copied()
        .collect();
    PageImage { width, height, rgb }
}
