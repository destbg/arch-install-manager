use gtk4::prelude::*;
use gtk4::{Align, ApplicationWindow, Box as GtkBox, Button, Frame, Label, Orientation};
use std::rc::Rc;
use vte4::prelude::*;
use vte4::{Format, Terminal};

use crate::helpers::get_navigation_stack::get_navigation_stack;
use crate::helpers::settings::load_settings;
use crate::helpers::terminal::spawn_terminal;
use crate::helpers::tray_integration::trigger_check_service;
use crate::log_info;
use crate::ui::main_window::{POST_UPDATE_PAGE, load_packages};
use crate::ui::post_update_page::{
    create_post_update_page, reset_post_update_page, run_post_update_detections,
};

pub fn run_command_in_dialog<F>(
    window: &ApplicationWindow,
    command: &str,
    offer_checks: bool,
    on_finished: F,
) where
    F: Fn() + 'static,
{
    let dialog = gtk4::Window::builder()
        .title("Running command")
        .transient_for(window)
        .modal(true)
        .default_width(860)
        .default_height(600)
        .build();

    let content_area = GtkBox::new(Orientation::Vertical, 0);
    content_area.set_vexpand(true);
    dialog.set_child(Some(&content_area));

    let header = GtkBox::new(Orientation::Vertical, 4);
    header.set_margin_start(12);
    header.set_margin_end(12);
    header.set_margin_top(12);
    header.set_margin_bottom(8);

    let title_label = Label::new(Some("Running..."));
    title_label.add_css_class("title-3");
    title_label.set_xalign(0.0);
    header.append(&title_label);

    let subtitle_label = Label::new(Some(
        "Follow the terminal below. You can close this once it finishes.",
    ));
    subtitle_label.add_css_class("dim-label");
    subtitle_label.set_xalign(0.0);
    subtitle_label.set_wrap(true);
    header.append(&subtitle_label);

    content_area.append(&header);

    let terminal = Terminal::new();
    terminal.set_vexpand(true);
    terminal.set_hexpand(true);
    terminal.set_scrollback_lines(1000);
    terminal.set_scroll_on_output(true);
    terminal.set_scroll_on_keystroke(true);
    terminal.set_audible_bell(false);

    let frame = Frame::new(Some("Terminal"));
    frame.set_child(Some(&terminal));
    frame.set_margin_start(12);
    frame.set_margin_end(12);
    frame.set_vexpand(true);
    content_area.append(&frame);

    let button_row = GtkBox::new(Orientation::Horizontal, 8);
    button_row.set_halign(Align::End);
    button_row.set_margin_start(12);
    button_row.set_margin_end(12);
    button_row.set_margin_top(8);
    button_row.set_margin_bottom(12);

    let close_btn = Button::with_label("Cancel");
    close_btn.add_css_class("suggested-action");
    button_row.append(&close_btn);

    let checks_btn = Button::with_label("Post-update checks");
    checks_btn.set_visible(false);
    button_row.append(&checks_btn);

    content_area.append(&button_row);

    let dialog_for_btn = dialog.clone();
    close_btn.connect_clicked(move |_| dialog_for_btn.close());

    let on_finished_rc: Rc<dyn Fn()> = Rc::new(on_finished);
    dialog.connect_close_request(move |_| {
        on_finished_rc();
        return glib::Propagation::Proceed;
    });

    let window_for_checks = window.clone();
    let dialog_for_checks = dialog.clone();
    let content_for_checks = content_area.clone();
    let header_for_checks = header.clone();
    let frame_for_checks = frame.clone();
    let button_row_for_checks = button_row.clone();
    checks_btn.connect_clicked(move |_| {
        let page = create_post_update_page();
        POST_UPDATE_PAGE.with(|cell| {
            *cell.borrow_mut() = Some(page.clone());
        });

        page.back_button.set_label("Close");
        let dialog_for_back = dialog_for_checks.clone();
        page.back_button
            .connect_clicked(move |_| dialog_for_back.close());

        content_for_checks.remove(&header_for_checks);
        content_for_checks.remove(&frame_for_checks);
        content_for_checks.remove(&button_row_for_checks);
        page.container.set_vexpand(true);
        content_for_checks.append(&page.container);

        reset_post_update_page(&page);
        run_post_update_detections(window_for_checks.clone());
    });

    let title_for_exit = title_label.clone();
    let close_for_exit = close_btn.clone();
    let checks_for_exit = checks_btn.clone();
    terminal.connect_child_exited(move |term, exit_status| {
        log_info!("dialog terminal command exited: status={}", exit_status);
        capture_terminal_output(term, "dialog");

        if exit_status != 0 {
            title_for_exit.set_text(&format!("Failed (exit {})", exit_status));
            close_for_exit.set_label("Close");
            return;
        }

        title_for_exit.set_text("Done");
        close_for_exit.set_label("Close");
        trigger_check_service();

        if offer_checks && load_settings().run_post_update_checks {
            checks_for_exit.set_visible(true);
        }
    });

    log_info!("spawning dialog terminal command: {}", command);
    dialog.present();
    spawn_terminal(&terminal, vec!["bash", "-lc", command]);
}

pub fn run_update_install_dialog(window: &ApplicationWindow, command: &str) {
    let window_for_finish = window.clone();
    run_command_in_dialog(window, command, true, move || {
        refresh_update_list(&window_for_finish);
    });
}

fn refresh_update_list(window: &ApplicationWindow) {
    if let Some(main_box) = window.child().and_downcast::<GtkBox>() {
        refresh_package_list(&main_box);
    }
}

fn capture_terminal_output(terminal: &vte4::Terminal, label: &str) {
    let Some(text) = terminal.text_format(Format::Text) else {
        return;
    };
    let text_str: String = text.to_string();
    let trimmed = text_str.trim_end();
    if trimmed.is_empty() {
        return;
    }
    log_info!("terminal output ({}):\n{}", label, trimmed);
}

fn refresh_package_list(main_box: &GtkBox) {
    let Some((content_box, window)) = get_navigation_stack(main_box) else {
        return;
    };

    load_packages(content_box, window);
}
