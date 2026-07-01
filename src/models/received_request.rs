use std::os::fd::OwnedFd;

use crate::models::request::Request;

pub struct ReceivedRequest {
    pub req: Request,
    pub fds: Vec<OwnedFd>,
}
