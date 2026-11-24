//! 增量块上传管线：流式复用 `lanvault_core::{chunk_stream, pack_chunk, unpack_chunk}`，
//! 集成令牌桶限速 (`max_kbps`)、场景化省电门控 (`wifi_only`, `allowed_ssids`, `require_charging`)
//! 与全量分块上传后的快照提交闭环 (`POST /api/vaults/{vault_id}/snapshots`)。

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::BufReader;
use std::sync::atomic::Ordering;
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

use crate::error::ClientError;
use crate::scanner::{checkpoint_key, DiffResult, FileMetadataSnapshot};

/// Result of one chunk PUT: `(stored_hash, raw_size, server_side_dedup)`.
type ChunkUploadOutcome = Result<([u8; 32], usize, bool), ClientError>;

/// Bounded cap for client-side remote chunk dedup cache (~3.2 MiB max RAM).
const MAX_KNOWN_REMOTE_CHUNKS: usize = 100_000;

/// 一轮同步中所有文件共享的在途块请求上限。
const CHUNK_UPLOAD_CONCURRENCY: usize = 32;

/// 同时处理（分块+上传）的文件数。小文件目录（照片、文档）里单文件并发不足，
/// 必须让多个文件并行，否则每文件一次快照提交会把吞吐压到几百 KB/s。
const FILE_SYNC_CONCURRENCY: usize = 4;

/// 每个打包线程的原始块队列深度。
const CHUNK_QUEUE_DEPTH: usize = 2;

async fn send_retriable(
    req: reqwest::RequestBuilder,
    upload: Option<(&TokenBucket, usize)>,
) -> Result<reqwest::Response, reqwest::Error> {
    for delay in [250, 1000] {
        let Some(attempt) = req.try_clone() else {
            break;
        };
        if let Some((rate_limiter, bytes)) = upload {
            rate_limiter.acquire(bytes).await;
        }
        match attempt.send().await {
            Ok(response)
                if matches!(response.status().as_u16(), 408 | 500 | 502 | 503 | 504) => {}
            Err(err) if err.is_connect() || err.is_timeout() || err.is_request() => {}
            other => return other,
        }
        time::sleep(Duration::from_millis(delay)).await;
    }
    if let Some((rate_limiter, bytes)) = upload {
        rate_limiter.acquire(bytes).await;
    }
    req.send().await
}

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
            state.last_refill = state.last_refill.max(now);

            let needed = bytes as f64;
            if state.tokens >= needed {
                state.tokens -= needed;
                return;
            }
            let deficit = needed - state.tokens;
            state.tokens = 0.0;
            let wait = Duration::from_secs_f64(deficit / state.rate_bytes_per_sec);
            state.last_refill += wait;
            state.last_refill.saturating_duration_since(now)
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

/// 单个文件的同步结果，供文件级并发聚合。
struct FileSyncOutcome {
    snapshot: FileMetadataSnapshot,
    chunk_hashes: Vec<[u8; 32]>,
    chunks_uploaded: u64,
    chunks_deduplicated: u64,
    bytes_processed: u64,
}

/// 投递给常驻打包线程池的一个块任务。
struct PackJob {
    idx: usize,
    raw_bytes: Vec<u8>,
    seal_ctx: Arc<ChunkSealContext>,
}

/// 打包线程回传的结果。`idx == usize::MAX` 表示生产者上报的致命错误。
struct PackResult {
    idx: usize,
    packed: Result<PackedChunk, ClientError>,
}

/// 打包线程数：以 CPU 核数为上限，且不低于 2、不高于块级并发。
fn pack_pool_size() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().clamp(2, 8))
        .unwrap_or(4)
}

/// 全局（跨文件）实时进度计数器，供并发文件共享同一份进度视图。
struct SharedProgress {
    uploaded: std::sync::atomic::AtomicU64,
    deduplicated: std::sync::atomic::AtomicU64,
    bytes: std::sync::atomic::AtomicU64,
    files_done: std::sync::atomic::AtomicU64,
    total_bytes: u64,
    started: Instant,
    /// 滑动窗口采样：`(采样时刻, 采样时的累计字节)`，用于给出"当前速度"而不是全程平均。
    window: std::sync::Mutex<(Instant, u64)>,
}

impl SharedProgress {
    fn new(total_bytes: u64) -> Self {
        let now = Instant::now();
        Self {
            uploaded: std::sync::atomic::AtomicU64::new(0),
            deduplicated: std::sync::atomic::AtomicU64::new(0),
            bytes: std::sync::atomic::AtomicU64::new(0),
            files_done: std::sync::atomic::AtomicU64::new(0),
            total_bytes,
            started: now,
            window: std::sync::Mutex::new((now, 0)),
        }
    }

    /// 当前瞬时吞吐（字节/秒）。窗口不足 250ms 时返回上一窗口的换算值，
    /// 避免每秒一次的事件流把速度抖动成 0。
    fn instantaneous_bps(&self) -> u64 {
        let total = self.bytes.load(Ordering::Relaxed);
        let Ok(mut window) = self.window.lock() else {
            return 0;
        };
        let elapsed = window.0.elapsed();
        if elapsed < Duration::from_millis(250) {
            let whole = self.started.elapsed().as_secs_f64().max(0.001);
            return (total as f64 / whole) as u64;
        }
        let delta = total.saturating_sub(window.1);
        let bps = (delta as f64 / elapsed.as_secs_f64()) as u64;
        *window = (Instant::now(), total);
        bps
    }
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
    upload_slots: Arc<Semaphore>,
    /// 每文件打包线程池大小（压缩+加密的并行度）。
    pack_pool: usize,
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
        // 单条长连接的吞吐直接受"连接池太小导致反复重建 TCP"与 Nagle 延迟影响：
        // 显式放大 keep-alive 池并关闭 nodelay 之外的额外协商开销。
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(60))
            .pool_max_idle_per_host(CHUNK_UPLOAD_CONCURRENCY)
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_nodelay(true)
            .http1_only()
            .redirect(reqwest::redirect::Policy::none())
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
            upload_slots: Arc::new(Semaphore::new(CHUNK_UPLOAD_CONCURRENCY)),
            pack_pool: pack_pool_size(),
        })
    }

    pub fn subscribe_progress(&self) -> broadcast::Receiver<SyncProgress> {
        self.progress_tx.subscribe()
    }

    pub fn has_pending_checkpoint(
        db: &Database,
        rule_id: &str,
        vault_id: &str,
    ) -> Result<bool, ClientError> {
        Ok(db
            .get_setting(&checkpoint_key(vault_id, rule_id))?
            .is_some_and(|value| value.starts_with("inflight:") || value.starts_with("rebuild:")))
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
    ///
    /// 文件级并发（`FILE_SYNC_CONCURRENCY`）x 块级并发（`CHUNK_UPLOAD_CONCURRENCY`）：
    /// 单文件只能吃满一个块的往返延迟，多文件并行才能把小文件目录的吞吐拉到链路带宽。
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

        let changed: Vec<&FileMetadataSnapshot> = diff.changed_files().collect();
        let total_bytes: u64 = changed.iter().map(|f| f.size).sum();
        let mut summary = SyncSummary::default();
        let file_count = changed.len();

        let checkpoint_key = checkpoint_key(vault_id, rule_id);
        let checkpoint = db.get_setting(&checkpoint_key)?.unwrap_or_default();
        let pending = checkpoint
            .strip_prefix("inflight:")
            .or_else(|| checkpoint.strip_prefix("rebuild:"));
        if diff.is_empty() && pending.is_none() {
            return Ok(summary);
        }

        let previous_snapshot = pending.unwrap_or(&checkpoint);
        if pending.is_none() {
            db.set_setting(&checkpoint_key, &format!("inflight:{previous_snapshot}"))?;
        }

        let chunk_key = self.vault_master_key.derive_vault_chunk_key(vault_id);
        let progress = Arc::new(SharedProgress::new(total_bytes));
        let started = Instant::now();
        let mut pending_db_states: Vec<LocalFileStateRecord> = Vec::with_capacity(file_count);

        // 用流式 FuturesUnordered 而不是无界 `join_all`：并发窗口恒定，大目录也不会
        // 一次性把成千上万个 Future / 块队列堆进内存。
        let mut inflight = futures_util::stream::FuturesUnordered::new();
        let mut next_index = 0usize;
        let mut file_err: Option<ClientError> = None;

        loop {
            while inflight.len() < FILE_SYNC_CONCURRENCY && next_index < changed.len() {
                let snapshot = changed[next_index].clone();
                next_index += 1;

                let current_policy = self.policy.read().await.clone();
                let current_env = self.env.read().await.clone();
                if let Err(reason) = current_policy.evaluate(&current_env) {
                    file_err = Some(ClientError::PolicyBlocked(reason));
                    break;
                }

                inflight.push(self.sync_single_file(
                    rule_id,
                    vault_id,
                    &chunk_key,
                    snapshot,
                    Arc::clone(&progress),
                    pending.is_some(),
                ));
            }
            if file_err.is_some() {
                break;
            }

            let Some(joined) = futures_util::StreamExt::next(&mut inflight).await else {
                break;
            };
            match joined {
                Ok(outcome) => {
                    let mut blob = Vec::with_capacity(outcome.chunk_hashes.len() * 32);
                    for h in &outcome.chunk_hashes {
                        blob.extend_from_slice(h);
                    }
                    pending_db_states.push(LocalFileStateRecord {
                        rule_id: rule_id.to_string(),
                        target_vault_id: vault_id.to_string(),
                        rel_path: outcome.snapshot.rel_path.clone(),
                        size: outcome.snapshot.size,
                        mtime_ns: outcome.snapshot.mtime_ns,
                        file_id: outcome.snapshot.file_id,
                        chunk_hashes: blob,
                    });
                    summary.files_synced += 1;
                    summary.chunks_uploaded += outcome.chunks_uploaded;
                    summary.chunks_deduplicated += outcome.chunks_deduplicated;
                    summary.bytes_processed += outcome.bytes_processed;
                    progress
                        .files_done
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                Err(err) => {
                    file_err = Some(err);
                    break;
                }
            }
        }

        if let Some(err) = file_err {
            // Leave the previous version intact. The next pass will probe uploaded chunks.
            return Err(err);
        }

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
        let mut projected_states: Vec<_> = projected_map.into_values().collect();
        let committed = self
            .commit_snapshot(rule_id, vault_id, previous_snapshot, &mut projected_states)
            .await;
        if matches!(&committed, Err(ClientError::RemoteRejected { status: 404, .. })) {
            db.set_setting(&checkpoint_key, &format!("rebuild:{previous_snapshot}"))?;
        }
        let snapshot_id = committed?;
        db.commit_local_snapshot(
            rule_id,
            &pending_db_states,
            &diff.deleted,
            &checkpoint_key,
            &snapshot_id,
        )?;
        summary.snapshot_id = Some(snapshot_id);

        let elapsed = started.elapsed().as_secs_f64().max(0.001);
        info!(
            rule_id = %rule_id,
            vault_id = %vault_id,
            files = summary.files_synced,
            uploaded_chunks = summary.chunks_uploaded,
            dedup_chunks = summary.chunks_deduplicated,
            bytes = summary.bytes_processed,
            throughput_mbps = (summary.bytes_processed as f64 / elapsed / (1024.0 * 1024.0)),
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
        snapshot: FileMetadataSnapshot,
        progress: Arc<SharedProgress>,
        resume: bool,
    ) -> Result<FileSyncOutcome, ClientError> {
        let mut file_probe = ProbeGuard::start(
            "client.sync",
            "sync.parallel_pack_upload",
            Some(format!("file={}", snapshot.rel_path)),
        );
        let seal_ctx = Arc::new(ChunkSealContext::new(chunk_key, vault_id.as_bytes()));

        let abs_path = snapshot.abs_path.clone();
        let expected_size = snapshot.size;
        let expected_mtime_ns = snapshot.mtime_ns;

        // Each worker packs multiple chunks. Sequence numbers preserve file order.
        let pack_pool = self.pack_pool;
        let (result_tx, mut result_rx) = mpsc::channel::<PackResult>(pack_pool);
        let producer_error_tx = result_tx.clone();

        // 每个打包线程独占一个 sync_channel：`std::sync::mpsc::Receiver` 不是 Sync，
        // 若多个线程共享一个 Receiver，`recv()` 必须在锁内阻塞，等于把并行度压回 1。
        // 生产者按轮询把块投给各线程，天然无锁且保持有界背压。
        let mut job_senders: Vec<std::sync::mpsc::SyncSender<PackJob>> =
            Vec::with_capacity(pack_pool);
        let mut pack_threads = Vec::with_capacity(pack_pool);
        for idx in 0..pack_pool {
            let (job_tx, job_rx) = std::sync::mpsc::sync_channel::<PackJob>(CHUNK_QUEUE_DEPTH);
            job_senders.push(job_tx);
            let result_tx = result_tx.clone();
            pack_threads.push(
                std::thread::Builder::new()
                    .name(format!("lanvault-pack-{idx}"))
                    .spawn(move || {
                        while let Ok(job) = job_rx.recv() {
                            let packed = pack_chunk_with_context(
                                &job.raw_bytes,
                                Some(&job.seal_ctx),
                                DEFAULT_ZSTD_LEVEL,
                            )
                            .map_err(ClientError::Core);
                            // 消费者已退出时静默结束本线程。
                            if result_tx
                                .blocking_send(PackResult { idx: job.idx, packed })
                                .is_err()
                            {
                                break;
                            }
                        }
                    })
                    ?,
            );
        }
        drop(result_tx);

        let seal_for_producer = Arc::clone(&seal_ctx);
        // 生产者侧的硬错误（文件打不开、读一半被改）通过与块结果同一个通道上报，
        // 用 `idx == usize::MAX` 作哨兵，正常块的 idx 单调递增不会撞上。
        let producer = tokio::task::spawn_blocking(move || {
            let file = match File::open(&abs_path) {
                Ok(f) => f,
                Err(err) => {
                    let _ = producer_error_tx.blocking_send(PackResult {
                        idx: usize::MAX,
                        packed: Err(ClientError::Io(err)),
                    });
                    return;
                }
            };
            let reader = BufReader::with_capacity(1024 * 1024, file);
            let mut seq_idx: usize = 0;
            let mut next_worker = 0usize;
            let res = chunk_stream(reader, ChunkParams::default(), |_desc, raw_bytes| {
                let idx = seq_idx;
                seq_idx += 1;
                let sender = &job_senders[next_worker % job_senders.len()];
                next_worker += 1;
                let job = PackJob {
                    idx,
                    raw_bytes,
                    seal_ctx: Arc::clone(&seal_for_producer),
                };
                if sender.send(job).is_err() {
                    return Err(lanvault_core::LanVaultError::Io(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "chunk pack workers stopped",
                    )));
                }
                Ok(())
            });
            if let Err(err) = res {
                let _ = producer_error_tx.blocking_send(PackResult {
                    idx: usize::MAX,
                    packed: Err(ClientError::Core(err)),
                });
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
                    let _ = producer_error_tx.blocking_send(PackResult {
                        idx: usize::MAX,
                        packed: Err(ClientError::InvalidInput(format!(
                            "file changed while it was being backed up: {}",
                            abs_path.display()
                        ))),
                    });
                }
                Err(err) => {
                    let _ = producer_error_tx.blocking_send(PackResult {
                        idx: usize::MAX,
                        packed: Err(ClientError::Io(err)),
                    });
                }
            }
        });

        let mut ordered_hashes: Vec<(usize, [u8; 32])> = Vec::new();
        let mut inflight_uploads: tokio::task::JoinSet<ChunkUploadOutcome> =
            tokio::task::JoinSet::new();
        let mut loop_err: Option<ClientError> = None;
        let mut chunks_uploaded: u64 = 0;
        let mut chunks_deduplicated: u64 = 0;
        let mut file_bytes: u64 = 0;

        while let Some(item) = result_rx.recv().await {
            let packed = match item.packed {
                Ok(packed) => packed,
                Err(err) => {
                    loop_err = Some(err);
                    break;
                }
            };
            // 生产者报错时使用 `usize::MAX` 作为哨兵，正常块的 idx 单调递增不会撞上。
            let idx = item.idx;

            ordered_hashes.push((idx, packed.stored_hash));
            file_probe.add_bytes(packed.raw_size as u64);

            let cache_key = (vault_id.to_string(), packed.stored_hash);
            let already_exists = self
                .known_remote_chunks
                .read()
                .await
                .contains(&cache_key);

            if already_exists {
                chunks_deduplicated += 1;
                file_bytes += packed.raw_size as u64;
                progress.deduplicated.fetch_add(1, Ordering::Relaxed);
                progress
                    .bytes
                    .fetch_add(packed.raw_size as u64, Ordering::Relaxed);
                self.emit_shared_progress(rule_id, &snapshot.rel_path, &progress);
                continue;
            }

            let Ok(permit) = self.upload_slots.clone().acquire_owned().await else {
                loop_err = Some(ClientError::Io(std::io::Error::other(
                    "upload semaphore closed",
                )));
                break;
            };
            let http = self.http.clone();
            let base = self.endpoint_base_url.clone();
            let cookie = self.session_cookie.clone();
            let vid = vault_id.to_string();
            let raw_size = packed.raw_size;
            let stored_hash = packed.stored_hash;
            let stored_bytes = packed.stored_bytes;
            let rate_limiter = Arc::clone(&self.rate_limiter);

            inflight_uploads.spawn(async move {
                let _permit = permit;
                let mut up_probe = ProbeGuard::start("client.sync", "sync.upload_chunk", None);
                let hash_hex = hex::encode(stored_hash);
                let url = format!("{base}/api/vaults/{vid}/chunks/{hash_hex}");
                if resume {
                    let mut head = http.head(&url);
                    if let Some(c) = &cookie {
                        head = head.header("Cookie", format!("lv_session={c}"));
                    }
                    let response = send_retriable(head, None).await?;
                    match response.status() {
                        StatusCode::OK => return Ok((stored_hash, raw_size, true)),
                        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED => {}
                        status => {
                            return Err(ClientError::RemoteRejected {
                                status: status.as_u16(),
                                body: "chunk availability check failed".to_string(),
                            });
                        }
                    }
                }

                let stored_size = stored_bytes.len();
                up_probe.add_bytes(stored_size as u64);
                let mut req = http
                    .put(&url)
                    .header("Content-Type", "application/octet-stream")
                    .header("X-LanVault-Encoding", "xchacha20poly1305+zstd")
                    .header("X-LanVault-Raw-Size", raw_size.to_string())
                    .body(stored_bytes);
                if let Some(c) = cookie {
                    req = req.header("Cookie", format!("lv_session={c}"));
                }
                let resp = send_retriable(req, Some((&rate_limiter, stored_size))).await?;
                match resp.status() {
                    StatusCode::CREATED => Ok((stored_hash, raw_size, false)),
                    StatusCode::OK => {
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
                        self.record_remote_chunk(vault_id, h).await;
                        file_bytes += raw_sz as u64;
                        if server_dedup {
                            chunks_deduplicated += 1;
                            progress.deduplicated.fetch_add(1, Ordering::Relaxed);
                        } else {
                            chunks_uploaded += 1;
                            progress.uploaded.fetch_add(1, Ordering::Relaxed);
                        }
                        progress.bytes.fetch_add(raw_sz as u64, Ordering::Relaxed);
                        self.emit_shared_progress(rule_id, &snapshot.rel_path, &progress);
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

        // 生产者线程持有 `job_senders` 的所有权；它结束后打包线程自然会看到 channel 关闭。
        drop(result_rx);
        if let Err(err) = producer.await {
            loop_err.get_or_insert_with(|| ClientError::Io(std::io::Error::other(err)));
        }
        for handle in pack_threads {
            if handle.join().is_err() {
                loop_err.get_or_insert_with(|| {
                    ClientError::Io(std::io::Error::other("chunk pack worker panicked"))
                });
            }
        }

        if loop_err.is_some() {
            inflight_uploads.abort_all();
        }

        while let Some(joined) = inflight_uploads.join_next().await {
            if loop_err.is_some() {
                continue;
            }
            match joined {
                Ok(Ok((h, raw_sz, server_dedup))) => {
                    self.record_remote_chunk(vault_id, h).await;
                    file_bytes += raw_sz as u64;
                    if server_dedup {
                        chunks_deduplicated += 1;
                        progress.deduplicated.fetch_add(1, Ordering::Relaxed);
                    } else {
                        chunks_uploaded += 1;
                        progress.uploaded.fetch_add(1, Ordering::Relaxed);
                    }
                    progress.bytes.fetch_add(raw_sz as u64, Ordering::Relaxed);
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
        if file_bytes != expected_size {
            return Err(ClientError::InvalidInput(format!(
                "file size changed while backing up {}",
                snapshot.rel_path
            )));
        }

        ordered_hashes.sort_unstable_by_key(|(idx, _)| *idx);
        let hashes: Vec<[u8; 32]> = ordered_hashes.into_iter().map(|(_, h)| h).collect();

        debug!(
            file = %snapshot.rel_path,
            chunks = hashes.len(),
            "file streamed, packed, and uploaded"
        );
        Ok(FileSyncOutcome {
            snapshot,
            chunk_hashes: hashes,
            chunks_uploaded,
            chunks_deduplicated,
            bytes_processed: file_bytes,
        })
    }

    /// 把已确认落盘的远端块写入去重缓存（带 25% 渐进淘汰，避免整表清空）。
    async fn record_remote_chunk(&self, vault_id: &str, hash: [u8; 32]) {
        let mut guard = self.known_remote_chunks.write().await;
        if guard.len() >= MAX_KNOWN_REMOTE_CHUNKS {
            let to_remove = guard.len() / 4;
            let evict_keys: Vec<(String, [u8; 32])> = guard.iter().take(to_remove).cloned().collect();
            for key in evict_keys {
                guard.remove(&key);
            }
        }
        guard.insert((vault_id.to_string(), hash));
    }

    /// 用跨文件共享计数器推送一条实时进度事件。
    fn emit_shared_progress(
        &self,
        rule_id: &str,
        current_file: &str,
        progress: &SharedProgress,
    ) {
        let bytes_processed = progress.bytes.load(Ordering::Relaxed);
        let _ = self.progress_tx.send(SyncProgress {
            rule_id: rule_id.to_string(),
            current_file: current_file.to_string(),
            bytes_processed,
            total_bytes: progress.total_bytes,
            chunks_uploaded: progress.uploaded.load(Ordering::Relaxed),
            chunks_deduplicated: progress.deduplicated.load(Ordering::Relaxed),
            throughput_bps: progress.instantaneous_bps(),
            status: "syncing".to_string(),
        });
    }

    async fn commit_snapshot(
        &self,
        rule_id: &str,
        vault_id: &str,
        previous_snapshot: &str,
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
        let mut commit_identity = b"EveryoneNAS snapshot commit v1".to_vec();
        for field in [vault_id, rule_id, &self.device_id, previous_snapshot] {
            commit_identity.extend_from_slice(&(field.len() as u64).to_le_bytes());
            commit_identity.extend_from_slice(field.as_bytes());
        }
        commit_identity.extend_from_slice(&root_tree_hash);
        let snapshot_id = hex::encode(blake3_hash(&commit_identity));
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

        let resp = send_retriable(req, None).await?;
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
