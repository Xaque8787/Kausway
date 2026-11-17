#![cfg(unix)]

use kausway_config::{Store, fsutil::DataDirLock};
use kausway_manager::request;
use serde_json::Value;
use std::{
    fs,
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

struct Harness {
    root: PathBuf,
    manager: Option<Child>,
    address: String,
}
impl Harness {
    fn new(mode: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "kw-mgr-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("mode"), mode).unwrap();
        fs::write(root.join("host-fixture"), include_str!("fixtures/host.py")).unwrap();
        fs::set_permissions(root.join("host-fixture"), fs::Permissions::from_mode(0o700)).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        drop(listener);
        Self {
            root,
            manager: None,
            address,
        }
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_kausway"));
        for (key, _) in std::env::vars().filter(|(key, _)| key.starts_with("KAUSWAY_")) {
            cmd.env_remove(key);
        }
        cmd.args(["run", "--config"])
            .arg(self.root.join("config.toml"))
            .arg("--data-root")
            .arg(&self.root)
            .arg("--host-binary")
            .arg(self.root.join("host-fixture"))
            .arg("--control-listen")
            .arg(&self.address)
            .args([
                "--shutdown-timeout-secs",
                "1",
                "--readiness-timeout-secs",
                "1",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd
    }
    async fn launch(&mut self, args: &[&str]) {
        self.manager = Some(self.command().args(args).spawn().unwrap());
        self.until(|_| true).await;
    }
    async fn until(&self, predicate: impl Fn(&Value) -> bool) -> Value {
        tokio::time::timeout(Duration::from_secs(12), async {
            loop {
                if let Ok(value) = request(&self.root, "status").await
                    && predicate(&value["status"])
                {
                    return value["status"].clone();
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .expect("manager status condition timed out")
    }
    async fn state(&self, state: &str) -> Value {
        self.until(|v| v["host"]["observed"] == state).await
    }
    async fn exit(&mut self) {
        request(&self.root, "shutdown").await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if self.manager.as_mut().unwrap().try_wait().unwrap().is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        self.manager = None;
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(child) = &mut self.manager {
            unsafe {
                libc::kill(child.id() as i32, libc::SIGTERM);
            }
            for _ in 0..250 {
                if child.try_wait().ok().flatten().is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn lifecycle_persistence_child_ownership_and_secret_redaction() {
    let mut h = Harness::new("normal");
    h.launch(&[]).await;
    h.state("stopped").await;
    request(&h.root, "host.start").await.unwrap();
    let running = h.state("running").await;
    assert_eq!(running["desired"], "enabled");
    assert!(!running.to_string().contains("SENSITIVE"));
    let pid = running["host"]["pid"].as_u64().unwrap();
    // Competing manager cannot remove or replace the existing manager socket.
    let competing = h.command().output().unwrap();
    assert!(!competing.status.success());
    assert!(request(&h.root, "status").await.is_ok());
    let mode = fs::metadata(h.root.join("manager/control.sock"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    request(&h.root, "host.restart").await.unwrap();
    let restarted = h.state("running").await;
    assert_ne!(pid, restarted["host"]["pid"].as_u64().unwrap());
    request(&h.root, "host.stop").await.unwrap();
    h.state("stopped").await;
    assert!(h.root.join("graceful").exists());
    assert!(
        !Store::new(h.root.join("config.toml"))
            .load_or_create()
            .unwrap()
            .config
            .manager
            .host_enabled
    );
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(h.state("stopped").await["desired"], "disabled");
    h.exit().await;
    assert!(!h.root.join("manager/control.sock").exists());
    // Disabled desired state survives a manager restart.
    h.launch(&[]).await;
    h.state("stopped").await;
    h.exit().await;
}

#[tokio::test]
async fn crashes_back_off_stop_at_threshold_and_can_be_retried_explicitly() {
    let mut h = Harness::new("crash");
    h.launch(&["--host-enabled", "--max-failures", "2"]).await;
    let status = h.until(|v| v["host"]["consecutive_failures"] == 2).await;
    assert_eq!(status["desired"], "enabled");
    assert!(status["host"]["retry_in_ms"].is_null());
    assert_eq!(
        fs::read_to_string(h.root.join("launches"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    fs::write(h.root.join("mode"), "normal").unwrap();
    request(&h.root, "host.start").await.unwrap();
    h.state("running").await;
    h.exit().await;
    // Manager shutdown preserves the enabled state for the next launch.
    assert!(
        Store::new(h.root.join("config.toml"))
            .load_or_create()
            .unwrap()
            .config
            .manager
            .host_enabled
    );
    h.launch(&[]).await;
    h.state("running").await;
    h.exit().await;
}

#[tokio::test]
async fn failed_authenticated_readiness_is_not_reported_as_running() {
    let mut h = Harness::new("unready");
    h.launch(&["--host-enabled", "--restart-on-failure=false"])
        .await;
    let status = h.state("failed").await;
    assert!(status["host"]["pid"].is_null());
    assert!(status["host"]["retry_in_ms"].is_null());
    assert!(
        status["diagnostics"]
            .to_string()
            .contains("readiness timed out")
    );
    h.exit().await;
}

#[tokio::test]
async fn uncooperative_child_is_killed_and_reaped() {
    let mut h = Harness::new("ignore_term");
    h.launch(&["--host-enabled"]).await;
    h.state("running").await;
    request(&h.root, "host.stop").await.unwrap();
    let status = h.state("stopped").await;
    assert!(
        status["diagnostics"]
            .to_string()
            .contains("forcibly terminated")
    );
    assert!(!h.root.join("graceful").exists());
    h.exit().await;
}

#[tokio::test]
async fn externally_owned_host_storage_is_never_adopted_or_modified() {
    let mut h = Harness::new("normal");
    fs::create_dir_all(h.root.join("host")).unwrap();
    fs::write(h.root.join("host/control.token"), "external-owner-token").unwrap();
    let _lock = DataDirLock::acquire(&h.root.join("host")).unwrap();
    h.launch(&["--host-enabled", "--restart-on-failure=false"])
        .await;
    let status = h.state("failed").await;
    assert!(status["host"]["pid"].is_null());
    assert_eq!(
        fs::read_to_string(h.root.join("host/control.token")).unwrap(),
        "external-owner-token"
    );
    assert!(!h.root.join("launches").exists());
    h.exit().await;
}

#[tokio::test]
async fn admin_child_survives_host_stop_and_is_reaped_on_signal() {
    let mut h = Harness::new("normal");
    let admin = h.root.join("admin-fixture");
    fs::write(
        &admin,
        "#!/usr/bin/env python3\nimport time\ntime.sleep(300)\n",
    )
    .unwrap();
    fs::set_permissions(&admin, fs::Permissions::from_mode(0o700)).unwrap();
    h.launch(&[
        "--host-enabled",
        "--admin-ui",
        "--admin-binary",
        admin.to_str().unwrap(),
    ])
    .await;
    let status = h.state("running").await;
    let admin_pid = status["admin"]["pid"].as_u64().unwrap();
    let host_pid = status["host"]["pid"].as_u64().unwrap();
    request(&h.root, "host.stop").await.unwrap();
    let status = h.state("stopped").await;
    assert_eq!(status["admin"]["pid"], admin_pid);
    let manager = h.manager.as_mut().unwrap();
    unsafe {
        libc::kill(manager.id() as i32, libc::SIGTERM);
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while manager.try_wait().unwrap().is_none() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    h.manager = None;
    for pid in [host_pid, admin_pid] {
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
    }
}

#[tokio::test]
async fn competing_starts_are_serialized_and_invalid_requests_do_not_change_state() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::UnixStream,
    };
    let mut h = Harness::new("normal");
    h.launch(&[]).await;
    let (first, second) = tokio::join!(
        request(&h.root, "host.start"),
        request(&h.root, "host.start")
    );
    first.unwrap();
    second.unwrap();
    h.state("running").await;
    assert_eq!(
        fs::read_to_string(h.root.join("launches"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert!(request(&h.root, "host.adopt").await.is_err());
    for bytes in [b"{bad json}\n".to_vec(), vec![b'x'; 20 * 1024]] {
        let mut socket = UnixStream::connect(h.root.join("manager/control.sock"))
            .await
            .unwrap();
        let _ = socket.write_all(&bytes).await;
        let mut reply = Vec::new();
        let _ = socket.read_to_end(&mut reply).await;
        let reply: Value = serde_json::from_slice(&reply).unwrap();
        assert_eq!(reply["ok"], false);
    }
    h.state("running").await;
    h.exit().await;
}
