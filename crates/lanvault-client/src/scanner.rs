//! 本地目录遍历、SQLite `local_file_state(size, mtime_ns, file_id)` 快速元数据 Diff 对账（legacy），
//! 以及 v2 确定性源代扫描 (`scan_source_generation`)：规范化路径、流式 BLAKE3 校验、
//! 并发修改（volatile）检测、符号链接/重解析点拦截与跨平台路径碰撞检测。

use std::collections::HashMap;
use std::fs::{self, File, Metadata};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use lanvault_core::{
    blake3_hash_reader, detect_path_collisions, normalize_canonical_path, Database,
    LocalFileStateRecord, PathCollision,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::error::ClientError;

/// 本地文件元数据快照（legacy）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMetadataSnapshot {
    /// 相对于同步根目录的 POSIX 风格相对路径（统一使用 `/` 分隔符）。
    pub rel_path: String,
    /// 本地绝对路径。
    pub abs_path: PathBuf,
    /// 文件字节大小。
    pub size: u64,
    /// 修改时间（纳秒级 Unix 时间戳）。
    pub mtime_ns: i64,
    /// 平台文件唯一标识（Unix inode / Windows creation_time）。
    pub file_id: Option<i64>,
}

/// 数据库中存储的本地文件状态记录（legacy）。
#[derive(Debug, Clone)]
pub struct StoredFileState {
    pub target_vault_id: String,
    pub size: u64,
    pub mtime_ns: i64,
    pub file_id: Option<i64>,
    pub chunk_hashes: Vec<u8>,
}

/// 目录元数据快速 Diff 对账结果（legacy）。
#[derive(Debug, Clone, Default)]
pub struct DiffResult {
    /// 新增文件列表。
    pub added: Vec<FileMetadataSnapshot>,
    /// 大小、修改时间或 `file_id` 发生变化的老文件列表。
    pub modified: Vec<FileMetadataSnapshot>,
    /// 本地已删除的相对路径列表。
    pub deleted: Vec<String>,
    /// 元数据完全一致、可跳过分块的未变更文件数量。
    pub unchanged_count: usize,
}

impl DiffResult {
    /// 返回需要执行增量分块与上传的全部变更文件（新增 + 修改）。
    pub fn changed_files(&self) -> impl Iterator<Item = &FileMetadataSnapshot> {
        self.added.iter().chain(self.modified.iter())
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.modified.is_empty() && self.deleted.is_empty()
    }
}

// -----------------------------------------------------------------------------
// v2 Source Generation Scanner
// -----------------------------------------------------------------------------

/// v2 扫描条目类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScannedEntryKind {
    File,
    Dir,
}

impl ScannedEntryKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Dir => "dir",
        }
    }
}

/// v2 确定性源代扫描条目（按 `path_key` 严格字典序排列，文件必含真实流式 BLAKE3 摘要）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScannedEntry {
    pub path_key: String,
    pub display_path: String,
    pub abs_path: PathBuf,
    pub kind: ScannedEntryKind,
    pub size: u64,
    pub mtime_ns: i64,
    pub file_id: Option<i64>,
    pub content_hash: Option<[u8; 32]>,
}

/// v2 一次完整或受阻的源目录代际扫描结果。
///
/// 仅当 `complete == true` 时下游合并/镜像引擎才允许推导“源端已删除”操作。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceGeneration {
    pub root_dir: PathBuf,
    pub root_identity: Option<String>,
    pub complete: bool,
    pub blocked_reasons: Vec<String>,
    pub volatile_paths: Vec<String>,
    pub collisions: Vec<PathCollision>,
    pub entries: Vec<ScannedEntry>,
}

/// 扫描源目录生成确定性 `SourceGeneration`：
/// - 校验 `root_dir` 为真实目录且非符号链接/重解析点；
/// - 迭代遍历目录树，拦截符号链接、Windows junction/reparse point 与特殊文件并记录至 `blocked_reasons`；
/// - 通过 `lanvault_core::normalize_canonical_path` 规范化每条相对路径（非法路径标记 `complete = false`）；
/// - 任何 `PermissionDenied` 或 I/O 错误均标记 `complete = false` 并记录原因，绝不触发误删；
/// - 对每个常规文件计算流式 BLAKE3，读前/读后比对 `(file_id, size, mtime_ns)`，读取中途变更则记入 `volatile_paths` 并跳过；
/// - 按 `path_key` 规范排序并运行 `lanvault_core::detect_path_collisions`。
pub fn scan_source_generation(
    root_dir: &Path,
    exclude_patterns: &[String],
) -> Result<SourceGeneration, ClientError> {
    let _probe = lanvault_core::ProbeGuard::start(
        "client.scanner",
        "scanner.scan_source_generation",
        Some(format!("root={}", root_dir.display())),
    );

    let root_meta = fs::symlink_metadata(root_dir).map_err(|err| ClientError::InvalidPath {
        path: root_dir.to_path_buf(),
        reason: format!("cannot stat scan root: {err}"),
    })?;
    if root_meta.file_type().is_symlink() || is_reparse_point(&root_meta) {
        return Err(ClientError::InvalidPath {
            path: root_dir.to_path_buf(),
            reason: "scan root must not be a symlink or reparse point".to_string(),
        });
    }
    if !root_meta.is_dir() {
        return Err(ClientError::InvalidPath {
            path: root_dir.to_path_buf(),
            reason: "scan root must be an existing directory".to_string(),
        });
    }

    let clean_root = strip_windows_verbatim(root_dir);
    let root_identity = extract_root_identity(&root_meta);

    let mut complete = true;
    let mut blocked_reasons: Vec<String> = Vec::new();
    let mut volatile_paths: Vec<String> = Vec::new();
    let mut entries: Vec<ScannedEntry> = Vec::new();

    let mut stack = vec![root_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let read_dir = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(err) => {
                complete = false;
                blocked_reasons.push(format!("cannot read directory `{}`: {err}", dir.display()));
                warn!(path = %dir.display(), "incomplete scan: unreadable directory: {err}");
                continue;
            }
        };

        for entry_res in read_dir {
            let entry = match entry_res {
                Ok(e) => e,
                Err(err) => {
                    complete = false;
                    blocked_reasons
                        .push(format!("cannot read entry in `{}`: {err}", dir.display()));
                    warn!(dir = %dir.display(), "incomplete scan: unreadable dir entry: {err}");
                    continue;
                }
            };

            let abs_path = entry.path();
            let raw_rel = match extract_raw_rel_path(&clean_root, &abs_path) {
                Ok(r) => r,
                Err(err) => {
                    complete = false;
                    blocked_reasons.push(format!(
                        "invalid relative path `{}`: {err}",
                        abs_path.display()
                    ));
                    continue;
                }
            };

            if is_excluded(&raw_rel, exclude_patterns) {
                continue;
            }

            let meta = match fs::symlink_metadata(&abs_path) {
                Ok(m) => m,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    complete = false;
                    volatile_paths.push(raw_rel);
                    continue;
                }
                Err(err) => {
                    complete = false;
                    blocked_reasons.push(format!("cannot stat `{}`: {err}", abs_path.display()));
                    continue;
                }
            };

            let ft = meta.file_type();
            if ft.is_symlink() || is_reparse_point(&meta) {
                blocked_reasons.push(format!(
                    "unsupported symlink or reparse point skipped: `{raw_rel}`"
                ));
                continue;
            }

            if !ft.is_file() && !ft.is_dir() {
                blocked_reasons.push(format!("unsupported special file skipped: `{raw_rel}`"));
                continue;
            }

            let path_key = match normalize_canonical_path(&raw_rel) {
                Ok(canonical) => canonical,
                Err(err) => {
                    complete = false;
                    blocked_reasons.push(format!("invalid path `{raw_rel}`: {err}"));
                    continue;
                }
            };

            let mtime_ns = match extract_mtime_ns(&meta) {
                Ok(ts) => ts,
                Err(err) => {
                    complete = false;
                    blocked_reasons.push(format!("cannot read mtime for `{path_key}`: {err}"));
                    continue;
                }
            };
            let file_id = extract_file_id(&meta);

            if ft.is_dir() {
                entries.push(ScannedEntry {
                    path_key: path_key.clone(),
                    display_path: path_key,
                    abs_path: abs_path.clone(),
                    kind: ScannedEntryKind::Dir,
                    size: 0,
                    mtime_ns,
                    file_id,
                    content_hash: None,
                });
                stack.push(abs_path);
                continue;
            }

            // Regular file: compute streaming BLAKE3 and verify (file_id, size, mtime_ns) before & after.
            let pre_size = meta.len();
            let pre_sig = (file_id, pre_size, mtime_ns);

            let hash = match hash_regular_file(&abs_path) {
                Ok(h) => h,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    complete = false;
                    volatile_paths.push(path_key);
                    continue;
                }
                Err(err) => {
                    complete = false;
                    blocked_reasons.push(format!("cannot hash `{path_key}`: {err}"));
                    continue;
                }
            };

            let post_meta = match fs::symlink_metadata(&abs_path) {
                Ok(m) => m,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    complete = false;
                    volatile_paths.push(path_key);
                    continue;
                }
                Err(err) => {
                    complete = false;
                    blocked_reasons.push(format!("cannot re-stat `{path_key}`: {err}"));
                    continue;
                }
            };
            let post_mtime_ns = match extract_mtime_ns(&post_meta) {
                Ok(ts) => ts,
                Err(err) => {
                    complete = false;
                    blocked_reasons.push(format!("cannot re-read mtime for `{path_key}`: {err}"));
                    continue;
                }
            };
            let post_sig = (extract_file_id(&post_meta), post_meta.len(), post_mtime_ns);

            if !post_meta.file_type().is_file() || post_sig != pre_sig {
                complete = false;
                volatile_paths.push(path_key);
                continue;
            }

            entries.push(ScannedEntry {
                path_key: path_key.clone(),
                display_path: path_key,
                abs_path,
                kind: ScannedEntryKind::File,
                size: pre_size,
                mtime_ns,
                file_id,
                content_hash: Some(hash),
            });
        }
    }

    entries.sort_by(|a, b| a.path_key.cmp(&b.path_key));
    volatile_paths.sort();
    volatile_paths.dedup();

    let all_paths: Vec<String> = entries.iter().map(|e| e.path_key.clone()).collect();
    let collisions = detect_path_collisions(&all_paths);

    Ok(SourceGeneration {
        root_dir: root_dir.to_path_buf(),
        root_identity,
        complete,
        blocked_reasons,
        volatile_paths,
        collisions,
        entries,
    })
}

fn hash_regular_file(path: &Path) -> std::io::Result<[u8; 32]> {
    let file = File::open(path)?;
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    blake3_hash_reader(&mut reader).map_err(|err| match err {
        lanvault_core::LanVaultError::Io(io_err) => io_err,
        other => std::io::Error::other(other.to_string()),
    })
}

fn extract_raw_rel_path(clean_root: &Path, abs_path: &Path) -> Result<String, ClientError> {
    let clean_abs = strip_windows_verbatim(abs_path);
    let rel = clean_abs
        .strip_prefix(clean_root)
        .map_err(|_| ClientError::InvalidPath {
            path: abs_path.to_path_buf(),
            reason: format!("path is outside scan root {}", clean_root.display()),
        })?;
    normalize_rel_path(rel)
}

fn is_excluded(rel_path: &str, exclude_patterns: &[String]) -> bool {
    for raw in exclude_patterns {
        let pat = raw.trim().trim_matches('/');
        if pat.is_empty() {
            continue;
        }
        if let Some(suffix) = pat.strip_prefix('*') {
            if rel_path.ends_with(suffix) || rel_path.split('/').any(|seg| seg.ends_with(suffix)) {
                return true;
            }
        } else if rel_path == pat
            || rel_path.starts_with(&format!("{pat}/"))
            || rel_path.split('/').any(|seg| seg == pat)
        {
            return true;
        }
    }
    false
}

fn extract_mtime_ns(meta: &Metadata) -> Result<i64, ClientError> {
    let mtime = meta
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    Ok(mtime)
}

#[cfg(unix)]
fn extract_root_identity(meta: &Metadata) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    Some(format!("unix:{}:{}", meta.dev(), meta.ino()))
}

#[cfg(windows)]
fn extract_root_identity(meta: &Metadata) -> Option<String> {
    use std::os::windows::fs::MetadataExt;
    Some(format!("win:{}", meta.creation_time()))
}

#[cfg(not(any(unix, windows)))]
fn extract_root_identity(_meta: &Metadata) -> Option<String> {
    None
}

#[cfg(windows)]
fn is_reparse_point(meta: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_meta: &Metadata) -> bool {
    false
}

// -----------------------------------------------------------------------------
// Legacy Directory Reconciliation
// -----------------------------------------------------------------------------

/// 对 `root_dir` 执行全量目录遍历，并与 `lanvault_core::Database` 中的 `local_file_state`
/// 进行 `(size, mtime_ns, file_id)` 快速 Diff 对账。
pub fn reconcile_directory(
    db: &Database,
    rule_id: &str,
    target_vault_id: &str,
    root_dir: &Path,
) -> Result<DiffResult, ClientError> {
    let _probe = lanvault_core::ProbeGuard::start(
        "client.scanner",
        "scanner.reconcile_dir",
        Some(format!("rule_id={rule_id} root={}", root_dir.display())),
    );
    if !root_dir.is_dir() {
        return Err(ClientError::InvalidPath {
            path: root_dir.to_path_buf(),
            reason: "scan target must be an existing directory".to_string(),
        });
    }

    let mut stored_map: HashMap<String, StoredFileState> = db
        .list_local_file_states(rule_id)?
        .into_iter()
        .map(|rec| {
            (
                rec.rel_path,
                StoredFileState {
                    target_vault_id: rec.target_vault_id,
                    size: rec.size,
                    mtime_ns: rec.mtime_ns,
                    file_id: rec.file_id,
                    chunk_hashes: rec.chunk_hashes,
                },
            )
        })
        .collect();

    let rebuild = db
        .get_setting(&checkpoint_key(target_vault_id, rule_id))?
        .is_some_and(|checkpoint| checkpoint.starts_with("rebuild:"));
    let mut diff = DiffResult::default();
    let mut stack = vec![root_dir.to_path_buf()];

    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;

            // 跳过符号链接，防止目录环路。
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() || is_ignored_file(&path) {
                continue;
            }

            let meta = entry.metadata()?;
            let snapshot = match build_snapshot(root_dir, &path, &meta) {
                Ok(snapshot) => snapshot,
                Err(ClientError::InvalidPath { path, reason }) => {
                    warn!(path = %path.display(), "skipping unportable file path: {reason}");
                    continue;
                }
                Err(err) => return Err(err),
            };
            if is_file_still_writing(&meta) {
                if stored_map.remove(&snapshot.rel_path).is_some() {
                    diff.unchanged_count += 1;
                }
                debug!(path = %path.display(), "deferring recently modified file still being written");
                continue;
            }

            match stored_map.remove(&snapshot.rel_path) {
                None => diff.added.push(snapshot),
                Some(prev) => {
                    if rebuild
                        || prev.target_vault_id != target_vault_id
                        || prev.size != snapshot.size
                        || prev.mtime_ns != snapshot.mtime_ns
                        || prev.file_id != snapshot.file_id
                    {
                        diff.modified.push(snapshot);
                    } else {
                        diff.unchanged_count += 1;
                    }
                }
            }
        }
    }

    diff.deleted = stored_map.into_keys().collect();
    diff.deleted.sort();

    debug!(
        rule_id = %rule_id,
        added = diff.added.len(),
        modified = diff.modified.len(),
        deleted = diff.deleted.len(),
        unchanged = diff.unchanged_count,
        "completed metadata diff reconciliation"
    );

    lanvault_core::record_debug(
        "client.scanner",
        Some("scanner.diff_summary"),
        None,
        || {
            format!(
                "rule={rule_id} added={} modified={} deleted={} unchanged={}",
                diff.added.len(),
                diff.modified.len(),
                diff.deleted.len(),
                diff.unchanged_count
            )
        },
        None,
    );
    Ok(diff)
}

pub(crate) fn checkpoint_key(vault_id: &str, rule_id: &str) -> String {
    format!("sync_checkpoint:{}:{vault_id}:{rule_id}", vault_id.len())
}

/// 针对实时监听上报的局部路径构建元数据快照。
pub fn inspect_single_file(
    root_dir: &Path,
    abs_path: &Path,
) -> Result<Option<FileMetadataSnapshot>, ClientError> {
    match fs::symlink_metadata(abs_path) {
        Ok(meta) if meta.is_file() => Ok(Some(build_snapshot(root_dir, abs_path, &meta)?)),
        Ok(_) => Ok(None),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(ClientError::Io(err)),
    }
}

/// 在文件块成功上传后，通过 `lanvault_core::Database` 原子更新 `local_file_state` 记录。
pub fn upsert_file_state(
    db: &Database,
    rule_id: &str,
    target_vault_id: &str,
    snapshot: &FileMetadataSnapshot,
    chunk_hashes_blob: &[u8],
) -> Result<(), ClientError> {
    db.upsert_local_file_state(&LocalFileStateRecord {
        rule_id: rule_id.to_string(),
        target_vault_id: target_vault_id.to_string(),
        rel_path: snapshot.rel_path.clone(),
        size: snapshot.size,
        mtime_ns: snapshot.mtime_ns,
        file_id: snapshot.file_id,
        chunk_hashes: chunk_hashes_blob.to_vec(),
    })?;
    Ok(())
}

/// 删除已不存在文件的本地状态索引（复用 `Database::delete_local_file_states` 事务批量删除）。
pub fn delete_file_states(
    db: &Database,
    rule_id: &str,
    rel_paths: &[String],
) -> Result<usize, ClientError> {
    Ok(db.delete_local_file_states(rule_id, rel_paths)?)
}

fn strip_windows_verbatim(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let raw = path.to_string_lossy();
        if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{unc}"));
        }
        if let Some(stripped) = raw.strip_prefix(r"\\?\") {
            return PathBuf::from(stripped);
        }
    }
    path.to_path_buf()
}

fn build_snapshot(
    root_dir: &Path,
    abs_path: &Path,
    meta: &Metadata,
) -> Result<FileMetadataSnapshot, ClientError> {
    let clean_root = strip_windows_verbatim(root_dir);
    let clean_abs = strip_windows_verbatim(abs_path);
    let rel = clean_abs
        .strip_prefix(&clean_root)
        .map_err(|_| ClientError::InvalidPath {
            path: abs_path.to_path_buf(),
            reason: format!("path is outside watch root {}", root_dir.display()),
        })?;

    let rel_path = normalize_rel_path(rel)?;
    let mtime_ns = extract_mtime_ns(meta)?;

    Ok(FileMetadataSnapshot {
        rel_path,
        abs_path: abs_path.to_path_buf(),
        size: meta.len(),
        mtime_ns,
        file_id: extract_file_id(meta),
    })
}

fn normalize_rel_path(rel: &Path) -> Result<String, ClientError> {
    let mut parts = Vec::new();
    for component in rel.components() {
        let Some(part) = component.as_os_str().to_str() else {
            return Err(ClientError::InvalidPath {
                path: rel.to_path_buf(),
                reason: "file names must be valid UTF-8 for portable snapshots".to_string(),
            });
        };
        parts.push(part);
    }
    Ok(parts.join("/"))
}

#[cfg(unix)]
fn extract_file_id(meta: &Metadata) -> Option<i64> {
    use std::os::unix::fs::MetadataExt;
    Some(meta.ino() as i64)
}

#[cfg(windows)]
fn extract_file_id(meta: &Metadata) -> Option<i64> {
    use std::os::windows::fs::MetadataExt;
    Some(meta.creation_time() as i64)
}

#[cfg(not(any(unix, windows)))]
fn extract_file_id(_meta: &Metadata) -> Option<i64> {
    None
}

/// 过滤临时文件、下载半成品与系统缩略图垃圾文件。
fn is_ignored_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    if lower == ".ds_store"
        || lower == "thumbs.db"
        || lower == "desktop.ini"
        || lower.starts_with("~$")
    {
        return true;
    }
    matches!(
        Path::new(&lower).extension().and_then(|e| e.to_str()),
        Some("tmp" | "part" | "crdownload" | "downloading" | "swp" | "swo")
    )
}

/// 写入防抖：若文件在最近 1.5 秒内刚被修改，暂缓本次扫描读取，等待外部写入稳定。
fn is_file_still_writing(meta: &Metadata) -> bool {
    let Ok(modified) = meta.modified() else {
        return false;
    };
    let Ok(elapsed) = std::time::SystemTime::now().duration_since(modified) else {
        return false;
    };
    elapsed.as_millis() < 1500
}
