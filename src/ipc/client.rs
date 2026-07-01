use std::fs::{File, Permissions};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::thread::sleep;
use std::time::{Duration, Instant};

use nix::sys::socket::getsockopt;
use nix::sys::socket::sockopt::PeerCredentials;

use crate::ipc::protocol::{read_response, send_request, socket_dir_for_uid};
use crate::models::op::Op;
use crate::models::request::Request;
use crate::models::response::Response;

const HELPER_ENV_FD: &str = "DAIM_HELPER_FD";
const START_TIMEOUT: Duration = Duration::from_secs(120);

static PRIMARY: Mutex<Option<UnixStream>> = Mutex::new(None);

static LAUNCHER: Mutex<Option<String>> = Mutex::new(None);

static HELPER_CHILD: Mutex<Option<Child>> = Mutex::new(None);

pub fn set_launcher(launcher: &str) {
    *LAUNCHER.lock().unwrap() = Some(launcher.to_string());
}

pub fn ensure_running() -> io::Result<()> {
    return with_primary(|_| Ok(()));
}

pub fn attach_session() -> io::Result<()> {
    return ensure_running();
}

pub fn call(op: Op) -> io::Result<Response> {
    return with_primary(|stream| {
        send_request(stream, &request(op), &[])?;
        return read_response(stream);
    });
}

pub fn call_with_tty(op: Op) -> io::Result<Response> {
    return with_primary(|stream| {
        send_request(stream, &request(op), &[0, 1, 2])?;
        return read_response(stream);
    });
}

pub fn set_ignore_pkg(name: &str, ignored: bool) -> io::Result<Response> {
    return call(Op::SetIgnorePkg {
        name: name.to_string(),
        ignored,
    });
}

pub fn mint_terminal_session() -> io::Result<OwnedFd> {
    let (local, remote) = UnixStream::pair()?;
    let resp = with_primary(|stream| {
        send_request(stream, &request(Op::NewSession), &[remote.as_raw_fd()])?;
        return read_response(stream);
    })?;
    drop(remote);
    if !resp.is_success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "the helper refused a new session",
        ));
    }
    return Ok(OwnedFd::from(local));
}

fn with_primary<T>(f: impl FnOnce(&mut UnixStream) -> io::Result<T>) -> io::Result<T> {
    let mut guard = PRIMARY.lock().unwrap();
    if guard.is_none() {
        *guard = Some(establish()?);
    }
    let stream = guard.as_mut().unwrap();
    return f(stream);
}

fn establish() -> io::Result<UnixStream> {
    harden()?;
    if let Some(fd) = inherited_helper_fd() {
        set_cloexec(fd);
        return Ok(unsafe { UnixStream::from_raw_fd(fd) });
    }
    return connect_back_launch();
}

fn harden() -> io::Result<()> {
    set_non_dumpable();
    return ensure_not_traced();
}

fn set_non_dumpable() {
    unsafe {
        libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
    }
}

fn ensure_not_traced() -> io::Result<()> {
    let status = match std::fs::read_to_string("/proc/self/status") {
        Ok(s) => s,
        Err(_) => return Ok(()),
    };
    for line in status.lines() {
        let Some(rest) = line.strip_prefix("TracerPid:") else {
            continue;
        };
        let pid: i64 = rest.trim().parse().unwrap_or(0);
        if pid != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "refusing to get admin rights while another process is tracing this app",
            ));
        }
        return Ok(());
    }
    return Ok(());
}

fn inherited_helper_fd() -> Option<RawFd> {
    let value = std::env::var(HELPER_ENV_FD).ok()?;
    return value.trim().parse::<RawFd>().ok();
}

fn set_cloexec(fd: RawFd) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        }
    }
}

fn connect_back_launch() -> io::Result<UnixStream> {
    let dir = socket_dir_for_uid(current_uid());
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, Permissions::from_mode(0o700))?;

    let path = dir.join(control_socket_name());
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, Permissions::from_mode(0o600))?;

    let child = spawn_helper(&path)?;
    let result = accept_root_peer(&listener, child);
    let _ = std::fs::remove_file(&path);
    return result;
}

fn accept_root_peer(listener: &UnixListener, mut child: Child) -> io::Result<UnixStream> {
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if peer_uid(&stream) == Some(0) {
                    stream.set_nonblocking(false)?;
                    *HELPER_CHILD.lock().unwrap() = Some(child);
                    return Ok(stream);
                }
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                if let Ok(Some(status)) = child.try_wait() {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!("authorization for daim-helper failed (exit {status})"),
                    ));
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "timed out waiting for daim-helper to start",
                    ));
                }
                sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e),
        }
    }
}

fn spawn_helper(path: &Path) -> io::Result<Child> {
    let launcher = resolve_launcher();
    let helper_bin =
        std::env::var("DAIM_HELPER_BIN").unwrap_or_else(|_| "/usr/bin/daim-helper".to_string());
    let path_str = path.to_string_lossy();
    let helper_args = ["--connect", path_str.as_ref()];

    let mut command = if launcher.is_empty() {
        let mut c = Command::new(&helper_bin);
        c.args(helper_args);
        c
    } else {
        let mut c = Command::new(&launcher);
        c.arg(&helper_bin).args(helper_args);
        c
    };

    return command.stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

fn peer_uid(stream: &UnixStream) -> Option<u32> {
    return getsockopt(stream, PeerCredentials).ok().map(|c| c.uid());
}

fn control_socket_name() -> String {
    let pid = std::process::id();
    let mut bytes = [0u8; 8];
    if let Ok(mut file) = File::open("/dev/urandom") {
        let _ = file.read_exact(&mut bytes);
    }
    let mut suffix = String::with_capacity(16);
    for b in bytes {
        suffix.push_str(&format!("{b:02x}"));
    }
    return format!("ctl-{pid}-{suffix}.sock");
}

fn current_uid() -> u32 {
    return unsafe { libc::geteuid() };
}

fn request(op: Op) -> Request {
    let with_tty = op.wants_tty();
    return Request { op, with_tty };
}

fn resolve_launcher() -> String {
    if let Ok(env) = std::env::var("DAIM_HELPER_LAUNCHER") {
        return env;
    }
    if let Some(launcher) = LAUNCHER.lock().unwrap().clone() {
        return launcher;
    }
    return "pkexec".to_string();
}
