use serde::{Deserialize, Serialize};

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
}

impl Response {
    pub fn is_success(&self) -> bool {
        return matches!(self, Response::Done { exit_code: 0, .. });
    }

    pub fn error(message: impl Into<String>) -> Self {
        return Response::Error {
            message: message.into(),
        };
    }
}
