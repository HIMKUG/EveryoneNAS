//! 增量块上传管线：流式复用 `lanvault_core::{chunk_stream, pack_chunk, unpack_chunk}`，
//! 集成令牌桶限速 (`max_kbps`)、场景化省电门控 (`wifi_only`, `allowed_ssids`, `require_charging`)
//! 与全量分块上传后的快照提交闭环 (`POST /api/vaults/{vault_id}/snapshots`)。

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::BufReader;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use lanvault_core::{
    blake3_hash, chunk_stream, is_incompressible_extension, pack_chunk_adaptive_with_context,
    unpack_chunk, ChunkCompression, ChunkParams,
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

/// 单个文件内同时在途的块上传数量。
///
/// 千兆/2.5G 链路的瓶颈是"每块一次请求"的往返时延而不是带宽：1 MiB 块 + 序列化 +
/// 服务端写盘，若只并发 8 个，常见家用交换机的 RTT 抖动就会把吞吐压到 1~2 MB/s。
/// 32 路在途可以把任何 2.5GbE 链路喂满，同时把内存占用控制在 ~32 MiB（块均 1 MiB）。
#[cfg(target_os = "android")]
const CHUNK_UPLOAD_CONCURRENCY: usize = 8;
#[cfg(not(target_os = "android"))]
const CHUNK_UPLOAD_CONCURRENCY: usize = 16;

#[cfg(target_os = "android")]
const FILE_SYNC_CONCURRENCY: usize = 2;
#[cfg(not(target_os = "android"))]
const FILE_SYNC_CONCURRENCY: usize = 4;

/// 单文件打包队列深度严格收紧，避免多线程叠加产生 GB 级内存峰值（OOM 防护）。
const CHUNK_QUEUE_DEPTH: usize = 4;

/// 全局内存字节预算（以 64 KiB 为单位：Android 上限 48 MiB，桌面端上限 128 MiB），严格封顶在途内存。
const MEM_UNIT_BYTES: usize = 64 * 1024;
#[cfg(target_os = "android")]
const GLOBAL_MEM_BUDGET_UNITS: usize = 768;
#[cfg(not(target_os = "android"))]
const GLOBAL_MEM_BUDGET_UNITS: usize = 2048;

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
    packed: Result<(PackedChunk, ChunkCompression), ClientError>,
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
    session_cookie: Arc<RwLock<Option<String>>>,
    device_id: String,
    vault_master_key: SymmetricKey,
    policy: Arc<RwLock<PowerNetworkPolicy>>,
    env: Arc<RwLock<DeviceEnvironment>>,
    rate_limiter: Arc<TokenBucket>,
    progress_tx: broadcast::Sender<SyncProgress>,
    known_remote_chunks: Arc<RwLock<HashSet<(String, [u8; 32])>>>,
    /// 每文件打包线程池大小（压缩+加密的并行度）。
    pack_pool: usize,
    /// 快速模式开关：启用后使用固定 4 MiB 顺序大块 + Raw 免压缩，最大化局域网吞吐。
    fast_mode: AtomicBool,
    /// 全局在途字节预算信号量（按 64 KiB 单位计费），彻底防止多文件并发 OOM。
    mem_budget: Arc<Semaphore>,
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
        pin: crate::transport::PeerFingerprint,
    ) -> Result<Self, ClientError> {
        // 单条长连接的吞吐直接受"连接池太小导致反复重建 TCP"与 Nagle 延迟影响：
        // 显式放大 keep-alive 池并关闭 nodelay 之外的额外协商开销。
        let http = crate::transport::peer_http_builder(pin)?
            .read_timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(CHUNK_UPLOAD_CONCURRENCY * FILE_SYNC_CONCURRENCY)
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_nodelay(true)
            .http1_only()
            .build()?;
        let rate_limiter = shared_rate_limiter
            .unwrap_or_else(|| Arc::new(TokenBucket::new(initial_policy.max_kbps)));
        let (progress_tx, _) = broadcast::channel(256);

        Ok(Self {
            http,
            endpoint_base_url: endpoint_base_url.trim_end_matches('/').to_string(),
            session_cookie: Arc::new(RwLock::new(session_cookie)),
            device_id,
            vault_master_key,
            policy: Arc::new(RwLock::new(initial_policy)),
            env: Arc::new(RwLock::new(environment)),
            rate_limiter,
            progress_tx,
            known_remote_chunks: Arc::new(RwLock::new(HashSet::new())),
            pack_pool: pack_pool_size(),
            fast_mode: AtomicBool::new(false),
            mem_budget: Arc::new(Semaphore::new(GLOBAL_MEM_BUDGET_UNITS)),
        })
    }

    pub fn set_fast_mode(&self, enabled: bool) {
        self.fast_mode.store(enabled, Ordering::Relaxed);
    }

    pub async fn update_session_cookie(&self, cookie: Option<String>) {
        *self.session_cookie.write().await = cookie;
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

        if file_count == 0 {
            if !diff.deleted.is_empty() {
                self.commit_deletions(db, rule_id, vault_id, diff).await?;
            }
            return Ok(summary);
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
            // 仅提交已成功文件的阶段快照，严禁携带未确认的 diff.deleted，避免误删远端文件。
            if !pending_db_states.is_empty() {
                let partial_diff = DiffResult {
                    added: diff.added.clone(),
                    modified: diff.modified.clone(),
                    deleted: Vec::new(),
                    unchanged_count: diff.unchanged_count,
                };
                if let Err(commit_err) = self
                    .finalize_pass(db, rule_id, vault_id, &mut summary, pending_db_states, &partial_diff)
                    .await
                {
                    tracing::warn!(error = %commit_err, "partial snapshot commit failed after file error");
                }
            }
            return Err(err);
        }

        self.finalize_pass(db, rule_id, vault_id, &mut summary, pending_db_states, diff)
            .await?;

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

    /// 提交本轮的快照清单并落盘本地 `local_file_state`（删除项一并处理）。
    async fn finalize_pass(
        &self,
        db: &Database,
        rule_id: &str,
        vault_id: &str,
        summary: &mut SyncSummary,
        pending_db_states: Vec<LocalFileStateRecord>,
        diff: &DiffResult,
    ) -> Result<(), ClientError> {
        if diff.is_empty() && pending_db_states.is_empty() {
            return Ok(());
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
        let mut projected_states: Vec<LocalFileStateRecord> = projected_map.into_values().collect();
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
        Ok(())
    }

    /// 仅处理删除（没有新增/修改文件时仍要让远端快照反映删除）。
    async fn commit_deletions(
        &self,
        db: &Database,
        rule_id: &str,
        vault_id: &str,
        diff: &DiffResult,
    ) -> Result<(), ClientError> {
        let mut noop_summary = SyncSummary::default();
        self.finalize_pass(
            db,
            rule_id,
            vault_id,
            &mut noop_summary,
            Vec::new(),
            diff,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn sync_single_file(
        &self,
        rule_id: &str,
        vault_id: &str,
        chunk_key: &SymmetricKey,
        snapshot: FileMetadataSnapshot,
        progress: Arc<SharedProgress>,
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

        // 每个块都 `spawn_blocking` 一次意味着每 MiB 两次线程池调度（提交 + 唤醒）加一次
        // JoinHandle 分配。这里改为「常驻打包线程池 + 单个有序回执通道」：
        // - 线程在整个进程生命周期外只创建一次（按文件惰性复用池）；
        // - 每块只是一次有界 channel 投递，不再分配 tokio 任务；
        // - 消费者按 `idx` 重排，块序与文件内容严格一致。
        let pack_pool = self.pack_pool;
        let (result_tx, mut result_rx) = mpsc::channel::<PackResult>(CHUNK_QUEUE_DEPTH * 2);
        // 生产者线程也需要向同一个结果通道上报致命错误，因此多留一个句柄。
        let result_error_tx = result_tx.clone();

        // 每个打包线程独占一个 sync_channel：`std::sync::mpsc::Receiver` 不是 Sync，
        // 若多个线程共享一个 Receiver，`recv()` 必须在锁内阻塞，等于把并行度压回 1。
        // 生产者按轮询把块投给各线程，天然无锁且保持有界背压。
        let mut job_senders: Vec<std::sync::mpsc::SyncSender<PackJob>> =
            Vec::with_capacity(pack_pool);
        let mut pack_threads = Vec::with_capacity(pack_pool);
        let is_fast_mode = self.fast_mode.load(Ordering::Relaxed);
        let force_raw = is_fast_mode || is_incompressible_extension(&abs_path);
        for idx in 0..pack_pool {
            let (job_tx, job_rx) = std::sync::mpsc::sync_channel::<PackJob>(CHUNK_QUEUE_DEPTH);
            job_senders.push(job_tx);
            let result_tx = result_tx.clone();
            pack_threads.push(
                std::thread::Builder::new()
                    .name(format!("lanvault-pack-{idx}"))
                    .spawn(move || {
                        while let Ok(job) = job_rx.recv() {
                            let packed = pack_chunk_adaptive_with_context(
                                &job.raw_bytes,
                                Some(&job.seal_ctx),
                                DEFAULT_ZSTD_LEVEL,
                                force_raw,
                            )
                            .map_err(ClientError::Core);
                            if result_tx
                                .blocking_send(PackResult { idx: job.idx, packed })
                                .is_err()
                            {
                                break;
                            }
                        }
                    })
                    .expect("spawn chunk pack worker"),
            );
        }
        drop(result_tx);

        let seal_for_producer = Arc::clone(&seal_ctx);
        // 生产者侧的硬错误（文件打不开、读一半被改）通过与块结果同一个通道上报，
        // 用 `idx == usize::MAX` 作哨兵，正常块的 idx 单调递增不会撞上。
        let producer_error_tx = result_error_tx.clone();
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
            if is_fast_mode {
                use std::io::Read;
                let mut reader = BufReader::with_capacity(1024 * 1024, file);
                let mut buf = vec![0u8; CHUNK_MAX_SIZE as usize];
                let mut seq_idx: usize = 0;
                loop {
                    let mut filled = 0usize;
                    while filled < buf.len() {
                        match reader.read(&mut buf[filled..]) {
                            Ok(0) => break,
                            Ok(n) => filled += n,
                            Err(err) => {
                                let _ = producer_error_tx.blocking_send(PackResult {
                                    idx: usize::MAX,
                                    packed: Err(ClientError::Io(err)),
                                });
                                return;
                            }
                        }
                    }
                    if filled == 0 {
                        break;
                    }
                    let sender = &job_senders[seq_idx % job_senders.len()];
                    if sender
                        .send(PackJob {
                            idx: seq_idx,
                            raw_bytes: buf[..filled].to_vec(),
                            seal_ctx: Arc::clone(&seal_for_producer),
                        })
                        .is_err()
                    {
                        break;
                    }
                    seq_idx += 1;
                }
            } else {
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
        let upload_sem = Arc::new(Semaphore::new(CHUNK_UPLOAD_CONCURRENCY));
        let mut inflight_uploads: tokio::task::JoinSet<ChunkUploadOutcome> =
            tokio::task::JoinSet::new();
        let mut loop_err: Option<ClientError> = None;
        let mut chunks_uploaded: u64 = 0;
        let mut chunks_deduplicated: u64 = 0;
        let mut file_bytes: u64 = 0;

        let mut pending_check: Vec<(PackedChunk, ChunkCompression)> = Vec::with_capacity(16);
        loop {
            let maybe_item = result_rx.recv().await;
            if let Some(item) = maybe_item {
                let (packed, compression) = match item.packed {
                    Ok(res) => res,
                    Err(err) => {
                        loop_err = Some(err);
                        break;
                    }
                };
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
                pending_check.push((packed, compression));
                let pending_bytes: usize = pending_check.iter().map(|(p, _)| p.stored_bytes.len()).sum();
                if pending_check.len() < 16 && pending_bytes < 8 * 1024 * 1024 {
                    continue;
                }
            } else if pending_check.is_empty() {
                break;
            }

            let batch = std::mem::take(&mut pending_check);
            let missing_set = self
                .filter_missing_remote_chunks(
                    vault_id,
                    &batch.iter().map(|(p, _)| p.stored_hash).collect::<Vec<_>>(),
                )
                .await;

            for (packed, compression) in batch {
                if let Some(ref missing) = missing_set {
                    if !missing.contains(&packed.stored_hash) {
                        self.record_remote_chunk(vault_id, packed.stored_hash).await;
                        chunks_deduplicated += 1;
                        file_bytes += packed.raw_size as u64;
                        progress.deduplicated.fetch_add(1, Ordering::Relaxed);
                        progress
                            .bytes
                            .fetch_add(packed.raw_size as u64, Ordering::Relaxed);
                        self.emit_shared_progress(rule_id, &snapshot.rel_path, &progress);
                        continue;
                    }
                }

                let Ok(permit) = upload_sem.clone().acquire_owned().await else {
                    loop_err = Some(ClientError::Io(std::io::Error::other(
                        "upload semaphore closed",
                    )));
                    break;
                };
                let mem_units = packed.stored_bytes.len().div_ceil(MEM_UNIT_BYTES).clamp(1, GLOBAL_MEM_BUDGET_UNITS) as u32;
                let Ok(mem_permit) = self.mem_budget.clone().acquire_many_owned(mem_units).await else {
                    loop_err = Some(ClientError::Io(std::io::Error::other(
                        "memory budget semaphore closed",
                    )));
                    break;
                };
                self.rate_limiter.acquire(packed.stored_bytes.len()).await;

                let http = self.http.clone();
                let base = self.endpoint_base_url.clone();
                let cookie = self.session_cookie.read().await.clone();
                let vid = vault_id.to_string();
                let raw_size = packed.raw_size;
                let stored_hash = packed.stored_hash;
                let stored_bytes = packed.stored_bytes;
                let wire_encoding = compression.wire_encoding();

                inflight_uploads.spawn(async move {
                    let _permit = permit;
                    let _mem_permit = mem_permit;
                    let mut up_probe = ProbeGuard::start("client.sync", "sync.upload_chunk", None);
                    up_probe.add_bytes(stored_bytes.len() as u64);
                    let hash_hex = hex::encode(stored_hash);
                    let url = format!("{base}/api/vaults/{vid}/chunks/{hash_hex}");
                    let max_attempts: u32 = 4;
                    let mut attempt: u32 = 0;
                    loop {
                        attempt += 1;
                        let mut req = http
                            .put(&url)
                            .header("Content-Type", "application/octet-stream")
                            .header("X-LanVault-Encoding", wire_encoding)
                            .header("X-LanVault-Raw-Size", raw_size.to_string())
                            .body(stored_bytes.clone());
                    if let Some(ref c) = cookie {
                        req = req.header("Cookie", format!("lv_session={c}"));
                    }
                    match req.send().await {
                        Ok(resp) => match resp.status() {
                            StatusCode::CREATED => break Ok((stored_hash, raw_size, false)),
                            StatusCode::OK | StatusCode::CONFLICT | StatusCode::NOT_MODIFIED => {
                                break Ok((stored_hash, raw_size, true))
                            }
                            other if matches!(other.as_u16(), 408 | 429 | 500 | 502 | 503 | 504)
                                && attempt < max_attempts =>
                            {
                                let delay_ms = 400u64 << (attempt - 1);
                                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                            }
                            other => {
                                let body = resp.text().await.unwrap_or_default();
                                break Err(ClientError::RemoteRejected {
                                    status: other.as_u16(),
                                    body,
                                });
                            }
                        },
                        Err(err) if attempt < max_attempts => {
                            let delay_ms = 400u64 << (attempt - 1);
                            tracing::warn!(attempt, delay_ms, error = %err, "chunk upload transient error, retrying");
                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        }
                        Err(err) => break Err(err.into()),
                        }
                    }
                });
            }

            if loop_err.is_some() {
                break;
            }

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

        // 先丢弃接收端，确保提前退出时不会因有界通道满导致打包线程与生产者永久死锁。
        drop(result_rx);
        let _ = producer.await;
        for handle in pack_threads {
            let _ = handle.join();
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

    /// 批量向服务端探测缺失分片，跳过已落盘分片实现大文件内秒级断点续传。
    async fn filter_missing_remote_chunks(
        &self,
        vault_id: &str,
        hashes: &[[u8; 32]],
    ) -> Option<HashSet<[u8; 32]>> {
        if hashes.is_empty() {
            return Some(HashSet::new());
        }
        let url = format!("{}/api/vaults/{}/chunks/exists", self.endpoint_base_url, vault_id);
        let hashes_hex: Vec<String> = hashes.iter().map(hex::encode).collect();
        let mut req = self.http.post(&url).json(&serde_json::json!({ "hashes_hex": hashes_hex }));
        if let Some(cookie) = self.session_cookie.read().await.as_ref() {
            req = req.header("Cookie", format!("lv_session={cookie}"));
        }
        let resp = req.send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        #[derive(Deserialize)]
        struct ExistsResp {
            missing: Vec<String>,
        }
        let parsed: ExistsResp = resp.json().await.ok()?;
        let mut set = HashSet::with_capacity(parsed.missing.len());
        for h_hex in parsed.missing {
            if let Ok(bytes) = hex::decode(&h_hex) {
                if let Ok(arr) = <[u8; 32]>::try_from(bytes.as_slice()) {
                    set.insert(arr);
                }
            }
        }
        Some(set)
    }

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
        // 基于 (rule_id, vault_id, root_tree_hash) 派生确定性 snapshot_id，确保网络重试精准命中服务端幂等逻辑。
        let mut id_seed = Vec::new();
        id_seed.extend_from_slice(rule_id.as_bytes());
        id_seed.push(0);
        id_seed.extend_from_slice(vault_id.as_bytes());
        id_seed.push(0);
        id_seed.extend_from_slice(&root_tree_hash);
        let id_hash = blake3_hash(&id_seed);
        let mut uuid_bytes = [0u8; 16];
        uuid_bytes.copy_from_slice(&id_hash[..16]);
        let snapshot_id = Uuid::from_bytes(uuid_bytes).to_string();

        let payload = CommitSnapshotRequest {
            snapshot_id: snapshot_id.clone(),
            rule_id: rule_id.to_string(),
            device_id: self.device_id.clone(),
            root_tree_hash_hex: hex::encode(root_tree_hash),
            retention_tier: "daily".to_string(),
            files,
        };

        let url = format!("{}/api/vaults/{}/snapshots", self.endpoint_base_url, vault_id);
        let max_attempts: u32 = 3;
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            let mut req = self.http.post(&url).json(&payload);
            if let Some(cookie) = self.session_cookie.read().await.as_ref() {
                req = req.header("Cookie", format!("lv_session={cookie}"));
            }
            match req.send().await {
                Ok(resp) if resp.status().is_success() => return Ok(snapshot_id),
                Ok(resp) => {
                    let status = resp.status();
                    if matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504)
                        && attempt < max_attempts
                    {
                        tokio::time::sleep(Duration::from_millis(500u64 << (attempt - 1))).await;
                        continue;
                    }
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
                Err(err) if attempt < max_attempts => {
                    tokio::time::sleep(Duration::from_millis(500u64 << (attempt - 1))).await;
                }
                Err(err) => return Err(err.into()),
            }
        }
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
