//! `--bench-scroll <out.json>`: scrolls from top to bottom at a fixed step per timer tick, waits
//! for memory to settle, writes timings and exits. Memory itself is sampled from outside by
//! `bench/measure.ps1`, the same way Acrobat is measured.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use slint::{Timer, TimerMode};

use crate::{MainWindow, viewer};

/// Fraction of the view height scrolled per tick; half a screen keeps 1000-page runs short.
const STEP: f32 = 0.5;
const SETTLE: Duration = Duration::from_secs(3);

pub fn start(_window: &MainWindow, out: PathBuf) {
    let timer = Timer::default();
    let started = Instant::now();
    let mut ticks = 0u64;
    let mut y = 0.0f32;
    let mut done = false;

    // The timer must outlive this function; it is stopped by the event loop quitting.
    let timer = Box::leak(Box::new(timer));
    timer.start(TimerMode::Repeated, Duration::from_millis(1), move || {
        if done {
            return;
        }
        let Some((has_document, height, (_, view_height))) =
            viewer::with(|app| (app.has_document(), app.document_height(), app.view_size()))
        else {
            return;
        };
        if !has_document {
            // No document: measure the empty window.
            done = true;
            let out = out.clone();
            Timer::single_shot(SETTLE, move || {
                let _ = std::fs::write(&out, "{\"pages\": 0}\n");
                let _ = slint::quit_event_loop();
            });
            return;
        }
        let bottom = height - view_height;
        if bottom <= 0.0 {
            return; // layout not ready yet
        }
        ticks += 1;
        y = (y + STEP * view_height).min(bottom);
        viewer::with(|app| app.scroll_to_y(y));
        if y < bottom {
            return;
        }

        let scroll = started.elapsed();
        let (pages, renders) =
            viewer::with(|app| (app.page_count(), app.renders())).unwrap_or_default();
        let report = format!(
            "{{\"backend\": \"{}\", \"pages\": {pages}, \"ticks\": {ticks}, \"scroll_seconds\": {:.3}, \"ticks_per_second\": {:.1}, \"tiles_rendered\": {renders}}}\n",
            std::env::var("SLINT_BACKEND").unwrap_or_default(),
            scroll.as_secs_f64(),
            ticks as f64 / scroll.as_secs_f64(),
        );
        let out = out.clone();
        done = true;
        Timer::single_shot(SETTLE, move || {
            if let Err(e) = std::fs::write(&out, &report) {
                eprintln!("could not write {}: {e}", out.display());
            }
            let _ = slint::quit_event_loop();
        });
    });
}
