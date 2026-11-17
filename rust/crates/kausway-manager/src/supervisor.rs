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
}
struct Call {
    operation: String,
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
    enabled: bool,
    observed: Observed,
    host: Option<OwnedHost>,
    admin: Option<Child>,
    admin_observed: Observed,
    failures: u32,
    retry: Option<Instant>,
    events: VecDeque<String>,
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
    }
    fn status(&self) -> Value {
        json!({"protocol": 1, "desired": if self.enabled {"enabled"} else {"disabled"},
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
            .arg("--control-listen")
            .arg(address.to_string())
            .arg("--control-allow-remote=false")
            .env("KAUSWAY_CONTROL_TOKEN", &token);
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
            let result = terminate(
                &mut host.child,
                Duration::from_secs(self.policy.shutdown_timeout_secs),
            )
            .await;
            self.observed = Observed::Stopped;
            if result? {
                self.event("host exceeded shutdown timeout and was forcibly terminated");
            } else {
                self.event("host stopped and reaped");
            }
        } else {
            self.observed = Observed::Stopped;
        }
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

fn policy(args: &RunArgs, mut saved: Manager) -> Result<Manager, String> {
    macro_rules! override_value {
        ($field:ident) => {
            if let Some(value) = args.$field {
                saved.$field = value;
            }
        };
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
            while matches!(stdout.read(&mut bytes).await, Ok(n) if n > 0) {}
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
async fn peer(mut stream: UnixStream, calls: mpsc::Sender<Call>) {
    let (written, when_written) = oneshot::channel();
    let response = async {
        let bytes = read_message(&mut stream).await?;
        let request: Request =
            serde_json::from_slice(&bytes).map_err(|_| "invalid request".to_string())?;
        let (reply, receive) = oneshot::channel();
        calls
            .send(Call {
                operation: request.operation,
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
    let mut supervisor = Supervisor {
        args,
        config,
        root,
        host_binary,
        admin_binary,
        policy,
        enabled,
        observed: Observed::Stopped,
        host: None,
        admin: None,
        admin_observed: Observed::Stopped,
        failures: 0,
        retry: None,
        events: VecDeque::new(),
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
            .arg(&socket_path);
        supervisor.admin = Some(
            spawn(&mut command)
                .map_err(|e| format!("cannot launch admin (Phase 3 binary required): {e}"))?,
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
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let result = loop {
        tokio::select! {
            _ = term.recv() => break Ok(()),
            _ = interrupt.recv() => break Ok(()),
            Some(call) = receive.recv() => {
                let result = supervisor.operation(&call.operation).await;
                let shutdown = call.operation == "shutdown" && result.is_ok();
                let response = match result {
                    Ok(()) => json!({"ok": true, "status": supervisor.status()}),
                    Err(error) => json!({"ok": false, "error": error, "status": supervisor.status()}),
                };
                let _ = call.reply.send(response);
                if shutdown {
                    let _ = timeout(Duration::from_secs(5), call.written).await;
                    break Ok(());
                }
            }
            result = listener.accept() => match result {
                Ok((stream, _)) => {
                    // Socket mode and parent directory restrict access; additionally enforce UID.
                    if stream.peer_cred().map(|c| c.uid()).ok() != Some(unsafe { libc::geteuid() }) { continue; }
                    if let Ok(permit) = slots.clone().try_acquire_owned() {
                        let send = send.clone(); tokio::spawn(async move { let _permit = permit; peer(stream, send).await; });
                    }
                }
                Err(e) => break Err(e.to_string()),
            },
            _ = tick.tick() => if let Err(e) = supervisor.tick().await { break Err(e); },
        }
    };
    let shutdown_result = supervisor.shutdown().await;
    result.and(shutdown_result)
}
