use gio::Cancellable;
use glib::SpawnFlags;
use vte4::{PtyFlags, Terminal, TerminalExtManual};

pub fn spawn_terminal(terminal: &Terminal, args: Vec<&str>) {
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
