use std::process::exit;

use arch_install_manager::engine;
use arch_install_manager::ipc::client;
use arch_install_manager::ipc::protocol::{MirrorTool, Op, Response};

fn main() {
    if unsafe { libc::geteuid() } == 0 {
        eprintln!(
            "daim: refusing to run as root. Run it as your normal user. daim asks for admin\n      \
             rights only when it needs them. It builds AUR packages as you."
        );
        exit(1);
    }
    client::set_launcher("sudo");

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        usage();
        exit(2);
    }

    match args[0].as_str() {
        "upgrade" | "u" => exit(engine::upgrade()),
        "install" | "i" => {
            let mut skip_review = false;
            let mut reinstall = false;
            let mut pkgs = Vec::new();
            for a in &args[1..] {
                match a.as_str() {
                    "--skip-review" => skip_review = true,
                    "--reinstall" => reinstall = true,
                    other => pkgs.push(other.to_string()),
                }
            }
            exit(engine::install(&pkgs, skip_review, reinstall));
        }
        "search" | "s" | "query" | "q" => {
            let mut select = true;
            let mut terms = Vec::new();
            for a in &args[1..] {
                match a.as_str() {
                    "--no-select" | "-n" => select = false,
                    other => terms.push(other.to_string()),
                }
            }
            exit(engine::search(&terms.join(" "), select));
        }
        _ => {}
    }

    let op = match parse(&args) {
        Ok(op) => op,
        Err(msg) => {
            eprintln!("daim: {msg}");
            usage();
            exit(2);
        }
    };

    let result = if op.wants_tty() {
        client::call_with_tty(op)
    } else {
        client::call(op)
    };

    match result {
        Ok(Response::Done {
            exit_code,
            stdout,
            stderr,
        }) => {
            if !stdout.is_empty() {
                print!("{stdout}");
            }
            if !stderr.is_empty() {
                eprint!("{stderr}");
            }
            exit(exit_code);
        }
        Ok(Response::Pong) => exit(0),
        Ok(Response::Error { message }) => {
            eprintln!("daim: {message}");
            exit(1);
        }
        Err(e) => {
            eprintln!("daim: {e}");
            exit(1);
        }
    }
}

fn parse(args: &[String]) -> Result<Op, String> {
    let cmd = args[0].as_str();
    let rest = &args[1..];
    let op = match cmd {
        "sync" | "sy" => Op::SyncDb,
        "install-file" | "if" => Op::InstallFiles {
            paths: rest.to_vec(),
            as_deps: false,
        },
        "remove" | "r" => {
            let mut cascade = false;
            let mut nosave = false;
            let mut targets = Vec::new();
            for a in rest {
                match a.as_str() {
                    "--cascade" => cascade = true,
                    "--nosave" => nosave = true,
                    other => targets.push(other.to_string()),
                }
            }
            Op::Remove {
                targets,
                cascade,
                nosave,
            }
        }
        "paccache" | "pc" => {
            let flag_value = |flag: &str| {
                rest.iter()
                    .position(|a| a == flag)
                    .and_then(|i| rest.get(i + 1))
                    .and_then(|v| v.parse().ok())
            };
            let keep = flag_value("--keep").unwrap_or(3);
            Op::PaccacheClean {
                keep,
                keep_uninstalled: flag_value("--keep-uninstalled").unwrap_or(keep),
            }
        }
        "restart-service" | "rs" => Op::RestartService {
            name: rest
                .first()
                .ok_or("restart-service requires a name")?
                .clone(),
        },
        "snapshot-timeshift" | "st" => Op::SnapshotTimeshift {
            comment: rest.join(" "),
        },
        "snapshot-snapper" | "sn" => Op::SnapshotSnapper {
            description: rest.join(" "),
        },
        "refresh-mirrors" | "mr" => {
            let tool = match rest.first().map(|s| s.as_str()) {
                Some("reflector") => MirrorTool::Reflector,
                _ => MirrorTool::RateMirrors,
            };
            Op::RefreshMirrors { tool }
        }
        "pacdiff" | "pd" => Op::RunPacdiff,
        "remove-db-lock" | "unlock" => Op::RemoveDbLock,
        "set-ignore" | "ignore" => {
            let ignored = match rest.first().map(|s| s.as_str()) {
                Some("add") => true,
                Some("remove") => false,
                _ => return Err("set-ignore requires add|remove <pkg>".into()),
            };
            Op::SetIgnorePkg {
                name: rest.get(1).ok_or("set-ignore requires a package")?.clone(),
                ignored,
            }
        }
        other => return Err(format!("unknown command: {other}")),
    };
    return Ok(op);
}

fn usage() {
    eprintln!(
        "usage: daim <command> [args]\n\
         \n\
         commands:\n  \
         s | search | q | query <term> [--no-select]\n                                search the repositories and the AUR,\n                                then pick numbers to install (--no-select to only list)\n  \
         sync                          refresh the package databases\n  \
         upgrade                       upgrade installed packages\n  \
         install <pkg>...              install packages\n  \
         install-file <path>...        install package files\n  \
         remove [--cascade] [--nosave] <pkg>...\n  \
         paccache [--keep N]           clean the package cache\n  \
         restart-service <name>        restart a system service\n  \
         snapshot-timeshift <comment>  create a Timeshift snapshot\n  \
         snapshot-snapper <desc>       create a Snapper snapshot\n  \
         refresh-mirrors [rate-mirrors|reflector]\n  \
         pacdiff                       merge pacnew files\n  \
         remove-db-lock                remove a stale pacman db lock\n  \
         set-ignore <add|remove> <pkg> blacklist a package from updates"
    );
}
