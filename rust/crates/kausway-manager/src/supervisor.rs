use crate::RunArgs;
use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Limited};
use hyper_util::rt::TokioIo;
use kausway_config::{
    Manager, Paths, Store,
    fsutil::{DataDirLock, ensure_private_dir, write_private_atomic},
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    net::SocketAddr,
    os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    process::{Child, Command},
    sync::{Semaphore, mpsc, oneshot},
    time::{Instant, timeout},
};

const MAX_MESSAGE: u64 = 16 * 1024;
const MAX_EVENTS: usize = 64;
const STABLE_RUN: Duration = Duration::from_secs(60);

#[derive(Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Observed {
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    operation: String,
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
}
struct Call {
    request: Request,
    reply: oneshot::Sender<Value>,
    written: oneshot::Receiver<()>,
}
struct SocketGuard(PathBuf);
impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

struct OwnedHost {
    child: Child,
    address: SocketAddr,
    token: String,
    launched: Instant,
    ready_deadline: Instant,
}
struct Supervisor {
    args: RunArgs,
    config: PathBuf,
    root: PathBuf,
    host_binary: PathBuf,
    admin_binary: PathBuf,
    policy: Manager,
    manager_sources: std::collections::BTreeMap<String, kausway_config::Source>,
    admin_enabled: bool,
    admin_source: kausway_config::Source,
    enabled: bool,
    observed: Observed,
    host: Option<OwnedHost>,
    admin: Option<Child>,
    admin_observed: Observed,
    failures: u32,
    retry: Option<Instant>,
    events: VecDeque<String>,
    published: Arc<std::sync::Mutex<Value>>,
}
impl Supervisor {
    fn event(&mut self, message: impl Into<String>) {
        let mut message = message.into();
        // Keep status replies bounded even when an OS error includes a long path.
        if message.len() > 256 {
            let mut end = 256;
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message.truncate(end);
            message.push_str("...");
        }
        eprintln!("kausway-manager: {message}");
        if self.events.len() == MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(message);
        self.publish();
    }
    fn publish(&self) {
        if let Ok(mut value) = self.published.lock() {
            *value = self.status();
        }
    }
    fn status(&self) -> Value {
        json!({"protocol": 1, "config_path": self.config, "host_data_dir": self.root.join("host"),
            "control_address": self.host.as_ref().map(|h| h.address),
            "restart_policy": self.policy.restart_policy, "application_name": self.policy.application_name,
            "effective_manager": self.policy, "manager_sources": self.manager_sources, "admin_enabled": self.admin_enabled, "admin_source": self.admin_source,
            "desired": if self.enabled {"enabled"} else {"disabled"},
            "host": {"observed": self.observed, "pid": self.host.as_ref().and_then(|h| h.child.id()),
                "consecutive_failures": self.failures, "retry_in_ms": self.retry.map(|t| t.saturating_duration_since(Instant::now()).as_millis() as u64)},
            "admin": {"observed": self.admin_observed, "pid": self.admin.as_ref().and_then(|a| a.id())},
            "diagnostics": self.events})
    }
    fn persist_enabled(&mut self, enabled: bool) -> Result<(), String> {
        let store = Store::new(&self.config);
        let mut snapshot = store.load_or_create().map_err(|e| e.to_string())?;
        snapshot.config.manager.host_enabled = enabled;
        store
            .save(&snapshot.config, &snapshot.revision)
            .map_err(|e| e.to_string())?;
        self.enabled = enabled;
        self.publish();
        Ok(())
    }
    fn launch_host(&mut self) -> Result<(), String> {
        if self.host.is_some() {
            return Ok(());
        }
        let saved = Store::new(&self.config)
            .load_or_create()
            .map_err(|e| e.to_string())?
            .config;
        self.policy = policy(&self.args, saved.manager)?;
        self.manager_sources = manager_sources(&self.args, &self.config);
        let address: SocketAddr = self
            .args
            .control_listen
            .map(|a| a.to_string())
            .or(saved.host.control.listen)
            .unwrap_or_else(|| "127.0.0.1:47950".into())
            .parse()
            .map_err(|_| "managed host requires a loopback control IP:port (not off)")?;
        if !address.ip().is_loopback() || address.port() == 0 {
            return Err(
                "managed host control listener must use a concrete loopback IP and nonzero port"
                    .into(),
            );
        }
        let host_dir = self.root.join("host");
        ensure_private_dir(&host_dir).map_err(|e| e.to_string())?;
        // Refuse existing owners before changing credentials; the child also takes this lock.
        let lock = DataDirLock::acquire(&host_dir).map_err(|e| e.to_string())?;
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        write_private_atomic(&host_dir.join("control.token"), token.as_bytes())
            .map_err(|e| e.to_string())?;
        let mut command = Command::new(&self.host_binary);
        command
            .arg("serve")
            .arg("--config")
            .arg(&self.config)
            .arg("--data-dir")
            .arg(&host_dir)
            .arg("--managed")
            .env("KAUSWAY_CONTROL_TOKEN", &token);
        if self.args.sources.get("control_listen") == Some(&kausway_config::Source::Cli) {
            command.arg("--control-listen").arg(address.to_string());
        }
        for (flag, value) in [
            ("--origin", &self.args.origin),
            ("--relay-url", &self.args.relay_url),
            ("--policy", &self.args.policy),
            ("--websockets", &self.args.websockets),
            ("--websocket-origin", &self.args.websocket_origin),
        ] {
            if let Some(value) = value {
                command.arg(flag).arg(value);
            }
        }
        // Avoid retaining a parent-side lock that would prevent the child taking ownership.
        drop(lock);
        let child = spawn(&mut command).map_err(|e| format!("host launch failed: {e}"))?;
        let launched = Instant::now();
        self.host = Some(OwnedHost {
            child,
            address,
            token,
            launched,
            ready_deadline: launched + Duration::from_secs(self.policy.readiness_timeout_secs),
        });
        self.observed = Observed::Starting;
        self.retry = None;
        self.event("host child launched; waiting for authenticated readiness");
        Ok(())
    }
    fn failure(&mut self, reason: String) {
        self.observed = Observed::Failed;
        self.failures = self.failures.saturating_add(1);
        self.event(reason);
        self.retry = None;
        if self.enabled
            && self.policy.restart_on_failure
            && self.failures < self.policy.max_failures
        {
            let delay = self
                .policy
                .restart_backoff_initial_secs
                .saturating_mul(1u64 << self.failures.saturating_sub(1).min(31))
                .min(self.policy.restart_backoff_max_secs);
            self.retry = Some(Instant::now() + Duration::from_secs(delay));
            self.event(format!("host retry scheduled in {delay}s"));
        } else {
            self.event("automatic host retries stopped; explicit start/restart required");
        }
    }
    async fn stop_host(&mut self) -> Result<(), String> {
        self.retry = None;
        if let Some(mut host) = self.host.take() {
            self.observed = Observed::Stopping;
            self.publish();
            let result = terminate(
                &mut host.child,
                Duration::from_secs(self.policy.shutdown_timeout_secs),
            )
            .await;
            self.observed = Observed::Stopped;
            let forced = match result {
                Ok(value) => value,
                Err(error) => {
                    self.observed = Observed::Failed;
                    self.event("host shutdown failed");
                    return Err(error);
                }
            };
            if forced {
                self.event("host exceeded shutdown timeout and was forcibly terminated");
            } else {
                self.event("host stopped and reaped");
            }
        } else {
            self.observed = Observed::Stopped;
        }
        self.publish();
        Ok(())
    }
    async fn tick(&mut self) -> Result<(), String> {
        if let Some(admin) = &mut self.admin
            && let Some(status) = admin.try_wait().map_err(|e| e.to_string())?
        {
            self.admin = None;
            self.admin_observed = Observed::Failed;
            self.event(format!(
                "admin child exited: {status}; host supervision remains available"
            ));
        }
        if let Some(host) = &mut self.host {
            if let Some(status) = host.child.try_wait().map_err(|e| e.to_string())? {
                if self.observed == Observed::Running && host.launched.elapsed() >= STABLE_RUN {
                    self.failures = 0;
                }
                self.host = None;
                self.failure(format!("host child exited unexpectedly: {status}"));
            } else if self.observed == Observed::Starting {
                if ready(host.address, &host.token).await {
                    self.observed = Observed::Running;
                    self.event("host authenticated and ready");
                } else if Instant::now() >= host.ready_deadline {
                    self.stop_host().await?;
                    self.failure("host readiness timed out".into());
                }
            }
        }
        if self.host.is_none()
            && self.retry.is_some_and(|t| Instant::now() >= t)
            && let Err(e) = self.launch_host()
        {
            self.failure(e);
        }
        Ok(())
    }
    async fn operation(&mut self, operation: &str) -> Result<(), String> {
        match operation {
            "status" | "shutdown" => Ok(()),
            "host.stop" => {
                self.persist_enabled(false)?;
                self.stop_host().await
            }
            "host.start" | "host.restart" => {
                self.persist_enabled(true)?;
                if operation == "host.restart" {
                    self.stop_host().await?;
                }
                if self.host.is_none() {
                    self.failures = 0;
                    self.retry = None;
                    if let Err(e) = self.launch_host() {
                        self.failure(e.clone());
                        return Err(e);
                    }
                }
                Ok(())
            }
            _ => Err("unknown operation".into()),
        }
    }
    async fn dispatch(&mut self, request: Request) -> Result<Value, String> {
        let admin_operation = match request.operation.as_str() {
            "admin.open" => Some("entry-link"),
            "admin.status" => Some("status"),
            "admin.credentials.initialize" => Some("credentials.set"),
            "admin.credentials.reset" => Some("credentials.reset"),
            _ => None,
        };
        if let Some(operation) = admin_operation {
            if self.admin.is_none() {
                return Err(
                    "Admin is not running; enable --admin-ui and wait for readiness".into(),
                );
            }
            let result = admin_request(
                &self.root.join("admin"),
                &self.config,
                operation,
                &request.username,
                &request.password,
            )
            .await?;
            return Ok(json!({"ok":true,"result":result,"status":self.status()}));
        }
        self.operation(&request.operation).await?;
        Ok(json!({"ok":true,"status":self.status()}))
    }
    async fn shutdown(&mut self) -> Result<(), String> {
        let host_result = self.stop_host().await;
        let admin_result = if let Some(mut admin) = self.admin.take() {
            terminate(
                &mut admin,
                Duration::from_secs(self.policy.shutdown_timeout_secs),
            )
            .await
            .map(|_| ())
        } else {
            Ok(())
        };
        self.admin_observed = Observed::Stopped;
        host_result.and(admin_result)
    }
}

fn manager_sources(
    args: &RunArgs,
    config: &Path,
) -> std::collections::BTreeMap<String, kausway_config::Source> {
    serde_json::to_value(Manager::default())
        .unwrap()
        .as_object()
        .unwrap()
        .keys()
        .map(|key| {
            (
                key.clone(),
                args.sources.get(key).copied().unwrap_or_else(|| {
                    kausway_config::administration::file_source(config, &format!("manager.{key}"))
                }),
            )
        })
        .collect()
}

fn policy(args: &RunArgs, mut saved: Manager) -> Result<Manager, String> {
    macro_rules! override_value {
        ($field:ident) => {
            if let Some(value) = args.$field {
                saved.$field = value;
            }
        };
    }
    if let Some(value) = &args.restart_policy {
        saved.restart_policy = if value == "application" {
            kausway_config::RestartPolicy::Application
        } else {
            kausway_config::RestartPolicy::Host
        };
    }
    if let Some(value) = &args.application_name {
        saved.application_name = value.clone();
    }
    override_value!(shutdown_timeout_secs);
    override_value!(restart_on_failure);
    override_value!(restart_backoff_initial_secs);
    override_value!(restart_backoff_max_secs);
    override_value!(max_failures);
    override_value!(readiness_timeout_secs);
    let config = kausway_config::Config {
        manager: saved.clone(),
        ..Default::default()
    };
    config.validate().map_err(|e| e.to_string())?;
    Ok(saved)
}

fn spawn(command: &mut Command) -> std::io::Result<Child> {
    spawn_child(command, false, false)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminStartupEvent {
    event: String,
    url: String,
    expires_in_secs: u64,
}
fn forward_admin_event(bytes: &[u8]) {
    let Ok(event) = serde_json::from_slice::<AdminStartupEvent>(bytes) else {
        return;
    };
    let Some((base, token)) = event.url.split_once("#token=") else {
        return;
    };
    let Ok(uri) = base.parse::<hyper::Uri>() else {
        return;
    };
    let local = uri.host().is_some_and(|h| {
        h == "localhost"
            || h.trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if event.event == "setup_link"
        && event.expires_in_secs == 900
        && uri.scheme_str() == Some("http")
        && local
        && uri.path() == "/setup"
        && uri.query().is_none()
        && token.len() == 64
        && token.bytes().all(|c| c.is_ascii_hexdigit())
    {
        // Intentionally sensitive startup output, never retained in diagnostics/status.
        eprintln!(
            "Kausway administrator setup (expires in 15 minutes): {}",
            event.url
        );
    }
}
fn spawn_child(
    command: &mut Command,
    admin_events: bool,
    forward_setup: bool,
) -> std::io::Result<Child> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    // Arbitrary child output can contain invite links, tokens and passwords. Drain it
    // in fixed buffers; expose only manager-generated lifecycle diagnostics.
    if let Some(mut stdout) = child.stdout.take() {
        tokio::spawn(async move {
            let mut bytes = [0u8; 4096];
            let mut line = Vec::new();
            let mut oversized = false;
            while let Ok(n) = stdout.read(&mut bytes).await {
                if n == 0 {
                    break;
                }
                if !admin_events || !forward_setup {
                    continue;
                }
                for byte in &bytes[..n] {
                    if *byte == b'\n' {
                        if !oversized {
                            forward_admin_event(&line);
                        }
                        line.clear();
                        oversized = false;
                    } else if line.len() < 4096 && !oversized {
                        line.push(*byte);
                    } else {
                        line.clear();
                        oversized = true;
                    }
                }
            }
        });
    }
    if let Some(mut stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut bytes = [0u8; 4096];
            while matches!(stderr.read(&mut bytes).await, Ok(n) if n > 0) {}
        });
    }
    Ok(child)
}

async fn terminate(child: &mut Child, grace: Duration) -> Result<bool, String> {
    if child.try_wait().map_err(|e| e.to_string())?.is_some() {
        return Ok(false);
    }
    if let Some(pid) = child.id() {
        // This PID belongs to an unreaped Child, so it cannot have been recycled.
        let result = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if result != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
            child.kill().await.map_err(|e| e.to_string())?;
            return Ok(true);
        }
    }
    match timeout(grace, child.wait()).await {
        Ok(result) => {
            result.map_err(|e| e.to_string())?;
            Ok(false)
        }
        Err(_) => {
            child.kill().await.map_err(|e| e.to_string())?;
            Ok(true)
        }
    }
}

async fn ready(address: SocketAddr, token: &str) -> bool {
    timeout(Duration::from_millis(250), async {
        let stream = tokio::net::TcpStream::connect(address).await.ok()?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .ok()?;
        let driver = tokio::spawn(async {
            let _ = connection.await;
        });
        // Abort the driver even when the request or body fails.
        struct Driver(tokio::task::JoinHandle<()>);
        impl Drop for Driver {
            fn drop(&mut self) {
                self.0.abort();
            }
        }
        let _driver = Driver(driver);
        let request = hyper::Request::builder()
            .uri("/v1/status")
            .header("Host", address.to_string())
            .header("Authorization", format!("Bearer {token}"))
            .body(Empty::<Bytes>::new())
            .ok()?;
        let response = sender.send_request(request).await.ok()?;
        if response.status() != hyper::StatusCode::OK {
            return None;
        }
        let body = Limited::new(response.into_body(), 64 * 1024)
            .collect()
            .await
            .ok()?
            .to_bytes();
        let value: Value = serde_json::from_slice(&body).ok()?;
        (value["control_api"] == 1 && value["endpoint_id"].as_str().is_some()).then_some(())
    })
    .await
    .is_ok_and(|v| v.is_some())
}

async fn read_message(stream: &mut (impl tokio::io::AsyncRead + Unpin)) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let n = timeout(
        Duration::from_secs(5),
        BufReader::new(stream)
            .take(MAX_MESSAGE + 1)
            .read_until(b'\n', &mut bytes),
    )
    .await
    .map_err(|_| "control read timed out")?
    .map_err(|e| e.to_string())?;
    if n == 0 || n as u64 > MAX_MESSAGE || bytes.last() != Some(&b'\n') {
        return Err("invalid control frame".into());
    }
    Ok(bytes)
}
async fn peer(
    mut stream: UnixStream,
    calls: mpsc::Sender<Call>,
    published: Arc<std::sync::Mutex<Value>>,
) {
    let (written, when_written) = oneshot::channel();
    let response = async {
        let bytes = read_message(&mut stream).await?;
        let request: Request =
            serde_json::from_slice(&bytes).map_err(|_| "invalid request".to_string())?;
        if request.operation == "status" {
            return published
                .lock()
                .map(|value| json!({"ok":true,"status":value.clone()}))
                .map_err(|_| "Manager status unavailable".to_string());
        }
        let (reply, receive) = oneshot::channel();
        calls
            .send(Call {
                request,
                reply,
                written: when_written,
            })
            .await
            .map_err(|_| "manager stopping".to_string())?;
        receive.await.map_err(|_| "manager stopping".to_string())
    }
    .await
    .unwrap_or_else(|e: String| json!({"ok": false, "error": e}));
    if let Ok(mut bytes) = serde_json::to_vec(&response) {
        bytes.push(b'\n');
        let _ = timeout(Duration::from_secs(5), stream.write_all(&bytes)).await;
    }
    let _ = written.send(());
}

pub async fn request(root: &Path, operation: &str) -> Result<Value, String> {
    let mut stream = UnixStream::connect(root.join("manager/control.sock"))
        .await
        .map_err(|e| format!("cannot connect to manager: {e}"))?;
    let mut bytes =
        serde_json::to_vec(&json!({"operation": operation})).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    stream.write_all(&bytes).await.map_err(|e| e.to_string())?;
    // Lifecycle operations can include a configured graceful shutdown timeout.
    let bytes = timeout(Duration::from_secs(300), async {
        let mut bytes = Vec::new();
        BufReader::new(&mut stream)
            .take(64 * 1024)
            .read_until(b'\n', &mut bytes)
            .await
            .map_err(|e| e.to_string())?;
        Ok::<_, String>(bytes)
    })
    .await
    .map_err(|_| "manager response timed out")??;
    let response: Value = serde_json::from_slice(&bytes).map_err(|_| "invalid manager response")?;
    if response["ok"] != true {
        return Err(response["error"]
            .as_str()
            .unwrap_or("manager request failed")
            .into());
    }
    Ok(response)
}

pub async fn run(args: RunArgs) -> Result<(), String> {
    // Register signals before launching any children.
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|e| e.to_string())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|e| e.to_string())?;
    let defaults = if args.config.is_none() || args.data_root.is_none() {
        Some(Paths::defaults().map_err(|e| e.to_string())?)
    } else {
        None
    };
    let config = args
        .config
        .clone()
        .unwrap_or_else(|| defaults.as_ref().unwrap().config.clone());
    let saved = Store::new(&config)
        .load_or_create()
        .map_err(|e| e.to_string())?
        .config;
    let config = fs::canonicalize(config).map_err(|e| e.to_string())?;
    let root = args
        .data_root
        .clone()
        .unwrap_or_else(|| defaults.as_ref().unwrap().data_root.clone());
    ensure_private_dir(&root).map_err(|e| e.to_string())?;
    let root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    let manager_dir = root.join("manager");
    ensure_private_dir(&manager_dir).map_err(|e| e.to_string())?;
    let _lock: File = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(manager_dir.join("manager.lock"))
        .map_err(|e| e.to_string())?;
    _lock
        .try_lock()
        .map_err(|_| "another manager is already using this suite data root")?;
    let socket_path = manager_dir.join("control.sock");
    match fs::symlink_metadata(&socket_path) {
        Ok(meta) if meta.file_type().is_socket() => {
            fs::remove_file(&socket_path).map_err(|e| e.to_string())?
        }
        Ok(_) => return Err("manager control path exists and is not a socket".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.to_string()),
    }
    let listener = UnixListener::bind(&socket_path).map_err(|e| e.to_string())?;
    let _socket = SocketGuard(socket_path.clone());
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    let sibling = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .ok_or("no executable directory")?
        .to_path_buf();
    let enabled = args.host_enabled.unwrap_or(saved.manager.host_enabled);
    let admin_enabled = args.admin_ui.unwrap_or(saved.admin.enabled);
    let host_binary = args
        .host_binary
        .clone()
        .unwrap_or_else(|| sibling.join("kausway-host"));
    let admin_binary = args
        .admin_binary
        .clone()
        .unwrap_or_else(|| sibling.join("kausway-admin"));
    let policy = policy(&args, saved.manager)?;
    let sources = manager_sources(&args, &config);
    let admin_source =
        args.sources.get("admin_ui").copied().unwrap_or_else(|| {
            kausway_config::administration::file_source(&config, "admin.enabled")
        });
    let mut supervisor = Supervisor {
        args,
        config,
        root,
        host_binary,
        admin_binary,
        policy,
        manager_sources: sources,
        admin_enabled,
        admin_source,
        enabled,
        observed: Observed::Stopped,
        host: None,
        admin: None,
        admin_observed: Observed::Stopped,
        failures: 0,
        retry: None,
        events: VecDeque::new(),
        published: Arc::new(std::sync::Mutex::new(Value::Null)),
    };
    if admin_enabled {
        ensure_private_dir(&supervisor.root.join("admin")).map_err(|e| e.to_string())?;
        let mut command = Command::new(&supervisor.admin_binary);
        command
            .arg("--config")
            .arg(&supervisor.config)
            .arg("--data-dir")
            .arg(supervisor.root.join("admin"))
            .arg("--manager-socket")
            .arg(&socket_path)
            .arg("--host-data-dir")
            .arg(supervisor.root.join("host"))
            .env("KAUSWAY_ADMIN_STARTUP_EVENTS", "1");
        supervisor.admin = Some(
            spawn_child(&mut command, true, !supervisor.args.integration_stdio)
                .map_err(|e| format!("cannot launch admin: {e}"))?,
        );
        supervisor.admin_observed = Observed::Running;
        supervisor.event("admin child launched (process status only)");
    }
    if enabled && let Err(e) = supervisor.launch_host() {
        supervisor.failure(e);
    }
    supervisor.event("manager control ready");
    let (send, mut receive) = mpsc::channel::<Call>(32);
    let slots = Arc::new(Semaphore::new(32));
    let (stop_listener, mut listener_stopped) = tokio::sync::watch::channel(false);
    let published = supervisor.published.clone();
    let accept_send = send.clone();
    let mut accept_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = listener_stopped.changed() => return Ok::<(), String>(()),
                result = listener.accept() => {
                    let (stream,_) = result.map_err(|e| e.to_string())?;
                    if stream.peer_cred().map(|c| c.uid()).ok() != Some(unsafe { libc::geteuid() }) { continue; }
                    if let Ok(permit) = slots.clone().try_acquire_owned() {
                        let send = accept_send.clone(); let published = published.clone();
                        tokio::spawn(async move { let _permit = permit; peer(stream,send,published).await; });
                    }
                }
            }
        }
    });
    let mut integration_task = if supervisor.args.integration_stdio {
        Some(tokio::spawn(integration_stdio(
            send.clone(),
            supervisor.published.clone(),
        )))
    } else {
        None
    };
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let result = loop {
        tokio::select! {
            _ = term.recv() => break Ok(()),
            _ = interrupt.recv() => break Ok(()),
            Some(call) = receive.recv() => {
                let shutdown = call.request.operation == "shutdown";
                let result = supervisor.dispatch(call.request).await;
                let shutdown = shutdown && result.is_ok();
                let response = match result {
                    Ok(value) => value,
                    Err(error) => json!({"ok": false, "error": error, "status": supervisor.status()}),
                };
                let _ = call.reply.send(response);
                if shutdown {
                    let _ = timeout(Duration::from_secs(5), call.written).await;
                    break Ok(());
                }
            }
            result = async { integration_task.as_mut().unwrap().await }, if integration_task.is_some() => break result.map_err(|e| e.to_string()).and_then(|v| v),
            result = &mut accept_task => break result.map_err(|e| e.to_string()).and_then(|v| v),
            _ = tick.tick() => if let Err(e) = supervisor.tick().await { break Err(e); },
        }
    };
    let shutdown_result = supervisor.shutdown().await;
    if let Some(task) = integration_task {
        task.abort();
    }
    let _ = stop_listener.send(true);
    if !accept_task.is_finished() {
        let _ = accept_task.await;
    }
    result.and(shutdown_result)
}

async fn admin_request(
    data: &Path,
    config: &Path,
    operation: &str,
    username: &str,
    password: &str,
) -> Result<Value, String> {
    timeout(Duration::from_secs(10), async {
        let mut stream = UnixStream::connect(data.join("control.sock"))
            .await
            .map_err(|_| "Admin local interface unavailable; wait for readiness")?;
        let mut bytes = serde_json::to_vec(
            &json!({"operation":operation,"config":config,"username":username,"password":password}),
        )
        .map_err(|_| "Could not encode admin operation")?;
        bytes.push(b'\n');
        if bytes.len() as u64 > MAX_MESSAGE {
            return Err("Admin operation exceeds frame limit".into());
        }
        stream
            .write_all(&bytes)
            .await
            .map_err(|_| "Could not send admin operation")?;
        let response: Value = serde_json::from_slice(&read_message(&mut stream).await?)
            .map_err(|_| "Invalid admin response")?;
        if response["ok"] != true {
            return Err(response["error"]
                .as_str()
                .unwrap_or("Admin operation failed")
                .into());
        }
        Ok(response["result"].clone())
    })
    .await
    .map_err(|_| "Admin operation timed out")?
}

async fn integration_stdio(
    calls: mpsc::Sender<Call>,
    published: Arc<std::sync::Mutex<Value>>,
) -> Result<(), String> {
    let mut input = BufReader::new(PrivatePipe::new(libc::STDIN_FILENO)?);
    let mut output = PrivatePipe::new(libc::STDOUT_FILENO)?;
    let status = published
        .lock()
        .map_err(|_| "Manager status unavailable")?
        .clone();
    write_frame(
        &mut output,
        json!({"event":"ready","protocol":1,"status":status}),
    )
    .await?;
    loop {
        let mut bytes = Vec::new();
        let count = (&mut input)
            .take(MAX_MESSAGE + 1)
            .read_until(b'\n', &mut bytes)
            .await
            .map_err(|_| "Integration input failed")?;
        if count == 0 {
            return Ok(());
        }
        if count as u64 > MAX_MESSAGE || bytes.last() != Some(&b'\n') {
            return Err("Invalid or oversized integration frame".into());
        }
        let request: Request = match serde_json::from_slice(&bytes) {
            Ok(request) => request,
            Err(_) => {
                write_frame(
                    &mut output,
                    json!({"ok":false,"id":null,"error":"Invalid integration request"}),
                )
                .await?;
                continue;
            }
        };
        let Some(id) = request.id else {
            write_frame(
                &mut output,
                json!({"ok":false,"id":null,"error":"An integer request id is required"}),
            )
            .await?;
            continue;
        };
        let shutdown = request.operation == "shutdown";
        let (written, when_written) = oneshot::channel();
        let mut response = if request.operation == "status" {
            let status = published
                .lock()
                .map_err(|_| "Manager status unavailable")?
                .clone();
            json!({"ok":true,"status":status})
        } else {
            let (reply, receive) = oneshot::channel();
            calls
                .send(Call {
                    request,
                    reply,
                    written: when_written,
                })
                .await
                .map_err(|_| "Manager stopping")?;
            receive.await.map_err(|_| "Manager stopping")?
        };
        response["id"] = json!(id);
        let successful = response["ok"] == true;
        write_frame(&mut output, response).await?;
        let _ = written.send(());
        if shutdown && successful {
            return Ok(());
        }
    }
}
async fn write_frame(
    output: &mut (impl tokio::io::AsyncWrite + Unpin),
    value: Value,
) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(&value).map_err(|_| "Could not encode integration reply")?;
    bytes.push(b'\n');
    timeout(Duration::from_secs(5), async {
        output
            .write_all(&bytes)
            .await
            .map_err(|_| "Integration output closed")?;
        output
            .flush()
            .await
            .map_err(|_| "Integration output closed".to_string())
    })
    .await
    .map_err(|_| "Integration output blocked")?
}

/// Nonblocking inherited pipes. Tokio's blocking stdin/stdout adapters cannot be
/// cancelled on parent disconnect/backpressure and can prevent runtime shutdown.
struct PrivatePipe(tokio::io::unix::AsyncFd<File>);
impl PrivatePipe {
    fn new(fd: libc::c_int) -> Result<Self, String> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let duplicated = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
        if duplicated < 0 {
            return Err("Could not duplicate private integration descriptor".into());
        }
        let file = unsafe { File::from_raw_fd(duplicated) };
        let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
        {
            return Err("Could not configure private nonblocking integration pipe".into());
        }
        // SAFETY: File owns this duplicated descriptor exclusively and keeps it
        // open and unchanged for the complete AsyncFd lifetime.
        unsafe { tokio::io::unix::AsyncFd::register(file) }
            .map(Self)
            .map_err(|_| "Integration stdio requires inherited pipes or sockets".into())
    }
}
impl tokio::io::AsyncRead for PrivatePipe {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        use std::{
            os::fd::AsRawFd,
            task::{Poll, ready},
        };
        if buffer.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            let mut guard = ready!(self.0.poll_read_ready(cx))?;
            match guard.try_io(|inner| {
                let free = buffer.initialize_unfilled();
                let read = unsafe {
                    libc::read(
                        inner.get_ref().as_raw_fd(),
                        free.as_mut_ptr().cast(),
                        free.len(),
                    )
                };
                if read < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                buffer.advance(read as usize);
                Ok(())
            }) {
                Ok(result) => return Poll::Ready(result),
                Err(_) => continue,
            }
        }
    }
}
impl tokio::io::AsyncWrite for PrivatePipe {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        use std::{
            os::fd::AsRawFd,
            task::{Poll, ready},
        };
        loop {
            let mut guard = ready!(self.0.poll_write_ready(cx))?;
            match guard.try_io(|inner| {
                let written = unsafe {
                    libc::write(
                        inner.get_ref().as_raw_fd(),
                        bytes.as_ptr().cast(),
                        bytes.len(),
                    )
                };
                if written < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(written as usize)
                }
            }) {
                Ok(result) => return Poll::Ready(result),
                Err(_) => continue,
            }
        }
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}
