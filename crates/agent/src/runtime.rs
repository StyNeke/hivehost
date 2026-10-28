// The game runner. The agent owns a registry of servers it created; every container name, UID,
// host path and mount is derived from that registry and the server's UUID, never from requests.
use crate::{
    commands::run_checked,
    engine,
    files::{self, as_server},
    host::AgentConfig,
    storage::{self, Layout},
};
use manager_core::{
    capacity::Resources,
    host::{PortBinding, Transport, private_allocation_ip},
    lifecycle::{CrashDecision, on_unexpected_exit},
};
use manager_protocol::{
    AgentResult, DetectedGame, ExtensionFolder, MAX_COMMAND_BYTES, MAX_CONSOLE_LINES,
    OperationKind, OperationState, OperationStatus, PowerState, Rejection, RemoteFile,
    RuntimeStatus, ServerSpec,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;
use uuid::Uuid;

#[path = "runtime_backups.rs"]
mod backup_operations;
#[path = "runtime_files.rs"]
mod file_operations;

pub const SERVER_LABEL: &str = "dev.game-manager.server";
pub const INSTALL_LABEL: &str = "dev.game-manager.install";
const INSTALL_TIMEOUT_SECS: u64 = 30 * 60;
const PULL_TIMEOUT_SECS: u64 = 20 * 60;
const MAX_EXTENSION_BYTES: u64 = 128 * 1024 * 1024;
const MAX_EXTENSION_TOTAL: u64 = 256 * 1024 * 1024;
const RESERVED_ENV: &[&str] = &[
    "SERVER_MEMORY",
    "SERVER_IP",
    "SERVER_PORT",
    "STARTUP",
    "P_SERVER_UUID",
    "HOME",
    "TZ",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "JAVA_TOOL_OPTIONS",
];

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn rejected(code: &str, message: impl Into<String>) -> AgentResult {
    AgentResult::Rejected(Rejection {
        code: code.into(),
        message: message.into(),
    })
}

pub fn runtime_name(id: Uuid) -> String {
    format!("gm-{id}")
}

pub fn install_name(id: Uuid) -> String {
    format!("gm-install-{id}")
}

fn valid_image(image: &str) -> bool {
    !image.is_empty()
        && image.len() <= 512
        && !image.starts_with('-')
        && image
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/_.:@-".contains(&b))
}

// Validates everything that reaches a docker argument or container environment.
pub fn validate_spec(spec: &ServerSpec) -> Result<(), &'static str> {
    if !valid_image(&spec.image) || !valid_image(&spec.installer.container) {
        return Err("invalid image reference");
    }
    let entrypoint = &spec.installer.entrypoint;
    if entrypoint.is_empty()
        || entrypoint.starts_with('-')
        || !entrypoint
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/_-.".contains(&b))
    {
        return Err("invalid installer entrypoint");
    }
    if spec.installer.script.len() > 256 * 1024 || spec.installer.script.contains('\0') {
        return Err("invalid installer script");
    }
    if spec.startup.is_empty()
        || spec.startup.len() > 4096
        || spec.startup.contains(['\0', '\n', '\r'])
    {
        return Err("invalid startup command");
    }
    if spec.stop.is_empty()
        || spec.stop.len() > MAX_COMMAND_BYTES
        || spec.stop.contains(['\0', '\n', '\r'])
    {
        return Err("invalid stop command");
    }
    if spec.ready_when.is_empty() || spec.ready_when.len() > 512 {
        return Err("invalid readiness marker");
    }
    if spec.limits.memory_bytes < 256 * 1024 * 1024
        || spec.limits.disk_bytes < 64 * 1024 * 1024
        || spec.limits.cpu_millis < 10
        || spec.limits.memory_bytes > i64::MAX as u64
        || spec.limits.disk_bytes > i64::MAX as u64
    {
        return Err("resource limits are outside the supported range");
    }
    if !private_allocation_ip(spec.binding.ip) || spec.binding.port == 0 {
        return Err("allocation must be a private address and port");
    }
    if spec.variables.len() > 128 {
        return Err("too many variables");
    }
    for (name, value) in &spec.variables {
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            || name.as_bytes()[0].is_ascii_digit()
            || RESERVED_ENV.contains(&name.as_str())
            || value.len() > 4096
            || value.contains('\0')
        {
            return Err("invalid variable");
        }
    }
    for (path, transform) in &spec.files {
        if transform.parser != "properties" || files::resolve(Path::new("/"), path).is_err() {
            return Err("unsupported configuration file transform");
        }
    }
    Ok(())
}

// Java gets most of the container limit; the remainder covers metaspace, threads and buffers.
pub fn server_memory_mib(limit_bytes: u64) -> u64 {
    let mib = limit_bytes / (1024 * 1024);
    (mib * 85 / 100).max(mib.saturating_sub(1024)).max(128)
}

// Optional egress settings for hosts whose outbound traffic goes through a proxy, possibly one
// that inspects TLS with its own CA. Trust material is mounted read-only; nothing is by default.
#[derive(Debug, Clone, Copy, Default)]
pub struct Egress<'a> {
    pub proxy: Option<&'a (String, u16)>,
    pub ca_file: Option<&'a Path>,
    pub java_truststore: Option<&'a Path>,
}

const CA_TARGET: &str = "/etc/game-manager/ca.pem";
const TRUSTSTORE_TARGET: &str = "/etc/game-manager/cacerts";

pub fn environment(spec: &ServerSpec, egress: &Egress) -> Vec<(String, String)> {
    let mut environment: Vec<(String, String)> = spec
        .variables
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    environment.extend([
        ("STARTUP".into(), spec.startup.clone()),
        (
            "SERVER_MEMORY".into(),
            server_memory_mib(spec.limits.memory_bytes).to_string(),
        ),
        ("SERVER_IP".into(), "0.0.0.0".into()),
        ("SERVER_PORT".into(), spec.binding.port.to_string()),
        ("P_SERVER_UUID".into(), spec.server_id.to_string()),
        ("HOME".into(), "/home/container".into()),
        ("TZ".into(), "UTC".into()),
    ]);
    let mut java = Vec::new();
    if let Some((host, port)) = egress.proxy {
        let url = format!("http://{host}:{port}");
        for name in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
            environment.push((name.into(), url.clone()));
        }
        environment.push(("NO_PROXY".into(), "localhost,127.0.0.1".into()));
        environment.push(("no_proxy".into(), "localhost,127.0.0.1".into()));
        java.push(format!(
            "-Dhttp.proxyHost={host} -Dhttp.proxyPort={port} -Dhttps.proxyHost={host} -Dhttps.proxyPort={port}"
        ));
    }
    if egress.ca_file.is_some() {
        for name in ["SSL_CERT_FILE", "CURL_CA_BUNDLE", "REQUESTS_CA_BUNDLE"] {
            environment.push((name.into(), CA_TARGET.into()));
        }
    }
    if egress.java_truststore.is_some() {
        java.push(format!("-Djavax.net.ssl.trustStore={TRUSTSTORE_TARGET}"));
    }
    if !java.is_empty() {
        environment.push(("JAVA_TOOL_OPTIONS".into(), java.join(" ")));
    }
    environment
}

fn trust_mounts(egress: &Egress) -> Vec<OsString> {
    let mut args = Vec::new();
    for (source, target) in [
        (egress.ca_file, CA_TARGET),
        (egress.java_truststore, TRUSTSTORE_TARGET),
    ] {
        if let Some(source) = source {
            args.push("--mount".into());
            args.push(
                format!(
                    "type=bind,source={},target={target},readonly",
                    source.display()
                )
                .into(),
            );
        }
    }
    args
}

fn cpus(millis: u32) -> String {
    format!("{}.{:03}", millis / 1000, millis % 1000)
}

fn limit_args(limits: &Resources, pids: u32) -> Vec<OsString> {
    vec![
        "--memory".into(),
        limits.memory_bytes.to_string().into(),
        "--memory-swap".into(),
        limits.memory_bytes.to_string().into(),
        "--cpus".into(),
        cpus(limits.cpu_millis).into(),
        "--pids-limit".into(),
        pids.to_string().into(),
        "--security-opt".into(),
        "no-new-privileges".into(),
        "--log-driver".into(),
        "json-file".into(),
        "--log-opt".into(),
        "max-size=10m".into(),
        "--log-opt".into(),
        "max-file=3".into(),
    ]
}

fn env_args(environment: &[(String, String)]) -> Vec<OsString> {
    environment
        .iter()
        .flat_map(|(k, v)| ["--env".into(), OsString::from(format!("{k}={v}"))])
        .collect()
}

pub struct ContainerPlan<'a> {
    pub spec: &'a ServerSpec,
    pub uid: u32,
    pub image: &'a str,
    pub volume: &'a Path,
    pub network: &'a str,
    pub egress: Egress<'a>,
    pub stop_grace_secs: u64,
}

// Arguments for `docker run --detach` of the game container. Non-root, no capabilities, own volume only.
pub fn runtime_args(plan: &ContainerPlan) -> Vec<OsString> {
    let id = plan.spec.server_id;
    let binding = &plan.spec.binding;
    let transport = match binding.transport {
        Transport::Tcp => "tcp",
        Transport::Udp => "udp",
    };
    let host_ip = match binding.ip {
        std::net::IpAddr::V6(ip) => format!("[{ip}]"),
        ip => ip.to_string(),
    };
    let mut args: Vec<OsString> = vec![
        // `run --detach` keeps StdinOnce=false; `create -i` would close stdin after the first
        // console command disconnects.
        "run".into(),
        "--detach".into(),
        "--name".into(),
        runtime_name(id).into(),
        "--label".into(),
        format!("{SERVER_LABEL}={id}").into(),
        "--user".into(),
        format!("{0}:{0}", plan.uid).into(),
        "--network".into(),
        plan.network.into(),
        "--publish".into(),
        format!("{host_ip}:{0}:{0}/{transport}", binding.port).into(),
        "--cap-drop".into(),
        "ALL".into(),
        "--restart".into(),
        "no".into(),
        "--interactive".into(),
        "--read-only".into(),
        "--tmpfs".into(),
        "/tmp:rw,exec,nosuid,nodev,size=268435456".into(),
        "--stop-timeout".into(),
        plan.stop_grace_secs.to_string().into(),
        "--mount".into(),
        format!(
            "type=bind,source={},target=/home/container",
            plan.volume.display()
        )
        .into(),
        "--workdir".into(),
        "/home/container".into(),
    ];
    args.extend(limit_args(&plan.spec.limits, 512));
    args.extend(trust_mounts(&plan.egress));
    args.extend(env_args(&environment(plan.spec, &plan.egress)));
    args.push("--".into());
    args.push(plan.image.into());
    args
}

// Installers run as root inside their container (package managers need it) with a minimal
// capability set, the server's volume and a read-only script directory, and nothing else.
pub fn installer_args(plan: &ContainerPlan, scripts: &Path) -> Vec<OsString> {
    let id = plan.spec.server_id;
    let mut args: Vec<OsString> = vec![
        "create".into(),
        "--name".into(),
        install_name(id).into(),
        "--label".into(),
        format!("{INSTALL_LABEL}={id}").into(),
        "--network".into(),
        plan.network.into(),
        "--cap-drop".into(),
        "ALL".into(),
    ];
    for capability in [
        "CHOWN",
        "DAC_OVERRIDE",
        "FOWNER",
        "FSETID",
        "SETUID",
        "SETGID",
    ] {
        args.push("--cap-add".into());
        args.push(capability.into());
    }
    args.extend([
        "--restart".into(),
        "no".into(),
        "--mount".into(),
        format!(
            "type=bind,source={},target=/mnt/server",
            plan.volume.display()
        )
        .into(),
        "--mount".into(),
        format!(
            "type=bind,source={},target=/mnt/install,readonly",
            scripts.display()
        )
        .into(),
        "--workdir".into(),
        "/mnt/server".into(),
        "--entrypoint".into(),
        plan.spec.installer.entrypoint.clone().into(),
    ]);
    args.extend(limit_args(&plan.spec.limits, 1024));
    args.extend(trust_mounts(&plan.egress));
    args.extend(env_args(&environment(plan.spec, &plan.egress)));
    args.push("--".into());
    args.push(plan.spec.installer.container.clone().into());
    args.push("/mnt/install/install.sh".into());
    args
}

// Parses Docker's RFC 3339 UTC timestamps ("2026-10-27T22:51:03.123456789Z") to Unix seconds.
pub fn parse_docker_time(value: &str) -> Option<i64> {
    let value = value.strip_suffix('Z')?;
    let (date, time) = value.split_once('T')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    let time = time.split('.').next()?;
    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next()?.parse().ok()?;
    if year < 1970
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    // Days from civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hour * 3600 + minute * 60 + second)
}

pub fn valid_command(command: &str) -> bool {
    !command.trim().is_empty()
        && command.len() <= MAX_COMMAND_BYTES
        && !command.chars().any(char::is_control)
}

pub fn valid_extension_name(name: &str) -> bool {
    name.len() <= 128
        && name.ends_with(".jar")
        && name.len() > 4
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
}

pub fn valid_remote_file(file: &RemoteFile) -> bool {
    file.url.starts_with("https://cdn.modrinth.com/data/")
        && file.url.len() <= 1024
        && !file.url.contains(['\0', ' ', '\n', '\r', '#', '\\'])
        && !file.url["https://cdn.modrinth.com/".len()..].contains("..")
        && file.sha512.len() == 128
        && file.sha512.bytes().all(|b| b.is_ascii_hexdigit())
        && file.size > 0
        && file.size <= MAX_EXTENSION_BYTES
        && valid_extension_name(&file.filename)
}

fn folder_name(folder: ExtensionFolder) -> &'static str {
    match folder {
        ExtensionFolder::Mods => "mods",
        ExtensionFolder::Plugins => "plugins",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    spec: ServerSpec,
    uid: u32,
    image: Option<String>,
    installed: bool,
    desired_running: bool,
    stop_requested_at: Option<i64>,
    restarts: Vec<i64>,
    pending_restart_at: Option<i64>,
    problem: Option<String>,
    operation: Option<OperationStatus>,
    game: Option<DetectedGame>,
}

impl Record {
    fn busy(&self) -> bool {
        self.operation
            .as_ref()
            .is_some_and(|op| op.state == OperationState::Running)
    }
}

#[derive(Debug, Clone, Default)]
struct ContainerState {
    exists: bool,
    running: bool,
    exit_code: Option<i64>,
    started_at: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerState {
    running: bool,
    exit_code: i64,
    started_at: String,
}

pub struct Runtime {
    pub config: AgentConfig,
    layout: Layout,
    records: Mutex<BTreeMap<Uuid, Record>>,
    // Started-at timestamp for which the readiness marker has been seen.
    ready: std::sync::Mutex<HashMap<Uuid, i64>>,
}

impl Runtime {
    pub fn new(config: AgentConfig) -> Self {
        let layout = Layout {
            data_root: config.data_root.clone(),
        };
        Self {
            config,
            layout,
            records: Mutex::new(BTreeMap::new()),
            ready: std::sync::Mutex::new(HashMap::new()),
        }
    }

    // Loads the registry. Records left mid-operation by a crash become failed operations.
    pub async fn load(&self) -> io::Result<()> {
        self.layout.prepare()?;
        let mut records = self.records.lock().await;
        for entry in std::fs::read_dir(self.layout.registry())? {
            let path = entry?.path();
            let Some(id) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.parse::<Uuid>().ok())
            else {
                continue;
            };
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let mut record: Record =
                serde_json::from_slice(&std::fs::read(&path)?).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "corrupt registry record")
                })?;
            if record.spec.server_id != id {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "registry record id mismatch",
                ));
            }
            if let Some(op) = record
                .operation
                .as_mut()
                .filter(|op| op.state == OperationState::Running)
            {
                op.state = OperationState::Failed;
                op.message = "interrupted by an agent restart; retry the operation".into();
                op.updated_at = now();
            }
            records.insert(id, record);
        }
        for (id, record) in records.iter() {
            persist(&self.layout, record)?;
            if self.layout.image(*id).exists() {
                storage::ensure_mounted(&self.layout, *id).await?;
            }
        }
        drop(records);
        self.clean_partial_backups()?;
        self.recover_restores().await?;
        self.clean_uploads().await;
        Ok(())
    }

    pub async fn owned_ids(&self) -> std::collections::HashSet<Uuid> {
        self.records.lock().await.keys().copied().collect()
    }

    pub async fn owned_image_bytes(&self) -> u64 {
        let ids: Vec<Uuid> = self.records.lock().await.keys().copied().collect();
        ids.iter()
            .filter_map(|id| std::fs::metadata(self.layout.image(*id)).ok())
            .map(|meta| meta.len())
            .fold(0u64, u64::saturating_add)
    }

    fn egress(&self) -> Egress<'_> {
        Egress {
            proxy: self.config.container_proxy.as_ref(),
            ca_file: self.config.container_ca_file.as_deref(),
            java_truststore: self.config.container_java_truststore.as_deref(),
        }
    }

    async fn docker(
        &self,
        arguments: Vec<OsString>,
        seconds: u64,
        limit: usize,
    ) -> io::Result<Vec<u8>> {
        let mut full: Vec<OsString> = vec![
            "--host".into(),
            format!("unix://{}", self.config.docker_socket.display()).into(),
            "--config".into(),
            self.layout.docker_config().into(),
        ];
        full.extend(arguments);
        run_checked(
            Path::new("/usr/bin/docker"),
            &full,
            Duration::from_secs(seconds),
            limit,
        )
        .await
    }

    async fn ensure_network(&self) -> io::Result<()> {
        let name = &self.config.network_name;
        if self
            .docker(
                vec!["network".into(), "inspect".into(), name.into()],
                30,
                1024 * 1024,
            )
            .await
            .is_ok()
        {
            return Ok(());
        }
        self.docker(
            vec![
                "network".into(),
                "create".into(),
                "--driver".into(),
                "bridge".into(),
                "--subnet".into(),
                self.config.network_subnet.clone().into(),
                "--opt".into(),
                "com.docker.network.bridge.enable_icc=false".into(),
                "--label".into(),
                "dev.game-manager=network".into(),
                name.into(),
            ],
            30,
            64 * 1024,
        )
        .await
        .map(|_| ())
    }

    async fn container_state(&self, name: &str) -> io::Result<ContainerState> {
        let output = match self
            .docker(
                vec![
                    "inspect".into(),
                    "--type".into(),
                    "container".into(),
                    "--format".into(),
                    "{{json .State}}".into(),
                    "--".into(),
                    name.into(),
                ],
                15,
                64 * 1024,
            )
            .await
        {
            Ok(output) => output,
            Err(error) if error.to_string().contains("No such container") => {
                return Ok(ContainerState::default());
            }
            Err(error) => return Err(error),
        };
        let state: DockerState = serde_json::from_slice(&output).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "unexpected container state")
        })?;
        Ok(ContainerState {
            exists: true,
            running: state.running,
            exit_code: (!state.running).then_some(state.exit_code),
            started_at: parse_docker_time(&state.started_at),
        })
    }

    async fn remove_container(&self, name: &str) -> io::Result<()> {
        match self
            .docker(
                vec!["rm".into(), "--force".into(), "--".into(), name.into()],
                60,
                64 * 1024,
            )
            .await
        {
            Err(error) if !error.to_string().contains("No such container") => Err(error),
            _ => Ok(()),
        }
    }

    async fn update<T>(&self, id: Uuid, change: impl FnOnce(&mut Record) -> T) -> io::Result<T> {
        let mut records = self.records.lock().await;
        let record = records
            .get_mut(&id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "server not found"))?;
        let result = change(record);
        persist(&self.layout, record)?;
        Ok(result)
    }

    async fn finish(&self, id: Uuid, kind: OperationKind, outcome: io::Result<String>) {
        let (state, message) = match outcome {
            Ok(message) => (OperationState::Succeeded, message),
            Err(error) => (OperationState::Failed, error.to_string()),
        };
        let _ = self
            .update(id, |record| {
                record.operation = Some(OperationStatus {
                    kind,
                    state,
                    message: message.chars().take(4000).collect(),
                    updated_at: now(),
                });
            })
            .await;
    }

    pub async fn install(self: &Arc<Self>, spec: ServerSpec) -> AgentResult {
        if let Err(reason) = validate_spec(&spec) {
            return rejected("invalid_spec", reason);
        }
        let id = spec.server_id;
        {
            let mut records = self.records.lock().await;
            if let Some(record) = records.get(&id) {
                if record.busy() {
                    return rejected("busy", "another operation is running for this server");
                }
                if record.installed {
                    return rejected("already_installed", "server is already installed");
                }
            }
            let uid = match records.get(&id) {
                Some(record) => record.uid,
                None => {
                    let used: std::collections::HashSet<u32> =
                        records.values().map(|r| r.uid).collect();
                    match (0..100_000u32)
                        .map(|n| self.config.uid_base.saturating_add(n))
                        .find(|uid| !used.contains(uid))
                    {
                        Some(uid) => uid,
                        None => return rejected("exhausted", "no free server UID"),
                    }
                }
            };
            let record = Record {
                spec: spec.clone(),
                uid,
                image: None,
                installed: false,
                desired_running: false,
                stop_requested_at: None,
                restarts: vec![],
                pending_restart_at: None,
                problem: None,
                operation: Some(OperationStatus {
                    kind: OperationKind::Install,
                    state: OperationState::Running,
                    message: "Preparing storage".into(),
                    updated_at: now(),
                }),
                game: None,
            };
            if let Err(error) = persist(&self.layout, &record) {
                return rejected("storage", error.to_string());
            }
            records.insert(id, record);
        }
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            let outcome = runtime.run_install(id).await;
            runtime.finish(id, OperationKind::Install, outcome).await;
        });
        AgentResult::Accepted
    }

    async fn progress(&self, id: Uuid, message: &str) {
        let _ = self
            .update(id, |record| {
                if let Some(op) = record.operation.as_mut() {
                    op.message = message.into();
                    op.updated_at = now();
                }
            })
            .await;
    }

    async fn run_install(&self, id: Uuid) -> io::Result<String> {
        let (spec, uid) = {
            let records = self.records.lock().await;
            let record = records
                .get(&id)
                .ok_or_else(|| io::Error::other("server vanished"))?;
            (record.spec.clone(), record.uid)
        };
        self.ensure_network().await?;
        self.remove_container(&install_name(id)).await?;
        // A failed earlier attempt leaves storage behind; reinstalling starts from a clean volume.
        storage::remove_volume(&self.layout, id).await?;
        storage::create_volume(&self.layout, id, uid, spec.limits.disk_bytes).await?;
        self.progress(id, "Downloading images").await;
        for image in [&spec.image, &spec.installer.container] {
            self.docker(
                vec!["pull".into(), "--quiet".into(), "--".into(), image.into()],
                PULL_TIMEOUT_SECS,
                64 * 1024,
            )
            .await
            .map_err(|e| io::Error::other(format!("could not download {image}: {e}")))?;
        }
        let pinned = self.pinned_image(&spec.image).await?;
        let scripts = self.layout.scripts(id);
        std::fs::create_dir_all(&scripts)?;
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&scripts, std::fs::Permissions::from_mode(0o755))?;
            let script = spec.installer.script.replace("\r\n", "\n");
            std::fs::write(scripts.join("install.sh"), script)?;
            std::fs::set_permissions(
                scripts.join("install.sh"),
                std::fs::Permissions::from_mode(0o644),
            )?;
        }
        self.progress(id, "Running the installer").await;
        let volume = self.layout.volume(id);
        let plan = ContainerPlan {
            spec: &spec,
            uid,
            image: &pinned,
            volume: &volume,
            network: &self.config.network_name,
            egress: self.egress(),
            stop_grace_secs: self.config.stop_grace_secs,
        };
        let name = install_name(id);
        self.docker(installer_args(&plan, &scripts), 120, 64 * 1024)
            .await?;
        let result = async {
            self.docker(
                vec!["start".into(), "--".into(), name.clone().into()],
                120,
                64 * 1024,
            )
            .await?;
            let code = self
                .docker(
                    vec!["wait".into(), "--".into(), name.clone().into()],
                    INSTALL_TIMEOUT_SECS,
                    1024,
                )
                .await
                .map_err(|e| io::Error::other(format!("installer did not finish: {e}")))?;
            let code: i64 = String::from_utf8_lossy(&code).trim().parse().unwrap_or(-1);
            if code != 0 {
                let lines = engine::logs(&self.config.docker_socket, &name, "tail=25", 1024 * 1024)
                    .await
                    .map(|bytes| engine::console_lines(&bytes).join("\n"))
                    .unwrap_or_default();
                return Err(io::Error::other(format!(
                    "installer exited with code {code}. Last output:\n{lines}"
                )));
            }
            Ok(())
        }
        .await;
        let _ = self.remove_container(&name).await;
        result?;
        self.progress(id, "Finishing installation").await;
        let root = volume.clone();
        tokio::task::spawn_blocking(move || files::chown_tree(&root, uid))
            .await
            .map_err(|_| io::Error::other("ownership fix-up failed"))??;
        let root = volume.clone();
        let game =
            tokio::task::spawn_blocking(move || as_server(uid, || files::detect_game(&root)))
                .await
                .map_err(|_| io::Error::other("game detection failed"))?
                .unwrap_or(None);
        let summary = match &game {
            Some(game) => format!(
                "Installed Minecraft {}{}",
                game.version,
                game.loader
                    .as_ref()
                    .map(|l| format!(" with {l}"))
                    .unwrap_or_default()
            ),
            None => "Installed".into(),
        };
        self.update(id, |record| {
            record.installed = true;
            record.image = Some(pinned);
            record.game = game;
        })
        .await?;
        Ok(summary)
    }

    // Resolves the image to an immutable reference so later starts run exactly what was installed.
    async fn pinned_image(&self, image: &str) -> io::Result<String> {
        let output = self
            .docker(
                vec![
                    "image".into(),
                    "inspect".into(),
                    "--format".into(),
                    "{{json .RepoDigests}} {{json .Id}}".into(),
                    "--".into(),
                    image.into(),
                ],
                30,
                64 * 1024,
            )
            .await?;
        let text = String::from_utf8_lossy(&output);
        let (digests, image_id) = text
            .trim()
            .split_once(' ')
            .ok_or_else(|| io::Error::other("unexpected image metadata"))?;
        let digests: Vec<String> = serde_json::from_str(digests).unwrap_or_default();
        let repository = image.split('@').next().unwrap_or(image);
        let repository = match repository.rsplit_once(':') {
            Some((name, tag)) if !tag.contains('/') => name,
            _ => repository,
        };
        if let Some(digest) = digests
            .iter()
            .find(|d| d.starts_with(&format!("{repository}@")))
            .or(digests.first())
        {
            return Ok(digest.clone());
        }
        let image_id: String =
            serde_json::from_str(image_id).map_err(|_| io::Error::other("unexpected image id"))?;
        if !image_id.starts_with("sha256:") || !valid_image(&image_id) {
            return Err(io::Error::other("image has no immutable identifier"));
        }
        Ok(image_id)
    }

    pub async fn start(self: &Arc<Self>, spec: ServerSpec) -> AgentResult {
        if let Err(reason) = validate_spec(&spec) {
            return rejected("invalid_spec", reason);
        }
        let id = spec.server_id;
        {
            let mut records = self.records.lock().await;
            let Some(record) = records.get_mut(&id) else {
                return rejected("not_found", "server is not installed");
            };
            if record.busy() {
                return rejected("busy", "another operation is running for this server");
            }
            if !record.installed {
                return rejected("not_installed", "install the server before starting it");
            }
            record.spec = spec;
            // A manual start is a fresh decision: it clears the crash circuit breaker.
            record.restarts.clear();
            record.problem = None;
            record.pending_restart_at = None;
            if let Err(error) = persist(&self.layout, record) {
                return rejected("storage", error.to_string());
            }
        }
        match self.launch(id).await {
            Ok(()) => AgentResult::Accepted,
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                rejected("port_in_use", error.to_string())
            }
            Err(error) => rejected("start_failed", error.to_string()),
        }
    }

    // Applies configuration, recreates the container from the pinned image and starts it.
    async fn launch(&self, id: Uuid) -> io::Result<()> {
        let name = runtime_name(id);
        if self.container_state(&name).await?.running {
            self.update(id, |record| {
                record.desired_running = true;
                record.stop_requested_at = None;
            })
            .await?;
            return Ok(());
        }
        let (spec, uid, image) = {
            let records = self.records.lock().await;
            let record = records
                .get(&id)
                .ok_or_else(|| io::Error::other("server vanished"))?;
            (
                record.spec.clone(),
                record.uid,
                record
                    .image
                    .clone()
                    .ok_or_else(|| io::Error::other("installed image is unknown"))?,
            )
        };
        storage::ensure_mounted(&self.layout, id).await?;
        check_port_free(&spec.binding)?;
        let volume = self.layout.volume(id);
        let root = volume.clone();
        let transform_spec = spec.clone();
        tokio::task::spawn_blocking(move || {
            as_server(uid, || {
                for (path, transform) in &transform_spec.files {
                    let mut values = BTreeMap::new();
                    for (key, value) in &transform.find {
                        values.insert(
                            key.clone(),
                            files::render(
                                value,
                                &transform_spec.binding,
                                &transform_spec.variables,
                            )?,
                        );
                    }
                    let existing = files::read_text(&root, path)?.unwrap_or_default();
                    files::write_atomic(
                        &root,
                        path,
                        files::set_properties(&existing, &values).as_bytes(),
                    )?;
                }
                if transform_spec.accept_eula {
                    files::write_atomic(&root, "eula.txt", b"eula=true\n")?;
                }
                Ok(())
            })
        })
        .await
        .map_err(|_| io::Error::other("configuration update failed"))??;
        self.ensure_network().await?;
        self.remove_container(&name).await?;
        let plan = ContainerPlan {
            spec: &spec,
            uid,
            image: &image,
            volume: &volume,
            network: &self.config.network_name,
            egress: self.egress(),
            stop_grace_secs: self.config.stop_grace_secs,
        };
        self.ready
            .lock()
            .map_err(|_| io::Error::other("lock poisoned"))?
            .remove(&id);
        if let Err(error) = self.docker(runtime_args(&plan), 120, 64 * 1024).await {
            let _ = self.remove_container(&name).await;
            let message = error.to_string();
            if message.contains("address already in use")
                || message.contains("port is already allocated")
            {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "the server's port is already in use",
                ));
            }
            return Err(error);
        }
        self.update(id, |record| {
            record.desired_running = true;
            record.stop_requested_at = None;
            record.pending_restart_at = None;
        })
        .await
    }

    pub async fn stop(self: &Arc<Self>, id: Uuid) -> AgentResult {
        let stop_command = {
            let mut records = self.records.lock().await;
            let Some(record) = records.get_mut(&id) else {
                return rejected("not_found", "server not found");
            };
            record.desired_running = false;
            record.pending_restart_at = None;
            if record.stop_requested_at.is_none() {
                record.stop_requested_at = Some(now());
            }
            if let Err(error) = persist(&self.layout, record) {
                return rejected("storage", error.to_string());
            }
            record.spec.stop.clone()
        };
        match self.container_state(&runtime_name(id)).await {
            Ok(state) if state.running => {}
            Ok(_) => {
                let _ = self.update(id, |r| r.stop_requested_at = None).await;
                return AgentResult::Accepted;
            }
            Err(error) => return rejected("docker", error.to_string()),
        }
        if let Err(error) = self.send_stop(id, &stop_command).await {
            return rejected("stop_failed", error.to_string());
        }
        let runtime = Arc::clone(self);
        tokio::spawn(async move { runtime.enforce_grace(id).await });
        AgentResult::Accepted
    }

    async fn send_stop(&self, id: Uuid, stop: &str) -> io::Result<()> {
        let name = runtime_name(id);
        if stop == "^C" || stop == "^^C" {
            let signal = if stop == "^C" { "SIGINT" } else { "SIGKILL" };
            self.docker(
                vec![
                    "kill".into(),
                    "--signal".into(),
                    signal.into(),
                    "--".into(),
                    name.into(),
                ],
                30,
                1024,
            )
            .await
            .map(|_| ())
        } else {
            engine::write_stdin(
                &self.config.docker_socket,
                &name,
                format!("{stop}\n").as_bytes(),
            )
            .await
        }
    }

    // Waits for a requested stop; after the grace period the container is killed and the forced
    // stop is recorded so it shows up for users and the activity log.
    async fn enforce_grace(&self, id: Uuid) {
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(self.config.stop_grace_secs);
        while tokio::time::Instant::now() < deadline {
            match self.container_state(&runtime_name(id)).await {
                Ok(state) if !state.running => {
                    let _ = self.update(id, |r| r.stop_requested_at = None).await;
                    return;
                }
                _ => tokio::time::sleep(Duration::from_secs(1)).await,
            }
        }
        let _ = self
            .docker(
                vec!["kill".into(), "--".into(), runtime_name(id).into()],
                30,
                1024,
            )
            .await;
        let grace = self.config.stop_grace_secs;
        let _ = self
            .update(id, |record| {
                record.stop_requested_at = None;
                record.problem = Some(format!(
                    "The server did not stop within {grace} seconds and was force-stopped"
                ));
            })
            .await;
    }

    pub async fn kill(&self, id: Uuid) -> AgentResult {
        if self
            .update(id, |record| {
                record.desired_running = false;
                record.pending_restart_at = None;
                record.stop_requested_at = None;
            })
            .await
            .is_err()
        {
            return rejected("not_found", "server not found");
        }
        match self
            .docker(
                vec!["kill".into(), "--".into(), runtime_name(id).into()],
                30,
                1024,
            )
            .await
        {
            Ok(_) => AgentResult::Accepted,
            Err(error)
                if error.to_string().contains("is not running")
                    || error.to_string().contains("No such container") =>
            {
                AgentResult::Accepted
            }
            Err(error) => rejected("docker", error.to_string()),
        }
    }

    pub async fn send_command(&self, id: Uuid, command: &str) -> AgentResult {
        if !valid_command(command) {
            return rejected(
                "invalid_command",
                "commands must be one line of at most 512 bytes",
            );
        }
        let stopping = match self.records.lock().await.get(&id) {
            None => return rejected("not_found", "server not found"),
            Some(record) => record.stop_requested_at.is_some(),
        };
        match self.container_state(&runtime_name(id)).await {
            Ok(state) if state.running && !stopping => {}
            Ok(_) => return rejected("not_running", "the server is not running"),
            Err(error) => return rejected("docker", error.to_string()),
        }
        match engine::write_stdin(
            &self.config.docker_socket,
            &runtime_name(id),
            format!("{command}\n").as_bytes(),
        )
        .await
        {
            Ok(()) => AgentResult::Accepted,
            Err(error) => rejected("command_failed", error.to_string()),
        }
    }

    pub async fn console(&self, id: Uuid, lines: u16) -> AgentResult {
        let lines = lines.clamp(1, MAX_CONSOLE_LINES);
        let installing = match self.records.lock().await.get(&id) {
            None => return rejected("not_found", "server not found"),
            Some(record) => {
                record.busy()
                    && record
                        .operation
                        .as_ref()
                        .is_some_and(|op| op.kind == OperationKind::Install)
            }
        };
        let name = if installing {
            install_name(id)
        } else {
            runtime_name(id)
        };
        match engine::logs(
            &self.config.docker_socket,
            &name,
            &format!("tail={lines}"),
            4 * 1024 * 1024,
        )
        .await
        {
            Ok(bytes) => {
                let mut output = engine::console_lines(&bytes);
                let excess = output.len().saturating_sub(lines as usize);
                output.drain(..excess);
                AgentResult::Console(output)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => AgentResult::Console(vec![]),
            Err(error) => rejected("docker", error.to_string()),
        }
    }

    async fn readiness(&self, id: Uuid, started_at: i64, marker: &str) -> bool {
        if self.ready.lock().ok().and_then(|r| r.get(&id).copied()) == Some(started_at) {
            return true;
        }
        let query = format!("since={}", started_at.saturating_sub(1));
        let found = match engine::logs(
            &self.config.docker_socket,
            &runtime_name(id),
            &query,
            4 * 1024 * 1024,
        )
        .await
        {
            Ok(bytes) => String::from_utf8_lossy(&bytes).contains(marker),
            // Very chatty servers exceed the bound; check the recent tail instead.
            Err(_) => engine::logs(
                &self.config.docker_socket,
                &runtime_name(id),
                "tail=2000",
                4 * 1024 * 1024,
            )
            .await
            .is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains(marker)),
        };
        if found && let Ok(mut ready) = self.ready.lock() {
            ready.insert(id, started_at);
        }
        found
    }

    pub async fn status(&self, id: Uuid) -> AgentResult {
        let Some(record) = self.records.lock().await.get(&id).cloned() else {
            return rejected("not_found", "server not found");
        };
        let state = if record.installed {
            match self.container_state(&runtime_name(id)).await {
                Ok(state) => state,
                Err(error) => return rejected("docker", error.to_string()),
            }
        } else {
            ContainerState::default()
        };
        let power = match (state.running, state.started_at) {
            (false, _) => PowerState::Offline,
            (true, _) if record.stop_requested_at.is_some() => PowerState::Stopping,
            (true, Some(started)) if self.readiness(id, started, &record.spec.ready_when).await => {
                PowerState::Running
            }
            (true, _) => PowerState::Starting,
        };
        let usage = if storage::mounted(&self.layout.volume(id)).unwrap_or(false) {
            storage::volume_usage(&self.layout.volume(id)).ok()
        } else {
            None
        };
        AgentResult::ServerStatus(Box::new(RuntimeStatus {
            installed: record.installed,
            operation: record.operation.clone(),
            power,
            desired_running: record.desired_running,
            problem: record.problem.clone(),
            started_at: state.running.then_some(state.started_at).flatten(),
            exit_code: state.exit_code.filter(|_| state.exists),
            game: record.game.clone(),
            disk_used_bytes: usage.map(|u| u.0),
            disk_total_bytes: usage.map(|u| u.1),
        }))
    }

    pub async fn list_files(&self, id: Uuid, folder: ExtensionFolder) -> AgentResult {
        let Some(uid) = self.records.lock().await.get(&id).map(|r| r.uid) else {
            return rejected("not_found", "server not found");
        };
        if !storage::mounted(&self.layout.volume(id)).unwrap_or(false) {
            return AgentResult::Files(vec![]);
        }
        let root = self.layout.volume(id);
        match tokio::task::spawn_blocking(move || {
            as_server(uid, || {
                files::list_files(&root, folder_name(folder), ".jar")
            })
        })
        .await
        {
            Ok(Ok(names)) => AgentResult::Files(names),
            Ok(Err(error)) => rejected("files", error.to_string()),
            Err(_) => rejected("files", "listing failed"),
        }
    }

    pub async fn install_files(
        self: &Arc<Self>,
        id: Uuid,
        folder: ExtensionFolder,
        downloads: Vec<RemoteFile>,
        remove: Vec<String>,
    ) -> AgentResult {
        if downloads.is_empty() && remove.is_empty() || downloads.len() + remove.len() > 64 {
            return rejected("invalid_files", "choose between 1 and 64 files");
        }
        if !downloads.iter().all(valid_remote_file)
            || !remove.iter().all(|n| valid_extension_name(n))
        {
            return rejected(
                "invalid_files",
                "files must be Modrinth CDN jars with SHA-512 hashes",
            );
        }
        let total = downloads
            .iter()
            .map(|f| f.size)
            .fold(0u64, u64::saturating_add);
        if total > MAX_EXTENSION_TOTAL {
            return rejected("too_large", "the selected files are too large");
        }
        let mut names = std::collections::HashSet::new();
        if !downloads
            .iter()
            .all(|f| names.insert(f.filename.to_ascii_lowercase()))
        {
            return rejected("invalid_files", "duplicate file names");
        }
        {
            let mut records = self.records.lock().await;
            let Some(record) = records.get_mut(&id) else {
                return rejected("not_found", "server not found");
            };
            if record.busy() {
                return rejected("busy", "another operation is running for this server");
            }
            if !record.installed {
                return rejected("not_installed", "install the server first");
            }
            if record.desired_running {
                return rejected("running", "stop the server before changing mods or plugins");
            }
            record.operation = Some(OperationStatus {
                kind: OperationKind::Extensions,
                state: OperationState::Running,
                message: "Downloading files".into(),
                updated_at: now(),
            });
            if let Err(error) = persist(&self.layout, record) {
                return rejected("storage", error.to_string());
            }
        }
        match self.container_state(&runtime_name(id)).await {
            Ok(state) if !state.running => {}
            _ => {
                self.finish(
                    id,
                    OperationKind::Extensions,
                    Err(io::Error::other(
                        "stop the server before changing mods or plugins",
                    )),
                )
                .await;
                return rejected("running", "stop the server before changing mods or plugins");
            }
        }
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            let outcome = runtime
                .run_install_files(id, folder, downloads, remove)
                .await;
            runtime.finish(id, OperationKind::Extensions, outcome).await;
        });
        AgentResult::Accepted
    }

    async fn run_install_files(
        &self,
        id: Uuid,
        folder: ExtensionFolder,
        downloads: Vec<RemoteFile>,
        remove: Vec<String>,
    ) -> io::Result<String> {
        let uid = self
            .records
            .lock()
            .await
            .get(&id)
            .map(|r| r.uid)
            .ok_or_else(|| io::Error::other("server vanished"))?;
        // Download and verify everything before touching the server, so a failure changes nothing.
        let client = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(io::Error::other)?;
        let mut fetched = Vec::new();
        for file in &downloads {
            fetched.push((
                file.filename.clone(),
                download_verified(&client, file).await?,
            ));
        }
        storage::ensure_mounted(&self.layout, id).await?;
        let root = self.layout.volume(id);
        let folder = folder_name(folder);
        let installed = fetched.len();
        let removed = remove.len();
        tokio::task::spawn_blocking(move || {
            as_server(uid, || {
                let existing: std::collections::HashSet<String> =
                    files::list_files(&root, folder, ".jar")?
                        .into_iter()
                        .collect();
                let mut written = Vec::new();
                for (name, bytes) in &fetched {
                    if let Err(error) =
                        files::write_atomic(&root, &format!("{folder}/{name}"), bytes)
                    {
                        for name in written {
                            let _ = files::remove(&root, &format!("{folder}/{name}"));
                        }
                        return Err(error);
                    }
                    if !existing.contains(name) {
                        written.push(name.clone());
                    }
                }
                for name in &remove {
                    if !fetched.iter().any(|(fetched, _)| fetched == name) {
                        files::remove(&root, &format!("{folder}/{name}"))?;
                    }
                }
                Ok(())
            })
        })
        .await
        .map_err(|_| io::Error::other("file installation failed"))??;
        Ok(format!("Installed {installed} file(s), removed {removed}"))
    }

    pub async fn delete(self: &Arc<Self>, id: Uuid, keep_backups: bool) -> AgentResult {
        {
            let mut records = self.records.lock().await;
            let Some(record) = records.get_mut(&id) else {
                return rejected("not_found", "server not found");
            };
            // Installs are bounded by their own deadlines; deleting underneath one would race it.
            if record.busy() {
                return rejected("busy", "another operation is running for this server");
            }
            record.desired_running = false;
            record.pending_restart_at = None;
            record.operation = Some(OperationStatus {
                kind: OperationKind::Delete,
                state: OperationState::Running,
                message: "Deleting".into(),
                updated_at: now(),
            });
            if let Err(error) = persist(&self.layout, record) {
                return rejected("storage", error.to_string());
            }
        }
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            match runtime.run_delete(id, keep_backups).await {
                Ok(()) => {
                    runtime.records.lock().await.remove(&id);
                }
                Err(error) => runtime.finish(id, OperationKind::Delete, Err(error)).await,
            }
        });
        AgentResult::Accepted
    }

    async fn run_delete(&self, id: Uuid, keep_backups: bool) -> io::Result<()> {
        self.remove_container(&runtime_name(id)).await?;
        if !keep_backups {
            self.remove_all_backups(id)?;
        }
        self.remove_container(&install_name(id)).await?;
        storage::remove_volume(&self.layout, id).await?;
        match std::fs::remove_dir_all(self.layout.scripts(id)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        match std::fs::remove_file(registry_path(&self.layout, id)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    // Starts servers that were running before the agent stopped, one at a time.
    pub async fn restore(self: &Arc<Self>) {
        let ids: Vec<Uuid> = self
            .records
            .lock()
            .await
            .iter()
            .filter(|(_, r)| r.desired_running && r.installed && !r.busy())
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if let Err(error) = self.launch(id).await {
                let _ = self
                    .update(id, |r| {
                        r.problem = Some(format!("Could not start after a host restart: {error}"))
                    })
                    .await;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    // Watches desired-running servers and applies the crash restart policy.
    pub async fn supervise_once(self: &Arc<Self>) {
        let candidates: Vec<(Uuid, Option<i64>)> = self
            .records
            .lock()
            .await
            .iter()
            .filter(|(_, r)| {
                r.desired_running && r.installed && !r.busy() && r.stop_requested_at.is_none()
            })
            .map(|(id, r)| (*id, r.pending_restart_at))
            .collect();
        for (id, pending) in candidates {
            let Ok(state) = self.container_state(&runtime_name(id)).await else {
                continue;
            };
            if state.running {
                continue;
            }
            let current = now();
            if let Some(at) = pending {
                if current >= at
                    && let Err(error) = self.launch(id).await
                {
                    let _ = self
                        .update(id, |r| {
                            r.pending_restart_at = None;
                            r.desired_running = false;
                            r.problem = Some(format!("Automatic restart failed: {error}"));
                        })
                        .await;
                }
                continue;
            }
            let code = state.exit_code.unwrap_or(-1);
            let _ = self
                .update(id, |record| match on_unexpected_exit(&mut record.restarts, current) {
                    CrashDecision::Restart { delay_secs } => {
                        record.pending_restart_at = Some(current + delay_secs as i64);
                        record.problem = Some(format!(
                            "The server stopped unexpectedly (exit code {code}); restarting in {delay_secs} seconds"
                        ));
                    }
                    CrashDecision::GiveUp => {
                        record.desired_running = false;
                        record.problem = Some(format!(
                            "The server crashed repeatedly (last exit code {code}); automatic restarts are paused until it is started manually"
                        ));
                    }
                })
                .await;
        }
    }

    // Controlled host shutdown: ask every running server to stop, wait up to the grace period
    // concurrently, then kill what is left. Desired state is kept so the servers return at boot.
    pub async fn shutdown(self: &Arc<Self>) {
        let servers: Vec<(Uuid, String)> = self
            .records
            .lock()
            .await
            .iter()
            .filter(|(_, r)| r.installed)
            .map(|(id, r)| (*id, r.spec.stop.clone()))
            .collect();
        let mut waits = Vec::new();
        for (id, stop) in servers {
            if !self
                .container_state(&runtime_name(id))
                .await
                .is_ok_and(|s| s.running)
            {
                continue;
            }
            let _ = self.send_stop(id, &stop).await;
            let runtime = Arc::clone(self);
            waits.push(tokio::spawn(async move {
                let deadline = tokio::time::Instant::now()
                    + Duration::from_secs(runtime.config.stop_grace_secs);
                while tokio::time::Instant::now() < deadline {
                    if !runtime
                        .container_state(&runtime_name(id))
                        .await
                        .is_ok_and(|s| s.running)
                    {
                        return;
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                let _ = runtime
                    .docker(
                        vec!["kill".into(), "--".into(), runtime_name(id).into()],
                        30,
                        1024,
                    )
                    .await;
                let _ = runtime
                    .update(id, |r| {
                        r.problem =
                            Some("Force-stopped during host shutdown after the grace period".into())
                    })
                    .await;
            }));
        }
        for wait in waits {
            let _ = wait.await;
        }
    }
}

fn registry_path(layout: &Layout, id: Uuid) -> PathBuf {
    layout.registry().join(format!("{id}.json"))
}

fn persist(layout: &Layout, record: &Record) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = registry_path(layout, record.spec.server_id);
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(record).map_err(io::Error::other)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    std::fs::rename(&temporary, &path)
}

// Verifies nothing else on the host already listens on the allocation before Docker binds it.
pub fn check_port_free(binding: &PortBinding) -> io::Result<()> {
    let address = std::net::SocketAddr::new(binding.ip, binding.port);
    let result = match binding.transport {
        Transport::Tcp => std::net::TcpListener::bind(address).map(drop),
        Transport::Udp => std::net::UdpSocket::bind(address).map(drop),
    };
    result.map_err(|error| match error.kind() {
        io::ErrorKind::AddrInUse => io::Error::new(
            io::ErrorKind::AddrInUse,
            "the server's port is already in use",
        ),
        io::ErrorKind::AddrNotAvailable => io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "the server's IP address is not present on this host",
        ),
        _ => error,
    })
}

async fn download_verified(client: &reqwest::Client, file: &RemoteFile) -> io::Result<Vec<u8>> {
    use futures_util::StreamExt;
    use sha2::{Digest, Sha512};
    let response = client
        .get(&file.url)
        .send()
        .await
        .map_err(|e| io::Error::other(format!("download of {} failed: {e}", file.filename)))?;
    if !response.status().is_success() {
        return Err(io::Error::other(format!(
            "download of {} failed with HTTP {}",
            file.filename,
            response.status().as_u16()
        )));
    }
    let mut bytes = Vec::with_capacity(file.size as usize);
    let mut hasher = Sha512::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|e| io::Error::other(format!("download of {} failed: {e}", file.filename)))?;
        if bytes.len() as u64 + chunk.len() as u64 > file.size {
            return Err(io::Error::other(format!(
                "{} is larger than expected",
                file.filename
            )));
        }
        hasher.update(&chunk);
        bytes.extend_from_slice(&chunk);
    }
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if bytes.len() as u64 != file.size || digest != file.sha512.to_ascii_lowercase() {
        return Err(io::Error::other(format!(
            "{} failed its integrity check; nothing was installed",
            file.filename
        )));
    }
    Ok(bytes)
}
