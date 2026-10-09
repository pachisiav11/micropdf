#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use micropdf::settings::Settings;
use micropdf::{MainWindow, bench, install, instance, viewer, wire};
use slint::ComponentHandle;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};

fn main() -> Result<(), slint::PlatformError> {
    let mut files = Vec::new();
    let mut bench_out = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--bench-scroll" {
            bench_out = args.next().map(PathBuf::from);
        } else if arg == "--install" {
            if !install::install() {
                return Ok(());
            }
        } else if arg == "--uninstall" {
            install::uninstall();
            return Ok(());
        } else {
            files.push(PathBuf::from(arg));
        }
    }
    // Benchmarks run isolated: no hand-off, no session, no settings written.
    let bench = bench_out.is_some();
    if !bench && instance::hand_off(&files) {
        return Ok(());
    }

    let mut settings = if bench {
        Settings::default()
    } else {
        Settings::load()
    };
    let restore = if !settings.session.clean_exit && files.is_empty() {
        std::mem::take(&mut settings.session.files)
    } else {
        Vec::new()
    };
    settings.session.clean_exit = bench;

    let window = MainWindow::new()?;
    viewer::install(viewer::App::new(&window, settings, !bench));
    wire(&window);

    viewer::with(|app| {
        app.save_session();
        app.open_paths(files);
        app.offer_restore(restore);
    });
    if !bench {
        let weak = window.as_weak();
        instance::listen(move |paths| {
            let _ = weak.upgrade_in_event_loop(move |window| {
                viewer::with(|app| app.open_paths(paths));
                window.window().with_winit_window(|w| {
                    w.set_minimized(false);
                    w.focus_window();
                });
            });
        });
    }

    window.show()?;
    window.window().on_winit_window_event(|_, event| {
        if let winit::event::WindowEvent::DroppedFile(path) = event {
            let path = path.clone();
            let _ = slint::invoke_from_event_loop(move || {
                viewer::with(|app| app.open(path));
            });
        }
        EventResult::Propagate
    });
    if let Some(out) = bench_out {
        bench::start(&window, out);
    }
    slint::run_event_loop()?;
    window.hide()?;
    viewer::with(viewer::App::shutdown);
    micropdf::update::after_exit();
    // Settings are saved; skip tearing down render workers and MuPDF one by one.
    std::process::exit(0)
}
