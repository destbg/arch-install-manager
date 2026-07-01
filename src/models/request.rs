use serde::{Deserialize, Serialize};

use crate::models::op::Op;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub op: Op,
    pub with_tty: bool,
}
