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

struct Job {
    list: Arc<DisplayList>,
    scale: f32,
    reply: Sender<Result<PageImage, Error>>,
}

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

    /// Queues a whole-page render at `scale` (1.0 = 72 dpi). The result arrives on the receiver.
    pub fn render(&self, list: Arc<DisplayList>, scale: f32) -> Receiver<Result<PageImage, Error>> {
        let (reply, rx) = mpsc::channel();
        let job = Job { list, scale, reply };
        if let Err(mpsc::SendError(job)) = self.tx.as_ref().expect("taken only in drop").send(job) {
            let _ = job.reply.send(Err(Error::Stopped));
        }
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

fn work(rx: &Mutex<Receiver<Job>>) {
    loop {
        // Hold the lock only while taking a job, never while rendering.
        let job = match rx.lock() {
            Ok(rx) => rx.recv(),
            Err(_) => return,
        };
        let Ok(job) = job else { return };
        let result = job
            .list
            .to_pixmap(
                &Matrix::new_scale(job.scale, job.scale),
                &Colorspace::device_rgb(),
                false,
            )
            .map(|pixmap| page_image(&pixmap))
            .map_err(Error::from);
        let _ = job.reply.send(result);
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
