use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use kausway_admin::{
    auth::State,
    server::{Shared, serve},
};
use kausway_config::{Component, Paths, Store, fsutil::ensure_private_dir};
use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    process::ExitCode,
};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(
    name = "kausway-admin",
    version,
    about = "Local Kausway administrator setup and login"
)]
struct Cli {
    #[arg(long, global = true, env = "KAUSWAY_CONFIG")]
    config: Option<PathBuf>,
    #[arg(long, global = true, env = "KAUSWAY_ADMIN_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[arg(long, env = "KAUSWAY_ADMIN_LISTEN")]
    listen: Option<SocketAddr>,
    #[arg(long, env = "KAUSWAY_ADMIN_BROWSER_BASE_URL")]
    browser_base_url: Option<String>,
    /// Local manager endpoint for verified host lifecycle controls.
    #[arg(long)]
    manager_socket: Option<PathBuf>,
    #[arg(long, env = "KAUSWAY_ADMIN_HOST_DATA_DIR")]
    host_data_dir: Option<PathBuf>,
    #[arg(long, env = "KAUSWAY_ADMIN_HOST_CONTROL_LISTEN")]
    host_control_listen: Option<SocketAddr>,
    #[arg(long, env = "KAUSWAY_CONTROL_TOKEN", hide_env_values = true)]
    host_control_token: Option<String>,
    #[arg(long, env="KAUSWAY_ADMIN_RESTART_POLICY", value_parser=["host", "application"], default_value="application")]
    restart_policy: String,
    #[arg(long, env = "KAUSWAY_APPLICATION_NAME", default_value = "application")]
    application_name: String,
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    /// Replace the current expiring setup link (requires running admin).
    SetupLink,
    Credentials {
        #[command(subcommand)]
        command: Credentials,
    },
}
#[derive(Subcommand)]
enum Credentials {
    /// Create initial credentials; refuses to overwrite an account.
    Set(CredentialArgs),
    /// Explicitly replace credentials and invalidate existing sessions.
    Reset(CredentialArgs),
}
#[derive(Args)]
struct CredentialArgs {
    #[arg(long)]
    username: Option<String>,
    /// Read the password from stdin instead of a hidden terminal prompt.
    #[arg(long)]
    password_stdin: bool,
}
fn lock(data: &Path) -> io::Result<File> {
    ensure_private_dir(data)?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(data.join("admin.lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Admin is already running",
        )),
        Err(TryLockError::Error(e)) => Err(e),
    }
}
fn credentials(args: CredentialArgs) -> Result<(String, String), String> {
    let username = match args.username {
        Some(u) => u,
        None if args.password_stdin => {
            return Err("--username is required with --password-stdin".into());
        }
        None => {
            eprint!("Username: ");
            io::stderr().flush().map_err(|_| "Could not flush prompt")?;
            let mut value = String::new();
            io::BufRead::read_line(&mut io::BufReader::new(io::stdin().take(66)), &mut value)
                .map_err(|_| "Could not read username")?;
            value.trim_end_matches(['\r', '\n']).to_string()
        }
    };
    let password = if args.password_stdin {
        let mut bytes = Vec::new();
        io::stdin()
            .take(1027)
            .read_to_end(&mut bytes)
            .map_err(|_| "Could not read password")?;
        let mut value = String::from_utf8(bytes).map_err(|_| "Password must be UTF-8")?;
        if value.ends_with('\n') {
            value.pop();
            if value.ends_with('\r') {
                value.pop();
            }
        }
        if value.len() > 1024 {
            return Err("Password exceeds 1024 bytes".into());
        }
        value
    } else {
        let value = rpassword::prompt_password("Password: ")
            .map_err(|_| "Could not read hidden password; use --password-stdin for automation")?;
        let confirm = rpassword::prompt_password("Confirm password: ")
            .map_err(|_| "Could not read password confirmation")?;
        if value != confirm {
            return Err("Passwords do not match".into());
        }
        value
    };
    Ok((username, password))
}
#[tokio::main]
async fn main() -> ExitCode {
    let matches = Cli::command().get_matches();
    let cli = Cli::from_arg_matches(&matches).expect("validated command line");
    let sources: std::collections::BTreeMap<String, kausway_config::Source> =
        ["listen", "browser_base_url"]
            .iter()
            .filter_map(|key| {
                matches.value_source(key).map(|source| {
                    (
                        key.to_string(),
                        if source == clap::parser::ValueSource::CommandLine {
                            kausway_config::Source::Cli
                        } else {
                            kausway_config::Source::Environment
                        },
                    )
                })
            })
            .collect();
    match run(cli, sources).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
async fn run(
    cli: Cli,
    sources: std::collections::BTreeMap<String, kausway_config::Source>,
) -> Result<(), String> {
    let defaults = if cli.config.is_none() || cli.data_dir.is_none() {
        Some(Paths::defaults().map_err(|e| e.to_string())?)
    } else {
        None
    };
    let config = cli
        .config
        .unwrap_or_else(|| defaults.as_ref().unwrap().config.clone());
    let saved = Store::new(&config)
        .load_or_create()
        .map_err(|e| e.to_string())?
        .config;
    let config = fs::canonicalize(config).map_err(|e| e.to_string())?;
    let data = cli
        .data_dir
        .unwrap_or_else(|| defaults.as_ref().unwrap().component(Component::Admin));
    if let Some(command) = cli.command {
        let (operation, username, password) = match command {
            Command::SetupLink => ("setup-link", String::new(), String::new()),
            Command::Credentials { command } => {
                let (args, reset) = match command {
                    Credentials::Set(a) => (a, false),
                    Credentials::Reset(a) => (a, true),
                };
                let (username, password) = credentials(args)?;
                (
                    if reset {
                        "credentials.reset"
                    } else {
                        "credentials.set"
                    },
                    username,
                    password,
                )
            }
        };
        let instance_lock = lock(&data);
        match instance_lock {
            Ok(_lock) if operation != "setup-link" => {
                kausway_admin::auth::provision_offline(
                    config,
                    &username,
                    &password,
                    operation == "credentials.reset",
                )
                .map_err(|(_, e)| e.to_string())?;
                println!("Administrator credentials saved.");
                return Ok(());
            }
            Ok(_) => return Err("Start kausway-admin before requesting a setup link".into()),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.to_string()),
        }
        #[cfg(unix)]
        {
            let result = kausway_admin::local::request(
                &data,
                kausway_admin::local::Request {
                    operation: operation.into(),
                    config,
                    username,
                    password,
                },
            )
            .await?;
            if let Some(url) = result["url"].as_str() {
                println!("{url}");
            } else {
                println!("Administrator credentials saved; previous sessions invalidated.");
            }
            return Ok(());
        }
        #[cfg(not(unix))]
        return Err(
            "Stop admin before changing credentials; live local commands need platform IPC support"
                .into(),
        );
    }
    let _lock = lock(&data).map_err(|e| e.to_string())?;
    let state = State::new(config.clone()).map_err(|(_, e)| e.to_string())?;
    let listen: SocketAddr = cli.listen.unwrap_or(
        saved
            .admin
            .listen
            .parse()
            .map_err(|_| "Invalid admin.listen")?,
    );
    if !listen.ip().is_loopback() && !listen.ip().is_unspecified() {
        return Err("Admin must bind to loopback or an unspecified container interface".into());
    }
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|e| e.to_string())?;
    let actual = listener.local_addr().map_err(|e| e.to_string())?;
    let default_base = if actual.ip().is_unspecified() {
        let browser_host = if actual.is_ipv6() {
            "[::1]"
        } else {
            "127.0.0.1"
        };
        format!("http://{browser_host}:{}", actual.port())
    } else {
        format!("http://{actual}")
    };
    let base = cli
        .browser_base_url
        .or(saved.admin.browser_base_url)
        .unwrap_or(default_base)
        .trim_end_matches('/')
        .to_string();
    let mut shared = Shared::new(state, base.clone(), actual)?;
    let host_data = cli
        .host_data_dir
        .unwrap_or_else(|| data.parent().unwrap_or(Path::new(".")).join("host"));
    shared.operations = Some(std::sync::Arc::new(
        kausway_admin::operations::Operations::new(
            config.clone(),
            data.clone(),
            host_data,
            cli.host_control_listen,
            cli.host_control_token,
            cli.manager_socket,
            cli.restart_policy,
            cli.application_name,
            serde_json::json!({
                "admin.listen": {"value": actual.to_string(), "source": sources.get("listen").copied().unwrap_or_else(|| kausway_config::administration::file_source(&config, "admin.listen"))},
                "admin.browser_base_url": {"value": base, "source": sources.get("browser_base_url").copied().unwrap_or_else(|| kausway_config::administration::file_source(&config, "admin.browser_base_url"))},
                "admin.enabled": {"value": true, "source": "cli"}
            }),
        ),
    ));
    #[cfg(unix)]
    let local_listener = kausway_admin::local::bind(&data.join("control.sock"))?;
    if !shared
        .work(|s| s.status(None))
        .await
        .map_err(|(_, e)| e.to_string())?
        .configured
    {
        let url = shared.setup_link().await.map_err(|(_, e)| e.to_string())?;
        if std::env::var_os("KAUSWAY_ADMIN_STARTUP_EVENTS").is_some() {
            println!(
                "{}",
                serde_json::json!({"event":"setup_link", "url":url, "expires_in_secs":900})
            );
        } else {
            eprintln!("Administrator setup (expires in 15 minutes): {url}");
        }
    }
    eprintln!("Kausway admin listening at {base}");
    let stop = CancellationToken::new();
    let stop_signal = stop.clone();
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut term) => {
                    tokio::select! { _ = term.recv() => {}, _ = tokio::signal::ctrl_c() => {} }
                }
                Err(_) => {
                    let _ = tokio::signal::ctrl_c().await;
                }
            }
        }
        #[cfg(not(unix))]
        let _ = tokio::signal::ctrl_c().await;
        stop_signal.cancel();
    });
    #[cfg(unix)]
    {
        let http = serve(listener, shared.clone(), stop.clone());
        let local = kausway_admin::local::serve(
            local_listener,
            data.join("control.sock"),
            config,
            shared,
            stop.clone(),
        );
        tokio::pin!(http, local);
        let result = tokio::select! { result = &mut http => { stop.cancel(); let _ = local.await; result }, result = &mut local => { stop.cancel(); let _ = http.await; result } };
        result
    }
    #[cfg(not(unix))]
    serve(listener, shared, stop).await
}
