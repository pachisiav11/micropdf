//! The micropdf desktop app as a library, so `main.rs` stays thin and UI tests can drive the
//! real window on Slint's testing backend.

pub mod assoc;
pub mod bench;
pub mod commands;
pub mod instance;
pub mod layout;
pub mod palette;
pub mod print;
pub mod recolor;
pub mod settings;
pub mod viewer;

slint::include_modules!();

/// Connects every UI callback to the controller.
pub fn wire(window: &MainWindow) {
    window.window().on_close_requested(|| {
        if viewer::with(viewer::App::confirm_quit).unwrap_or(true) {
            slint::CloseRequestResponse::HideWindow
        } else {
            slint::CloseRequestResponse::KeepWindowShown
        }
    });
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
    window.on_comment_clicked(|i| {
        viewer::with(|app| app.comment_clicked(i as usize));
    });
    window.on_comment_edit(|i| {
        viewer::with(|app| app.comment_edit(i as usize));
    });
    window.on_comment_delete(|i| {
        viewer::with(|app| app.comment_delete(i as usize));
    });
    window.on_comment_reply(|i| {
        viewer::with(|app| app.comment_reply(i as usize));
    });
    window.on_comment_status(|i, state| {
        viewer::with(|app| app.comment_status(i as usize, &state));
    });
    window.on_comment_filter_edited(|text| {
        viewer::with(|app| app.comment_filter_edited(text.into()));
    });
    window.on_sign_pad_down(|x, y| {
        viewer::with(|app| app.pad_down(x, y));
    });
    window.on_sign_pad_move(|x, y| {
        viewer::with(|app| app.pad_move(x, y));
    });
}
