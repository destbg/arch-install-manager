use std::process::Command;

use chrono::Utc;

use arch_install_manager::helpers::aur::get_aur_updates;
use arch_install_manager::helpers::network::is_network_metered;
use arch_install_manager::helpers::pacman_ignore::list_managed_ignores;
use arch_install_manager::helpers::power::is_on_battery;
use arch_install_manager::helpers::settings::load_settings;
use arch_install_manager::helpers::snooze::current_snooze_until;
use arch_install_manager::helpers::tray_state::write_tray_state;
use arch_install_manager::models::tray_state::TrayState;

fn main() {
    let manual = std::env::args().any(|a| a == "--manual");
    let settings = load_settings();

    if let Some(until) = current_snooze_until() {
        eprintln!(
            "Skipping update check: snoozed until {}.",
            until.format("%Y-%m-%d %H:%M:%S UTC")
        );
        return;
    }
    if !manual && settings.skip_check_on_metered && is_network_metered() {
        eprintln!("Skipping update check: network is metered.");
        return;
    }
    if !manual && settings.skip_check_on_battery && is_on_battery() {
        eprintln!("Skipping update check: running on battery.");
        return;
    }

    let blacklist = list_managed_ignores();

    let packages: Vec<String> = get_repo_updates()
        .unwrap_or_else(|e| {
            eprintln!("Failed to get repo updates: {}", e);
            Vec::new()
        })
        .into_iter()
        .filter(|line| !line_starts_with_any(line, &blacklist))
        .collect();

    let aur = if settings.enable_aur_support {
        match get_aur_updates() {
            Ok(updates) => updates
                .into_iter()
                .filter(|u| !blacklist.contains(&u.name))
                .map(|u| format!("{} {} -> {}", u.name, u.current_version, u.new_version))
                .collect(),
            Err(e) => {
                eprintln!("Failed to get AUR updates: {}", e);
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    let flatpak: Vec<String> = Vec::new();
    let appimage: Vec<String> = Vec::new();

    let state = TrayState {
        last_check: Some(Utc::now()),
        packages,
        aur,
        flatpak,
        appimage,
    };

    if let Err(e) = write_tray_state(&state) {
        eprintln!("Failed to write state file: {}", e);
        std::process::exit(1);
    }

    signal_tray();
}

fn signal_tray() {
    let _ = Command::new("pkill")
        .args(["-USR1", "-f", "daim-tray"])
        .status();
}

fn line_starts_with_any(line: &str, names: &[String]) -> bool {
    let first = line.split_whitespace().next().unwrap_or("");
    if first.is_empty() {
        return false;
    }
    return names.iter().any(|n| n == first);
}

fn get_repo_updates() -> anyhow::Result<Vec<String>> {
    let sync = Command::new("pacman").args(["-Sy"]).output()?;
    if !sync.status.success() {
        let stderr = String::from_utf8_lossy(&sync.stderr);
        return Err(anyhow::anyhow!("pacman -Sy failed: {}", stderr.trim()));
    }

    let output = Command::new("pacman").args(["-Qu"]).output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut updates = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            updates.push(trimmed.to_string());
        }
    }
    return Ok(updates);
}
