//! LanVault Windows Session 0 系统服务宿主程序 (`lanvault-win-svc`)。
//!
//! 参考依据 (`vendor_refs/rust-windows-service`):
//! - `examples/ping_service.rs`: `define_windows_service!`、`service_dispatcher::start` 与
//!   `service_control_handler::register` 控制循环模板。
//! - `examples/install_service.rs` 与 `examples/uninstall_service.rs`: SCM 注册与卸载轮询。
//! - `examples/service_failure_actions.rs` 与 `src/service.rs`: 通过 `ChangeServiceConfig2W`
//!   配置 `1s / 5s / 10s` 崩溃自动重启恢复策略。

mod firewall;
mod volume_guid;

use anyhow::Result;

/// Windows SCM 注册服务名称。
pub const SERVICE_NAME: &str = "EveryoneNASService";
pub const SERVICE_DISPLAY_NAME: &str = "EveryoneNAS LAN Backup & Storage Service";
pub const SERVICE_DESCRIPTION: &str =
    "EveryoneNAS cross-platform encrypted LAN incremental backup and storage vault service";
pub const DEFAULT_LAN_PORT: u16 = 7842;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let subcommand = std::env::args().nth(1);
    dispatch_cli(subcommand.as_deref())
}

#[cfg(windows)]
fn dispatch_cli(subcommand: Option<&str>) -> Result<()> {
    match subcommand {
        Some("install") => win_scm::install_service(),
        Some("uninstall") => win_scm::uninstall_service(),
        Some("volumes") => {
            for vol in volume_guid::enumerate_volumes()? {
                println!("{} -> {:?}", vol.volume_guid, vol.mount_paths);
            }
            Ok(())
        }
        Some("run") | None => win_scm::run_as_service(),
        Some(other) => {
            anyhow::bail!("unknown subcommand `{other}`; expected `install`, `uninstall`, `volumes`, or `run`")
        }
    }
}

#[cfg(not(windows))]
fn dispatch_cli(subcommand: Option<&str>) -> Result<()> {
    let cmd = subcommand.unwrap_or("run");
    anyhow::bail!(
        "lanvault-win-svc (`{cmd}`) requires Windows Session 0 SCM and cannot run on {}",
        std::env::consts::OS
    )
}

#[cfg(windows)]
mod win_scm {
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::net::SocketAddr;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    use anyhow::{Context, Result};
    use lanvault_core::Database;
    use lanvault_server::{ServerConfig, ServerExitReason};
    use tokio::sync::{oneshot, watch};
    use tracing::{error, info, warn};
    use windows_service::{
        define_windows_service,
        service::{
            ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
            ServiceErrorControl, ServiceExitCode, ServiceFailureActions,
            ServiceFailureResetPeriod, ServiceInfo, ServiceStartType, ServiceState, ServiceStatus,
            ServiceType,
        },
        service_control_handler::{self, ServiceControlHandlerResult},
        service_dispatcher,
        service_manager::{ServiceManager, ServiceManagerAccess},
    };
    use windows_sys::Win32::Foundation::ERROR_SERVICE_DOES_NOT_EXIST;

    use crate::{
        firewall, volume_guid, DEFAULT_LAN_PORT, SERVICE_DESCRIPTION, SERVICE_DISPLAY_NAME,
        SERVICE_NAME,
    };

    const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;
    const DEFAULT_PROGRAM_DATA_ROOT: &str = r"C:\ProgramData\EveryoneNAS";
    const GRACEFUL_STOP_TIMEOUT: Duration = Duration::from_secs(3);
    const SERVER_BIND_TIMEOUT: Duration = Duration::from_secs(10);

    define_windows_service!(ffi_service_main, lanvault_service_main);

    pub fn run_as_service() -> Result<()> {
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)
            .context("failed to start Windows Service Control Manager dispatcher")
    }

    fn lanvault_service_main(_arguments: Vec<OsString>) {
        if let Err(err) = run_service_loop() {
            error!(error = %err, "EveryoneNAS Windows service exited with error");
        }
    }

    fn load_service_config() -> Result<ServerConfig> {
        if let Some(config_path) = std::env::var_os("LANVAULT_CONFIG_PATH") {
            let path = PathBuf::from(config_path);
            let mut cfg = ServerConfig::load_from_file(&path)
                .with_context(|| format!("failed to load server config from {}", path.display()))?;
            cfg.keep_alive_level = "WIN_SERVICE".to_string();
            validate_service_config(&cfg)?;
            return Ok(cfg);
        }

        let mut cfg = ServerConfig {
            port: DEFAULT_LAN_PORT,
            bind_addr: "0.0.0.0".to_string(),
            vault_root: PathBuf::from(DEFAULT_PROGRAM_DATA_ROOT),
            keep_alive_level: "WIN_SERVICE".to_string(),
            ..ServerConfig::default()
        };

        if let Some(vault_root) = std::env::var_os("LANVAULT_VAULT_ROOT") {
            cfg.vault_root = PathBuf::from(vault_root);
        }
        if let Ok(port_raw) = std::env::var("LANVAULT_PORT") {
            cfg.port = port_raw
                .parse()
                .with_context(|| format!("invalid LANVAULT_PORT value: `{port_raw}`"))?;
        }
        if let Ok(quota_gb_raw) = std::env::var("LANVAULT_QUOTA_GB") {
            let quota_gb: u64 = quota_gb_raw
                .parse()
                .with_context(|| format!("invalid LANVAULT_QUOTA_GB value: `{quota_gb_raw}`"))?;
            cfg.default_quota_bytes = quota_gb.saturating_mul(1024 * 1024 * 1024);
        }
        if let Ok(web_password) = std::env::var("LANVAULT_WEB_PASSWORD") {
            cfg.web_password = web_password;
        }
        validate_service_config(&cfg)?;
        Ok(cfg)
    }

    fn validate_service_config(config: &ServerConfig) -> Result<()> {
        if config.web_password.len() < 8 {
            anyhow::bail!(
                "LANVAULT_WEB_PASSWORD or web_password in LANVAULT_CONFIG_PATH must be at least 8 characters"
            );
        }
        Ok(())
    }

    /// 将当前系统枚举到的 Volume GUID 盘符映射写回 `index.db` 的 `vault_volumes` 表，落实盘符漂移自愈。
    fn reconcile_volume_mounts(db_path: &Path, volumes: &[volume_guid::VolumeInfo]) -> Result<()> {
        if !db_path.exists() {
            return Ok(());
        }

        let db = Database::open(db_path)
            .with_context(|| format!("failed to open index.db at {}", db_path.display()))?;

        let guid_to_mount: HashMap<&str, &Path> = volumes
            .iter()
            .filter_map(|v| v.mount_paths.first().map(|p| (v.volume_guid.as_str(), p.as_path())))
            .collect();

        let mut updated_count = 0usize;
        for vault in db.list_vaults()? {
            for vol in db.list_vault_volumes(&vault.vault_id)? {
                let Some(guid) = vol.volume_guid.as_deref() else {
                    continue;
                };
                match guid_to_mount.get(guid) {
                    Some(current_root) => {
                        let resolved = rebind_mount_path(&vol.mount_path, current_root);
                        if resolved != vol.mount_path || !vol.is_online {
                            db.update_volume_mount_path(&vol.volume_id, &resolved, true)?;
                            info!(
                                volume_id = %vol.volume_id,
                                guid = %guid,
                                old_path = %vol.mount_path,
                                new_path = %resolved,
                                "healed Windows volume mount path from Volume GUID"
                            );
                            updated_count += 1;
                        }
                    }
                    None => {
                        if vol.is_online {
                            db.update_volume_mount_path(&vol.volume_id, &vol.mount_path, false)?;
                            warn!(
                                volume_id = %vol.volume_id,
                                guid = %guid,
                                "marked missing Windows volume offline"
                            );
                            updated_count += 1;
                        }
                    }
                }
            }
        }

        info!(
            updated_count,
            "completed startup Volume GUID mount reconciliation"
        );
        Ok(())
    }

    fn backfill_volume_guids(db_path: &Path) -> Result<()> {
        if !db_path.exists() {
            return Ok(());
        }
        let db = Database::open(db_path)
            .with_context(|| format!("failed to open index.db at {}", db_path.display()))?;
        for vault in db.list_vaults()? {
            for volume in db.list_vault_volumes(&vault.vault_id)? {
                if volume.volume_guid.is_some() {
                    continue;
                }
                let guid = volume_guid::guid_for_mount_path(Path::new(&volume.mount_path))?;
                db.update_volume_guid(&volume.volume_id, &guid)?;
                info!(
                    volume_id = %volume.volume_id,
                    guid = %guid,
                    "backfilled missing Windows Volume GUID"
                );
            }
        }
        Ok(())
    }

    /// 若旧挂载路径包含 Windows 盘符前缀（如 `D:\LanVault`），将其替换为新盘符根路径（如 `E:\LanVault`）。
    fn rebind_mount_path(old_mount: &str, new_root: &Path) -> String {
        let bytes = old_mount.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/')
        {
            let raw_suffix = old_mount[3..].trim_start_matches(['\\', '/']);
            let mount_name = new_root.file_name().and_then(|name| name.to_str());
            let suffix = match mount_name {
                Some(name)
                    if raw_suffix.starts_with(name)
                        && raw_suffix
                            .as_bytes()
                            .get(name.len())
                            .is_some_and(|next| *next == b'\\' || *next == b'/') =>
                {
                    raw_suffix[name.len() + 1..].to_string()
                }
                _ => raw_suffix.to_string(),
            };
            if suffix.is_empty() {
                return new_root.to_string_lossy().into_owned();
            }
            return new_root.join(suffix).to_string_lossy().into_owned();
        }
        new_root.to_string_lossy().into_owned()
    }

    fn run_service_loop() -> Result<()> {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();

        let event_handler = move |control_event| -> ServiceControlHandlerResult {
            match control_event {
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    let _ = stop_tx.send(());
                    ServiceControlHandlerResult::NoError
                }
                _ => ServiceControlHandlerResult::NotImplemented,
            }
        };

        let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)?;

        status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::StartPending,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 1,
            wait_hint: Duration::from_secs(5),
            process_id: None,
        })?;

        let server_config = load_service_config()?;
        let db_path = server_config.vault_root.join("index.db");

        // 启动时解析所有在线 Volume GUID 并回写至 index.db，落实盘符漂移自愈。
        let volumes = volume_guid::enumerate_volumes()?;
        info!(
            volume_count = volumes.len(),
            "resolved Windows Volume GUIDs on startup"
        );
        reconcile_volume_mounts(&db_path, &volumes)?;

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("lanvault-win-svc")
            .build()?;

        let (svc_stop_tx, svc_stop_rx) = watch::channel(false);
        let (bound_tx, bound_rx) = oneshot::channel::<SocketAddr>();
        let (server_exit_tx, server_exit_rx) = mpsc::channel::<()>();
        let mut initial_bound_tx = Some(bound_tx);

        let mut server_task = runtime.spawn(async move {
            loop {
                let (iter_shutdown_tx, iter_shutdown_rx) = oneshot::channel::<()>();
                let mut stop_rx_iter = svc_stop_rx.clone();
                let stop_bridge = tokio::spawn(async move {
                    if *stop_rx_iter.borrow() || stop_rx_iter.changed().await.is_ok() {
                        let _ = iter_shutdown_tx.send(());
                    }
                });

                let current_bound_tx = initial_bound_tx.take();
                let exit_res = lanvault_server::serve_with_shutdown(
                    server_config.clone(),
                    iter_shutdown_rx,
                    current_bound_tx,
                )
                .await;
                stop_bridge.abort();

                match exit_res {
                    Ok(ServerExitReason::GracefulRestart) => {
                        if *svc_stop_rx.borrow() {
                            break;
                        }
                        info!(
                            "EveryoneNAS Windows service server triggered GracefulRestart; restarting loop"
                        );
                        continue;
                    }
                    Ok(ServerExitReason::Stopped) => {
                        info!("EveryoneNAS Windows Session 0 server stopped");
                        break;
                    }
                    Err(err) => {
                        error!(error = %err, "EveryoneNAS Windows Session 0 server exited with error");
                        break;
                    }
                }
            }
            let _ = server_exit_tx.send(());
        });

        let bound_addr = runtime.block_on(async {
            tokio::time::timeout(SERVER_BIND_TIMEOUT, bound_rx)
                .await
                .context("timed out waiting for LanVault server to bind")?
                .context("EveryoneNAS server exited before binding a port")
        })?;
        info!(addr = %bound_addr, "EveryoneNAS Windows Session 0 server bound and ready");
        backfill_volume_guids(&db_path)?;

        status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Running,
            controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })?;

        let unexpected_exit = loop {
            match server_exit_rx.recv_timeout(Duration::from_millis(500)) {
                Ok(()) => break true,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if stop_rx.try_recv().is_ok() {
                        break false;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break true,
            }
        };

        if unexpected_exit {
            checkpoint_default_db(&db_path)?;
            status_handle.set_service_status(ServiceStatus {
                service_type: SERVICE_TYPE,
                current_state: ServiceState::Stopped,
                controls_accepted: ServiceControlAccept::empty(),
                exit_code: ServiceExitCode::ServiceSpecific(1),
                checkpoint: 0,
                wait_hint: Duration::default(),
                process_id: None,
            })?;
            anyhow::bail!("EveryoneNAS server exited while the service was running");
        }

        status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::StopPending,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 1,
            wait_hint: Duration::from_secs(5),
            process_id: None,
        })?;

        let _ = svc_stop_tx.send(true);
        runtime.block_on(async {
            if tokio::time::timeout(GRACEFUL_STOP_TIMEOUT, &mut server_task)
                .await
                .is_err()
            {
                warn!("EveryoneNAS server task exceeded 3s stop timeout; aborting");
                server_task.abort();
            }
        });

        let checkpoint_result = checkpoint_default_db(&db_path);

        status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: if checkpoint_result.is_ok() {
                ServiceExitCode::Win32(0)
            } else {
                ServiceExitCode::ServiceSpecific(1)
            },
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })?;

        checkpoint_result
    }

    /// 注册 Windows Session 0 服务，配置 `ChangeServiceConfig2` (`1s / 5s / 10s`) 崩溃恢复与防火墙规则。
    pub fn install_service() -> Result<()> {
        let manager_access = ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE;
        let service_manager = ServiceManager::local_computer(None::<&str>, manager_access)?;

        let service_binary_path = std::env::current_exe()?;
        let service_info = ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from(SERVICE_DISPLAY_NAME),
            service_type: SERVICE_TYPE,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: service_binary_path,
            launch_arguments: vec![OsString::from("run")],
            dependencies: vec![],
            account_name: None, // LocalSystem (Session 0)
            account_password: None,
        };

        let service_access = ServiceAccess::QUERY_CONFIG
            | ServiceAccess::CHANGE_CONFIG
            | ServiceAccess::START
            | ServiceAccess::DELETE;

        let service = service_manager
            .create_service(&service_info, service_access)
            .or_else(|_| service_manager.open_service(SERVICE_NAME, service_access))?;

        service.change_config(&service_info)?;
        service.set_description(SERVICE_DESCRIPTION)?;

        // 底层调用 Win32 `ChangeServiceConfig2W(SERVICE_CONFIG_FAILURE_ACTIONS)` 配置 1s / 5s / 10s 自动重启。
        let failure_actions = ServiceFailureActions {
            reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(86_400)),
            reboot_msg: None,
            command: None,
            actions: Some(vec![
                ServiceAction {
                    action_type: ServiceActionType::Restart,
                    delay: Duration::from_secs(1),
                },
                ServiceAction {
                    action_type: ServiceActionType::Restart,
                    delay: Duration::from_secs(5),
                },
                ServiceAction {
                    action_type: ServiceActionType::Restart,
                    delay: Duration::from_secs(10),
                },
            ]),
        };
        service.update_failure_actions(failure_actions)?;
        service.set_failure_actions_on_non_crash_failures(true)?;

        let firewall_port = std::env::var("LANVAULT_PORT")
            .ok()
            .and_then(|port| port.parse::<u16>().ok())
            .unwrap_or(DEFAULT_LAN_PORT);
        firewall::ensure_firewall_rules(firewall_port)?;
        info!(
            service = SERVICE_NAME,
            "installed Windows service with 1s/5s/10s crash recovery"
        );
        Ok(())
    }

    /// 停止并卸载 Windows 服务，同时移除防火墙规则。
    pub fn uninstall_service() -> Result<()> {
        let service_manager =
            ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let service_access =
            ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE;
        let service = service_manager.open_service(SERVICE_NAME, service_access)?;

        if service.query_status()?.current_state != ServiceState::Stopped {
            service.stop()?;
            let stop_deadline = Instant::now() + Duration::from_secs(30);
            while service.query_status()?.current_state != ServiceState::Stopped {
                if Instant::now() >= stop_deadline {
                    anyhow::bail!("Windows service did not stop within 30 seconds");
                }
                sleep(Duration::from_millis(250));
            }
        }
        service.delete()?;
        drop(service);

        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if let Err(windows_service::Error::Winapi(err)) =
                service_manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS)
            {
                if err.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST as i32) {
                    break;
                }
            }
            sleep(Duration::from_millis(500));
        }

        firewall::remove_firewall_rules()?;
        info!(
            service = SERVICE_NAME,
            "uninstalled Windows service and removed firewall rules"
        );
        Ok(())
    }

    fn checkpoint_default_db(db_path: &Path) -> Result<()> {
        if !db_path.exists() {
            return Ok(());
        }
        let db = Database::open(db_path).map_err(|err| {
            error!(
                path = %db_path.display(),
                error = %err,
                "failed to open SQLite database for WAL checkpoint"
            );
            err
        })?;
        db.wal_checkpoint_truncate().map_err(|err| {
            error!(
                path = %db_path.display(),
                error = %err,
                "failed to execute PRAGMA wal_checkpoint(TRUNCATE)"
            );
            err
        })?;
        Ok(())
    }
}
