#[cfg(target_os = "linux")]
pub mod backups;
pub mod commands;
pub mod docker;
pub mod engine;
#[cfg(target_os = "linux")]
pub mod files;
pub mod host;
#[cfg(target_os = "linux")]
pub mod linux;
pub mod rpc;
#[cfg(target_os = "linux")]
pub mod runtime;
// Host operations need Linux; other platforms only build the protocol/domain pieces.
#[cfg(not(target_os = "linux"))]
pub mod runtime {
    pub struct Runtime {
        pub config: crate::host::AgentConfig,
    }
    impl Runtime {
        pub fn new(config: crate::host::AgentConfig) -> Self {
            Self { config }
        }
    }
}
#[cfg(target_os = "linux")]
pub mod storage;
