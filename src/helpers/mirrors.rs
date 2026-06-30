use crate::helpers::aur::is_command_available;

const MIRRORLIST: &str = "/etc/pacman.d/mirrorlist";
const STALE_AFTER_DAYS: u64 = 30;

pub fn mirrorlist_age_days() -> Option<u64> {
    let meta = std::fs::metadata(MIRRORLIST).ok()?;
    let modified = meta.modified().ok()?;
    let elapsed = modified.elapsed().ok()?;
    return Some(elapsed.as_secs() / 86_400);
}

pub fn is_mirrorlist_stale() -> bool {
    return mirrorlist_age_days()
        .map(|days| days >= STALE_AFTER_DAYS)
        .unwrap_or(false);
}

pub fn mirror_refresh_command() -> Option<String> {
    if is_command_available("rate-mirrors") {
        return Some("daim refresh-mirrors rate-mirrors".to_string());
    }

    if is_command_available("reflector") {
        return Some("daim refresh-mirrors reflector".to_string());
    }

    return None;
}
