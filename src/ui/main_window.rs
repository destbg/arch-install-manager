use crate::helpers::arch_news::news_to_show;
use crate::helpers::decorations::are_decorations_disabled;
use crate::helpers::elevated::open_url_as_user;
use crate::helpers::mirrors::{is_mirrorlist_stale, mirror_refresh_command, mirrorlist_age_days};
use crate::helpers::package_updates::get_package_updates;
use crate::helpers::pacman_ignore::{is_in_managed_ignore_pkg, list_managed_ignores};
use crate::helpers::release_notes::release_notes_url;
use crate::helpers::settings::{load_settings, save_settings};
use crate::helpers::tray_integration::trigger_check_service;
use crate::helpers::unselected_packages::load_unselected_packages;
use crate::log_info;
use crate::models::info_panel::InfoPanel;
use crate::models::package_object::PackageUpdateObject;
use crate::models::package_source::PackageSource;
use crate::models::package_update::PackageUpdate;
use crate::models::post_update_page::PostUpdatePage;
use crate::models::update_error::UpdateError;
use crate::ui::dialogs::{show_confirm_dialog, show_error_dialog};
use crate::ui::error_page::{create_error_page, update_error_page_message};
use crate::ui::history_dialog::show_history_dialog;
use crate::ui::info_panel::{create_info_panel, update_ignore_button_tooltip};
use crate::ui::loading::create_loading_page;
use crate::ui::news_dialog::show_news_dialog;
use crate::ui::no_updates::create_no_updates_page;
use crate::ui::package_list::{
    create_package_list, format_age, format_build_date, prefers_dark, update_statusbar,
};
use crate::ui::post_update_page::create_post_update_page;
use crate::ui::settings_dialog::show_settings_dialog;
use crate::ui::terminal_page::{create_terminal_page, run_command_in_dialog};
use crate::ui::toolbar::create_toolbar;
use crate::ui::vulnerabilities_dialog::show_vulnerabilities_dialog;
use gio::ListStore;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, ColumnView, ColumnViewColumn,
    FilterListModel, HeaderBar, Orientation, Paned, ScrolledWindow, SearchBar, SearchEntry,
    Separator, SingleSelection, SortListModel, Stack, StackSwitcher,
};
use std::cell::RefCell;

thread_local! {
    pub static POST_UPDATE_PAGE: RefCell<Option<PostUpdatePage>> = RefCell::new(None);
}

pub fn build_ui(app: &Application) {
    let window = ApplicationWindow::builder()
        .application(app)
        .title("Arch Install Manager")
        .icon_name("arch-install-manager")
        .default_width(960)
        .default_height(620)
        .build();

    let decorations_disabled = are_decorations_disabled();

    let header_bar = HeaderBar::new();

    if !decorations_disabled {
        let settings_button = Button::from_icon_name("preferences-system-symbolic");
        settings_button.set_tooltip_text(Some("Settings"));

        let window_clone = window.clone();
        settings_button.connect_clicked(move |_| {
            log_info!("header: Settings clicked");
            let settings = load_settings();
            let favorites_column = find_favorites_column(&window_clone);
            let package_store = find_package_store(&window_clone);
            show_settings_dialog(&window_clone, &settings, favorites_column, package_store);
        });

        header_bar.pack_end(&settings_button);

        let news_button = Button::from_icon_name("application-rss+xml-symbolic");
        news_button.set_tooltip_text(Some("Arch Linux News"));

        news_button.connect_clicked(|_| {
            log_info!("header: News clicked");
            open_url_as_user("https://archlinux.org/news/");
        });

        header_bar.pack_end(&news_button);

        let vulnerabilities_button = Button::from_icon_name("security-high-symbolic");
        vulnerabilities_button.set_tooltip_text(Some("Open vulnerabilities (no fix available)"));

        let window_for_vulnerabilities = window.clone();
        vulnerabilities_button.connect_clicked(move |_| {
            log_info!("header: Security clicked");
            show_vulnerabilities_dialog(&window_for_vulnerabilities);
        });

        header_bar.pack_end(&vulnerabilities_button);

        let history_button = Button::from_icon_name("document-open-recent-symbolic");
        history_button.set_tooltip_text(Some("Update history"));

        let window_for_history = window.clone();
        history_button.connect_clicked(move |_| {
            log_info!("header: History clicked");
            show_history_dialog(&window_for_history);
        });

        header_bar.pack_end(&history_button);
    }

    window.set_titlebar(Some(&header_bar));

    let main_box = GtkBox::new(Orientation::Vertical, 0);

    let stack = Stack::new();
    stack.set_vexpand(true);

    let loading_box = create_loading_page();
    stack.add_named(&loading_box, Some("loading"));

    let no_updates_box = create_no_updates_page();
    stack.add_named(&no_updates_box, Some("no-updates"));

    let error_box = create_error_page();
    stack.add_named(&error_box, Some("error"));

    let terminal_box = create_terminal_page();
    stack.add_named(&terminal_box, Some("terminal"));

    let post_update_page = create_post_update_page();
    stack.add_named(&post_update_page.container, Some("post-update"));
    wire_post_update_back_button(&post_update_page, &stack, &window);
    POST_UPDATE_PAGE.with(|cell| {
        *cell.borrow_mut() = Some(post_update_page);
    });

    let content_box = create_main_content(decorations_disabled, &stack, &window);
    stack.add_named(&content_box, Some("content"));

    if let Some(view_stack) = content_box.first_child().and_downcast::<Stack>() {
        install_tabs_css();
        let switcher = StackSwitcher::new();
        switcher.add_css_class("daim-tabs");
        switcher.set_stack(Some(&view_stack));
        header_bar.pack_start(&switcher);
    }

    main_box.append(&stack);

    window.set_child(Some(&main_box));

    stack.set_visible_child_name("loading");

    window.present();

    let stack_clone = stack.clone();
    let content_box_clone = content_box.clone();
    let window_clone2 = window.clone();
    glib::idle_add_local_once(move || {
        start_initial_load(stack_clone, content_box_clone, window_clone2);
    });
}

fn start_initial_load(stack: Stack, content_box: GtkBox, window: ApplicationWindow) {
    let check_news = load_settings().check_arch_news;

    load_packages(stack, content_box, window.clone());

    if !check_news {
        return;
    }

    glib::spawn_future_local(async move {
        let items = gio::spawn_blocking(news_to_show).await.unwrap_or_default();

        if !items.is_empty() {
            show_news_dialog(&window, &items);
        }
    });
}

fn build_mirror_banner(window: &ApplicationWindow) -> GtkBox {
    install_mirror_banner_css();

    let banner = GtkBox::new(Orientation::Horizontal, 12);
    banner.add_css_class("mirror-banner");
    banner.set_margin_start(12);
    banner.set_margin_end(12);
    banner.set_margin_top(8);

    let command = mirror_refresh_command();
    let enabled = load_settings().enable_mirror_refresh;
    banner.set_visible(enabled && is_mirrorlist_stale() && command.is_some());

    let icon = gtk4::Image::from_icon_name("network-server-symbolic");
    icon.set_pixel_size(20);
    icon.set_valign(gtk4::Align::Center);
    banner.append(&icon);

    let text_box = GtkBox::new(Orientation::Vertical, 2);
    text_box.set_hexpand(true);
    text_box.set_valign(gtk4::Align::Center);

    let title = gtk4::Label::new(Some("Your mirror list may be out of date"));
    title.add_css_class("heading");
    title.set_xalign(0.0);
    text_box.append(&title);

    let age = mirrorlist_age_days().unwrap_or(0);
    let body = gtk4::Label::new(Some(&format!(
        "It was last updated {} days ago. Refreshing it can make downloads faster.",
        age
    )));
    body.add_css_class("dim-label");
    body.add_css_class("caption");
    body.set_xalign(0.0);
    body.set_wrap(true);
    text_box.append(&body);

    banner.append(&text_box);

    let refresh_button = Button::with_label("Refresh mirrors");
    refresh_button.add_css_class("suggested-action");
    refresh_button.set_valign(gtk4::Align::Center);

    let banner_for_refresh = banner.clone();
    let window_for_refresh = window.clone();
    refresh_button.connect_clicked(move |_| {
        let Some(command) = mirror_refresh_command() else {
            return;
        };
        log_info!("mirror banner: Refresh mirrors clicked");
        let banner = banner_for_refresh.clone();
        run_command_in_dialog(
            window_for_refresh.upcast_ref::<gtk4::Window>(),
            &command,
            move || {
                banner.set_visible(false);
            },
        );
    });
    banner.append(&refresh_button);

    let close_button = Button::from_icon_name("window-close-symbolic");
    close_button.add_css_class("flat");
    close_button.set_valign(gtk4::Align::Center);
    close_button.set_tooltip_text(Some("Dismiss"));

    let banner_for_close = banner.clone();
    close_button.connect_clicked(move |_| {
        log_info!("mirror banner: dismissed");
        let mut settings = load_settings();
        settings.enable_mirror_refresh = false;
        let _ = save_settings(&settings);
        banner_for_close.set_visible(false);
    });
    banner.append(&close_button);

    return banner;
}

fn install_mirror_banner_css() {
    use std::sync::OnceLock;
    static CSS_INSTALLED: OnceLock<()> = OnceLock::new();

    CSS_INSTALLED.get_or_init(|| {
        let Some(display) = gtk4::gdk::Display::default() else {
            return;
        };

        let provider = gtk4::CssProvider::new();
        provider.load_from_data(
            ".mirror-banner {                background-color: alpha(currentColor, 0.05);                border: 1px solid alpha(currentColor, 0.1);                border-radius: 12px;                padding: 8px 12px;            }",
        );

        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });
}

fn install_tabs_css() {
    use std::sync::OnceLock;
    static CSS_INSTALLED: OnceLock<()> = OnceLock::new();

    CSS_INSTALLED.get_or_init(|| {
        let Some(display) = gtk4::gdk::Display::default() else {
            return;
        };

        let provider = gtk4::CssProvider::new();
        provider.load_from_data(
            ".daim-tabs { padding: 0; background: transparent; }\
             .daim-tabs > button {\
                 border: none;\
                 border-radius: 8px;\
                 box-shadow: none;\
                 outline: none;\
                 background: transparent;\
                 margin: 0 2px;\
                 padding: 4px 0px;\
                 min-height: 0;\
                 font-weight: normal;\
                 color: alpha(currentColor, 0.7);\
             }\
             .daim-tabs > button:hover {\
                 background: alpha(currentColor, 0.07);\
                 color: currentColor;\
             }\
             .daim-tabs > button:checked {\
                 background: alpha(currentColor, 0.13);\
                 color: currentColor;\
                 font-weight: bold;\
             }",
        );

        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });
}

fn build_update_tab(
    decorations_disabled: bool,
    stack: &Stack,
    window: &ApplicationWindow,
) -> GtkBox {
    let content_box = GtkBox::new(Orientation::Vertical, 0);

    let toolbar_container = create_toolbar(decorations_disabled);

    content_box.append(&toolbar_container);

    let separator = Separator::new(Orientation::Horizontal);
    content_box.append(&separator);

    let mirror_banner = build_mirror_banner(window);
    content_box.append(&mirror_banner);

    let paned = Paned::new(Orientation::Vertical);

    let search_entry = SearchEntry::new();
    search_entry.set_placeholder_text(Some("Filter packages by name or description"));
    search_entry.set_hexpand(true);

    let (list_view, store, statusbar, filter) = create_package_list(&search_entry);

    let search_bar = SearchBar::new();
    search_bar.set_child(Some(&search_entry));
    search_bar.connect_entry(&search_entry);
    search_bar.set_key_capture_widget(Some(&content_box));
    search_bar.set_show_close_button(true);

    {
        let filter = filter.clone();
        search_entry.connect_search_changed(move |_| {
            filter.changed(gtk4::FilterChange::Different);
        });
    }

    {
        let search_entry_clear = search_entry.clone();
        search_bar.connect_notify_local(Some("search-mode-enabled"), move |bar, _| {
            if !bar.is_search_mode() {
                search_entry_clear.set_text("");
            }
        });
    }

    content_box.append(&search_bar);

    let scrolled = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Automatic)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .vexpand(true)
        .child(&list_view)
        .build();

    paned.set_start_child(Some(&scrolled));

    let info_panel = create_info_panel();
    paned.set_end_child(Some(&info_panel.container));

    wire_ignore_button(&info_panel, stack, window);

    if let Some(selection_model) = list_view.model().and_downcast::<SingleSelection>() {
        let title_label = info_panel.title_label.clone();
        let created_label = info_panel.created_label.clone();
        let maintainer_label = info_panel.maintainer_label.clone();
        let permissions_label = info_panel.permissions_label.clone();
        let deps_label = info_panel.deps_label.clone();
        let info_container = info_panel.container.clone();
        let info_text = info_panel.info_text.clone();
        let url_button = info_panel.url_button.clone();
        let release_notes_button = info_panel.release_notes_button.clone();
        let pkgbuild_button = info_panel.pkgbuild_button.clone();
        let aur_scan_button = info_panel.aur_scan_button.clone();
        let ignore_button = info_panel.ignore_button.clone();
        let ignore_handler_id = info_panel.ignore_handler_id.clone();
        let current_url = info_panel.current_url.clone();
        let current_release_notes_url = info_panel.current_release_notes_url.clone();
        let current_package = info_panel.current_package.clone();
        selection_model.connect_selection_changed(move |model, _position, _n_items| {
            if let Some(package_obj) = model.selected_item().and_downcast::<PackageUpdateObject>() {
                let package_data = package_obj.data();
                info_container.set_visible(true);
                title_label.set_markup(&info_title_markup(&package_data));
                if package_data.source == PackageSource::Aur {
                    let mut parts: Vec<String> = Vec::new();
                    if let Some(ts) = package_data.first_submitted {
                        parts.push(format!("Created {}", format_age(ts)));
                    }
                    if let Some(votes) = package_data.num_votes {
                        parts.push(format!("{} votes", votes));
                    }
                    if let Some(popularity) = package_data.popularity {
                        parts.push(format!("popularity {:.2}", popularity));
                    }
                    let aur_url = format!(
                        "https://aur.archlinux.org/packages/{}",
                        glib::markup_escape_text(&package_data.name)
                    );
                    parts.push(format!("<a href=\"{}\">comments</a>", aur_url));
                    created_label.set_markup(&parts.join(" \u{00B7} "));
                    created_label.set_visible(true);
                } else {
                    created_label.set_visible(false);
                }
                if package_data.maintainer_changed() {
                    let previous = package_data
                        .previous_maintainer
                        .as_deref()
                        .unwrap_or("unknown");
                    let current = package_data.maintainer.as_deref().unwrap_or("unknown");
                    maintainer_label.set_markup(&format!(
                        "<span foreground=\"{}\">Maintainer changed from {} to {}</span>",
                        if prefers_dark() { "#ffa348" } else { "#e66100" },
                        glib::markup_escape_text(previous),
                        glib::markup_escape_text(current),
                    ));
                    maintainer_label.set_visible(true);
                } else {
                    maintainer_label.set_visible(false);
                }
                if package_data.new_permissions.is_empty() {
                    permissions_label.set_visible(false);
                } else {
                    let list = package_data.new_permissions.join(", ");
                    permissions_label.set_markup(&format!(
                        "<span foreground=\"{}\">Asks for new permissions: {}</span>",
                        if prefers_dark() { "#ffa348" } else { "#e66100" },
                        glib::markup_escape_text(&list),
                    ));
                    permissions_label.set_visible(true);
                }
                if package_data.extra_dependencies.is_empty() {
                    deps_label.set_visible(false);
                } else {
                    deps_label.set_text(&format!(
                        "Will also install: {}",
                        package_data.extra_dependencies.join(", ")
                    ));
                    deps_label.set_visible(true);
                }
                info_text.set_text(package_data.description.as_str());
                *current_url.borrow_mut() = package_data.url.clone();
                url_button.set_visible(package_data.url.is_some());

                let release_url = package_data.url.as_deref().and_then(release_notes_url);
                release_notes_button.set_visible(release_url.is_some());
                *current_release_notes_url.borrow_mut() = release_url;

                *current_package.borrow_mut() = Some(package_data.name.clone());
                pkgbuild_button.set_visible(package_data.source == PackageSource::Aur);
                aur_scan_button.set_visible(!package_data.aur_scan_findings.is_empty());
                let is_external = package_data.source == PackageSource::Flatpak
                    || package_data.source == PackageSource::AppImage;
                if is_external {
                    ignore_button.set_visible(false);
                } else {
                    let is_ignored = is_in_managed_ignore_pkg(&package_data.name);
                    if let Some(handler_id) = ignore_handler_id.borrow().as_ref() {
                        ignore_button.block_signal(handler_id);
                        ignore_button.set_active(is_ignored);
                        ignore_button.unblock_signal(handler_id);
                    } else {
                        ignore_button.set_active(is_ignored);
                    }
                    ignore_button.set_visible(true);
                    update_ignore_button_tooltip(&ignore_button);
                }
            } else {
                info_container.set_visible(false);
                title_label.set_text("Information");
                created_label.set_visible(false);
                maintainer_label.set_visible(false);
                permissions_label.set_visible(false);
                deps_label.set_visible(false);
                *current_url.borrow_mut() = None;
                url_button.set_visible(false);

                *current_release_notes_url.borrow_mut() = None;
                release_notes_button.set_visible(false);

                *current_package.borrow_mut() = None;
                pkgbuild_button.set_visible(false);
                aur_scan_button.set_visible(false);
                ignore_button.set_visible(false);
            }
        });
    }
    paned.set_position(380);

    content_box.append(&paned);

    update_statusbar(&statusbar, &store);
    content_box.append(&statusbar);

    return content_box;
}

fn create_main_content(
    decorations_disabled: bool,
    stack: &Stack,
    window: &ApplicationWindow,
) -> GtkBox {
    let content_box = GtkBox::new(Orientation::Vertical, 0);

    let view_stack = Stack::new();
    view_stack.set_vexpand(true);
    view_stack.add_titled(&build_install_tab(window), Some("install"), "Install");
    view_stack.add_titled(
        &build_update_tab(decorations_disabled, stack, window),
        Some("update"),
        "Update",
    );
    view_stack.add_titled(&build_manage_tab(window), Some("manage"), "Manage");
    view_stack.set_visible_child_name("update");

    content_box.append(&view_stack);
    return content_box;
}

pub fn update_layout(content_box: &GtkBox) -> Option<GtkBox> {
    let view_stack = content_box.first_child().and_downcast::<Stack>()?;
    return view_stack.child_by_name("update").and_downcast::<GtkBox>();
}

fn collect_selected_names(store: &ListStore) -> Vec<String> {
    let mut names = Vec::new();
    for i in 0..store.n_items() {
        if let Some(obj) = store.item(i).and_downcast::<PackageUpdateObject>() {
            let data = obj.data();
            if data.selected {
                names.push(data.name);
            }
        }
    }
    return names;
}

fn set_list_column_titles(column_view: &ColumnView, action: &str, size: &str) {
    let columns = column_view.columns();
    if let Some(col) = columns.item(2).and_downcast::<ColumnViewColumn>() {
        col.set_title(Some(action));
    }
    if let Some(col) = columns.item(5).and_downcast::<ColumnViewColumn>() {
        col.set_title(Some(size));
    }
}

fn bottom_bar(statusbar: &gtk4::Label) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 8);
    row.set_margin_start(8);
    row.set_margin_end(8);
    row.set_margin_top(6);
    row.set_margin_bottom(6);
    statusbar.set_hexpand(true);
    row.append(statusbar);
    return row;
}

fn build_install_tab(window: &ApplicationWindow) -> GtkBox {
    let tab = GtkBox::new(Orientation::Vertical, 0);

    let search_entry = SearchEntry::new();
    search_entry.set_placeholder_text(Some(
        "Search the repositories and the AUR, then press Enter",
    ));
    search_entry.set_hexpand(true);
    search_entry.set_margin_start(8);
    search_entry.set_margin_end(8);
    search_entry.set_margin_top(8);
    search_entry.set_margin_bottom(8);
    tab.append(&search_entry);

    let (list_view, store, statusbar, _filter) = create_package_list(&search_entry);
    set_list_column_titles(&list_view, "Install", "Size");
    let scrolled = ScrolledWindow::builder()
        .vexpand(true)
        .child(&list_view)
        .build();
    tab.append(&scrolled);

    let bottom = bottom_bar(&statusbar);
    let install_btn = Button::with_label("Install Selected");
    install_btn.add_css_class("suggested-action");
    bottom.append(&install_btn);
    tab.append(&bottom);

    let store_for_search = store.clone();
    search_entry.connect_activate(move |entry| {
        let query = entry.text().to_string();
        let store = store_for_search.clone();
        glib::spawn_future_local(async move {
            let results =
                gio::spawn_blocking(move || crate::helpers::search::search_packages(&query))
                    .await
                    .unwrap_or_default();
            store.remove_all();
            for pkg in results {
                store.append(&PackageUpdateObject::new(pkg));
            }
        });
    });

    let store_for_install = store.clone();
    let window_for_install = window.clone();
    install_btn.connect_clicked(move |_| {
        let names = collect_selected_names(&store_for_install);
        if names.is_empty() {
            return;
        }
        let _ = crate::ipc::client::attach_session();
        let command = format!("daim install {}", names.join(" "));
        let store_refresh = store_for_install.clone();
        run_command_in_dialog(
            window_for_install.upcast_ref::<gtk4::Window>(),
            &command,
            move || {
                store_refresh.remove_all();
            },
        );
    });

    return tab;
}

fn build_manage_tab(window: &ApplicationWindow) -> GtkBox {
    let tab = GtkBox::new(Orientation::Vertical, 0);

    let search_entry = SearchEntry::new();
    search_entry.set_placeholder_text(Some("Filter installed packages"));
    search_entry.set_hexpand(true);
    search_entry.set_margin_start(8);
    search_entry.set_margin_end(8);
    search_entry.set_margin_top(8);
    search_entry.set_margin_bottom(8);
    tab.append(&search_entry);

    let (list_view, store, statusbar, _filter) = create_package_list(&search_entry);
    set_list_column_titles(&list_view, "Select", "Size");
    let scrolled = ScrolledWindow::builder()
        .vexpand(true)
        .child(&list_view)
        .build();
    tab.append(&scrolled);

    let bottom = bottom_bar(&statusbar);
    let remove_btn = Button::with_label("Remove Selected");
    remove_btn.add_css_class("destructive-action");
    let orphans_btn = Button::with_label("Remove Orphans");
    let cache_btn = Button::with_label("Clean Cache");
    bottom.append(&remove_btn);
    bottom.append(&orphans_btn);
    bottom.append(&cache_btn);
    tab.append(&bottom);

    let store_pop = store.clone();
    let populate: std::rc::Rc<dyn Fn()> = std::rc::Rc::new(move || {
        let store = store_pop.clone();
        glib::spawn_future_local(async move {
            let packages =
                gio::spawn_blocking(crate::helpers::installed_packages::get_installed_packages)
                    .await
                    .unwrap_or_default();
            store.remove_all();
            for pkg in packages {
                store.append(&PackageUpdateObject::new(pkg));
            }
        });
    });
    populate();

    wire_manage_action(
        &remove_btn,
        window,
        &store,
        &populate,
        std::rc::Rc::new(|names: &[String]| format!("daim remove {}", names.join(" "))),
    );

    let window_for_orphans = window.clone();
    let populate_for_orphans = populate.clone();
    orphans_btn.connect_clicked(move |_| {
        let _ = crate::ipc::client::attach_session();
        let populate = populate_for_orphans.clone();
        run_command_in_dialog(
            window_for_orphans.upcast_ref::<gtk4::Window>(),
            "orphans=$(pacman -Qtdq); if [ -n \"$orphans\" ]; then daim remove --cascade $orphans; else echo 'No orphan packages found.'; fi",
            move || populate(),
        );
    });

    let window_for_cache = window.clone();
    cache_btn.connect_clicked(move |_| {
        let _ = crate::ipc::client::attach_session();
        run_command_in_dialog(
            window_for_cache.upcast_ref::<gtk4::Window>(),
            "daim paccache --keep 3",
            || {},
        );
    });

    return tab;
}

fn wire_manage_action(
    button: &Button,
    window: &ApplicationWindow,
    store: &ListStore,
    populate: &std::rc::Rc<dyn Fn()>,
    build_command: std::rc::Rc<dyn Fn(&[String]) -> String>,
) {
    let store = store.clone();
    let window = window.clone();
    let populate = populate.clone();
    button.connect_clicked(move |_| {
        let names = collect_selected_names(&store);
        if names.is_empty() {
            return;
        }
        let _ = crate::ipc::client::attach_session();
        let command = build_command(&names);
        let populate = populate.clone();
        run_command_in_dialog(window.upcast_ref::<gtk4::Window>(), &command, move || {
            populate()
        });
    });
}

fn wire_ignore_button(panel: &InfoPanel, stack: &Stack, window: &ApplicationWindow) {
    let stack = stack.clone();
    let window = window.clone();
    let current_package = panel.current_package.clone();
    let handler_id_cell = panel.ignore_handler_id.clone();
    let button = panel.ignore_button.clone();

    let handler_id = button.connect_toggled(move |btn| {
        let Some(pkg) = current_package.borrow().clone() else {
            return;
        };
        let target_state = btn.is_active();
        log_info!(
            "ignore toggle for {}: target={}",
            pkg,
            if target_state { "blacklist" } else { "unblacklist" }
        );

        let (title, message, accept_label) = if target_state {
            (
                "Add to blacklist?",
                format!(
                    "Add '{}' to /etc/pacman.conf IgnorePkg? Pacman will skip updates for this package until it is removed from the list.",
                    pkg
                ),
                "Add",
            )
        } else {
            (
                "Remove from blacklist?",
                format!(
                    "Remove '{}' from /etc/pacman.conf IgnorePkg? Pacman will resume updating this package.",
                    pkg
                ),
                "Remove",
            )
        };

        let stack_d = stack.clone();
        let window_d = window.clone();
        let pkg_d = pkg.clone();
        let btn_d = btn.clone();
        let handler_id_cell_d = handler_id_cell.clone();
        show_confirm_dialog(&window, title, &message, accept_label, move |accepted| {
            if !accepted {
                revert_toggle(&btn_d, &handler_id_cell_d, !target_state);
                return;
            }

            log_info!(
                "ignore toggle confirmed for {}: {}",
                pkg_d,
                if target_state { "added" } else { "removed" }
            );
            let _ = crate::ipc::client::attach_session();
            let result = crate::ipc::client::set_ignore_pkg(&pkg_d, target_state);
            match result {
                Ok(ref resp) if resp.is_success() => {
                    update_ignore_button_tooltip(&btn_d);
                    trigger_check_service();
                    if let Some(content_box) =
                        stack_d.child_by_name("content").and_downcast::<GtkBox>()
                    {
                        stack_d.set_visible_child_name("loading");
                        load_packages(stack_d.clone(), content_box, window_d.clone());
                    }
                }
                other => {
                    let msg = ignore_error_message(other);
                    eprintln!("Failed to update pacman.conf IgnorePkg: {}", msg);
                    show_error_dialog(
                        window_d.upcast_ref::<gtk4::Window>(),
                        "Failed to update pacman.conf",
                        &msg,
                    );
                    revert_toggle(&btn_d, &handler_id_cell_d, !target_state);
                }
            }
        });
    });

    *panel.ignore_handler_id.borrow_mut() = Some(handler_id);
}

fn ignore_error_message(result: std::io::Result<crate::ipc::protocol::Response>) -> String {
    use crate::ipc::protocol::Response;
    return match result {
        Ok(Response::Error { message }) => message,
        Ok(Response::Done { stderr, .. }) if !stderr.is_empty() => stderr,
        Ok(_) => "the helper reported a failure".to_string(),
        Err(e) => e.to_string(),
    };
}

fn revert_toggle(
    btn: &gtk4::ToggleButton,
    handler_id_cell: &std::rc::Rc<std::cell::RefCell<Option<glib::SignalHandlerId>>>,
    target: bool,
) {
    if let Some(h) = handler_id_cell.borrow().as_ref() {
        btn.block_signal(h);
        btn.set_active(target);
        btn.unblock_signal(h);
    } else {
        btn.set_active(target);
    }
    update_ignore_button_tooltip(btn);
}

fn wire_post_update_back_button(page: &PostUpdatePage, stack: &Stack, window: &ApplicationWindow) {
    let stack_clone = stack.clone();
    let window_clone = window.clone();
    page.back_button.connect_clicked(move |_| {
        log_info!("post-update: Back clicked");
        let Some(content_box) = stack_clone
            .child_by_name("content")
            .and_downcast::<GtkBox>()
        else {
            return;
        };
        stack_clone.set_visible_child_name("loading");
        load_packages(stack_clone.clone(), content_box, window_clone.clone());
    });
}

pub fn find_favorites_column(window: &ApplicationWindow) -> Option<ColumnViewColumn> {
    return find_column(window, 0);
}

fn info_title_markup(package: &PackageUpdate) -> String {
    let mut markup = glib::markup_escape_text(&package.name).to_string();
    if load_settings().show_updated_date {
        if let Some(ts) = package.build_date {
            let color = if prefers_dark() { "#c0bfbc" } else { "#9a9996" };
            markup.push_str(&format!(
                " <span foreground=\"{}\" size=\"small\">[{}]</span>",
                color,
                glib::markup_escape_text(&format_build_date(ts))
            ));
        }
    }
    return markup;
}

fn find_column(window: &ApplicationWindow, index: u32) -> Option<ColumnViewColumn> {
    let main_box = window.child().and_downcast::<GtkBox>()?;
    let stack = main_box.first_child().and_downcast::<Stack>()?;
    let content_box = stack.child_by_name("content").and_downcast::<GtkBox>()?;
    let update = update_layout(&content_box)?;
    let paned = update
        .last_child()
        .and_then(|c| c.prev_sibling())
        .and_downcast::<Paned>()?;
    let scrolled = paned.start_child().and_downcast::<ScrolledWindow>()?;
    let column_view = scrolled.child().and_downcast::<ColumnView>()?;
    return column_view
        .columns()
        .item(index)
        .and_downcast::<ColumnViewColumn>();
}

pub fn find_package_store(window: &ApplicationWindow) -> Option<ListStore> {
    let main_box = window.child().and_downcast::<GtkBox>()?;
    let stack = main_box.first_child().and_downcast::<Stack>()?;
    let content_box = stack.child_by_name("content").and_downcast::<GtkBox>()?;
    let update = update_layout(&content_box)?;
    let paned = update
        .last_child()
        .and_then(|c| c.prev_sibling())
        .and_downcast::<Paned>()?;
    let scrolled = paned.start_child().and_downcast::<ScrolledWindow>()?;
    let column_view = scrolled.child().and_downcast::<ColumnView>()?;
    return extract_list_store(&column_view);
}

fn extract_list_store(column_view: &ColumnView) -> Option<ListStore> {
    let selection_model = column_view.model()?;
    let single = selection_model.downcast_ref::<SingleSelection>()?;
    let filter_model = single.model()?.downcast::<FilterListModel>().ok()?;
    let sort_model = filter_model.model()?.downcast::<SortListModel>().ok()?;
    return sort_model.model().and_downcast::<ListStore>();
}

pub fn load_packages(stack: Stack, content_box: GtkBox, window: ApplicationWindow) {
    glib::spawn_future_local(async move {
        let packages_result = gio::spawn_blocking(|| get_package_updates()).await;

        match packages_result {
            Ok(Ok(mut packages)) => {
                let blacklisted = list_managed_ignores();
                if !blacklisted.is_empty() {
                    packages.retain(|p| !blacklisted.contains(&p.name));
                }

                let age_settings = load_settings();
                if age_settings.min_update_age_days > 0 {
                    let aur_only = age_settings.min_update_age_aur_only;
                    let cutoff = chrono::Utc::now().timestamp()
                        - (age_settings.min_update_age_days as i64) * 86_400;
                    packages.retain(|p| {
                        if aur_only && p.source != PackageSource::Aur {
                            return true;
                        }
                        match p.build_date {
                            Some(ts) => ts <= cutoff,
                            None => true,
                        }
                    });
                }

                if packages.is_empty() {
                    stack.set_visible_child_name("no-updates");
                    return;
                }

                let Some(update) = update_layout(&content_box) else {
                    eprintln!("Could not find update layout");
                    return;
                };

                let paned = update
                    .last_child()
                    .and_then(|child| child.prev_sibling())
                    .and_downcast::<Paned>();

                let Some(paned) = paned else {
                    eprintln!("Could not find paned widget");
                    return;
                };

                let scrolled = paned.start_child().and_downcast::<ScrolledWindow>();
                let Some(scrolled) = scrolled else {
                    eprintln!("Could not find scrolled window");
                    return;
                };

                let column_view = scrolled.child().and_downcast::<ColumnView>();
                let Some(column_view) = column_view else {
                    eprintln!("Could not find column view");
                    return;
                };

                let Some(list_store) = extract_list_store(&column_view) else {
                    eprintln!("Could not find list store");
                    return;
                };

                list_store.remove_all();

                let settings = load_settings();
                let unselected = if settings.remember_unselected_packages {
                    load_unselected_packages()
                } else {
                    Vec::new()
                };

                let mut packages = packages;
                packages.sort_by(|a, b| {
                    let a_fav = settings.enable_favorites && settings.is_favorite(&a.name);
                    let b_fav = settings.enable_favorites && settings.is_favorite(&b.name);
                    let a_aur = a.source == PackageSource::Aur;
                    let b_aur = b.source == PackageSource::Aur;
                    return b_fav.cmp(&a_fav).then_with(|| b_aur.cmp(&a_aur));
                });

                for mut package in packages {
                    if unselected.contains(&package.name) {
                        package.selected = false;
                    }
                    list_store.append(&PackageUpdateObject::new(package));
                }

                if let Some(statusbar) = update.last_child().and_downcast::<gtk4::Label>() {
                    update_statusbar(&statusbar, &list_store);
                }

                stack.set_visible_child_name("content");
            }
            Ok(Err(e)) => {
                if let UpdateError::SyncFailed(ref msg) = e {
                    if let Some(error_box) = stack.child_by_name("error").and_downcast::<GtkBox>() {
                        update_error_page_message(&error_box, msg);
                    }
                    stack.set_visible_child_name("error");
                } else {
                    show_error_dialog(
                        window.upcast_ref::<gtk4::Window>(),
                        "Error Loading Packages",
                        &format!("Failed to load package updates: {}", e),
                    );
                    eprintln!("Error loading packages: {}", e);
                    stack.set_visible_child_name("content");
                }
            }
            Err(e) => {
                eprintln!("Error in background thread: {:?}", e);
                stack.set_visible_child_name("content");
            }
        }
    });
}
