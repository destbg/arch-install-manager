use std::rc::Rc;

use gtk4::ApplicationWindow;
use gtk4::prelude::*;

use crate::constants::TIMESHIFT_COMMENT;
use crate::helpers::settings::load_settings;
use crate::helpers::snapper::{is_snap_pac_installed, is_snapper_installed};
use crate::ipc::client::call;
use crate::models::change_kind::ChangeKind;
use crate::models::op::Op;
use crate::ui::dialogs::{create_progress_dialog, show_error_dialog};
use crate::ui::terminal_page::run_command_in_dialog;

pub fn snapshot_flags(kind: ChangeKind) -> (bool, bool) {
    let settings = load_settings();
    let wanted = match kind {
        ChangeKind::Update => settings.snapshot_on_update,
        ChangeKind::Install => settings.snapshot_on_install,
        ChangeKind::Remove => settings.snapshot_on_remove,
    };
    if !wanted {
        return (false, false);
    }
    let timeshift = settings.create_timeshift_snapshot;
    let snapper =
        settings.create_snapper_snapshot && is_snapper_installed() && !is_snap_pac_installed();
    return (timeshift, snapper);
}

pub fn run_with_snapshots<F>(window: &ApplicationWindow, timeshift: bool, snapper: bool, proceed: F)
where
    F: FnOnce() + 'static,
{
    if !timeshift && !snapper {
        proceed();
        return;
    }

    let progress = create_progress_dialog(
        window.upcast_ref::<gtk4::Window>(),
        "Creating snapshot",
        "Creating a system snapshot before making changes.",
    );
    let window = window.clone();
    glib::spawn_future_local(async move {
        let outcome = gio::spawn_blocking(move || create_snapshots(timeshift, snapper)).await;
        progress.close();

        match outcome {
            Ok(Ok(())) => proceed(),
            Ok(Err(message)) => {
                eprintln!("Snapshot failed: {}", message);
                show_error_dialog(
                    window.upcast_ref::<gtk4::Window>(),
                    "Snapshot Failed",
                    &message,
                );
            }
            Err(_) => {
                show_error_dialog(
                    window.upcast_ref::<gtk4::Window>(),
                    "Snapshot Failed",
                    "The snapshot could not be created. Nothing was changed.",
                );
            }
        }
    });
    return;
}

pub fn run_change_command<F>(
    window: &ApplicationWindow,
    command: String,
    kind: ChangeKind,
    needs_helper: bool,
    offer_checks: bool,
    on_finished: F,
) where
    F: Fn() + 'static,
{
    let (timeshift, snapper) = snapshot_flags(kind);
    let window_for_run = window.clone();
    let on_finished = Rc::new(on_finished);
    run_with_snapshots(window, timeshift, snapper, move || {
        let on_finished = on_finished.clone();
        run_command_in_dialog(
            &window_for_run,
            &command,
            needs_helper,
            offer_checks,
            move || on_finished(),
        );
    });
    return;
}

fn create_snapshots(timeshift: bool, snapper: bool) -> Result<(), String> {
    if timeshift {
        let resp = call(Op::SnapshotTimeshift {
            comment: TIMESHIFT_COMMENT.to_string(),
        })
        .map_err(|e| e.to_string())?;
        if !resp.is_success() {
            return Err(
                "Could not create the Timeshift snapshot. Nothing was changed.".to_string(),
            );
        }
    }
    if snapper {
        let resp = call(Op::SnapshotSnapper {
            description: TIMESHIFT_COMMENT.to_string(),
        })
        .map_err(|e| e.to_string())?;
        if !resp.is_success() {
            return Err("Could not create the Snapper snapshot. Nothing was changed.".to_string());
        }
    }
    return Ok(());
}
