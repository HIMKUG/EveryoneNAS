//! Grandfather-Father-Son (GFS) snapshot retention and orphan chunk garbage collection.
//!
//! Retention policy (blueprint §3.1):
//! - Keep all `pinned` snapshots unconditionally
//! - Keep 1 snapshot per day for the last 7 days
//! - Keep 1 snapshot per week for the last 4 weeks
//! - Keep 1 snapshot per month for the last 6 months
//! - Delete expired snapshots and reclaim chunks with `ref_count == 0`.

use std::collections::HashSet;
use std::sync::Arc;

use lanvault_core::{ProbeGuard, Database, Result, RetentionTier, SnapshotRecord};
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
    /// Retains the newest three versions in each vault, rule, and device.
    /// Equal timestamps keep input order, newest insertion first from the database.
    pub fn select_expired_snapshots(
        snapshots: &[SnapshotRecord],
        now_unix_secs: i64,
        policy: GfsPolicy,
    ) -> Vec<String> {
        let mut expired = Vec::new();
        let mut scopes: std::collections::BTreeMap<(&str, &str, &str), Vec<&SnapshotRecord>> =
            std::collections::BTreeMap::new();
        for snapshot in snapshots {
            scopes
                .entry((&snapshot.vault_id, &snapshot.rule_id, &snapshot.device_id))
                .or_default()
                .push(snapshot);
        }
        for scope in scopes.values_mut() {
            scope.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.created_at));
            let mut seen = HashSet::new();
            for (index, snap) in scope.iter().enumerate() {
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
                let keep_bucket = snap.created_at >= cutoff
                    && seen.insert((snap.retention_tier.as_str(), bucket));
                if index >= 3 && !keep_bucket {
                    expired.push(snap.snapshot_id.clone());
                }
            }
        }

        expired
    }

    /// Applies GFS retention to `vault_id` and reclaims all orphan chunks (`ref_count == 0`).
    pub fn run_gc(
        &self,
        vault_id: &str,
        now_unix_secs: i64,
        policy: GfsPolicy,
    ) -> Result<GcReport> {
        let report = self.vault_store.with_write_lock(|| {
            let snapshots = self.db.list_snapshots(Some(vault_id))?;
            let evaluated_snapshots = snapshots.len() as u64;
            let expired_ids = Self::select_expired_snapshots(&snapshots, now_unix_secs, policy);
            let _probe = ProbeGuard::start(
                "server.gc",
                "server.gc_run",
                Some(format!("vault_id={vault_id}, snapshots={evaluated_snapshots}")),
            );
            let expired_count = self.db.delete_snapshots_batch(&expired_ids)? as u64;
            if !expired_ids.is_empty() {
                if let Ok(roots) = self.vault_store.manifest_backup_roots(vault_id) {
                    for root in roots {
                        for snapshot_id in &expired_ids {
                            let path = root.join("manifests").join(format!("{snapshot_id}.json"));
                            if let Err(err) = std::fs::remove_file(&path) {
                                if err.kind() != std::io::ErrorKind::NotFound {
                                    tracing::warn!(path = %path.display(), %err, "expired manifest deletion failed");
                                }
                            }
                        }
                    }
                }
            }

            const ORPHAN_GRACE_SECS: i64 = 86_400;
            let cutoff = now_unix_secs.saturating_sub(ORPHAN_GRACE_SECS);
            let orphans = self.db.list_orphan_chunks_before(vault_id, cutoff)?;
            let mut removed = Vec::with_capacity(orphans.len());
            for orphan in orphans {
                match self.vault_store.delete_chunk_files(vault_id, &orphan.hash_blake3) {
                    Ok(()) => removed.push(orphan.hash_blake3),
                    Err(err) => tracing::warn!(%vault_id, %err, "orphan chunk deletion failed"),
                }
            }
            let (deleted_chunks, reclaimed_bytes) = self.db.delete_chunks_batch(vault_id, &removed)?;
            Ok(GcReport {
                evaluated_snapshots,
                expired_snapshots: expired_count,
                deleted_chunks,
                reclaimed_bytes,
            })
        })?;
        self.db.prune_revoked_sessions(now_unix_secs)?;
        Ok(report)
    }
}
