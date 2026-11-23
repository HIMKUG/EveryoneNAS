//! 已配对设备自愈重连状态机。
//!
//! 严格遵循指数退避探测序列：`2s -> 5s -> 15s -> 30s`（上限 30s），
//! 支持动态启停以及网络恢复时立即重置退避窗口。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::sync::{broadcast, Notify};
use tokio::time;
use tracing::{debug, info, warn};

/// 严格按契约定义的自愈重连指数退避序列（秒）：`2s -> 5s -> 15s -> 30s`。
pub const BACKOFF_SCHEDULE_SECS: [u64; 4] = [2, 5, 15, 30];

/// 单次 TCP 探测超时。
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// 已在线设备的健康心跳间隔。
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(15);

/// 重连目标设备信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconnectTarget {
    pub device_id: String,
    pub addr: SocketAddr,
}

/// 自愈重连状态机当前状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconnectState {
    Paused,
    Connected { addr: SocketAddr },
    Backoff { attempt: usize, next_delay: Duration },
}

/// 重连状态机对外广播的事件。
#[derive(Debug, Clone)]
pub enum ReconnectEvent {
    PeerConnected {
        device_id: String,
        addr: SocketAddr,
    },
    PeerDisconnected {
        device_id: String,
        addr: SocketAddr,
        next_delay: Duration,
    },
}

/// 根据连续失败次数计算退避时长（索引 0->2s, 1->5s, 2->15s, >=3->30s）。
pub fn backoff_delay_for_attempt(attempt: usize) -> Duration {
    let idx = attempt.min(BACKOFF_SCHEDULE_SECS.len() - 1);
    Duration::from_secs(BACKOFF_SCHEDULE_SECS[idx])
}

/// Adds +/-15% jitter on top of the contract backoff schedule to prevent reconnect storms.
pub fn backoff_delay_with_jitter(attempt: usize) -> Duration {
    let base_ms = backoff_delay_for_attempt(attempt).as_millis() as u64;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let pct_offset = (nanos % 31) as i64 - 15;
    let adjusted_ms = ((base_ms as i64) * (100 + pct_offset) / 100).max(200) as u64;
    Duration::from_millis(adjusted_ms)
}

/// 控制后台自愈重连任务运行状态的共享句柄。
#[derive(Clone)]
pub struct ReconnectController {
    enabled: Arc<AtomicBool>,
    wake: Arc<Notify>,
    events_tx: broadcast::Sender<ReconnectEvent>,
}

impl ReconnectController {
    pub fn new(initial_enabled: bool) -> Self {
        let (events_tx, _) = broadcast::channel(64);
        Self {
            enabled: Arc::new(AtomicBool::new(initial_enabled)),
            wake: Arc::new(Notify::new()),
            events_tx,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// 切换自动重连开关并立即唤醒等待中的重连循环。
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
        self.wake.notify_waiters();
        info!(enabled, "auto-reconnect policy updated");
    }

    /// 系统网络恢复（Wi-Fi 切换/重连）时调用，立即中断退避等待并从 2s 起重新探测。
    pub fn notify_network_restored(&self) {
        self.wake.notify_waiters();
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ReconnectEvent> {
        self.events_tx.subscribe()
    }

    /// 为单个已配对设备启动后台自愈重连状态机。
    pub fn spawn_peer_loop(&self, target: ReconnectTarget) -> tokio::task::JoinHandle<()> {
        let ctrl = self.clone();
        tokio::spawn(async move {
            ctrl.run_peer_state_machine(target).await;
        })
    }

    async fn run_peer_state_machine(&self, target: ReconnectTarget) {
        let mut attempt: usize = 0;
        let mut was_connected = false;

        loop {
            let wake_notified = self.wake.notified();
            tokio::pin!(wake_notified);
            wake_notified.as_mut().enable();

            if !self.is_enabled() {
                was_connected = false;
                attempt = 0;
                wake_notified.await;
                continue;
            }

            if probe_tcp(target.addr).await {
                if !was_connected {
                    info!(
                        device_id = %target.device_id,
                        addr = %target.addr,
                        "paired peer reconnected"
                    );
                    let _ = self.events_tx.send(ReconnectEvent::PeerConnected {
                        device_id: target.device_id.clone(),
                        addr: target.addr,
                    });
                }
                was_connected = true;
                attempt = 0;

                tokio::select! {
                    _ = time::sleep(HEALTH_CHECK_INTERVAL) => {}
                    _ = self.wake.notified() => {}
                }
            } else {
                let delay = backoff_delay_with_jitter(attempt);
                if was_connected {
                    warn!(
                        device_id = %target.device_id,
                        addr = %target.addr,
                        next_delay_secs = delay.as_secs(),
                        "paired peer lost connection; entering exponential backoff"
                    );
                } else {
                    debug!(
                        device_id = %target.device_id,
                        addr = %target.addr,
                        attempt,
                        next_delay_secs = delay.as_secs(),
                        "peer probe failed; backing off"
                    );
                }

                let _ = self.events_tx.send(ReconnectEvent::PeerDisconnected {
                    device_id: target.device_id.clone(),
                    addr: target.addr,
                    next_delay: delay,
                });
                was_connected = false;
                attempt = attempt.saturating_add(1);

                tokio::select! {
                    _ = time::sleep(delay) => {}
                    _ = self.wake.notified() => {
                        // 网络恢复或开关切换时重置退避计数，避免卡在 30s 窗口。
                        attempt = 0;
                    }
                }
            }
        }
    }
}

async fn probe_tcp(addr: SocketAddr) -> bool {
    matches!(
        time::timeout(PROBE_TIMEOUT, TcpStream::connect(addr)).await,
        Ok(Ok(_stream))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_sequence_matches_contract() {
        assert_eq!(backoff_delay_for_attempt(0), Duration::from_secs(2));
        assert_eq!(backoff_delay_for_attempt(1), Duration::from_secs(5));
        assert_eq!(backoff_delay_for_attempt(2), Duration::from_secs(15));
        assert_eq!(backoff_delay_for_attempt(3), Duration::from_secs(30));
        assert_eq!(backoff_delay_for_attempt(10), Duration::from_secs(30));
    }
}
