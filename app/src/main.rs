#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bench;
mod commands;
mod instance;
mod layout;
mod palette;
mod print;
mod recolor;
mod settings;
mod viewer;

use std::path::PathBuf;

use slint::ComponentHandle;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};

use settings::Settings;

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    let mut files = Vec::new();
    let mut bench_out = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--bench-scroll" {
            bench_out = args.next().map(PathBuf::from);
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
    // Settings are saved; skip tearing down render workers and MuPDF one by one.
    std::process::exit(0)
}

fn wire(window: &MainWindow) {
    window.on_view_changed(|| {
        viewer::with(viewer::App::update_view);
    });
    window.on_thumbs_scrolled(|| {
        viewer::with(viewer::App::update_thumbs);
    });
    window.on_key_input(|text, ctrl, shift, alt| commands::key_input(&text, ctrl, shift, alt));
    window.on_pointer_down(|x, y, button, shift| {
        viewer::with(|app| app.pointer_down(x, y, button, shift));
    });
    window.on_pointer_move(|x, y| {
        viewer::with(|app| app.pointer_move(x, y));
    });
    window.on_pointer_up(|x, y| {
        viewer::with(|app| app.pointer_up(x, y));
    });
    window.on_pointer_double(|x, y| {
        viewer::with(|app| app.pointer_double(x, y));
    });
    window.on_hover(|x, y| {
        viewer::with(|app| app.hover(x, y));
    });
    window.on_wheel_zoom(|delta, x, y| {
        viewer::with(|app| app.wheel_zoom(delta, x, y));
    });
    window.on_command(|id| commands::run(&id));
    window.on_select_tab(|i| {
        viewer::with(|app| app.select(i as usize));
    });
    window.on_close_tab(|i| {
        viewer::with(|app| app.close_tab(i as usize));
    });
    window.on_thumb_clicked(|page| {
        viewer::with(|app| app.go_to(page as usize, None, true));
    });
    window.on_outline_clicked(|row| {
        viewer::with(|app| app.outline_clicked(row as usize));
    });
    window.on_outline_toggle(|row| {
        viewer::with(|app| app.outline_toggle(row as usize));
    });
    window.on_page_entered(|text| {
        viewer::with(|app| app.page_entered(&text));
    });
    window.on_find_edited(|text| {
        viewer::with(|app| app.find_edited(text.to_string()));
    });
    window.on_find_next(|| {
        viewer::with(|app| app.find_step(true));
    });
    window.on_find_prev(|| {
        viewer::with(|app| app.find_step(false));
    });
    window.on_palette_edited(|text| {
        viewer::with(|app| app.palette_edited(&text));
    });
    window.on_palette_accept(|i| commands::palette_accept(i.max(0) as usize));
    window.on_dialog_accept(|input| {
        viewer::with(|app| app.dialog_accept(input.to_string()));
    });
    window.on_dialog_cancel(|| {
        viewer::with(viewer::App::dialog_cancel);
    });
    window.on_recent_clicked(|i| {
        viewer::with(|app| app.open_recent(i as usize));
    });
    window.on_attachment_save(|i| {
        viewer::with(|app| app.attachment_save(i as usize));
    });
    window.on_layer_toggle(|i| {
        viewer::with(|app| app.layer_toggle(i as usize));
    });
}
