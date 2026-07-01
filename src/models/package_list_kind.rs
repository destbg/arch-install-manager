#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageListKind {
    Install,
    Update,
    Manage,
}

impl PackageListKind {
    pub fn is_installed(&self) -> bool {
        return matches!(self, PackageListKind::Update | PackageListKind::Manage);
    }
}
