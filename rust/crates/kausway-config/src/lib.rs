//! Shared suite configuration, paths and revision-checked persistence.
pub mod administration;
pub mod fsutil;

use fsutil::{ensure_private_dir, restrict_permissions, write_private_atomic};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    env,
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("configuration storage: {0}")]
    Io(#[from] io::Error),
    // Parser diagnostics can contain source text, including credentials.
    #[error("invalid configuration TOML (contents omitted)")]
    Parse,
    #[error("invalid setting: {0}")]
    Invalid(String),
    #[error("configuration changed; reload before saving")]
    Conflict,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub host: Host,
    pub manager: Manager,
    pub admin: Admin,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: 1,
            host: Host::default(),
            manager: Manager::default(),
            admin: Admin::default(),
        }
    }
}
// Deliberately never include credential material in Debug.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Config { contents: <redacted> }")
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Host {
    pub origin: Option<String>,
    pub policy: Option<String>,
    pub relay_url: Option<String>,
    pub websockets: Option<bool>,
    pub websocket_origin: Option<String>,
    pub control: Control,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Control {
    pub listen: Option<String>,
    pub allow_remote: Option<bool>,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartPolicy {
    #[default]
    Host,
    Application,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Manager {
    pub restart_policy: RestartPolicy,
    pub application_name: String,
    pub host_enabled: bool,
    pub shutdown_timeout_secs: u64,
    pub restart_on_failure: bool,
    pub restart_backoff_initial_secs: u64,
    pub restart_backoff_max_secs: u64,
    pub max_failures: u32,
    pub readiness_timeout_secs: u64,
}
impl Default for Manager {
    fn default() -> Self {
        Self {
            host_enabled: false,
            restart_policy: RestartPolicy::Host,
            application_name: "application".into(),
            shutdown_timeout_secs: 15,
            restart_on_failure: true,
            restart_backoff_initial_secs: 1,
            restart_backoff_max_secs: 30,
            max_failures: 5,
            readiness_timeout_secs: 30,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Admin {
    pub enabled: bool,
    pub listen: String,
    pub browser_base_url: Option<String>,
    pub auth: Auth,
}
impl Default for Admin {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: "127.0.0.1:47951".into(),
            browser_base_url: None,
            auth: Auth::default(),
        }
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Auth {
    pub username: Option<String>,
    pub password_hash: Option<String>,
}

impl Config {
    pub fn validate(&self) -> Result<(), Error> {
        let invalid = |s: &str| Error::Invalid(s.into());
        if let Some(origin) = &self.host.origin {
            kausway_transport::proxy::Origin::parse(origin)
                .map_err(|_| invalid("host.origin must be a valid plain HTTP origin"))?;
        }
        if let Some(relay) = &self.host.relay_url
            && !relay.trim().is_empty()
            && relay.trim() != "none"
        {
            relay
                .trim()
                .parse::<iroh::RelayUrl>()
                .map_err(|_| invalid("host.relay_url must be a valid relay URL, empty, or none"))?;
        }
        if self.manager.application_name.trim().is_empty()
            || self.manager.application_name.len() > 128
            || self.manager.application_name.chars().any(char::is_control)
        {
            return Err(invalid(
                "manager.application_name must contain 1–128 bytes without control characters",
            ));
        }
        if self.schema_version != 1 {
            return Err(invalid("schema_version must be 1"));
        }
        if !(1..=120).contains(&self.manager.shutdown_timeout_secs) {
            return Err(invalid(
                "manager.shutdown_timeout_secs must be between 1 and 120",
            ));
        }
        if self.manager.restart_backoff_initial_secs == 0
            || self.manager.restart_backoff_max_secs < self.manager.restart_backoff_initial_secs
            || self.manager.max_failures == 0
            || !(1..=300).contains(&self.manager.readiness_timeout_secs)
            || self.manager.restart_backoff_max_secs > 3600
            || self.manager.max_failures > 100
        {
            return Err(invalid(
                "manager requires readiness_timeout_secs 1..300, max_failures 1..100 and positive backoff bounds with initial <= max <= 3600",
            ));
        }
        let listen: std::net::SocketAddr = self
            .admin
            .listen
            .parse()
            .map_err(|_| invalid("admin.listen must be an IP:port"))?;
        if !listen.ip().is_loopback() && !listen.ip().is_unspecified() {
            return Err(invalid(
                "admin.listen must be loopback or an unspecified container interface",
            ));
        }
        match (&self.admin.auth.username, &self.admin.auth.password_hash) {
            (None, None) => (),
            (Some(u), Some(h)) if !u.trim().is_empty() && h.starts_with("$argon2id$") => (),
            _ => {
                return Err(invalid(
                    "admin.auth requires both username and an encoded Argon2id hash",
                ));
            }
        }
        if let Some(p) = &self.host.policy
            && !["auto", "relay_only", "p2p_only"].contains(&p.as_str())
        {
            return Err(invalid("host.policy must be auto, relay_only or p2p_only"));
        }
        if let Some(p) = &self.host.websocket_origin
            && !["rewrite", "pass_through"].contains(&p.as_str())
        {
            return Err(invalid(
                "host.websocket_origin must be rewrite or pass_through",
            ));
        }
        if let Some(p) = &self.host.control.listen
            && !p.eq_ignore_ascii_case("off")
        {
            let addr: std::net::SocketAddr = p
                .parse()
                .map_err(|_| invalid("host.control.listen must be IP:port or off"))?;
            if !addr.ip().is_loopback() && self.host.control.allow_remote != Some(true) {
                return Err(invalid(
                    "host.control.listen requires host.control.allow_remote for non-loopback binding",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Paths {
    pub config: PathBuf,
    pub data_root: PathBuf,
}
impl Paths {
    pub fn defaults() -> Result<Self, Error> {
        Self::from_environment(|k| env::var_os(k).map(PathBuf::from))
    }
    pub fn from_environment(get: impl Fn(&str) -> Option<PathBuf>) -> Result<Self, Error> {
        #[cfg(not(target_os = "android"))]
        let required = |key| {
            get(key).filter(|p| p.is_absolute()).ok_or_else(|| {
                Error::Invalid(format!("{key} must identify an absolute user directory"))
            })
        };
        #[cfg(target_os = "windows")]
        let (config, data) = {
            let base = required("LOCALAPPDATA")?.join("Kausway");
            (base.clone(), base)
        };
        #[cfg(target_os = "macos")]
        let (config, data) = {
            let base = required("HOME")?.join("Library/Application Support/Kausway");
            (base.clone(), base)
        };
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "android")))]
        let (config, data) = {
            let xdg = |key, fallback: &str| -> Result<PathBuf, Error> {
                match get(key).filter(|p| p.is_absolute()) {
                    Some(p) => Ok(p.join("kausway")),
                    None => Ok(required("HOME")?.join(fallback).join("kausway")),
                }
            };
            (
                xdg("XDG_CONFIG_HOME", ".config")?,
                xdg("XDG_DATA_HOME", ".local/share")?,
            )
        };
        #[cfg(target_os = "android")]
        let (config, data) = android_roots(get, env::current_dir)?;
        Ok(Self {
            config: config.join("config.toml"),
            data_root: data,
        })
    }
    pub fn component(&self, component: Component) -> PathBuf {
        self.data_root.join(component.as_str())
    }
}
// Android native executables have no Context from which to discover app-private
// storage. Terminal defaults use explicit absolute XDG/HOME roots when available,
// otherwise a deterministic working-directory layout. Never probe writability
// and silently fall back: initialization errors belong to the selected location.
#[cfg(any(target_os = "android", all(test, unix)))]
fn android_roots(
    get: impl Fn(&str) -> Option<PathBuf>,
    working_directory: impl FnOnce() -> io::Result<PathBuf>,
) -> Result<(PathBuf, PathBuf), Error> {
    let absolute = |key| get(key).filter(|p| p.is_absolute());
    let config = absolute("XDG_CONFIG_HOME")
        .map(|p| p.join("kausway"))
        .or_else(|| absolute("HOME").map(|p| p.join(".config/kausway")));
    let data = absolute("XDG_DATA_HOME")
        .map(|p| p.join("kausway"))
        .or_else(|| absolute("HOME").map(|p| p.join(".local/share/kausway")));
    match (config, data) {
        (Some(config), Some(data)) => Ok((config, data)),
        (config, data) => {
            let root = working_directory()?.join("kausway");
            Ok((
                config.unwrap_or_else(|| root.clone()),
                data.unwrap_or_else(|| root.join("data")),
            ))
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Component {
    Host,
    Manager,
    Admin,
    Client,
}
impl Component {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Manager => "manager",
            Self::Admin => "admin",
            Self::Client => "client",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Cli,
    Environment,
    File,
    Default,
    State,
    Unavailable,
}
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    Live,
    Restart,
    LaunchOnly,
    ManagedState,
}
#[derive(Debug, Clone, Serialize)]
pub struct Setting {
    pub saved: Option<serde_json::Value>,
    pub effective: serde_json::Value,
    pub source: Source,
    pub change: Change,
    pub sensitive: bool,
}
pub type Settings = BTreeMap<String, Setting>;
/// Redaction is performed before values enter diagnostic metadata.
pub fn setting(
    saved: Option<serde_json::Value>,
    effective: serde_json::Value,
    source: Source,
    change: Change,
    sensitive: bool,
) -> Setting {
    Setting {
        saved: if sensitive { None } else { saved },
        effective: if sensitive {
            serde_json::Value::String("<redacted>".into())
        } else {
            effective
        },
        source,
        change,
        sensitive,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision([u8; 32]);
pub struct Snapshot {
    pub config: Config,
    pub revision: Revision,
}
pub struct Store {
    path: PathBuf,
}
impl Store {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    fn lock(&self) -> Result<File, Error> {
        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        ensure_private_dir(parent)?;
        let lock = self.path.with_file_name(format!(
            "{}.lock",
            self.path
                .file_name()
                .ok_or_else(|| Error::Invalid("config must be a file path".into()))?
                .to_string_lossy()
        ));
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(lock)?;
        file.lock()?;
        Ok(file)
    }
    pub fn load_or_create(&self) -> Result<Snapshot, Error> {
        let _lock = self.lock()?;
        match fs::read(&self.path) {
            Ok(bytes) => self.decode(bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let config = Config::default();
                let bytes = toml::to_string_pretty(&config)
                    .map_err(|_| Error::Parse)?
                    .into_bytes();
                write_private_atomic(&self.path, &bytes)?;
                self.decode(bytes)
            }
            Err(e) => Err(e.into()),
        }
    }
    fn decode(&self, bytes: Vec<u8>) -> Result<Snapshot, Error> {
        let config: Config = toml::from_str(std::str::from_utf8(&bytes).map_err(|_| Error::Parse)?)
            .map_err(|_| Error::Parse)?;
        config.validate()?;
        restrict_permissions(&self.path)?;
        Ok(Snapshot {
            config,
            revision: Revision(Sha256::digest(&bytes).into()),
        })
    }
    /// Hold the shared config lock through a revision-checked external state update.
    pub fn with_revision<T>(
        &self,
        expected: &Revision,
        operation: impl FnOnce() -> Result<T, Error>,
    ) -> Result<T, Error> {
        let _lock = self.lock()?;
        let bytes = fs::read(&self.path)?;
        if Revision(Sha256::digest(&bytes).into()) != *expected {
            return Err(Error::Conflict);
        }
        operation()
    }

    pub fn save(&self, config: &Config, expected: &Revision) -> Result<Snapshot, Error> {
        config.validate()?;
        let _lock = self.lock()?;
        let bytes = fs::read(&self.path)?;
        if Revision(Sha256::digest(&bytes).into()) != *expected {
            return Err(Error::Conflict);
        }
        let bytes = toml::to_string_pretty(config)
            .map_err(|_| Error::Parse)?
            .into_bytes();
        write_private_atomic(&self.path, &bytes)?;
        self.decode(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil::test_support::TempDir;
    #[test]
    fn stale_writer_is_rejected_and_other_sections_survive() {
        let dir = TempDir::create();
        let store = Store::new(dir.0.join("nested/config.toml"));
        let first = store.load_or_create().unwrap();
        let second = store.load_or_create().unwrap();
        let mut changed = first.config;
        changed.host.origin = Some("http://localhost:8096".into());
        changed.admin.auth.username = Some("owner".into());
        changed.admin.auth.password_hash = Some("$argon2id$test-secret".into());
        let saved = store.save(&changed, &first.revision).unwrap();
        assert!(matches!(
            store.save(&second.config, &second.revision),
            Err(Error::Conflict)
        ));
        let loaded = store.load_or_create().unwrap();
        assert_eq!(loaded.config.host.origin, changed.host.origin);
        assert_eq!(
            loaded.config.admin.auth.password_hash,
            changed.admin.auth.password_hash
        );
        assert_eq!(loaded.revision, saved.revision);
        assert!(!format!("{:?}", loaded.config).contains("test-secret"));
    }
    #[test]
    fn corrupt_file_is_never_replaced_or_echoed() {
        let dir = TempDir::create();
        let path = dir.0.join("config.toml");
        let bytes = b"secret-password is not TOML";
        fs::write(&path, bytes).unwrap();
        let error = Store::new(&path).load_or_create().err().unwrap();
        assert!(!error.to_string().contains("secret-password"));
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    #[test]
    fn invalid_fields_and_partial_credentials_fail() {
        for text in [
            "schema_version = 9",
            "unknown = true",
            "[admin.auth]\nusername = 'alice'",
            "[host]\norigin = 'https://example.com'",
            "[host]\npolicy = 'invalid'",
        ] {
            let result = toml::from_str::<Config>(text)
                .map_err(|_| Error::Parse)
                .and_then(|c| c.validate());
            assert!(result.is_err(), "{text}");
        }
    }
    #[test]
    fn sensitive_metadata_does_not_serialize_secrets() {
        let s = setting(
            Some(serde_json::json!("secret")),
            serde_json::json!("secret"),
            Source::Environment,
            Change::Restart,
            true,
        );
        assert!(!serde_json::to_string(&s).unwrap().contains("secret"));
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn xdg_paths_and_component_layout_are_independent() {
        let paths = Paths::from_environment(|key| match key {
            "HOME" => Some("/users/test".into()),
            "XDG_CONFIG_HOME" => Some("/settings".into()),
            "XDG_DATA_HOME" => Some("relative-is-ignored".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(paths.config, PathBuf::from("/settings/kausway/config.toml"));
        assert_eq!(
            paths.component(Component::Host),
            PathBuf::from("/users/test/.local/share/kausway/host")
        );
    }
}

#[cfg(all(test, unix))]
mod android_path_tests {
    use super::*;
    #[test]
    fn missing_environment_uses_working_directory_with_separate_components() {
        let (config, data) = android_roots(|_| None, || Ok("/terminal/work".into())).unwrap();
        assert_eq!(
            config.join("config.toml"),
            PathBuf::from("/terminal/work/kausway/config.toml")
        );
        let paths = Paths {
            config: config.join("config.toml"),
            data_root: data,
        };
        assert_eq!(
            paths.component(Component::Host),
            PathBuf::from("/terminal/work/kausway/data/host")
        );
        assert_eq!(
            paths.component(Component::Client),
            PathBuf::from("/terminal/work/kausway/data/client")
        );
    }
    #[test]
    fn absolute_environment_paths_do_not_need_a_working_directory() {
        let (config, data) = android_roots(
            |key| match key {
                "HOME" => Some("/termux/home".into()),
                "XDG_CONFIG_HOME" => Some("/settings".into()),
                _ => None,
            },
            || panic!("must not inspect working directory"),
        )
        .unwrap();
        assert_eq!(config, PathBuf::from("/settings/kausway"));
        assert_eq!(data, PathBuf::from("/termux/home/.local/share/kausway"));
    }
    #[test]
    fn relative_environment_is_ignored_and_partial_xdg_is_respected() {
        let (config, data) = android_roots(
            |key| match key {
                "HOME" => Some("relative-home".into()),
                "XDG_CONFIG_HOME" => Some("relative-config".into()),
                "XDG_DATA_HOME" => Some("/data-root".into()),
                _ => None,
            },
            || Ok("/work".into()),
        )
        .unwrap();
        assert_eq!(config, PathBuf::from("/work/kausway"));
        assert_eq!(data, PathBuf::from("/data-root/kausway"));
    }
    #[test]
    fn inaccessible_working_directory_is_an_error_not_another_fallback() {
        let error = android_roots(
            |_| None,
            || Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied")),
        );
        assert!(matches!(error, Err(Error::Io(_))));
    }
}
