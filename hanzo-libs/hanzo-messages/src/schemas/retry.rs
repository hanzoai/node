use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

use crate::hanzo_message::hanzo_message::HanzoMessage;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RetryMessage {
    pub retry_count: u32,
    pub message: HanzoMessage,
    pub save_to_db_flag: bool,
    pub peer: (SocketAddr, String),
}
