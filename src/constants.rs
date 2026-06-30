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
