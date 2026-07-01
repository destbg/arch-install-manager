pub const TIMESHIFT_COMMENT: &str = "arch-install-manager";
pub const APP_ID: &str = "com.destbg.arch-install-manager";

pub const OWN_PACKAGES: [&str; 3] = [
    "arch-install-manager",
    "arch-install-manager-bin",
    "arch-install-manager-git",
];

pub fn is_own_package(name: &str) -> bool {
    return OWN_PACKAGES.contains(&name);
}

pub fn is_recently_created(first_submitted: Option<i64>) -> bool {
    const WEEK: i64 = 7 * 24 * 3600;
    let Some(ts) = first_submitted else {
        return false;
    };
    let diff = chrono::Utc::now().timestamp() - ts;
    return diff >= 0 && diff < WEEK;
}
