//! Bit-Rot silent corruption scrubbing and self-healing engine.
//!
//! Parallelizes on-disk BLAKE3 verification across available CPU threads (`std::thread::scope`)
//! and commits all `scrub_status` updates in a single SQLite batch transaction.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use lanvault_core::{
    blake3_hash, Database, LanVaultError, PoolMode, ProbeGuard, Result, ScrubStatus,
};
use serde::Serialize;
use tokio::sync::broadcast;

use crate::vault::VaultStore;

/// Per-worker scrub outcome: `(status_updates, alerts, bytes_verified, corrupted, healed)`.
type WorkerScrubOutcome = (Vec<([u8; 32], ScrubStatus)>, Vec<BitRotAlertEvent>, u64, u64, u64);

#[derive(Debug, Clone, Serialize)]
pub struct BitRotAlertEvent {
    pub chunk_hash: String,
    pub vault_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScrubReport {
    pub scrub_id: String,
    pub checked_chunks: u64,
    pub corrupted_chunks: u64,
    pub healed_chunks: u64,
    pub status: String,
}

pub struct BitRotScrubber {
    db: Arc<Database>,
    vault_store: Arc<VaultStore>,
    alert_tx: broadcast::Sender<BitRotAlertEvent>,
}

impl BitRotScrubber {
    pub fn new(
        db: Arc<Database>,
        vault_store: Arc<VaultStore>,
        alert_tx: broadcast::Sender<BitRotAlertEvent>,
    ) -> Self {
        Self {
            db,
            vault_store,
            alert_tx,
        }
    }

    /// Scans all chunks belonging to `vault_id` in parallel across CPU cores,
    /// auto-healing RAID-1 mirrors and committing all DB statuses in a single batch transaction.
    pub fn run_scrub(&self, vault_id: &str) -> Result<ScrubReport> {
        let mut probe = ProbeGuard::start(
            "server.scrub",
            "server.parallel_scrub",
            Some(format!("vault_id={vault_id}")),
        );
        let vault = self
            .db
            .get_vault(vault_id)?
            .ok_or_else(|| LanVaultError::NotFound(format!("vault {vault_id}")))?;
        let chunks = self.db.list_all_chunks(vault_id)?;
        let now = unix_timestamp_secs();
        let total_chunks = chunks.len() as u64;

        if chunks.is_empty() {
            return Ok(ScrubReport {
                scrub_id: uuid::Uuid::new_v4().to_string(),
                checked_chunks: 0,
                corrupted_chunks: 0,
                healed_chunks: 0,
                status: "completed".to_string(),
            });
        }

        let workers = std::thread::available_parallelism()
            .map(|n| n.get().clamp(1, 16))
            .unwrap_or(4)
            .min(chunks.len());
        let slice_len = chunks.len().div_ceil(workers);
        let is_raid1 = vault.pool_mode == PoolMode::MirrorRaid1;

        let mut all_updates: Vec<([u8; 32], ScrubStatus)> = Vec::with_capacity(chunks.len());
        let mut alerts: Vec<BitRotAlertEvent> = Vec::new();
        let mut total_bytes: u64 = 0;
        let mut corrupted: u64 = 0;
        let mut healed: u64 = 0;

        let per_thread_results: Vec<Result<WorkerScrubOutcome>> = std::thread::scope(|s| {
                let mut handles = Vec::with_capacity(workers);
                for part in chunks.chunks(slice_len) {
                    let store = Arc::clone(&self.vault_store);
                    handles.push(s.spawn(move || {
                        let mut local_updates = Vec::with_capacity(part.len());
                        let mut local_alerts = Vec::new();
                        let mut local_bytes = 0u64;
                        let mut local_corrupted = 0u64;
                        let mut local_healed = 0u64;

                        for chunk in part {
                            local_bytes = local_bytes.saturating_add(chunk.stored_size);
                            let replica_report =
                                store.inspect_chunk_copies(vault_id, &chunk.hash_blake3)?;
                            if replica_report.healthy_copies >= replica_report.required_copies {
                                local_updates.push((chunk.hash_blake3, ScrubStatus::Ok));
                                continue;
                            }

                            let hash_hex = hex::encode(chunk.hash_blake3);
                            if is_raid1 && replica_report.healthy_copies > 0 {
                                if let Ok(healthy_bytes) =
                                    store.get_chunk(vault_id, &chunk.hash_blake3)
                                {
                                    store.overwrite_chunk_for_heal(
                                        vault_id,
                                        &chunk.hash_blake3,
                                        &healthy_bytes,
                                    )?;
                                    local_updates.push((chunk.hash_blake3, ScrubStatus::Healed));
                                    local_healed += 1;
                                    local_alerts.push(BitRotAlertEvent {
                                        chunk_hash: hash_hex,
                                        vault_id: vault_id.to_string(),
                                        status: "healed".to_string(),
                                    });
                                    continue;
                                }
                            }

                            local_updates.push((chunk.hash_blake3, ScrubStatus::Corrupted));
                            local_corrupted += 1;
                            local_alerts.push(BitRotAlertEvent {
                                chunk_hash: hash_hex,
                                vault_id: vault_id.to_string(),
                                status: "corrupted".to_string(),
                            });
                        }

                        Ok((
                            local_updates,
                            local_alerts,
                            local_bytes,
                            local_corrupted,
                            local_healed,
                        ))
                    }));
                }

                handles
                    .into_iter()
                    .map(|h| {
                        h.join().map_err(|_| {
                            LanVaultError::InvalidState("scrub worker thread panicked".to_string())
                        })?
                    })
                    .collect()
            });

        for res in per_thread_results {
            let (u, a, b, c, h) = res?;
            all_updates.extend(u);
            alerts.extend(a);
            total_bytes = total_bytes.saturating_add(b);
            corrupted += c;
            healed += h;
        }

        probe.add_bytes(total_bytes);
        self.db
            .update_chunk_scrub_statuses_batch(vault_id, &all_updates, now)?;

        for ev in alerts {
            let _ = self.alert_tx.send(ev);
        }

        Ok(ScrubReport {
            scrub_id: uuid::Uuid::new_v4().to_string(),
            checked_chunks: total_chunks,
            corrupted_chunks: corrupted,
            healed_chunks: healed,
            status: "completed".to_string(),
        })
    }

    /// Returns hashes of corrupted chunks in `vault_id` that need re-upload from trusted peers.
    pub fn pending_heal_hashes(&self, vault_id: &str) -> Result<Vec<[u8; 32]>> {
        let corrupted = self.db.list_corrupted_chunks(vault_id)?;
        Ok(corrupted.into_iter().map(|c| c.hash_blake3).collect())
    }

    /// Heals a corrupted chunk using verified healthy bytes uploaded by a reconnecting peer.
    pub fn heal_chunk_from_peer(
        &self,
        vault_id: &str,
        expected_hash: &[u8; 32],
        healthy_bytes: &[u8],
    ) -> Result<()> {
        if blake3_hash(healthy_bytes) != *expected_hash {
            return Err(LanVaultError::IntegrityMismatch {
                expected: hex::encode(expected_hash),
                actual: hex::encode(blake3_hash(healthy_bytes)),
            });
        }
        self.vault_store
            .overwrite_chunk_for_heal(vault_id, expected_hash, healthy_bytes)?;
        let now = unix_timestamp_secs();
        self.db
            .update_chunk_scrub_status(vault_id, expected_hash, ScrubStatus::Healed, now)?;
        let _ = self.alert_tx.send(BitRotAlertEvent {
            chunk_hash: hex::encode(expected_hash),
            vault_id: vault_id.to_string(),
            status: "healed".to_string(),
        });
        Ok(())
    }
}

fn unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
