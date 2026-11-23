//! 增量块上传管线：流式复用 `lanvault_core::{chunk_stream, pack_chunk, unpack_chunk}`，
//! 集成令牌桶限速 (`max_kbps`)、场景化省电门控 (`wifi_only`, `allowed_ssids`, `require_charging`)
//! 与全量分块上传后的快照提交闭环 (`POST /api/vaults/{vault_id}/snapshots`)。

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::BufReader;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lanvault_core::{
    blake3_hash, chunk_stream, pack_chunk_with_context, unpack_chunk, ChunkParams,
    ChunkSealContext, Database, LocalFileStateRecord, PackedChunk, ProbeGuard, SymmetricKey, CHUNK_MAX_SIZE,
    DEFAULT_ZSTD_LEVEL,
};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, Mutex, RwLock, Semaphore};
use tokio::time;
use tracing::{debug, info};
use uuid::Uuid;

use crate::error::ClientError;
use crate::scanner::{delete_file_states, DiffResult, FileMetadataSnapshot};

/// Result of one chunk PUT: `(stored_hash, raw_size, server_side_dedup)`.
type ChunkUploadOutcome = Result<([u8; 32], usize, bool), ClientError>;

/// Bounded cap for client-side remote chunk dedup cache (~3.2 MiB max RAM).
const MAX_KNOWN_REMOTE_CHUNKS: usize = 100_000;

/// 场景化省电与网络门控策略。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerNetworkPolicy {
    pub wifi_only: bool,
    pub allowed_ssids: Vec<String>,
    pub require_charging: bool,
    /// 上传限速（KiB/s），`0` 表示不限速。
    pub max_kbps: u32,
}

impl Default for PowerNetworkPolicy {
    fn default() -> Self {
        Self {
            wifi_only: true,
            allowed_ssids: Vec::new(),
            require_charging: false,
            max_kbps: 0,
        }
    }
}

/// 当前设备运行环境快照（由平台层或 FFI 注入）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceEnvironment {
    pub on_wifi: bool,
    pub current_ssid: Option<String>,
    pub is_charging: bool,
}

impl Default for DeviceEnvironment {
    fn default() -> Self {
        Self {
            on_wifi: true,
            current_ssid: None,
            is_charging: true,
        }
    }
}

/// 门控拦截原因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicyBlockReason {
    NotOnWifi,
    SsidNotAllowed { current_ssid: Option<String> },
    NotCharging,
}

impl PowerNetworkPolicy {
    /// 评估当前设备状态是否满足全速扫描与上传条件。
    pub fn evaluate(&self, env: &DeviceEnvironment) -> Result<(), PolicyBlockReason> {
        if self.wifi_only && !env.on_wifi {
            return Err(PolicyBlockReason::NotOnWifi);
        }
        if !self.allowed_ssids.is_empty() {
            let matched = env
                .current_ssid
                .as_deref()
                .is_some_and(|ssid| self.allowed_ssids.iter().any(|allowed| allowed == ssid));
            if !matched {
                return Err(PolicyBlockReason::SsidNotAllowed {
                    current_ssid: env.current_ssid.clone(),
                });
            }
        }
        if self.require_charging && !env.is_charging {
            return Err(PolicyBlockReason::NotCharging);
        }
        Ok(())
    }
}

/// 令牌桶上传限速器（线程安全，支持运行时动态调整 `max_kbps`）。
pub struct TokenBucket {
    inner: Mutex<BucketState>,
}

struct BucketState {
    rate_bytes_per_sec: f64,
    capacity: f64,
    tokens: f64,
    last_refill: Instant,
}

impl TokenBucket {
    pub fn new(max_kbps: u32) -> Self {
        let rate = (max_kbps as f64) * 1024.0;
        Self {
            inner: Mutex::new(BucketState {
                rate_bytes_per_sec: rate,
                capacity: rate.max(64.0 * 1024.0),
                tokens: rate.max(64.0 * 1024.0),
                last_refill: Instant::now(),
            }),
        }
    }

    pub async fn update_max_kbps(&self, max_kbps: u32) {
        let mut state = self.inner.lock().await;
        let rate = (max_kbps as f64) * 1024.0;
        state.rate_bytes_per_sec = rate;
        state.capacity = rate.max(64.0 * 1024.0);
        state.tokens = state.tokens.min(state.capacity);
        state.last_refill = Instant::now();
    }

    /// 申请发送 `bytes` 字节的令牌；若限速为 0 则立即放行，否则精确休眠补齐赤字。
    pub async fn acquire(&self, bytes: usize) {
        let wait = {
            let mut state = self.inner.lock().await;
            if state.rate_bytes_per_sec <= 0.0 {
                return;
            }
            let now = Instant::now();
            let elapsed = now.saturating_duration_since(state.last_refill).as_secs_f64();
            state.tokens = (state.tokens + elapsed * state.rate_bytes_per_sec).min(state.capacity);
            state.last_refill = now;

            let needed = bytes as f64;
            if state.tokens >= needed {
                state.tokens -= needed;
                return;
            }
            let deficit = needed - state.tokens;
            state.tokens = 0.0;
            let wait = Duration::from_secs_f64(deficit / state.rate_bytes_per_sec);
            state.last_refill = now + wait;
            wait
        };
        time::sleep(wait).await;
    }
}

/// 同步进度实时事件（供 FFI `StreamSink<SyncProgressEvent>` 订阅）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncProgress {
    pub rule_id: String,
    pub current_file: String,
    pub bytes_processed: u64,
    pub total_bytes: u64,
    pub chunks_uploaded: u64,
    pub chunks_deduplicated: u64,
    pub throughput_bps: u64,
    pub status: String,
}

#[derive(Debug, Serialize)]
struct SnapshotFileManifestEntry {
    rel_path: String,
    size: u64,
    mtime_ns: i64,
    chunk_hashes_hex: Vec<String>,
}

#[derive(Debug, Serialize)]
struct CommitSnapshotRequest {
    snapshot_id: String,
    rule_id: String,
    device_id: String,
    root_tree_hash_hex: String,
    retention_tier: String,
    files: Vec<SnapshotFileManifestEntry>,
}

/// 客户端增量同步引擎。
pub struct SyncEngine {
    http: Client,
    endpoint_base_url: String,
    session_cookie: Option<String>,
    device_id: String,
    vault_master_key: SymmetricKey,
    policy: Arc<RwLock<PowerNetworkPolicy>>,
    env: Arc<RwLock<DeviceEnvironment>>,
    rate_limiter: Arc<TokenBucket>,
    progress_tx: broadcast::Sender<SyncProgress>,
    known_remote_chunks: Arc<RwLock<HashSet<(String, [u8; 32])>>>,
}

impl SyncEngine {
    pub fn new(
        endpoint_base_url: String,
        session_cookie: Option<String>,
        device_id: String,
        vault_master_key: SymmetricKey,
        initial_policy: PowerNetworkPolicy,
        environment: DeviceEnvironment,
        shared_rate_limiter: Option<Arc<TokenBucket>>,
    ) -> Result<Self, ClientError> {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
        let rate_limiter = shared_rate_limiter
            .unwrap_or_else(|| Arc::new(TokenBucket::new(initial_policy.max_kbps)));
        let (progress_tx, _) = broadcast::channel(256);

        Ok(Self {
            http,
            endpoint_base_url: endpoint_base_url.trim_end_matches('/').to_string(),
            session_cookie,
            device_id,
            vault_master_key,
            policy: Arc::new(RwLock::new(initial_policy)),
            env: Arc::new(RwLock::new(environment)),
            rate_limiter,
            progress_tx,
            known_remote_chunks: Arc::new(RwLock::new(HashSet::new())),
        })
    }

    pub fn subscribe_progress(&self) -> broadcast::Receiver<SyncProgress> {
        self.progress_tx.subscribe()
    }

    pub async fn update_policy(&self, policy: PowerNetworkPolicy) {
        self.rate_limiter.update_max_kbps(policy.max_kbps).await;
        *self.policy.write().await = policy;
    }

    pub async fn update_environment(&self, env: DeviceEnvironment) {
        *self.env.write().await = env;
    }

    /// 解密并解压单个远端密文块（统一使用 `derive_vault_chunk_key(vault_id)` 与 `aad = vault_id.as_bytes()`）。
    pub fn restore_chunk(
        &self,
        vault_id: &str,
        stored_bytes: &[u8],
        expected_stored_hash: &[u8; 32],
    ) -> Result<Vec<u8>, ClientError> {
        let chunk_key = self.vault_master_key.derive_vault_chunk_key(vault_id);
        let raw = unpack_chunk(
            stored_bytes,
            expected_stored_hash,
            Some(&chunk_key),
            vault_id.as_bytes(),
            CHUNK_MAX_SIZE as usize,
        )?;
        Ok(raw)
    }

    /// 对 `DiffResult` 中的所有新增与修改文件执行流式 FastCDC -> zstd -> E2EE 增量上传，
    /// 同步清理本地已删除记录，并在全部块落盘后向 `POST /api/vaults/{vault_id}/snapshots` 提交快照清单闭环。
    pub async fn sync_diff(
        &self,
        db: &Database,
        rule_id: &str,
        vault_id: &str,
        diff: &DiffResult,
    ) -> Result<SyncSummary, ClientError> {
        let policy = self.policy.read().await.clone();
        let env = self.env.read().await.clone();
        if let Err(reason) = policy.evaluate(&env) {
            return Err(ClientError::PolicyBlocked(reason));
        }

        let total_bytes: u64 = diff.changed_files().map(|f| f.size).sum();
        let started = Instant::now();
        let mut summary = SyncSummary::default();
        let chunk_key = self.vault_master_key.derive_vault_chunk_key(vault_id);
        let mut pending_db_states: Vec<LocalFileStateRecord> =
            Vec::with_capacity(diff.added.len() + diff.modified.len());

        for snapshot in diff.changed_files() {
            // 每处理一个文件前复检省电与网络门控，支持中途拔电或切流量立即暂停。
            let current_policy = self.policy.read().await.clone();
            let current_env = self.env.read().await.clone();
            if let Err(reason) = current_policy.evaluate(&current_env) {
                return Err(ClientError::PolicyBlocked(reason));
            }

            let file_hashes = self
                .sync_single_file(
                    rule_id,
                    vault_id,
                    &chunk_key,
                    snapshot,
                    total_bytes,
                    started,
                    &mut summary,
                )
                .await?;

            // 将该文件全部 32 字节块哈希扁平拼接写入 SQLite `local_file_state.chunk_hashes`。
            let mut blob = Vec::with_capacity(file_hashes.len() * 32);
            for h in &file_hashes {
                blob.extend_from_slice(h);
            }
            pending_db_states.push(LocalFileStateRecord {
                rule_id: rule_id.to_string(),
                target_vault_id: vault_id.to_string(),
                rel_path: snapshot.rel_path.clone(),
                size: snapshot.size,
                mtime_ns: snapshot.mtime_ns,
                file_id: snapshot.file_id,
                chunk_hashes: blob,
            });
            summary.files_synced += 1;
        }

        if !diff.is_empty() {
            let deleted: HashSet<&str> = diff.deleted.iter().map(String::as_str).collect();
            let mut projected_map: HashMap<String, LocalFileStateRecord> = db
                .list_local_file_states(rule_id)?
                .into_iter()
                .filter(|state| !deleted.contains(state.rel_path.as_str()))
                .map(|state| (state.rel_path.clone(), state))
                .collect();
            for pending in &pending_db_states {
                projected_map.insert(pending.rel_path.clone(), pending.clone());
            }
            let mut projected_states: Vec<LocalFileStateRecord> =
                projected_map.into_values().collect();
            let snapshot_id = self
                .commit_snapshot(rule_id, vault_id, &mut projected_states)
                .await?;
            summary.snapshot_id = Some(snapshot_id);

            if !pending_db_states.is_empty() {
                db.upsert_local_file_states_batch(&pending_db_states)?;
            }
            if !diff.deleted.is_empty() {
                delete_file_states(db, rule_id, &diff.deleted)?;
            }
        }

        info!(
            rule_id = %rule_id,
            vault_id = %vault_id,
            files = summary.files_synced,
            uploaded_chunks = summary.chunks_uploaded,
            dedup_chunks = summary.chunks_deduplicated,
            bytes = summary.bytes_processed,
            snapshot_id = ?summary.snapshot_id,
            "incremental sync pass and snapshot commit completed"
        );

        Ok(summary)
    }

    #[allow(clippy::too_many_arguments)]
    async fn sync_single_file(
        &self,
        rule_id: &str,
        vault_id: &str,
        chunk_key: &SymmetricKey,
        snapshot: &FileMetadataSnapshot,
        total_bytes: u64,
        started: Instant,
        summary: &mut SyncSummary,
    ) -> Result<Vec<[u8; 32]>, ClientError> {
        let mut file_probe = ProbeGuard::start(
            "client.sync",
            "sync.parallel_pack_upload",
            Some(format!("file={}", snapshot.rel_path)),
        );
        let seal_ctx = Arc::new(ChunkSealContext::new(chunk_key, vault_id.as_bytes()));
        let cpu_workers = std::thread::available_parallelism()
            .map(|n| n.get().clamp(2, 16))
            .unwrap_or(4);
        let upload_concurrency = cpu_workers.min(8);

        let abs_path = snapshot.abs_path.clone();
        let expected_size = snapshot.size;
        let expected_mtime_ns = snapshot.mtime_ns;
        let (tx, mut rx) = mpsc::channel::<
            Result<(usize, tokio::task::JoinHandle<Result<PackedChunk, ClientError>>), ClientError>,
        >(upload_concurrency * 2);

        let seal_for_producer = Arc::clone(&seal_ctx);
        let producer = tokio::task::spawn_blocking(move || {
            let file = match File::open(&abs_path) {
                Ok(f) => f,
                Err(err) => {
                    let _ = tx.blocking_send(Err(ClientError::Io(err)));
                    return;
                }
            };
            let reader = BufReader::with_capacity(256 * 1024, file);
            let mut seq_idx: usize = 0;
            let res = chunk_stream(reader, ChunkParams::default(), |_desc, raw_bytes| {
                let idx = seq_idx;
                seq_idx += 1;
                let ctx = Arc::clone(&seal_for_producer);
                let pack_handle = tokio::task::spawn_blocking(move || {
                    pack_chunk_with_context(&raw_bytes, Some(&ctx), DEFAULT_ZSTD_LEVEL)
                        .map_err(ClientError::Core)
                });
                if tx.blocking_send(Ok((idx, pack_handle))).is_err() {
                    return Err(lanvault_core::LanVaultError::Io(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "chunk upload consumer stopped",
                    )));
                }
                Ok(())
            });
            if let Err(err) = res {
                let _ = tx.blocking_send(Err(ClientError::Core(err)));
                return;
            }
            match fs::metadata(&abs_path) {
                Ok(metadata)
                    if metadata.len() == expected_size
                        && metadata
                            .modified()
                            .ok()
                            .and_then(|modified| {
                                modified
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .ok()
                                    .map(|duration| duration.as_nanos() as i64)
                            })
                            == Some(expected_mtime_ns) => {}
                Ok(_) => {
                    let _ = tx.blocking_send(Err(ClientError::InvalidInput(format!(
                        "file changed while it was being backed up: {}",
                        abs_path.display()
                    ))));
                }
                Err(err) => {
                    let _ = tx.blocking_send(Err(ClientError::Io(err)));
                }
            }
        });

        let mut ordered_hashes: Vec<(usize, [u8; 32])> = Vec::new();
        let upload_sem = Arc::new(Semaphore::new(upload_concurrency));
        let mut inflight_uploads: tokio::task::JoinSet<ChunkUploadOutcome> =
            tokio::task::JoinSet::new();
        let mut loop_err: Option<ClientError> = None;

        while let Some(item_res) = rx.recv().await {
            let (idx, pack_handle) = match item_res {
                Ok(pair) => pair,
                Err(err) => {
                    loop_err = Some(err);
                    break;
                }
            };
            let packed = match pack_handle.await {
                Ok(Ok(p)) => p,
                Ok(Err(err)) => {
                    loop_err = Some(err);
                    break;
                }
                Err(err) => {
                    loop_err = Some(ClientError::Io(std::io::Error::other(format!(
                        "chunk pack worker panicked: {err}"
                    ))));
                    break;
                }
            };

            ordered_hashes.push((idx, packed.stored_hash));
            file_probe.add_bytes(packed.raw_size as u64);

            let cache_key = (vault_id.to_string(), packed.stored_hash);
            let already_exists = self
                .known_remote_chunks
                .read()
                .await
                .contains(&cache_key);

            if already_exists {
                summary.chunks_deduplicated += 1;
                summary.bytes_processed += packed.raw_size as u64;
                let elapsed_secs = started.elapsed().as_secs_f64().max(0.001);
                let throughput_bps = (summary.bytes_processed as f64 / elapsed_secs) as u64;
                let _ = self.progress_tx.send(SyncProgress {
                    rule_id: rule_id.to_string(),
                    current_file: snapshot.rel_path.clone(),
                    bytes_processed: summary.bytes_processed,
                    total_bytes,
                    chunks_uploaded: summary.chunks_uploaded,
                    chunks_deduplicated: summary.chunks_deduplicated,
                    throughput_bps,
                    status: "syncing".to_string(),
                });
                continue;
            }

            let permit = upload_sem
                .clone()
                .acquire_owned()
                .await
                .expect("upload semaphore closed");
            self.rate_limiter.acquire(packed.stored_bytes.len()).await;

            let http = self.http.clone();
            let base = self.endpoint_base_url.clone();
            let cookie = self.session_cookie.clone();
            let vid = vault_id.to_string();
            let raw_size = packed.raw_size;
            let stored_hash = packed.stored_hash;
            let stored_bytes = packed.stored_bytes;

            inflight_uploads.spawn(async move {
                let _permit = permit;
                let mut up_probe = ProbeGuard::start("client.sync", "sync.upload_chunk", None);
                up_probe.add_bytes(stored_bytes.len() as u64);
                let hash_hex = hex::encode(stored_hash);
                let url = format!("{base}/api/vaults/{vid}/chunks/{hash_hex}");
                let mut req = http
                    .put(&url)
                    .header("Content-Type", "application/octet-stream")
                    .header("X-LanVault-Encoding", "xchacha20poly1305+zstd")
                    .header("X-LanVault-Raw-Size", raw_size.to_string())
                    .body(stored_bytes);
                if let Some(c) = cookie {
                    req = req.header("Cookie", format!("lv_session={c}"));
                }
                let resp = req.send().await?;
                match resp.status() {
                    StatusCode::CREATED => Ok((stored_hash, raw_size, false)),
                    StatusCode::OK | StatusCode::CONFLICT | StatusCode::NOT_MODIFIED => {
                        Ok((stored_hash, raw_size, true))
                    }
                    other => {
                        let body = resp.text().await.unwrap_or_default();
                        Err(ClientError::RemoteRejected {
                            status: other.as_u16(),
                            body,
                        })
                    }
                }
            });

            while let Some(joined) = inflight_uploads.try_join_next() {
                match joined {
                    Ok(Ok((h, raw_sz, server_dedup))) => {
                        {
                            let mut guard = self.known_remote_chunks.write().await;
                            if guard.len() >= MAX_KNOWN_REMOTE_CHUNKS {
                                guard.clear();
                            }
                            guard.insert((vault_id.to_string(), h));
                        }
                        if server_dedup {
                            summary.chunks_deduplicated += 1;
                        } else {
                            summary.chunks_uploaded += 1;
                        }
                        summary.bytes_processed += raw_sz as u64;
                        let elapsed_secs = started.elapsed().as_secs_f64().max(0.001);
                        let throughput_bps = (summary.bytes_processed as f64 / elapsed_secs) as u64;
                        let _ = self.progress_tx.send(SyncProgress {
                            rule_id: rule_id.to_string(),
                            current_file: snapshot.rel_path.clone(),
                            bytes_processed: summary.bytes_processed,
                            total_bytes,
                            chunks_uploaded: summary.chunks_uploaded,
                            chunks_deduplicated: summary.chunks_deduplicated,
                            throughput_bps,
                            status: "syncing".to_string(),
                        });
                    }
                    Ok(Err(err)) => {
                        loop_err = Some(err);
                        break;
                    }
                    Err(err) => {
                        loop_err = Some(ClientError::Io(std::io::Error::other(format!(
                            "upload task panicked: {err}"
                        ))));
                        break;
                    }
                }
            }
            if loop_err.is_some() {
                break;
            }
        }

        drop(rx);
        let _ = producer.await;

        while let Some(joined) = inflight_uploads.join_next().await {
            if loop_err.is_some() {
                continue;
            }
            match joined {
                Ok(Ok((h, raw_sz, server_dedup))) => {
                    {
                        let mut guard = self.known_remote_chunks.write().await;
                        if guard.len() >= MAX_KNOWN_REMOTE_CHUNKS {
                            guard.clear();
                        }
                        guard.insert((vault_id.to_string(), h));
                    }
                    if server_dedup {
                        summary.chunks_deduplicated += 1;
                    } else {
                        summary.chunks_uploaded += 1;
                    }
                    summary.bytes_processed += raw_sz as u64;
                }
                Ok(Err(err)) => loop_err = Some(err),
                Err(err) => {
                    loop_err = Some(ClientError::Io(std::io::Error::other(format!(
                        "upload task panicked: {err}"
                    ))));
                }
            }
        }

        if let Some(err) = loop_err {
            return Err(err);
        }

        ordered_hashes.sort_unstable_by_key(|(idx, _)| *idx);
        let hashes: Vec<[u8; 32]> = ordered_hashes.into_iter().map(|(_, h)| h).collect();

        debug!(
            file = %snapshot.rel_path,
            chunks = hashes.len(),
            "file streamed, packed, and uploaded"
        );
        Ok(hashes)
    }

    async fn commit_snapshot(
        &self,
        rule_id: &str,
        vault_id: &str,
        states: &mut [LocalFileStateRecord],
    ) -> Result<String, ClientError> {
        states.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));

        let mut tree_input = Vec::new();
        let mut files = Vec::with_capacity(states.len());

        for state in states {
            tree_input.extend_from_slice(state.rel_path.as_bytes());
            tree_input.push(0);
            tree_input.extend_from_slice(&state.size.to_le_bytes());
            tree_input.extend_from_slice(&state.mtime_ns.to_le_bytes());
            tree_input.extend_from_slice(&state.chunk_hashes);

            let chunk_hashes_hex: Vec<String> = state
                .chunk_hashes
                .as_chunks::<32>()
                .0
                .iter()
                .map(hex::encode)
                .collect();

            files.push(SnapshotFileManifestEntry {
                rel_path: state.rel_path.clone(),
                size: state.size,
                mtime_ns: state.mtime_ns,
                chunk_hashes_hex,
            });
        }

        let root_tree_hash = blake3_hash(&tree_input);
        let snapshot_id = Uuid::new_v4().to_string();
        let payload = CommitSnapshotRequest {
            snapshot_id: snapshot_id.clone(),
            rule_id: rule_id.to_string(),
            device_id: self.device_id.clone(),
            root_tree_hash_hex: hex::encode(root_tree_hash),
            retention_tier: "daily".to_string(),
            files,
        };

        let url = format!("{}/api/vaults/{}/snapshots", self.endpoint_base_url, vault_id);
        let mut req = self.http.post(&url).json(&payload);
        if let Some(cookie) = &self.session_cookie {
            req = req.header("Cookie", format!("lv_session={cookie}"));
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            if status == StatusCode::NOT_FOUND {
                self.known_remote_chunks
                    .write()
                    .await
                    .retain(|(vid, _)| vid != vault_id);
            }
            return Err(ClientError::RemoteRejected {
                status: status.as_u16(),
                body: format!("{url}: {body}"),
            });
        }
        Ok(snapshot_id)
    }
}

/// 单次同步汇总统计。
#[derive(Debug, Clone, Default)]
pub struct SyncSummary {
    pub files_synced: u64,
    pub bytes_processed: u64,
    pub chunks_uploaded: u64,
    pub chunks_deduplicated: u64,
    pub snapshot_id: Option<String>,
}
