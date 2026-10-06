#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bench;
mod viewer;

use std::path::PathBuf;

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    let mut file = None;
    let mut bench_out = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--bench-scroll" {
            bench_out = args.next().map(PathBuf::from);
        } else {
            file = Some(PathBuf::from(arg));
        }
    }

    let window = MainWindow::new()?;
    match &file {
        Some(path) => {
            if let Err(e) = viewer::open(&window, path) {
                window.set_status_text(format!("Could not open {}: {e}", path.display()).into());
            }
        }
        None => window.set_status_text("Pass a PDF to open: micropdf <file.pdf>".into()),
    }
    if let Some(out) = bench_out {
        bench::start(&window, out);
    }
    window.run()
}
