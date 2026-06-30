use arch_install_manager::constants::APP_ID;
use arch_install_manager::helpers::logger;
use arch_install_manager::helpers::settings::load_settings;
use arch_install_manager::log_info;
use arch_install_manager::ui::build_ui;
use gtk4::Application;
use gtk4::prelude::*;
use std::env;
use std::process::Command;

fn main() {
    arch_install_manager::ipc::client::set_launcher("pkexec");

    logger::init();
    logger::cleanup_old_logs(load_settings().log_retention_days);
    log_info!("application starting");

    gtk4::init().expect("Failed to initialize GTK4");

    apply_system_theme();

    let app = Application::builder().application_id(APP_ID).build();

    app.connect_activate(|app| {
        log_info!("building UI");
        build_ui(app);
    });

    app.run();
    log_info!("application exiting");
}

fn apply_system_theme() {
    let Some(settings) = gtk4::Settings::default() else {
        return;
    };

    if let Some(theme) = get_system_gtk_theme() {
        settings.set_gtk_theme_name(Some(&theme));
    }

    if let Some(prefers_dark) = detect_prefers_dark() {
        settings.set_gtk_application_prefer_dark_theme(prefers_dark);
    }
}

fn get_system_gtk_theme() -> Option<String> {
    if let Ok(theme_env) = env::var("GTK_THEME") {
        return Some(theme_env);
    }

    if let Ok(output) = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "gtk-theme"])
        .output()
    {
        if let Ok(val) = String::from_utf8(output.stdout) {
            let theme = val.trim().trim_matches('\'').to_string();
            if !theme.is_empty() {
                return Some(theme);
            }
        }
    }

    return None;
}

fn detect_prefers_dark() -> Option<bool> {
    if let Ok(theme_env) = env::var("GTK_THEME") {
        let t = theme_env.to_lowercase();
        return Some(t.contains("dark"));
    }

    if let Ok(output) = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
    {
        if let Ok(val) = String::from_utf8(output.stdout) {
            let v = val.trim().to_lowercase();
            if v.contains("prefer-dark") {
                return Some(true);
            }
            if v.contains("default") || v.contains("prefer-light") {
                return Some(false);
            }
        }
    }

    if let Ok(output) = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "gtk-theme"])
        .output()
    {
        if let Ok(val) = String::from_utf8(output.stdout) {
            if val.trim().to_lowercase().contains("dark") {
                return Some(true);
            }
        }
    }

    return None;
}
