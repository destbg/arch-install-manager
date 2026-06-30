#[derive(Debug, Clone, Copy)]
pub struct SearchSources {
    pub official: bool,
    pub aur: bool,
    pub flatpak: bool,
}
