//! 持久化非阻塞多目标消息发件箱引擎 (`message_outbox` / `message_outbox_targets`)。
//!
//! 职责：
//! - `enqueue_text_message` / `enqueue_file_message`：立即冻结目标集合并持久化至 SQLite
//!   （文件先校验 `<= MAX_TEMP_FILE_BYTES`、计算 BLAKE3 并拷贝至暂存目录），返回 `MessageOutboxRecord`；
//! - `retry_failed_outbox_message`：仅重置失败目标为待重试，已成功目标绝不重复发送；
//! - `dispatch_pending_outbox`：基于 `tokio::sync::Semaphore` 的有界并发后台投递，
//!   指数退避 `[2, 5, 15, 30]`s（最多 3 次瞬态重试，401/403/404/413 立即永久失败），
//!   上传前校验暂存文件 BLAKE3，兼容 v1 服务端返回 `unknown_legacy`。

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::stream::{FuturesUnordered, StreamExt};
use lanvault_core::{
    blake3_hash, blake3_hash_reader, Database, MessageOutboxRecord, MessageOutboxTargetRecord,
    SymmetricKey,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::clipboard::{ClipboardClient, MAX_TEMP_FILE_BYTES};
use crate::error::ClientError;

/// 默认指数退避序列（秒）。
pub const OUTBOX_BACKOFF_SECS: [i64; 4] = [2, 5, 15, 30];

/// 默认最大瞬态重试次数。
pub const OUTBOX_MAX_TRANSIENT_RETRIES: i64 = 3;

/// 入队时冻结的目标设备快照。
#[derive(Clone)]
pub struct FrozenOutboxTarget {
    pub device_id: String,
    pub device_name: String,
    pub endpoint_base_url: String,
    pub session_cookie: Option<String>,
    pub channel_key: SymmetricKey,
}

/// 发件箱调度配置。
#[derive(Debug, Clone)]
pub struct OutboxDispatchConfig {
    pub batch_limit: usize,
    pub max_transient_retries: i64,
    pub backoff_secs: Vec<i64>,
}

impl Default for OutboxDispatchConfig {
    fn default() -> Self {
        Self {
            batch_limit: 64,
            max_transient_retries: OUTBOX_MAX_TRANSIENT_RETRIES,
            backoff_secs: OUTBOX_BACKOFF_SECS.to_vec(),
        }
    }
}

/// 单轮发件箱调度汇总报告。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutboxDispatchReport {
    pub attempted: usize,
    pub sent: usize,
    pub scheduled_retry: usize,
    pub failed: usize,
    pub unknown_legacy: usize,
}

/// 将文本消息与冻结目标集合原子写入发件箱，不阻塞网络发送。
pub fn enqueue_text_message(
    db: &Database,
    conversation_key: &str,
    targets: &[FrozenOutboxTarget],
    text: &str,
) -> Result<MessageOutboxRecord, ClientError> {
    if conversation_key.trim().is_empty() {
        return Err(ClientError::InvalidInput(
            "conversation_key must not be empty".to_string(),
        ));
    }
    if text.is_empty() {
        return Err(ClientError::InvalidInput(
            "text message content must not be empty".to_string(),
        ));
    }
    if targets.is_empty() {
        return Err(ClientError::InvalidInput(
            "outbox message must have at least one target".to_string(),
        ));
    }

    let now = unix_now_secs();
    let client_msg_id = Uuid::new_v4().to_string();
    let title: String = text.chars().take(64).collect();
    let mut record = MessageOutboxRecord {
        client_msg_id: client_msg_id.clone(),
        local_seq: 0,
        session_key: conversation_key.to_string(),
        kind: "text".to_string(),
        title,
        content: Some(text.to_string()),
        file_path: None,
        file_hash_hex: None,
        size_bytes: text.len() as u64,
        overall_status: "sending".to_string(),
        created_at: now,
        updated_at: now,
    };

    let target_records = build_target_records(&client_msg_id, targets, now);
    let seq = db.enqueue_outbox_message(&record, &target_records)?;
    record.local_seq = seq;
    Ok(record)
}

/// 将文件消息暂存至 `staging_dir`（校验 `<= MAX_TEMP_FILE_BYTES` 并核对 BLAKE3），
/// 然后原子写入发件箱，确保后续重试安全且源内容可验证。
pub fn enqueue_file_message(
    db: &Database,
    staging_dir: &Path,
    conversation_key: &str,
    targets: &[FrozenOutboxTarget],
    file_path: &Path,
) -> Result<MessageOutboxRecord, ClientError> {
    if conversation_key.trim().is_empty() {
        return Err(ClientError::InvalidInput(
            "conversation_key must not be empty".to_string(),
        ));
    }
    if targets.is_empty() {
        return Err(ClientError::InvalidInput(
            "outbox file message must have at least one target".to_string(),
        ));
    }

    let meta = fs::symlink_metadata(file_path).map_err(|err| ClientError::InvalidPath {
        path: file_path.to_path_buf(),
        reason: format!("cannot stat outbox file: {err}"),
    })?;
    if !meta.file_type().is_file() {
        return Err(ClientError::InvalidPath {
            path: file_path.to_path_buf(),
            reason: "outbox file must be a regular file".to_string(),
        });
    }
    if meta.len() > MAX_TEMP_FILE_BYTES {
        return Err(ClientError::InvalidInput(format!(
            "outbox file exceeds limit ({} > {} bytes)",
            meta.len(),
            MAX_TEMP_FILE_BYTES
        )));
    }

    let filename = file_path
        .file_name()
        .ok_or_else(|| ClientError::InvalidPath {
            path: file_path.to_path_buf(),
            reason: "missing file name".to_string(),
        })?
        .to_string_lossy()
        .into_owned();

    let source_hash = {
        let f = File::open(file_path)?;
        let mut reader = BufReader::with_capacity(256 * 1024, f);
        blake3_hash_reader(&mut reader)?
    };
    let hash_hex = hex::encode(source_hash);

    let client_msg_id = Uuid::new_v4().to_string();
    fs::create_dir_all(staging_dir)?;
    let staged_name = format!("{client_msg_id}_{}", sanitize_stage_filename(&filename));
    let staged_path = staging_dir.join(staged_name);
    fs::copy(file_path, &staged_path)?;

    let staged_hash = {
        let f = File::open(&staged_path)?;
        let mut reader = BufReader::with_capacity(256 * 1024, f);
        blake3_hash_reader(&mut reader)?
    };
    if staged_hash != source_hash {
        let _ = fs::remove_file(&staged_path);
        return Err(ClientError::InvalidInput(format!(
            "source file `{}` changed while staging for outbox",
            file_path.display()
        )));
    }

    let now = unix_now_secs();
    let mut record = MessageOutboxRecord {
        client_msg_id: client_msg_id.clone(),
        local_seq: 0,
        session_key: conversation_key.to_string(),
        kind: "file".to_string(),
        title: filename,
        content: None,
        file_path: Some(staged_path.to_string_lossy().into_owned()),
        file_hash_hex: Some(hash_hex),
        size_bytes: meta.len(),
        overall_status: "sending".to_string(),
        created_at: now,
        updated_at: now,
    };

    let target_records = build_target_records(&client_msg_id, targets, now);
    let seq = db.enqueue_outbox_message(&record, &target_records)?;
    record.local_seq = seq;
    Ok(record)
}

/// 仅将 `client_msg_id` 下状态为 `failed` 的目标重置为待发送（已成功目标保持不变）。
pub fn retry_failed_outbox_message(
    db: &Database,
    client_msg_id: &str,
) -> Result<usize, ClientError> {
    Ok(db.reset_failed_outbox_targets_for_retry(client_msg_id)?)
}

enum DispatchOutcome {
    Sent,
    UnknownLegacy,
    TransientError {
        error_code: String,
        error_message: String,
    },
    PermanentError {
        error_code: String,
        error_message: String,
    },
}

/// 调度执行所有到期的待发送发件箱目标（有界并发 + 指数退避 + 暂存文件 BLAKE3 校验）。
pub async fn dispatch_pending_outbox<F>(
    db: &Database,
    resolve_session: F,
    max_concurrency: usize,
) -> Result<OutboxDispatchReport, ClientError>
where
    F: Fn(
        &str,
    ) -> Option<(
        String,
        String,
        SymmetricKey,
        crate::transport::PeerFingerprint,
    )>,
{
    dispatch_pending_outbox_with_config(
        db,
        resolve_session,
        max_concurrency,
        &OutboxDispatchConfig::default(),
    )
    .await
}

/// 使用自定义退避与批量配置调度待发送发件箱目标。
pub async fn dispatch_pending_outbox_with_config<F>(
    db: &Database,
    resolve_session: F,
    max_concurrency: usize,
    config: &OutboxDispatchConfig,
) -> Result<OutboxDispatchReport, ClientError>
where
    F: Fn(
        &str,
    ) -> Option<(
        String,
        String,
        SymmetricKey,
        crate::transport::PeerFingerprint,
    )>,
{
    let now = unix_now_secs();
    let pending_targets = db.list_pending_outbox_targets(now, config.batch_limit)?;
    if pending_targets.is_empty() {
        return Ok(OutboxDispatchReport::default());
    }

    let messages: HashMap<String, MessageOutboxRecord> = db
        .list_outbox_messages(None, 10_000)?
        .into_iter()
        .map(|m| (m.client_msg_id.clone(), m))
        .collect();

    let sem = Arc::new(Semaphore::new(max_concurrency.max(1)));
    let mut inflight = FuturesUnordered::new();

    for target in pending_targets {
        let Some(msg) = messages.get(&target.client_msg_id).cloned() else {
            continue;
        };
        let resolved = resolve_session(&target.target_device_id);
        let sem_clone = Arc::clone(&sem);

        inflight.push(async move {
            let _permit = sem_clone
                .acquire_owned()
                .await
                .expect("outbox semaphore open");
            let outcome = deliver_single_target(&msg, &target, resolved).await;
            (target, outcome)
        });
    }

    let mut report = OutboxDispatchReport::default();
    while let Some((target, outcome)) = inflight.next().await {
        report.attempted += 1;
        let next_attempt = target.attempt_count + 1;
        match outcome {
            DispatchOutcome::Sent => {
                db.update_outbox_target_status(
                    &target.client_msg_id,
                    &target.target_device_id,
                    "sent",
                    None,
                    None,
                    next_attempt,
                    None,
                )?;
                report.sent += 1;
            }
            DispatchOutcome::UnknownLegacy => {
                db.update_outbox_target_status(
                    &target.client_msg_id,
                    &target.target_device_id,
                    "unknown_legacy",
                    None,
                    Some("remote v1 server did not echo client_msg_id"),
                    next_attempt,
                    None,
                )?;
                report.unknown_legacy += 1;
            }
            DispatchOutcome::PermanentError {
                error_code,
                error_message,
            } => {
                db.update_outbox_target_status(
                    &target.client_msg_id,
                    &target.target_device_id,
                    "failed",
                    Some(&error_code),
                    Some(&error_message),
                    next_attempt,
                    None,
                )?;
                report.failed += 1;
            }
            DispatchOutcome::TransientError {
                error_code,
                error_message,
            } => {
                if next_attempt > config.max_transient_retries {
                    db.update_outbox_target_status(
                        &target.client_msg_id,
                        &target.target_device_id,
                        "failed",
                        Some(&error_code),
                        Some(&error_message),
                        next_attempt,
                        None,
                    )?;
                    report.failed += 1;
                } else {
                    let backoff_idx = usize::try_from(next_attempt - 1)
                        .unwrap_or(0)
                        .min(config.backoff_secs.len().saturating_sub(1));
                    let delay = config.backoff_secs.get(backoff_idx).copied().unwrap_or(30);
                    let retry_at = unix_now_secs() + delay;
                    db.update_outbox_target_status(
                        &target.client_msg_id,
                        &target.target_device_id,
                        "pending",
                        Some(&error_code),
                        Some(&error_message),
                        next_attempt,
                        Some(retry_at),
                    )?;
                    report.scheduled_retry += 1;
                }
            }
        }
    }

    Ok(report)
}

async fn deliver_single_target(
    msg: &MessageOutboxRecord,
    target: &MessageOutboxTargetRecord,
    resolved: Option<(
        String,
        String,
        SymmetricKey,
        crate::transport::PeerFingerprint,
    )>,
) -> DispatchOutcome {
    let Some((endpoint_base_url, session_cookie, channel_key, pin)) = resolved else {
        return DispatchOutcome::TransientError {
            error_code: "session_unavailable".to_string(),
            error_message: format!("no active session for target `{}`", target.target_device_id),
        };
    };

    let cookie_opt = if session_cookie.is_empty() {
        None
    } else {
        Some(session_cookie)
    };
    let client = match ClipboardClient::new(endpoint_base_url, cookie_opt, channel_key, pin) {
        Ok(c) => c,
        Err(err) => {
            return DispatchOutcome::PermanentError {
                error_code: "client_init_failed".to_string(),
                error_message: err.to_string(),
            };
        }
    };

    let send_res = match msg.kind.as_str() {
        "text" => {
            let Some(text) = msg.content.as_deref() else {
                return DispatchOutcome::PermanentError {
                    error_code: "missing_content".to_string(),
                    error_message: "text outbox message has no content".to_string(),
                };
            };
            client
                .push_text_with_id(
                    &msg.client_msg_id,
                    Some(target.target_device_id.clone()),
                    text,
                )
                .await
        }
        "file" => {
            let Some(staged_path) = msg.file_path.as_deref() else {
                return DispatchOutcome::PermanentError {
                    error_code: "missing_staged_file".to_string(),
                    error_message: "file outbox message has no staged file_path".to_string(),
                };
            };
            let bytes = match tokio::fs::read(staged_path).await {
                Ok(b) => b,
                Err(err) => {
                    return DispatchOutcome::PermanentError {
                        error_code: "staged_file_unreadable".to_string(),
                        error_message: format!("cannot read staged file `{staged_path}`: {err}"),
                    };
                }
            };
            if let Some(expected_hex) = msg.file_hash_hex.as_deref() {
                let actual_hex = hex::encode(blake3_hash(&bytes));
                if actual_hex != expected_hex {
                    return DispatchOutcome::PermanentError {
                        error_code: "staged_file_corrupt".to_string(),
                        error_message: format!(
                            "staged file BLAKE3 mismatch (expected {expected_hex}, got {actual_hex})"
                        ),
                    };
                }
            }
            client
                .push_file_bytes_with_id(
                    &msg.client_msg_id,
                    Some(target.target_device_id.clone()),
                    &msg.title,
                    &bytes,
                )
                .await
        }
        other => {
            return DispatchOutcome::PermanentError {
                error_code: "unsupported_kind".to_string(),
                error_message: format!("unsupported outbox message kind `{other}`"),
            };
        }
    };

    match send_res {
        Ok(resp) => {
            if resp.client_msg_id.is_empty() {
                DispatchOutcome::UnknownLegacy
            } else {
                DispatchOutcome::Sent
            }
        }
        Err(ClientError::RemoteStatus { status, url }) if is_permanent_http_status(status) => {
            DispatchOutcome::PermanentError {
                error_code: format!("http_{status}"),
                error_message: format!("remote `{url}` rejected message with HTTP {status}"),
            }
        }
        Err(ClientError::RemoteRejected { status, body }) if is_permanent_http_status(status) => {
            DispatchOutcome::PermanentError {
                error_code: format!("http_{status}"),
                error_message: body,
            }
        }
        Err(err) => DispatchOutcome::TransientError {
            error_code: "transport_error".to_string(),
            error_message: err.to_string(),
        },
    }
}

fn is_permanent_http_status(status: u16) -> bool {
    matches!(status, 401 | 403 | 404 | 413)
}

fn build_target_records(
    client_msg_id: &str,
    targets: &[FrozenOutboxTarget],
    now: i64,
) -> Vec<MessageOutboxTargetRecord> {
    let mut seen = HashSet::with_capacity(targets.len());
    let mut records = Vec::with_capacity(targets.len());
    for t in targets {
        if !seen.insert(t.device_id.clone()) {
            continue;
        }
        records.push(MessageOutboxTargetRecord {
            client_msg_id: client_msg_id.to_string(),
            target_device_id: t.device_id.clone(),
            channel: "clipboard".to_string(),
            status: "pending".to_string(),
            error_code: None,
            error_message: None,
            attempt_count: 0,
            next_retry_at: None,
            updated_at: now,
        });
    }
    records
}

fn sanitize_stage_filename(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn unix_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
