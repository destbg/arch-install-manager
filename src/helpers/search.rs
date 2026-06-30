use std::process::Command;

use crate::helpers::aur::search_aur;
use crate::models::package_source::PackageSource;
use crate::models::package_update::PackageUpdate;

pub fn search_packages(term: &str) -> Vec<PackageUpdate> {
    if term.trim().is_empty() {
        return Vec::new();
    }

    let mut out = search_repos(term);

    for (name, version, description) in search_aur(term) {
        out.push(PackageUpdate {
            source: PackageSource::Aur,
            repository: PackageSource::Aur.label().to_string(),
            name,
            new_version: version,
            description,
            ..Default::default()
        });
    }

    return out;
}

fn search_repos(term: &str) -> Vec<PackageUpdate> {
    let Ok(output) = Command::new("pacman").args(["-Ss", term]).output() else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);

    let mut out = Vec::new();
    let mut current: Option<PackageUpdate> = None;

    for line in text.lines() {
        if line.starts_with(char::is_whitespace) {
            if let Some(pkg) = current.as_mut() {
                if pkg.description.is_empty() {
                    pkg.description = line.trim().to_string();
                }
            }
            continue;
        }

        if let Some(pkg) = current.take() {
            out.push(pkg);
        }

        let mut parts = line.split_whitespace();
        let Some(repo_name) = parts.next() else {
            continue;
        };
        let version = parts.next().unwrap_or("").to_string();
        let (repository, name) = match repo_name.split_once('/') {
            Some((repo, name)) => (repo.to_string(), name.to_string()),
            None => (String::new(), repo_name.to_string()),
        };

        current = Some(PackageUpdate {
            source: PackageSource::Official,
            repository,
            name,
            new_version: version,
            ..Default::default()
        });
    }

    if let Some(pkg) = current.take() {
        out.push(pkg);
    }

    return out;
}
