use std::collections::{HashMap, HashSet};

use alpm::{Alpm, SigLevel};

use crate::helpers::aur::list_foreign_packages;
use crate::models::package_source::PackageSource;
use crate::models::package_update::PackageUpdate;

pub fn get_all_installed_packages() -> Vec<String> {
    match std::process::Command::new("pacman").arg("-Q").output() {
        Ok(output) => String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        Err(_) => Vec::new(),
    }
}

pub fn get_installed_packages() -> Vec<PackageUpdate> {
    let Ok(conf) = pacmanconf::Config::new() else {
        return Vec::new();
    };
    let Ok(alpm) = Alpm::new(conf.root_dir.as_str(), conf.db_path.as_str()) else {
        return Vec::new();
    };
    for repo in &conf.repos {
        let _ = alpm.register_syncdb(repo.name.as_str(), SigLevel::NONE);
    }

    let mut repo_of: HashMap<String, String> = HashMap::new();
    for db in alpm.syncdbs() {
        let repo_name = db.name().to_string();
        for pkg in db.pkgs() {
            repo_of
                .entry(pkg.name().to_string())
                .or_insert_with(|| repo_name.clone());
        }
    }

    let foreign: HashSet<String> = list_foreign_packages()
        .into_iter()
        .map(|(name, _)| name)
        .collect();

    let mut out = Vec::new();
    for pkg in alpm.localdb().pkgs() {
        let name = pkg.name().to_string();
        let (source, repository) = if foreign.contains(&name) {
            (PackageSource::Aur, PackageSource::Aur.label().to_string())
        } else {
            let repo = repo_of
                .get(&name)
                .cloned()
                .unwrap_or_else(|| PackageSource::Official.label().to_string());
            (PackageSource::Official, repo)
        };
        out.push(PackageUpdate {
            source,
            repository,
            name,
            description: pkg.desc().unwrap_or("").to_string(),
            new_version: pkg.version().to_string(),
            size: pkg.isize(),
            ..Default::default()
        });
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    return out;
}
