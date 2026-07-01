use std::io::{self, Read};
use std::os::fd::OwnedFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use nix::sys::socket::getsockopt;
use nix::sys::socket::sockopt::PeerCredentials;
use nix::unistd::{Gid, Uid, User, chown, setgid, setgroups, setuid};

use crate::helpers::database_lock::remove_database_lock;
use crate::helpers::pacman_ignore::{add_to_ignore_pkg, remove_from_ignore_pkg};
use crate::ipc::protocol::{MirrorTool, Op, Request, Response, recv_request, write_response};

const IDLE_TIMEOUT: Duration = Duration::from_secs(1800);
const BUILD_USER: &str = "daim-build";
const AUR_CLONE_SUBDIR: &str = ".cache/daim/aur";
const BUILD_ROOT: &str = "/var/lib/daim/build";
const BUILD_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin";

pub struct Config {
    pub uid: u32,
    pub gid: u32,
    pub socket_path: PathBuf,
}

static LAST_ACTIVITY: Mutex<Option<Instant>> = Mutex::new(None);

static ACTIVE_OPS: AtomicUsize = AtomicUsize::new(0);

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

fn touch_activity() {
    if let Ok(mut guard) = LAST_ACTIVITY.lock() {
        *guard = Some(Instant::now());
    }
}

fn spawn_idle_watchdog(socket_path: PathBuf) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(30));
            if ACTIVE_OPS.load(Ordering::SeqCst) > 0 {
                touch_activity();
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
            ACTIVE_OPS.fetch_add(1, Ordering::SeqCst);
            let resp = execute(&received.req, received.fds, authorized_uid);
            ACTIVE_OPS.fetch_sub(1, Ordering::SeqCst);
            touch_activity();
            let _ = write_response(&mut stream, &resp);
        }
    }
}

fn hold_attach(mut stream: UnixStream) {
    let mut buf = [0u8; 64];
    loop {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
    std::process::exit(0);
}

fn execute(req: &Request, fds: Vec<OwnedFd>, uid: u32) -> Response {
    match &req.op {
        Op::SyncDb => run_capture("pacman", &["-Sy"]),
        Op::SysUpgrade => run_tty_or_err("pacman", &["-Su"], &[], fds),
        Op::SysUpgradeNoConfirm => run_capture("pacman", &["-Su", "--noconfirm"]),
        Op::Install {
            targets,
            as_deps,
            reinstall,
        } => {
            if let Err(e) = validate_names(targets) {
                return Response::error(e);
            }
            let mut args = vec!["-S"];
            if !*reinstall {
                args.push("--needed");
            }
            if *as_deps {
                args.push("--asdeps");
            }
            args.push("--");
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
            args.push("--");
            args.extend(paths.iter().map(|s| s.as_str()));
            run_tty_or_err("pacman", &args, &[], fds)
        }
        Op::AurBuildInstall { name, as_deps } => build_and_install_aur(name, *as_deps, uid, fds),
        Op::RemoveMakeDeps { targets } => {
            if let Err(e) = validate_names(targets) {
                return Response::error(e);
            }
            let mut args = vec!["-Rs", "--noconfirm", "--"];
            args.extend(targets.iter().map(|s| s.as_str()));
            run_capture("pacman", &args)
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
            let mut args = vec![flag.as_str(), "--"];
            args.extend(targets.iter().map(|s| s.as_str()));
            run_tty_or_err("pacman", &args, &[], fds)
        }
        Op::SetIgnorePkg { name, ignored } => {
            if let Err(e) = validate_name(name) {
                return Response::error(e);
            }
            let result = if *ignored {
                add_to_ignore_pkg(name)
            } else {
                remove_from_ignore_pkg(name)
            };
            match result {
                Ok(()) => done_ok(),
                Err(e) => Response::error(e.to_string()),
            }
        }
        Op::RemoveDbLock => match remove_database_lock() {
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

fn build_and_install_aur(name: &str, as_deps: bool, uid: u32, fds: Vec<OwnedFd>) -> Response {
    if let Err(e) = validate_name(name) {
        return Response::error(e);
    }
    let tty = match into_tty(fds) {
        Some(t) => t,
        None => return Response::error("operation requires a terminal but none was provided"),
    };
    let caller = match User::from_uid(Uid::from_raw(uid)) {
        Ok(Some(u)) => u,
        _ => return Response::error("could not resolve the requesting user"),
    };
    let source_dir = caller.dir.join(AUR_CLONE_SUBDIR).join(name);
    if !source_dir.join("PKGBUILD").is_file() {
        return Response::error(format!("no PKGBUILD found for {name}"));
    }
    let builder = match User::from_name(BUILD_USER) {
        Ok(Some(u)) => u,
        _ => return Response::error("the daim-build user is missing, reinstall to create it"),
    };

    let build_dir = Path::new(BUILD_ROOT).join(name);
    if let Err(e) = prepare_build_dir(&source_dir, &build_dir, &builder) {
        let _ = std::fs::remove_dir_all(&build_dir);
        return Response::error(e);
    }

    let result = run_build_and_install(&build_dir, &builder, as_deps, &tty);
    let _ = std::fs::remove_dir_all(&build_dir);
    return result;
}

fn prepare_build_dir(source_dir: &Path, build_dir: &Path, builder: &User) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(build_dir);
    if let Some(parent) = build_dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let copied = Command::new("cp")
        .arg("-a")
        .arg("--no-preserve=ownership")
        .arg(source_dir)
        .arg(build_dir)
        .status()
        .map_err(|e| e.to_string())?;
    if !copied.success() {
        return Err("failed to copy the package sources".to_string());
    }

    let _ = std::fs::remove_dir_all(build_dir.join(".git"));

    let spec = format!("{}:{}", builder.uid.as_raw(), builder.gid.as_raw());
    let owned = Command::new("chown")
        .arg("-R")
        .arg(&spec)
        .arg(build_dir)
        .status()
        .map_err(|e| e.to_string())?;
    if !owned.success() {
        return Err("failed to set build directory ownership".to_string());
    }
    return Ok(());
}

fn run_build_and_install(
    build_dir: &Path,
    builder: &User,
    as_deps: bool,
    tty: &[OwnedFd; 3],
) -> Response {
    let uid = builder.uid.as_raw();
    let gid = builder.gid.as_raw();
    let home = builder.dir.to_string_lossy().to_string();

    let mut makepkg = Command::new("makepkg");
    makepkg
        .current_dir(build_dir)
        .args(["-f", "--noconfirm"])
        .env("HOME", &home)
        .env("PATH", BUILD_PATH);
    match spawn_tty(&mut makepkg, tty, Some((uid, gid))) {
        Ok(status) if status.success() => {}
        Ok(_) => return Response::error("makepkg failed while building the package"),
        Err(e) => return Response::error(format!("failed to run makepkg: {e}")),
    }

    let files = match build_package_list(build_dir, uid, gid, &home) {
        Ok(files) if !files.is_empty() => files,
        Ok(_) => return Response::error("no package files were produced"),
        Err(e) => return Response::error(e),
    };

    let mut pacman = Command::new("pacman");
    pacman.arg("-U");
    if as_deps {
        pacman.arg("--asdeps");
    }
    pacman.arg("--");
    for file in &files {
        pacman.arg(file);
    }
    match spawn_tty(&mut pacman, tty, None) {
        Ok(status) => Response::Done {
            exit_code: status.code().unwrap_or(-1),
            stdout: String::new(),
            stderr: String::new(),
        },
        Err(e) => Response::error(format!("failed to run pacman: {e}")),
    }
}

fn build_package_list(
    build_dir: &Path,
    uid: u32,
    gid: u32,
    home: &str,
) -> Result<Vec<String>, String> {
    let mut cmd = Command::new("makepkg");
    cmd.current_dir(build_dir)
        .arg("--packagelist")
        .env("HOME", home)
        .env("PATH", BUILD_PATH);
    unsafe {
        cmd.pre_exec(move || {
            drop_to_user(uid, gid)?;
            return Ok(());
        });
    }
    let output = cmd.output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("makepkg --packagelist failed".to_string());
    }
    return Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect());
}

fn spawn_tty(
    cmd: &mut Command,
    tty: &[OwnedFd; 3],
    drop_privs: Option<(u32, u32)>,
) -> io::Result<ExitStatus> {
    let stdin = tty[0].try_clone()?;
    let stdout = tty[1].try_clone()?;
    let stderr = tty[2].try_clone()?;
    cmd.stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    unsafe {
        cmd.pre_exec(move || {
            if let Some((uid, gid)) = drop_privs {
                drop_to_user(uid, gid)?;
            }
            libc::setsid();
            libc::ioctl(0, libc::TIOCSCTTY as libc::c_ulong, 0);
            return Ok(());
        });
    }
    return cmd.status();
}

fn into_tty(fds: Vec<OwnedFd>) -> Option<[OwnedFd; 3]> {
    if fds.len() != 3 {
        return None;
    }
    let mut it = fds.into_iter();
    let stdin = it.next().unwrap();
    let stdout = it.next().unwrap();
    let stderr = it.next().unwrap();
    return Some([stdin, stdout, stderr]);
}

fn drop_to_user(uid: u32, gid: u32) -> io::Result<()> {
    setgroups(&[Gid::from_raw(gid)]).map_err(nix_to_io)?;
    setgid(Gid::from_raw(gid)).map_err(nix_to_io)?;
    setuid(Uid::from_raw(uid)).map_err(nix_to_io)?;
    return Ok(());
}

fn nix_to_io(e: nix::Error) -> io::Error {
    return io::Error::from_raw_os_error(e as i32);
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 256 {
        return Err(format!("invalid name: {name:?}"));
    }
    if name.starts_with('-') {
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
        if p.starts_with('-') {
            return Err(format!("invalid package path: {p}"));
        }
        let path = Path::new(p);
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(_) => return Err(format!("not a file: {p}")),
        };
        if !metadata.file_type().is_file() {
            return Err(format!("not a regular file: {p}"));
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.contains(".pkg.tar") {
            return Err(format!("not a package file: {p}"));
        }
    }
    return Ok(());
}
