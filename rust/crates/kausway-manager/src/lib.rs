//! Optional process supervisor. No transport engine and no process adoption.
use clap::Args;
use std::path::PathBuf;

#[derive(Clone, Args)]
pub struct RunArgs {
    #[arg(skip)]
    pub sources: std::collections::BTreeMap<String, kausway_config::Source>,
    #[arg(long, env = "KAUSWAY_RESTART_POLICY", value_parser = ["host", "application"])]
    pub restart_policy: Option<String>,
    #[arg(long, env = "KAUSWAY_APPLICATION_NAME")]
    pub application_name: Option<String>,
    #[arg(long, env = "KAUSWAY_CONFIG")]
    pub config: Option<PathBuf>,
    /// Suite root: contains host/, manager/ and admin/ (not a host component directory).
    #[arg(long, env = "KAUSWAY_DATA_ROOT")]
    pub data_root: Option<PathBuf>,
    #[arg(long, env = "KAUSWAY_HOST_BINARY")]
    pub host_binary: Option<PathBuf>,
    #[arg(long, env = "KAUSWAY_ADMIN_BINARY")]
    pub admin_binary: Option<PathBuf>,
    #[arg(long, env = "KAUSWAY_HOST_ENABLED", action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub host_enabled: Option<bool>,
    #[arg(long, env = "KAUSWAY_ADMIN_ENABLED", action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub admin_ui: Option<bool>,
    /// Explicit host origin override (saved values remain unchanged).
    #[arg(long)]
    pub origin: Option<String>,
    #[arg(long)]
    pub relay_url: Option<String>,
    #[arg(long)]
    pub policy: Option<String>,
    #[arg(long)]
    pub websockets: Option<String>,
    #[arg(long)]
    pub websocket_origin: Option<String>,
    #[arg(long, env = "KAUSWAY_SHUTDOWN_TIMEOUT_SECS")]
    pub shutdown_timeout_secs: Option<u64>,
    #[arg(long, env = "KAUSWAY_RESTART_ON_FAILURE", action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub restart_on_failure: Option<bool>,
    #[arg(long, env = "KAUSWAY_RESTART_BACKOFF_INITIAL_SECS")]
    pub restart_backoff_initial_secs: Option<u64>,
    #[arg(long, env = "KAUSWAY_RESTART_BACKOFF_MAX_SECS")]
    pub restart_backoff_max_secs: Option<u64>,
    #[arg(long, env = "KAUSWAY_MAX_FAILURES")]
    pub max_failures: Option<u32>,
    #[arg(long, env = "KAUSWAY_READINESS_TIMEOUT_SECS")]
    pub readiness_timeout_secs: Option<u64>,
    /// Override the host control listener. Must be a concrete loopback IP:port.
    #[arg(long, env = "KAUSWAY_CONTROL_LISTEN")]
    pub control_listen: Option<std::net::SocketAddr>,
}
#[cfg(unix)]
mod supervisor;
#[cfg(unix)]
pub use supervisor::{request, run};
#[cfg(not(unix))]
pub async fn run(_: RunArgs) -> Result<(), String> {
    Err("manager currently requires Unix local IPC and process signals; Windows support is not implemented".into())
}
#[cfg(not(unix))]
pub async fn request(_: &std::path::Path, _: &str) -> Result<serde_json::Value, String> {
    Err("manager local control is not implemented on this platform".into())
}
