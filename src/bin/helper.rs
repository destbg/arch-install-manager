use std::path::PathBuf;
use std::process::exit;

use arch_install_manager::ipc::server::{Config, run};

fn main() {
    let config = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("daim-helper: {e}");
            eprintln!("usage: daim-helper --uid <uid> --gid <gid> --socket <path>");
            exit(2);
        }
    };

    if let Err(e) = run(config) {
        eprintln!("daim-helper: {e}");
        exit(1);
    }
}

fn parse_args() -> Result<Config, String> {
    let mut uid: Option<u32> = None;
    let mut gid: Option<u32> = None;
    let mut socket: Option<PathBuf> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--uid" => {
                uid = Some(
                    next_value(&mut args, "--uid")?
                        .parse()
                        .map_err(|_| "invalid uid")?,
                )
            }
            "--gid" => {
                gid = Some(
                    next_value(&mut args, "--gid")?
                        .parse()
                        .map_err(|_| "invalid gid")?,
                )
            }
            "--socket" => socket = Some(PathBuf::from(next_value(&mut args, "--socket")?)),
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    let uid = uid.ok_or("missing --uid")?;
    let gid = gid.ok_or("missing --gid")?;
    let socket_path =
        socket.unwrap_or_else(|| arch_install_manager::ipc::protocol::socket_path_for_uid(uid));

    return Ok(Config {
        uid,
        gid,
        socket_path,
    });
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    return args
        .next()
        .ok_or_else(|| format!("{flag} requires a value"));
}
