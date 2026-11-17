use clap::{Args, Parser, Subcommand};
use kausway_config::Paths;
use std::{path::PathBuf, process::ExitCode};

#[derive(Parser)]
#[command(
    name = "kausway",
    version,
    about = "Optional local Kausway process manager"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Supervise owned host and optional admin children in the foreground.
    Run(Box<kausway_manager::RunArgs>),
    /// Inspect this manager's owned children.
    Status(Location),
    /// Persist enabled state and start the owned host.
    Start(Location),
    /// Persist disabled state and stop the owned host.
    Stop(Location),
    /// Persist enabled state, stop and start the owned host.
    Restart(Location),
    /// Shut down the manager and its children without changing desired state.
    Shutdown(Location),
}
#[derive(Args)]
struct Location {
    #[arg(long, env = "KAUSWAY_DATA_ROOT")]
    data_root: Option<PathBuf>,
}
#[tokio::main]
async fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Run(args) => kausway_manager::run(*args).await,
        other => {
            let (operation, location) = match other {
                Command::Status(l) => ("status", l),
                Command::Start(l) => ("host.start", l),
                Command::Stop(l) => ("host.stop", l),
                Command::Restart(l) => ("host.restart", l),
                Command::Shutdown(l) => ("shutdown", l),
                Command::Run(_) => unreachable!(),
            };
            let root = match location.data_root {
                Some(p) => Ok(p),
                None => Paths::defaults()
                    .map(|p| p.data_root)
                    .map_err(|e| e.to_string()),
            };
            match root {
                Ok(root) => match kausway_manager::request(&root, operation).await {
                    Ok(value) => {
                        println!("{value}");
                        Ok(())
                    }
                    Err(e) => Err(e),
                },
                Err(e) => Err(e),
            }
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
