use std::io::{self, IoSlice, IoSliceMut, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use nix::sys::socket::{ControlMessage, ControlMessageOwned, MsgFlags, recvmsg, sendmsg};
use serde::{Deserialize, Serialize};

pub fn socket_dir_for_uid(uid: u32) -> PathBuf {
    return PathBuf::from(format!("/run/user/{uid}/daim"));
}

pub fn socket_path_for_uid(uid: u32) -> PathBuf {
    return socket_dir_for_uid(uid).join("helper.sock");
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum MirrorTool {
    RateMirrors,
    Reflector,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Op {
    Attach,
    Ping,
    Shutdown,
    SyncDb,
    SysUpgrade,
    SysUpgradeNoConfirm,
    Install {
        targets: Vec<String>,
        as_deps: bool,
        reinstall: bool,
    },
    InstallFiles {
        paths: Vec<String>,
        as_deps: bool,
    },
    RemoveMakeDeps {
        targets: Vec<String>,
    },
    Remove {
        targets: Vec<String>,
        cascade: bool,
        nosave: bool,
    },
    SetIgnorePkg {
        name: String,
        ignored: bool,
    },
    RemoveDbLock,
    PaccacheClean {
        keep: u32,
        keep_uninstalled: u32,
    },
    RestartService {
        name: String,
    },
    SnapshotTimeshift {
        comment: String,
    },
    SnapshotSnapper {
        description: String,
    },
    RefreshMirrors {
        tool: MirrorTool,
    },
    RunPacdiff,
}

impl Op {
    pub fn wants_tty(&self) -> bool {
        return matches!(
            self,
            Op::SysUpgrade
                | Op::Install { .. }
                | Op::InstallFiles { .. }
                | Op::Remove { .. }
                | Op::RefreshMirrors { .. }
                | Op::SnapshotSnapper { .. }
                | Op::RunPacdiff
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub op: Op,
    pub with_tty: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Done {
        exit_code: i32,
        stdout: String,
        stderr: String,
    },
    Error {
        message: String,
    },
    Pong,
}

impl Response {
    pub fn is_success(&self) -> bool {
        return matches!(self, Response::Done { exit_code: 0, .. } | Response::Pong);
    }

    pub fn error(message: impl Into<String>) -> Self {
        return Response::Error {
            message: message.into(),
        };
    }
}

pub fn send_request(stream: &UnixStream, req: &Request, fds: &[RawFd]) -> io::Result<()> {
    let body = serde_json::to_vec(req).map_err(io_err)?;
    let framed = frame(&body);
    let iov = [IoSlice::new(&framed)];
    let cmsgs: Vec<ControlMessage> = if fds.is_empty() {
        Vec::new()
    } else {
        vec![ControlMessage::ScmRights(fds)]
    };
    sendmsg::<()>(stream.as_raw_fd(), &iov, &cmsgs, MsgFlags::empty(), None).map_err(nix_err)?;
    return Ok(());
}

pub fn write_response(stream: &mut UnixStream, resp: &Response) -> io::Result<()> {
    let body = serde_json::to_vec(resp).map_err(io_err)?;
    stream.write_all(&frame(&body))?;
    return stream.flush();
}

pub fn read_response(stream: &mut UnixStream) -> io::Result<Response> {
    let body = read_frame(stream)?;
    return serde_json::from_slice(&body).map_err(io_err);
}

pub fn recv_request(stream: &UnixStream) -> io::Result<ReceivedRequest> {
    let mut buf = vec![0u8; 64 * 1024];
    let mut iov = [IoSliceMut::new(&mut buf)];
    let mut cmsg = nix::cmsg_space!([RawFd; 3]);
    let msg = recvmsg::<()>(
        stream.as_raw_fd(),
        &mut iov,
        Some(cmsg.as_mut_slice()),
        MsgFlags::empty(),
    )
    .map_err(nix_err)?;

    let mut fds = Vec::new();
    for cmsg in msg.cmsgs().map_err(nix_err)? {
        if let ControlMessageOwned::ScmRights(raw) = cmsg {
            for fd in raw {
                fds.push(unsafe { OwnedFd::from_raw_fd(fd) });
            }
        }
    }

    let nbytes = msg.bytes;
    if nbytes < 4 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "short request",
        ));
    }
    let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let end = (4 + len).min(nbytes);
    let req: Request = serde_json::from_slice(&buf[4..end]).map_err(io_err)?;
    return Ok(ReceivedRequest { req, fds });
}

fn io_err<E: std::fmt::Display>(e: E) -> io::Error {
    return io::Error::new(io::ErrorKind::Other, e.to_string());
}

fn nix_err(e: nix::Error) -> io::Error {
    return io::Error::from_raw_os_error(e as i32);
}

fn frame(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(body);
    return out;
}

pub struct ReceivedRequest {
    pub req: Request,
    pub fds: Vec<OwnedFd>,
}

fn read_frame(stream: &mut UnixStream) -> io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 16 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body)?;
    return Ok(body);
}
