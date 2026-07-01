use std::path::PathBuf;
use std::process::exit;

use arch_install_manager::ipc::server::{Config, run};

fn main() {
    let config = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("daim-helper: {e}");
            eprintln!("usage: daim-helper --connect <path>");
            exit(2);
        }
    };

    if let Err(e) = run(config) {
        eprintln!("daim-helper: {e}");
        exit(1);
    }
}

fn parse_args() -> Result<Config, String> {
    let mut connect_path: Option<PathBuf> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--connect" => connect_path = Some(PathBuf::from(next_value(&mut args, "--connect")?)),
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    let connect_path = connect_path.ok_or("missing --connect")?;

    return Ok(Config { connect_path });
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    return args
        .next()
        .ok_or_else(|| format!("{flag} requires a value"));
}
