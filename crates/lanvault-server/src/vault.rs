//! Content-addressed vault storage, atomic staging-to-chunks ingestion,
//! multi-volume pool topology (`single` / `striped` / `mirror_raid1`),
//! and Volume GUID metadata binding.
//!
//! Directory layout per volume root:
//! ```text
//! <VaultRoot>/
//! ├── vault.meta.json
//! ├── index.db          (managed by Database in primary root)
//! ├── chunks/<ab>/<full_blake3_hex>
//! └── staging/<uuid>.tmp
//! ```

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use lanvault_core::{
    blake3_hash, Database, LanVaultError, PoolMode, Result, VaultRecord, VaultVolumeRecord,
    VolumeRole,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Persisted `<VaultRoot>/vault.meta.json` descriptor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultMetaFile {
    pub vault_id: String,
    pub name: String,
    pub pool_mode: PoolMode,
    pub quota_bytes: u64,
    pub e2ee_enabled: bool,
    pub created_at: i64,
    pub volumes: Vec<VolumeMetaEntry>,
}

/// Volume entry recorded inside `vault.meta.json` with Windows Volume GUID binding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeMetaEntry {
    pub volume_id: String,
    pub mount_path: String,
    pub volume_guid: Option<String>,
    pub role: VolumeRole,
}

/// Specification for an additional volume when creating a striped or mirrored vault.
#[derive(Debug, Clone, Deserialize)]
pub struct ExtraVolumeSpec {
    pub mount_path: String,
    pub volume_guid: Option<String>,
    pub role: VolumeRole,
}

#[derive(Debug, Clone, Copy)]
pub struct ChunkReplicaReport {
    pub healthy_copies: usize,
    pub required_copies: usize,
}

/// Content-addressed multi-volume vault manager.
pub struct VaultStore {
    db: Arc<Database>,
    /// GC and snapshot publication exclude chunk reads and writes. Per-prefix chunk locks
    /// keep unrelated uploads parallel while serializing work on the same content hash.
    write_lock: RwLock<()>,
    chunk_locks: [Mutex<()>; 256],
}

impl VaultStore {
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            write_lock: RwLock::new(()),
            chunk_locks: std::array::from_fn(|_| Mutex::new(())),
        }
    }

    pub fn with_write_lock<T>(&self, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        let _guard = self
            .write_lock
            .write()
            .map_err(|_| LanVaultError::InvalidState("vault write lock poisoned".to_string()))?;
        operation()
    }

    fn with_chunk_lock<T>(
        &self,
        hash: &[u8; 32],
        operation: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let _vault = self
            .write_lock
            .read()
            .map_err(|_| LanVaultError::InvalidState("vault write lock poisoned".to_string()))?;
        let _chunk = self.chunk_locks[usize::from(hash[0])]
            .lock()
            .map_err(|_| LanVaultError::InvalidState("chunk write lock poisoned".to_string()))?;
        operation()
    }

    /// Creates a new vault, initializes `chunks/` and `staging/` on all volumes,
    /// atomically writes `vault.meta.json`, and persists records into SQLite.
    // Mirrors the create-vault request contract one field per parameter; a struct would only
    // duplicate the HTTP DTO in the transport layer.
    #[allow(clippy::too_many_arguments)]
    pub fn create_vault(
        &self,
        name: String,
        primary_mount_path: String,
        primary_volume_guid: Option<String>,
        pool_mode: PoolMode,
        quota_bytes: u64,
        e2ee_enabled: bool,
        extra_volumes: Vec<ExtraVolumeSpec>,
    ) -> Result<(VaultRecord, Vec<VaultVolumeRecord>)> {
        let _guard = self
            .write_lock
            .write()
            .map_err(|_| LanVaultError::InvalidState("vault write lock poisoned".to_string()))?;
        if name.trim().is_empty() {
            return Err(LanVaultError::InvalidInput(
                "vault name must not be empty".to_string(),
            ));
        }
        if quota_bytes == 0 {
            return Err(LanVaultError::InvalidInput(
                "quota_bytes must be greater than zero".to_string(),
            ));
        }
        if matches!(pool_mode, PoolMode::MirrorRaid1)
            && !extra_volumes.iter().any(|v| v.role == VolumeRole::Mirror)
        {
            return Err(LanVaultError::InvalidInput(
                "mirror_raid1 pool requires at least one extra volume with role 'mirror'"
                    .to_string(),
            ));
        }

        let mut mount_paths: Vec<PathBuf> = Vec::with_capacity(1 + extra_volumes.len());
        let primary_key = normalized_mount_key(&primary_mount_path)?;
        mount_paths.push(primary_key);
        for extra in &extra_volumes {
            let extra_key = normalized_mount_key(&extra.mount_path)?;
            if mount_paths
                .iter()
                .any(|existing| paths_overlap(existing, &extra_key))
            {
                return Err(LanVaultError::InvalidInput(
                    "vault volumes must use distinct, non-overlapping mount paths".to_string(),
                ));
            }
            mount_paths.push(extra_key);
        }
        for existing in self.db.list_all_volumes()? {
            let existing_path = normalized_mount_key(&existing.mount_path)?;
            for requested in &mount_paths {
                if paths_overlap(&existing_path, requested) {
                    return Err(LanVaultError::InvalidInput(format!(
                        "mount path overlaps existing volume {}",
                        existing.mount_path
                    )));
                }
            }
        }

        let vault_id = Uuid::new_v4().to_string();
        let created_at = unix_timestamp_secs();

        let mut volumes = Vec::with_capacity(1 + extra_volumes.len());
        volumes.push(VaultVolumeRecord {
            volume_id: Uuid::new_v4().to_string(),
            vault_id: vault_id.clone(),
            mount_path: primary_mount_path.clone(),
            volume_guid: primary_volume_guid,
            role: VolumeRole::Primary,
            is_online: true,
        });

        for extra in extra_volumes {
            volumes.push(VaultVolumeRecord {
                volume_id: Uuid::new_v4().to_string(),
                vault_id: vault_id.clone(),
                mount_path: extra.mount_path,
                volume_guid: extra.volume_guid,
                role: extra.role,
                is_online: true,
            });
        }

        let vault = VaultRecord {
            vault_id: vault_id.clone(),
            name,
            pool_mode,
            quota_bytes,
            used_bytes: 0,
            e2ee_enabled,
            created_at,
        };

        let meta = VaultMetaFile {
            vault_id: vault.vault_id.clone(),
            name: vault.name.clone(),
            pool_mode: vault.pool_mode,
            quota_bytes: vault.quota_bytes,
            e2ee_enabled: vault.e2ee_enabled,
            created_at: vault.created_at,
            volumes: volumes
                .iter()
                .map(|v| VolumeMetaEntry {
                    volume_id: v.volume_id.clone(),
                    mount_path: v.mount_path.clone(),
                    volume_guid: v.volume_guid.clone(),
                    role: v.role,
                })
                .collect(),
        };

        for vol in &volumes {
            ensure_volume_layout(Path::new(&vol.mount_path))?;
            write_vault_meta_atomic(Path::new(&vol.mount_path), &meta)?;
        }

        if let Err(err) = self.db.insert_vault(&vault, &volumes) {
            for vol in &volumes {
                let _ = fs::remove_file(Path::new(&vol.mount_path).join("vault.meta.json"));
            }
            return Err(err);
        }
        Ok((vault, volumes))
    }

    /// Reads `vault.meta.json` from a volume root (for startup recovery or Volume GUID binding).
    pub fn read_vault_meta(volume_root: &Path) -> Result<VaultMetaFile> {
        let bytes = fs::read(volume_root.join("vault.meta.json"))?;
        let meta: VaultMetaFile = serde_json::from_slice(&bytes)?;
        Ok(meta)
    }

    /// Ingests a chunk into the vault using atomic staging write + rename.
    ///
    /// Verifies the BLAKE3 hash of `stored_bytes` at the boundary and enforces quota limits.
    /// Returns `true` if a new chunk was stored, or `false` if deduplicated.
    ///
    /// Deliberately lock-free with respect to other uploads: the staging→`chunks/`
    /// rename is atomic and content addressing makes concurrent writers idempotent.
    /// Bit-Rot scrubbing is what re-verifies bytes on disk; the hot path only does an
    /// O(1) `stat`, never a full re-read of a 1 MiB chunk per duplicate.
    pub fn put_chunk(
        &self,
        vault_id: &str,
        expected_hash: &[u8; 32],
        raw_size: u64,
        stored_bytes: &[u8],
    ) -> Result<bool> {
        self.put_chunk_with_compression(vault_id, expected_hash, raw_size, stored_bytes, "zstd")
    }

    pub fn put_chunk_with_compression(
        &self,
        vault_id: &str,
        expected_hash: &[u8; 32],
        raw_size: u64,
        stored_bytes: &[u8],
        compression: &str,
    ) -> Result<bool> {
        self.with_chunk_lock(expected_hash, || {
            self.put_chunk_with_compression_locked(
                vault_id,
                expected_hash,
                raw_size,
                stored_bytes,
                compression,
            )
        })
    }

    fn put_chunk_with_compression_locked(
        &self,
        vault_id: &str,
        expected_hash: &[u8; 32],
        raw_size: u64,
        stored_bytes: &[u8],
        compression: &str,
    ) -> Result<bool> {
        if !matches!(compression, "zstd" | "raw") {
            return Err(LanVaultError::InvalidInput(format!(
                "unsupported chunk compression: `{compression}`"
            )));
        }
        let mut _probe = lanvault_core::ProbeGuard::start(
            "server.vault",
            "vault.put_chunk",
            Some(format!("vault={vault_id}")),
        );
        _probe.add_bytes(stored_bytes.len() as u64);
        let actual_hash = blake3_hash(stored_bytes);
        if actual_hash != *expected_hash {
            return Err(LanVaultError::CorruptedChunk {
                hash: hex::encode(expected_hash),
            });
        }

        let vault = self
            .db
            .get_vault(vault_id)?
            .ok_or_else(|| LanVaultError::NotFound(format!("vault {vault_id}")))?;
        let stored_size = stored_bytes.len() as u64;
        let hash_hex = hex::encode(expected_hash);
        let volumes = self.online_volumes(vault_id)?;

        // Deduplicate on metadata only (hash + placement + a cheap size check), while verifying
        // that the immutable compression metadata matches so retries never corrupt decode tags.
        let existing = self.db.get_chunk(vault_id, expected_hash)?;
        if let Some(existing) = existing.as_ref() {
            let existing_comp = self.db.get_chunk_compression(vault_id, expected_hash);
            if !existing_comp.eq_ignore_ascii_case(compression) {
                return Err(LanVaultError::InvalidState(format!(
                    "chunk compression conflict for {hash_hex}: stored as `{existing_comp}`, requested `{compression}`"
                )));
            }
            if existing.scrub_status == lanvault_core::ScrubStatus::Ok
                && self.chunk_matches_on_pool(
                    &vault,
                    &volumes,
                    expected_hash,
                    stored_bytes,
                )?
            {
                self.db.upsert_chunk_with_compression(
                    vault_id,
                    expected_hash,
                    raw_size,
                    stored_size,
                    compression,
                )?;
                self.db.renew_orphan_chunk(vault_id, expected_hash)?;
                return Ok(false);
            }
        }

        let available = vault.quota_bytes.saturating_sub(vault.used_bytes);
        if existing.is_none() && stored_size > available {
            return Err(LanVaultError::QuotaExceeded {
                required: stored_size,
                available,
            });
        }

        match vault.pool_mode {
            PoolMode::Single => {
                let primary = primary_volume(&volumes)?;
                write_chunk_atomic(Path::new(&primary.mount_path), &hash_hex, stored_bytes)?;
            }
            PoolMode::Striped => {
                let target = striped_target_volume(&volumes, expected_hash)?;
                write_chunk_atomic(Path::new(&target.mount_path), &hash_hex, stored_bytes)?;
            }
            PoolMode::MirrorRaid1 => {
                let writable_count = volumes
                    .iter()
                    .filter(|volume| {
                        matches!(volume.role, VolumeRole::Primary | VolumeRole::Mirror)
                    })
                    .count();
                if writable_count < 2 {
                    return Err(LanVaultError::InvalidState(
                        "mirror_raid1 requires a primary and at least one online mirror"
                            .to_string(),
                    ));
                }
                for vol in &volumes {
                    if matches!(vol.role, VolumeRole::Primary | VolumeRole::Mirror) {
                        write_chunk_atomic(Path::new(&vol.mount_path), &hash_hex, stored_bytes)?;
                    }
                }
            }
        }

        let is_new = match self.db.upsert_chunk_with_quota_and_compression(
            vault_id,
            expected_hash,
            raw_size,
            stored_size,
            compression,
        ) {
            Ok(is_new) => is_new,
            Err(err) => {
                if existing.is_none()
                    && self
                        .db
                        .get_chunk(vault_id, expected_hash)
                        .ok()
                        .flatten()
                        .is_none()
                {
                    let _ = self.delete_chunk_files(vault_id, expected_hash);
                }
                return Err(err);
            }
        };
        // `upsert_chunk` 对"索引里已有、但磁盘文件缺失/被截断"的情况返回 false，
        // 而我们刚刚确实重写了物理文件。返回值必须反映"本次是否写入了新字节"，
        // 否则损坏修复会被上报成"去重命中"，调用方与测试都会误判。
        Ok(is_new || existing.is_some())
    }

    /// Reads a chunk from the vault and verifies its BLAKE3 integrity.
    ///
    /// Under `mirror_raid1`, if the primary copy is unreadable or corrupted, transparently
    /// reads from a healthy mirror and repairs the primary copy.
    pub fn get_chunk(&self, vault_id: &str, hash: &[u8; 32]) -> Result<Vec<u8>> {
        self.with_chunk_lock(hash, || self.get_chunk_locked(vault_id, hash))
    }

    fn get_chunk_locked(&self, vault_id: &str, hash: &[u8; 32]) -> Result<Vec<u8>> {
        let _probe = lanvault_core::ProbeGuard::start(
            "server.vault",
            "vault.get_chunk",
            Some(format!("vault={vault_id}")),
        );
        let vault = self
            .db
            .get_vault(vault_id)?
            .ok_or_else(|| LanVaultError::NotFound(format!("vault {vault_id}")))?;
        let volumes = self.online_volumes(vault_id)?;
        let hash_hex = hex::encode(hash);

        match vault.pool_mode {
            PoolMode::Single => {
                let primary = primary_volume(&volumes)?;
                read_verified_chunk(Path::new(&primary.mount_path), &hash_hex, hash)
            }
            PoolMode::Striped => {
                let target = striped_target_volume(&volumes, hash)?;
                let target_path = chunk_file_path(Path::new(&target.mount_path), &hash_hex);
                if target_path.exists() {
                    return read_verified_chunk(Path::new(&target.mount_path), &hash_hex, hash);
                }
                for vol in &volumes {
                    let candidate = chunk_file_path(Path::new(&vol.mount_path), &hash_hex);
                    if candidate.exists() {
                        return read_verified_chunk(Path::new(&vol.mount_path), &hash_hex, hash);
                    }
                }
                Err(LanVaultError::NotFound(format!("chunk {hash_hex}")))
            }
            PoolMode::MirrorRaid1 => {
                let candidates: Vec<&VaultVolumeRecord> = volumes
                    .iter()
                    .filter(|volume| {
                        matches!(volume.role, VolumeRole::Primary | VolumeRole::Mirror)
                    })
                    .collect();
                for vol in &candidates {
                    if let Ok(bytes) =
                        read_verified_chunk(Path::new(&vol.mount_path), &hash_hex, hash)
                    {
                        if vol.role == VolumeRole::Mirror {
                            if let Some(primary) =
                                candidates.iter().find(|v| v.role == VolumeRole::Primary)
                            {
                                if let Err(err) = write_chunk_atomic(
                                    Path::new(&primary.mount_path),
                                    &hash_hex,
                                    &bytes,
                                ) {
                                    tracing::warn!(
                                        vault_id = %vault_id,
                                        hash = %hash_hex,
                                        "failed to self-heal primary chunk from mirror: {err}"
                                    );
                                }
                            }
                        }
                        return Ok(bytes);
                    }
                }
                Err(LanVaultError::CorruptedChunk { hash: hash_hex })
            }
        }
    }

    pub fn has_verified_chunk(&self, vault_id: &str, hash: &[u8; 32]) -> Result<bool> {
        self.with_chunk_lock(hash, || {
            match self.get_chunk_locked(vault_id, hash) {
                Ok(_) => {
                    self.db.renew_orphan_chunk(vault_id, hash)?;
                    Ok(true)
                }
                Err(LanVaultError::NotFound(_)) | Err(LanVaultError::CorruptedChunk { .. }) => {
                    Ok(false)
                }
                Err(LanVaultError::Io(err))
                    if err.kind() == std::io::ErrorKind::NotFound =>
                {
                    Ok(false)
                }
                Err(err) => Err(err),
            }
        })
    }

    pub fn resume_chunk(&self, vault_id: &str, hash: &[u8; 32]) -> Result<bool> {
        self.has_verified_chunk(vault_id, hash)
    }

    pub fn validate_snapshot_chunks(&self, vault_id: &str, hashes: &[[u8; 32]]) -> Result<()> {
        for hash in hashes {
            if !self.has_verified_chunk(vault_id, hash)? {
                return Err(LanVaultError::NotFound(format!(
                    "chunk {} in vault {vault_id}",
                    hex::encode(hash)
                )));
            }
        }
        Ok(())
    }

    /// O(1) 轻量级块完整性检查：验证索引记录与磁盘文件长度，避免快照提交时全盘重读和重复 BLAKE3 导致超时。
    pub fn has_intact_chunk(&self, vault_id: &str, hash: &[u8; 32]) -> Result<bool> {
        let Some(vault) = self.db.get_vault(vault_id)? else {
            return Ok(false);
        };
        let Some(record) = self.db.get_chunk(vault_id, hash)? else {
            return Ok(false);
        };
        let volumes = self.online_volumes(vault_id)?;
        let hash_hex = hex::encode(hash);
        Ok(self.chunk_intact_on_pool(&vault, &volumes, &hash_hex, record.stored_size))
    }

    pub fn manifest_backup_roots(&self, vault_id: &str) -> Result<Vec<PathBuf>> {
        let vault = self
            .db
            .get_vault(vault_id)?
            .ok_or_else(|| LanVaultError::NotFound(format!("vault {vault_id}")))?;
        let mut roots = Vec::new();
        for volume in self.online_volumes(vault_id)? {
            if vault.pool_mode == PoolMode::MirrorRaid1
                && !matches!(volume.role, VolumeRole::Primary | VolumeRole::Mirror)
            {
                continue;
            }
            roots.push(PathBuf::from(volume.mount_path));
        }
        if roots.is_empty() {
            return Err(LanVaultError::InvalidState(
                "snapshot manifest has no writable volume".to_string(),
            ));
        }
        Ok(roots)
    }

    /// Reads raw chunk bytes directly from disk without RAID failover (used by Bit-Rot scrub).
    pub fn read_chunk_raw_on_primary(&self, vault_id: &str, hash: &[u8; 32]) -> Result<Vec<u8>> {
        let vault = self
            .db
            .get_vault(vault_id)?
            .ok_or_else(|| LanVaultError::NotFound(format!("vault {vault_id}")))?;
        let volumes = self.online_volumes(vault_id)?;
        let hash_hex = hex::encode(hash);

        let vol = match vault.pool_mode {
            PoolMode::Striped => striped_target_volume(&volumes, hash)?,
            PoolMode::Single | PoolMode::MirrorRaid1 => primary_volume(&volumes)?,
        };
        let path = chunk_file_path(Path::new(&vol.mount_path), &hash_hex);
        Ok(fs::read(path)?)
    }

    /// Attempts to read a verified healthy copy from a `mirror_raid1` mirror volume.
    pub fn try_read_healthy_mirror(
        &self,
        vault_id: &str,
        hash: &[u8; 32],
    ) -> Result<Option<Vec<u8>>> {
        let volumes = self.online_volumes(vault_id)?;
        let hash_hex = hex::encode(hash);
        for vol in volumes.iter().filter(|v| v.role == VolumeRole::Mirror) {
            if let Ok(bytes) = read_verified_chunk(Path::new(&vol.mount_path), &hash_hex, hash) {
                return Ok(Some(bytes));
            }
        }
        Ok(None)
    }

    pub fn inspect_chunk_copies(
        &self,
        vault_id: &str,
        hash: &[u8; 32],
    ) -> Result<ChunkReplicaReport> {
        let vault = self
            .db
            .get_vault(vault_id)?
            .ok_or_else(|| LanVaultError::NotFound(format!("vault {vault_id}")))?;
        let volumes = self.db.list_vault_volumes(vault_id)?;
        let hash_hex = hex::encode(hash);
        let candidates: Vec<&VaultVolumeRecord> = match vault.pool_mode {
            PoolMode::Single => volumes
                .iter()
                .filter(|volume| volume.is_online && volume.role == VolumeRole::Primary)
                .collect(),
            PoolMode::Striped => volumes.iter().filter(|volume| volume.is_online).collect(),
            PoolMode::MirrorRaid1 => volumes
                .iter()
                .filter(|volume| {
                    volume.is_online
                        && matches!(volume.role, VolumeRole::Primary | VolumeRole::Mirror)
                })
                .collect(),
        };
        let required_copies = match vault.pool_mode {
            PoolMode::MirrorRaid1 => volumes
                .iter()
                .filter(|volume| matches!(volume.role, VolumeRole::Primary | VolumeRole::Mirror))
                .count()
                .max(2),
            PoolMode::Single | PoolMode::Striped => 1,
        };
        if candidates.is_empty() {
            return Ok(ChunkReplicaReport {
                healthy_copies: 0,
                required_copies,
            });
        }
        let mut healthy_copies = 0usize;
        for volume in candidates {
            if read_verified_chunk(Path::new(&volume.mount_path), &hash_hex, hash).is_ok() {
                healthy_copies += 1;
            }
        }
        Ok(ChunkReplicaReport {
            healthy_copies,
            required_copies,
        })
    }

    /// Atomically overwrites a corrupted chunk on all relevant volumes using verified healthy bytes.
    pub fn overwrite_chunk_for_heal(
        &self,
        vault_id: &str,
        expected_hash: &[u8; 32],
        healthy_bytes: &[u8],
    ) -> Result<()> {
        let _guard = self
            .write_lock
            .write()
            .map_err(|_| LanVaultError::InvalidState("vault write lock poisoned".to_string()))?;
        if blake3_hash(healthy_bytes) != *expected_hash {
            return Err(LanVaultError::CorruptedChunk {
                hash: hex::encode(expected_hash),
            });
        }

        let vault = self
            .db
            .get_vault(vault_id)?
            .ok_or_else(|| LanVaultError::NotFound(format!("vault {vault_id}")))?;
        let volumes = self.online_volumes(vault_id)?;
        let hash_hex = hex::encode(expected_hash);

        match vault.pool_mode {
            PoolMode::Single => {
                let primary = primary_volume(&volumes)?;
                write_chunk_atomic(Path::new(&primary.mount_path), &hash_hex, healthy_bytes)?;
            }
            PoolMode::Striped => {
                let target = striped_target_volume(&volumes, expected_hash)?;
                write_chunk_atomic(Path::new(&target.mount_path), &hash_hex, healthy_bytes)?;
            }
            PoolMode::MirrorRaid1 => {
                for vol in &volumes {
                    if matches!(vol.role, VolumeRole::Primary | VolumeRole::Mirror) {
                        write_chunk_atomic(Path::new(&vol.mount_path), &hash_hex, healthy_bytes)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Deletes a chunk's physical file(s) from all volumes of a vault during garbage collection.
    pub fn delete_chunk_files(&self, vault_id: &str, hash: &[u8; 32]) -> Result<()> {
        let volumes = self.db.list_vault_volumes(vault_id)?;
        let hash_hex = hex::encode(hash);
        for vol in &volumes {
            let path = chunk_file_path(Path::new(&vol.mount_path), &hash_hex);
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    pub(crate) fn online_volumes(&self, vault_id: &str) -> Result<Vec<VaultVolumeRecord>> {
        let volumes: Vec<VaultVolumeRecord> = self
            .db
            .list_vault_volumes(vault_id)?
            .into_iter()
            .filter(|v| v.is_online)
            .collect();
        if volumes.is_empty() {
            return Err(LanVaultError::NotFound(format!(
                "no online volumes for vault {vault_id}"
            )));
        }
        Ok(volumes)
    }

    /// O(1) `stat` 层面的"块已完整落盘"判断（配额与去重快路径用，不做整块回读）。
    ///
    /// 校验的是**文件长度等于存储字节数**，而不只是"文件存在"：去掉逐块 `fsync` 之后，
    /// 断电可能留下被截断的文件，只判断存在会把唯一的好副本判成"已存在"而跳过重传。
    fn chunk_intact_on_pool(
        &self,
        vault: &VaultRecord,
        volumes: &[VaultVolumeRecord],
        hash_hex: &str,
        expected_stored_size: u64,
    ) -> bool {
        let intact = |volume: &VaultVolumeRecord| {
            chunk_file_path(Path::new(&volume.mount_path), hash_hex)
                .metadata()
                .map(|meta| meta.is_file() && meta.len() == expected_stored_size)
                .unwrap_or(false)
        };
        match vault.pool_mode {
            PoolMode::Single => volumes
                .iter()
                .find(|volume| volume.role == VolumeRole::Primary)
                .is_some_and(intact),
            PoolMode::Striped => {
                let Some(first_byte) = hex::decode(&hash_hex[..2])
                    .ok()
                    .and_then(|bytes| bytes.first().copied())
                else {
                    return false;
                };
                volumes
                    .get(first_byte as usize % volumes.len().max(1))
                    .is_some_and(intact)
            }
            PoolMode::MirrorRaid1 => volumes
                .iter()
                .filter(|volume| matches!(volume.role, VolumeRole::Primary | VolumeRole::Mirror))
                .all(intact),
        }
    }

    fn chunk_matches_on_pool(
        &self,
        vault: &VaultRecord,
        volumes: &[VaultVolumeRecord],
        hash: &[u8; 32],
        expected_bytes: &[u8],
    ) -> Result<bool> {
        let hash_hex = hex::encode(hash);
        let required: Vec<&VaultVolumeRecord> = match vault.pool_mode {
            PoolMode::Single => volumes
                .iter()
                .filter(|volume| volume.role == VolumeRole::Primary)
                .collect(),
            PoolMode::Striped => match striped_target_volume(volumes, hash) {
                Ok(volume) => vec![volume],
                Err(_) => Vec::new(),
            },
            PoolMode::MirrorRaid1 => volumes
                .iter()
                .filter(|volume| matches!(volume.role, VolumeRole::Primary | VolumeRole::Mirror))
                .collect(),
        };
        if required.is_empty() {
            return Ok(false);
        }
        for volume in required {
            match read_verified_chunk(Path::new(&volume.mount_path), &hash_hex, hash) {
                Ok(bytes) if bytes == expected_bytes => {}
                Ok(_)
                | Err(LanVaultError::NotFound(_))
                | Err(LanVaultError::CorruptedChunk { .. }) => return Ok(false),
                Err(LanVaultError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(false);
                }
                Err(err) => return Err(err),
            }
        }
        Ok(true)
    }
}

fn ensure_volume_layout(root: &Path) -> Result<()> {
    fs::create_dir_all(root.join("chunks"))?;
    fs::create_dir_all(root.join("staging"))?;
    Ok(())
}

fn write_vault_meta_atomic(root: &Path, meta: &VaultMetaFile) -> Result<()> {
    let payload = serde_json::to_vec_pretty(meta)?;
    let tmp_path = root
        .join("staging")
        .join(format!("vault.meta.{}.tmp", Uuid::new_v4()));
    let final_path = root.join("vault.meta.json");

    {
        let mut file = File::create(&tmp_path)?;
        file.write_all(&payload)?;
        file.sync_all()?;
    }
    // Windows 上偶发杀毒软件扫描短暂锁住新目录项，短退避重试并在内容寻址已落盘时幂等放行。
    let mut last_err = None;
    for attempt in 0..4u32 {
        match fs::rename(&tmp_path, &final_path) {
            Ok(()) => {
                last_err = None;
                break;
            }
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied && attempt < 3 => {
                last_err = Some(err);
                std::thread::sleep(std::time::Duration::from_millis(25u64 << attempt));
            }
            Err(err) => {
                let _ = fs::remove_file(&tmp_path);
                return Err(err.into());
            }
        }
    }
    if let Some(err) = last_err {
        let _ = fs::remove_file(&tmp_path);
        return Err(err.into());
    }
    Ok(())
}

/// Computes `<root>/chunks/<first_2_hex>/<full_hex>`.
pub fn chunk_file_path(volume_root: &Path, hash_hex: &str) -> PathBuf {
    let bucket = hash_hex.get(..2).unwrap_or("00");
    volume_root.join("chunks").join(bucket).join(hash_hex)
}

fn write_chunk_atomic(volume_root: &Path, hash_hex: &str, bytes: &[u8]) -> Result<()> {
    let staging_dir = volume_root.join("staging");
    let final_path = chunk_file_path(volume_root, hash_hex);
    let bucket_dir = final_path
        .parent()
        .ok_or_else(|| LanVaultError::InvalidInput("chunk path has no parent".to_string()))?;
    // Two `create_dir_all` calls per chunk dominated small-chunk ingest; the bucket exists
    // after the first write in that 256-way prefix, so only create what is missing.
    if !bucket_dir.exists() {
        fs::create_dir_all(bucket_dir)?;
    }
    if !staging_dir.exists() {
        fs::create_dir_all(&staging_dir)?;
    }

    // Chunk payloads are content-addressed and BLAKE3-verified on every read plus by the
    // Bit-Rot scrubber, so per-chunk `fsync` only bought latency (~1 disk flush per MiB).
    // Landing in the page cache and renaming atomically is what lets a 2.5 GbE link be the
    // bottleneck instead of the spindle; a torn file after power loss is detected and healed.
    let tmp_path = staging_dir.join(format!("{}.{}.tmp", hash_hex, Uuid::new_v4()));
    {
        let mut file = File::create(&tmp_path)?;
        file.write_all(bytes)?;
    }
    fs::rename(&tmp_path, &final_path)?;
    Ok(())
}

fn read_verified_chunk(
    volume_root: &Path,
    hash_hex: &str,
    expected_hash: &[u8; 32],
) -> Result<Vec<u8>> {
    let path = chunk_file_path(volume_root, hash_hex);
    let bytes = fs::read(&path)?;
    if blake3_hash(&bytes) != *expected_hash {
        return Err(LanVaultError::CorruptedChunk {
            hash: hash_hex.to_string(),
        });
    }
    Ok(bytes)
}

fn primary_volume(volumes: &[VaultVolumeRecord]) -> Result<&VaultVolumeRecord> {
    volumes
        .iter()
        .find(|v| v.role == VolumeRole::Primary)
        .ok_or_else(|| LanVaultError::NotFound("primary volume".to_string()))
}

fn normalized_mount_key(raw: &str) -> Result<PathBuf> {
    let path = Path::new(raw);
    if raw.trim().is_empty() || !path.is_absolute() {
        return Err(LanVaultError::InvalidInput(
            "vault mount path must be an absolute path".to_string(),
        ));
    }
    let resolved = if path.exists() {
        fs::canonicalize(path)?
    } else {
        // Walk up to the longest existing ancestor so `..` or symlinks cannot bypass overlap checks.
        let mut ancestor = path.to_path_buf();
        let mut tail: Vec<std::ffi::OsString> = Vec::new();
        while !ancestor.exists() {
            let Some(name) = ancestor.file_name() else {
                break;
            };
            let name_str = name.to_string_lossy();
            if name_str == ".." {
                return Err(LanVaultError::InvalidInput(
                    "vault mount path must not contain parent '..' segments".to_string(),
                ));
            }
            if name_str != "." {
                tail.push(name.to_os_string());
            }
            if !ancestor.pop() {
                break;
            }
        }
        let mut base = if ancestor.exists() {
            fs::canonicalize(&ancestor)?
        } else {
            ancestor
        };
        for seg in tail.into_iter().rev() {
            base.push(seg);
        }
        base
    };
    #[cfg(windows)]
    let resolved = {
        let stripped = lanvault_core::strip_windows_verbatim(&resolved.to_string_lossy());
        PathBuf::from(stripped.to_lowercase())
    };
    Ok(resolved)
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn striped_target_volume<'a>(
    volumes: &'a [VaultVolumeRecord],
    hash: &[u8; 32],
) -> Result<&'a VaultVolumeRecord> {
    if volumes.is_empty() {
        return Err(LanVaultError::NotFound(
            "no online volumes in vault".to_string(),
        ));
    }
    let idx = (hash[0] as usize) % volumes.len();
    Ok(&volumes[idx])
}

pub(crate) fn unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_vault_rejects_nested_self_overlapping_extra_volumes() {
        let temp_dir = std::env::temp_dir().join(format!("lanvault-vault-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).unwrap();
        let db = Arc::new(Database::open(&temp_dir.join("index.db")).unwrap());
        let store = VaultStore::new(db);

        let primary = temp_dir.join("primary");
        let nested_mirror = primary.join("mirror_sub");
        let res = store.create_vault(
            "Test Pool".to_string(),
            primary.to_string_lossy().into_owned(),
            None,
            PoolMode::MirrorRaid1,
            1024 * 1024,
            true,
            vec![ExtraVolumeSpec {
                mount_path: nested_mirror.to_string_lossy().into_owned(),
                volume_guid: None,
                role: VolumeRole::Mirror,
            }],
        );
        assert!(matches!(res, Err(LanVaultError::InvalidInput(_))));
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
