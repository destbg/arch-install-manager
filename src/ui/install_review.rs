use std::cell::Cell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{
    Align, ApplicationWindow, Box as GtkBox, Button, Label, Orientation, PolicyType,
    ScrolledWindow, Separator, Window,
};

use crate::helpers::aur_pkgbuild::prepare_pkgbuild_review;
use crate::helpers::settings::load_settings;
use crate::log_info;
use crate::models::pkgbuild_review::PkgbuildReview;
use crate::models::review_file::ReviewFile;
use crate::ui::dialogs::create_progress_dialog;
use crate::ui::pkgbuild_review_dialog::build_review_view;

pub fn review_then_install(
    parent: &ApplicationWindow,
    aur_names: Vec<String>,
    on_proceed: impl Fn() + 'static,
) {
    if aur_names.is_empty() {
        on_proceed();
        return;
    }

    let always = load_settings().always_show_pkgbuild;
    let progress = create_progress_dialog(
        parent.upcast_ref::<Window>(),
        "Review",
        "Preparing package files for review...",
    );
    let parent = parent.clone();

    glib::spawn_future_local(async move {
        let mut reviews: Vec<PkgbuildReview> = Vec::new();
        for name in aur_names {
            let task_name = name.clone();
            let loaded = gio::spawn_blocking(move || prepare_pkgbuild_review(&task_name)).await;
            match loaded {
                Ok(Ok(review)) => reviews.push(review),
                _ => reviews.push(PkgbuildReview {
                    package: name,
                    diff: None,
                    needs_review: true,
                    files: vec![ReviewFile {
                        name: "PKGBUILD".to_string(),
                        content: "Could not load the package files for review.".to_string(),
                    }],
                }),
            }
        }

        let to_show: Vec<PkgbuildReview> = reviews
            .into_iter()
            .filter(|review| should_show(review, always))
            .collect();

        progress.close();

        if to_show.is_empty() {
            on_proceed();
            return;
        }

        show_review_window(&parent, to_show, on_proceed);
    });
}

fn should_show(review: &PkgbuildReview, always: bool) -> bool {
    if always {
        return true;
    }
    if review.diff.is_some() {
        return review.needs_review;
    }
    return true;
}

fn show_review_window(
    parent: &ApplicationWindow,
    reviews: Vec<PkgbuildReview>,
    on_proceed: impl Fn() + 'static,
) {
    let window = Window::builder()
        .title("Review packages before installing")
        .transient_for(parent)
        .modal(true)
        .default_width(820)
        .default_height(640)
        .build();

    let root = GtkBox::new(Orientation::Vertical, 0);

    let content = GtkBox::new(Orientation::Vertical, 0);
    content.set_vexpand(true);
    content.set_hexpand(true);
    let scroll = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never)
        .vscrollbar_policy(PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .child(&content)
        .build();
    root.append(&scroll);

    root.append(&Separator::new(Orientation::Horizontal));

    let button_row = GtkBox::new(Orientation::Horizontal, 8);
    button_row.set_halign(Align::End);
    button_row.set_margin_start(8);
    button_row.set_margin_end(8);
    button_row.set_margin_top(8);
    button_row.set_margin_bottom(8);

    let cancel = Button::with_label("Cancel");
    let window_for_cancel = window.clone();
    cancel.connect_clicked(move |_| {
        log_info!("install review cancelled");
        window_for_cancel.close();
    });
    button_row.append(&cancel);

    let primary = Button::with_label("Next");
    primary.add_css_class("suggested-action");
    button_row.append(&primary);

    root.append(&button_row);
    window.set_child(Some(&root));

    let reviews = Rc::new(reviews);
    let on_proceed = Rc::new(on_proceed);
    let index = Rc::new(Cell::new(0usize));

    let render: Rc<dyn Fn()> = {
        let content = content.clone();
        let reviews = reviews.clone();
        let primary = primary.clone();
        let index = index.clone();
        Rc::new(move || {
            let current = index.get();
            let total = reviews.len();
            while let Some(child) = content.first_child() {
                content.remove(&child);
            }

            let review = &reviews[current];
            let header = Label::new(None);
            header.set_xalign(0.0);
            header.set_margin_start(12);
            header.set_margin_end(12);
            header.set_margin_top(10);
            header.set_margin_bottom(4);
            header.add_css_class("title-4");
            header.set_markup(&format!(
                "<b>{}</b>   <span size=\"small\">({} of {})</span>",
                glib::markup_escape_text(&review.package),
                current + 1,
                total
            ));
            content.append(&header);
            content.append(&build_review_view(review));

            primary.set_label(if current + 1 >= total {
                "Install"
            } else {
                "Next"
            });
        })
    };

    let render_for_click = render.clone();
    let reviews_for_click = reviews.clone();
    let index_for_click = index.clone();
    let window_for_click = window.clone();
    let on_proceed_for_click = on_proceed.clone();
    primary.connect_clicked(move |_| {
        let current = index_for_click.get();
        if current + 1 >= reviews_for_click.len() {
            log_info!("install review accepted");
            window_for_click.close();
            on_proceed_for_click();
        } else {
            index_for_click.set(current + 1);
            render_for_click();
        }
    });

    render();
    window.present();
}
