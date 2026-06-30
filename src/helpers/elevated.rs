use std::path::Path;
use std::process::Command;

pub fn get_original_user() -> Option<String> {
    return None;
}

pub fn chown_to_user(_path: &Path) {}

pub fn open_url_as_user(url: &str) {
    if !is_safe_url(url) {
        return;
    }
    let _ = Command::new("xdg-open").arg(url).spawn();
}

pub fn spawn_as_user_or_root(program: &str, args: &[&str]) {
    let _ = Command::new(program).args(args).spawn();
}

fn is_safe_url(url: &str) -> bool {
    const SAFE_SCHEMES: &[&str] = &["http://", "https://", "appstream://"];
    for scheme in SAFE_SCHEMES {
        if url.starts_with(scheme) {
            return true;
        }
    }
    return false;
}
