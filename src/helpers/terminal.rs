use std::env::vars;
use std::os::fd::{OwnedFd, RawFd};

use gio::Cancellable;
use glib::SpawnFlags;
use vte4::{PtyFlags, Terminal, TerminalExtManual};

const HELPER_CHILD_FD: RawFd = 3;

pub fn spawn_terminal(terminal: &Terminal, args: Vec<&str>, session: Option<OwnedFd>) {
    match session {
        Some(session) => spawn_with_session(terminal, args, session),
        None => spawn_plain(terminal, args),
    }
}

fn spawn_plain(terminal: &Terminal, args: Vec<&str>) {
    terminal.spawn_async(
        PtyFlags::DEFAULT,
        None,
        &args,
        &[],
        SpawnFlags::DEFAULT,
        || {},
        -1,
        None::<&Cancellable>,
        |result| {
            if let Err(e) = result {
                eprintln!("Failed to spawn terminal: {}", e);
            }
        },
    );
}

fn spawn_with_session(terminal: &Terminal, args: Vec<&str>, session: OwnedFd) {
    let env_owned = build_child_env();
    let env: Vec<&str> = env_owned.iter().map(|s| s.as_str()).collect();

    unsafe {
        terminal.spawn_with_fds_async(
            PtyFlags::DEFAULT,
            None,
            &args,
            &env,
            vec![session],
            &[HELPER_CHILD_FD],
            SpawnFlags::DEFAULT,
            || {},
            -1,
            None::<&Cancellable>,
            |result| {
                if let Err(e) = result {
                    eprintln!("Failed to spawn terminal: {}", e);
                }
            },
        );
    }
}

fn build_child_env() -> Vec<String> {
    let mut env: Vec<String> = vars().map(|(k, v)| format!("{k}={v}")).collect();
    env.push(format!("DAIM_HELPER_FD={HELPER_CHILD_FD}"));
    return env;
}
