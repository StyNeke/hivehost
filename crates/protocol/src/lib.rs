use manager_core::{
    capacity::Resources,
    eggs::{FileTransform, Installer},
    host::PortBinding,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

pub const VERSION: u16 = 1;
// Large enough for a base64-encoded 1 MiB file chunk; still bounded on both sides.
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
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
        // Keep the server's backups after deletion; they remain listable and downloadable.
        #[serde(default)]
        keep_backups: bool,
    },
    CreateBackup {
        server_id: Uuid,
        retention: u8,
    },
    ListBackups {
        server_id: Uuid,
    },
    RestoreBackup {
        server_id: Uuid,
        backup_id: Uuid,
    },
    DeleteBackup {
        server_id: Uuid,
        backup_id: Uuid,
    },
    ListDirectory {
        server_id: Uuid,
        path: String,
    },
    ReadText {
        server_id: Uuid,
        path: String,
    },
    WriteText {
        server_id: Uuid,
        path: String,
        text: String,
        // The version that was read; None creates a new file and never overwrites.
        expected_version: Option<String>,
    },
    ReadChunk {
        server_id: Uuid,
        path: String,
        offset: u64,
        length: u32,
    },
    WriteChunk {
        server_id: Uuid,
        upload_id: Uuid,
        path: String,
        offset: u64,
        // Base64-encoded bytes.
        data: String,
        last: bool,
    },
    CreateDirectory {
        server_id: Uuid,
        path: String,
    },
    RenamePath {
        server_id: Uuid,
        from: String,
        to: String,
    },
    DeletePath {
        server_id: Uuid,
        path: String,
    },
    UsageAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    File,
    Directory,
    Link,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub kind: FileKind,
    pub size: u64,
    pub modified: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupKind {
    Manual,
    // Made automatically before a restore so the replaced data can be recovered.
    PreRestore,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupInfo {
    pub id: Uuid,
    pub kind: BackupKind,
    pub created_at: i64,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Install,
    Extensions,
    Delete,
    Backup,
    Restore,
    // Preparing a changed runtime image before a start.
    Update,
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

// Latest measured usage of a running server. CPU is in millicores (1000 = one logical CPU).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerUsage {
    pub cpu_millis: u32,
    pub memory_bytes: u64,
    pub memory_limit_bytes: u64,
    pub network_rx_bytes_per_sec: u64,
    pub network_tx_bytes_per_sec: u64,
    pub measured_at: i64,
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
    #[serde(default)]
    pub usage: Option<ServerUsage>,
}

// Usage of every managed server for overview pages, including disk use of stopped servers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageSummary {
    pub usage: Option<ServerUsage>,
    pub disk_used_bytes: Option<u64>,
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
    Backups(Vec<BackupInfo>),
    Directory(Vec<FileEntry>),
    Text { text: String, version: String },
    Saved { version: String },
    Chunk { data: String, size: u64 },
    Usage(std::collections::BTreeMap<Uuid, UsageSummary>),
}
