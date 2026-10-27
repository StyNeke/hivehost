use manager_core::{
    capacity::Resources,
    eggs::{FileTransform, Installer},
    host::PortBinding,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

pub const VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 65536;
pub const MAX_CONSOLE_LINES: u16 = 500;
pub const MAX_COMMAND_BYTES: usize = 512;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRequest {
    pub version: u16,
    pub request_id: Uuid,
    pub operation: Operation,
}

// Everything the agent needs to install or run one server. The agent derives all host paths,
// container names and the game UID from server_id and its own registry; none are accepted here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerSpec {
    pub server_id: Uuid,
    pub image: String,
    pub startup: String,
    pub stop: String,
    pub ready_when: String,
    pub installer: Installer,
    pub variables: BTreeMap<String, String>,
    pub limits: Resources,
    pub binding: PortBinding,
    pub files: BTreeMap<String, FileTransform>,
    pub accept_eula: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionFolder {
    Mods,
    Plugins,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteFile {
    pub url: String,
    pub sha512: String,
    pub size: u64,
    pub filename: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Ping,
    InspectHost,
    InstallServer {
        spec: Box<ServerSpec>,
    },
    StartServer {
        spec: Box<ServerSpec>,
    },
    StopServer {
        server_id: Uuid,
    },
    KillServer {
        server_id: Uuid,
    },
    SendCommand {
        server_id: Uuid,
        command: String,
    },
    ServerStatus {
        server_id: Uuid,
    },
    ReadConsole {
        server_id: Uuid,
        lines: u16,
    },
    InstallFiles {
        server_id: Uuid,
        folder: ExtensionFolder,
        files: Vec<RemoteFile>,
        remove: Vec<String>,
    },
    ListFiles {
        server_id: Uuid,
        folder: ExtensionFolder,
    },
    DeleteServer {
        server_id: Uuid,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Install,
    Extensions,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationStatus {
    pub kind: OperationKind,
    pub state: OperationState,
    pub message: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerState {
    Offline,
    Starting,
    Running,
    Stopping,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedGame {
    pub version: String,
    pub loader: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeStatus {
    pub installed: bool,
    pub operation: Option<OperationStatus>,
    pub power: PowerState,
    pub desired_running: bool,
    // Set when the crash limit stopped automatic restarts, or a stop needed a forced kill.
    pub problem: Option<String>,
    pub started_at: Option<i64>,
    pub exit_code: Option<i64>,
    pub game: Option<DetectedGame>,
    pub disk_used_bytes: Option<u64>,
    pub disk_total_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejection {
    pub code: String,
    pub message: String,
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
    Accepted,
    Rejected(Rejection),
    ServerStatus(Box<RuntimeStatus>),
    Console(Vec<String>),
    Files(Vec<String>),
}
