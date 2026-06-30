use crate::{
    helpers::aur_maintainers::{read_maintainers, write_maintainers},
    helpers::aur_pkgbuild::pkgbuild_needs_review,
    helpers::aur_scan::enrich_with_aur_scan,
    helpers::network::http_get,
    models::{aur_info::AurInfo, package_source::PackageSource, package_update::PackageUpdate},
};
use anyhow::Result;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::process::Command;

const AUR_RPC_TIMEOUT_SECS: u32 = 5;

pub fn is_command_available(command: &str) -> bool {
    return Command::new("which")
        .arg(command)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
}

pub fn list_foreign_packages() -> Vec<(String, String)> {
    let Ok(output) = Command::new("pacman").args(["-Qm"]).output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut result = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        if let (Some(name), Some(version)) = (parts.next(), parts.next()) {
            result.push((name.to_string(), version.to_string()));
        }
    }
    return result;
}

pub fn get_aur_updates() -> Result<Vec<PackageUpdate>> {
    let foreign = list_foreign_packages();
    if foreign.is_empty() {
        return Ok(Vec::new());
    }

    let names: Vec<&str> = foreign.iter().map(|(name, _)| name.as_str()).collect();
    let info_map = fetch_aur_info(&names);

    let mut updates = Vec::new();
    for (name, installed_version) in &foreign {
        let Some(info) = info_map.get(name) else {
            continue;
        };
        let Some(aur_version) = info.version.as_deref() else {
            continue;
        };
        if alpm::vercmp(aur_version, installed_version.as_str()) != Ordering::Greater {
            continue;
        }
        updates.push(new_aur_update(name, installed_version, aur_version));
    }

    enrich_with_aur_info(&mut updates);
    for update in &mut updates {
        update.pkgbuild_needs_review = pkgbuild_needs_review(&update.name);
    }
    enrich_with_aur_scan(&mut updates);
    return Ok(updates);
}

fn new_aur_update(name: &str, current_version: &str, new_version: &str) -> PackageUpdate {
    return PackageUpdate {
        source: PackageSource::Aur,
        repository: PackageSource::Aur.label().to_string(),
        selected: true,
        name: name.to_string(),
        description: format!("AUR package: {}", name),
        current_version: current_version.to_string(),
        new_version: new_version.to_string(),
        size: 0,
        url: Some(format!("https://aur.archlinux.org/packages/{}", name)),
        build_date: None,
        first_submitted: None,
        out_of_date: None,
        orphaned: false,
        maintainer: None,
        previous_maintainer: None,
        num_votes: None,
        popularity: None,
        security_severity: None,
        security_issues: Vec::new(),
        new_permissions: Vec::new(),
        extra_dependencies: Vec::new(),
        pkgbuild_needs_review: false,
        aur_scan_findings: Vec::new(),
        flatpak_installation: None,
        appimage_path: None,
    };
}

pub(crate) fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    return out;
}

fn enrich_with_aur_info(updates: &mut [PackageUpdate]) {
    if updates.is_empty() {
        return;
    }

    let names: Vec<&str> = updates.iter().map(|u| u.name.as_str()).collect();
    let info_map = fetch_aur_info(&names);
    if info_map.is_empty() {
        return;
    }

    let mut known_maintainers = read_maintainers();
    let mut maintainers_changed = false;

    for update in updates.iter_mut() {
        let Some(info) = info_map.get(&update.name) else {
            continue;
        };

        if let Some(description) = &info.description {
            if !description.is_empty() {
                update.description = description.clone();
            }
        }
        if let Some(url) = &info.url {
            if !url.is_empty() {
                update.url = Some(url.clone());
            }
        }
        update.build_date = info.last_modified;
        update.first_submitted = info.first_submitted;
        update.out_of_date = info.out_of_date;
        update.orphaned = info.maintainer.is_none();
        update.maintainer = info.maintainer.clone();
        update.num_votes = info.num_votes;
        update.popularity = info.popularity;

        if let Some(current) = &info.maintainer {
            match known_maintainers.get(&update.name) {
                Some(previous) if previous != current => {
                    update.previous_maintainer = Some(previous.clone());
                }
                None => {
                    known_maintainers.insert(update.name.clone(), current.clone());
                    maintainers_changed = true;
                }
                _ => {}
            }
        }
    }

    if maintainers_changed {
        write_maintainers(&known_maintainers);
    }
}

pub fn fetch_aur_info(names: &[&str]) -> HashMap<String, AurInfo> {
    let mut map = HashMap::new();
    if names.is_empty() {
        return map;
    }

    let url = aur_rpc_info_url(names);
    let Ok(body) = http_get(&url, AUR_RPC_TIMEOUT_SECS) else {
        return map;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) else {
        return map;
    };
    let Some(results) = json.get("results").and_then(|r| r.as_array()) else {
        return map;
    };

    for entry in results {
        let Some(name) = entry.get("Name").and_then(|n| n.as_str()) else {
            continue;
        };
        let str_field = |key: &str| {
            entry
                .get(key)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        };
        map.insert(
            name.to_string(),
            AurInfo {
                version: str_field("Version"),
                description: str_field("Description"),
                url: str_field("URL"),
                last_modified: entry.get("LastModified").and_then(|v| v.as_i64()),
                first_submitted: entry.get("FirstSubmitted").and_then(|v| v.as_i64()),
                out_of_date: entry.get("OutOfDate").and_then(|v| v.as_i64()),
                maintainer: str_field("Maintainer"),
                num_votes: entry.get("NumVotes").and_then(|v| v.as_i64()),
                popularity: entry.get("Popularity").and_then(|v| v.as_f64()),
            },
        );
    }

    return map;
}

pub fn search_aur(term: &str) -> Vec<(String, String, String)> {
    if term.trim().is_empty() {
        return Vec::new();
    }
    let url = format!(
        "https://aur.archlinux.org/rpc/v5/search/{}",
        url_encode(term)
    );
    let Ok(body) = http_get(&url, AUR_RPC_TIMEOUT_SECS) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) else {
        return Vec::new();
    };
    let Some(results) = json.get("results").and_then(|r| r.as_array()) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for entry in results {
        let name = entry
            .get("Name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }
        let version = entry
            .get("Version")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let description = entry
            .get("Description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        out.push((name, version, description));
    }
    return out;
}

fn aur_rpc_info_url(names: &[&str]) -> String {
    let mut url = String::from("https://aur.archlinux.org/rpc/v5/info?");
    for (i, name) in names.iter().enumerate() {
        if i > 0 {
            url.push('&');
        }

        url.push_str("arg%5B%5D=");
        url.push_str(&url_encode(name));
    }
    return url;
}
