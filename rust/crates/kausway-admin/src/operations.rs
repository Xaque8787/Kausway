//! Explicit local operations, not an arbitrary URL/request proxy.
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper_util::rt::TokioIo;
use kausway_config::{
    Store,
    administration::{self, Update},
    fsutil::{DataDirLock, ensure_private_dir},
};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

type Result<T> = std::result::Result<T, (u16, String)>;
fn failure(message: &str) -> (u16, String) {
    (503, message.into())
}
fn absolute(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.into()
        } else {
            std::env::current_dir().unwrap_or_default().join(path)
        }
    })
}
pub struct Operations {
    config: PathBuf,
    admin_data: PathBuf,
    host_data: PathBuf,
    control: Option<SocketAddr>,
    token: Option<String>,
    manager: Option<PathBuf>,
    restart_policy: String,
    application_name: String,
    admin_effective: Value,
    mutations: tokio::sync::Mutex<()>,
}
impl Operations {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: PathBuf,
        admin_data: PathBuf,
        host_data: PathBuf,
        control: Option<SocketAddr>,
        token: Option<String>,
        manager: Option<PathBuf>,
        restart_policy: String,
        application_name: String,
        admin_effective: Value,
    ) -> Self {
        Self {
            config: absolute(&config),
            admin_data: absolute(&admin_data),
            host_data: absolute(&host_data),
            control,
            token,
            manager,
            restart_policy,
            application_name,
            admin_effective,
            mutations: tokio::sync::Mutex::new(()),
        }
    }
    #[cfg(unix)]
    async fn manager_call(&self, operation: &str) -> Result<Value> {
        let path = self
            .manager
            .as_ref()
            .ok_or_else(|| failure("No manager configured; the host is externally supervised."))?;
        tokio::time::timeout(Duration::from_secs(125), async {
            let mut stream = tokio::net::UnixStream::connect(path)
                .await
                .map_err(|_| failure("Configured manager is unavailable."))?;
            let request = format!("{}\n", json!({"operation":operation}));
            stream
                .write_all(request.as_bytes())
                .await
                .map_err(|_| failure("Manager request failed."))?;
            let mut bytes = Vec::new();
            BufReader::new(&mut stream)
                .take(65537)
                .read_until(b'\n', &mut bytes)
                .await
                .map_err(|_| failure("Manager response failed."))?;
            if bytes.len() > 65536 {
                return Err(failure("Manager response exceeded its limit."));
            }
            let value: Value =
                serde_json::from_slice(&bytes).map_err(|_| failure("Invalid manager response."))?;
            if value["ok"] != true {
                return Err((
                    409,
                    value["error"]
                        .as_str()
                        .unwrap_or("Manager operation failed.")
                        .into(),
                ));
            }
            let status = value["status"].clone();
            if status["config_path"].as_str().map(PathBuf::from) != Some(self.config.clone())
                || status["host_data_dir"].as_str().map(PathBuf::from)
                    != Some(self.host_data.clone())
            {
                return Err(failure(
                    "Configured manager owns a different configuration or host directory.",
                ));
            }
            Ok(status)
        })
        .await
        .map_err(|_| failure("Manager request timed out."))?
    }
    #[cfg(not(unix))]
    async fn manager_call(&self, _: &str) -> Result<Value> {
        Err(failure("Manager IPC is not available on this platform."))
    }
    async fn manager_status(&self) -> Result<Value> {
        // Keep status failures short, distinct from a deliberate graceful stop timeout.
        tokio::time::timeout(Duration::from_secs(2), self.manager_call("status"))
            .await
            .map_err(|_| failure("Configured manager is unavailable."))?
    }
    async fn target(&self) -> Result<(SocketAddr, String)> {
        let manager = if self.manager.is_some() {
            Some(self.manager_status().await?)
        } else {
            None
        };
        let address = if let Some(status) = &manager {
            status["control_address"]
                .as_str()
                .ok_or_else(|| failure("Managed host is stopped or unavailable."))?
                .parse()
                .map_err(|_| failure("Invalid manager host listener."))?
        } else if let Some(address) = self.control {
            address
        } else {
            let path = self.host_data.join("control-address.json");
            let config = self.config.clone();
            tokio::task::spawn_blocking(move || {
                if let Ok(bytes) = std::fs::read(path)
                    && bytes.len() < 4096
                    && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
                    && let Some(listen) = value["listen"].as_str()
                {
                    return listen
                        .parse()
                        .map_err(|_| failure("Invalid published control listener."));
                }
                let saved = Store::new(config)
                    .load_or_create()
                    .map_err(|_| failure("Could not load host configuration."))?;
                saved
                    .config
                    .host
                    .control
                    .listen
                    .unwrap_or_else(|| "127.0.0.1:47950".into())
                    .parse()
                    .map_err(|_| failure("Host control API is disabled or invalid."))
            })
            .await
            .map_err(|_| failure("Could not resolve host listener."))??
        };
        if !address.ip().is_loopback() || address.port() == 0 {
            return Err(failure(
                "Admin only connects to a concrete loopback host control listener.",
            ));
        }
        let token = if manager.is_none() && self.token.is_some() {
            self.token.clone().unwrap()
        } else {
            let path = self.host_data.join("control.token");
            tokio::task::spawn_blocking(move || {
                if std::fs::metadata(&path)
                    .map_err(|_| failure("Host control token is unavailable."))?
                    .len()
                    > 4096
                {
                    return Err(failure("Host control token is invalid."));
                }
                std::fs::read_to_string(path)
                    .map(|v| v.trim().to_string())
                    .map_err(|_| failure("Host control token is unavailable."))
            })
            .await
            .map_err(|_| failure("Could not read host control token."))??
        };
        if token.len() < 32 || token.len() > 4096 {
            return Err(failure("Host control token is invalid."));
        }
        Ok((address, token))
    }
    async fn host(&self, method: &str, path: &str, body: Value) -> Result<Value> {
        let (address, token) = self.target().await?;
        tokio::time::timeout(Duration::from_secs(5), async {
            let stream = tokio::net::TcpStream::connect(address)
                .await
                .map_err(|_| failure("Host is stopped or its control API is unreachable."))?;
            let (mut sender, connection) =
                hyper::client::conn::http1::handshake(TokioIo::new(stream))
                    .await
                    .map_err(|_| failure("Host connection failed."))?;
            let driver = tokio::spawn(async {
                let _ = connection.await;
            });
            struct Guard(tokio::task::JoinHandle<()>);
            impl Drop for Guard {
                fn drop(&mut self) {
                    self.0.abort();
                }
            }
            let _guard = Guard(driver);
            let request = hyper::Request::builder()
                .method(method)
                .uri(path)
                .header("Host", address.to_string())
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(Full::new(Bytes::from(body.to_string())))
                .map_err(|_| failure("Invalid local host request."))?;
            let response = sender
                .send_request(request)
                .await
                .map_err(|_| failure("Host request failed."))?;
            let status = response.status();
            let bytes = Limited::new(response.into_body(), 1024 * 1024)
                .collect()
                .await
                .map_err(|_| failure("Host response exceeded its limit or failed."))?
                .to_bytes();
            let value: Value = if bytes.is_empty() {
                json!({"ok":true})
            } else {
                serde_json::from_slice(&bytes).map_err(|_| failure("Invalid host response."))?
            };
            if status == hyper::StatusCode::UNAUTHORIZED || status == hyper::StatusCode::FORBIDDEN {
                return Err(failure(
                    "Host control authorization failed; check the selected host storage and token.",
                ));
            }
            if !status.is_success() {
                return Err((
                    status.as_u16(),
                    value["message"]
                        .as_str()
                        .unwrap_or("Host operation failed.")
                        .into(),
                ));
            }
            Ok(value)
        })
        .await
        .map_err(|_| failure("Host request timed out."))?
    }
    fn verify_host_config(&self, value: &Value) -> Result<()> {
        if value["storage"]["config"].as_str().map(PathBuf::from) != Some(self.config.clone())
            || value["storage"]["host_data"]
                .as_str()
                .map(|p| absolute(Path::new(p)))
                != Some(self.host_data.clone())
        {
            return Err(failure(
                "Host uses a different configuration or storage directory.",
            ));
        }
        Ok(())
    }
    async fn verified_host(&self, method: &str, path: &str, body: Value) -> Result<Value> {
        self.verify_host_config(&self.host("GET", "/v1/config", Value::Null).await?)?;
        self.host(method, path, body).await
    }

    async fn lifecycle_policy(&self) -> Result<Value> {
        let status = self.manager_status().await?;
        if status["restart_policy"] == "application" {
            return Err((
                409,
                format!(
                    "Restart {} for lifecycle changes.",
                    status["application_name"]
                        .as_str()
                        .unwrap_or("the application")
                ),
            ));
        }
        Ok(status)
    }
    pub async fn overview(&self) -> Result<Value> {
        let manager = if self.manager.is_some() {
            match self.manager_status().await {
                Ok(status) => json!({"configured":true,"available":true,"status":status}),
                Err((_, message)) => json!({"configured":true,"available":false,"error":message}),
            }
        } else {
            json!({"configured":false,"available":false})
        };
        let status = self.host("GET", "/v1/status", Value::Null).await;
        let mut value = json!({"manager":manager,"host":{"available":false},"devices":[],"invitations":[],"sessions":[],
            "lifecycle":{"allowed":false,"restart_policy":self.restart_policy,"application_name":self.application_name}});
        if let Ok(host) = status {
            value["host"] = json!({"available":true,"status":host});
            let (devices, invitations, sessions) = tokio::join!(
                self.host("GET", "/v1/devices", Value::Null),
                self.host("GET", "/v1/invitations", Value::Null),
                self.host("GET", "/v1/sessions", Value::Null)
            );
            for (key, result) in [
                ("devices", devices),
                ("invitations", invitations),
                ("sessions", sessions),
            ] {
                match result {
                    Ok(data) => value[key] = data[key].clone(),
                    Err((_, message)) => value[format!("{key}_error")] = json!(message),
                }
            }
        } else if let Err((_, message)) = status {
            value["host"]["error"] = json!(message);
        }
        if value["manager"]["available"] == true {
            let manager = value["manager"]["status"].clone();
            value["lifecycle"] = json!({"allowed":manager["restart_policy"] == "host", "restart_policy":manager["restart_policy"], "application_name":manager["application_name"]});
        }
        Ok(value)
    }
    pub async fn configuration(&self) -> Result<Value> {
        let path = self.config.clone();
        let data = self.host_data.clone();
        let offline =
            tokio::task::spawn_blocking(move || administration::view(&path, &data, None, None))
                .await
                .map_err(|_| failure("Configuration read failed."))?
                .map_err(|e| (409, e.to_string()))?;
        let mut value = match self.host("GET", "/v1/config", Value::Null).await {
            Ok(live) => {
                self.verify_host_config(&live)?;
                live
            }
            Err((_, message)) => {
                let mut view = offline;
                view["effective_error"] = json!(message);
                view
            }
        };
        let manager_status = self.manager_status().await.ok();
        if let Some(manager) = &manager_status
            && let Some(fields) = manager["effective_manager"].as_object()
        {
            for (key, effective) in fields {
                let name = format!("manager.{key}");
                if value["fields"].get(&name).is_some() {
                    value["fields"][&name]["effective"] = if key == "host_enabled" {
                        json!(manager["desired"] == "enabled")
                    } else {
                        effective.clone()
                    };
                    value["fields"][&name]["source"] = manager["manager_sources"]
                        .get(key)
                        .cloned()
                        .unwrap_or(json!("file"));
                }
            }
        }
        for (key, effective) in self.admin_effective.as_object().unwrap() {
            value["fields"][key]["effective"] = effective["value"].clone();
            value["fields"][key]["source"] = effective["source"].clone();
        }
        if let Some(manager) = &manager_status {
            value["fields"]["admin.enabled"]["effective"] = manager["admin_enabled"].clone();
            value["fields"]["admin.enabled"]["source"] = manager["admin_source"].clone();
        }
        administration::annotate(&mut value);
        value["storage"]["admin_data"] = json!(self.admin_data);
        let policy = manager_status
            .as_ref()
            .and_then(|v| v["restart_policy"].as_str())
            .unwrap_or(&self.restart_policy);
        let application = manager_status
            .as_ref()
            .and_then(|v| v["application_name"].as_str())
            .unwrap_or(&self.application_name);
        value["restart_note"] = json!(if self.manager.is_some() && manager_status.is_none() {
            "Configured manager unavailable; lifecycle controls are disabled.".to_string()
        } else if policy == "application" {
            format!("Restart {application} for startup changes to take effect.")
        } else {
            "Restart the host for startup changes. Admin listener changes require restarting admin; manager launch settings apply at the next host launch.".to_string()
        });
        Ok(value)
    }
    pub async fn mutate(&self, path: &str, mut input: Value) -> Result<Value> {
        let _guard = self.mutations.lock().await;
        if let Some(object) = input.as_object_mut() {
            object.remove("csrf");
        }
        match path {
            "/api/lifecycle" => {
                self.lifecycle_policy().await?;
                let operation = match input["action"].as_str() {
                    Some("start") => "host.start",
                    Some("stop") => "host.stop",
                    Some("restart") => "host.restart",
                    _ => return Err((400, "Invalid lifecycle action.".into())),
                };
                Ok(json!({"accepted":true,"manager":self.manager_call(operation).await?}))
            }
            "/api/config/save" | "/api/config/save-restart" => {
                if path.ends_with("save-restart") {
                    self.lifecycle_policy().await?;
                }
                let live = self.host("GET", "/v1/config", Value::Null).await;
                let saved = match live {
                    Ok(view) => {
                        self.verify_host_config(&view)?;
                        self.host("PUT", "/v1/config", input).await?
                    }
                    Err(_) => {
                        let update: Update = serde_json::from_value(input)
                            .map_err(|_| (400, "Invalid configuration update.".into()))?;
                        let path = self.config.clone();
                        let data = self.host_data.clone();
                        tokio::task::spawn_blocking(move || {
                            let _config_lease = kausway_config::fsutil::host_config_lease(&path).map_err(|_| failure("A running host owns this configuration but could not be validated; settings were not saved."))?;
                            ensure_private_dir(&data)
                                .map_err(|_| failure("Host storage is inaccessible."))?;
                            let _lock = DataDirLock::acquire(&data).map_err(|_| {
                                failure(
                                    "Host is running but unavailable; configuration was not saved.",
                                )
                            })?;
                            administration::save(&path, update)
                                .map_err(|e| (409, e.to_string()))?;
                            Ok::<_, (u16, String)>(
                                json!({"saved":true,"applied":false,"host_stopped":true}),
                            )
                        })
                        .await
                        .map_err(|_| failure("Configuration save failed."))??
                    }
                };
                if path.ends_with("save-restart") {
                    // Saved configuration is retained even if restart fails.
                    return match self.manager_call("host.restart").await {
                        Ok(status) => Ok(
                            json!({"saved":true,"applied":false,"restart_requested":true,"manager":status}),
                        ),
                        Err((_, message)) => Ok(
                            json!({"saved":true,"applied":false,"restart_requested":false,"restart_error":message}),
                        ),
                    };
                }
                Ok(saved)
            }
            "/api/config/apply" => {
                self.verify_host_config(&self.host("GET", "/v1/config", Value::Null).await?)?;
                self.host("POST", "/v1/config/apply", input).await
            }
            "/api/invitations/create" => self.verified_host("POST", "/v1/invitations", input).await,
            "/api/invitations/cancel" | "/api/devices/revoke" => {
                let id = input["id"]
                    .as_str()
                    .ok_or((400, "An identifier is required.".into()))?;
                if id.is_empty()
                    || id.len() > 128
                    || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                {
                    return Err((400, "Invalid identifier.".into()));
                }
                let resource = if path.contains("invitations") {
                    "invitations"
                } else {
                    "devices"
                };
                self.host("DELETE", &format!("/v1/{resource}/{id}"), Value::Null)
                    .await
            }
            _ => Err((404, "Unknown administration operation.".into())),
        }
    }
}
