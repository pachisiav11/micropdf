//! The micropdf desktop app as a library, so `main.rs` stays thin and UI tests can drive the
//! real window on Slint's testing backend.

pub mod assistant;
pub mod assoc;
pub mod batch;
pub mod bench;
pub mod commands;
pub mod content;
pub mod convert;
pub mod install;
pub mod instance;
pub mod layout;
pub mod library;
pub mod measure;
pub mod palette;
pub mod prepare;
pub mod print;
pub mod recolor;
pub mod settings;
pub mod signing;
pub mod split;
pub mod tools;
pub mod update;
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
    window.on_pointer_left(|| {
        viewer::with(|app| app.pointer_left());
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
    window.on_thumb_press(|page, ctrl, shift, right| {
        viewer::with(|app| app.thumb_press(page as usize, ctrl, shift, right));
    });
    window.on_thumb_drop(|page, dy| {
        viewer::with(|app| app.thumb_drop(page as usize, dy));
    });
    window.on_form_edited(|i, text| {
        viewer::with(|app| app.form_change(i as usize, |f| f.text = text));
    });
    window.on_form_checked(|i, on| {
        viewer::with(|app| app.form_change(i as usize, |f| f.checked = on));
    });
    window.on_form_chosen(|i, index| {
        viewer::with(|app| app.form_change(i as usize, |f| f.index = index));
    });
    window.on_outline_clicked(|row| {
        viewer::with(|app| app.outline_clicked(row as usize));
    });
    window.on_outline_toggle(|row| {
        viewer::with(|app| app.outline_toggle(row as usize));
    });
    window.on_outline_menu(|row| {
        viewer::with(|app| content::outline_menu(app, row as usize));
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
    window.on_signature_go(|i| {
        viewer::with(|app| app.signature_go(i as usize));
    });
    window.on_split_scrolled(|| {
        viewer::with(split::scrolled);
    });
    window.on_attachment_delete(|i| {
        viewer::with(|app| app.attachment_delete(i as usize));
    });
    window.on_style_color_picked(|i| {
        viewer::with(|app| app.style_color(i as usize));
    });
    window.on_style_fill_toggled(|| {
        viewer::with(|app| app.style_fill());
    });
    window.on_style_width_picked(|w| {
        viewer::with(|app| app.style_width(w));
    });
    window.on_style_opacity_picked(|o| {
        viewer::with(|app| app.style_opacity(o));
    });
    window.on_style_border_picked(|b| {
        viewer::with(|app| app.style_border(b));
    });
    window.on_style_end_picked(|start, name| {
        viewer::with(|app| app.style_line_end(start, &name));
    });
    window.on_style_font_size_picked(|size| {
        viewer::with(|app| app.style_font_size(size));
    });
    window.on_style_properties(|| {
        viewer::with(|app| app.style_properties());
    });
    window.on_field_commit(|step| {
        viewer::with(|app| app.field_commit(step));
    });
    window.on_field_cancel(|| {
        viewer::with(|app| app.field_cancel());
    });
    window.on_field_choose(|i| {
        viewer::with(|app| app.field_choose(i as usize));
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
    assistant::wire(window);
}
