use std::collections::HashSet;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::constants::{is_own_package, is_recently_created};
use crate::engine::picker::pick_updates;
use crate::helpers::appimage::build_appimage_update_commands;
use crate::helpers::arch_news::news_to_show;
use crate::helpers::flatpak::build_flatpak_update_command;
use crate::helpers::format::format_build_date;
use crate::helpers::package_updates::get_package_updates;
use crate::helpers::post_update::{
    get_cache_candidates, get_orphan_packages, get_pacnew_files, get_services_needing_restart,
};
use crate::helpers::repo_switches::detect_repo_switches;
use crate::helpers::search::search_packages;
use crate::helpers::settings::load_settings;
use crate::ipc::client;
use crate::ipc::protocol::{Op, Response};
use crate::models::aur_build::AurBuild;
use crate::models::aur_plan::AurPlan;
use crate::models::package_source::PackageSource;
use crate::models::package_update::PackageUpdate;
use crate::models::search_sources::SearchSources;

mod picker;

const CLI_SEARCH_SOURCES: SearchSources = SearchSources {
    official: true,
    aur: true,
    flatpak: false,
};

pub fn install(targets: &[String], skip_review: bool, reinstall: bool) -> i32 {
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

    let mut plan = AurPlan::default();
    let mut visited = HashSet::new();
    for name in &aur_targets {
        if let Err(e) = resolve_aur(name, true, &mut visited, &mut plan) {
            eprintln!("daim: failed to prepare {name}: {e}");
            return 1;
        }
    }

    if !skip_review && !plan.builds.is_empty() && !review_all_terminal(&plan.builds) {
        eprintln!("daim: installation cancelled");
        return 1;
    }

    if !repo_targets.is_empty() {
        match client::call_with_tty(Op::Install {
            targets: repo_targets,
            as_deps: false,
            reinstall,
        }) {
            Ok(resp) if resp.is_success() => {}
            Ok(resp) => return report_failure(&resp),
            Err(e) => {
                eprintln!("daim: {e}");
                return 1;
            }
        }
    }

    let repo_deps = pending_repo_deps(&plan.repo_deps);
    let mut deps_installed: Vec<String> = Vec::new();
    if !repo_deps.is_empty() {
        match client::call_with_tty(Op::Install {
            targets: repo_deps.clone(),
            as_deps: true,
            reinstall: false,
        }) {
            Ok(resp) if resp.is_success() => deps_installed = repo_deps,
            Ok(resp) => return report_failure(&resp),
            Err(e) => {
                eprintln!("daim: {e}");
                return 1;
            }
        }
    }

    for build in &plan.builds {
        if let Err(e) = build_and_install(build) {
            eprintln!("daim: failed to install {}: {e}", build.name);
            return 1;
        }
    }

    cleanup_make_deps(&deps_installed);

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

    let names: Vec<String> = picks
        .into_iter()
        .map(|i| {
            let item = &items[i];
            if item.source == PackageSource::Aur {
                format!("aur/{}", item.name)
            } else {
                item.name.clone()
            }
        })
        .collect();
    return install(&names, false, false);
}

pub fn upgrade() -> i32 {
    show_news_cli();

    if let Err(e) = client::ensure_running() {
        eprintln!("daim: could not start the privileged helper: {e}");
        return 1;
    }

    println!("Refreshing package databases...");
    let _ = client::call(Op::SyncDb);

    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    if !interactive {
        return full_upgrade();
    }

    println!("Checking for updates...");
    let updates = match get_package_updates() {
        Ok(updates) => updates,
        Err(e) => {
            eprintln!("daim: failed to check for updates: {e}");
            return 1;
        }
    };

    if updates.is_empty() {
        println!("System is already up to date.");
        run_after_update_checks();
        return 0;
    }

    let Some(picks) = pick_updates(&updates) else {
        println!("Upgrade cancelled.");
        return 0;
    };
    if picks.is_empty() {
        println!("No packages selected.");
        return 0;
    }

    let selected: Vec<&PackageUpdate> = picks.iter().map(|&i| &updates[i]).collect();
    let code = install_selected_updates(&selected);
    if code != 0 {
        return code;
    }

    run_after_update_checks();
    return 0;
}

fn render(items: &[PackageUpdate], numbered: bool) {
    let tty = std::io::stdout().is_terminal();

    let mut blocks: Vec<String> = Vec::with_capacity(items.len());
    for (index, pkg) in items.iter().enumerate() {
        let number = if numbered { Some(index + 1) } else { None };
        blocks.push(format_item(number, pkg, tty));
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

fn format_item(number: Option<usize>, pkg: &PackageUpdate, tty: bool) -> String {
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

    for (text, code) in item_chips(pkg) {
        header.push(' ');
        header.push_str(&paint(tty, code, &format!("[{text}]")));
    }

    if let Some(ts) = pkg.build_date {
        header.push_str("  ");
        header.push_str(&paint(
            tty,
            "90",
            &format!("updated {}", format_build_date(ts)),
        ));
    }

    let mut block = header;
    if !pkg.description.is_empty() {
        block.push_str("\n    ");
        block.push_str(&pkg.description);
    }
    return block;
}

fn item_chips(pkg: &PackageUpdate) -> Vec<(String, &'static str)> {
    let mut chips = Vec::new();
    if !pkg.current_version.is_empty() {
        chips.push(("installed".to_string(), "1;32"));
    }
    if pkg.is_repo_switch {
        chips.push(("repo switch".to_string(), "1;34"));
    }
    if is_recently_created(pkg.first_submitted) {
        chips.push(("new".to_string(), "1;31"));
    }
    if pkg.maintainer_changed() {
        chips.push(("maintainer changed".to_string(), "33"));
    }
    if !pkg.new_permissions.is_empty() {
        chips.push(("new permissions".to_string(), "33"));
    }
    if pkg.pkgbuild_needs_review {
        chips.push(("review PKGBUILD".to_string(), "33"));
    }
    if pkg.orphaned {
        chips.push(("orphaned".to_string(), "1;33"));
    }
    if pkg.out_of_date.is_some() {
        chips.push(("out of date".to_string(), "90"));
    }
    if let Some(severity) = &pkg.security_severity {
        chips.push((severity.clone(), "1;31"));
    }
    if !is_own_package(&pkg.name) {
        if let Some((severity, count)) = pkg.aur_scan_summary() {
            chips.push((format!("aur-scan: {severity} ({count})"), "1;31"));
        }
    }
    return chips;
}

fn paint(tty: bool, code: &str, text: &str) -> String {
    if tty {
        return format!("\x1b[{code}m{text}\x1b[0m");
    }
    return text.to_string();
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
        if let Some(name) = target.strip_prefix("aur/") {
            aur.push(name.to_string());
        } else if is_repo_package(target) {
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

fn resolve_aur(
    name: &str,
    explicit: bool,
    visited: &mut HashSet<String>,
    plan: &mut AurPlan,
) -> Result<(), String> {
    if !is_valid_aur_name(name) {
        return Err(format!("invalid package name: {name}"));
    }
    if !visited.insert(name.to_string()) {
        return Ok(());
    }

    let (dir, fresh, prev_commit) = fetch_aur(name)?;

    for dependency in parse_srcinfo_deps(&dir) {
        if is_satisfied(&dependency) {
            continue;
        }
        let base = dependency_base_name(&dependency);
        if is_repo_package(&base) {
            plan.repo_deps.push(base);
        } else {
            resolve_aur(&base, false, visited, plan)?;
        }
    }

    plan.builds.push(AurBuild {
        name: name.to_string(),
        dir,
        explicit,
        fresh,
        prev_commit,
    });

    return Ok(());
}

fn pending_repo_deps(deps: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for dep in deps {
        if seen.insert(dep.clone()) && !is_satisfied(dep) {
            out.push(dep.clone());
        }
    }
    return out;
}

fn build_and_install(build: &AurBuild) -> Result<(), String> {
    let resp = client::call_with_tty(Op::AurBuildInstall {
        name: build.name.clone(),
        as_deps: !build.explicit,
    })
    .map_err(|e| e.to_string())?;
    if !resp.is_success() {
        return Err(format!("failed to install {}", build.name));
    }

    return Ok(());
}

fn review_all_terminal(builds: &[AurBuild]) -> bool {
    println!();
    println!(
        "==> {} package(s) will be built from the AUR. Review the files before continuing.",
        builds.len()
    );
    for build in builds {
        print_review(build);
    }
    return prompt_proceed();
}

fn print_review(build: &AurBuild) {
    println!();
    println!("================ {} ================", build.name);

    if !build.fresh {
        if let Some(prev) = &build.prev_commit {
            match git_diff(&build.dir, prev) {
                Some(diff) if !diff.trim().is_empty() => {
                    println!("Changes since your installed version:");
                    print!("{diff}");
                    if !diff.ends_with('\n') {
                        println!();
                    }
                    return;
                }
                Some(_) => {
                    println!("No changes to the package files since your installed version.");
                    return;
                }
                None => {}
            }
        }
    }

    match read_pkgbuild(&build.dir) {
        Some(text) => {
            println!("PKGBUILD:");
            println!("{text}");
        }
        None => println!("(could not read PKGBUILD)"),
    }
}

fn prompt_proceed() -> bool {
    if !std::io::stdin().is_terminal() {
        return true;
    }
    print!("\n==> Proceed with build and install? [Y/n] ");
    let _ = std::io::stdout().flush();

    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
        return true;
    }
    let answer = line.trim().to_ascii_lowercase();
    return answer.is_empty() || answer == "y" || answer == "yes";
}

fn cleanup_make_deps(deps_installed: &[String]) {
    if deps_installed.is_empty() {
        return;
    }
    let orphans = current_orphans();
    let removable: Vec<String> = deps_installed
        .iter()
        .filter(|dep| orphans.contains(dep.as_str()))
        .cloned()
        .collect();
    if removable.is_empty() {
        return;
    }
    println!(
        "==> Removing build-only dependencies that are no longer needed: {}",
        removable.join(", ")
    );
    if let Err(e) = client::call(Op::RemoveMakeDeps { targets: removable }) {
        eprintln!("daim: could not remove build-only dependencies: {e}");
    }
}

fn current_orphans() -> HashSet<String> {
    let Ok(output) = Command::new("pacman").args(["-Qtdq"]).output() else {
        return HashSet::new();
    };
    return String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
}

fn aur_cache_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    return PathBuf::from(home).join(".cache").join("daim").join("aur");
}

fn fetch_aur(name: &str) -> Result<(PathBuf, bool, Option<String>), String> {
    let dir = aur_cache_dir().join(name);
    let dir_str = dir.to_string_lossy().to_string();

    if dir.join(".git").is_dir() {
        let prev = git_head(&dir);
        run_inherit(Command::new("git").args(["-C", &dir_str, "pull", "--ff-only"]))?;
        return Ok((dir, false, prev));
    }

    std::fs::create_dir_all(aur_cache_dir()).map_err(|e| e.to_string())?;
    let url = format!("https://aur.archlinux.org/{name}.git");
    run_inherit(Command::new("git").args(["clone", &url, &dir_str]))?;
    return Ok((dir, true, None));
}

fn git_head(dir: &Path) -> Option<String> {
    let dir_str = dir.to_string_lossy().to_string();
    let output = Command::new("git")
        .args(["-C", &dir_str, "rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if head.is_empty() {
        return None;
    }
    return Some(head);
}

fn git_diff(dir: &Path, from: &str) -> Option<String> {
    let dir_str = dir.to_string_lossy().to_string();
    let output = Command::new("git")
        .args([
            "-C",
            &dir_str,
            "diff",
            "--no-color",
            from,
            "HEAD",
            "--",
            ".",
            ":(exclude).SRCINFO",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    return Some(String::from_utf8_lossy(&output.stdout).into_owned());
}

fn read_pkgbuild(dir: &Path) -> Option<String> {
    return std::fs::read_to_string(dir.join("PKGBUILD")).ok();
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

fn full_upgrade() -> i32 {
    match client::call(Op::SysUpgradeNoConfirm) {
        Ok(Response::Done {
            exit_code,
            stdout,
            stderr,
        }) => {
            if !stdout.is_empty() {
                print!("{stdout}");
            }
            if !stderr.is_empty() {
                eprint!("{stderr}");
            }
            exit_code
        }
        Ok(_) => 0,
        Err(e) => {
            eprintln!("daim: {e}");
            1
        }
    }
}

fn install_selected_updates(selected: &[&PackageUpdate]) -> i32 {
    let mut targets = Vec::new();
    let mut flatpak: Vec<&PackageUpdate> = Vec::new();
    let mut appimage: Vec<&PackageUpdate> = Vec::new();

    for &pkg in selected {
        match pkg.source {
            PackageSource::Aur => targets.push(format!("aur/{}", pkg.name)),
            PackageSource::Official => targets.push(pkg.name.clone()),
            PackageSource::Flatpak => flatpak.push(pkg),
            PackageSource::AppImage => appimage.push(pkg),
        }
    }

    if !targets.is_empty() {
        let code = install(&targets, false, false);
        if code != 0 {
            return code;
        }
    }

    if !flatpak.is_empty() {
        run_shell(build_flatpak_update_command(&flatpak));
    }

    if !appimage.is_empty() {
        let commands = build_appimage_update_commands(&appimage);
        if !commands.is_empty() {
            run_shell(Some(commands.join(" && ")));
        }
    }

    return 0;
}

fn run_shell(command: Option<String>) {
    let Some(command) = command else {
        return;
    };
    let _ = Command::new("bash").args(["-lc", &command]).status();
}

fn show_news_cli() {
    if !load_settings().check_arch_news {
        return;
    }
    let items = news_to_show();
    if items.is_empty() {
        return;
    }
    println!("== Arch Linux news ==");
    for item in &items {
        println!();
        println!("{}", item.title);
        if !item.link.is_empty() {
            println!("{}", item.link);
        }
    }
    println!();
}

fn run_after_update_checks() {
    if !std::io::stdin().is_terminal() {
        return;
    }
    println!();
    println!("== After update checks ==");
    check_orphans();
    check_pacnew();
    check_services();
    check_repo_switches();
    check_cache();
}

fn check_orphans() {
    let Ok(orphans) = get_orphan_packages() else {
        return;
    };
    if orphans.is_empty() {
        return;
    }
    println!();
    println!("These packages are no longer needed by anything.");
    println!("  {}", orphans.join(", "));
    if !confirm("Remove them?") {
        return;
    }
    let _ = client::call_with_tty(Op::Remove {
        targets: orphans,
        cascade: true,
        nosave: true,
    });
}

fn check_pacnew() {
    let Ok(files) = get_pacnew_files() else {
        return;
    };
    if files.is_empty() {
        return;
    }
    println!();
    println!("Some configuration files were updated and need review.");
    println!("  {}", files.join(", "));
    if !confirm("Merge them with pacdiff?") {
        return;
    }
    let _ = client::call_with_tty(Op::RunPacdiff);
}

fn check_services() {
    let Ok(services) = get_services_needing_restart() else {
        return;
    };
    if services.is_empty() {
        return;
    }
    println!();
    println!("These services still run code from packages that were updated.");
    println!("  {}", services.join(", "));
    if !confirm("Restart them?") {
        return;
    }
    for name in services {
        let _ = client::call(Op::RestartService { name });
    }
}

fn check_repo_switches() {
    let Ok(switches) = detect_repo_switches() else {
        return;
    };
    if switches.is_empty() {
        return;
    }
    println!();
    println!("Some installed packages can move to an official repository.");
    for switch in &switches {
        println!(
            "  {} ({} to {})",
            switch.installed_name, switch.installed_repo, switch.target_repo
        );
    }
    if !confirm("Switch them now?") {
        return;
    }
    let targets: Vec<String> = switches.iter().map(|s| s.target_name.clone()).collect();
    install(&targets, false, true);
}

fn check_cache() {
    let settings = load_settings();
    let Ok(cache) = get_cache_candidates(
        settings.keep_old_packages,
        settings.keep_uninstalled_packages,
    ) else {
        return;
    };
    if cache.old_count == 0 && cache.uninstalled_count == 0 {
        return;
    }
    println!();
    println!(
        "The package cache has {} old and {} uninstalled versions to clean.",
        cache.old_count, cache.uninstalled_count
    );
    if !confirm("Clean the cache?") {
        return;
    }
    let _ = client::call(Op::PaccacheClean {
        keep: settings.keep_old_packages,
        keep_uninstalled: settings.keep_uninstalled_packages,
    });
}

fn confirm(question: &str) -> bool {
    print!("{question} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
        return false;
    }
    let answer = line.trim().to_ascii_lowercase();
    return answer == "y" || answer == "yes";
}
