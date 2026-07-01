use crate::models::review_file::ReviewFile;

pub struct PkgbuildReview {
    pub package: String,
    pub diff: Option<String>,
    pub needs_review: bool,
    pub files: Vec<ReviewFile>,
}
