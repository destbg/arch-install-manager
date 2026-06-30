use std::io::{self, Read};
use std::os::fd::OwnedFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use nix::sys::socket::getsockopt;
use nix::sys::socket::sockopt::PeerCredentials;
use nix::unistd::{Gid, Uid, chown};

use crate::ipc::protocol::{MirrorTool, Op, Request, Response, recv_request, write_response};

pub struct Config {
    pub uid: u32,
    pub gid: u32,
    pub socket_path: PathBuf,
}

static ATTACHED: AtomicBool = AtomicBool::new(false);

static LAST_ACTIVITY: Mutex<Option<Instant>> = Mutex::new(None);

const IDLE_TIMEOUT: Duration = Duration::from_secs(900);

fn touch_activity() {
    if let Ok(mut guard) = LAST_ACTIVITY.lock() {
        *guard = Some(Instant::now());
    }
}

pub fn run(config: Config) -> io::Result<()> {
    if let Some(dir) = config.socket_path.parent() {
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        let _ = chown(
            dir,
            Some(Uid::from_raw(config.uid)),
            Some(Gid::from_raw(config.gid)),
        );
    }
    let _ = std::fs::remove_file(&config.socket_path);

    let listener = UnixListener::bind(&config.socket_path)?;
    std::fs::set_permissions(&config.socket_path, std::fs::Permissions::from_mode(0o600))?;
    let _ = chown(
        &config.socket_path,
        Some(Uid::from_raw(config.uid)),
        Some(Gid::from_raw(config.gid)),
    );

    touch_activity();
    spawn_idle_watchdog(config.socket_path.clone());

    println!("READY");
    use std::io::Write;
    let _ = io::stdout().flush();

    let authorized = config.uid;
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        std::thread::spawn(move || handle_connection(stream, authorized));
    }
    return Ok(());
}

fn spawn_idle_watchdog(socket_path: PathBuf) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(60));
            if ATTACHED.load(Ordering::SeqCst) {
                continue;
            }
            let idle = LAST_ACTIVITY
                .lock()
                .ok()
                .and_then(|g| *g)
                .map(|t| t.elapsed())
                .unwrap_or(Duration::ZERO);
            if idle > IDLE_TIMEOUT {
                let _ = std::fs::remove_file(&socket_path);
                std::process::exit(0);
            }
        }
    });
}

fn peer_uid(stream: &UnixStream) -> Option<u32> {
    return getsockopt(stream, PeerCredentials).ok().map(|c| c.uid());
}

fn handle_connection(mut stream: UnixStream, authorized_uid: u32) {
    match peer_uid(&stream) {
        Some(uid) if uid == authorized_uid || uid == 0 => {}
        _ => {
            let _ = write_response(&mut stream, &Response::error("unauthorized peer"));
            return;
        }
    }

    touch_activity();

    let received = match recv_request(&stream) {
        Ok(r) => r,
        Err(e) => {
            let _ = write_response(&mut stream, &Response::error(format!("bad request: {e}")));
            return;
        }
    };

    match received.req.op {
        Op::Attach => hold_attach(stream),
        Op::Ping => {
            let _ = write_response(&mut stream, &Response::Pong);
        }
        Op::Shutdown => {
            let _ = write_response(
                &mut stream,
                &Response::Done {
                    exit_code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                },
            );
            std::process::exit(0);
        }
        _ => {
            let resp = execute(&received.req, received.fds);
            let _ = write_response(&mut stream, &resp);
        }
    }
}

fn hold_attach(mut stream: UnixStream) {
    ATTACHED.store(true, Ordering::SeqCst);
    let mut buf = [0u8; 64];
    loop {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
    std::process::exit(0);
}

fn execute(req: &Request, fds: Vec<OwnedFd>) -> Response {
    match &req.op {
        Op::SyncDb => run_capture("pacman", &["-Sy"]),
        Op::SysUpgrade => run_tty_or_err("pacman", &["-Su"], &[], fds),
        Op::Install { targets, as_deps } => {
            if let Err(e) = validate_names(targets) {
                return Response::error(e);
            }
            let mut args = vec!["-S", "--needed"];
            if *as_deps {
                args.push("--asdeps");
            }
            args.extend(targets.iter().map(|s| s.as_str()));
            run_tty_or_err("pacman", &args, &[], fds)
        }
        Op::InstallFiles { paths, as_deps } => {
            if let Err(e) = validate_pkg_paths(paths) {
                return Response::error(e);
            }
            let mut args = vec!["-U"];
            if *as_deps {
                args.push("--asdeps");
            }
            args.extend(paths.iter().map(|s| s.as_str()));
            run_tty_or_err("pacman", &args, &[], fds)
        }
        Op::Remove {
            targets,
            cascade,
            nosave,
        } => {
            if let Err(e) = validate_names(targets) {
                return Response::error(e);
            }
            let mut flag = String::from("-R");
            if *cascade {
                flag.push('s');
            }
            if *nosave {
                flag.push('n');
            }
            let mut args = vec![flag.as_str()];
            args.extend(targets.iter().map(|s| s.as_str()));
            run_tty_or_err("pacman", &args, &[], fds)
        }
        Op::SetIgnorePkg { name, ignored } => {
            if let Err(e) = validate_name(name) {
                return Response::error(e);
            }
            let result = if *ignored {
                crate::helpers::pacman_ignore::add_to_ignore_pkg(name)
            } else {
                crate::helpers::pacman_ignore::remove_from_ignore_pkg(name)
            };
            match result {
                Ok(()) => done_ok(),
                Err(e) => Response::error(e.to_string()),
            }
        }
        Op::RemoveDbLock => match crate::helpers::database_lock::remove_database_lock() {
            Ok(()) => done_ok(),
            Err(e) => Response::error(e.to_string()),
        },
        Op::PaccacheClean {
            keep,
            keep_uninstalled,
        } => {
            let keep_flag = format!("-rk{keep}");
            let keep_uninstalled_flag = format!("-ruk{keep_uninstalled}");
            let first = run_capture("paccache", &[keep_flag.as_str()]);
            if !first.is_success() {
                return first;
            }
            run_capture("paccache", &[keep_uninstalled_flag.as_str()])
        }
        Op::RestartService { name } => {
            if let Err(e) = validate_name(name) {
                return Response::error(e);
            }
            run_capture("systemctl", &["restart", name])
        }
        Op::SnapshotTimeshift { comment } => run_capture(
            "timeshift",
            &["--create", "--comments", comment, "--tags", "O"],
        ),
        Op::SnapshotSnapper { description } => run_tty_or_err(
            "snapper",
            &[
                "-c",
                "root",
                "create",
                "--type",
                "single",
                "--description",
                description,
            ],
            &[],
            fds,
        ),
        Op::RefreshMirrors { tool } => run_refresh_mirrors(*tool, fds),
        Op::RunPacdiff => run_tty_or_err("pacdiff", &[], &[("DIFFPROG", "")], fds),
        Op::Attach | Op::Ping | Op::Shutdown => Response::error("unexpected control op"),
    }
}

fn run_refresh_mirrors(tool: MirrorTool, fds: Vec<OwnedFd>) -> Response {
    let mirrorlist = "/etc/pacman.d/mirrorlist";
    let _ = std::fs::copy(mirrorlist, format!("{mirrorlist}.bak"));
    match tool {
        MirrorTool::RateMirrors => run_tty_or_err(
            "rate-mirrors",
            &[
                "--save",
                mirrorlist,
                "--allow-root",
                "--protocol",
                "https",
                "arch",
            ],
            &[],
            fds,
        ),
        MirrorTool::Reflector => run_tty_or_err(
            "reflector",
            &[
                "--save",
                mirrorlist,
                "--protocol",
                "https",
                "--latest",
                "20",
                "--sort",
                "rate",
            ],
            &[],
            fds,
        ),
    }
}

fn done_ok() -> Response {
    return Response::Done {
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
    };
}

fn run_capture(program: &str, args: &[&str]) -> Response {
    match Command::new(program).args(args).output() {
        Ok(out) => Response::Done {
            exit_code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        },
        Err(e) => Response::error(format!("failed to run {program}: {e}")),
    }
}

fn run_tty_or_err(
    program: &str,
    args: &[&str],
    envs: &[(&str, &str)],
    mut fds: Vec<OwnedFd>,
) -> Response {
    if fds.len() < 3 {
        return Response::error("operation requires a terminal but none was provided");
    }
    let stderr_fd = fds.pop().unwrap();
    let stdout_fd = fds.pop().unwrap();
    let stdin_fd = fds.pop().unwrap();

    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::from(stdin_fd))
        .stdout(Stdio::from(stdout_fd))
        .stderr(Stdio::from(stderr_fd));
    for (k, v) in envs {
        if v.is_empty() {
            continue;
        }
        cmd.env(k, v);
    }

    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            libc::ioctl(0, libc::TIOCSCTTY as libc::c_ulong, 0);
            return Ok(());
        });
    }

    match cmd.status() {
        Ok(status) => Response::Done {
            exit_code: status.code().unwrap_or(-1),
            stdout: String::new(),
            stderr: String::new(),
        },
        Err(e) => Response::error(format!("failed to run {program}: {e}")),
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 256 {
        return Err(format!("invalid name: {name:?}"));
    }
    let ok = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '+' | '-' | ':'));
    if !ok {
        return Err(format!("invalid characters in name: {name:?}"));
    }
    return Ok(());
}

fn validate_names(names: &[String]) -> Result<(), String> {
    if names.is_empty() {
        return Err("no targets supplied".into());
    }
    for name in names {
        validate_name(name)?;
    }
    return Ok(());
}

fn validate_pkg_paths(paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("no package files supplied".into());
    }
    for p in paths {
        let path = Path::new(p);
        if !path.is_file() {
            return Err(format!("not a file: {p}"));
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.contains(".pkg.tar") {
            return Err(format!("not a package file: {p}"));
        }
    }
    return Ok(());
}
