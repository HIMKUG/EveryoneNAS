//! SQLite WAL persistence layer for LanVault.
//!
//! Implements the exact 7-table schema defined in the LanVault blueprint §5.3:
//! `trusted_peers`, `vaults`, `vault_volumes`, `chunks`, `snapshots`, `sync_rules`,
//! and `local_file_state`, along with indices, `PRAGMA wal_checkpoint(TRUNCATE)`,
//! and strongly typed CRUD methods.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{LanVaultError, Result};

/// Storage pool topology for a vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PoolMode {
    Single,
    Striped,
    MirrorRaid1,
}

impl PoolMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Striped => "striped",
            Self::MirrorRaid1 => "mirror_raid1",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "single" => Ok(Self::Single),
            "striped" => Ok(Self::Striped),
            "mirror_raid1" => Ok(Self::MirrorRaid1),
            other => Err(LanVaultError::InvalidInput(format!(
                "invalid pool_mode: {other}"
            ))),
        }
    }
}

/// Role of a physical volume within a vault pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeRole {
    Primary,
    Mirror,
    PoolExt,
}

impl VolumeRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Mirror => "mirror",
            Self::PoolExt => "pool_ext",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "primary" => Ok(Self::Primary),
            "mirror" => Ok(Self::Mirror),
            "pool_ext" => Ok(Self::PoolExt),
            other => Err(LanVaultError::InvalidInput(format!(
                "invalid volume role: {other}"
            ))),
        }
    }
}

/// Bit-Rot scrub health status for a stored chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrubStatus {
    Ok,
    Corrupted,
    Healed,
}

impl ScrubStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Corrupted => "corrupted",
            Self::Healed => "healed",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "ok" => Ok(Self::Ok),
            "corrupted" => Ok(Self::Corrupted),
            "healed" => Ok(Self::Healed),
            other => Err(LanVaultError::InvalidInput(format!(
                "invalid scrub_status: {other}"
            ))),
        }
    }
}

/// GFS snapshot retention tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionTier {
    Hourly,
    Daily,
    Weekly,
    Monthly,
    Pinned,
}

impl RetentionTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hourly => "hourly",
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
            Self::Pinned => "pinned",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "hourly" => Ok(Self::Hourly),
            "daily" => Ok(Self::Daily),
            "weekly" => Ok(Self::Weekly),
            "monthly" => Ok(Self::Monthly),
            "pinned" => Ok(Self::Pinned),
            other => Err(LanVaultError::InvalidInput(format!(
                "invalid retention_tier: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedPeer {
    pub device_id: String,
    pub device_name: String,
    pub os_type: String,
    pub ed25519_pubkey: Vec<u8>,
    pub last_ip: Option<String>,
    pub last_port: Option<u16>,
    pub auto_connect: bool,
    pub paired_at: i64,
    pub revoked_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultRecord {
    pub vault_id: String,
    pub name: String,
    pub pool_mode: PoolMode,
    pub quota_bytes: u64,
    pub used_bytes: u64,
    pub e2ee_enabled: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultVolumeRecord {
    pub volume_id: String,
    pub vault_id: String,
    pub mount_path: String,
    pub volume_guid: Option<String>,
    pub role: VolumeRole,
    pub is_online: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkRecord {
    pub vault_id: String,
    pub hash_blake3: [u8; 32],
    pub raw_size: u64,
    pub stored_size: u64,
    pub ref_count: i64,
    pub last_scrub_at: Option<i64>,
    pub scrub_status: ScrubStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotRecord {
    pub snapshot_id: String,
    pub vault_id: String,
    pub rule_id: String,
    pub device_id: String,
    pub root_tree_hash: [u8; 32],
    pub retention_tier: RetentionTier,
    pub file_count: u64,
    pub total_bytes: u64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRuleRecord {
    pub rule_id: String,
    pub local_dir: String,
    pub target_vault_id: String,
    pub wifi_only: bool,
    pub allowed_ssids: Option<String>,
    pub require_charging: bool,
    pub max_kbps: u32,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalFileStateRecord {
    pub rule_id: String,
    pub target_vault_id: String,
    pub rel_path: String,
    pub size: u64,
    pub mtime_ns: i64,
    pub file_id: Option<i64>,
    pub chunk_hashes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferHistoryRecord {
    pub id: i64,
    pub direction: String,
    pub kind: String,
    pub title: String,
    pub subtitle: String,
    pub peer_name: String,
    pub content: Option<String>,
    pub size_bytes: u64,
    pub created_at: i64,
}

/// Result of `PRAGMA wal_checkpoint(TRUNCATE)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalCheckpointReport {
    pub busy: i32,
    pub log_frames: i32,
    pub checkpointed_frames: i32,
}

/// Thread-safe SQLite WAL database handle.
pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    /// Opens (or creates) a SQLite database at `path`, enables WAL mode and foreign keys,
    /// and initializes all 7 schema tables and indices.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path)?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.configure_and_migrate()?;
        Ok(db)
    }

    /// Opens an in-memory SQLite database (useful for unit and integration tests).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.configure_and_migrate()?;
        Ok(db)
    }

    fn lock_conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("database mutex poisoned")
    }

    fn configure_and_migrate(&self) -> Result<()> {
        let conn = self.lock_conn();
        #[cfg(target_os = "android")]
        const MEMORY_PRAGMAS: &str = "PRAGMA cache_size = -16384;
PRAGMA mmap_size = 0;";
        #[cfg(not(target_os = "android"))]
        const MEMORY_PRAGMAS: &str = "PRAGMA cache_size = -65536;
PRAGMA mmap_size = 268435456;";
        conn.execute_batch(MEMORY_PRAGMAS)?;
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = FULL;
            PRAGMA foreign_keys = ON;
            PRAGMA busy_timeout = 5000;
            PRAGMA temp_store = MEMORY;
            PRAGMA wal_autocheckpoint = 4000;

            CREATE TABLE IF NOT EXISTS trusted_peers (
                device_id       TEXT PRIMARY KEY,
                device_name     TEXT NOT NULL,
                os_type         TEXT NOT NULL,
                ed25519_pubkey  BLOB NOT NULL,
                last_ip         TEXT,
                last_port       INTEGER,
                auto_connect    INTEGER NOT NULL DEFAULT 1,
                paired_at       INTEGER NOT NULL,
                revoked_at      INTEGER
            );

            CREATE TABLE IF NOT EXISTS vaults (
                vault_id        TEXT PRIMARY KEY,
                name            TEXT NOT NULL,
                pool_mode       TEXT NOT NULL DEFAULT 'single',
                quota_bytes     INTEGER NOT NULL,
                used_bytes      INTEGER NOT NULL DEFAULT 0,
                e2ee_enabled    INTEGER NOT NULL DEFAULT 1,
                created_at      INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS vault_volumes (
                volume_id       TEXT PRIMARY KEY,
                vault_id        TEXT NOT NULL REFERENCES vaults(vault_id) ON DELETE CASCADE,
                mount_path      TEXT NOT NULL,
                volume_guid     TEXT,
                role            TEXT NOT NULL DEFAULT 'primary',
                is_online       INTEGER NOT NULL DEFAULT 1
            );

            CREATE TABLE IF NOT EXISTS chunks (
                vault_id        TEXT NOT NULL REFERENCES vaults(vault_id) ON DELETE CASCADE,
                hash_blake3     BLOB NOT NULL,
                raw_size        INTEGER NOT NULL,
                stored_size     INTEGER NOT NULL,
                ref_count       INTEGER NOT NULL DEFAULT 1,
                last_scrub_at   INTEGER,
                scrub_status    TEXT NOT NULL DEFAULT 'ok',
                PRIMARY KEY (vault_id, hash_blake3)
            );

            CREATE TABLE IF NOT EXISTS snapshots (
                snapshot_id     TEXT PRIMARY KEY,
                vault_id        TEXT NOT NULL REFERENCES vaults(vault_id),
                rule_id         TEXT NOT NULL DEFAULT '',
                device_id       TEXT NOT NULL REFERENCES trusted_peers(device_id),
                root_tree_hash  BLOB NOT NULL,
                retention_tier  TEXT NOT NULL DEFAULT 'daily',
                file_count      INTEGER NOT NULL,
                total_bytes     INTEGER NOT NULL,
                created_at      INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS sync_rules (
                rule_id             TEXT PRIMARY KEY,
                local_dir           TEXT NOT NULL,
                target_vault_id     TEXT NOT NULL,
                wifi_only           INTEGER NOT NULL DEFAULT 1,
                allowed_ssids       TEXT,
                require_charging    INTEGER NOT NULL DEFAULT 0,
                max_kbps            INTEGER NOT NULL DEFAULT 0,
                enabled             INTEGER NOT NULL DEFAULT 1
            );

            CREATE TABLE IF NOT EXISTS local_file_state (
                rule_id         TEXT NOT NULL,
                target_vault_id TEXT NOT NULL DEFAULT '',
                rel_path        TEXT NOT NULL,
                size            INTEGER NOT NULL,
                mtime_ns        INTEGER NOT NULL,
                file_id         INTEGER,
                chunk_hashes    BLOB NOT NULL,
                PRIMARY KEY (rule_id, rel_path)
            );

            CREATE TABLE IF NOT EXISTS app_settings (
                key             TEXT PRIMARY KEY,
                value           TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS snapshot_chunks (
                snapshot_id TEXT NOT NULL REFERENCES snapshots(snapshot_id) ON DELETE CASCADE,
                vault_id    TEXT NOT NULL,
                chunk_hash  BLOB NOT NULL,
                PRIMARY KEY (snapshot_id, chunk_hash)
            );

            CREATE TABLE IF NOT EXISTS revoked_sessions (
                token_hash TEXT PRIMARY KEY,
                expires_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS transfer_history (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                direction   TEXT NOT NULL,
                kind        TEXT NOT NULL,
                title       TEXT NOT NULL,
                subtitle    TEXT NOT NULL,
                peer_name   TEXT NOT NULL,
                content     TEXT,
                size_bytes  INTEGER NOT NULL DEFAULT 0,
                created_at  INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_vault_volumes_vault_id ON vault_volumes(vault_id);
            CREATE INDEX IF NOT EXISTS idx_chunks_scrub_status ON chunks(scrub_status);
            CREATE INDEX IF NOT EXISTS idx_chunks_vault_scrub_status ON chunks(vault_id, scrub_status);
            CREATE INDEX IF NOT EXISTS idx_chunks_vault_ref_count ON chunks(vault_id, ref_count);
            CREATE INDEX IF NOT EXISTS idx_snapshots_vault_created ON snapshots(vault_id, created_at DESC);
            CREATE INDEX IF NOT EXISTS idx_snapshots_retention ON snapshots(vault_id, retention_tier);
            CREATE INDEX IF NOT EXISTS idx_local_file_state_rule ON local_file_state(rule_id);
            CREATE INDEX IF NOT EXISTS idx_snapshot_chunks_vault_hash
                ON snapshot_chunks(vault_id, chunk_hash);
            "#,
        )?;
        ensure_column(
            &conn,
            "trusted_peers",
            "revoked_at",
            "revoked_at INTEGER",
        )?;
        ensure_column(
            &conn,
            "snapshots",
            "rule_id",
            "rule_id TEXT NOT NULL DEFAULT ''",
        )?;
        ensure_column(
            &conn,
            "local_file_state",
            "target_vault_id",
            "target_vault_id TEXT NOT NULL DEFAULT ''",
        )?;
        Ok(())
    }

    /// Executes `PRAGMA wal_checkpoint(TRUNCATE)` before graceful restart or shutdown.
    pub fn wal_checkpoint_truncate(&self) -> Result<WalCheckpointReport> {
        let _probe = crate::debug_telemetry::ProbeGuard::start(
            "core.db",
            "db.wal_checkpoint",
            None,
        );
        let conn = self.lock_conn();
        let _ = conn.execute_batch("PRAGMA optimize;");
        let report = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE);", [], |row| {
            Ok(WalCheckpointReport {
                busy: row.get(0)?,
                log_frames: row.get(1)?,
                checkpointed_frames: row.get(2)?,
            })
        })?;
        if report.busy != 0 {
            return Err(LanVaultError::InvalidState(format!(
                "WAL checkpoint remained busy with {} frames",
                report.log_frames
            )));
        }
        Ok(report)
    }

    // -------------------------------------------------------------------------
    // trusted_peers CRUD
    // -------------------------------------------------------------------------

    pub fn upsert_trusted_peer(&self, peer: &TrustedPeer) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            r#"
            INSERT INTO trusted_peers (
                device_id, device_name, os_type, ed25519_pubkey,
                last_ip, last_port, auto_connect, paired_at, revoked_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(device_id) DO UPDATE SET
                device_name = excluded.device_name,
                os_type = excluded.os_type,
                ed25519_pubkey = excluded.ed25519_pubkey,
                last_ip = excluded.last_ip,
                last_port = excluded.last_port,
                auto_connect = excluded.auto_connect,
                revoked_at = excluded.revoked_at
            "#,
            params![
                peer.device_id,
                peer.device_name,
                peer.os_type,
                peer.ed25519_pubkey,
                peer.last_ip,
                peer.last_port.map(i64::from),
                i64::from(peer.auto_connect),
                peer.paired_at,
                peer.revoked_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_trusted_peer(&self, device_id: &str) -> Result<Option<TrustedPeer>> {
        let conn = self.lock_conn();
        let row = conn
            .query_row(
                r#"
                SELECT device_id, device_name, os_type, ed25519_pubkey,
                       last_ip, last_port, auto_connect, paired_at, revoked_at
                FROM trusted_peers
                WHERE device_id = ?1 AND revoked_at IS NULL
                "#,
                params![device_id],
                row_to_trusted_peer,
            )
            .optional()?;
        Ok(row)
    }

    pub fn list_trusted_peers(&self) -> Result<Vec<TrustedPeer>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            r#"
            SELECT device_id, device_name, os_type, ed25519_pubkey,
                   last_ip, last_port, auto_connect, paired_at, revoked_at
            FROM trusted_peers
            WHERE revoked_at IS NULL
            ORDER BY paired_at DESC
            "#,
        )?;
        let rows = stmt
            .query_map([], row_to_trusted_peer)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn delete_trusted_peer(&self, device_id: &str) -> Result<bool> {
        let conn = self.lock_conn();
        let affected = conn.execute(
            "UPDATE trusted_peers
             SET revoked_at = ?1, auto_connect = 0
             WHERE device_id = ?2 AND revoked_at IS NULL",
            params![unix_timestamp_secs(), device_id],
        )?;
        Ok(affected > 0)
    }

    // -------------------------------------------------------------------------
    // vaults & vault_volumes CRUD
    // -------------------------------------------------------------------------

    pub fn insert_vault(&self, vault: &VaultRecord, volumes: &[VaultVolumeRecord]) -> Result<()> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        tx.execute(
            r#"
            INSERT INTO vaults (
                vault_id, name, pool_mode, quota_bytes, used_bytes, e2ee_enabled, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                vault.vault_id,
                vault.name,
                vault.pool_mode.as_str(),
                vault.quota_bytes as i64,
                vault.used_bytes as i64,
                i64::from(vault.e2ee_enabled),
                vault.created_at,
            ],
        )?;
        for vol in volumes {
            tx.execute(
                r#"
                INSERT INTO vault_volumes (
                    volume_id, vault_id, mount_path, volume_guid, role, is_online
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
                params![
                    vol.volume_id,
                    vol.vault_id,
                    vol.mount_path,
                    vol.volume_guid,
                    vol.role.as_str(),
                    i64::from(vol.is_online),
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_vault(&self, vault_id: &str) -> Result<Option<VaultRecord>> {
        let conn = self.lock_conn();
        let row = conn
            .query_row(
                r#"
                SELECT vault_id, name, pool_mode, quota_bytes, used_bytes, e2ee_enabled, created_at
                FROM vaults
                WHERE vault_id = ?1
                "#,
                params![vault_id],
                row_to_vault,
            )
            .optional()?;
        Ok(row)
    }

    pub fn list_vaults(&self) -> Result<Vec<VaultRecord>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            r#"
            SELECT vault_id, name, pool_mode, quota_bytes, used_bytes, e2ee_enabled, created_at
            FROM vaults
            ORDER BY created_at ASC
            "#,
        )?;
        let rows = stmt
            .query_map([], row_to_vault)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn list_vault_volumes(&self, vault_id: &str) -> Result<Vec<VaultVolumeRecord>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            r#"
            SELECT volume_id, vault_id, mount_path, volume_guid, role, is_online
            FROM vault_volumes
            WHERE vault_id = ?1
            ORDER BY role ASC, volume_id ASC
            "#,
        )?;
        let rows = stmt
            .query_map(params![vault_id], row_to_vault_volume)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn update_vault_used_bytes(&self, vault_id: &str, delta_bytes: i64) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE vaults SET used_bytes = MAX(0, used_bytes + ?1) WHERE vault_id = ?2",
            params![delta_bytes, vault_id],
        )?;
        Ok(())
    }

    pub fn set_vault_used_bytes(&self, vault_id: &str, used_bytes: u64) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE vaults SET used_bytes = ?1 WHERE vault_id = ?2",
            params![used_bytes as i64, vault_id],
        )?;
        Ok(())
    }

    pub fn update_volume_mount_path(
        &self,
        volume_id: &str,
        mount_path: &str,
        is_online: bool,
    ) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE vault_volumes SET mount_path = ?1, is_online = ?2 WHERE volume_id = ?3",
            params![mount_path, i64::from(is_online), volume_id],
        )?;
        Ok(())
    }

    pub fn update_volume_guid(&self, volume_id: &str, volume_guid: &str) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE vault_volumes SET volume_guid = ?1 WHERE volume_id = ?2",
            params![volume_guid, volume_id],
        )?;
        Ok(())
    }

    // -------------------------------------------------------------------------
    // chunks CRUD & Bit-Rot / GC helpers (scoped by vault_id)
    // -------------------------------------------------------------------------

    pub fn upsert_chunk(
        &self,
        vault_id: &str,
        hash_blake3: &[u8; 32],
        raw_size: u64,
        stored_size: u64,
    ) -> Result<bool> {
        let conn = self.lock_conn();
        let existing: Option<i64> = conn
            .query_row(
                "SELECT ref_count FROM chunks WHERE vault_id = ?1 AND hash_blake3 = ?2",
                params![vault_id, hash_blake3.as_slice()],
                |row| row.get(0),
            )
            .optional()?;

        if existing.is_some() {
            conn.execute(
                "UPDATE chunks
                 SET raw_size = ?1, stored_size = ?2, scrub_status = 'ok', last_scrub_at = NULL
                 WHERE vault_id = ?3 AND hash_blake3 = ?4",
                params![
                    raw_size as i64,
                    stored_size as i64,
                    vault_id,
                    hash_blake3.as_slice()
                ],
            )?;
            Ok(false)
        } else {
            conn.execute(
                r#"
                INSERT INTO chunks (vault_id, hash_blake3, raw_size, stored_size, ref_count, scrub_status)
                VALUES (?1, ?2, ?3, ?4, 0, 'ok')
                "#,
                params![
                    vault_id,
                    hash_blake3.as_slice(),
                    raw_size as i64,
                    stored_size as i64
                ],
            )?;
            Ok(true)
        }
    }

    pub fn insert_chunk_if_missing(
        &self,
        vault_id: &str,
        hash_blake3: &[u8; 32],
        raw_size: u64,
        stored_size: u64,
    ) -> Result<bool> {
        let conn = self.lock_conn();
        let affected = conn.execute(
            "INSERT OR IGNORE INTO chunks
                (vault_id, hash_blake3, raw_size, stored_size, ref_count, scrub_status)
             VALUES (?1, ?2, ?3, ?4, 0, 'ok')",
            params![
                vault_id,
                hash_blake3.as_slice(),
                raw_size as i64,
                stored_size as i64
            ],
        )?;
        Ok(affected == 1)
    }

    pub fn get_chunk(&self, vault_id: &str, hash_blake3: &[u8; 32]) -> Result<Option<ChunkRecord>> {
        let conn = self.lock_conn();
        let row = conn
            .query_row(
                r#"
                SELECT vault_id, hash_blake3, raw_size, stored_size, ref_count, last_scrub_at, scrub_status
                FROM chunks
                WHERE vault_id = ?1 AND hash_blake3 = ?2
                "#,
                params![vault_id, hash_blake3.as_slice()],
                row_to_chunk,
            )
            .optional()?;
        Ok(row)
    }

    pub fn list_all_chunks(&self, vault_id: &str) -> Result<Vec<ChunkRecord>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            r#"
            SELECT vault_id, hash_blake3, raw_size, stored_size, ref_count, last_scrub_at, scrub_status
            FROM chunks
            WHERE vault_id = ?1
            "#,
        )?;
        let rows = stmt
            .query_map(params![vault_id], row_to_chunk)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn list_corrupted_chunks(&self, vault_id: &str) -> Result<Vec<ChunkRecord>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            r#"
            SELECT vault_id, hash_blake3, raw_size, stored_size, ref_count, last_scrub_at, scrub_status
            FROM chunks
            WHERE vault_id = ?1 AND scrub_status = 'corrupted'
            "#,
        )?;
        let rows = stmt
            .query_map(params![vault_id], row_to_chunk)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn count_corrupted_chunks(&self) -> Result<u64> {
        let conn = self.lock_conn();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM chunks WHERE scrub_status = 'corrupted'",
            [],
            |row| row.get(0),
        )?;
        Ok(count as u64)
    }

    pub fn update_chunk_scrub_status(
        &self,
        vault_id: &str,
        hash_blake3: &[u8; 32],
        status: ScrubStatus,
        scrubbed_at: i64,
    ) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE chunks SET scrub_status = ?1, last_scrub_at = ?2 WHERE vault_id = ?3 AND hash_blake3 = ?4",
            params![status.as_str(), scrubbed_at, vault_id, hash_blake3.as_slice()],
        )?;
        Ok(())
    }

    pub fn decrement_chunk_ref(&self, vault_id: &str, hash_blake3: &[u8; 32]) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE chunks SET ref_count = MAX(0, ref_count - 1) WHERE vault_id = ?1 AND hash_blake3 = ?2",
            params![vault_id, hash_blake3.as_slice()],
        )?;
        Ok(())
    }

    pub fn list_orphan_chunks(&self, vault_id: &str) -> Result<Vec<ChunkRecord>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            r#"
            SELECT vault_id, hash_blake3, raw_size, stored_size, ref_count, last_scrub_at, scrub_status
            FROM chunks
            WHERE vault_id = ?1 AND ref_count <= 0
            "#,
        )?;
        let rows = stmt
            .query_map(params![vault_id], row_to_chunk)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn delete_chunk(&self, vault_id: &str, hash_blake3: &[u8; 32]) -> Result<bool> {
        let conn = self.lock_conn();
        let affected = conn.execute(
            "DELETE FROM chunks WHERE vault_id = ?1 AND hash_blake3 = ?2 AND ref_count <= 0",
            params![vault_id, hash_blake3.as_slice()],
        )?;
        Ok(affected > 0)
    }

    // -------------------------------------------------------------------------
    // snapshots CRUD
    // -------------------------------------------------------------------------

    pub fn insert_snapshot(&self, snap: &SnapshotRecord) -> Result<()> {
        self.insert_snapshot_with_chunks(snap, &[])?;
        Ok(())
    }

    pub fn insert_snapshot_with_chunks(
        &self,
        snap: &SnapshotRecord,
        chunk_hashes: &[[u8; 32]],
    ) -> Result<bool> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        let existing = tx
            .query_row(
                "SELECT vault_id, rule_id, device_id, root_tree_hash, retention_tier,
                        file_count, total_bytes, created_at
                 FROM snapshots WHERE snapshot_id = ?1",
                params![snap.snapshot_id],
                |row| {
                    let root: Vec<u8> = row.get(3)?;
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        root,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                    ))
                },
            )
            .optional()?;
        if let Some(existing) = existing {
            let identical = existing.0 == snap.vault_id
                && existing.1 == snap.rule_id
                && existing.2 == snap.device_id
                && existing.3 == snap.root_tree_hash
                && existing.4 == snap.retention_tier.as_str()
                && existing.5 == snap.file_count as i64
                && existing.6 == snap.total_bytes as i64
                && existing.7 == snap.created_at;
            if !identical {
                return Err(LanVaultError::InvalidInput(format!(
                    "snapshot_id {} already exists with different contents",
                    snap.snapshot_id
                )));
            }
            for hash in chunk_hashes {
                let inserted = tx.execute(
                    "INSERT OR IGNORE INTO snapshot_chunks
                        (snapshot_id, vault_id, chunk_hash)
                     VALUES (?1, ?2, ?3)",
                    params![snap.snapshot_id, snap.vault_id, hash.as_slice()],
                )?;
                if inserted == 1 {
                    tx.execute(
                        "UPDATE chunks SET ref_count = ref_count + 1
                         WHERE vault_id = ?1 AND hash_blake3 = ?2",
                        params![snap.vault_id, hash.as_slice()],
                    )?;
                }
            }
            tx.commit()?;
            return Ok(false);
        }
        tx.execute(
            r#"
            INSERT INTO snapshots (
                snapshot_id, vault_id, rule_id, device_id, root_tree_hash,
                retention_tier, file_count, total_bytes, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
            params![
                snap.snapshot_id,
                snap.vault_id,
                snap.rule_id,
                snap.device_id,
                snap.root_tree_hash.as_slice(),
                snap.retention_tier.as_str(),
                snap.file_count as i64,
                snap.total_bytes as i64,
                snap.created_at,
            ],
        )?;
        for hash in chunk_hashes {
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO snapshot_chunks (snapshot_id, vault_id, chunk_hash)
                 VALUES (?1, ?2, ?3)",
                params![snap.snapshot_id, snap.vault_id, hash.as_slice()],
            )?;
            if inserted == 1 {
                let affected = tx.execute(
                    "UPDATE chunks SET ref_count = ref_count + 1
                     WHERE vault_id = ?1 AND hash_blake3 = ?2",
                    params![snap.vault_id, hash.as_slice()],
                )?;
                if affected != 1 {
                    return Err(LanVaultError::NotFound(format!(
                        "chunk {} for snapshot {}",
                        hex::encode(hash),
                        snap.snapshot_id
                    )));
                }
            }
        }
        tx.commit()?;
        Ok(true)
    }

    pub fn get_snapshot(&self, snapshot_id: &str) -> Result<Option<SnapshotRecord>> {
        let conn = self.lock_conn();
        let row = conn
            .query_row(
                r#"
                SELECT snapshot_id, vault_id, rule_id, device_id, root_tree_hash,
                       retention_tier, file_count, total_bytes, created_at
                FROM snapshots
                WHERE snapshot_id = ?1
                "#,
                params![snapshot_id],
                row_to_snapshot,
            )
            .optional()?;
        Ok(row)
    }

    pub fn list_snapshots(&self, vault_id: Option<&str>) -> Result<Vec<SnapshotRecord>> {
        let conn = self.lock_conn();
        match vault_id {
            Some(vid) => {
                let mut stmt = conn.prepare(
                    r#"
                    SELECT snapshot_id, vault_id, rule_id, device_id, root_tree_hash,
                           retention_tier, file_count, total_bytes, created_at
                    FROM snapshots
                    WHERE vault_id = ?1
                    ORDER BY created_at DESC
                    "#,
                )?;
                let rows = stmt
                    .query_map(params![vid], row_to_snapshot)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(rows)
            }
            None => {
                let mut stmt = conn.prepare(
                    r#"
                    SELECT snapshot_id, vault_id, rule_id, device_id, root_tree_hash,
                           retention_tier, file_count, total_bytes, created_at
                    FROM snapshots
                    ORDER BY created_at DESC
                    "#,
                )?;
                let rows = stmt
                    .query_map([], row_to_snapshot)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(rows)
            }
        }
    }

    pub fn delete_snapshot(&self, snapshot_id: &str) -> Result<bool> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE chunks
             SET ref_count = MAX(
                 0,
                 ref_count - (
                     SELECT COUNT(*) FROM snapshot_chunks sc
                     WHERE sc.snapshot_id = ?1
                       AND sc.vault_id = chunks.vault_id
                       AND sc.chunk_hash = chunks.hash_blake3
                 )
             )
             WHERE EXISTS (
                 SELECT 1 FROM snapshot_chunks sc
                 WHERE sc.snapshot_id = ?1
                   AND sc.vault_id = chunks.vault_id
                   AND sc.chunk_hash = chunks.hash_blake3
             )",
            params![snapshot_id],
        )?;
        tx.execute(
            "DELETE FROM snapshot_chunks WHERE snapshot_id = ?1",
            params![snapshot_id],
        )?;
        let affected = tx.execute(
            "DELETE FROM snapshots WHERE snapshot_id = ?1",
            params![snapshot_id],
        )?;
        tx.commit()?;
        Ok(affected > 0)
    }

    // -------------------------------------------------------------------------
    // sync_rules CRUD
    // -------------------------------------------------------------------------

    pub fn upsert_sync_rule(&self, rule: &SyncRuleRecord) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            r#"
            INSERT INTO sync_rules (
                rule_id, local_dir, target_vault_id, wifi_only,
                allowed_ssids, require_charging, max_kbps, enabled
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(rule_id) DO UPDATE SET
                local_dir = excluded.local_dir,
                target_vault_id = excluded.target_vault_id,
                wifi_only = excluded.wifi_only,
                allowed_ssids = excluded.allowed_ssids,
                require_charging = excluded.require_charging,
                max_kbps = excluded.max_kbps,
                enabled = excluded.enabled
            "#,
            params![
                rule.rule_id,
                rule.local_dir,
                rule.target_vault_id,
                i64::from(rule.wifi_only),
                rule.allowed_ssids,
                i64::from(rule.require_charging),
                i64::from(rule.max_kbps),
                i64::from(rule.enabled),
            ],
        )?;
        Ok(())
    }

    pub fn get_sync_rule(&self, rule_id: &str) -> Result<Option<SyncRuleRecord>> {
        let conn = self.lock_conn();
        let row = conn
            .query_row(
                r#"
                SELECT rule_id, local_dir, target_vault_id, wifi_only,
                       allowed_ssids, require_charging, max_kbps, enabled
                FROM sync_rules
                WHERE rule_id = ?1
                "#,
                params![rule_id],
                row_to_sync_rule,
            )
            .optional()?;
        Ok(row)
    }

    pub fn list_sync_rules(&self) -> Result<Vec<SyncRuleRecord>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            r#"
            SELECT rule_id, local_dir, target_vault_id, wifi_only,
                   allowed_ssids, require_charging, max_kbps, enabled
            FROM sync_rules
            ORDER BY rule_id ASC
            "#,
        )?;
        let rows = stmt
            .query_map([], row_to_sync_rule)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn delete_sync_rule(&self, rule_id: &str) -> Result<bool> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM local_file_state WHERE rule_id = ?1",
            params![rule_id],
        )?;
        let affected = tx.execute(
            "DELETE FROM sync_rules WHERE rule_id = ?1",
            params![rule_id],
        )?;
        tx.commit()?;
        Ok(affected > 0)
    }

    // -------------------------------------------------------------------------
    // local_file_state CRUD
    // -------------------------------------------------------------------------

    pub fn upsert_local_file_state(&self, state: &LocalFileStateRecord) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            r#"
            INSERT INTO local_file_state (
                rule_id, target_vault_id, rel_path, size, mtime_ns, file_id, chunk_hashes
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(rule_id, rel_path) DO UPDATE SET
                target_vault_id = excluded.target_vault_id,
                size = excluded.size,
                mtime_ns = excluded.mtime_ns,
                file_id = excluded.file_id,
                chunk_hashes = excluded.chunk_hashes
            "#,
            params![
                state.rule_id,
                state.target_vault_id,
                state.rel_path,
                state.size as i64,
                state.mtime_ns,
                state.file_id,
                state.chunk_hashes,
            ],
        )?;
        Ok(())
    }

    pub fn list_local_file_states(&self, rule_id: &str) -> Result<Vec<LocalFileStateRecord>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            r#"
            SELECT rule_id, target_vault_id, rel_path, size, mtime_ns, file_id, chunk_hashes
            FROM local_file_state
            WHERE rule_id = ?1
            "#,
        )?;
        let rows = stmt
            .query_map(params![rule_id], row_to_local_file_state)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn delete_local_file_state(&self, rule_id: &str, rel_path: &str) -> Result<bool> {
        let conn = self.lock_conn();
        let affected = conn.execute(
            "DELETE FROM local_file_state WHERE rule_id = ?1 AND rel_path = ?2",
            params![rule_id, rel_path],
        )?;
        Ok(affected > 0)
    }

    pub fn delete_local_file_states(&self, rule_id: &str, rel_paths: &[String]) -> Result<usize> {
        if rel_paths.is_empty() {
            return Ok(0);
        }
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        let mut total_deleted = 0usize;
        {
            let mut stmt = tx.prepare(
                "DELETE FROM local_file_state WHERE rule_id = ?1 AND rel_path = ?2",
            )?;
            for rel_path in rel_paths {
                total_deleted += stmt.execute(params![rule_id, rel_path])?;
            }
        }
        tx.commit()?;
        Ok(total_deleted)
    }

    pub fn clear_local_file_states(&self) -> Result<usize> {
        let conn = self.lock_conn();
        Ok(conn.execute("DELETE FROM local_file_state", [])?)
    }

    // -------------------------------------------------------------------------
    // app_settings KV store
    // -------------------------------------------------------------------------


    /// Batch-updates scrub statuses in a single SQLite transaction (eliminates N+1 commits during parallel scrub).
    pub fn update_chunk_scrub_statuses_batch(
        &self,
        vault_id: &str,
        updates: &[([u8; 32], ScrubStatus)],
        scrubbed_at: i64,
    ) -> Result<usize> {
        if updates.is_empty() {
            return Ok(0);
        }
        let _probe = crate::debug_telemetry::ProbeGuard::start(
            "core.db",
            "db.batch_scrub_update",
            Some(format!("vault_id={vault_id}, count={}", updates.len())),
        );
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        let mut total = 0usize;
        {
            let mut stmt = tx.prepare_cached(
                "UPDATE chunks SET scrub_status = ?1, last_scrub_at = ?2 WHERE vault_id = ?3 AND hash_blake3 = ?4",
            )?;
            for (hash, status) in updates {
                total += stmt.execute(params![status.as_str(), scrubbed_at, vault_id, hash.as_slice()])?;
            }
        }
        tx.commit()?;
        Ok(total)
    }

    /// Batch-deletes expired snapshots in a single transaction (eliminates N+1 commits in GC).
    pub fn delete_snapshots_batch(&self, snapshot_ids: &[String]) -> Result<usize> {
        if snapshot_ids.is_empty() {
            return Ok(0);
        }
        let _probe = crate::debug_telemetry::ProbeGuard::start(
            "core.db",
            "db.batch_delete_snapshots",
            Some(format!("count={}", snapshot_ids.len())),
        );
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        let mut total = 0usize;
        for id in snapshot_ids {
            tx.execute(
                "UPDATE chunks
                 SET ref_count = MAX(
                     0,
                     ref_count - (
                         SELECT COUNT(*) FROM snapshot_chunks sc
                         WHERE sc.snapshot_id = ?1
                           AND sc.vault_id = chunks.vault_id
                           AND sc.chunk_hash = chunks.hash_blake3
                     )
                 )
                 WHERE EXISTS (
                     SELECT 1 FROM snapshot_chunks sc
                     WHERE sc.snapshot_id = ?1
                       AND sc.vault_id = chunks.vault_id
                       AND sc.chunk_hash = chunks.hash_blake3
                 )",
                params![id],
            )?;
            tx.execute(
                "DELETE FROM snapshot_chunks WHERE snapshot_id = ?1",
                params![id],
            )?;
            total += tx.execute(
                "DELETE FROM snapshots WHERE snapshot_id = ?1",
                params![id],
            )?;
        }
        tx.commit()?;
        Ok(total)
    }

    /// Batch-deletes orphan chunks in a single transaction (eliminates N+1 commits in GC).
    pub fn delete_chunks_batch(&self, vault_id: &str, hashes: &[[u8; 32]]) -> Result<usize> {
        if hashes.is_empty() {
            return Ok(0);
        }
        let _probe = crate::debug_telemetry::ProbeGuard::start(
            "core.db",
            "db.batch_delete_chunks",
            Some(format!("vault_id={vault_id}, count={}", hashes.len())),
        );
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        let mut total = 0usize;
        {
            let mut stmt = tx.prepare_cached(
                "DELETE FROM chunks
                 WHERE vault_id = ?1 AND hash_blake3 = ?2 AND ref_count <= 0
                   AND NOT EXISTS (
                       SELECT 1 FROM snapshot_chunks sc
                       WHERE sc.vault_id = ?1 AND sc.chunk_hash = ?2
                   )",
            )?;
            for h in hashes {
                total += stmt.execute(params![vault_id, h.as_slice()])?;
            }
        }
        tx.commit()?;
        Ok(total)
    }

    /// Batch-upserts local file states in a single transaction after sync_diff.
    pub fn upsert_local_file_states_batch(&self, states: &[LocalFileStateRecord]) -> Result<usize> {
        if states.is_empty() {
            return Ok(0);
        }
        let _probe = crate::debug_telemetry::ProbeGuard::start(
            "core.db",
            "db.batch_upsert_file_states",
            Some(format!("count={}", states.len())),
        );
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        let mut total = 0usize;
        {
            let mut stmt = tx.prepare_cached(
                r#"
                INSERT INTO local_file_state
                    (rule_id, target_vault_id, rel_path, size, mtime_ns, file_id, chunk_hashes)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(rule_id, rel_path) DO UPDATE SET
                    target_vault_id = excluded.target_vault_id,
                    size = excluded.size,
                    mtime_ns = excluded.mtime_ns,
                    file_id = excluded.file_id,
                    chunk_hashes = excluded.chunk_hashes
                "#,
            )?;
            for s in states {
                total += stmt.execute(params![
                    s.rule_id,
                    s.target_vault_id,
                    s.rel_path,
                    s.size as i64,
                    s.mtime_ns,
                    s.file_id,
                    s.chunk_hashes,
                ])?;
            }
        }
        tx.commit()?;
        Ok(total)
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let conn = self.lock_conn();
        let value = conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            r#"
            INSERT INTO app_settings (key, value)
            VALUES (?1, ?2)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            "#,
            params![key, value],
        )?;
        Ok(())
    }

    pub fn revoke_session(&self, token_hash: &str, expires_at: i64) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO revoked_sessions (token_hash, expires_at)
             VALUES (?1, ?2)
             ON CONFLICT(token_hash) DO UPDATE SET expires_at = excluded.expires_at",
            params![token_hash, expires_at],
        )?;
        Ok(())
    }

    pub fn is_session_revoked(&self, token_hash: &str, now: i64) -> Result<bool> {
        let conn = self.lock_conn();
        let found: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM revoked_sessions WHERE token_hash = ?1 AND expires_at > ?2",
                params![token_hash, now],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    pub fn list_snapshot_chunks(&self, snapshot_id: &str) -> Result<Vec<[u8; 32]>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT chunk_hash FROM snapshot_chunks WHERE snapshot_id = ?1 ORDER BY rowid ASC",
        )?;
        let rows = stmt.query_map(params![snapshot_id], |row| row.get::<_, Vec<u8>>(0))?;
        let mut hashes = Vec::new();
        for item in rows {
            let bytes = item?;
            let arr: [u8; 32] = bytes.try_into().map_err(|invalid: Vec<u8>| {
                LanVaultError::CorruptedChunk {
                    hash: format!("snapshot_chunks:{snapshot_id}:len={}", invalid.len()),
                }
            })?;
            hashes.push(arr);
        }
        Ok(hashes)
    }

    /// Returns `(unique_raw_bytes, unique_stored_bytes, logical_referenced_bytes)` across all vaults.
    pub fn get_chunk_storage_stats(&self) -> Result<(u64, u64, u64)> {
        let conn = self.lock_conn();
        let (raw_sum, stored_sum, logical_sum): (i64, i64, i64) = conn.query_row(
            "SELECT
                COALESCE(SUM(raw_size), 0),
                COALESCE(SUM(stored_size), 0),
                COALESCE(SUM(raw_size * MAX(1, ref_count)), 0)
             FROM chunks",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        Ok((
            raw_sum.max(0) as u64,
            stored_sum.max(0) as u64,
            logical_sum.max(0) as u64,
        ))
    }

    pub fn insert_transfer_record(
        &self,
        direction: &str,
        kind: &str,
        title: &str,
        subtitle: &str,
        peer_name: &str,
        content: Option<&str>,
        size_bytes: u64,
        created_at: i64,
    ) -> Result<i64> {
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO transfer_history (
                direction, kind, title, subtitle, peer_name, content, size_bytes, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                direction,
                kind,
                title,
                subtitle,
                peer_name,
                content,
                size_bytes as i64,
                created_at
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_transfer_records(&self, limit: usize) -> Result<Vec<TransferHistoryRecord>> {
        let conn = self.lock_conn();
        let capped = limit.clamp(1, 200) as i64;
        let mut stmt = conn.prepare(
            "SELECT id, direction, kind, title, subtitle, peer_name, content, size_bytes, created_at
             FROM transfer_history
             ORDER BY id DESC
             LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![capped], |row| {
                Ok(TransferHistoryRecord {
                    id: row.get(0)?,
                    direction: row.get(1)?,
                    kind: row.get(2)?,
                    title: row.get(3)?,
                    subtitle: row.get(4)?,
                    peer_name: row.get(5)?,
                    content: row.get(6)?,
                    size_bytes: row.get::<_, i64>(7)?.max(0) as u64,
                    created_at: row.get(8)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn prune_revoked_sessions(&self, now: i64) -> Result<usize> {
        let conn = self.lock_conn();
        let deleted = conn.execute(
            "DELETE FROM revoked_sessions WHERE expires_at <= ?1",
            params![now],
        )?;
        Ok(deleted)
    }
}

fn row_to_trusted_peer(row: &rusqlite::Row<'_>) -> rusqlite::Result<TrustedPeer> {
    let last_port: Option<i64> = row.get(5)?;
    let auto_connect: i64 = row.get(6)?;
    Ok(TrustedPeer {
        device_id: row.get(0)?,
        device_name: row.get(1)?,
        os_type: row.get(2)?,
        ed25519_pubkey: row.get(3)?,
        last_ip: row.get(4)?,
        last_port: last_port.map(|p| p as u16),
        auto_connect: auto_connect != 0,
        paired_at: row.get(7)?,
        revoked_at: row.get(8)?,
    })
}

fn row_to_vault(row: &rusqlite::Row<'_>) -> rusqlite::Result<VaultRecord> {
    let mode_str: String = row.get(2)?;
    let pool_mode = PoolMode::parse(&mode_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(e),
        )
    })?;
    let quota_bytes: i64 = row.get(3)?;
    let used_bytes: i64 = row.get(4)?;
    let e2ee: i64 = row.get(5)?;
    Ok(VaultRecord {
        vault_id: row.get(0)?,
        name: row.get(1)?,
        pool_mode,
        quota_bytes: quota_bytes as u64,
        used_bytes: used_bytes as u64,
        e2ee_enabled: e2ee != 0,
        created_at: row.get(6)?,
    })
}

fn row_to_vault_volume(row: &rusqlite::Row<'_>) -> rusqlite::Result<VaultVolumeRecord> {
    let role_str: String = row.get(4)?;
    let role = VolumeRole::parse(&role_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Text,
            Box::new(e),
        )
    })?;
    let is_online: i64 = row.get(5)?;
    Ok(VaultVolumeRecord {
        volume_id: row.get(0)?,
        vault_id: row.get(1)?,
        mount_path: row.get(2)?,
        volume_guid: row.get(3)?,
        role,
        is_online: is_online != 0,
    })
}

fn row_to_chunk(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChunkRecord> {
    let hash_vec: Vec<u8> = row.get(1)?;
    let hash_blake3: [u8; 32] = hash_vec.try_into().map_err(|v: Vec<u8>| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Blob,
            Box::new(LanVaultError::InvalidHashLength {
                expected: 32,
                actual: v.len(),
            }),
        )
    })?;
    let raw_size: i64 = row.get(2)?;
    let stored_size: i64 = row.get(3)?;
    let status_str: String = row.get(6)?;
    let scrub_status = ScrubStatus::parse(&status_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            6,
            rusqlite::types::Type::Text,
            Box::new(e),
        )
    })?;
    Ok(ChunkRecord {
        vault_id: row.get(0)?,
        hash_blake3,
        raw_size: raw_size as u64,
        stored_size: stored_size as u64,
        ref_count: row.get(4)?,
        last_scrub_at: row.get(5)?,
        scrub_status,
    })
}

fn row_to_snapshot(row: &rusqlite::Row<'_>) -> rusqlite::Result<SnapshotRecord> {
    let hash_vec: Vec<u8> = row.get(4)?;
    let root_tree_hash: [u8; 32] = hash_vec.try_into().map_err(|v: Vec<u8>| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Blob,
            Box::new(LanVaultError::InvalidHashLength {
                expected: 32,
                actual: v.len(),
            }),
        )
    })?;
    let tier_str: String = row.get(5)?;
    let retention_tier = RetentionTier::parse(&tier_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            5,
            rusqlite::types::Type::Text,
            Box::new(e),
        )
    })?;
    let file_count: i64 = row.get(6)?;
    let total_bytes: i64 = row.get(7)?;
    Ok(SnapshotRecord {
        snapshot_id: row.get(0)?,
        vault_id: row.get(1)?,
        rule_id: row.get(2)?,
        device_id: row.get(3)?,
        root_tree_hash,
        retention_tier,
        file_count: file_count as u64,
        total_bytes: total_bytes as u64,
        created_at: row.get(8)?,
    })
}

fn row_to_sync_rule(row: &rusqlite::Row<'_>) -> rusqlite::Result<SyncRuleRecord> {
    let wifi_only: i64 = row.get(3)?;
    let require_charging: i64 = row.get(5)?;
    let max_kbps: i64 = row.get(6)?;
    let enabled: i64 = row.get(7)?;
    Ok(SyncRuleRecord {
        rule_id: row.get(0)?,
        local_dir: row.get(1)?,
        target_vault_id: row.get(2)?,
        wifi_only: wifi_only != 0,
        allowed_ssids: row.get(4)?,
        require_charging: require_charging != 0,
        max_kbps: max_kbps as u32,
        enabled: enabled != 0,
    })
}

fn row_to_local_file_state(row: &rusqlite::Row<'_>) -> rusqlite::Result<LocalFileStateRecord> {
    let size: i64 = row.get(3)?;
    Ok(LocalFileStateRecord {
        rule_id: row.get(0)?,
        target_vault_id: row.get(1)?,
        rel_path: row.get(2)?,
        size: size as u64,
        mtime_ns: row.get(4)?,
        file_id: row.get(5)?,
        chunk_hashes: row.get(6)?,
    })
}

fn unix_timestamp_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn ensure_column(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let exists = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<std::result::Result<Vec<_>, _>>()?
        .iter()
        .any(|name| name == column);
    drop(stmt);
    if !exists {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {definition};"))?;
    }
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_settings_get_set_roundtrip() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.get_setting("role_mode").unwrap(), None);
        db.set_setting("role_mode", "dual").unwrap();
        assert_eq!(db.get_setting("role_mode").unwrap().as_deref(), Some("dual"));
        db.set_setting("role_mode", "server_only").unwrap();
        assert_eq!(
            db.get_setting("role_mode").unwrap().as_deref(),
            Some("server_only")
        );
    }

    #[test]
    fn get_sync_rule_and_batch_delete_local_file_states() {
        let db = Database::open_in_memory().unwrap();
        let rule = SyncRuleRecord {
            rule_id: "rule-1".to_string(),
            local_dir: "/tmp/docs".to_string(),
            target_vault_id: "vault-1".to_string(),
            wifi_only: true,
            allowed_ssids: Some("HomeWiFi".to_string()),
            require_charging: false,
            max_kbps: 1024,
            enabled: true,
        };
        db.upsert_sync_rule(&rule).unwrap();
        let fetched = db.get_sync_rule("rule-1").unwrap().unwrap();
        assert_eq!(fetched.local_dir, "/tmp/docs");
        assert!(db.get_sync_rule("missing").unwrap().is_none());

        for name in ["a.txt", "b.txt", "c.txt"] {
            db.upsert_local_file_state(&LocalFileStateRecord {
                rule_id: "rule-1".to_string(),
                target_vault_id: "vault-1".to_string(),
                rel_path: name.to_string(),
                size: 10,
                mtime_ns: 100,
                file_id: None,
                chunk_hashes: vec![1, 2, 3],
            })
            .unwrap();
        }
        let deleted = db
            .delete_local_file_states("rule-1", &["a.txt".to_string(), "c.txt".to_string()])
            .unwrap();
        assert_eq!(deleted, 2);
        let remaining = db.list_local_file_states("rule-1").unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].rel_path, "b.txt");
    }

    #[test]
    fn snapshot_references_are_counted_once_and_released_transactionally() {
        let db = Database::open_in_memory().unwrap();
        db.insert_vault(
            &VaultRecord {
                vault_id: "vault-1".to_string(),
                name: "Vault".to_string(),
                pool_mode: PoolMode::Single,
                quota_bytes: 1024,
                used_bytes: 0,
                e2ee_enabled: true,
                created_at: 1,
            },
            &[],
        )
        .unwrap();
        db.upsert_trusted_peer(&TrustedPeer {
            device_id: "device-1".to_string(),
            device_name: "Device".to_string(),
            os_type: "test".to_string(),
            ed25519_pubkey: vec![1; 32],
            last_ip: None,
            last_port: None,
            auto_connect: false,
            paired_at: 1,
            revoked_at: None,
        })
        .unwrap();
        let hash = [7u8; 32];
        assert!(db
            .upsert_chunk("vault-1", &hash, 10, 10)
            .unwrap());
        let snapshot = SnapshotRecord {
            snapshot_id: "snapshot-1".to_string(),
            vault_id: "vault-1".to_string(),
            rule_id: "rule-1".to_string(),
            device_id: "device-1".to_string(),
            root_tree_hash: [9u8; 32],
            retention_tier: RetentionTier::Daily,
            file_count: 1,
            total_bytes: 10,
            created_at: 100,
        };
        assert!(db
            .insert_snapshot_with_chunks(&snapshot, &[hash])
            .unwrap());
        assert!(!db
            .insert_snapshot_with_chunks(&snapshot, &[hash])
            .unwrap());
        assert_eq!(
            db.get_chunk("vault-1", &hash)
                .unwrap()
                .unwrap()
                .ref_count,
            1
        );
        assert!(db.delete_snapshot("snapshot-1").unwrap());
        assert_eq!(
            db.get_chunk("vault-1", &hash)
                .unwrap()
                .unwrap()
                .ref_count,
            0
        );
    }
}
