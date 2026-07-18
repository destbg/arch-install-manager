use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread::{sleep, spawn};
use std::time::Duration;

use nix::sys::socket::getsockopt;
use nix::sys::socket::sockopt::PeerCredentials;
use nix::unistd::{Gid, Uid, User, setgid, setgroups, setuid};

use crate::helpers::database_lock::remove_database_lock;
use crate::helpers::package_updates::SYNC_STAMP_FILE;
use crate::helpers::pacman_ignore::{add_to_ignore_pkg, remove_from_ignore_pkg};
use crate::ipc::protocol::{recv_request, write_response};
use crate::models::mirror_tool::MirrorTool;
use crate::models::op::Op;
use crate::models::request::Request;
use crate::models::response::Response;

const BUILD_USER: &str = "daim-build";
const AUR_CLONE_SUBDIR: &str = ".cache/daim/aur";
const BUILD_ROOT: &str = "/var/lib/daim/build";
const BUILD_HOME: &str = "/var/lib/daim/home";
const BUILD_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin";
const CONNECT_ATTEMPTS: u32 = 30;
const CHECK_DB_MAX_AGE: Duration = Duration::from_secs(30 * 60);

pub struct Config {
    pub connect_path: PathBuf,
}

pub fn run(config: Config) -> io::Result<()> {
    set_non_dumpable();

    let stream = connect_back(&config.connect_path)?;
    let authorized_uid = peer_uid(&stream).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "could not read the identity of the requesting process",
        )
    })?;

    serve(stream, authorized_uid);
    return Ok(());
}

fn set_non_dumpable() {
    unsafe {
        libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
    }
}

fn connect_back(path: &Path) -> io::Result<UnixStream> {
    let mut last_err = io::Error::new(io::ErrorKind::NotFound, "control socket not found");
    for _ in 0..CONNECT_ATTEMPTS {
        match UnixStream::connect(path) {
            Ok(stream) => return Ok(stream),
            Err(e) => {
                last_err = e;
                sleep(Duration::from_millis(100));
            }
        }
    }
    return Err(last_err);
}

fn peer_uid(stream: &UnixStream) -> Option<u32> {
    return getsockopt(stream, PeerCredentials).ok().map(|c| c.uid());
}

fn serve(stream: UnixStream, authorized_uid: u32) {
    let mut stream = stream;
    loop {
        let received = match recv_request(&stream) {
            Ok(r) => r,
            Err(_) => return,
        };
        let resp = match received.req.op {
            Op::NewSession => spawn_session(received.fds, authorized_uid),
            _ => execute(&received.req, received.fds, authorized_uid),
        };
        if write_response(&mut stream, &resp).is_err() {
            return;
        }
    }
}

fn spawn_session(mut fds: Vec<OwnedFd>, authorized_uid: u32) -> Response {
    let Some(fd) = fds.pop() else {
        return Response::error("a new session needs a socket");
    };
    let session = UnixStream::from(fd);
    spawn(move || serve(session, authorized_uid));
    return done_ok();
}

fn execute(req: &Request, fds: Vec<OwnedFd>, uid: u32) -> Response {
    if op_uses_pacman_db(&req.op) {
        clear_stale_lock();
    }
    match &req.op {
        Op::SyncDb => sync_databases(uid),
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
        Op::SnapshotSnapper { description } => run_capture(
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
        ),
        Op::RefreshMirrors { tool } => run_refresh_mirrors(*tool, fds),
        Op::RunPacdiff => run_tty_or_err("pacdiff", &[], &[("DIFFPROG", "")], fds),
        Op::NewSession => Response::error("a new session is handled by the server loop"),
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

fn op_uses_pacman_db(op: &Op) -> bool {
    return matches!(
        op,
        Op::SyncDb
            | Op::SysUpgrade
            | Op::SysUpgradeNoConfirm
            | Op::Install { .. }
            | Op::InstallFiles { .. }
            | Op::AurBuildInstall { .. }
            | Op::RemoveMakeDeps { .. }
            | Op::Remove { .. }
    );
}

fn clear_stale_lock() {
    let lock = "/var/lib/pacman/db.lck";
    if !Path::new(lock).exists() {
        return;
    }
    let pacman_running = Command::new("pgrep")
        .args(["-x", "pacman"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(true);
    if pacman_running {
        return;
    }
    let _ = std::fs::remove_file(lock);
}

fn sync_databases(uid: u32) -> Response {
    if adopt_check_db(uid) {
        return done_ok();
    }
    return run_capture("pacman", &["-Sy"]);
}

fn adopt_check_db(uid: u32) -> bool {
    let sync_dir = std::env::temp_dir()
        .join(format!("daim-checkup-db-{uid}"))
        .join("sync");

    let Ok(dir_meta) = std::fs::symlink_metadata(&sync_dir) else {
        return false;
    };
    if !dir_meta.is_dir() || dir_meta.uid() != uid {
        return false;
    }

    let stamp = sync_dir.join(SYNC_STAMP_FILE);
    let Ok(stamp_meta) = std::fs::symlink_metadata(&stamp) else {
        return false;
    };
    if !stamp_meta.file_type().is_file() || stamp_meta.uid() != uid {
        return false;
    }
    let fresh = stamp_meta
        .modified()
        .ok()
        .and_then(|mtime| mtime.elapsed().ok())
        .map(|age| age <= CHECK_DB_MAX_AGE)
        .unwrap_or(false);
    if !fresh {
        return false;
    }

    let dest_dir = Path::new("/var/lib/pacman/sync");
    let Ok(entries) = std::fs::read_dir(&sync_dir) else {
        return false;
    };
    let mut copied = false;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.ends_with(".db") && !name.ends_with(".db.sig") {
            continue;
        }
        if copy_db_file(&entry.path(), &dest_dir.join(name), uid).is_ok() {
            copied = true;
        }
    }
    return copied;
}

fn copy_db_file(src: &Path, dest: &Path, uid: u32) -> io::Result<()> {
    let mut open = std::fs::OpenOptions::new();
    open.read(true).custom_flags(libc::O_NOFOLLOW);
    let mut from = open.open(src)?;

    let meta = from.metadata()?;
    if !meta.is_file() || meta.uid() != uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "untrusted database file",
        ));
    }

    let mut to = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(dest)?;
    io::copy(&mut from, &mut to)?;
    return Ok(());
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

fn ensure_build_home(builder: &User) -> Result<String, String> {
    let home = Path::new(BUILD_HOME);
    std::fs::create_dir_all(home).map_err(|e| e.to_string())?;
    std::fs::set_permissions(home, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())?;

    let spec = format!("{}:{}", builder.uid.as_raw(), builder.gid.as_raw());
    let owned = Command::new("chown")
        .arg(&spec)
        .arg(home)
        .status()
        .map_err(|e| e.to_string())?;
    if !owned.success() {
        return Err("failed to set build home ownership".to_string());
    }
    return Ok(home.to_string_lossy().to_string());
}

fn run_build_and_install(
    build_dir: &Path,
    builder: &User,
    as_deps: bool,
    tty: &[OwnedFd; 3],
) -> Response {
    let uid = builder.uid.as_raw();
    let gid = builder.gid.as_raw();
    let home = match ensure_build_home(builder) {
        Ok(home) => home,
        Err(e) => return Response::error(e),
    };

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
