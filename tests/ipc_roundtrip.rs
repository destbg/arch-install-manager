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

#[test]
fn managed_timeshift_snapshot_applies_retention_and_reports_failures() {
    use std::os::unix::fs::PermissionsExt;

    use arch_install_manager::models::snapshot_retention_period::SnapshotRetentionPeriod;
    use chrono::{Duration as Age, Local};

    let dir = std::env::temp_dir().join(format!("daim-snapshot-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mock = dir.join("timeshift");
    std::fs::write(
        &mock,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$MOCK_CALLS"
case "$1" in
    --create) [ "$MOCK_FAILURE" != create ] ;;
    --list)
        [ "$MOCK_FAILURE" != list ] || exit 1
        [ "$MOCK_FAILURE" != malformed ] || { printf 'unrecognized output\n'; exit 0; }
        printf '0 > %s  O    arch-install-manager\n' "$MOCK_OLD"
        printf '1 > %s  O    arch-update-manager\n' "$MOCK_FOREIGN"
        printf '2 > %s  O\n' "$MOCK_UNLABELED"
        printf '3 > %s  O W  arch-install-manager\n' "$MOCK_NEW"
        ;;
    --delete) [ "$MOCK_FAILURE" != delete ] ;;
    *) exit 99 ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&mock, std::fs::Permissions::from_mode(0o700)).unwrap();
    let old = (Local::now() - Age::days(10))
        .format("%Y-%m-%d_%H-%M-%S")
        .to_string();
    let newest = Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
    for (failure, count, period, success, deletes, creates) in [
        ("", 1, SnapshotRetentionPeriod::Forever, true, 1, 1),
        ("", 10, SnapshotRetentionPeriod::Day, true, 1, 1),
        ("", 10, SnapshotRetentionPeriod::Forever, true, 0, 1),
        ("create", 1, SnapshotRetentionPeriod::Forever, false, 0, 1),
        ("list", 1, SnapshotRetentionPeriod::Forever, false, 0, 1),
        (
            "malformed",
            1,
            SnapshotRetentionPeriod::Forever,
            false,
            0,
            1,
        ),
        ("delete", 1, SnapshotRetentionPeriod::Forever, false, 1, 1),
        ("", 0, SnapshotRetentionPeriod::Forever, false, 0, 0),
    ] {
        let sock = dir.join("ctl.sock");
        let calls = dir.join("calls");
        let _ = std::fs::remove_file(&sock);
        std::fs::write(&calls, "").unwrap();
        let listener = UnixListener::bind(&sock).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_daim-helper"))
            .args(["--connect", sock.to_str().unwrap()])
            .env("PATH", &dir)
            .env("MOCK_CALLS", &calls)
            .env("MOCK_FAILURE", failure)
            .env("MOCK_OLD", &old)
            .env("MOCK_NEW", &newest)
            .env("MOCK_FOREIGN", "2001-01-01_12-00-00")
            .env("MOCK_UNLABELED", "2002-01-01_12-00-00")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut stream = accept_with_deadline(&listener, Instant::now() + Duration::from_secs(5));
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        send_request(
            &stream,
            &Request {
                op: Op::SnapshotTimeshiftAndCleanup {
                    retention_count: count,
                    retention_period: period,
                },
                with_tty: false,
            },
            &[],
        )
        .unwrap();
        let response = read_response(&mut stream).unwrap();
        drop(stream);
        assert!(child.wait().unwrap().success());
        assert_eq!(response.is_success(), success, "{failure}: {response:?}");
        let commands = std::fs::read_to_string(&calls).unwrap();
        let deleted: Vec<_> = commands
            .lines()
            .filter(|line| line.starts_with("--delete"))
            .collect();
        assert_eq!(deleted.len(), deletes, "{failure}: {commands}");
        assert!(
            deleted
                .iter()
                .all(|line| *line == format!("--delete --snapshot {old} --yes --scripted"))
        );
        assert_eq!(
            commands
                .lines()
                .filter(|line| line.starts_with("--create"))
                .count(),
            creates
        );
        if failure == "create" {
            assert!(!commands.contains("--list"));
        }
        if failure == "delete" {
            assert!(
                matches!(response, Response::Error { message } if message.contains("cleanup failed"))
            );
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
