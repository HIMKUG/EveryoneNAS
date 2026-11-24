//! Grandfather-Father-Son (GFS) snapshot retention and orphan chunk garbage collection.
//!
//! Retention policy (blueprint §3.1):
//! - Keep all `pinned` snapshots unconditionally
//! - Keep 1 snapshot per day for the last 7 days
//! - Keep 1 snapshot per week for the last 4 weeks
//! - Keep 1 snapshot per month for the last 6 months
//! - Recycle expired mirror archive catalog versions (`expires_at <= now_ts AND recycled_at IS NULL`)
//! - Delete expired snapshots and reclaim chunks with `ref_count == 0`.

use std::collections::HashSet;
use std::sync::Arc;

use lanvault_core::{Database, ProbeGuard, Result, RetentionTier, SnapshotRecord};
use serde::{Deserialize, Serialize};

use crate::vault::VaultStore;

const SECS_PER_DAY: i64 = 86_400;
const SECS_PER_WEEK: i64 = 7 * SECS_PER_DAY;
const SECS_PER_MONTH: i64 = 30 * SECS_PER_DAY;

/// Configurable GFS retention window counts.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct GfsPolicy {
    pub hourly_keep: i64,
    pub daily_keep: i64,
    pub weekly_keep: i64,
    pub monthly_keep: i64,
}

impl Default for GfsPolicy {
    fn default() -> Self {
        Self {
            hourly_keep: 24,
            daily_keep: 7,
            weekly_keep: 4,
            monthly_keep: 6,
        }
    }
}

/// Summary of a completed GC pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcReport {
    pub evaluated_snapshots: u64,
    pub expired_snapshots: u64,
    pub deleted_chunks: u64,
    pub reclaimed_bytes: u64,
}

/// Executes GFS snapshot pruning and `ref_count == 0` chunk reclamation.
pub struct GarbageCollector {
    db: Arc<Database>,
    vault_store: Arc<VaultStore>,
}

impl GarbageCollector {
    pub fn new(db: Arc<Database>, vault_store: Arc<VaultStore>) -> Self {
        Self { db, vault_store }
    }

    /// Evaluates which snapshots are expired under `policy` at timestamp `now_unix_secs`.
    /// Input snapshots may be in any order; newest snapshots in each GFS bucket are retained.
    pub fn select_expired_snapshots(
        snapshots: &[SnapshotRecord],
        now_unix_secs: i64,
        policy: GfsPolicy,
    ) -> Vec<String> {
        let mut expired = Vec::new();
        let mut scopes: std::collections::BTreeMap<(&str, &str), Vec<&SnapshotRecord>> =
            std::collections::BTreeMap::new();
        for snapshot in snapshots {
            scopes
                .entry((snapshot.rule_id.as_str(), snapshot.device_id.as_str()))
                .or_default()
                .push(snapshot);
        }
        for scope in scopes.values_mut() {
            scope.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.created_at));
            let mut seen = HashSet::new();
            for snap in scope {
                if snap.retention_tier == RetentionTier::Pinned {
                    continue;
                }
                let (window_secs, bucket_secs) = match snap.retention_tier {
                    RetentionTier::Hourly => (policy.hourly_keep * 3_600, 3_600),
                    RetentionTier::Daily => (policy.daily_keep * SECS_PER_DAY, SECS_PER_DAY),
                    RetentionTier::Weekly => (policy.weekly_keep * SECS_PER_WEEK, SECS_PER_WEEK),
                    RetentionTier::Monthly => {
                        (policy.monthly_keep * SECS_PER_MONTH, SECS_PER_MONTH)
                    }
                    RetentionTier::Pinned => continue,
                };
                let cutoff = now_unix_secs.saturating_sub(window_secs);
                let bucket = snap.created_at.div_euclid(bucket_secs);
                if snap.created_at < cutoff || !seen.insert((snap.retention_tier.as_str(), bucket))
                {
                    expired.push(snap.snapshot_id.clone());
                }
            }
        }

        expired
    }

    /// Applies GFS retention to `vault_id`, recycles expired mirror archive catalog versions,
    /// and reclaims all orphan chunks (`ref_count == 0`).
    pub fn run_gc(
        &self,
        vault_id: &str,
        now_unix_secs: i64,
        policy: GfsPolicy,
    ) -> Result<GcReport> {
        let snapshots = self.db.list_snapshots(Some(vault_id))?;
        let evaluated_snapshots = snapshots.len() as u64;
        let expired_ids = Self::select_expired_snapshots(&snapshots, now_unix_secs, policy);

        let _probe = ProbeGuard::start(
            "server.gc",
            "server.gc_run",
            Some(format!(
                "vault_id={vault_id}, snapshots={evaluated_snapshots}"
            )),
        );
        let expired_count = self.db.delete_snapshots_batch(&expired_ids)? as u64;
        if !expired_ids.is_empty() {
            if let Ok(roots) = self.vault_store.manifest_backup_roots(vault_id) {
                for root in roots {
                    for snapshot_id in &expired_ids {
                        let path = root.join("manifests").join(format!("{snapshot_id}.json"));
                        if let Err(err) = std::fs::remove_file(&path) {
                            if err.kind() != std::io::ErrorKind::NotFound {
                                tracing::warn!(
                                    path = %path.display(),
                                    "failed to remove expired snapshot manifest: {err}"
                                );
                            }
                        }
                    }
                }
            }
        }

        // Recycle expired mirror archive catalog versions (`expires_at <= now_ts AND recycled_at IS NULL`)
        // before orphan chunk reclamation; diff_backup versions (`expires_at IS NULL`) and
        // `source_deleted = 1` current entries remain untouched.
        let recycled_versions = self
            .db
            .recycle_expired_catalog_versions(now_unix_secs, 500)?;
        if !recycled_versions.is_empty() {
            let total_recycled: usize = recycled_versions.iter().map(|(_, count)| *count).sum();
            tracing::info!(
                vault_id = %vault_id,
                total_recycled,
                "recycled expired mirror archive catalog versions"
            );
        }

        let (deleted_chunks, reclaimed_bytes) = self.vault_store.with_write_lock(|| {
            // 5-minute grace period prevents GC from deleting in-flight chunks before `create_snapshot` commits.
            const ORPHAN_GRACE_SECS: i64 = 300;
            let cutoff = now_unix_secs.saturating_sub(ORPHAN_GRACE_SECS);
            let orphans = self.db.list_orphan_chunks_before(vault_id, cutoff)?;
            let mut removed = Vec::with_capacity(orphans.len());
            for orphan in orphans {
                match self
                    .vault_store
                    .delete_chunk_files(vault_id, &orphan.hash_blake3)
                {
                    Ok(()) => removed.push(orphan.hash_blake3),
                    Err(err) => {
                        tracing::warn!(
                            vault_id = %vault_id,
                            hash = %hex::encode(orphan.hash_blake3),
                            %err,
                            "orphan chunk file deletion failed; index retained for retry"
                        );
                    }
                }
            }
            let (deleted, reclaimed) = self.db.delete_chunks_batch(vault_id, &removed)?;
            Ok((deleted as u64, reclaimed))
        })?;

        self.db.prune_revoked_sessions(now_unix_secs)?;
        // 清理崩溃残留的 staging/*.tmp（超过 1 小时未修改）。
        if let Ok(vols) = self.vault_store.online_volumes(vault_id) {
            for vol in vols {
                let staging_dir = std::path::Path::new(&vol.mount_path).join("staging");
                if let Ok(entries) = std::fs::read_dir(&staging_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().and_then(|e| e.to_str()) == Some("tmp") {
                            let stale = entry
                                .metadata()
                                .ok()
                                .and_then(|m| m.modified().ok())
                                .and_then(|t| std::time::SystemTime::now().duration_since(t).ok())
                                .map(|d| d.as_secs() > 3600)
                                .unwrap_or(false);
                            if stale {
                                let _ = std::fs::remove_file(&path);
                            }
                        }
                    }
                }
            }
        }

        Ok(GcReport {
            evaluated_snapshots,
            expired_snapshots: expired_count,
            deleted_chunks,
            reclaimed_bytes,
        })
    }
}
