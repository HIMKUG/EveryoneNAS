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

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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
    write_lock: Mutex<()>,
}

impl VaultStore {
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            write_lock: Mutex::new(()),
        }
    }

    pub fn with_write_lock<T>(&self, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| LanVaultError::InvalidState("vault write lock poisoned".to_string()))?;
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
            .lock()
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

        let mut mount_paths = HashSet::new();
        mount_paths.insert(normalized_mount_key(&primary_mount_path)?);
        for extra in &extra_volumes {
            if !mount_paths.insert(normalized_mount_key(&extra.mount_path)?) {
                return Err(LanVaultError::InvalidInput(
                    "vault volumes must use distinct mount paths".to_string(),
                ));
            }
        }
        for existing_vault in self.db.list_vaults()? {
            for existing in self.db.list_vault_volumes(&existing_vault.vault_id)? {
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

        self.db.insert_vault(&vault, &volumes)?;
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
    pub fn put_chunk(
        &self,
        vault_id: &str,
        expected_hash: &[u8; 32],
        raw_size: u64,
        stored_bytes: &[u8],
    ) -> Result<bool> {
        let _guard = self.write_lock.lock().map_err(|_| {
            LanVaultError::InvalidState("vault write lock poisoned".to_string())
        })?;
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

        // Deduplicate only when the registered bytes can still be read and verified.
        let existing = self.db.get_chunk(vault_id, expected_hash)?;
        if let Some(existing) = existing.as_ref() {
            if existing.scrub_status == lanvault_core::ScrubStatus::Ok
                && self.get_chunk(vault_id, expected_hash).is_ok()
            {
                self.db
                    .upsert_chunk(vault_id, expected_hash, raw_size, stored_size)?;
                return Ok(false);
            }
        }

        let available = vault.quota_bytes.saturating_sub(vault.used_bytes);
        if stored_size > available {
            return Err(LanVaultError::QuotaExceeded {
                required: stored_size,
                available,
            });
        }

        let volumes = self.online_volumes(vault_id)?;
        let hash_hex = hex::encode(expected_hash);

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
                        "mirror_raid1 requires a primary and at least one online mirror".to_string(),
                    ));
                }
                for vol in &volumes {
                    if matches!(vol.role, VolumeRole::Primary | VolumeRole::Mirror) {
                        write_chunk_atomic(Path::new(&vol.mount_path), &hash_hex, stored_bytes)?;
                    }
                }
            }
        }

        let is_new = match self
            .db
            .upsert_chunk(vault_id, expected_hash, raw_size, stored_size)
        {
            Ok(is_new) => is_new,
            Err(err) => {
                if existing.is_none() {
                    let _ = self.delete_chunk_files(vault_id, expected_hash);
                }
                return Err(err);
            }
        };
        if is_new {
            self.db
                .update_vault_used_bytes(vault_id, stored_size as i64)?;
        }
        Ok(is_new)
    }

    /// Reads a chunk from the vault and verifies its BLAKE3 integrity.
    ///
    /// Under `mirror_raid1`, if the primary copy is unreadable or corrupted, transparently
    /// reads from a healthy mirror and repairs the primary copy.
    pub fn get_chunk(&self, vault_id: &str, hash: &[u8; 32]) -> Result<Vec<u8>> {
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
                for vol in volumes.iter().filter(|volume| {
                    matches!(volume.role, VolumeRole::Primary | VolumeRole::Mirror)
                }) {
                    if let Ok(bytes) =
                        read_verified_chunk(Path::new(&vol.mount_path), &hash_hex, hash)
                    {
                        return Ok(bytes);
                    }
                }
                Err(LanVaultError::CorruptedChunk { hash: hash_hex })
            }
        }
    }

    pub fn has_verified_chunk(&self, vault_id: &str, hash: &[u8; 32]) -> Result<bool> {
        match self.get_chunk(vault_id, hash) {
            Ok(_) => Ok(true),
            Err(LanVaultError::NotFound(_)) | Err(LanVaultError::CorruptedChunk { .. }) => {
                Ok(false)
            }
            Err(err) => Err(err),
        }
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
    pub fn read_chunk_raw_on_primary(
        &self,
        vault_id: &str,
        hash: &[u8; 32],
    ) -> Result<Vec<u8>> {
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
            PoolMode::Single => vec![primary_volume(&volumes)?],
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
            PoolMode::MirrorRaid1 => candidates.len(),
            PoolMode::Single | PoolMode::Striped => 1,
        };
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
            if path.exists() {
                fs::remove_file(path)?;
            }
        }
        Ok(())
    }

    fn online_volumes(&self, vault_id: &str) -> Result<Vec<VaultVolumeRecord>> {
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
    fs::rename(&tmp_path, &final_path)?;
    Ok(())
}

/// Computes `<root>/chunks/<first_2_hex>/<full_hex>`.
pub fn chunk_file_path(volume_root: &Path, hash_hex: &str) -> PathBuf {
    let bucket = &hash_hex[..2];
    volume_root.join("chunks").join(bucket).join(hash_hex)
}

fn write_chunk_atomic(volume_root: &Path, hash_hex: &str, bytes: &[u8]) -> Result<()> {
    let staging_dir = volume_root.join("staging");
    fs::create_dir_all(&staging_dir)?;

    let final_path = chunk_file_path(volume_root, hash_hex);
    if let Some(parent) = final_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let tmp_path = staging_dir.join(format!("{}.{}.tmp", hash_hex, Uuid::new_v4()));
    {
        let mut file = File::create(&tmp_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
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
    let mut resolved = path.to_path_buf();
    if resolved.exists() {
        resolved = fs::canonicalize(&resolved)?;
    }
    Ok(resolved)
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn striped_target_volume<'a>(
    volumes: &'a [VaultVolumeRecord],
    hash: &[u8; 32],
) -> Result<&'a VaultVolumeRecord> {
    let idx = (hash[0] as usize) % volumes.len();
    Ok(&volumes[idx])
}

pub(crate) fn unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX_EPOCH")
        .as_secs() as i64
}
