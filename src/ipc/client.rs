use std::io;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::ipc::protocol::{
    Op, Request, Response, read_response, send_request, socket_path_for_uid,
};

static LAUNCH_LOCK: Mutex<()> = Mutex::new(());

static LAUNCHER: Mutex<Option<String>> = Mutex::new(None);

pub struct HelperHandle {
    _stream: UnixStream,
}

static SESSION: Mutex<Option<HelperHandle>> = Mutex::new(None);

pub fn set_launcher(launcher: &str) {
    *LAUNCHER.lock().unwrap() = Some(launcher.to_string());
}

pub fn ensure_running() -> io::Result<()> {
    if ping() {
        return Ok(());
    }
    let _guard = LAUNCH_LOCK.lock().unwrap();
    if ping() {
        return Ok(());
    }

    let launcher = resolve_launcher();
    let helper_bin =
        std::env::var("DAIM_HELPER_BIN").unwrap_or_else(|_| "/usr/bin/daim-helper".to_string());

    let uid = current_uid().to_string();
    let gid = current_gid().to_string();
    let sock = socket_path();
    let sock = sock.to_string_lossy();
    let helper_args = [
        "--uid",
        uid.as_str(),
        "--gid",
        gid.as_str(),
        "--socket",
        sock.as_ref(),
    ];

    let mut command = if launcher.is_empty() {
        let mut c = Command::new(&helper_bin);
        c.args(helper_args);
        c
    } else {
        let mut c = Command::new(&launcher);
        c.arg(&helper_bin).args(helper_args);
        c
    };

    let mut child = command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if ping() {
            return Ok(());
        }
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
        std::thread::sleep(Duration::from_millis(200));
    }
}

pub fn attach() -> io::Result<HelperHandle> {
    ensure_running()?;
    let stream = connect()?;
    send_request(&stream, &request(Op::Attach), &[])?;
    return Ok(HelperHandle { _stream: stream });
}

pub fn call(op: Op) -> io::Result<Response> {
    ensure_running()?;
    let mut stream = connect()?;
    send_request(&stream, &request(op), &[])?;
    return read_response(&mut stream);
}

pub fn call_with_tty(op: Op) -> io::Result<Response> {
    ensure_running()?;
    let mut stream = connect()?;
    send_request(&stream, &request(op), &[0, 1, 2])?;
    return read_response(&mut stream);
}

pub fn set_ignore_pkg(name: &str, ignored: bool) -> io::Result<Response> {
    return call(Op::SetIgnorePkg {
        name: name.to_string(),
        ignored,
    });
}

pub fn attach_session() -> io::Result<()> {
    let mut session = SESSION.lock().unwrap();
    if session.is_some() {
        return Ok(());
    }
    *session = Some(attach()?);
    return Ok(());
}

fn current_uid() -> u32 {
    return unsafe { libc::geteuid() };
}

fn current_gid() -> u32 {
    return unsafe { libc::getegid() };
}

fn socket_path() -> PathBuf {
    if let Ok(p) = std::env::var("DAIM_HELPER_SOCKET") {
        return PathBuf::from(p);
    }
    return socket_path_for_uid(current_uid());
}

fn connect() -> io::Result<UnixStream> {
    return UnixStream::connect(socket_path());
}

fn ping() -> bool {
    let Ok(stream) = connect() else {
        return false;
    };
    if send_request(&stream, &request(Op::Ping), &[]).is_err() {
        return false;
    }
    let mut stream = stream;
    return matches!(read_response(&mut stream), Ok(Response::Pong));
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
