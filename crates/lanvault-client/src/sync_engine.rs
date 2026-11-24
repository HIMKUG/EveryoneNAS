//! 增量块上传管线：流式复用 `lanvault_core::{chunk_stream, pack_chunk, unpack_chunk}`，
//! 集成令牌桶限速 (`max_kbps`)、场景化省电门控 (`wifi_only`, `allowed_ssids`, `require_charging`)、
//! 传统快照提交闭环 (`SyncEngine::sync_diff`)，以及 v2 增量目录双指针合并与幂等绑定同步
//! (`compute_two_pointer_merge` / `SyncEngine::sync_binding_v2`)。

use std::cmp::Ordering as CmpOrdering;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::BufReader;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lanvault_core::{
    blake3_hash, chunk_stream, is_incompressible_extension, pack_chunk_adaptive_with_context,
    pack_chunk_with_context, unpack_chunk, CatalogCommitRequest, CatalogCommitResponse,
    CatalogDeltaKind, CatalogDeltaOp, CatalogEntryRecord, ChunkBatchCheckRequest,
    ChunkBatchCheckResponse, ChunkCompression, ChunkParams, ChunkSealContext, Database,
    GroupSyncRuleRecord, LocalFileStateRecord, PackedChunk, ProbeGuard, ProtocolCapabilities,
    RuleTargetBindingRecord, SymmetricKey, SyncLeaseRecord, SyncRuleMode, CHUNK_MAX_SIZE,
    DEFAULT_ZSTD_LEVEL,
};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, Mutex, RwLock, Semaphore};
use tokio::time;
use tracing::{debug, info};
use uuid::Uuid;

use crate::error::ClientError;
use crate::scanner::{
    delete_file_states, DiffResult, FileMetadataSnapshot, ScannedEntry, ScannedEntryKind,
    SourceGeneration,
};

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

/// v2 目录分页每页条目上限。
const CATALOG_PAGE_LIMIT: usize = 1000;

/// v2 同步租约默认续期时长（秒）。
const DEFAULT_SYNC_LEASE_TTL_SECS: i64 = 120;

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
            let elapsed = now
                .saturating_duration_since(state.last_refill)
                .as_secs_f64();
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

// -----------------------------------------------------------------------------
// v2 Two-Pointer Merge Plan & Binding Sync Report
// -----------------------------------------------------------------------------

/// 单条 v2 增量目录合并动作。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeAction {
    /// 仅源端存在：新增文件或目录。
    Add(ScannedEntry),
    /// 两端均存在但内容或类型（文件 <-> 目录）不一致：归档旧当前版本并写入新版本。
    Update {
        source: ScannedEntry,
        prev_version_id: String,
        type_swap: bool,
    },
    /// 仅目标端存在且处于 `DiffBackup` 模式（且当前尚未标记 `source_deleted`）。
    MarkSourceDeleted {
        path_key: String,
        display_path: String,
    },
    /// 两端均存在且内容完全一致，但目标端此前带有 `source_deleted` 标记：仅清除标记，零上传。
    ClearSourceDeleted { source: ScannedEntry },
    /// 仅目标端存在且处于 `MirrorSync` 模式：归档并从当前目录移除。
    ArchiveAndRemove {
        path_key: String,
        display_path: String,
        prev_version_id: String,
    },
}

/// O(N + M) 双指针目录合并计划。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergePlan {
    pub actions: Vec<MergeAction>,
    pub unchanged_count: usize,
    pub blocked_deletions: usize,
}

impl MergePlan {
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }
}

/// 对按 `path_key` 升序排列的源端快照与目标端目录执行严格 O(N + M) 双指针合并。
///
/// 覆盖规范全部 5 种状态：
/// 1. 仅源端存在 -> `Add`
/// 2. 两端均存在且内容不同 -> `Update`（严格基于 `content_hash`，绝不以 `(size, mtime)` 猜测相等）
/// 3. 仅目标端存在 -> 仅当 `source_complete == true` 时：`DiffBackup` 生成 `MarkSourceDeleted`
///    （若已为 `source_deleted == true` 则跳过，不刷新删除时间），`MirrorSync` 生成 `ArchiveAndRemove`；
///    若 `!source_complete`，保留目标状态并累加 `blocked_deletions`
/// 4. 两端均存在且内容相同 -> 若 `target.source_deleted` 生成 `ClearSourceDeleted`，否则计入 `unchanged_count`
/// 5. 类型不一致（file vs dir） -> `Update { type_swap: true }`
pub fn compute_two_pointer_merge(
    source: &[ScannedEntry],
    target: &[CatalogEntryRecord],
    mode: SyncRuleMode,
    source_complete: bool,
) -> MergePlan {
    let mut plan = MergePlan::default();
    let (mut i, mut j) = (0usize, 0usize);

    while i < source.len() && j < target.len() {
        let s = &source[i];
        let t = &target[j];
        match s.path_key.cmp(&t.path_key) {
            CmpOrdering::Less => {
                plan.actions.push(MergeAction::Add(s.clone()));
                i += 1;
            }
            CmpOrdering::Greater => {
                handle_target_only(t, mode, source_complete, &mut plan);
                j += 1;
            }
            CmpOrdering::Equal => {
                let kind_matches = s.kind.as_str() == t.entry_kind.as_str();
                if !kind_matches {
                    plan.actions.push(MergeAction::Update {
                        source: s.clone(),
                        prev_version_id: t.version_id.clone(),
                        type_swap: true,
                    });
                } else {
                    let content_identical = match s.kind {
                        ScannedEntryKind::Dir => true,
                        ScannedEntryKind::File => match (&s.content_hash, &t.content_hash) {
                            (Some(sh), Some(th)) => sh.as_slice() == th.as_slice(),
                            _ => false,
                        },
                    };
                    if content_identical {
                        if t.source_deleted {
                            plan.actions
                                .push(MergeAction::ClearSourceDeleted { source: s.clone() });
                        } else {
                            plan.unchanged_count += 1;
                        }
                    } else {
                        plan.actions.push(MergeAction::Update {
                            source: s.clone(),
                            prev_version_id: t.version_id.clone(),
                            type_swap: false,
                        });
                    }
                }
                i += 1;
                j += 1;
            }
        }
    }

    while i < source.len() {
        plan.actions.push(MergeAction::Add(source[i].clone()));
        i += 1;
    }
    while j < target.len() {
        handle_target_only(&target[j], mode, source_complete, &mut plan);
        j += 1;
    }

    plan
}

fn handle_target_only(
    target: &CatalogEntryRecord,
    mode: SyncRuleMode,
    source_complete: bool,
    plan: &mut MergePlan,
) {
    if !source_complete {
        plan.blocked_deletions += 1;
        return;
    }
    match mode {
        SyncRuleMode::DiffBackup => {
            if !target.source_deleted {
                plan.actions.push(MergeAction::MarkSourceDeleted {
                    path_key: target.path_key.clone(),
                    display_path: target.display_path.clone(),
                });
            } else {
                plan.unchanged_count += 1;
            }
        }
        SyncRuleMode::MirrorSync => {
            plan.actions.push(MergeAction::ArchiveAndRemove {
                path_key: target.path_key.clone(),
                display_path: target.display_path.clone(),
                prev_version_id: target.version_id.clone(),
            });
        }
    }
}

/// v2 单次绑定增量同步报告。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BindingSyncReport {
    pub binding_id: String,
    pub rule_id: String,
    pub operation_id: Option<String>,
    pub new_revision: Option<i64>,
    pub added: usize,
    pub updated: usize,
    pub marked_deleted: usize,
    pub cleared_deleted: usize,
    pub archived_and_removed: usize,
    pub unchanged: usize,
    pub blocked_deletions: usize,
    pub chunks_uploaded: u64,
    pub chunks_deduplicated: u64,
    pub bytes_processed: u64,
    pub deduplicated_replay: bool,
}

#[derive(Debug, Serialize)]
struct AcquireLeaseRequest<'a> {
    rule_id: &'a str,
    holder_device_id: &'a str,
    group_revision: i64,
    rule_revision: i64,
    ttl_secs: i64,
}

#[derive(Debug, Deserialize)]
struct CatalogPageResponse {
    entries: Vec<CatalogEntryRecord>,
    #[serde(default)]
    next_after_path_key: Option<String>,
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

    // -------------------------------------------------------------------------
    // v2 Incremental Catalog Binding Sync
    // -------------------------------------------------------------------------

    /// 执行 v2 绑定级增量目录同步：
    /// 1. 校验远端 `GET /api/capabilities` 支持 `incremental_catalog`；
    /// 2. 申请带栅栏令牌的同步租约 (`POST /api/bindings/{binding_id}/leases`)；
    /// 3. 游标分页拉取目标目录 (`GET /api/bindings/{binding_id}/catalog`)；
    /// 4. 执行 O(N + M) `compute_two_pointer_merge`，无差异时立即零上传零提交返回；
    /// 5. 对 `Add`/`Update` 文件流式 FastCDC + 每目标库独立 `ChunkSealContext` 加密，
    ///    批量检查缺失块 (`POST /api/vaults/{vault_id}/chunks/check`) 并仅上传缺失块；
    /// 6. 以稳定 `operation_id` 提交增量批次 (`POST /api/bindings/{binding_id}/catalog/commit`)，
    ///    发送后遇传输错误先查 `GET /api/bindings/{binding_id}/catalog/operations/{operation_id}` 再决定是否重试。
    pub async fn sync_binding_v2(
        &self,
        rule: &GroupSyncRuleRecord,
        binding: &RuleTargetBindingRecord,
        generation: &SourceGeneration,
        holder_id: &str,
    ) -> Result<BindingSyncReport, ClientError> {
        self.check_policy().await?;

        let vault_id = binding.target_vault_id.as_deref().ok_or_else(|| {
            ClientError::InvalidInput(format!(
                "binding `{}` is missing target_vault_id",
                binding.binding_id
            ))
        })?;

        let caps = self.fetch_capabilities().await?;
        if !caps.incremental_catalog {
            return Err(ClientError::InvalidInput(format!(
                "remote peer `{}` does not support v2 incremental_catalog",
                self.endpoint_base_url
            )));
        }

        let mut lease = self
            .acquire_binding_lease(
                &binding.binding_id,
                &rule.rule_id,
                holder_id,
                rule.group_revision,
                rule.revision,
            )
            .await?;
        let mut lease_refreshed_at = Instant::now();

        let target_entries = self.fetch_full_target_catalog(&binding.binding_id).await?;
        let plan = compute_two_pointer_merge(
            &generation.entries,
            &target_entries,
            rule.mode,
            generation.complete,
        );

        let mut report = BindingSyncReport {
            binding_id: binding.binding_id.clone(),
            rule_id: rule.rule_id.clone(),
            unchanged: plan.unchanged_count,
            blocked_deletions: plan.blocked_deletions,
            ..Default::default()
        };

        if plan.is_empty() {
            return Ok(report);
        }

        let chunk_key = self.vault_master_key.derive_vault_chunk_key(vault_id);
        let seal_ctx = Arc::new(ChunkSealContext::new(&chunk_key, vault_id.as_bytes()));
        let now_ts = unix_now_secs();
        let mirror_expires_at = match rule.mode {
            SyncRuleMode::MirrorSync => {
                Some(now_ts + i64::from(rule.archive_retention_days) * 86_400)
            }
            SyncRuleMode::DiffBackup => None,
        };

        let mut delta_ops: Vec<CatalogDeltaOp> = Vec::with_capacity(plan.actions.len());
        for action in plan.actions {
            if lease_refreshed_at.elapsed() >= Duration::from_secs(60) {
                lease = self
                    .acquire_binding_lease(
                        &binding.binding_id,
                        &rule.rule_id,
                        holder_id,
                        rule.group_revision,
                        rule.revision,
                    )
                    .await?;
                lease_refreshed_at = Instant::now();
            }
            match action {
                MergeAction::Add(entry) => {
                    report.added += 1;
                    let op = self
                        .build_upsert_delta_op(vault_id, &seal_ctx, &entry, None, None, &mut report)
                        .await?;
                    delta_ops.push(op);
                }
                MergeAction::Update {
                    source,
                    prev_version_id: _,
                    type_swap,
                } => {
                    report.updated += 1;
                    let reason = match (rule.mode, type_swap) {
                        (SyncRuleMode::MirrorSync, _) => "mirror_overwritten",
                        (SyncRuleMode::DiffBackup, true) => "diff_type_replaced",
                        (SyncRuleMode::DiffBackup, false) => "diff_replaced",
                    };
                    let op = self
                        .build_upsert_delta_op(
                            vault_id,
                            &seal_ctx,
                            &source,
                            Some(reason.to_string()),
                            mirror_expires_at,
                            &mut report,
                        )
                        .await?;
                    delta_ops.push(op);
                }
                MergeAction::MarkSourceDeleted {
                    path_key,
                    display_path,
                } => {
                    report.marked_deleted += 1;
                    delta_ops.push(CatalogDeltaOp {
                        path_key,
                        display_path,
                        entry_kind: "file".to_string(),
                        kind: CatalogDeltaKind::MarkSourceDeleted,
                        size: 0,
                        mtime_ns: 0,
                        content_hash_hex: None,
                        chunk_hashes_hex: Vec::new(),
                        archive_reason: None,
                        expires_at: None,
                    });
                }
                MergeAction::ClearSourceDeleted { source } => {
                    report.cleared_deleted += 1;
                    delta_ops.push(CatalogDeltaOp {
                        path_key: source.path_key,
                        display_path: source.display_path,
                        entry_kind: source.kind.as_str().to_string(),
                        kind: CatalogDeltaKind::ClearSourceDeleted,
                        size: source.size,
                        mtime_ns: source.mtime_ns,
                        content_hash_hex: source.content_hash.map(hex::encode),
                        chunk_hashes_hex: Vec::new(),
                        archive_reason: None,
                        expires_at: None,
                    });
                }
                MergeAction::ArchiveAndRemove {
                    path_key,
                    display_path,
                    prev_version_id: _,
                } => {
                    report.archived_and_removed += 1;
                    delta_ops.push(CatalogDeltaOp {
                        path_key,
                        display_path,
                        entry_kind: "file".to_string(),
                        kind: CatalogDeltaKind::ArchiveAndRemove,
                        size: 0,
                        mtime_ns: 0,
                        content_hash_hex: None,
                        chunk_hashes_hex: Vec::new(),
                        archive_reason: Some("mirror_removed".to_string()),
                        expires_at: mirror_expires_at,
                    });
                }
            }
        }

        if lease_refreshed_at.elapsed() >= Duration::from_secs(60) {
            lease = self
                .acquire_binding_lease(
                    &binding.binding_id,
                    &rule.rule_id,
                    holder_id,
                    rule.group_revision,
                    rule.revision,
                )
                .await?;
        }

        let operation_id = Uuid::new_v4().to_string();
        let commit_req = CatalogCommitRequest {
            operation_id: operation_id.clone(),
            rule_id: rule.rule_id.clone(),
            group_revision: lease.group_revision,
            rule_revision: lease.rule_revision,
            fence_token: lease.fence_token,
            base_revision: binding.applied_revision,
            source_gen: binding.last_source_gen + 1,
            ops: delta_ops,
        };

        let commit_resp = self
            .commit_catalog_with_idempotent_recovery(&binding.binding_id, &commit_req)
            .await?;

        report.operation_id = Some(commit_resp.operation_id);
        report.new_revision = Some(commit_resp.new_revision);
        report.deduplicated_replay = commit_resp.deduplicated_replay;
        Ok(report)
    }

    async fn check_policy(&self) -> Result<(), ClientError> {
        let policy = self.policy.read().await.clone();
        let env = self.env.read().await.clone();
        policy.evaluate(&env).map_err(ClientError::PolicyBlocked)
    }

    async fn apply_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self.session_cookie.read().await.as_ref() {
            Some(cookie) => req.header("Cookie", format!("lv_session={cookie}")),
            None => req,
        }
    }

    async fn fetch_capabilities(&self) -> Result<ProtocolCapabilities, ClientError> {
        let url = format!("{}/api/capabilities", self.endpoint_base_url);
        let resp = self.apply_auth(self.http.get(&url)).await.send().await?;
        if !resp.status().is_success() {
            return Err(ClientError::RemoteStatus {
                url,
                status: resp.status().as_u16(),
            });
        }
        Ok(resp.json::<ProtocolCapabilities>().await?)
    }

    async fn acquire_binding_lease(
        &self,
        binding_id: &str,
        rule_id: &str,
        holder_id: &str,
        group_revision: i64,
        rule_revision: i64,
    ) -> Result<SyncLeaseRecord, ClientError> {
        let url = format!(
            "{}/api/bindings/{binding_id}/leases",
            self.endpoint_base_url
        );
        let body = AcquireLeaseRequest {
            rule_id,
            holder_device_id: holder_id,
            group_revision,
            rule_revision,
            ttl_secs: DEFAULT_SYNC_LEASE_TTL_SECS,
        };
        let resp = self
            .apply_auth(self.http.post(&url).json(&body))
            .await
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(ClientError::RemoteRejected { status, body: text });
        }
        Ok(resp.json::<SyncLeaseRecord>().await?)
    }

    async fn fetch_full_target_catalog(
        &self,
        binding_id: &str,
    ) -> Result<Vec<CatalogEntryRecord>, ClientError> {
        let mut all = Vec::new();
        let mut after_path_key: Option<String> = None;

        loop {
            let url = format!(
                "{}/api/bindings/{binding_id}/catalog",
                self.endpoint_base_url
            );
            let mut req = self
                .apply_auth(self.http.get(&url))
                .await
                .query(&[("limit", CATALOG_PAGE_LIMIT.to_string())]);
            if let Some(ref cursor) = after_path_key {
                req = req.query(&[("after_path_key", cursor.as_str())]);
            }

            let resp = req.send().await?;
            if !resp.status().is_success() {
                return Err(ClientError::RemoteStatus {
                    url,
                    status: resp.status().as_u16(),
                });
            }

            let page = resp.json::<CatalogPageResponse>().await?;
            let page_len = page.entries.len();
            let next_cursor = page
                .next_after_path_key
                .or_else(|| page.entries.last().map(|e| e.path_key.clone()));
            all.extend(page.entries);

            if page_len < CATALOG_PAGE_LIMIT {
                break;
            }
            match next_cursor {
                Some(cursor) if after_path_key.as_deref() != Some(cursor.as_str()) => {
                    after_path_key = Some(cursor);
                }
                _ => break,
            }
        }

        all.sort_by(|a, b| a.path_key.cmp(&b.path_key));
        Ok(all)
    }

    async fn build_upsert_delta_op(
        &self,
        vault_id: &str,
        seal_ctx: &Arc<ChunkSealContext>,
        entry: &ScannedEntry,
        archive_reason: Option<String>,
        expires_at: Option<i64>,
        report: &mut BindingSyncReport,
    ) -> Result<CatalogDeltaOp, ClientError> {
        if entry.kind == ScannedEntryKind::Dir {
            return Ok(CatalogDeltaOp {
                path_key: entry.path_key.clone(),
                display_path: entry.display_path.clone(),
                entry_kind: "dir".to_string(),
                kind: CatalogDeltaKind::UpsertDir,
                size: 0,
                mtime_ns: entry.mtime_ns,
                content_hash_hex: None,
                chunk_hashes_hex: Vec::new(),
                archive_reason,
                expires_at,
            });
        }

        self.check_policy().await?;
        let abs_path = entry.abs_path.clone();
        let seal_for_worker = Arc::clone(seal_ctx);

        let packed_chunks: Vec<PackedChunk> = tokio::task::spawn_blocking(move || {
            let file = File::open(&abs_path)?;
            let reader = BufReader::with_capacity(512 * 1024, file);
            let mut chunks = Vec::new();
            chunk_stream(reader, ChunkParams::default(), |_desc, raw_bytes| {
                let packed = pack_chunk_with_context(
                    &raw_bytes,
                    Some(&seal_for_worker),
                    DEFAULT_ZSTD_LEVEL,
                )?;
                chunks.push(packed);
                Ok(())
            })?;
            Ok::<Vec<PackedChunk>, ClientError>(chunks)
        })
        .await
        .map_err(|err| {
            ClientError::Io(std::io::Error::other(format!(
                "chunk worker panicked: {err}"
            )))
        })??;

        let chunk_hashes_hex: Vec<String> = packed_chunks
            .iter()
            .map(|c| hex::encode(c.stored_hash))
            .collect();

        if !packed_chunks.is_empty() {
            let unique_hashes: Vec<String> = {
                let mut seen = HashSet::with_capacity(chunk_hashes_hex.len());
                chunk_hashes_hex
                    .iter()
                    .filter(|h| seen.insert((*h).clone()))
                    .cloned()
                    .collect()
            };
            let check = self.batch_check_chunks(vault_id, unique_hashes).await?;
            let missing_set: HashSet<String> = check.missing.into_iter().collect();

            let mut uploaded_in_file: HashSet<[u8; 32]> = HashSet::new();
            for chunk in packed_chunks {
                let hash_hex = hex::encode(chunk.stored_hash);
                report.bytes_processed += chunk.raw_size as u64;
                if !missing_set.contains(&hash_hex) || !uploaded_in_file.insert(chunk.stored_hash) {
                    report.chunks_deduplicated += 1;
                    continue;
                }
                self.check_policy().await?;
                self.rate_limiter.acquire(chunk.stored_bytes.len()).await;
                let server_dedup = self
                    .upload_single_chunk(vault_id, &hash_hex, chunk.raw_size, chunk.stored_bytes)
                    .await?;
                self.record_remote_chunk(vault_id, chunk.stored_hash).await;
                if server_dedup {
                    report.chunks_deduplicated += 1;
                } else {
                    report.chunks_uploaded += 1;
                }
            }
        }

        Ok(CatalogDeltaOp {
            path_key: entry.path_key.clone(),
            display_path: entry.display_path.clone(),
            entry_kind: "file".to_string(),
            kind: CatalogDeltaKind::UpsertFile,
            size: entry.size,
            mtime_ns: entry.mtime_ns,
            content_hash_hex: entry.content_hash.map(hex::encode),
            chunk_hashes_hex,
            archive_reason,
            expires_at,
        })
    }

    async fn batch_check_chunks(
        &self,
        vault_id: &str,
        hashes_hex: Vec<String>,
    ) -> Result<ChunkBatchCheckResponse, ClientError> {
        const MAX_HASHES_PER_CHECK: usize = 4096;
        let mut present = Vec::with_capacity(hashes_hex.len());
        let mut missing = Vec::with_capacity(hashes_hex.len());
        for chunk in hashes_hex.chunks(MAX_HASHES_PER_CHECK) {
            let mut response = self
                .batch_check_chunks_once(vault_id, chunk.to_vec())
                .await?;
            present.append(&mut response.present);
            missing.append(&mut response.missing);
        }
        Ok(ChunkBatchCheckResponse { present, missing })
    }

    async fn batch_check_chunks_once(
        &self,
        vault_id: &str,
        hashes_hex: Vec<String>,
    ) -> Result<ChunkBatchCheckResponse, ClientError> {
        let url = format!(
            "{}/api/vaults/{vault_id}/chunks/check",
            self.endpoint_base_url
        );
        let resp = self
            .apply_auth(
                self.http
                    .post(&url)
                    .json(&ChunkBatchCheckRequest { hashes_hex }),
            )
            .await
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::RemoteRejected { status, body });
        }
        Ok(resp.json::<ChunkBatchCheckResponse>().await?)
    }

    async fn upload_single_chunk(
        &self,
        vault_id: &str,
        hash_hex: &str,
        raw_size: usize,
        stored_bytes: Vec<u8>,
    ) -> Result<bool, ClientError> {
        let url = format!(
            "{}/api/vaults/{vault_id}/chunks/{hash_hex}",
            self.endpoint_base_url
        );
        let resp = self
            .apply_auth(
                self.http
                    .put(&url)
                    .header("Content-Type", "application/octet-stream")
                    .header("X-LanVault-Encoding", "xchacha20poly1305+zstd")
                    .header("X-LanVault-Raw-Size", raw_size.to_string())
                    .body(stored_bytes),
            )
            .await
            .send()
            .await?;
        match resp.status() {
            StatusCode::CREATED => Ok(false),
            StatusCode::OK | StatusCode::CONFLICT | StatusCode::NOT_MODIFIED => Ok(true),
            other => {
                let body = resp.text().await.unwrap_or_default();
                Err(ClientError::RemoteRejected {
                    status: other.as_u16(),
                    body,
                })
            }
        }
    }

    async fn commit_catalog_with_idempotent_recovery(
        &self,
        binding_id: &str,
        req_payload: &CatalogCommitRequest,
    ) -> Result<CatalogCommitResponse, ClientError> {
        let commit_url = format!(
            "{}/api/bindings/{binding_id}/catalog/commit",
            self.endpoint_base_url
        );
        match self.send_catalog_commit(&commit_url, req_payload).await {
            Ok(resp) => Ok(resp),
            Err(ClientError::Http(transport_err)) => {
                // On transport error after send, inspect operation status before retrying.
                if let Some(mut parsed) = self
                    .query_catalog_operation(binding_id, &req_payload.operation_id)
                    .await?
                {
                    parsed.deduplicated_replay = true;
                    return Ok(parsed);
                }
                debug!(
                    operation_id = %req_payload.operation_id,
                    "operation not found after transport error ({transport_err}); retrying commit once"
                );
                self.send_catalog_commit(&commit_url, req_payload).await
            }
            Err(other) => Err(other),
        }
    }

    async fn send_catalog_commit(
        &self,
        commit_url: &str,
        req_payload: &CatalogCommitRequest,
    ) -> Result<CatalogCommitResponse, ClientError> {
        let resp = self
            .apply_auth(self.http.post(commit_url).json(req_payload))
            .await
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::RemoteRejected { status, body });
        }
        Ok(resp.json::<CatalogCommitResponse>().await?)
    }

    async fn query_catalog_operation(
        &self,
        binding_id: &str,
        operation_id: &str,
    ) -> Result<Option<CatalogCommitResponse>, ClientError> {
        let url = format!(
            "{}/api/bindings/{binding_id}/catalog/operations/{operation_id}",
            self.endpoint_base_url
        );
        let resp = self.apply_auth(self.http.get(&url)).await.send().await?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(ClientError::RemoteStatus {
                url,
                status: resp.status().as_u16(),
            });
        }
        #[derive(Deserialize)]
        struct OperationResponse {
            response: CatalogCommitResponse,
        }
        Ok(Some(resp.json::<OperationResponse>().await?.response))
    }

    // -------------------------------------------------------------------------
    // Legacy Diff Sync Pipeline
    // -------------------------------------------------------------------------

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
                    .finalize_pass(
                        db,
                        rule_id,
                        vault_id,
                        &mut summary,
                        pending_db_states,
                        &partial_diff,
                    )
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

    async fn commit_deletions(
        &self,
        db: &Database,
        rule_id: &str,
        vault_id: &str,
        diff: &DiffResult,
    ) -> Result<(), ClientError> {
        let mut noop_summary = SyncSummary::default();
        self.finalize_pass(db, rule_id, vault_id, &mut noop_summary, Vec::new(), diff)
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

        let pack_pool = self.pack_pool;
        let (result_tx, mut result_rx) = mpsc::channel::<PackResult>(CHUNK_QUEUE_DEPTH * 2);
        let result_error_tx = result_tx.clone();

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
                                .blocking_send(PackResult {
                                    idx: job.idx,
                                    packed,
                                })
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
                        && metadata.modified().ok().and_then(|modified| {
                            modified
                                .duration_since(std::time::UNIX_EPOCH)
                                .ok()
                                .map(|duration| duration.as_nanos() as i64)
                        }) == Some(expected_mtime_ns) => {}
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
                let already_exists = self.known_remote_chunks.read().await.contains(&cache_key);

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
                let pending_bytes: usize = pending_check
                    .iter()
                    .map(|(p, _)| p.stored_bytes.len())
                    .sum();
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
                let mem_units = packed
                    .stored_bytes
                    .len()
                    .div_ceil(MEM_UNIT_BYTES)
                    .clamp(1, GLOBAL_MEM_BUDGET_UNITS) as u32;
                let Ok(mem_permit) = self.mem_budget.clone().acquire_many_owned(mem_units).await
                else {
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
        let url = format!(
            "{}/api/vaults/{}/chunks/exists",
            self.endpoint_base_url, vault_id
        );
        let hashes_hex: Vec<String> = hashes.iter().map(hex::encode).collect();
        let mut req = self
            .http
            .post(&url)
            .json(&serde_json::json!({ "hashes_hex": hashes_hex }));
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
            let evict_keys: Vec<(String, [u8; 32])> =
                guard.iter().take(to_remove).cloned().collect();
            for key in evict_keys {
                guard.remove(&key);
            }
        }
        guard.insert((vault_id.to_string(), hash));
    }

    fn emit_shared_progress(&self, rule_id: &str, current_file: &str, progress: &SharedProgress) {
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

        let url = format!(
            "{}/api/vaults/{}/snapshots",
            self.endpoint_base_url, vault_id
        );
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
                Err(_err) if attempt < max_attempts => {
                    tokio::time::sleep(Duration::from_millis(500u64 << (attempt - 1))).await;
                }
                Err(err) => return Err(err.into()),
            }
        }
    }
}

fn unix_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 单次同步汇总统计（legacy）。
#[derive(Debug, Clone, Default)]
pub struct SyncSummary {
    pub files_synced: u64,
    pub bytes_processed: u64,
    pub chunks_uploaded: u64,
    pub chunks_deduplicated: u64,
    pub snapshot_id: Option<String>,
}
