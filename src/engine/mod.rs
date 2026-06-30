use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::helpers::search::search_packages;
use crate::ipc::client;
use crate::ipc::protocol::{Op, Response};
use crate::models::package_source::PackageSource;
use crate::models::package_update::PackageUpdate;
use crate::models::search_sources::SearchSources;

const CLI_SEARCH_SOURCES: SearchSources = SearchSources {
    official: true,
    aur: true,
    flatpak: false,
};

pub fn install(targets: &[String]) -> i32 {
    if targets.is_empty() {
        eprintln!("daim: no packages specified");
        return 2;
    }

    if let Err(e) = client::ensure_running() {
        eprintln!("daim: could not start the privileged helper: {e}");
        return 1;
    }

    let _ = client::call(Op::SyncDb);

    let (repo_targets, aur_targets) = partition_targets(targets);

    if !repo_targets.is_empty() {
        match client::call_with_tty(Op::Install {
            targets: repo_targets,
            as_deps: false,
        }) {
            Ok(resp) if resp.is_success() => {}
            Ok(resp) => return report_failure(&resp),
            Err(e) => {
                eprintln!("daim: {e}");
                return 1;
            }
        }
    }

    let mut visited = HashSet::new();
    for name in &aur_targets {
        if let Err(e) = ensure_aur_installed(name, true, &mut visited) {
            eprintln!("daim: failed to install {name}: {e}");
            return 1;
        }
    }

    return 0;
}

pub fn search(term: &str, select: bool) -> i32 {
    let items = search_packages(term, CLI_SEARCH_SOURCES);
    if items.is_empty() {
        eprintln!("daim: no packages found for '{term}'");
        return 1;
    }
    render(&items, select);

    if !select {
        return 0;
    }

    print!("==> Packages to install (e.g. 1, 1 3 5, 2-4): ");
    let _ = std::io::stdout().flush();

    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
        return 0;
    }

    let picks = parse_picks(&line, items.len());
    if picks.is_empty() {
        return 0;
    }

    let names: Vec<String> = picks.into_iter().map(|i| items[i].name.clone()).collect();
    return install(&names);
}

fn render(items: &[PackageUpdate], numbered: bool) {
    let tty = std::io::stdout().is_terminal();
    let installed = installed_versions();

    let mut blocks: Vec<String> = Vec::with_capacity(items.len());
    for (index, pkg) in items.iter().enumerate() {
        let number = if numbered { Some(index + 1) } else { None };
        blocks.push(format_item(number, pkg, &installed, tty));
    }

    if numbered {
        for block in blocks.iter().rev() {
            println!("{block}");
        }
    } else {
        for block in &blocks {
            println!("{block}");
        }
    }
}

fn format_item(
    number: Option<usize>,
    pkg: &PackageUpdate,
    installed: &HashMap<String, String>,
    tty: bool,
) -> String {
    let repo = if pkg.source == PackageSource::Aur {
        "aur".to_string()
    } else if pkg.repository.is_empty() {
        "repo".to_string()
    } else {
        pkg.repository.clone()
    };
    let repo_color = if pkg.source == PackageSource::Aur {
        "1;36"
    } else {
        "1;35"
    };

    let mut header = String::new();
    if let Some(num) = number {
        header.push_str(&paint(tty, "1;34", &num.to_string()));
        header.push(' ');
    }
    header.push_str(&paint(tty, repo_color, &repo));
    header.push('/');
    header.push_str(&paint(tty, "1", &pkg.name));
    header.push(' ');
    header.push_str(&paint(tty, "1;32", &pkg.new_version));

    if let Some(version) = installed.get(&pkg.name) {
        let tag = if version == &pkg.new_version {
            "[installed]".to_string()
        } else {
            format!("[installed: {version}]")
        };
        header.push(' ');
        header.push_str(&paint(tty, "1;34", &tag));
    }

    let mut block = header;
    if !pkg.description.is_empty() {
        block.push_str("\n    ");
        block.push_str(&pkg.description);
    }
    return block;
}

fn paint(tty: bool, code: &str, text: &str) -> String {
    if tty {
        return format!("\x1b[{code}m{text}\x1b[0m");
    }
    return text.to_string();
}

fn installed_versions() -> HashMap<String, String> {
    let Ok(output) = Command::new("pacman").arg("-Q").output() else {
        return HashMap::new();
    };
    let mut map = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.split_whitespace();
        if let Some(name) = parts.next() {
            map.insert(name.to_string(), parts.next().unwrap_or("").to_string());
        }
    }
    return map;
}

fn parse_picks(line: &str, total: usize) -> Vec<usize> {
    let mut out = Vec::new();
    for token in line.split(|c: char| c.is_whitespace() || c == ',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        if let Some((a, b)) = token.split_once('-') {
            if let (Ok(a), Ok(b)) = (a.trim().parse::<usize>(), b.trim().parse::<usize>()) {
                let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                for number in lo..=hi {
                    push_pick(&mut out, number, total);
                }
            }
        } else if let Ok(number) = token.parse::<usize>() {
            push_pick(&mut out, number, total);
        }
    }
    return out;
}

fn push_pick(out: &mut Vec<usize>, number: usize, total: usize) {
    if number >= 1 && number <= total {
        let index = number - 1;
        if !out.contains(&index) {
            out.push(index);
        }
    }
}

fn partition_targets(targets: &[String]) -> (Vec<String>, Vec<String>) {
    let mut repo = Vec::new();
    let mut aur = Vec::new();
    for target in targets {
        if is_repo_package(target) {
            repo.push(target.clone());
        } else {
            aur.push(target.clone());
        }
    }
    return (repo, aur);
}

fn is_repo_package(name: &str) -> bool {
    return Command::new("pacman")
        .args(["-Si", name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
}

fn is_satisfied(dependency: &str) -> bool {
    return Command::new("pacman")
        .args(["-T", dependency])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
}

fn ensure_aur_installed(
    name: &str,
    explicit: bool,
    visited: &mut HashSet<String>,
) -> Result<(), String> {
    if !visited.insert(name.to_string()) {
        return Ok(());
    }

    let dir = fetch_aur(name)?;

    let mut repo_deps = Vec::new();
    for dependency in parse_srcinfo_deps(&dir) {
        if is_satisfied(&dependency) {
            continue;
        }
        let base = dependency_base_name(&dependency);
        if is_repo_package(&base) {
            repo_deps.push(base);
        } else {
            ensure_aur_installed(&base, false, visited)?;
        }
    }

    if !repo_deps.is_empty() {
        let resp = client::call_with_tty(Op::Install {
            targets: repo_deps,
            as_deps: true,
        })
        .map_err(|e| e.to_string())?;
        if !resp.is_success() {
            return Err("failed to install repository dependencies".to_string());
        }
    }

    makepkg(&dir)?;

    let files = packagelist(&dir)?;
    if files.is_empty() {
        return Err(format!("no package files were produced for {name}"));
    }

    let resp = client::call_with_tty(Op::InstallFiles {
        paths: files,
        as_deps: !explicit,
    })
    .map_err(|e| e.to_string())?;
    if !resp.is_success() {
        return Err(format!("failed to install {name}"));
    }

    return Ok(());
}

fn aur_cache_dir() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache"));
    return base.join("daim").join("aur");
}

fn fetch_aur(name: &str) -> Result<PathBuf, String> {
    let dir = aur_cache_dir().join(name);
    let dir_str = dir.to_string_lossy().to_string();

    if dir.join(".git").is_dir() {
        run_inherit(Command::new("git").args(["-C", &dir_str, "pull", "--ff-only"]))?;
    } else {
        std::fs::create_dir_all(aur_cache_dir()).map_err(|e| e.to_string())?;
        let url = format!("https://aur.archlinux.org/{name}.git");
        run_inherit(Command::new("git").args(["clone", &url, &dir_str]))?;
    }
    return Ok(dir);
}

fn makepkg(dir: &Path) -> Result<(), String> {
    run_inherit(
        Command::new("makepkg")
            .current_dir(dir)
            .args(["-f", "--noconfirm"]),
    )?;
    return Ok(());
}

fn packagelist(dir: &Path) -> Result<Vec<String>, String> {
    let output = Command::new("makepkg")
        .current_dir(dir)
        .arg("--packagelist")
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("makepkg --packagelist failed".to_string());
    }
    return Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect());
}

fn parse_srcinfo_deps(dir: &Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(dir.join(".SRCINFO")) else {
        return Vec::new();
    };
    let mut deps = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        for key in ["depends", "makedepends", "checkdepends"] {
            if let Some(rest) = line.strip_prefix(key) {
                if let Some(value) = rest.trim_start().strip_prefix('=') {
                    let value = value.trim();
                    if !value.is_empty() {
                        deps.push(value.to_string());
                    }
                }
            }
        }
    }
    return deps;
}

fn dependency_base_name(dependency: &str) -> String {
    let end = dependency
        .find(|c| matches!(c, '>' | '<' | '='))
        .unwrap_or(dependency.len());
    return dependency[..end].to_string();
}

fn run_inherit(command: &mut Command) -> Result<(), String> {
    let status = command.status().map_err(|e| e.to_string())?;
    if status.success() {
        return Ok(());
    }
    return Err("command exited with a non-zero status".to_string());
}

fn report_failure(resp: &Response) -> i32 {
    match resp {
        Response::Done {
            exit_code, stderr, ..
        } => {
            if !stderr.is_empty() {
                eprint!("{stderr}");
            }
            *exit_code
        }
        Response::Error { message } => {
            eprintln!("daim: {message}");
            1
        }
        Response::Pong => 0,
    }
}
