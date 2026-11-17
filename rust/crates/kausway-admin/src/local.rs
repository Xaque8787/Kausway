//! Same-user control channel; never exposed through HTTP.
use crate::{auth, server::Shared};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::Semaphore,
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub operation: String,
    pub config: PathBuf,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
}
struct SocketGuard(PathBuf);
impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
async fn frame(stream: &mut UnixStream) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        BufReader::new(stream)
            .take(16385)
            .read_until(b'\n', &mut bytes),
    )
    .await
    .map_err(|_| "Local request timed out")?
    .map_err(|_| "Could not read local request")?;
    if bytes.len() > 16384 || bytes.last() != Some(&b'\n') {
        return Err("Invalid local frame".into());
    }
    Ok(bytes)
}
pub fn bind(path: &Path) -> Result<UnixListener, String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => {
            fs::remove_file(path).map_err(|e| e.to_string())?
        }
        Ok(_) => return Err("Admin control path exists and is not a socket".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.to_string()),
    }
    let listener = UnixListener::bind(path).map_err(|e| e.to_string())?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    Ok(listener)
}
pub async fn serve(
    listener: UnixListener,
    path: PathBuf,
    config: PathBuf,
    shared: Shared,
    stop: tokio_util::sync::CancellationToken,
) -> Result<(), String> {
    let _socket = SocketGuard(path);
    let slots = Arc::new(Semaphore::new(16));
    let mut peers = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = stop.cancelled() => break,
            Some(_) = peers.join_next(), if !peers.is_empty() => {},
            accepted = listener.accept() => {
                let (mut stream, _) = accepted.map_err(|e| e.to_string())?;
                if stream.peer_cred().map(|c| c.uid()).ok() != Some(unsafe { libc::geteuid() }) { continue; }
                let Ok(permit) = slots.clone().try_acquire_owned() else { continue; };
                let shared = shared.clone(); let config = config.clone();
                peers.spawn(async move {
                    let _permit = permit;
                    let result: Result<Value, String> = async {
                        let bytes = frame(&mut stream).await?;
                        let input: Request = serde_json::from_slice(&bytes).map_err(|_| "Invalid local request")?;
                        if input.config != config { return Err("Selected configuration does not match the running admin".into()); }
                        let value = match input.operation.as_str() {
                            "setup-link" => json!({"url": shared.setup_link().await.map_err(|(_, e)| e.to_string())?, "expires_in_secs": auth::SETUP_LIFETIME.as_secs()}),
                            "credentials.set" | "credentials.reset" => {
                                shared.work(move |s| s.provision(&input.username, &input.password, input.operation == "credentials.reset")).await.map_err(|(_, e)| e.to_string())?;
                                json!({"saved": true})
                            },
                            _ => return Err("Unknown local operation".into()),
                        };
                        Ok(value)
                    }.await;
                    let reply = match result { Ok(value) => json!({"ok": true, "result": value}), Err(e) => json!({"ok": false, "error": e}) };
                    let mut bytes = serde_json::to_vec(&reply).unwrap(); bytes.push(b'\n');
                    let _ = tokio::time::timeout(Duration::from_secs(5), stream.write_all(&bytes)).await;
                });
            }
        }
    }
    if tokio::time::timeout(Duration::from_secs(5), async {
        while peers.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        peers.abort_all();
    }
    Ok(())
}
pub async fn request(data: &Path, request: Request) -> Result<Value, String> {
    let mut stream = UnixStream::connect(data.join("control.sock"))
        .await
        .map_err(|_| "Admin is not running or its local socket is unavailable")?;
    let mut bytes = serde_json::to_vec(&request).map_err(|_| "Could not encode local request")?;
    bytes.push(b'\n');
    if bytes.len() > 16384 {
        return Err("Local request is too large".into());
    }
    stream
        .write_all(&bytes)
        .await
        .map_err(|_| "Could not send local request")?;
    let response: Value =
        serde_json::from_slice(&frame(&mut stream).await?).map_err(|_| "Invalid admin response")?;
    if response["ok"] != true {
        return Err(response["error"]
            .as_str()
            .unwrap_or("Admin operation failed")
            .into());
    }
    Ok(response["result"].clone())
}
