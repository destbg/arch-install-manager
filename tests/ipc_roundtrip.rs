use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use arch_install_manager::ipc::protocol::{read_response, send_request};
use arch_install_manager::models::op::Op;
use arch_install_manager::models::request::Request;
use arch_install_manager::models::response::Response;

fn accept_with_deadline(listener: &UnixListener, deadline: Instant) -> UnixStream {
    listener.set_nonblocking(true).unwrap();
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                return stream;
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline {
                    panic!("daim-helper never connected back");
                }
                sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("accept failed: {e}"),
        }
    }
}

#[test]
fn helper_ipc_round_trip() {
    let dir = std::env::temp_dir().join(format!("daim-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("ctl.sock");
    let _ = std::fs::remove_file(&sock);

    let listener = UnixListener::bind(&sock).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_daim-helper"))
        .args(["--connect", sock.to_str().unwrap()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn daim-helper");

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = accept_with_deadline(&listener, deadline);

    send_request(
        &stream,
        &Request {
            op: Op::Install {
                targets: vec![],
                as_deps: false,
                reinstall: false,
            },
            with_tty: false,
        },
        &[],
    )
    .expect("send install");
    let err = read_response(&mut stream).expect("install response");
    assert!(
        matches!(err, Response::Error { .. }),
        "expected Error, got {err:?}"
    );

    drop(stream);
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
