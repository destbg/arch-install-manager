use serde::{Deserialize, Serialize};

use crate::models::mirror_tool::MirrorTool;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Op {
    NewSession,
    SyncDb,
    SysUpgrade,
    SysUpgradeNoConfirm,
    Install {
        targets: Vec<String>,
        as_deps: bool,
        reinstall: bool,
    },
    InstallFiles {
        paths: Vec<String>,
        as_deps: bool,
    },
    AurBuildInstall {
        name: String,
        as_deps: bool,
    },
    RemoveMakeDeps {
        targets: Vec<String>,
    },
    Remove {
        targets: Vec<String>,
        cascade: bool,
        nosave: bool,
    },
    SetIgnorePkg {
        name: String,
        ignored: bool,
    },
    RemoveDbLock,
    PaccacheClean {
        keep: u32,
        keep_uninstalled: u32,
    },
    RestartService {
        name: String,
    },
    SnapshotTimeshift {
        comment: String,
    },
    SnapshotSnapper {
        description: String,
    },
    RefreshMirrors {
        tool: MirrorTool,
    },
    RunPacdiff,
}

impl Op {
    pub fn wants_tty(&self) -> bool {
        return matches!(
            self,
            Op::SysUpgrade
                | Op::Install { .. }
                | Op::InstallFiles { .. }
                | Op::AurBuildInstall { .. }
                | Op::Remove { .. }
                | Op::RefreshMirrors { .. }
                | Op::RunPacdiff
        );
    }
}
