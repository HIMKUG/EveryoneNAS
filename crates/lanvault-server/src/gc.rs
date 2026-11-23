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
    pub daily_keep: i64,
    pub weekly_keep: i64,
    pub monthly_keep: i64,
}

impl Default for GfsPolicy {
    fn default() -> Self {
        Self {
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
        let mut sorted: Vec<&SnapshotRecord> = snapshots.iter().collect();
        sorted.sort_by_key(|s| std::cmp::Reverse(s.created_at));

        let daily_cutoff = now_unix_secs - policy.daily_keep * SECS_PER_DAY;
        let weekly_cutoff = now_unix_secs - policy.weekly_keep * SECS_PER_WEEK;
        let monthly_cutoff = now_unix_secs - policy.monthly_keep * SECS_PER_MONTH;

        let mut seen_days: HashSet<i64> = HashSet::with_capacity(32);
        let mut seen_weeks: HashSet<i64> = HashSet::with_capacity(32);
        let mut seen_months: HashSet<i64> = HashSet::with_capacity(32);
        let mut expired = Vec::new();

        for snap in sorted {
            if snap.retention_tier == RetentionTier::Pinned {
                continue;
            }

            let mut retained = false;

            if snap.created_at >= daily_cutoff {
                let day_bucket = snap.created_at.div_euclid(SECS_PER_DAY);
                if seen_days.insert(day_bucket) {
                    retained = true;
                }
            }

            if !retained && snap.created_at >= weekly_cutoff {
                let week_bucket = snap.created_at.div_euclid(SECS_PER_WEEK);
                if seen_weeks.insert(week_bucket) {
                    retained = true;
                }
            }

            if !retained && snap.created_at >= monthly_cutoff {
                let month_bucket = snap.created_at.div_euclid(SECS_PER_MONTH);
                if seen_months.insert(month_bucket) {
                    retained = true;
                }
            }

            if !retained {
                expired.push(snap.snapshot_id.clone());
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
        let snapshots = self.db.list_snapshots(Some(vault_id))?;
        let evaluated_snapshots = snapshots.len() as u64;
        let expired_ids = Self::select_expired_snapshots(&snapshots, now_unix_secs, policy);

        let _probe = ProbeGuard::start(
            "server.gc",
            "server.gc_run",
            Some(format!("vault_id={vault_id}, snapshots={evaluated_snapshots}")),
        );
        let expired_count = self.db.delete_snapshots_batch(&expired_ids)? as u64;

        let orphans = self.db.list_orphan_chunks(vault_id)?;
        let mut orphan_hashes: Vec<[u8; 32]> = Vec::with_capacity(orphans.len());
        let mut reclaimed_bytes = 0u64;

        for orphan in &orphans {
            self.vault_store
                .delete_chunk_files(vault_id, &orphan.hash_blake3)?;
            orphan_hashes.push(orphan.hash_blake3);
            reclaimed_bytes = reclaimed_bytes.saturating_add(orphan.stored_size);
        }
        let deleted_chunks = self.db.delete_chunks_batch(vault_id, &orphan_hashes)? as u64;

        if reclaimed_bytes > 0 {
            self.db
                .update_vault_used_bytes(vault_id, -(reclaimed_bytes as i64))?;
        }

        Ok(GcReport {
            evaluated_snapshots,
            expired_snapshots: expired_count,
            deleted_chunks,
            reclaimed_bytes,
        })
    }
}
