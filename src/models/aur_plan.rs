use crate::models::aur_build::AurBuild;

#[derive(Default)]
pub struct AurPlan {
    pub builds: Vec<AurBuild>,
    pub repo_deps: Vec<String>,
}
