//! 本地目录遍历与 SQLite `local_file_state(size, mtime_ns, file_id)` 快速元数据 Diff 对账。
//!
//! 职责：在重连成功、定时巡检或 `watcher.rs` 捕获队列溢出时，100% 捕获离线期间的老文件修改、
//! 新文件创建与已删除文件。所有持久化读写统一委托给 `lanvault_core::Database`。

use std::collections::HashMap;
use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use lanvault_core::{Database, LocalFileStateRecord};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::error::ClientError;

/// 本地文件元数据快照。
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

/// 数据库中存储的本地文件状态记录。
#[derive(Debug, Clone)]
pub struct StoredFileState {
    pub size: u64,
    pub mtime_ns: i64,
    pub file_id: Option<i64>,
    pub chunk_hashes: Vec<u8>,
}

/// 目录元数据快速 Diff 对账结果。
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

/// 对 `root_dir` 执行全量目录遍历，并与 `lanvault_core::Database` 中的 `local_file_state`
/// 进行 `(size, mtime_ns, file_id)` 快速 Diff 对账。
pub fn reconcile_directory(
    db: &Database,
    rule_id: &str,
    root_dir: &Path,
) -> Result<DiffResult, ClientError> {
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
                    size: rec.size,
                    mtime_ns: rec.mtime_ns,
                    file_id: rec.file_id,
                    chunk_hashes: rec.chunk_hashes,
                },
            )
        })
        .collect();

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
            if !file_type.is_file() {
                continue;
            }

            let meta = entry.metadata()?;
            let snapshot = build_snapshot(root_dir, &path, &meta)?;

            match stored_map.remove(&snapshot.rel_path) {
                None => diff.added.push(snapshot),
                Some(prev) => {
                    if prev.size != snapshot.size
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

    Ok(diff)
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
    snapshot: &FileMetadataSnapshot,
    chunk_hashes_blob: &[u8],
) -> Result<(), ClientError> {
    db.upsert_local_file_state(&LocalFileStateRecord {
        rule_id: rule_id.to_string(),
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

fn build_snapshot(
    root_dir: &Path,
    abs_path: &Path,
    meta: &Metadata,
) -> Result<FileMetadataSnapshot, ClientError> {
    let rel = abs_path
        .strip_prefix(root_dir)
        .map_err(|_| ClientError::InvalidPath {
            path: abs_path.to_path_buf(),
            reason: format!("path is outside watch root {}", root_dir.display()),
        })?;

    let rel_path = normalize_rel_path(rel);
    let mtime_ns = meta
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);

    Ok(FileMetadataSnapshot {
        rel_path,
        abs_path: abs_path.to_path_buf(),
        size: meta.len(),
        mtime_ns,
        file_id: extract_file_id(meta),
    })
}

fn normalize_rel_path(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
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
