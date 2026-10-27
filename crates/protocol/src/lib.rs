use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 65536;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRequest {
    pub version: u16,
    pub request_id: Uuid,
    pub operation: Operation,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Ping,
    InspectHost,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentResponse {
    pub version: u16,
    pub request_id: Uuid,
    pub result: AgentResult,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentResult {
    Pong,
    UnsupportedVersion,
    HostStatus(Box<manager_core::host::HostStatus>),
    Unavailable,
}
