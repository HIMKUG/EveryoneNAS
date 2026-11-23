//! CLI entry point for `lanvault-server`, supporting `--config <path>` for
//! Magisk (`service.sh`) and Windows Service (`lanvault-win-svc`) invocation.

use std::path::PathBuf;

use lanvault_server::{serve_with_shutdown, ServerConfig, ServerExitReason};
use tokio::sync::oneshot;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

/// Non-zero exit code signaled to SCM / Magisk watchdog to trigger immediate restart.
const EXIT_CODE_GRACEFUL_RESTART: i32 = 75;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = match parse_cli_config() {
        Ok(cfg) => cfg,
        Err(err) => {
            error!("configuration error: {err}");
            std::process::exit(2);
        }
    };

    let (_shutdown_tx, shutdown_rx) = oneshot::channel();
    match serve_with_shutdown(config, shutdown_rx, None).await {
        Ok(ServerExitReason::Stopped) => {}
        Ok(ServerExitReason::GracefulRestart) => {
            info!(
                exit_code = EXIT_CODE_GRACEFUL_RESTART,
                "graceful restart drain complete; exiting for supervisor restart"
            );
            std::process::exit(EXIT_CODE_GRACEFUL_RESTART);
        }
        Err(err) => {
            error!("lanvault-server exited with error: {err}");
            std::process::exit(1);
        }
    }
}

fn parse_cli_config() -> lanvault_core::Result<ServerConfig> {
    let mut args = std::env::args().skip(1);
    let mut config_path: Option<PathBuf> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" | "-c" => {
                let path = args.next().ok_or_else(|| {
                    lanvault_core::LanVaultError::InvalidInput(
                        "--config requires a file path argument".to_string(),
                    )
                })?;
                config_path = Some(PathBuf::from(path));
            }
            other => {
                return Err(lanvault_core::LanVaultError::InvalidInput(format!(
                    "unrecognized CLI argument: {other}"
                )));
            }
        }
    }

    match config_path {
        Some(path) => ServerConfig::load_from_file(&path),
        None => Ok(ServerConfig::default()),
    }
}
