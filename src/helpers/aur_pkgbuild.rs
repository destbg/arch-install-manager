use crate::{
    helpers::aur::url_encode, helpers::elevated::get_original_user, helpers::network::http_get,
    models::pkgbuild_review::PkgbuildReview, models::review_file::ReviewFile,
};
use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const PKGBUILD_FETCH_TIMEOUT_SECS: u32 = 10;
const MAX_REVIEW_FILE_BYTES: usize = 1_000_000;

pub fn prepare_pkgbuild_review(package: &str) -> Result<PkgbuildReview> {
    if let Some(dir) = find_clone_dir(package) {
        if let Some((diff, needs_review)) = compute_review(&dir) {
            return Ok(PkgbuildReview {
                package: package.to_string(),
                diff: Some(diff),
                needs_review,
                files: read_repo_files(&dir),
            });
        }
    }

    let files = files_from_fresh_clone(package);
    if !files.is_empty() {
        return Ok(PkgbuildReview {
            package: package.to_string(),
            diff: None,
            needs_review: false,
            files,
        });
    }

    let pkgbuild = fetch_remote_pkgbuild(package)?;
    return Ok(PkgbuildReview {
        package: package.to_string(),
        diff: None,
        needs_review: false,
        files: vec![ReviewFile {
            name: "PKGBUILD".to_string(),
            content: pkgbuild,
        }],
    });
}

pub fn pkgbuild_needs_review(package: &str) -> bool {
    if let Some(dir) = find_clone_dir(package) {
        if let Some((_, needs_review)) = compute_review(&dir) {
            return needs_review;
        }
    }
    return false;
}

pub fn find_clone_dir(package: &str) -> Option<PathBuf> {
    let cache = PathBuf::from(user_home()?).join(".cache");

    let candidates = [
        cache.join("daim").join("aur").join(package),
        cache.join("paru").join("clone").join(package),
        cache.join("paru").join(package),
        cache.join("yay").join(package),
        cache.join("trizen").join(package),
        cache.join("pikaur").join("aur_repos").join(package),
    ];

    for dir in candidates {
        if dir.join(".git").exists() && dir.join("PKGBUILD").exists() {
            return Some(dir);
        }
    }

    return None;
}

fn compute_review(dir: &Path) -> Option<(String, bool)> {
    let user = get_original_user();
    let dir_str = dir.to_str()?;

    let _ = run_git(user.as_deref(), dir_str, &["fetch", "--quiet"]);

    let output = run_git(
        user.as_deref(),
        dir_str,
        &[
            "diff",
            "--no-color",
            "--unified=100000",
            "HEAD",
            "FETCH_HEAD",
            "--",
            ".",
            ":(exclude).SRCINFO",
            ":(exclude).gitignore",
            ":(exclude).gitattributes",
        ],
    )?;
    if !output.status.success() {
        return None;
    }

    let diff = String::from_utf8_lossy(&output.stdout).into_owned();
    let needs_review = diff_adds_or_removes_lines(&diff);
    return Some((diff, needs_review));
}

fn run_git(user: Option<&str>, dir: &str, args: &[&str]) -> Option<std::process::Output> {
    let mut cmd = match user {
        Some(u) => {
            let mut c = Command::new("sudo");
            c.args(["-u", u, "git", "-C", dir]);
            c
        }
        None => {
            let mut c = Command::new("git");
            c.args(["-C", dir]);
            c
        }
    };
    cmd.args(args);
    return cmd.output().ok();
}

fn diff_adds_or_removes_lines(diff: &str) -> bool {
    let mut added = 0i32;
    let mut removed = 0i32;
    let mut in_hunk = false;
    let mut needs = false;

    for line in diff.lines() {
        let is_boundary = line.starts_with("@@")
            || line.starts_with("diff --git")
            || line.starts_with("index ")
            || line.starts_with("--- ")
            || line.starts_with("+++ ")
            || line.starts_with("new file")
            || line.starts_with("deleted file")
            || line.starts_with("rename ")
            || line.starts_with("similarity ")
            || line.starts_with("old mode")
            || line.starts_with("new mode")
            || line.starts_with("Binary files");

        if is_boundary {
            if in_hunk && added != removed {
                needs = true;
            }
            added = 0;
            removed = 0;
            in_hunk = line.starts_with("@@");
            continue;
        }

        if in_hunk {
            if line.starts_with('+') {
                added += 1;
            } else if line.starts_with('-') {
                removed += 1;
            }
        }
    }
    if in_hunk && added != removed {
        needs = true;
    }

    return needs;
}

fn read_repo_files(dir: &Path) -> Vec<ReviewFile> {
    let mut files = Vec::new();
    collect_files(dir, dir, &mut files);
    files.sort_by(|a, b| {
        review_rank(&a.name)
            .cmp(&review_rank(&b.name))
            .then_with(|| a.name.cmp(&b.name))
    });
    return files;
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<ReviewFile>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if is_skipped_entry(&file_name) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            collect_files(root, &path, out);
            continue;
        }
        let Some(rel) = path.strip_prefix(root).ok().and_then(|p| p.to_str()) else {
            continue;
        };
        let content = if file_type.is_symlink() {
            "(symlink, not shown)".to_string()
        } else {
            match read_text_file(&path) {
                Some(text) => text,
                None => continue,
            }
        };
        out.push(ReviewFile {
            name: rel.to_string(),
            content,
        });
    }
}

fn is_skipped_entry(name: &str) -> bool {
    return name == ".git"
        || name == ".SRCINFO"
        || name == ".gitignore"
        || name == ".gitattributes";
}

fn read_text_file(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    if bytes.len() > MAX_REVIEW_FILE_BYTES {
        return Some(format!(
            "(file is {} bytes, too large to display here)",
            bytes.len()
        ));
    }
    if bytes.contains(&0) {
        return Some(format!("(binary file, {} bytes, not shown)", bytes.len()));
    }
    return Some(match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => "(file is not valid text, not shown)".to_string(),
    });
}

fn review_rank(name: &str) -> u8 {
    if name == "PKGBUILD" {
        return 0;
    }
    if name.ends_with(".install") {
        return 1;
    }
    return 2;
}

fn files_from_fresh_clone(package: &str) -> Vec<ReviewFile> {
    let Some(dir) = clone_to_temp(package) else {
        return Vec::new();
    };
    let files = read_repo_files(&dir);
    let _ = fs::remove_dir_all(&dir);
    return files;
}

fn clone_to_temp(package: &str) -> Option<PathBuf> {
    if !is_valid_aur_name(package) {
        return None;
    }
    let dir = env::temp_dir().join(format!("daim-review-{}-{}", std::process::id(), package));
    let _ = fs::remove_dir_all(&dir);

    let url = format!("https://aur.archlinux.org/{}.git", package);
    let status = Command::new("git")
        .args(["clone", "--depth", "1"])
        .arg(&url)
        .arg(&dir)
        .status()
        .ok()?;
    if status.success() && dir.join("PKGBUILD").exists() {
        return Some(dir);
    }
    let _ = fs::remove_dir_all(&dir);
    return None;
}

fn is_valid_aur_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 256 {
        return false;
    }
    if name.starts_with('-') || name.starts_with('.') {
        return false;
    }
    return name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '+' | '-'));
}

fn fetch_remote_pkgbuild(package: &str) -> Result<String> {
    let url = format!(
        "https://aur.archlinux.org/cgit/aur.git/plain/PKGBUILD?h={}",
        url_encode(package)
    );
    return http_get(&url, PKGBUILD_FETCH_TIMEOUT_SECS)
        .with_context(|| format!("Could not download the PKGBUILD for {}", package));
}

fn user_home() -> Option<String> {
    if let Some(user) = get_original_user() {
        return Some(format!("/home/{}", user));
    }
    return std::env::var("HOME").ok();
}
