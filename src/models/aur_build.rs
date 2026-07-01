use std::path::PathBuf;

pub struct AurBuild {
    pub name: String,
    pub dir: PathBuf,
    pub explicit: bool,
    pub fresh: bool,
    pub prev_commit: Option<String>,
}
