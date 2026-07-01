use std::io;
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use arch_install_manager::ipc::protocol::{Op, Request, Response, read_response, send_request};

fn connect_with_retry(sock: &std::path::Path, deadline: Instant) -> UnixStream {
    loop {
        if let Ok(s) = UnixStream::connect(sock) {
            return s;
        }
        if Instant::now() > deadline {
            panic!("daim-helper never accepted connections");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn round_trip(sock: &std::path::Path, op: Op, with_tty: bool) -> io::Result<Response> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = connect_with_retry(sock, deadline);
    send_request(&stream, &Request { op, with_tty }, &[])?;
    return read_response(&mut stream);
}

#[test]
fn helper_ipc_round_trip() {
    let dir = std::env::temp_dir().join(format!("daim-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("helper.sock");

    let uid = unsafe { libc::geteuid() }.to_string();
    let gid = unsafe { libc::getegid() }.to_string();

    let mut child = Command::new(env!("CARGO_BIN_EXE_daim-helper"))
        .args([
            "--uid",
            &uid,
            "--gid",
            &gid,
            "--socket",
            sock.to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn daim-helper");

    let pong = round_trip(&sock, Op::Ping, false).expect("ping");
    assert!(
        matches!(pong, Response::Pong),
        "expected Pong, got {pong:?}"
    );

    let err = round_trip(
        &sock,
        Op::Install {
            targets: vec![],
            as_deps: false,
            reinstall: false,
        },
        false,
    )
    .expect("install");
    assert!(
        matches!(err, Response::Error { .. }),
        "expected Error, got {err:?}"
    );

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
