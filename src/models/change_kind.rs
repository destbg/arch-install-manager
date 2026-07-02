#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Update,
    Install,
    Remove,
}
