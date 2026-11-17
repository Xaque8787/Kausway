use crate::auth::{self, State};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Request, Response, body::Incoming, header};
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::TcpListener, sync::Semaphore};

#[derive(Clone)]
pub struct Shared {
    pub state: Arc<Mutex<State>>,
    pub operations: Option<Arc<crate::operations::Operations>>,
    workers: Arc<Semaphore>,
    pub base: String,
    authorities: Arc<Vec<String>>,
    cookie_name: String,
}
impl Shared {
    pub fn new(state: State, base: String, actual: SocketAddr) -> Result<Self, String> {
        let uri: hyper::Uri = base.parse().map_err(|_| "Invalid browser base URL")?;
        let host = uri.host().ok_or("Browser URL requires a host")?;
        let loopback = host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|a| a.is_loopback());
        if uri.scheme_str() != Some("http")
            || !loopback
            || uri.path() != "/"
            || uri.query().is_some()
            || uri.authority().is_some_and(|a| a.as_str().contains('@'))
        {
            return Err("Browser base URL must be a plain HTTP loopback origin without path, query or user information".into());
        }
        let authorities = vec![
            uri.authority().unwrap().as_str().to_string(),
            format!("127.0.0.1:{}", actual.port()),
            format!("localhost:{}", actual.port()),
            format!("[::1]:{}", actual.port()),
        ];
        Ok(Self {
            state: Arc::new(Mutex::new(state)),
            operations: None,
            workers: Arc::new(Semaphore::new(4)),
            base,
            authorities: Arc::new(authorities),
            cookie_name: format!("kausway_session_{}", uri.port_u16().unwrap_or(80)),
        })
    }
    pub async fn work<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut State) -> auth::Result<T> + Send + 'static,
    ) -> auth::Result<T> {
        let permit = self
            .workers
            .clone()
            .try_acquire_owned()
            .map_err(|_| (429, "Administrator is busy. Try again shortly."))?;
        let state = self.state.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut state = state
                .lock()
                .map_err(|_| (500, "Administrator state is unavailable."))?;
            operation(&mut state)
        })
        .await
        .map_err(|_| (500, "Administrator operation failed."))?
    }
    pub async fn setup_link(&self) -> auth::Result<String> {
        let base = self.base.clone();
        self.work(move |s| s.setup_link(&base)).await
    }
}

fn response(status: u16, content_type: &str, body: impl Into<Bytes>) -> Response<Full<Bytes>> {
    Response::builder().status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .header("Referrer-Policy", "no-referrer")
        .header("Content-Security-Policy", "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'")
        .body(Full::new(body.into())).unwrap()
}
fn json_response(status: u16, value: Value) -> Response<Full<Bytes>> {
    response(status, "application/json", value.to_string())
}
fn error((status, message): auth::Failure) -> Response<Full<Bytes>> {
    json_response(status, json!({"error": message}))
}
fn redirect(to: &'static str) -> Response<Full<Bytes>> {
    let mut reply = response(303, "text/plain", "Redirecting");
    reply
        .headers_mut()
        .insert(header::LOCATION, to.parse().unwrap());
    reply
}
fn session(request: &Request<Incoming>, cookie_name: &str) -> Option<String> {
    request
        .headers()
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|entry| {
            let (name, value) = entry.trim().split_once('=')?;
            (name == cookie_name
                && value.len() == 64
                && value.bytes().all(|c| c.is_ascii_hexdigit()))
            .then(|| value.to_string())
        })
}
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct Input {
    token: String,
    username: String,
    password: String,
    current_password: String,
    csrf: String,
}
async fn route(request: Request<Incoming>, shared: Shared) -> Response<Full<Bytes>> {
    let Some(host) = request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
    else {
        return error((400, "Host header required."));
    };
    if !shared.authorities.iter().any(|a| a == host) {
        return error((403, "Unrecognized local administrator host."));
    }
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let cookie = session(&request, &shared.cookie_name);
    if method == hyper::Method::GET {
        return match path.as_str() {
            "/app.js" => response(
                200,
                "text/javascript; charset=utf-8",
                include_str!("assets/app.js"),
            ),
            "/style.css" => response(
                200,
                "text/css; charset=utf-8",
                include_str!("assets/style.css"),
            ),
            "/api/overview" | "/api/config" => {
                let auth_cookie = cookie.clone().unwrap_or_default();
                if let Err(e) = shared
                    .work(move |s| s.check_session(&auth_cookie, None))
                    .await
                {
                    return error(e);
                }
                let Some(operations) = &shared.operations else {
                    return error((503, "Host administration is not configured."));
                };
                let result = if path == "/api/config" {
                    operations.configuration().await
                } else {
                    operations.overview().await
                };
                match result {
                    Ok(value) => json_response(200, value),
                    Err((status, message)) => json_response(status, json!({"error":message})),
                }
            }
            "/" | "/api/status" | "/admin" => {
                match shared.work(move |s| s.status(cookie.as_deref())).await {
                    Ok(status) if path == "/api/status" => {
                        json_response(200, serde_json::to_value(status).unwrap())
                    }
                    Ok(status) if path == "/" => redirect(if !status.configured {
                        "/setup"
                    } else if status.authenticated {
                        "/admin"
                    } else {
                        "/login"
                    }),
                    Ok(status) if !status.authenticated => redirect(if status.configured {
                        "/login"
                    } else {
                        "/setup"
                    }),
                    Ok(_) => response(
                        200,
                        "text/html; charset=utf-8",
                        include_str!("assets/index.html"),
                    ),
                    Err(e) => error(e),
                }
            }
            "/setup" | "/login" => response(
                200,
                "text/html; charset=utf-8",
                include_str!("assets/index.html"),
            ),
            _ => error((404, "Not found.")),
        };
    }
    if method != hyper::Method::POST {
        return error((405, "Method not allowed."));
    }
    let expected_origin = format!("http://{host}");
    if request
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        != Some(expected_origin.as_str())
        || request
            .headers()
            .get("Sec-Fetch-Site")
            .is_some_and(|v| v == "cross-site")
    {
        return error((403, "Same-origin request required."));
    }
    if request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap().trim())
        != Some("application/json")
    {
        return error((415, "JSON request required."));
    }
    let body = match tokio::time::timeout(
        Duration::from_secs(5),
        Limited::new(request.into_body(), 16384).collect(),
    )
    .await
    {
        Ok(Ok(body)) => body.to_bytes(),
        _ => return error((413, "Request body is too large or incomplete.")),
    };
    if matches!(
        path.as_str(),
        "/api/config/save"
            | "/api/config/apply"
            | "/api/config/save-restart"
            | "/api/lifecycle"
            | "/api/invitations/create"
            | "/api/invitations/cancel"
            | "/api/devices/revoke"
    ) {
        let value: Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(_) => return error((400, "Invalid JSON request.")),
        };
        let csrf = value
            .get("csrf")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let auth_cookie = cookie.unwrap_or_default();
        if let Err(e) = shared
            .work(move |s| s.check_session(&auth_cookie, Some(&csrf)))
            .await
        {
            return error(e);
        }
        let Some(operations) = &shared.operations else {
            return error((503, "Host administration is not configured."));
        };
        return match operations.mutate(&path, value).await {
            Ok(value) => json_response(200, value),
            Err((status, message)) => json_response(status, json!({"error":message})),
        };
    }
    let input: Input = match serde_json::from_slice(&body) {
        Ok(i) => i,
        Err(_) => return error((400, "Invalid JSON request.")),
    };
    let cookie_name = shared.cookie_name.clone();
    let result = shared
        .work(move |s| match path.as_str() {
            "/api/setup" => {
                s.setup(&input.token, &input.username, &input.password)?;
                Ok(json_response(200, json!({"ok": true})))
            }
            "/api/login" => {
                let login = s.login(&input.username, &input.password)?;
                let mut reply = json_response(200, json!({"ok": true, "csrf": login.csrf}));
                reply.headers_mut().insert(
                    header::SET_COOKIE,
                    format!(
                        "{}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age=3600",
                        cookie_name, login.cookie
                    )
                    .parse()
                    .unwrap(),
                );
                Ok(reply)
            }
            "/api/logout" => {
                s.logout(cookie.as_deref().unwrap_or(""), &input.csrf)?;
                let mut reply = json_response(200, json!({"ok": true}));
                reply.headers_mut().insert(
                    header::SET_COOKIE,
                    format!("{cookie_name}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
                        .parse()
                        .unwrap(),
                );
                Ok(reply)
            }
            "/api/password" => {
                s.change_password(
                    cookie.as_deref().unwrap_or(""),
                    &input.csrf,
                    &input.current_password,
                    &input.username,
                    &input.password,
                )?;
                let mut reply = json_response(200, json!({"ok": true}));
                reply.headers_mut().insert(
                    header::SET_COOKIE,
                    format!("{cookie_name}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
                        .parse()
                        .unwrap(),
                );
                Ok(reply)
            }
            _ => Err((404, "Not found.")),
        })
        .await;
    result.unwrap_or_else(error)
}

pub async fn serve(
    listener: TcpListener,
    shared: Shared,
    shutdown: tokio_util::sync::CancellationToken,
) -> Result<(), String> {
    let slots = Arc::new(Semaphore::new(32));
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            Some(_) = connections.join_next(), if !connections.is_empty() => {},
            accepted = listener.accept() => {
                let (stream, _) = accepted.map_err(|e| e.to_string())?;
                let Ok(permit) = slots.clone().try_acquire_owned() else { continue; };
                let shared = shared.clone();
                connections.spawn(async move {
                    let _permit = permit;
                    let service = hyper::service::service_fn(move |request| {
                        let shared = shared.clone();
                        async move { Ok::<_, std::convert::Infallible>(route(request, shared).await) }
                    });
                    let _ = tokio::time::timeout(Duration::from_secs(150), hyper::server::conn::http1::Builder::new()
                        .timer(hyper_util::rt::TokioTimer::new()).header_read_timeout(Duration::from_secs(10)).max_headers(64)
                        .max_buf_size(16 * 1024).keep_alive(false).serve_connection(TokioIo::new(stream), service)).await;
                });
            }
        }
    }
    // Allow in-flight account writes to finish before releasing the instance lock.
    if tokio::time::timeout(Duration::from_secs(5), async {
        while connections.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        connections.abort_all();
    }
    Ok(())
}
