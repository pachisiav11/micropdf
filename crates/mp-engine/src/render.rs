use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use mupdf::{Colorspace, Context, Device, DisplayList, IRect, Matrix, Pixmap};

use crate::Error;

/// Share of MuPDF's fixed 256 MB resource store kept after each render (about 30 MB).
const STORE_PERCENT: u32 = 12;

/// An RGB render, rows packed with no padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageImage {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// A rectangle of device pixels within a page rendered at some scale and rotation. The page's
/// rendered bounds always start at (0, 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tile {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
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

    /// Runs `job` on a worker. Jobs start in the order they were queued.
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

/// Renders a whole page at `scale` (1.0 = 72 dpi) on the calling thread.
pub fn render(list: &DisplayList, scale: f32) -> Result<PageImage, Error> {
    let pixmap = list.to_pixmap(
        &Matrix::new_scale(scale, scale),
        &Colorspace::device_rgb(),
        false,
    )?;
    shrink_store();
    Ok(page_image(&pixmap))
}

/// Size in device pixels of a page rendered at `scale` and `rotation` (degrees, multiple of 90).
pub fn rendered_size(list: &DisplayList, scale: f32, rotation: i32) -> (i32, i32) {
    let b = list.bounds().transform(&page_matrix(scale, rotation));
    ((b.x1 - b.x0).ceil() as i32, (b.y1 - b.y0).ceil() as i32)
}

/// Renders one tile of a page at `scale` and `rotation` on the calling thread.
pub fn render_tile(
    list: &DisplayList,
    scale: f32,
    rotation: i32,
    tile: Tile,
) -> Result<PageImage, Error> {
    let mut ctm = page_matrix(scale, rotation);
    let bounds = list.bounds().transform(&ctm);
    ctm.concat(Matrix::new_translate(-bounds.x0, -bounds.y0));

    let rect = IRect {
        x0: tile.x,
        y0: tile.y,
        x1: tile.x + tile.width,
        y1: tile.y + tile.height,
    };
    let mut pixmap = Pixmap::new_with_rect(&Colorspace::device_rgb(), rect, false)?;
    pixmap.clear_with(255)?;
    {
        // The device must be closed (dropped) before the pixels are read.
        let device = Device::from_pixmap(&pixmap)?;
        list.run(&device, &ctm, rect.into())?;
    }
    shrink_store();
    Ok(page_image(&pixmap))
}

fn page_matrix(scale: f32, rotation: i32) -> Matrix {
    let mut m = Matrix::new_scale(scale, scale);
    m.concat(Matrix::new_rotate(rotation.rem_euclid(360) as f32));
    m
}

fn shrink_store() {
    Context::get().shrink_store(STORE_PERCENT);
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
