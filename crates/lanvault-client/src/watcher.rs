//! 实时目录监听与 500ms 防抖窗口，显式捕获队列溢出事件并触发全量元数据 Diff 对账。
//!
//! 参考依据 (`vendor_refs/rust-notify`):
//! - `notify/src/windows.rs`: `completion_rescan_event` 将 `ERROR_NOTIFY_ENUM_DIR` (Win32 1022)
//!   及 0 字节完成包映射为 `Event::new(EventKind::Other).set_flag(Flag::Rescan)`。
//! - `notify/src/inotify.rs`: `EventMask::Q_OVERFLOW` 映射为 `Flag::Rescan`。
//! - `examples/debouncer_mini.rs` 与 `examples/async_monitor.rs`: 防抖与异步通道转发模式。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{Config, ErrorKind, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio::time::{self, Instant};
use tracing::{error, info, warn};

use crate::error::ClientError;

/// 实时监听防抖窗口：500ms，合并连续写入，避免半成品文件重复触发上传。
pub const DEBOUNCE_WINDOW: Duration = Duration::from_millis(500);

/// Windows `ReadDirectoryChangesW` 缓冲区溢出错误码 (`ERROR_NOTIFY_ENUM_DIR`)。
pub const WIN32_ERROR_NOTIFY_ENUM_DIR: i32 = 1022;

/// 触发全量元数据 Diff 对账的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RescanReason {
    /// `notify` 后端上报 `Flag::Rescan`（Windows `ERROR_NOTIFY_ENUM_DIR` / 0 字节完成，或 Linux/Android `IN_Q_OVERFLOW`）。
    NotifyFlagRescan,
    /// `notify` I/O 错误直接携带 Win32 `ERROR_NOTIFY_ENUM_DIR` (1022)。
    Win32NotifyEnumDir,
}

/// 经防抖与溢出检测后输出给同步引擎的事件。
#[derive(Debug, Clone)]
pub enum WatchEvent {
    /// 500ms 防抖窗口聚合后的变更路径集合（已去重）。
    PathsChanged {
        rule_id: String,
        paths: Vec<PathBuf>,
    },
    /// 监听器发生队列溢出，必须立即执行 `scanner::reconcile_directory` 全量元数据 Diff 对账。
    RescanRequired {
        rule_id: String,
        root_dir: PathBuf,
        reason: RescanReason,
    },
}

enum RawSignal {
    Paths(Vec<PathBuf>),
    Rescan(RescanReason),
}

/// 封装 `notify::RecommendedWatcher` 与 500ms 异步防抖任务的目录监听器。
pub struct DirectoryWatcher {
    _watcher: RecommendedWatcher,
    _debounce_task: tokio::task::JoinHandle<()>,
}

impl DirectoryWatcher {
    /// 启动对指定目录树的递归监听，并将聚合事件发送至 `event_tx`。
    ///
    /// 在外部边界校验 `root_dir` 是否存在且为目录；内部回调信任通道与类型契约。
    pub fn start(
        rule_id: String,
        root_dir: PathBuf,
        event_tx: mpsc::Sender<WatchEvent>,
    ) -> Result<Self, ClientError> {
        if !root_dir.is_dir() {
            return Err(ClientError::InvalidPath {
                path: root_dir,
                reason: "watch target must be an existing directory".to_string(),
            });
        }

        let (raw_tx, raw_rx) = mpsc::unbounded_channel::<RawSignal>();
        let rule_id_cb = rule_id.clone();

        let mut watcher = RecommendedWatcher::new(
            move |res: notify::Result<Event>| match res {
                Ok(event) => {
                    if event.need_rescan() {
                        warn!(
                            rule_id = %rule_id_cb,
                            "notify reported Flag::Rescan (queue overflow); triggering metadata diff reconciliation"
                        );
                        let _ = raw_tx.send(RawSignal::Rescan(RescanReason::NotifyFlagRescan));
                        return;
                    }

                    // 忽略纯读取访问事件，仅响应创建/修改/删除/重命名等变更。
                    if matches!(event.kind, EventKind::Access(_)) {
                        return;
                    }

                    if !event.paths.is_empty() {
                        let _ = raw_tx.send(RawSignal::Paths(event.paths));
                    }
                }
                Err(err) => {
                    if is_win32_notify_enum_dir(&err) {
                        warn!(
                            rule_id = %rule_id_cb,
                            "caught Win32 ERROR_NOTIFY_ENUM_DIR (1022); triggering metadata diff reconciliation"
                        );
                        let _ = raw_tx.send(RawSignal::Rescan(RescanReason::Win32NotifyEnumDir));
                    } else {
                        error!(rule_id = %rule_id_cb, error = %err, "filesystem watcher error");
                    }
                }
            },
            Config::default(),
        )?;

        watcher.watch(&root_dir, RecursiveMode::Recursive)?;
        info!(rule_id = %rule_id, root = %root_dir.display(), "directory watcher started (500ms debounce)");

        let debounce_task = tokio::spawn(run_debounce_loop(rule_id, root_dir, raw_rx, event_tx));

        Ok(Self {
            _watcher: watcher,
            _debounce_task: debounce_task,
        })
    }
}

fn is_win32_notify_enum_dir(err: &notify::Error) -> bool {
    matches!(
        &err.kind,
        ErrorKind::Io(io_err) if io_err.raw_os_error() == Some(WIN32_ERROR_NOTIFY_ENUM_DIR)
    )
}

async fn run_debounce_loop(
    rule_id: String,
    root_dir: PathBuf,
    mut raw_rx: mpsc::UnboundedReceiver<RawSignal>,
    event_tx: mpsc::Sender<WatchEvent>,
) {
    let mut pending: HashMap<PathBuf, Instant> = HashMap::new();
    let mut tick = time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            signal = raw_rx.recv() => {
                let Some(signal) = signal else {
                    break;
                };
                match signal {
                    RawSignal::Paths(paths) => {
                        let now = Instant::now();
                        for path in paths {
                            pending.insert(path, now);
                        }
                    }
                    RawSignal::Rescan(reason) => {
                        // 溢出时清空挂起的碎片路径，直接发出全量对账指令。
                        pending.clear();
                        if event_tx
                            .send(WatchEvent::RescanRequired {
                                rule_id: rule_id.clone(),
                                root_dir: root_dir.clone(),
                                reason,
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            _ = tick.tick(), if !pending.is_empty() => {
                let now = Instant::now();
                let mut ready = Vec::new();
                pending.retain(|path, last_seen| {
                    if now.duration_since(*last_seen) >= DEBOUNCE_WINDOW {
                        ready.push(path.clone());
                        false
                    } else {
                        true
                    }
                });

                if !ready.is_empty()
                    && event_tx
                        .send(WatchEvent::PathsChanged {
                            rule_id: rule_id.clone(),
                            paths: ready,
                        })
                        .await
                        .is_err()
                {
                    break;
                }
            }
        }
    }
}

/// 验证并规范化被监听目录路径的辅助函数。
pub fn canonicalize_watch_root(path: &Path) -> Result<PathBuf, ClientError> {
    if !path.is_dir() {
        return Err(ClientError::InvalidPath {
            path: path.to_path_buf(),
            reason: "path does not exist or is not a directory".to_string(),
        });
    }
    std::fs::canonicalize(path).map_err(ClientError::Io)
}
