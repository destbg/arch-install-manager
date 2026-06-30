use alpm::{Alpm, SigLevel};
use std::collections::HashSet;

use crate::helpers::aur::search_aur_packages;
use crate::helpers::flatpak::search_flatpak;
use crate::models::package_source::PackageSource;
use crate::models::package_update::PackageUpdate;
use crate::models::search_sources::SearchSources;

const FEATURED_PACKAGES: &[&str] = &[
    "firefox",
    "chromium",
    "vlc",
    "mpv",
    "gimp",
    "inkscape",
    "krita",
    "blender",
    "obs-studio",
    "audacity",
    "libreoffice-fresh",
    "thunderbird",
    "neovim",
    "code",
    "steam",
    "telegram-desktop",
    "discord",
    "kdenlive",
    "keepassxc",
    "flameshot",
    "btop",
    "fastfetch",
];

pub fn search_packages(term: &str, sources: SearchSources) -> Vec<PackageUpdate> {
    if term.trim().is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();

    if sources.official {
        out.extend(search_repos(term));
    }

    if sources.aur {
        out.extend(search_aur_packages(term));
    }

    if sources.flatpak {
        out.extend(search_flatpak(term));
    }

    let needle = term.trim().to_lowercase();
    out.sort_by(|a, b| {
        return match_rank(&a.name, &a.description, &needle)
            .cmp(&match_rank(&b.name, &b.description, &needle))
            .then_with(|| source_rank(a.source).cmp(&source_rank(b.source)))
            .then_with(|| a.name.len().cmp(&b.name.len()))
            .then_with(|| a.name.cmp(&b.name));
    });

    return out;
}

pub fn featured_packages() -> Vec<PackageUpdate> {
    let Ok(conf) = pacmanconf::Config::new() else {
        return Vec::new();
    };
    let Ok(alpm) = Alpm::new(conf.root_dir.as_str(), conf.db_path.as_str()) else {
        return Vec::new();
    };
    for repo in &conf.repos {
        let _ = alpm.register_syncdb(repo.name.as_str(), SigLevel::NONE);
    }

    let mut out = Vec::new();
    for &name in FEATURED_PACKAGES {
        for db in alpm.syncdbs() {
            let Ok(pkg) = db.pkg(name) else {
                continue;
            };
            let build_date = pkg.build_date();
            out.push(PackageUpdate {
                source: PackageSource::Official,
                repository: db.name().to_string(),
                name: name.to_string(),
                new_version: pkg.version().to_string(),
                description: pkg.desc().unwrap_or("").to_string(),
                size: Some(pkg.isize()),
                url: pkg.url().map(|s| s.to_string()),
                build_date: if build_date > 0 {
                    Some(build_date)
                } else {
                    None
                },
                ..Default::default()
            });
            break;
        }
    }

    return out;
}

fn match_rank(name: &str, description: &str, needle: &str) -> u8 {
    if needle.is_empty() {
        return 0;
    }
    let name = name.to_lowercase();
    if name == needle {
        return 0;
    }
    if name.starts_with(needle) {
        return 1;
    }
    if name.contains(needle) {
        return 2;
    }
    if description.to_lowercase().contains(needle) {
        return 3;
    }
    return 4;
}

fn source_rank(source: PackageSource) -> u8 {
    return match source {
        PackageSource::Official => 0,
        PackageSource::Aur => 1,
        PackageSource::Flatpak => 2,
        PackageSource::AppImage => 3,
    };
}

fn search_repos(term: &str) -> Vec<PackageUpdate> {
    let Ok(conf) = pacmanconf::Config::new() else {
        return Vec::new();
    };
    let Ok(alpm) = Alpm::new(conf.root_dir.as_str(), conf.db_path.as_str()) else {
        return Vec::new();
    };
    for repo in &conf.repos {
        let _ = alpm.register_syncdb(repo.name.as_str(), SigLevel::NONE);
    }

    let terms: Vec<&str> = term.split_whitespace().collect();
    if terms.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for db in alpm.syncdbs() {
        let Ok(pkgs) = db.search(terms.iter().cloned()) else {
            continue;
        };
        for pkg in pkgs.iter() {
            let name = pkg.name().to_string();
            if !seen.insert(name.clone()) {
                continue;
            }
            let build_date = pkg.build_date();
            out.push(PackageUpdate {
                source: PackageSource::Official,
                repository: db.name().to_string(),
                name,
                new_version: pkg.version().to_string(),
                description: pkg.desc().unwrap_or("").to_string(),
                size: Some(pkg.isize()),
                url: pkg.url().map(|s| s.to_string()),
                build_date: if build_date > 0 {
                    Some(build_date)
                } else {
                    None
                },
                ..Default::default()
            });
        }
    }

    return out;
}
