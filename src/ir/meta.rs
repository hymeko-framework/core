use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    pub created_at_unix_ns: i128,
    pub build_id: [u8; 16], // random/session ID
}

impl Meta {
    pub fn new(created_at_unix_ns: i128, build_id: [u8; 16]) -> Self {
        Self {
            created_at_unix_ns,
            build_id,
        }
    }
}
