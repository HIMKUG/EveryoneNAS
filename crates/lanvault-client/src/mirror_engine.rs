//! 权威源 -> 已授权镜像目录的崩溃安全单向镜像执行引擎。
//!
//! 职责：
//! - `preview_mirror_sync`：扫描真实 `target_root` 捕获外部漂移与碰撞，生成差异预览报告；
//! - `execute_mirror_sync`：基于 `mirror_apply_journal` 的 6 阶段崩溃安全应用
//!   (`planned -> archived -> staged -> applied -> verified -> committed`)，
//!   在覆盖/删除前将目标真实旧文件归档至 `archive_root/<binding_id>/<version_id>` 并记录
//!   `CatalogVersionRecord`，严禁在不完整源扫描或外部编辑冲突时覆盖/删除数据；
//! - `recover_mirror_journal`：崩溃恢复未完成日志项，避免重复归档或丢失唯一副本。

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use lanvault_core::{
    blake3_hash_reader, CatalogVersionRecord, Database, GroupSyncRuleRecord, MirrorJournalRecord,
    MirrorPreviewReport, RuleTargetBindingRecord,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ClientError;
use crate::scanner::{scan_source_generation, ScannedEntry, ScannedEntryKind, SourceGeneration};

/// 同卷暂存目录名（位于 `target_root` 内部，确保 `fs::rename` 原子生效）。
pub const MIRROR_STAGING_DIR: &str = ".lanvault_mirror_staging";

/// 确定性 3 阶段故障注入点（仅用于崩溃恢复测试）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MirrorFaultStage {
    BeforeArchive,
    AfterArchiveBeforeApply,
    AfterApplyBeforeCommit,
}

/// 单次镜像同步执行结果。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MirrorApplyReport {
    pub operation_id: String,
    pub binding_id: String,
    pub added: usize,
    pub overwritten: usize,
    pub removed: usize,
    pub unchanged: usize,
    pub archived_versions: Vec<String>,
    pub conflict_blocked: Vec<String>,
    pub journal_entries: usize,
}

/// 崩溃恢复执行结果。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MirrorRecoveryReport {
    pub binding_id: String,
    pub inspected: usize,
    pub committed: usize,
    pub conflict_blocked: usize,
    pub cleaned_staging: usize,
}

/// 扫描真实 `target_root` 并与 `source_gen` 对比，生成零写入的镜像差异预览报告。
pub fn preview_mirror_sync(
    source_gen: &SourceGeneration,
    target_root: &Path,
    rule_id: &str,
    binding_id: &str,
    target_device_id: &str,
) -> Result<MirrorPreviewReport, ClientError> {
    validate_mirror_target_root(target_root)?;
    let target_gen = scan_source_generation(target_root, &[MIRROR_STAGING_DIR.to_string()])?;

    let mut report = MirrorPreviewReport {
        rule_id: rule_id.to_string(),
        binding_id: binding_id.to_string(),
        target_device_id: target_device_id.to_string(),
        add_count: 0,
        overwrite_count: 0,
        remove_count: 0,
        unchanged_count: 0,
        blocked_count: 0,
        conflicts: Vec::new(),
    };

    for c in &source_gen.collisions {
        report.blocked_count += 1;
        report
            .conflicts
            .push(format!("source collision on `{}`: {}", c.path, c.detail));
    }
    for c in &target_gen.collisions {
        report.blocked_count += 1;
        report
            .conflicts
            .push(format!("target collision on `{}`: {}", c.path, c.detail));
    }
    for reason in &source_gen.blocked_reasons {
        report.blocked_count += 1;
        report.conflicts.push(format!("source blocked: {reason}"));
    }
    for reason in &target_gen.blocked_reasons {
        report.blocked_count += 1;
        report.conflicts.push(format!("target blocked: {reason}"));
    }

    let (mut i, mut j) = (0usize, 0usize);
    let src = &source_gen.entries;
    let tgt = &target_gen.entries;

    while i < src.len() && j < tgt.len() {
        match src[i].path_key.cmp(&tgt[j].path_key) {
            Ordering::Less => {
                report.add_count += 1;
                i += 1;
            }
            Ordering::Greater => {
                if source_gen.complete {
                    report.remove_count += 1;
                } else {
                    report.blocked_count += 1;
                    report.conflicts.push(format!(
                        "removal of `{}` blocked because source scan is incomplete",
                        tgt[j].path_key
                    ));
                }
                j += 1;
            }
            Ordering::Equal => {
                if src[i].kind != tgt[j].kind {
                    report.overwrite_count += 1;
                    report.conflicts.push(format!(
                        "type conflict on `{}`: source is {:?}, target is {:?}",
                        src[i].path_key, src[i].kind, tgt[j].kind
                    ));
                } else if entries_identical(&src[i], &tgt[j]) {
                    report.unchanged_count += 1;
                } else {
                    report.overwrite_count += 1;
                }
                i += 1;
                j += 1;
            }
        }
    }

    while i < src.len() {
        report.add_count += 1;
        i += 1;
    }
    while j < tgt.len() {
        if source_gen.complete {
            report.remove_count += 1;
        } else {
            report.blocked_count += 1;
            report.conflicts.push(format!(
                "removal of `{}` blocked because source scan is incomplete",
                tgt[j].path_key
            ));
        }
        j += 1;
    }

    Ok(report)
}

#[derive(Debug, Clone)]
enum PlannedMirrorOp {
    RemoveFile {
        path_key: String,
        target_entry: ScannedEntry,
    },
    RemoveDir {
        path_key: String,
    },
    CreateDir {
        path_key: String,
    },
    WriteFile {
        source_entry: ScannedEntry,
        existing_target: Option<ScannedEntry>,
    },
}

/// 执行权威源 -> 目标镜像目录的崩溃安全镜像同步。
///
/// 严格执行目录依赖顺序与 6 阶段日志：
/// `planned -> archived -> staged -> applied -> verified -> committed`。
#[allow(clippy::too_many_arguments)]
pub fn execute_mirror_sync(
    db: &Database,
    source_gen: &SourceGeneration,
    target_root: &Path,
    archive_root: &Path,
    rule: &GroupSyncRuleRecord,
    binding: &RuleTargetBindingRecord,
    operation_id: &str,
    fault_inject_stage: Option<MirrorFaultStage>,
) -> Result<MirrorApplyReport, ClientError> {
    validate_mirror_target_root(target_root)?;

    if !source_gen.complete {
        return Err(ClientError::InvalidInput(format!(
            "refusing mirror sync for binding `{}`: source scan is incomplete ({} blocked, {} volatile)",
            binding.binding_id,
            source_gen.blocked_reasons.len(),
            source_gen.volatile_paths.len()
        )));
    }
    if !source_gen.collisions.is_empty() {
        return Err(ClientError::InvalidInput(format!(
            "refusing mirror sync for binding `{}`: {} path collision(s) in source",
            binding.binding_id,
            source_gen.collisions.len()
        )));
    }

    let target_gen = scan_source_generation(target_root, &[MIRROR_STAGING_DIR.to_string()])?;
    if !target_gen.complete {
        return Err(ClientError::InvalidInput(format!(
            "refusing mirror sync for binding `{}`: target scan is incomplete",
            binding.binding_id
        )));
    }

    // Load known catalog baseline hashes (if any) to detect out-of-band external edits on target.
    let baseline_hashes = load_catalog_baseline_hashes(db, &binding.binding_id)?;

    let mut report = MirrorApplyReport {
        operation_id: operation_id.to_string(),
        binding_id: binding.binding_id.clone(),
        ..Default::default()
    };

    let mut file_removals: Vec<PlannedMirrorOp> = Vec::new();
    let mut dir_removals: Vec<PlannedMirrorOp> = Vec::new();
    let mut dir_creates: Vec<PlannedMirrorOp> = Vec::new();
    let mut file_writes: Vec<PlannedMirrorOp> = Vec::new();

    let (mut i, mut j) = (0usize, 0usize);
    let src = &source_gen.entries;
    let tgt = &target_gen.entries;

    while i < src.len() && j < tgt.len() {
        match src[i].path_key.cmp(&tgt[j].path_key) {
            Ordering::Less => {
                match src[i].kind {
                    ScannedEntryKind::Dir => dir_creates.push(PlannedMirrorOp::CreateDir {
                        path_key: src[i].path_key.clone(),
                    }),
                    ScannedEntryKind::File => file_writes.push(PlannedMirrorOp::WriteFile {
                        source_entry: src[i].clone(),
                        existing_target: None,
                    }),
                }
                i += 1;
            }
            Ordering::Greater => {
                match tgt[j].kind {
                    ScannedEntryKind::File => file_removals.push(PlannedMirrorOp::RemoveFile {
                        path_key: tgt[j].path_key.clone(),
                        target_entry: tgt[j].clone(),
                    }),
                    ScannedEntryKind::Dir => dir_removals.push(PlannedMirrorOp::RemoveDir {
                        path_key: tgt[j].path_key.clone(),
                    }),
                }
                j += 1;
            }
            Ordering::Equal => {
                if src[i].kind == tgt[j].kind && entries_identical(&src[i], &tgt[j]) {
                    report.unchanged += 1;
                } else {
                    match (src[i].kind, tgt[j].kind) {
                        (ScannedEntryKind::File, ScannedEntryKind::File) => {
                            file_writes.push(PlannedMirrorOp::WriteFile {
                                source_entry: src[i].clone(),
                                existing_target: Some(tgt[j].clone()),
                            });
                        }
                        (ScannedEntryKind::File, ScannedEntryKind::Dir) => {
                            // Blocking dir will have its children removed first, then dir removed, then file written.
                            dir_removals.push(PlannedMirrorOp::RemoveDir {
                                path_key: tgt[j].path_key.clone(),
                            });
                            file_writes.push(PlannedMirrorOp::WriteFile {
                                source_entry: src[i].clone(),
                                existing_target: None,
                            });
                        }
                        (ScannedEntryKind::Dir, ScannedEntryKind::File) => {
                            // Remove blocking file (with archival) before creating directory.
                            file_removals.push(PlannedMirrorOp::RemoveFile {
                                path_key: tgt[j].path_key.clone(),
                                target_entry: tgt[j].clone(),
                            });
                            dir_creates.push(PlannedMirrorOp::CreateDir {
                                path_key: src[i].path_key.clone(),
                            });
                        }
                        (ScannedEntryKind::Dir, ScannedEntryKind::Dir) => {
                            report.unchanged += 1;
                        }
                    }
                }
                i += 1;
                j += 1;
            }
        }
    }

    while i < src.len() {
        match src[i].kind {
            ScannedEntryKind::Dir => dir_creates.push(PlannedMirrorOp::CreateDir {
                path_key: src[i].path_key.clone(),
            }),
            ScannedEntryKind::File => file_writes.push(PlannedMirrorOp::WriteFile {
                source_entry: src[i].clone(),
                existing_target: None,
            }),
        }
        i += 1;
    }
    while j < tgt.len() {
        match tgt[j].kind {
            ScannedEntryKind::File => file_removals.push(PlannedMirrorOp::RemoveFile {
                path_key: tgt[j].path_key.clone(),
                target_entry: tgt[j].clone(),
            }),
            ScannedEntryKind::Dir => dir_removals.push(PlannedMirrorOp::RemoveDir {
                path_key: tgt[j].path_key.clone(),
            }),
        }
        j += 1;
    }

    // Strict directory dependency order:
    // 1. Remove extra/blocking files (deepest first).
    // 2. Remove empty blocking dirs that conflict with incoming files (deepest first).
    // 3. Create parent/target directories (shallowest first).
    // 4. Write/overwrite files (journaled: planned -> archived -> staged -> applied -> verified -> committed).
    // 5. Remove remaining extra empty directories (deepest first).
    file_removals.sort_by(|a, b| path_depth_desc(op_path(a), op_path(b)));
    dir_removals.sort_by(|a, b| path_depth_desc(op_path(a), op_path(b)));
    dir_creates.sort_by(|a, b| path_depth_asc(op_path(a), op_path(b)));

    let staging_dir = target_root.join(MIRROR_STAGING_DIR);
    let vault_id = binding
        .target_vault_id
        .as_deref()
        .unwrap_or(&binding.binding_id);
    let retention_secs = i64::from(rule.archive_retention_days) * 86_400;

    // Step 1: Execute journaled file removals.
    for op in file_removals {
        if let PlannedMirrorOp::RemoveFile {
            path_key,
            target_entry,
        } = op
        {
            apply_journaled_file_remove(
                db,
                target_root,
                archive_root,
                rule,
                binding,
                vault_id,
                retention_secs,
                operation_id,
                &path_key,
                &target_entry,
                baseline_hashes.get(&path_key).map(Vec::as_slice),
                fault_inject_stage,
                &mut report,
            )?;
        }
    }

    // Step 2: Remove blocking dirs that are now empty (e.g., dir replaced by file).
    let mut deferred_dir_removals = Vec::new();
    for op in dir_removals {
        if let PlannedMirrorOp::RemoveDir { path_key } = op {
            let abs_dir = target_root.join(&path_key);
            if abs_dir.is_dir() && fs::remove_dir(&abs_dir).is_ok() {
                report.removed += 1;
            } else if abs_dir.exists() {
                deferred_dir_removals.push(path_key);
            }
        }
    }

    // Step 3: Create directories shallowest-first.
    for op in dir_creates {
        if let PlannedMirrorOp::CreateDir { path_key } = op {
            let abs_dir = target_root.join(&path_key);
            if !abs_dir.exists() {
                fs::create_dir_all(&abs_dir)?;
                report.added += 1;
            }
        }
    }

    // Step 4: Execute journaled file additions & overwrites.
    for op in file_writes {
        if let PlannedMirrorOp::WriteFile {
            source_entry,
            existing_target,
        } = op
        {
            apply_journaled_file_write(
                db,
                target_root,
                &staging_dir,
                archive_root,
                rule,
                binding,
                vault_id,
                retention_secs,
                operation_id,
                &source_entry,
                existing_target.as_ref(),
                baseline_hashes
                    .get(&source_entry.path_key)
                    .map(Vec::as_slice),
                fault_inject_stage,
                &mut report,
            )?;
        }
    }

    // Step 5: Remove any remaining extra empty directories deepest-first.
    deferred_dir_removals.sort_by(|a, b| path_depth_desc(a, b));
    for path_key in deferred_dir_removals {
        let abs_dir = target_root.join(&path_key);
        if abs_dir.is_dir() && fs::remove_dir(&abs_dir).is_ok() {
            report.removed += 1;
        }
    }

    // Best-effort cleanup of empty staging dir.
    let _ = fs::remove_dir(&staging_dir);

    Ok(report)
}

#[allow(clippy::too_many_arguments)]
fn apply_journaled_file_remove(
    db: &Database,
    target_root: &Path,
    archive_root: &Path,
    rule: &GroupSyncRuleRecord,
    binding: &RuleTargetBindingRecord,
    vault_id: &str,
    retention_secs: i64,
    operation_id: &str,
    path_key: &str,
    target_entry: &ScannedEntry,
    baseline_hash: Option<&[u8]>,
    fault_inject_stage: Option<MirrorFaultStage>,
    report: &mut MirrorApplyReport,
) -> Result<(), ClientError> {
    let now = unix_now_secs();
    let journal_id = Uuid::new_v4().to_string();
    let expected_prev = target_entry.content_hash.map(|h| h.to_vec());

    let mut journal = MirrorJournalRecord {
        journal_id: journal_id.clone(),
        operation_id: operation_id.to_string(),
        rule_id: rule.rule_id.clone(),
        binding_id: binding.binding_id.clone(),
        path_key: path_key.to_string(),
        source_gen: binding.last_source_gen + 1,
        base_revision: binding.applied_revision,
        expected_prev_hash: expected_prev.clone(),
        archive_version_id: None,
        staging_path: None,
        phase: "planned".to_string(),
        error_message: None,
        created_at: now,
        updated_at: now,
    };
    db.upsert_mirror_journal(&journal)?;
    report.journal_entries += 1;

    if fault_inject_stage == Some(MirrorFaultStage::BeforeArchive) {
        return Err(ClientError::InvalidInput(format!(
            "fault injected at BeforeArchive for `{path_key}`"
        )));
    }

    let target_path = target_root.join(path_key);
    let live_hash = hash_file_strict(&target_path)?;

    // Detect external modification either relative to catalog baseline or since scan.
    if let Some(base) = baseline_hash {
        if live_hash.as_slice() != base {
            let msg = format!("external edit detected on `{path_key}` before removal");
            db.update_mirror_journal_phase(&journal_id, "conflict_blocked", Some(&msg))?;
            report.conflict_blocked.push(path_key.to_string());
            return Ok(());
        }
    }
    if let Some(ref exp) = expected_prev {
        if live_hash.as_slice() != exp.as_slice() {
            let msg = format!("target `{path_key}` modified since scan before removal");
            db.update_mirror_journal_phase(&journal_id, "conflict_blocked", Some(&msg))?;
            report.conflict_blocked.push(path_key.to_string());
            return Ok(());
        }
    }

    // Phase: archived
    let version_id = archive_target_file(
        db,
        archive_root,
        binding,
        vault_id,
        operation_id,
        path_key,
        &target_path,
        &live_hash,
        "mirror_removed",
        retention_secs,
    )?;
    journal.archive_version_id = Some(version_id.clone());
    journal.phase = "archived".to_string();
    journal.updated_at = unix_now_secs();
    db.upsert_mirror_journal(&journal)?;
    report.archived_versions.push(version_id);

    if fault_inject_stage == Some(MirrorFaultStage::AfterArchiveBeforeApply) {
        return Err(ClientError::InvalidInput(format!(
            "fault injected at AfterArchiveBeforeApply for `{path_key}`"
        )));
    }

    // Phase: applied
    fs::remove_file(&target_path)?;
    db.update_mirror_journal_phase(&journal_id, "applied", None)?;

    if fault_inject_stage == Some(MirrorFaultStage::AfterApplyBeforeCommit) {
        return Err(ClientError::InvalidInput(format!(
            "fault injected at AfterApplyBeforeCommit for `{path_key}`"
        )));
    }

    // Phase: verified
    if target_path.exists() {
        return Err(ClientError::Io(std::io::Error::other(format!(
            "mirror removal verification failed: `{path_key}` still exists"
        ))));
    }
    db.update_mirror_journal_phase(&journal_id, "verified", None)?;

    // Phase: committed
    db.update_mirror_journal_phase(&journal_id, "committed", None)?;
    report.removed += 1;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn apply_journaled_file_write(
    db: &Database,
    target_root: &Path,
    staging_dir: &Path,
    archive_root: &Path,
    rule: &GroupSyncRuleRecord,
    binding: &RuleTargetBindingRecord,
    vault_id: &str,
    retention_secs: i64,
    operation_id: &str,
    source_entry: &ScannedEntry,
    existing_target: Option<&ScannedEntry>,
    baseline_hash: Option<&[u8]>,
    fault_inject_stage: Option<MirrorFaultStage>,
    report: &mut MirrorApplyReport,
) -> Result<(), ClientError> {
    let source_hash = source_entry.content_hash.ok_or_else(|| {
        ClientError::InvalidInput(format!(
            "source file `{}` is missing BLAKE3 content_hash",
            source_entry.path_key
        ))
    })?;
    let path_key = &source_entry.path_key;
    let now = unix_now_secs();
    let journal_id = Uuid::new_v4().to_string();
    let expected_prev = existing_target.and_then(|t| t.content_hash.map(|h| h.to_vec()));

    let mut journal = MirrorJournalRecord {
        journal_id: journal_id.clone(),
        operation_id: operation_id.to_string(),
        rule_id: rule.rule_id.clone(),
        binding_id: binding.binding_id.clone(),
        path_key: path_key.clone(),
        source_gen: binding.last_source_gen + 1,
        base_revision: binding.applied_revision,
        expected_prev_hash: expected_prev.clone(),
        archive_version_id: None,
        staging_path: None,
        phase: "planned".to_string(),
        error_message: None,
        created_at: now,
        updated_at: now,
    };
    db.upsert_mirror_journal(&journal)?;
    report.journal_entries += 1;

    if fault_inject_stage == Some(MirrorFaultStage::BeforeArchive) {
        return Err(ClientError::InvalidInput(format!(
            "fault injected at BeforeArchive for `{path_key}`"
        )));
    }

    let target_path = target_root.join(path_key);
    let is_overwrite = existing_target.is_some();

    if is_overwrite {
        let live_hash = hash_file_strict(&target_path)?;
        if let Some(base) = baseline_hash {
            if live_hash.as_slice() != base {
                let msg = format!("external edit detected on `{path_key}` before overwrite");
                db.update_mirror_journal_phase(&journal_id, "conflict_blocked", Some(&msg))?;
                report.conflict_blocked.push(path_key.clone());
                return Ok(());
            }
        }
        if let Some(ref exp) = expected_prev {
            if live_hash.as_slice() != exp.as_slice() {
                let msg = format!("target `{path_key}` modified since scan before overwrite");
                db.update_mirror_journal_phase(&journal_id, "conflict_blocked", Some(&msg))?;
                report.conflict_blocked.push(path_key.clone());
                return Ok(());
            }
        }

        // Phase: archived
        let version_id = archive_target_file(
            db,
            archive_root,
            binding,
            vault_id,
            operation_id,
            path_key,
            &target_path,
            &live_hash,
            "mirror_overwritten",
            retention_secs,
        )?;
        journal.archive_version_id = Some(version_id.clone());
        journal.phase = "archived".to_string();
        journal.updated_at = unix_now_secs();
        db.upsert_mirror_journal(&journal)?;
        report.archived_versions.push(version_id);
    } else if target_path.exists() {
        // No-clobber guard: target appeared externally after scan.
        let msg = format!("target `{path_key}` appeared externally since scan");
        db.update_mirror_journal_phase(&journal_id, "conflict_blocked", Some(&msg))?;
        report.conflict_blocked.push(path_key.clone());
        return Ok(());
    }

    if fault_inject_stage == Some(MirrorFaultStage::AfterArchiveBeforeApply) {
        return Err(ClientError::InvalidInput(format!(
            "fault injected at AfterArchiveBeforeApply for `{path_key}`"
        )));
    }

    // Phase: staged (same-volume staging + BLAKE3 verification)
    fs::create_dir_all(staging_dir)?;
    let staging_path = staging_dir.join(format!("{journal_id}.tmp"));
    let staged_hash = copy_and_hash(&source_entry.abs_path, &staging_path)?;
    if staged_hash != source_hash {
        let _ = fs::remove_file(&staging_path);
        return Err(ClientError::InvalidInput(format!(
            "staging BLAKE3 mismatch for `{path_key}` (source changed mid-copy)"
        )));
    }
    journal.staging_path = Some(staging_path.to_string_lossy().into_owned());
    journal.phase = "staged".to_string();
    journal.updated_at = unix_now_secs();
    db.upsert_mirror_journal(&journal)?;

    // Phase: applied (create parent dirs, replace hardlinks via atomic rename rather than in-place write)
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)?;
    }
    replace_file_atomically(&staging_path, &target_path, is_overwrite)?;
    db.update_mirror_journal_phase(&journal_id, "applied", None)?;

    if fault_inject_stage == Some(MirrorFaultStage::AfterApplyBeforeCommit) {
        return Err(ClientError::InvalidInput(format!(
            "fault injected at AfterApplyBeforeCommit for `{path_key}`"
        )));
    }

    // Phase: verified (re-hash final target file)
    let final_hash = hash_file_strict(&target_path)?;
    if final_hash != source_hash {
        return Err(ClientError::Io(std::io::Error::other(format!(
            "mirror post-apply BLAKE3 verification failed for `{path_key}`"
        ))));
    }
    db.update_mirror_journal_phase(&journal_id, "verified", None)?;

    // Phase: committed
    db.update_mirror_journal_phase(&journal_id, "committed", None)?;
    if is_overwrite {
        report.overwritten += 1;
    } else {
        report.added += 1;
    }
    Ok(())
}

/// 扫描并安全恢复指定绑定的未完成 `mirror_apply_journal` 日志项：
/// - 绝不重复归档；
/// - 绝不丢失唯一副本；
/// - 发现目标文件被外部修改时标记为 `conflict_blocked`。
pub fn recover_mirror_journal(
    db: &Database,
    target_root: &Path,
    archive_root: &Path,
    binding_id: &str,
) -> Result<MirrorRecoveryReport, ClientError> {
    validate_mirror_target_root(target_root)?;

    let unfinished = db.list_unfinished_mirror_journals(Some(binding_id))?;
    let mut report = MirrorRecoveryReport {
        binding_id: binding_id.to_string(),
        inspected: unfinished.len(),
        ..Default::default()
    };

    for entry in unfinished {
        let target_path = target_root.join(&entry.path_key);
        let staging_path = entry.staging_path.as_deref().map(PathBuf::from);

        match entry.phase.as_str() {
            "conflict_blocked" => {
                report.conflict_blocked += 1;
            }
            "planned" => {
                // Crashed before archive/staging: verify disk was not externally altered, then mark committed so next sync re-plans cleanly.
                if let Some(ref exp) = entry.expected_prev_hash {
                    if target_path.is_file() {
                        let live_hash = hash_file_strict(&target_path)?;
                        if live_hash.as_slice() != exp.as_slice() {
                            let msg = format!(
                                "recovery conflict on `{}`: target modified externally after crash in planned phase",
                                entry.path_key
                            );
                            db.update_mirror_journal_phase(
                                &entry.journal_id,
                                "conflict_blocked",
                                Some(&msg),
                            )?;
                            report.conflict_blocked += 1;
                            continue;
                        }
                    }
                }
                if let Some(ref sp) = staging_path {
                    if sp.exists() && fs::remove_file(sp).is_ok() {
                        report.cleaned_staging += 1;
                    }
                }
                db.update_mirror_journal_phase(&entry.journal_id, "committed", None)?;
                report.committed += 1;
            }
            "archived" => {
                // Archive already exists; do NOT double-archive. Verify archive integrity and check for external target edits.
                if let Some(ref vid) = entry.archive_version_id {
                    let archive_file = archive_root.join(binding_id).join(vid);
                    if !archive_file.is_file() {
                        let msg = format!(
                            "recovery conflict on `{}`: archive version `{vid}` missing on disk",
                            entry.path_key
                        );
                        db.update_mirror_journal_phase(
                            &entry.journal_id,
                            "conflict_blocked",
                            Some(&msg),
                        )?;
                        report.conflict_blocked += 1;
                        continue;
                    }
                }
                if let Some(ref exp) = entry.expected_prev_hash {
                    if target_path.is_file() {
                        let live_hash = hash_file_strict(&target_path)?;
                        if live_hash.as_slice() != exp.as_slice() {
                            let msg = format!(
                                "recovery conflict on `{}`: target modified externally after archive",
                                entry.path_key
                            );
                            db.update_mirror_journal_phase(
                                &entry.journal_id,
                                "conflict_blocked",
                                Some(&msg),
                            )?;
                            report.conflict_blocked += 1;
                            continue;
                        }
                    }
                }
                if let Some(ref sp) = staging_path {
                    if sp.exists() && fs::remove_file(sp).is_ok() {
                        report.cleaned_staging += 1;
                    }
                }
                // Safe to close out journal without double-archiving; next sync converges remaining diff.
                db.update_mirror_journal_phase(&entry.journal_id, "committed", None)?;
                report.committed += 1;
            }
            "staged" => {
                let Some(ref sp) = staging_path else {
                    let msg = format!(
                        "recovery conflict on `{}`: staged phase missing staging_path",
                        entry.path_key
                    );
                    db.update_mirror_journal_phase(
                        &entry.journal_id,
                        "conflict_blocked",
                        Some(&msg),
                    )?;
                    report.conflict_blocked += 1;
                    continue;
                };
                if !sp.is_file() {
                    // Staging already moved or vanished; verify target exists before committing.
                    if target_path.is_file() {
                        db.update_mirror_journal_phase(&entry.journal_id, "committed", None)?;
                        report.committed += 1;
                    } else {
                        let msg = format!(
                            "recovery conflict on `{}`: both staging file and target file missing",
                            entry.path_key
                        );
                        db.update_mirror_journal_phase(
                            &entry.journal_id,
                            "conflict_blocked",
                            Some(&msg),
                        )?;
                        report.conflict_blocked += 1;
                    }
                    continue;
                }
                if let Some(ref exp) = entry.expected_prev_hash {
                    if target_path.is_file() {
                        let live_hash = hash_file_strict(&target_path)?;
                        if live_hash.as_slice() != exp.as_slice() {
                            let msg = format!(
                                "recovery conflict on `{}`: target modified externally while staged",
                                entry.path_key
                            );
                            db.update_mirror_journal_phase(
                                &entry.journal_id,
                                "conflict_blocked",
                                Some(&msg),
                            )?;
                            report.conflict_blocked += 1;
                            continue;
                        }
                    }
                }
                let expected_new_hash = hash_file_strict(sp)?;
                if let Some(parent) = target_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                replace_file_atomically(sp, &target_path, entry.expected_prev_hash.is_some())?;
                let actual_hash = hash_file_strict(&target_path)?;
                if actual_hash != expected_new_hash {
                    let msg = format!(
                        "recovery verification hash mismatch on `{}`",
                        entry.path_key
                    );
                    db.update_mirror_journal_phase(
                        &entry.journal_id,
                        "conflict_blocked",
                        Some(&msg),
                    )?;
                    report.conflict_blocked += 1;
                    continue;
                }
                db.update_mirror_journal_phase(&entry.journal_id, "committed", None)?;
                report.committed += 1;
            }
            "applied" | "verified" => {
                if let Some(ref sp) = staging_path {
                    if sp.exists() && fs::remove_file(sp).is_ok() {
                        report.cleaned_staging += 1;
                    }
                }
                let was_write = entry.staging_path.is_some();
                if was_write && !target_path.is_file() {
                    let msg = format!(
                        "recovery conflict on `{}`: applied write missing final target file",
                        entry.path_key
                    );
                    db.update_mirror_journal_phase(
                        &entry.journal_id,
                        "conflict_blocked",
                        Some(&msg),
                    )?;
                    report.conflict_blocked += 1;
                    continue;
                }
                db.update_mirror_journal_phase(&entry.journal_id, "committed", None)?;
                report.committed += 1;
            }
            other => {
                let msg = format!("unknown mirror journal phase `{other}`");
                db.update_mirror_journal_phase(&entry.journal_id, "conflict_blocked", Some(&msg))?;
                report.conflict_blocked += 1;
            }
        }
    }

    Ok(report)
}

#[allow(clippy::too_many_arguments)]
fn archive_target_file(
    db: &Database,
    archive_root: &Path,
    binding: &RuleTargetBindingRecord,
    vault_id: &str,
    operation_id: &str,
    path_key: &str,
    target_path: &Path,
    expected_hash: &[u8; 32],
    archive_reason: &str,
    retention_secs: i64,
) -> Result<String, ClientError> {
    let version_id = Uuid::new_v4().to_string();
    let binding_archive_dir = archive_root.join(&binding.binding_id);
    fs::create_dir_all(&binding_archive_dir)?;

    let archive_file = binding_archive_dir.join(&version_id);
    let archive_hash = copy_and_hash(target_path, &archive_file)?;
    if archive_hash != *expected_hash {
        let _ = fs::remove_file(&archive_file);
        return Err(ClientError::Io(std::io::Error::other(format!(
            "archive BLAKE3 verification failed for `{path_key}`"
        ))));
    }

    let meta = fs::symlink_metadata(target_path)?;
    let mtime_ns = meta
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    let now = unix_now_secs();

    let version_record = CatalogVersionRecord {
        version_id: version_id.clone(),
        binding_id: binding.binding_id.clone(),
        vault_id: vault_id.to_string(),
        path_key: path_key.to_string(),
        display_path: path_key.to_string(),
        entry_kind: "file".to_string(),
        size: meta.len(),
        mtime_ns,
        content_hash: Some(archive_hash.to_vec()),
        chunk_hashes_json: "[]".to_string(),
        archive_reason: archive_reason.to_string(),
        prev_state: "present".to_string(),
        operation_id: operation_id.to_string(),
        archived_at: now,
        expires_at: Some(now + retention_secs),
        recycled_at: None,
    };
    db.insert_catalog_version(&version_record)?;
    Ok(version_id)
}

fn validate_mirror_target_root(target_root: &Path) -> Result<(), ClientError> {
    let meta = fs::symlink_metadata(target_root).map_err(|err| ClientError::InvalidPath {
        path: target_root.to_path_buf(),
        reason: format!("cannot stat mirror target_root: {err}"),
    })?;
    if meta.file_type().is_symlink() || is_reparse_point(&meta) {
        return Err(ClientError::InvalidPath {
            path: target_root.to_path_buf(),
            reason: "mirror target_root must not be a symlink or reparse point".to_string(),
        });
    }
    if !meta.is_dir() {
        return Err(ClientError::InvalidPath {
            path: target_root.to_path_buf(),
            reason: "mirror target_root must be an existing directory".to_string(),
        });
    }
    Ok(())
}

fn load_catalog_baseline_hashes(
    db: &Database,
    binding_id: &str,
) -> Result<HashMap<String, Vec<u8>>, ClientError> {
    let mut map = HashMap::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = db.list_catalog_entries_page(binding_id, cursor.as_deref(), 1000, false)?;
        let len = page.len();
        if let Some(last) = page.last() {
            cursor = Some(last.path_key.clone());
        }
        for entry in page {
            if let Some(hash) = entry.content_hash {
                map.insert(entry.path_key, hash);
            }
        }
        if len < 1000 {
            break;
        }
    }
    Ok(map)
}

fn entries_identical(a: &ScannedEntry, b: &ScannedEntry) -> bool {
    if a.kind != b.kind {
        return false;
    }
    match a.kind {
        ScannedEntryKind::Dir => true,
        ScannedEntryKind::File => a.content_hash.is_some() && a.content_hash == b.content_hash,
    }
}

fn hash_file_strict(path: &Path) -> Result<[u8; 32], ClientError> {
    let file = File::open(path)?;
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    Ok(blake3_hash_reader(&mut reader)?)
}

fn copy_and_hash(src: &Path, dst: &Path) -> Result<[u8; 32], ClientError> {
    let mut reader = BufReader::with_capacity(256 * 1024, File::open(src)?);
    let mut writer = OpenOptions::new().write(true).create_new(true).open(dst)?;
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
    }
    writer.sync_all()?;
    drop(writer);
    // Re-read written file from disk to verify actual persisted bytes.
    hash_file_strict(dst)
}

fn replace_file_atomically(
    staging_path: &Path,
    target_path: &Path,
    allow_overwrite: bool,
) -> Result<(), ClientError> {
    if !allow_overwrite && target_path.exists() {
        return Err(ClientError::InvalidInput(format!(
            "refusing to clobber existing file `{}`",
            target_path.display()
        )));
    }
    // Always replace via rename (never in-place write), so hardlinks are cleanly broken.
    #[cfg(windows)]
    if allow_overwrite && target_path.exists() {
        fs::remove_file(target_path)?;
    }
    fs::rename(staging_path, target_path)?;
    Ok(())
}

fn op_path(op: &PlannedMirrorOp) -> &str {
    match op {
        PlannedMirrorOp::RemoveFile { path_key, .. } => path_key,
        PlannedMirrorOp::RemoveDir { path_key } => path_key,
        PlannedMirrorOp::CreateDir { path_key } => path_key,
        PlannedMirrorOp::WriteFile { source_entry, .. } => &source_entry.path_key,
    }
}

fn path_depth_desc(a: &str, b: &str) -> Ordering {
    let da = a.bytes().filter(|&c| c == b'/').count();
    let db = b.bytes().filter(|&c| c == b'/').count();
    db.cmp(&da).then_with(|| b.cmp(a))
}

fn path_depth_asc(a: &str, b: &str) -> Ordering {
    let da = a.bytes().filter(|&c| c == b'/').count();
    let db = b.bytes().filter(|&c| c == b'/').count();
    da.cmp(&db).then_with(|| a.cmp(b))
}

fn unix_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(windows)]
fn is_reparse_point(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_meta: &fs::Metadata) -> bool {
    false
}
