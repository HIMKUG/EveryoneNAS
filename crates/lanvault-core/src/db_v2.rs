//! Versioned SQLite v2 schema migrations and domain persistence for LanVault:
//! - Device groups & membership with monotonic revisions
//! - Group sync rules (diff backup & mirror sync) and per-target bindings
//! - Mirror root registrations, monotonic fenced sync leases, and idempotent operations
//! - Incremental path catalog (`catalog_entries`), historical versions (`catalog_versions`),
//!   and chunk reference tracking (`catalog_chunk_refs`) unified with `chunks.ref_count`
//! - Crash-safe mirror apply journal (`mirror_apply_journal`)
//! - Persistent multi-target message outbox & receiver deduplication receipts

use std::collections::BTreeMap;

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::db::{ensure_column, unix_timestamp_secs, Database};
use crate::error::{LanVaultError, Result};
use crate::path_norm::normalize_canonical_path;
use crate::protocol::{
    CatalogCommitRequest, CatalogCommitResponse, CatalogDeltaKind, SyncRuleMode,
};

const CURRENT_SCHEMA_VERSION: i64 = 3;

// -----------------------------------------------------------------------------
// v2 Record Structs
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceGroupRecord {
    pub group_id: String,
    pub name: String,
    pub coordinator_device_id: String,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceGroupMemberRecord {
    pub group_id: String,
    pub device_id: String,
    pub role: String,
    pub added_at: i64,
    pub removed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupSyncRuleRecord {
    pub rule_id: String,
    pub group_id: String,
    pub name: String,
    pub mode: SyncRuleMode,
    pub source_device_id: String,
    pub source_root: String,
    pub source_root_id: Option<String>,
    pub archive_retention_days: u32,
    pub wifi_only: bool,
    pub allowed_ssids: Option<String>,
    pub require_charging: bool,
    pub max_kbps: u32,
    pub schedule_interval_secs: u64,
    pub enabled: bool,
    pub revision: i64,
    pub group_revision: i64,
    pub needs_binding_confirm: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleTargetBindingRecord {
    pub binding_id: String,
    pub rule_id: String,
    pub group_id: String,
    pub target_device_id: String,
    pub target_vault_id: Option<String>,
    pub mirror_root_id: Option<String>,
    pub target_root: Option<String>,
    pub enabled: bool,
    pub revoked_at: Option<i64>,
    pub applied_revision: i64,
    pub last_source_gen: i64,
    pub status: String,
    pub last_error: Option<String>,
    pub needs_binding_confirm: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MirrorRootRegistration {
    pub mirror_root_id: String,
    pub device_id: String,
    pub canonical_root: String,
    pub root_identity: Option<String>,
    pub label: String,
    pub created_at: i64,
    pub revoked_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncLeaseRecord {
    pub rule_id: String,
    pub binding_id: String,
    pub holder_device_id: String,
    pub fence_token: i64,
    pub group_revision: i64,
    pub rule_revision: i64,
    pub acquired_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncOperationRecord {
    pub operation_id: String,
    pub rule_id: String,
    pub binding_id: String,
    pub payload_hash: Vec<u8>,
    pub status: String,
    pub response_json: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogEntryRecord {
    pub binding_id: String,
    pub path_key: String,
    pub display_path: String,
    pub entry_kind: String,
    pub version_id: String,
    pub size: u64,
    pub mtime_ns: i64,
    pub content_hash: Option<Vec<u8>>,
    pub chunk_hashes_json: String,
    pub source_deleted: bool,
    pub source_deleted_at: Option<i64>,
    pub updated_revision: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogVersionRecord {
    pub version_id: String,
    pub binding_id: String,
    pub vault_id: String,
    pub path_key: String,
    pub display_path: String,
    pub entry_kind: String,
    pub size: u64,
    pub mtime_ns: i64,
    pub content_hash: Option<Vec<u8>>,
    pub chunk_hashes_json: String,
    pub archive_reason: String,
    pub prev_state: String,
    pub operation_id: String,
    pub archived_at: i64,
    pub expires_at: Option<i64>,
    pub recycled_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MirrorJournalRecord {
    pub journal_id: String,
    pub operation_id: String,
    pub rule_id: String,
    pub binding_id: String,
    pub path_key: String,
    pub source_gen: i64,
    pub base_revision: i64,
    pub expected_prev_hash: Option<Vec<u8>>,
    pub archive_version_id: Option<String>,
    pub staging_path: Option<String>,
    pub phase: String,
    pub error_message: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageOutboxRecord {
    pub client_msg_id: String,
    pub local_seq: i64,
    pub session_key: String,
    pub kind: String,
    pub title: String,
    pub content: Option<String>,
    pub file_path: Option<String>,
    pub file_hash_hex: Option<String>,
    pub size_bytes: u64,
    pub overall_status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageOutboxTargetRecord {
    pub client_msg_id: String,
    pub target_device_id: String,
    pub channel: String,
    pub status: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempt_count: i64,
    pub next_retry_at: Option<i64>,
    pub updated_at: i64,
}

// -----------------------------------------------------------------------------
// Explicit Versioned Migrations (`PRAGMA user_version`)
// -----------------------------------------------------------------------------

pub(crate) fn run_versioned_migrations(conn: &Connection) -> Result<()> {
    let current_version: i64 = conn.query_row("PRAGMA user_version;", [], |row| row.get(0))?;
    if current_version >= CURRENT_SCHEMA_VERSION {
        return Ok(());
    }

    conn.execute_batch("BEGIN IMMEDIATE;")?;
    let migration_result = (|| {
        if current_version < 2 {
            apply_v2_migration(conn)?;
        }
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_catalog_versions_timeline
                 ON catalog_versions(binding_id, archived_at DESC, version_id DESC);
             CREATE INDEX IF NOT EXISTS idx_mirror_journal_archive_in_progress
                 ON mirror_apply_journal(archive_version_id)
                 WHERE phase != 'committed';
             PRAGMA user_version = 3;",
        )?;
        Ok(())
    })();
    match migration_result {
        Ok(()) => {
            conn.execute_batch("COMMIT;")?;
            Ok(())
        }
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(err)
        }
    }
}

fn apply_v2_migration(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS device_groups (
            group_id              TEXT PRIMARY KEY,
            name                  TEXT NOT NULL,
            coordinator_device_id TEXT NOT NULL,
            revision              INTEGER NOT NULL DEFAULT 1,
            created_at            INTEGER NOT NULL,
            updated_at            INTEGER NOT NULL,
            deleted_at            INTEGER
        );

        CREATE TABLE IF NOT EXISTS device_group_members (
            group_id    TEXT NOT NULL REFERENCES device_groups(group_id) ON DELETE CASCADE,
            device_id   TEXT NOT NULL,
            role        TEXT NOT NULL DEFAULT 'member',
            added_at    INTEGER NOT NULL,
            removed_at  INTEGER,
            PRIMARY KEY (group_id, device_id)
        );
        CREATE INDEX IF NOT EXISTS idx_device_group_members_device
            ON device_group_members(device_id, removed_at);

        CREATE TABLE IF NOT EXISTS group_sync_rules (
            rule_id                 TEXT PRIMARY KEY,
            group_id                TEXT NOT NULL REFERENCES device_groups(group_id) ON DELETE CASCADE,
            name                    TEXT NOT NULL DEFAULT '',
            mode                    TEXT NOT NULL DEFAULT 'diff_backup',
            source_device_id        TEXT NOT NULL,
            source_root             TEXT NOT NULL,
            source_root_id          TEXT,
            archive_retention_days  INTEGER NOT NULL DEFAULT 30,
            wifi_only               INTEGER NOT NULL DEFAULT 1,
            allowed_ssids           TEXT,
            require_charging        INTEGER NOT NULL DEFAULT 0,
            max_kbps                INTEGER NOT NULL DEFAULT 0,
            schedule_interval_secs  INTEGER NOT NULL DEFAULT 0,
            enabled                 INTEGER NOT NULL DEFAULT 1,
            revision                INTEGER NOT NULL DEFAULT 1,
            group_revision          INTEGER NOT NULL DEFAULT 1,
            needs_binding_confirm   INTEGER NOT NULL DEFAULT 0,
            created_at              INTEGER NOT NULL,
            updated_at              INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_group_sync_rules_group
            ON group_sync_rules(group_id, enabled);

        CREATE TABLE IF NOT EXISTS rule_target_bindings (
            binding_id              TEXT PRIMARY KEY,
            rule_id                 TEXT NOT NULL REFERENCES group_sync_rules(rule_id) ON DELETE CASCADE,
            group_id                TEXT NOT NULL,
            target_device_id        TEXT NOT NULL,
            target_vault_id         TEXT,
            mirror_root_id          TEXT,
            target_root             TEXT,
            enabled                 INTEGER NOT NULL DEFAULT 1,
            revoked_at              INTEGER,
            applied_revision        INTEGER NOT NULL DEFAULT 0,
            last_source_gen         INTEGER NOT NULL DEFAULT 0,
            status                  TEXT NOT NULL DEFAULT 'pending',
            last_error              TEXT,
            needs_binding_confirm   INTEGER NOT NULL DEFAULT 0,
            created_at              INTEGER NOT NULL,
            updated_at              INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_rule_target_bindings_rule
            ON rule_target_bindings(rule_id, enabled);
        CREATE INDEX IF NOT EXISTS idx_rule_target_bindings_device
            ON rule_target_bindings(target_device_id, enabled);

        CREATE TABLE IF NOT EXISTS mirror_root_registrations (
            mirror_root_id  TEXT PRIMARY KEY,
            device_id       TEXT NOT NULL,
            canonical_root  TEXT NOT NULL,
            root_identity   TEXT,
            label           TEXT NOT NULL DEFAULT '',
            created_at      INTEGER NOT NULL,
            revoked_at      INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_mirror_roots_device
            ON mirror_root_registrations(device_id, revoked_at);

        CREATE TABLE IF NOT EXISTS sync_leases (
            rule_id          TEXT NOT NULL,
            binding_id       TEXT NOT NULL,
            holder_device_id TEXT NOT NULL,
            fence_token      INTEGER NOT NULL,
            group_revision   INTEGER NOT NULL,
            rule_revision    INTEGER NOT NULL,
            acquired_at      INTEGER NOT NULL,
            expires_at       INTEGER NOT NULL,
            PRIMARY KEY (rule_id, binding_id)
        );

        CREATE TABLE IF NOT EXISTS sync_operations (
            operation_id    TEXT PRIMARY KEY,
            rule_id         TEXT NOT NULL,
            binding_id      TEXT NOT NULL,
            payload_hash    BLOB NOT NULL,
            status          TEXT NOT NULL,
            response_json   TEXT NOT NULL,
            created_at      INTEGER NOT NULL,
            updated_at      INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_sync_operations_binding
            ON sync_operations(binding_id, created_at DESC);

        CREATE TABLE IF NOT EXISTS catalog_entries (
            binding_id        TEXT NOT NULL,
            path_key          TEXT NOT NULL,
            display_path      TEXT NOT NULL,
            entry_kind        TEXT NOT NULL,
            version_id        TEXT NOT NULL,
            size              INTEGER NOT NULL DEFAULT 0,
            mtime_ns          INTEGER NOT NULL DEFAULT 0,
            content_hash      BLOB,
            chunk_hashes_json TEXT NOT NULL DEFAULT '[]',
            source_deleted    INTEGER NOT NULL DEFAULT 0,
            source_deleted_at INTEGER,
            updated_revision  INTEGER NOT NULL DEFAULT 0,
            updated_at        INTEGER NOT NULL,
            PRIMARY KEY (binding_id, path_key)
        );
        CREATE INDEX IF NOT EXISTS idx_catalog_source_deleted
            ON catalog_entries(binding_id, path_key)
            WHERE source_deleted = 1;

        CREATE TABLE IF NOT EXISTS catalog_versions (
            version_id        TEXT PRIMARY KEY,
            binding_id        TEXT NOT NULL,
            vault_id          TEXT NOT NULL,
            path_key          TEXT NOT NULL,
            display_path      TEXT NOT NULL,
            entry_kind        TEXT NOT NULL,
            size              INTEGER NOT NULL DEFAULT 0,
            mtime_ns          INTEGER NOT NULL DEFAULT 0,
            content_hash      BLOB,
            chunk_hashes_json TEXT NOT NULL DEFAULT '[]',
            archive_reason    TEXT NOT NULL,
            prev_state        TEXT NOT NULL DEFAULT '',
            operation_id      TEXT NOT NULL,
            archived_at       INTEGER NOT NULL,
            expires_at        INTEGER,
            recycled_at       INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_catalog_versions_history
            ON catalog_versions(binding_id, path_key, archived_at DESC, version_id DESC);
        CREATE INDEX IF NOT EXISTS idx_catalog_versions_expiry
            ON catalog_versions(expires_at, version_id)
            WHERE expires_at IS NOT NULL AND recycled_at IS NULL;

        CREATE TABLE IF NOT EXISTS catalog_chunk_refs (
            owner_kind  TEXT NOT NULL,
            owner_id    TEXT NOT NULL,
            vault_id    TEXT NOT NULL,
            chunk_hash  BLOB NOT NULL,
            PRIMARY KEY (owner_kind, owner_id, chunk_hash)
        );
        CREATE INDEX IF NOT EXISTS idx_catalog_chunk_refs_vault_hash
            ON catalog_chunk_refs(vault_id, chunk_hash);

        CREATE TABLE IF NOT EXISTS mirror_apply_journal (
            journal_id          TEXT PRIMARY KEY,
            operation_id        TEXT NOT NULL,
            rule_id             TEXT NOT NULL,
            binding_id          TEXT NOT NULL,
            path_key            TEXT NOT NULL,
            source_gen          INTEGER NOT NULL,
            base_revision       INTEGER NOT NULL,
            expected_prev_hash  BLOB,
            archive_version_id  TEXT,
            staging_path        TEXT,
            phase               TEXT NOT NULL,
            error_message       TEXT,
            created_at          INTEGER NOT NULL,
            updated_at          INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_mirror_journal_binding_phase
            ON mirror_apply_journal(binding_id, phase);

        CREATE TABLE IF NOT EXISTS message_outbox (
            client_msg_id   TEXT PRIMARY KEY,
            local_seq       INTEGER NOT NULL UNIQUE,
            session_key     TEXT NOT NULL,
            kind            TEXT NOT NULL,
            title           TEXT NOT NULL,
            content         TEXT,
            file_path       TEXT,
            file_hash_hex   TEXT,
            size_bytes      INTEGER NOT NULL DEFAULT 0,
            overall_status  TEXT NOT NULL DEFAULT 'sending',
            created_at      INTEGER NOT NULL,
            updated_at      INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_message_outbox_session_seq
            ON message_outbox(session_key, local_seq DESC);

        CREATE TABLE IF NOT EXISTS message_outbox_targets (
            client_msg_id     TEXT NOT NULL REFERENCES message_outbox(client_msg_id) ON DELETE CASCADE,
            target_device_id  TEXT NOT NULL,
            channel           TEXT NOT NULL,
            status            TEXT NOT NULL DEFAULT 'sending',
            error_code        TEXT,
            error_message     TEXT,
            attempt_count     INTEGER NOT NULL DEFAULT 0,
            next_retry_at     INTEGER,
            updated_at        INTEGER NOT NULL,
            PRIMARY KEY (client_msg_id, target_device_id)
        );
        CREATE INDEX IF NOT EXISTS idx_message_outbox_targets_status
            ON message_outbox_targets(status, next_retry_at);

        CREATE TABLE IF NOT EXISTS message_dedup_receipts (
            sender_device_id    TEXT NOT NULL,
            client_msg_id       TEXT NOT NULL,
            receiver_device_id  TEXT NOT NULL,
            ack_json            TEXT NOT NULL,
            received_at         INTEGER NOT NULL,
            PRIMARY KEY (sender_device_id, client_msg_id, receiver_device_id)
        );
        "#,
    )?;

    ensure_column(
        conn,
        "transfer_history",
        "client_msg_id",
        "client_msg_id TEXT",
    )?;
    ensure_column(conn, "transfer_history", "status", "status TEXT")?;
    ensure_column(conn, "transfer_history", "error_code", "error_code TEXT")?;
    ensure_column(
        conn,
        "transfer_history",
        "error_message",
        "error_message TEXT",
    )?;
    ensure_column(
        conn,
        "transfer_history",
        "target_device_id",
        "target_device_id TEXT",
    )?;
    ensure_column(conn, "transfer_history", "local_seq", "local_seq INTEGER")?;

    conn.execute_batch("PRAGMA user_version = 2;")?;
    Ok(())
}

// -----------------------------------------------------------------------------
// Database v2 Methods
// -----------------------------------------------------------------------------

impl Database {
    // -------------------------------------------------------------------------
    // Device Groups & Members
    // -------------------------------------------------------------------------

    pub fn create_device_group(&self, group: &DeviceGroupRecord) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            r#"
            INSERT INTO device_groups (
                group_id, name, coordinator_device_id, revision, created_at, updated_at, deleted_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                group.group_id,
                group.name,
                group.coordinator_device_id,
                group.revision,
                group.created_at,
                group.updated_at,
                group.deleted_at,
            ],
        )?;
        Ok(())
    }

    pub fn rename_device_group(&self, group_id: &str, new_name: &str) -> Result<bool> {
        let conn = self.lock_conn();
        let now = unix_timestamp_secs();
        let affected = conn.execute(
            "UPDATE device_groups
             SET name = ?1, revision = revision + 1, updated_at = ?2
             WHERE group_id = ?3 AND deleted_at IS NULL",
            params![new_name, now, group_id],
        )?;
        Ok(affected > 0)
    }

    /// Disables all linked rules and bindings, invalidates active leases, marks the group
    /// deleted, and bumps its revision while preserving catalog data and recovery entries.
    pub fn delete_device_group(&self, group_id: &str) -> Result<bool> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = unix_timestamp_secs();
        tx.execute(
            "UPDATE group_sync_rules
             SET enabled = 0, revision = revision + 1, updated_at = ?1
             WHERE group_id = ?2",
            params![now, group_id],
        )?;
        tx.execute(
            "UPDATE rule_target_bindings
             SET enabled = 0, revoked_at = COALESCE(revoked_at, ?1), updated_at = ?1
             WHERE group_id = ?2",
            params![now, group_id],
        )?;
        tx.execute(
            "DELETE FROM sync_leases
             WHERE rule_id IN (SELECT rule_id FROM group_sync_rules WHERE group_id = ?1)",
            params![group_id],
        )?;
        let affected = tx.execute(
            "UPDATE device_groups
             SET deleted_at = ?1, revision = revision + 1, updated_at = ?1
             WHERE group_id = ?2 AND deleted_at IS NULL",
            params![now, group_id],
        )?;
        tx.commit()?;
        Ok(affected > 0)
    }

    pub fn list_device_groups(&self) -> Result<Vec<DeviceGroupRecord>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            r#"
            SELECT group_id, name, coordinator_device_id, revision, created_at, updated_at, deleted_at
            FROM device_groups
            WHERE deleted_at IS NULL
            ORDER BY created_at ASC
            "#,
        )?;
        let rows = stmt
            .query_map([], row_to_device_group)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_device_group(&self, group_id: &str) -> Result<Option<DeviceGroupRecord>> {
        let conn = self.lock_conn();
        let row = conn
            .query_row(
                r#"
                SELECT group_id, name, coordinator_device_id, revision, created_at, updated_at, deleted_at
                FROM device_groups
                WHERE group_id = ?1 AND deleted_at IS NULL
                "#,
                params![group_id],
                row_to_device_group,
            )
            .optional()?;
        Ok(row)
    }

    pub fn add_group_member(&self, group_id: &str, device_id: &str, role: &str) -> Result<()> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = unix_timestamp_secs();
        tx.execute(
            r#"
            INSERT INTO device_group_members (group_id, device_id, role, added_at, removed_at)
            VALUES (?1, ?2, ?3, ?4, NULL)
            ON CONFLICT(group_id, device_id) DO UPDATE SET
                role = excluded.role,
                added_at = excluded.added_at,
                removed_at = NULL
            "#,
            params![group_id, device_id, role, now],
        )?;
        let updated = tx.execute(
            "UPDATE device_groups
             SET revision = revision + 1, updated_at = ?1
             WHERE group_id = ?2 AND deleted_at IS NULL",
            params![now, group_id],
        )?;
        if updated == 0 {
            return Err(LanVaultError::NotFound(format!(
                "device_group `{group_id}`"
            )));
        }
        tx.commit()?;
        Ok(())
    }

    /// Removes a member from a group, disables their target bindings in that group,
    /// deletes their active leases, and bumps `device_groups.revision`.
    pub fn remove_group_member(&self, group_id: &str, device_id: &str) -> Result<bool> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = unix_timestamp_secs();
        let affected = tx.execute(
            "UPDATE device_group_members
             SET removed_at = ?1
             WHERE group_id = ?2 AND device_id = ?3 AND removed_at IS NULL",
            params![now, group_id, device_id],
        )?;
        if affected == 0 {
            tx.commit()?;
            return Ok(false);
        }
        tx.execute(
            "UPDATE rule_target_bindings
             SET enabled = 0, revoked_at = COALESCE(revoked_at, ?1), updated_at = ?1
             WHERE group_id = ?2 AND target_device_id = ?3",
            params![now, group_id, device_id],
        )?;
        tx.execute(
            "UPDATE group_sync_rules
             SET enabled = 0, revision = revision + 1, updated_at = ?1
             WHERE group_id = ?2 AND source_device_id = ?3",
            params![now, group_id, device_id],
        )?;
        tx.execute(
            "DELETE FROM sync_leases
             WHERE (holder_device_id = ?1 AND rule_id IN (SELECT rule_id FROM group_sync_rules WHERE group_id = ?2))
                OR binding_id IN (SELECT binding_id FROM rule_target_bindings WHERE group_id = ?2 AND target_device_id = ?1)",
            params![device_id, group_id],
        )?;
        tx.execute(
            "UPDATE device_groups
             SET revision = revision + 1, updated_at = ?1
             WHERE group_id = ?2 AND deleted_at IS NULL",
            params![now, group_id],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn list_group_members(&self, group_id: &str) -> Result<Vec<DeviceGroupMemberRecord>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            r#"
            SELECT group_id, device_id, role, added_at, removed_at
            FROM device_group_members
            WHERE group_id = ?1 AND removed_at IS NULL
            ORDER BY added_at ASC
            "#,
        )?;
        let rows = stmt
            .query_map(params![group_id], row_to_device_group_member)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn is_active_group_member(&self, group_id: &str, device_id: &str) -> Result<bool> {
        let conn = self.lock_conn();
        let found: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM device_group_members m
                 INNER JOIN device_groups g ON g.group_id = m.group_id
                 WHERE m.group_id = ?1 AND m.device_id = ?2
                   AND m.removed_at IS NULL AND g.deleted_at IS NULL",
                params![group_id, device_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// A rule source may access only the vaults explicitly targeted by its enabled,
    /// confirmed bindings while it remains an active member of that group.
    pub fn is_group_source_for_vault(&self, device_id: &str, vault_id: &str) -> Result<bool> {
        let conn = self.lock_read_conn();
        let found: Option<i64> = conn
            .query_row(
                "SELECT 1
                 FROM group_sync_rules r
                 INNER JOIN rule_target_bindings b ON b.rule_id = r.rule_id
                 INNER JOIN device_group_members m ON m.group_id = r.group_id
                 INNER JOIN device_groups g ON g.group_id = r.group_id
                 WHERE r.source_device_id = ?1
                   AND b.target_vault_id = ?2
                   AND r.enabled = 1
                   AND r.needs_binding_confirm = 0
                   AND b.enabled = 1
                   AND b.revoked_at IS NULL
                   AND b.needs_binding_confirm = 0
                   AND m.device_id = ?1
                   AND m.removed_at IS NULL
                   AND g.deleted_at IS NULL
                 LIMIT 1",
                params![device_id, vault_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Called on peer revocation: removes the peer from all active groups, disables all
    /// associated bindings and source rules, invalidates leases, and bumps affected group revisions.
    pub fn revoke_peer_from_all_groups(&self, device_id: &str) -> Result<usize> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = unix_timestamp_secs();

        let group_ids: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT group_id FROM device_group_members
                 WHERE device_id = ?1 AND removed_at IS NULL",
            )?;
            let rows = stmt
                .query_map(params![device_id], |row| row.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };

        tx.execute(
            "UPDATE device_group_members
             SET removed_at = ?1
             WHERE device_id = ?2 AND removed_at IS NULL",
            params![now, device_id],
        )?;
        tx.execute(
            "UPDATE rule_target_bindings
             SET enabled = 0, revoked_at = COALESCE(revoked_at, ?1), updated_at = ?1
             WHERE target_device_id = ?2",
            params![now, device_id],
        )?;
        tx.execute(
            "UPDATE group_sync_rules
             SET enabled = 0, revision = revision + 1, updated_at = ?1
             WHERE source_device_id = ?2",
            params![now, device_id],
        )?;
        tx.execute(
            "DELETE FROM sync_leases
             WHERE holder_device_id = ?1
                OR binding_id IN (SELECT binding_id FROM rule_target_bindings WHERE target_device_id = ?1)
                OR rule_id IN (SELECT rule_id FROM group_sync_rules WHERE source_device_id = ?1)",
            params![device_id],
        )?;
        for gid in &group_ids {
            tx.execute(
                "UPDATE device_groups
                 SET revision = revision + 1, updated_at = ?1
                 WHERE group_id = ?2 AND deleted_at IS NULL",
                params![now, gid],
            )?;
        }

        tx.commit()?;
        Ok(group_ids.len())
    }

    // -------------------------------------------------------------------------
    // Group Sync Rules & Rule Target Bindings
    // -------------------------------------------------------------------------

    pub fn upsert_group_sync_rule(&self, rule: &GroupSyncRuleRecord) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            r#"
            INSERT INTO group_sync_rules (
                rule_id, group_id, name, mode, source_device_id, source_root, source_root_id,
                archive_retention_days, wifi_only, allowed_ssids, require_charging, max_kbps,
                schedule_interval_secs, enabled, revision, group_revision, needs_binding_confirm,
                created_at, updated_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19
            )
            ON CONFLICT(rule_id) DO UPDATE SET
                group_id = excluded.group_id,
                name = excluded.name,
                mode = excluded.mode,
                source_device_id = excluded.source_device_id,
                source_root = excluded.source_root,
                source_root_id = excluded.source_root_id,
                archive_retention_days = excluded.archive_retention_days,
                wifi_only = excluded.wifi_only,
                allowed_ssids = excluded.allowed_ssids,
                require_charging = excluded.require_charging,
                max_kbps = excluded.max_kbps,
                schedule_interval_secs = excluded.schedule_interval_secs,
                enabled = excluded.enabled,
                revision = group_sync_rules.revision + 1,
                group_revision = excluded.group_revision,
                needs_binding_confirm = excluded.needs_binding_confirm,
                updated_at = excluded.updated_at
            "#,
            params![
                rule.rule_id,
                rule.group_id,
                rule.name,
                rule.mode.as_str(),
                rule.source_device_id,
                rule.source_root,
                rule.source_root_id,
                i64::from(rule.archive_retention_days),
                i64::from(rule.wifi_only),
                rule.allowed_ssids,
                i64::from(rule.require_charging),
                i64::from(rule.max_kbps),
                rule.schedule_interval_secs as i64,
                i64::from(rule.enabled),
                rule.revision,
                rule.group_revision,
                i64::from(rule.needs_binding_confirm),
                rule.created_at,
                rule.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_group_sync_rule(&self, rule_id: &str) -> Result<Option<GroupSyncRuleRecord>> {
        let conn = self.lock_conn();
        let row = conn
            .query_row(
                r#"
                SELECT rule_id, group_id, name, mode, source_device_id, source_root, source_root_id,
                       archive_retention_days, wifi_only, allowed_ssids, require_charging, max_kbps,
                       schedule_interval_secs, enabled, revision, group_revision, needs_binding_confirm,
                       created_at, updated_at
                FROM group_sync_rules
                WHERE rule_id = ?1
                "#,
                params![rule_id],
                row_to_group_sync_rule,
            )
            .optional()?;
        Ok(row)
    }

    pub fn list_group_sync_rules(
        &self,
        group_id: Option<&str>,
    ) -> Result<Vec<GroupSyncRuleRecord>> {
        let conn = self.lock_conn();
        match group_id {
            Some(gid) => {
                let mut stmt = conn.prepare(
                    r#"
                    SELECT rule_id, group_id, name, mode, source_device_id, source_root, source_root_id,
                           archive_retention_days, wifi_only, allowed_ssids, require_charging, max_kbps,
                           schedule_interval_secs, enabled, revision, group_revision, needs_binding_confirm,
                           created_at, updated_at
                    FROM group_sync_rules
                    WHERE group_id = ?1
                    ORDER BY created_at ASC
                    "#,
                )?;
                let rows = stmt
                    .query_map(params![gid], row_to_group_sync_rule)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(rows)
            }
            None => {
                let mut stmt = conn.prepare(
                    r#"
                    SELECT rule_id, group_id, name, mode, source_device_id, source_root, source_root_id,
                           archive_retention_days, wifi_only, allowed_ssids, require_charging, max_kbps,
                           schedule_interval_secs, enabled, revision, group_revision, needs_binding_confirm,
                           created_at, updated_at
                    FROM group_sync_rules
                    ORDER BY created_at ASC
                    "#,
                )?;
                let rows = stmt
                    .query_map([], row_to_group_sync_rule)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(rows)
            }
        }
    }

    pub fn delete_group_sync_rule(&self, rule_id: &str) -> Result<bool> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM sync_leases WHERE rule_id = ?1",
            params![rule_id],
        )?;
        tx.execute(
            "DELETE FROM rule_target_bindings WHERE rule_id = ?1",
            params![rule_id],
        )?;
        let affected = tx.execute(
            "DELETE FROM group_sync_rules WHERE rule_id = ?1",
            params![rule_id],
        )?;
        tx.commit()?;
        Ok(affected > 0)
    }

    pub fn toggle_group_sync_rule(&self, rule_id: &str, enabled: bool) -> Result<bool> {
        let conn = self.lock_conn();
        let now = unix_timestamp_secs();
        let affected = conn.execute(
            "UPDATE group_sync_rules
             SET enabled = ?1, revision = revision + 1, updated_at = ?2
             WHERE rule_id = ?3",
            params![i64::from(enabled), now, rule_id],
        )?;
        Ok(affected > 0)
    }

    pub fn upsert_rule_target_binding(&self, binding: &RuleTargetBindingRecord) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            r#"
            INSERT INTO rule_target_bindings (
                binding_id, rule_id, group_id, target_device_id, target_vault_id,
                mirror_root_id, target_root, enabled, revoked_at, applied_revision,
                last_source_gen, status, last_error, needs_binding_confirm,
                created_at, updated_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16
            )
            ON CONFLICT(binding_id) DO UPDATE SET
                target_vault_id = excluded.target_vault_id,
                mirror_root_id = excluded.mirror_root_id,
                target_root = excluded.target_root,
                enabled = excluded.enabled,
                revoked_at = excluded.revoked_at,
                status = excluded.status,
                last_error = excluded.last_error,
                needs_binding_confirm = excluded.needs_binding_confirm,
                updated_at = excluded.updated_at
            "#,
            params![
                binding.binding_id,
                binding.rule_id,
                binding.group_id,
                binding.target_device_id,
                binding.target_vault_id,
                binding.mirror_root_id,
                binding.target_root,
                i64::from(binding.enabled),
                binding.revoked_at,
                binding.applied_revision,
                binding.last_source_gen,
                binding.status,
                binding.last_error,
                i64::from(binding.needs_binding_confirm),
                binding.created_at,
                binding.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_rule_target_binding(
        &self,
        binding_id: &str,
    ) -> Result<Option<RuleTargetBindingRecord>> {
        let conn = self.lock_conn();
        let row = conn
            .query_row(
                r#"
                SELECT binding_id, rule_id, group_id, target_device_id, target_vault_id,
                       mirror_root_id, target_root, enabled, revoked_at, applied_revision,
                       last_source_gen, status, last_error, needs_binding_confirm,
                       created_at, updated_at
                FROM rule_target_bindings
                WHERE binding_id = ?1
                "#,
                params![binding_id],
                row_to_rule_target_binding,
            )
            .optional()?;
        Ok(row)
    }

    pub fn list_rule_target_bindings(&self, rule_id: &str) -> Result<Vec<RuleTargetBindingRecord>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            r#"
            SELECT binding_id, rule_id, group_id, target_device_id, target_vault_id,
                   mirror_root_id, target_root, enabled, revoked_at, applied_revision,
                   last_source_gen, status, last_error, needs_binding_confirm,
                   created_at, updated_at
            FROM rule_target_bindings
            WHERE rule_id = ?1
            ORDER BY created_at ASC
            "#,
        )?;
        let rows = stmt
            .query_map(params![rule_id], row_to_rule_target_binding)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn update_binding_status(
        &self,
        binding_id: &str,
        status: &str,
        last_error: Option<&str>,
    ) -> Result<bool> {
        let conn = self.lock_conn();
        let now = unix_timestamp_secs();
        let affected = conn.execute(
            "UPDATE rule_target_bindings
             SET status = ?1, last_error = ?2, updated_at = ?3
             WHERE binding_id = ?4",
            params![status, last_error, now, binding_id],
        )?;
        Ok(affected > 0)
    }

    pub fn disable_rule_target_binding(&self, binding_id: &str) -> Result<bool> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = unix_timestamp_secs();
        tx.execute(
            "DELETE FROM sync_leases WHERE binding_id = ?1",
            params![binding_id],
        )?;
        let affected = tx.execute(
            "UPDATE rule_target_bindings
             SET enabled = 0, revoked_at = COALESCE(revoked_at, ?1), updated_at = ?1
             WHERE binding_id = ?2",
            params![now, binding_id],
        )?;
        tx.commit()?;
        Ok(affected > 0)
    }

    // -------------------------------------------------------------------------
    // Mirror Root Registrations
    // -------------------------------------------------------------------------

    pub fn register_mirror_root(&self, reg: &MirrorRootRegistration) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            r#"
            INSERT INTO mirror_root_registrations (
                mirror_root_id, device_id, canonical_root, root_identity, label, created_at, revoked_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(mirror_root_id) DO UPDATE SET
                canonical_root = excluded.canonical_root,
                root_identity = excluded.root_identity,
                label = excluded.label,
                revoked_at = excluded.revoked_at
            "#,
            params![
                reg.mirror_root_id,
                reg.device_id,
                reg.canonical_root,
                reg.root_identity,
                reg.label,
                reg.created_at,
                reg.revoked_at,
            ],
        )?;
        Ok(())
    }

    pub fn list_mirror_roots(
        &self,
        device_id: Option<&str>,
    ) -> Result<Vec<MirrorRootRegistration>> {
        let conn = self.lock_conn();
        match device_id {
            Some(did) => {
                let mut stmt = conn.prepare(
                    r#"
                    SELECT mirror_root_id, device_id, canonical_root, root_identity, label, created_at, revoked_at
                    FROM mirror_root_registrations
                    WHERE device_id = ?1 AND revoked_at IS NULL
                    ORDER BY created_at ASC
                    "#,
                )?;
                let rows = stmt
                    .query_map(params![did], row_to_mirror_root)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(rows)
            }
            None => {
                let mut stmt = conn.prepare(
                    r#"
                    SELECT mirror_root_id, device_id, canonical_root, root_identity, label, created_at, revoked_at
                    FROM mirror_root_registrations
                    WHERE revoked_at IS NULL
                    ORDER BY created_at ASC
                    "#,
                )?;
                let rows = stmt
                    .query_map([], row_to_mirror_root)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(rows)
            }
        }
    }

    pub fn get_mirror_root(&self, mirror_root_id: &str) -> Result<Option<MirrorRootRegistration>> {
        let conn = self.lock_conn();
        let row = conn
            .query_row(
                r#"
                SELECT mirror_root_id, device_id, canonical_root, root_identity, label, created_at, revoked_at
                FROM mirror_root_registrations
                WHERE mirror_root_id = ?1
                "#,
                params![mirror_root_id],
                row_to_mirror_root,
            )
            .optional()?;
        Ok(row)
    }

    pub fn revoke_mirror_root(&self, mirror_root_id: &str) -> Result<bool> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = unix_timestamp_secs();
        let affected = tx.execute(
            "UPDATE mirror_root_registrations
             SET revoked_at = ?1
             WHERE mirror_root_id = ?2 AND revoked_at IS NULL",
            params![now, mirror_root_id],
        )?;
        tx.execute(
            "DELETE FROM sync_leases
             WHERE binding_id IN (SELECT binding_id FROM rule_target_bindings WHERE mirror_root_id = ?1)",
            params![mirror_root_id],
        )?;
        tx.execute(
            "UPDATE rule_target_bindings
             SET enabled = 0, revoked_at = COALESCE(revoked_at, ?1), status = 'revoked', updated_at = ?1
             WHERE mirror_root_id = ?2",
            params![now, mirror_root_id],
        )?;
        tx.commit()?;
        Ok(affected > 0)
    }

    // -------------------------------------------------------------------------
    // Sync Leases & Operations
    // -------------------------------------------------------------------------

    /// Acquires or renews a persistent fenced lease for `(rule_id, binding_id)`.
    /// Enforces monotonic `fence_token` and rejects stale holders or stale group/rule revisions.
    pub fn acquire_or_renew_sync_lease(
        &self,
        rule_id: &str,
        binding_id: &str,
        holder_device_id: &str,
        group_revision: i64,
        rule_revision: i64,
        ttl_secs: i64,
    ) -> Result<SyncLeaseRecord> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = unix_timestamp_secs();
        let expires_at = now + ttl_secs.max(1);

        // Verify binding, rule, and group are active and revisions are current.
        let binding_meta: Option<(String, String, i64, Option<i64>)> = tx
            .query_row(
                "SELECT rule_id, group_id, enabled, revoked_at
                 FROM rule_target_bindings
                 WHERE binding_id = ?1",
                params![binding_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let (b_rule_id, b_group_id, b_enabled, b_revoked_at) = binding_meta.ok_or_else(|| {
            LanVaultError::NotFound(format!("rule_target_binding `{binding_id}`"))
        })?;
        if b_rule_id != rule_id || b_enabled == 0 || b_revoked_at.is_some() {
            return Err(LanVaultError::InvalidState(format!(
                "binding `{binding_id}` is disabled, revoked, or does not belong to rule `{rule_id}`"
            )));
        }

        let rule_meta: Option<(i64, i64)> = tx
            .query_row(
                "SELECT enabled, revision FROM group_sync_rules WHERE rule_id = ?1",
                params![rule_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (r_enabled, current_rule_rev) = rule_meta
            .ok_or_else(|| LanVaultError::NotFound(format!("group_sync_rule `{rule_id}`")))?;
        if r_enabled == 0 || rule_revision != current_rule_rev {
            return Err(LanVaultError::InvalidState(format!(
                "rule `{rule_id}` is disabled or revision mismatch (expected {current_rule_rev}, got {rule_revision})"
            )));
        }

        let group_meta: Option<(i64, Option<i64>)> = tx
            .query_row(
                "SELECT revision, deleted_at FROM device_groups WHERE group_id = ?1",
                params![b_group_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (current_group_rev, g_deleted_at) = group_meta
            .ok_or_else(|| LanVaultError::NotFound(format!("device_group `{b_group_id}`")))?;
        if g_deleted_at.is_some() || group_revision != current_group_rev {
            return Err(LanVaultError::InvalidState(format!(
                "group `{b_group_id}` is deleted or revision mismatch (expected {current_group_rev}, got {group_revision})"
            )));
        }

        let existing: Option<SyncLeaseRecord> = tx
            .query_row(
                r#"
                SELECT rule_id, binding_id, holder_device_id, fence_token,
                       group_revision, rule_revision, acquired_at, expires_at
                FROM sync_leases
                WHERE rule_id = ?1 AND binding_id = ?2
                "#,
                params![rule_id, binding_id],
                row_to_sync_lease,
            )
            .optional()?;

        let next_fence = match existing {
            Some(prev) => {
                if prev.expires_at > now && prev.holder_device_id != holder_device_id {
                    return Err(LanVaultError::InvalidState(format!(
                        "sync lease for ({rule_id}, {binding_id}) is actively held by `{}` until {}",
                        prev.holder_device_id, prev.expires_at
                    )));
                }
                if group_revision < prev.group_revision || rule_revision < prev.rule_revision {
                    return Err(LanVaultError::InvalidState(format!(
                        "stale lease revision for ({rule_id}, {binding_id})"
                    )));
                }
                prev.fence_token + 1
            }
            None => 1,
        };

        tx.execute(
            r#"
            INSERT INTO sync_leases (
                rule_id, binding_id, holder_device_id, fence_token,
                group_revision, rule_revision, acquired_at, expires_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(rule_id, binding_id) DO UPDATE SET
                holder_device_id = excluded.holder_device_id,
                fence_token = excluded.fence_token,
                group_revision = excluded.group_revision,
                rule_revision = excluded.rule_revision,
                acquired_at = excluded.acquired_at,
                expires_at = excluded.expires_at
            "#,
            params![
                rule_id,
                binding_id,
                holder_device_id,
                next_fence,
                group_revision,
                rule_revision,
                now,
                expires_at,
            ],
        )?;
        tx.commit()?;

        Ok(SyncLeaseRecord {
            rule_id: rule_id.to_string(),
            binding_id: binding_id.to_string(),
            holder_device_id: holder_device_id.to_string(),
            fence_token: next_fence,
            group_revision,
            rule_revision,
            acquired_at: now,
            expires_at,
        })
    }

    pub fn verify_sync_lease(
        &self,
        rule_id: &str,
        binding_id: &str,
        fence_token: i64,
        group_revision: i64,
        rule_revision: i64,
    ) -> Result<bool> {
        let conn = self.lock_conn();
        let now = unix_timestamp_secs();
        let found: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM sync_leases
                 WHERE rule_id = ?1 AND binding_id = ?2
                   AND fence_token = ?3 AND group_revision = ?4
                   AND rule_revision = ?5 AND expires_at >= ?6",
                params![
                    rule_id,
                    binding_id,
                    fence_token,
                    group_revision,
                    rule_revision,
                    now
                ],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    pub fn get_sync_operation(&self, operation_id: &str) -> Result<Option<SyncOperationRecord>> {
        let conn = self.lock_conn();
        let row = conn
            .query_row(
                r#"
                SELECT operation_id, rule_id, binding_id, payload_hash, status,
                       response_json, created_at, updated_at
                FROM sync_operations
                WHERE operation_id = ?1
                "#,
                params![operation_id],
                row_to_sync_operation,
            )
            .optional()?;
        Ok(row)
    }

    // -------------------------------------------------------------------------
    // Incremental Catalog & Historical Versions (Transactional + Refcounted)
    // -------------------------------------------------------------------------

    /// Atomically commits a batch of incremental catalog delta operations for a binding,
    /// maintaining `catalog_entries`, `catalog_versions`, `catalog_chunk_refs`, and
    /// `chunks.ref_count` in a single SQLite transaction with full idempotency.
    pub fn commit_catalog_delta(
        &self,
        binding_id: &str,
        vault_id: &str,
        req: &CatalogCommitRequest,
    ) -> Result<CatalogCommitResponse> {
        let payload_bytes = serde_json::to_vec(req)?;
        let payload_hash = blake3::hash(&payload_bytes).as_bytes().to_vec();

        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = unix_timestamp_secs();

        // 1. Idempotent operation replay check.
        if let Some((stored_hash, response_json)) = tx
            .query_row(
                "SELECT payload_hash, response_json FROM sync_operations WHERE operation_id = ?1",
                params![req.operation_id],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
        {
            if stored_hash != payload_hash {
                return Err(LanVaultError::InvalidState(format!(
                    "operation_id `{}` already committed with a different payload",
                    req.operation_id
                )));
            }
            let mut resp: CatalogCommitResponse = serde_json::from_str(&response_json)?;
            resp.deduplicated_replay = true;
            tx.commit()?;
            return Ok(resp);
        }

        // 2. Verify binding state and base revision.
        let binding_row: Option<(String, i64, Option<i64>, i64)> = tx
            .query_row(
                "SELECT rule_id, enabled, revoked_at, applied_revision
                 FROM rule_target_bindings
                 WHERE binding_id = ?1",
                params![binding_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let (b_rule_id, b_enabled, b_revoked_at, applied_revision) =
            binding_row.ok_or_else(|| {
                LanVaultError::NotFound(format!("rule_target_binding `{binding_id}`"))
            })?;
        if b_rule_id != req.rule_id || b_enabled == 0 || b_revoked_at.is_some() {
            return Err(LanVaultError::InvalidState(format!(
                "binding `{binding_id}` is disabled, revoked, or mismatched rule_id"
            )));
        }
        if applied_revision != req.base_revision {
            return Err(LanVaultError::InvalidState(format!(
                "binding `{binding_id}` base_revision mismatch: expected {applied_revision}, got {}",
                req.base_revision
            )));
        }

        // 3. Verify active fenced sync lease.
        let lease_valid: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM sync_leases
                 WHERE rule_id = ?1 AND binding_id = ?2
                   AND fence_token = ?3 AND group_revision = ?4
                   AND rule_revision = ?5 AND expires_at >= ?6",
                params![
                    req.rule_id,
                    binding_id,
                    req.fence_token,
                    req.group_revision,
                    req.rule_revision,
                    now
                ],
                |row| row.get(0),
            )
            .optional()?;
        if lease_valid.is_none() {
            return Err(LanVaultError::InvalidState(format!(
                "stale or expired sync lease for ({}, {binding_id}) fence_token={}",
                req.rule_id, req.fence_token
            )));
        }

        // 4. Apply delta operations atomically.
        let new_revision = req.base_revision + 1;
        let mut applied_ops = 0usize;
        let mut archived_versions = 0usize;
        let mut marked_deleted = 0usize;
        let mut cleared_deleted = 0usize;

        for op in &req.ops {
            let path_key = normalize_canonical_path(&op.path_key)?;
            let existing = fetch_existing_catalog_entry(&tx, binding_id, &path_key)?;

            match op.kind {
                CatalogDeltaKind::UpsertFile | CatalogDeltaKind::UpsertDir => {
                    let new_content_hash =
                        decode_optional_hash_hex(op.content_hash_hex.as_deref())?;
                    let new_chunks = decode_chunk_hashes_hex(&op.chunk_hashes_hex)?;
                    let chunk_hashes_json = serde_json::to_string(&op.chunk_hashes_hex)?;

                    match existing {
                        Some(ex) => {
                            let same_content = ex.content_hash == new_content_hash
                                && ex.entry_kind == op.entry_kind;
                            if same_content {
                                if ex.source_deleted {
                                    tx.execute(
                                        "UPDATE catalog_entries
                                         SET display_path = ?1, mtime_ns = ?2,
                                             source_deleted = 0, source_deleted_at = NULL,
                                             updated_revision = ?3, updated_at = ?4
                                         WHERE binding_id = ?5 AND path_key = ?6",
                                        params![
                                            op.display_path,
                                            op.mtime_ns,
                                            new_revision,
                                            now,
                                            binding_id,
                                            path_key,
                                        ],
                                    )?;
                                    cleared_deleted += 1;
                                } else {
                                    tx.execute(
                                        "UPDATE catalog_entries
                                         SET display_path = ?1, mtime_ns = ?2,
                                             updated_revision = ?3, updated_at = ?4
                                         WHERE binding_id = ?5 AND path_key = ?6",
                                        params![
                                            op.display_path,
                                            op.mtime_ns,
                                            new_revision,
                                            now,
                                            binding_id,
                                            path_key,
                                        ],
                                    )?;
                                }
                            } else {
                                // Archive previous current version and transfer chunk refs with zero churn.
                                let reason =
                                    op.archive_reason.as_deref().unwrap_or("diff_replaced");
                                archive_existing_entry(
                                    &tx,
                                    &ex,
                                    vault_id,
                                    reason,
                                    &req.operation_id,
                                    now,
                                    op.expires_at,
                                )?;
                                archived_versions += 1;
                                if ex.source_deleted {
                                    cleared_deleted += 1;
                                }

                                let new_version_id = uuid::Uuid::new_v4().to_string();
                                tx.execute(
                                    "UPDATE catalog_entries
                                     SET display_path = ?1, entry_kind = ?2, version_id = ?3,
                                         size = ?4, mtime_ns = ?5, content_hash = ?6,
                                         chunk_hashes_json = ?7, source_deleted = 0,
                                         source_deleted_at = NULL, updated_revision = ?8,
                                         updated_at = ?9
                                     WHERE binding_id = ?10 AND path_key = ?11",
                                    params![
                                        op.display_path,
                                        op.entry_kind,
                                        new_version_id,
                                        op.size as i64,
                                        op.mtime_ns,
                                        new_content_hash,
                                        chunk_hashes_json,
                                        new_revision,
                                        now,
                                        binding_id,
                                        path_key,
                                    ],
                                )?;
                                attach_current_chunk_refs(
                                    &tx,
                                    &new_version_id,
                                    vault_id,
                                    &new_chunks,
                                )?;
                            }
                        }
                        None => {
                            let new_version_id = uuid::Uuid::new_v4().to_string();
                            tx.execute(
                                r#"
                                INSERT INTO catalog_entries (
                                    binding_id, path_key, display_path, entry_kind, version_id,
                                    size, mtime_ns, content_hash, chunk_hashes_json,
                                    source_deleted, source_deleted_at, updated_revision, updated_at
                                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, NULL, ?10, ?11)
                                "#,
                                params![
                                    binding_id,
                                    path_key,
                                    op.display_path,
                                    op.entry_kind,
                                    new_version_id,
                                    op.size as i64,
                                    op.mtime_ns,
                                    new_content_hash,
                                    chunk_hashes_json,
                                    new_revision,
                                    now,
                                ],
                            )?;
                            attach_current_chunk_refs(&tx, &new_version_id, vault_id, &new_chunks)?;
                        }
                    }
                    applied_ops += 1;
                }
                CatalogDeltaKind::MarkSourceDeleted => {
                    if let Some(ex) = existing {
                        // Never refresh `source_deleted_at` if already marked deleted.
                        if !ex.source_deleted {
                            tx.execute(
                                "UPDATE catalog_entries
                                 SET source_deleted = 1, source_deleted_at = ?1,
                                     updated_revision = ?2, updated_at = ?1
                                 WHERE binding_id = ?3 AND path_key = ?4",
                                params![now, new_revision, binding_id, path_key],
                            )?;
                            marked_deleted += 1;
                            applied_ops += 1;
                        }
                    }
                }
                CatalogDeltaKind::ClearSourceDeleted => {
                    if let Some(ex) = existing {
                        if ex.source_deleted {
                            tx.execute(
                                "UPDATE catalog_entries
                                 SET source_deleted = 0, source_deleted_at = NULL,
                                     updated_revision = ?1, updated_at = ?2
                                 WHERE binding_id = ?3 AND path_key = ?4",
                                params![new_revision, now, binding_id, path_key],
                            )?;
                            cleared_deleted += 1;
                            applied_ops += 1;
                        }
                    }
                }
                CatalogDeltaKind::ArchiveAndRemove => {
                    if let Some(ex) = existing {
                        let reason = op.archive_reason.as_deref().unwrap_or("mirror_removed");
                        archive_existing_entry(
                            &tx,
                            &ex,
                            vault_id,
                            reason,
                            &req.operation_id,
                            now,
                            op.expires_at,
                        )?;
                        tx.execute(
                            "DELETE FROM catalog_entries WHERE binding_id = ?1 AND path_key = ?2",
                            params![binding_id, path_key],
                        )?;
                        archived_versions += 1;
                        applied_ops += 1;
                    }
                }
                CatalogDeltaKind::MetadataOnly => {
                    if existing.is_some() {
                        tx.execute(
                            "UPDATE catalog_entries
                             SET display_path = ?1, mtime_ns = ?2,
                                 updated_revision = ?3, updated_at = ?4
                             WHERE binding_id = ?5 AND path_key = ?6",
                            params![
                                op.display_path,
                                op.mtime_ns,
                                new_revision,
                                now,
                                binding_id,
                                path_key,
                            ],
                        )?;
                        applied_ops += 1;
                    }
                }
            }
        }

        // 5. Advance binding cursor & record committed operation.
        tx.execute(
            "UPDATE rule_target_bindings
             SET applied_revision = ?1, last_source_gen = ?2,
                 status = 'synced', last_error = NULL, updated_at = ?3
             WHERE binding_id = ?4",
            params![new_revision, req.source_gen, now, binding_id],
        )?;

        let response = CatalogCommitResponse {
            operation_id: req.operation_id.clone(),
            binding_id: binding_id.to_string(),
            new_revision,
            applied_ops,
            archived_versions,
            marked_deleted,
            cleared_deleted,
            deduplicated_replay: false,
        };
        let response_json = serde_json::to_string(&response)?;
        tx.execute(
            r#"
            INSERT INTO sync_operations (
                operation_id, rule_id, binding_id, payload_hash, status,
                response_json, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, 'committed', ?5, ?6, ?6)
            "#,
            params![
                req.operation_id,
                req.rule_id,
                binding_id,
                payload_hash,
                response_json,
                now,
            ],
        )?;

        tx.commit()?;
        Ok(response)
    }

    /// Strict cursor-paginated listing of current catalog entries ordered by `path_key ASC` (no OFFSET).
    pub fn list_catalog_entries_page(
        &self,
        binding_id: &str,
        after_path_key: Option<&str>,
        limit: usize,
        source_deleted_only: bool,
    ) -> Result<Vec<CatalogEntryRecord>> {
        let conn = self.lock_read_conn();
        let limit_i64 = i64::try_from(limit.max(1)).unwrap_or(i64::MAX);
        let cursor = after_path_key.unwrap_or("");
        let sql = if source_deleted_only {
            r#"
            SELECT binding_id, path_key, display_path, entry_kind, version_id,
                   size, mtime_ns, content_hash, chunk_hashes_json,
                   source_deleted, source_deleted_at, updated_revision, updated_at
            FROM catalog_entries
            WHERE binding_id = ?1 AND source_deleted = 1 AND path_key > ?2
            ORDER BY path_key ASC
            LIMIT ?3
            "#
        } else {
            r#"
            SELECT binding_id, path_key, display_path, entry_kind, version_id,
                   size, mtime_ns, content_hash, chunk_hashes_json,
                   source_deleted, source_deleted_at, updated_revision, updated_at
            FROM catalog_entries
            WHERE binding_id = ?1 AND path_key > ?2
            ORDER BY path_key ASC
            LIMIT ?3
            "#
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt
            .query_map(params![binding_id, cursor, limit_i64], row_to_catalog_entry)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Strict cursor-paginated listing of historical versions ordered by `(archived_at DESC, version_id DESC)`.
    pub fn list_catalog_versions_page(
        &self,
        binding_id: &str,
        path_key: Option<&str>,
        after_archived_at: Option<i64>,
        after_version_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<CatalogVersionRecord>> {
        let conn = self.lock_read_conn();
        let limit_i64 = i64::try_from(limit.max(1)).unwrap_or(i64::MAX);
        match (path_key, after_archived_at, after_version_id) {
            (Some(pk), Some(ts), Some(vid)) => {
                let mut stmt = conn.prepare(
                    r#"
                    SELECT version_id, binding_id, vault_id, path_key, display_path, entry_kind,
                           size, mtime_ns, content_hash, chunk_hashes_json, archive_reason,
                           prev_state, operation_id, archived_at, expires_at, recycled_at
                    FROM catalog_versions
                    WHERE binding_id = ?1 AND path_key = ?2
                      AND (archived_at, version_id) < (?3, ?4)
                    ORDER BY archived_at DESC, version_id DESC
                    LIMIT ?5
                    "#,
                )?;
                let rows = stmt
                    .query_map(
                        params![binding_id, pk, ts, vid, limit_i64],
                        row_to_catalog_version,
                    )?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(rows)
            }
            (Some(pk), _, _) => {
                let mut stmt = conn.prepare(
                    r#"
                    SELECT version_id, binding_id, vault_id, path_key, display_path, entry_kind,
                           size, mtime_ns, content_hash, chunk_hashes_json, archive_reason,
                           prev_state, operation_id, archived_at, expires_at, recycled_at
                    FROM catalog_versions
                    WHERE binding_id = ?1 AND path_key = ?2
                    ORDER BY archived_at DESC, version_id DESC
                    LIMIT ?3
                    "#,
                )?;
                let rows = stmt
                    .query_map(params![binding_id, pk, limit_i64], row_to_catalog_version)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(rows)
            }
            (None, Some(ts), Some(vid)) => {
                let mut stmt = conn.prepare(
                    r#"
                    SELECT version_id, binding_id, vault_id, path_key, display_path, entry_kind,
                           size, mtime_ns, content_hash, chunk_hashes_json, archive_reason,
                           prev_state, operation_id, archived_at, expires_at, recycled_at
                    FROM catalog_versions
                    WHERE binding_id = ?1
                      AND (archived_at, version_id) < (?2, ?3)
                    ORDER BY archived_at DESC, version_id DESC
                    LIMIT ?4
                    "#,
                )?;
                let rows = stmt
                    .query_map(
                        params![binding_id, ts, vid, limit_i64],
                        row_to_catalog_version,
                    )?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(rows)
            }
            (None, _, _) => {
                let mut stmt = conn.prepare(
                    r#"
                    SELECT version_id, binding_id, vault_id, path_key, display_path, entry_kind,
                           size, mtime_ns, content_hash, chunk_hashes_json, archive_reason,
                           prev_state, operation_id, archived_at, expires_at, recycled_at
                    FROM catalog_versions
                    WHERE binding_id = ?1
                    ORDER BY archived_at DESC, version_id DESC
                    LIMIT ?2
                    "#,
                )?;
                let rows = stmt
                    .query_map(params![binding_id, limit_i64], row_to_catalog_version)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(rows)
            }
        }
    }

    pub fn get_catalog_version(&self, version_id: &str) -> Result<Option<CatalogVersionRecord>> {
        let conn = self.lock_conn();
        let row = conn
            .query_row(
                r#"
                SELECT version_id, binding_id, vault_id, path_key, display_path, entry_kind,
                       size, mtime_ns, content_hash, chunk_hashes_json, archive_reason,
                       prev_state, operation_id, archived_at, expires_at, recycled_at
                FROM catalog_versions
                WHERE version_id = ?1
                "#,
                params![version_id],
                row_to_catalog_version,
            )
            .optional()?;
        Ok(row)
    }

    /// Persists a `CatalogVersionRecord` (idempotent on `version_id`), used by local mirror
    /// archival to register retention-managed archive versions before mutating the target tree.
    pub fn insert_catalog_version(&self, record: &CatalogVersionRecord) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            r#"
            INSERT OR IGNORE INTO catalog_versions (
                version_id, binding_id, vault_id, path_key, display_path, entry_kind,
                size, mtime_ns, content_hash, chunk_hashes_json, archive_reason,
                prev_state, operation_id, archived_at, expires_at, recycled_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
            "#,
            params![
                record.version_id,
                record.binding_id,
                record.vault_id,
                record.path_key,
                record.display_path,
                record.entry_kind,
                record.size as i64,
                record.mtime_ns,
                record.content_hash,
                record.chunk_hashes_json,
                record.archive_reason,
                record.prev_state,
                record.operation_id,
                record.archived_at,
                record.expires_at,
                record.recycled_at,
            ],
        )?;
        Ok(())
    }

    /// Recycles expired mirror archive versions (`expires_at <= now_ts AND recycled_at IS NULL`)
    /// while excluding bindings with active leases or unfinished mirror journal entries.
    /// Releases `catalog_chunk_refs` (`owner_kind = 'version'`), decrements `chunks.ref_count`,
    /// marks `recycled_at = now_ts`, and returns `(vault_id, recycled_count)` per vault.
    pub fn recycle_expired_catalog_versions(
        &self,
        now_ts: i64,
        batch_limit: usize,
    ) -> Result<Vec<(String, usize)>> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let limit_i64 = i64::try_from(batch_limit.max(1)).unwrap_or(i64::MAX);

        let candidates: Vec<(String, String)> = {
            let mut stmt = tx.prepare(
                r#"
                SELECT cv.version_id, cv.vault_id
                FROM catalog_versions cv
                WHERE cv.expires_at IS NOT NULL
                  AND cv.expires_at <= ?1
                  AND cv.recycled_at IS NULL
                  AND NOT EXISTS (
                      SELECT 1 FROM sync_leases sl
                      WHERE sl.binding_id = cv.binding_id AND sl.expires_at > ?1
                  )
                  AND NOT EXISTS (
                      SELECT 1 FROM mirror_apply_journal mj
                      WHERE mj.archive_version_id = cv.version_id AND mj.phase != 'committed'
                  )
                ORDER BY cv.expires_at ASC, cv.version_id ASC
                LIMIT ?2
                "#,
            )?;
            let rows = stmt
                .query_map(params![now_ts, limit_i64], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };

        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for (version_id, vault_id) in candidates {
            tx.execute(
                "UPDATE chunks
                 SET ref_count = MAX(
                     0,
                     ref_count - (
                         SELECT COUNT(*) FROM catalog_chunk_refs ccr
                         WHERE ccr.owner_kind = 'version'
                           AND ccr.owner_id = ?1
                           AND ccr.vault_id = chunks.vault_id
                           AND ccr.chunk_hash = chunks.hash_blake3
                     )
                 )
                 WHERE EXISTS (
                     SELECT 1 FROM catalog_chunk_refs ccr
                     WHERE ccr.owner_kind = 'version'
                       AND ccr.owner_id = ?1
                       AND ccr.vault_id = chunks.vault_id
                       AND ccr.chunk_hash = chunks.hash_blake3
                 )",
                params![version_id],
            )?;
            tx.execute(
                "DELETE FROM catalog_chunk_refs
                 WHERE owner_kind = 'version' AND owner_id = ?1",
                params![version_id],
            )?;
            tx.execute(
                "UPDATE catalog_versions SET recycled_at = ?1 WHERE version_id = ?2",
                params![now_ts, version_id],
            )?;
            *counts.entry(vault_id).or_insert(0) += 1;
        }

        tx.commit()?;
        Ok(counts.into_iter().collect())
    }

    // -------------------------------------------------------------------------
    // Mirror Apply Journal
    // -------------------------------------------------------------------------

    pub fn upsert_mirror_journal(&self, record: &MirrorJournalRecord) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            r#"
            INSERT INTO mirror_apply_journal (
                journal_id, operation_id, rule_id, binding_id, path_key,
                source_gen, base_revision, expected_prev_hash, archive_version_id,
                staging_path, phase, error_message, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
            ON CONFLICT(journal_id) DO UPDATE SET
                archive_version_id = excluded.archive_version_id,
                staging_path = excluded.staging_path,
                phase = excluded.phase,
                error_message = excluded.error_message,
                updated_at = excluded.updated_at
            "#,
            params![
                record.journal_id,
                record.operation_id,
                record.rule_id,
                record.binding_id,
                record.path_key,
                record.source_gen,
                record.base_revision,
                record.expected_prev_hash,
                record.archive_version_id,
                record.staging_path,
                record.phase,
                record.error_message,
                record.created_at,
                record.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn list_unfinished_mirror_journals(
        &self,
        binding_id: Option<&str>,
    ) -> Result<Vec<MirrorJournalRecord>> {
        let conn = self.lock_conn();
        match binding_id {
            Some(bid) => {
                let mut stmt = conn.prepare(
                    r#"
                    SELECT journal_id, operation_id, rule_id, binding_id, path_key,
                           source_gen, base_revision, expected_prev_hash, archive_version_id,
                           staging_path, phase, error_message, created_at, updated_at
                    FROM mirror_apply_journal
                    WHERE binding_id = ?1 AND phase != 'committed'
                    ORDER BY created_at ASC
                    "#,
                )?;
                let rows = stmt
                    .query_map(params![bid], row_to_mirror_journal)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(rows)
            }
            None => {
                let mut stmt = conn.prepare(
                    r#"
                    SELECT journal_id, operation_id, rule_id, binding_id, path_key,
                           source_gen, base_revision, expected_prev_hash, archive_version_id,
                           staging_path, phase, error_message, created_at, updated_at
                    FROM mirror_apply_journal
                    WHERE phase != 'committed'
                    ORDER BY created_at ASC
                    "#,
                )?;
                let rows = stmt
                    .query_map([], row_to_mirror_journal)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(rows)
            }
        }
    }

    pub fn update_mirror_journal_phase(
        &self,
        journal_id: &str,
        phase: &str,
        error_message: Option<&str>,
    ) -> Result<bool> {
        let conn = self.lock_conn();
        let now = unix_timestamp_secs();
        let affected = conn.execute(
            "UPDATE mirror_apply_journal
             SET phase = ?1, error_message = ?2, updated_at = ?3
             WHERE journal_id = ?4",
            params![phase, error_message, now, journal_id],
        )?;
        Ok(affected > 0)
    }

    // -------------------------------------------------------------------------
    // Message Outbox & Receiver Deduplication
    // -------------------------------------------------------------------------

    /// Enqueues a message with a monotonically assigned `local_seq` and freezes its
    /// target set inside a single transaction.
    pub fn enqueue_outbox_message(
        &self,
        msg: &MessageOutboxRecord,
        targets: &[MessageOutboxTargetRecord],
    ) -> Result<i64> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let next_seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(local_seq), 0) + 1 FROM message_outbox",
            [],
            |row| row.get(0),
        )?;
        let initial_status = if targets.is_empty() {
            "failed"
        } else {
            msg.overall_status.as_str()
        };
        tx.execute(
            r#"
            INSERT INTO message_outbox (
                client_msg_id, local_seq, session_key, kind, title, content,
                file_path, file_hash_hex, size_bytes, overall_status, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            "#,
            params![
                msg.client_msg_id,
                next_seq,
                msg.session_key,
                msg.kind,
                msg.title,
                msg.content,
                msg.file_path,
                msg.file_hash_hex,
                msg.size_bytes as i64,
                initial_status,
                msg.created_at,
                msg.updated_at,
            ],
        )?;
        for target in targets {
            tx.execute(
                r#"
                INSERT INTO message_outbox_targets (
                    client_msg_id, target_device_id, channel, status,
                    error_code, error_message, attempt_count, next_retry_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                "#,
                params![
                    msg.client_msg_id,
                    target.target_device_id,
                    target.channel,
                    target.status,
                    target.error_code,
                    target.error_message,
                    target.attempt_count,
                    target.next_retry_at,
                    target.updated_at,
                ],
            )?;
        }
        tx.commit()?;
        Ok(next_seq)
    }

    pub fn list_outbox_messages(
        &self,
        session_key: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MessageOutboxRecord>> {
        let conn = self.lock_conn();
        let limit_i64 = i64::try_from(limit.max(1)).unwrap_or(i64::MAX);
        match session_key {
            Some(sk) => {
                let mut stmt = conn.prepare(
                    r#"
                    SELECT client_msg_id, local_seq, session_key, kind, title, content,
                           file_path, file_hash_hex, size_bytes, overall_status, created_at, updated_at
                    FROM message_outbox
                    WHERE session_key = ?1
                    ORDER BY local_seq DESC
                    LIMIT ?2
                    "#,
                )?;
                let rows = stmt
                    .query_map(params![sk, limit_i64], row_to_message_outbox)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(rows)
            }
            None => {
                let mut stmt = conn.prepare(
                    r#"
                    SELECT client_msg_id, local_seq, session_key, kind, title, content,
                           file_path, file_hash_hex, size_bytes, overall_status, created_at, updated_at
                    FROM message_outbox
                    ORDER BY local_seq DESC
                    LIMIT ?1
                    "#,
                )?;
                let rows = stmt
                    .query_map(params![limit_i64], row_to_message_outbox)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(rows)
            }
        }
    }

    pub fn list_outbox_targets(
        &self,
        client_msg_id: &str,
    ) -> Result<Vec<MessageOutboxTargetRecord>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            r#"
            SELECT client_msg_id, target_device_id, channel, status,
                   error_code, error_message, attempt_count, next_retry_at, updated_at
            FROM message_outbox_targets
            WHERE client_msg_id = ?1
            ORDER BY target_device_id ASC
            "#,
        )?;
        let rows = stmt
            .query_map(params![client_msg_id], row_to_message_outbox_target)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn list_pending_outbox_targets(
        &self,
        now_ts: i64,
        limit: usize,
    ) -> Result<Vec<MessageOutboxTargetRecord>> {
        let conn = self.lock_conn();
        let limit_i64 = i64::try_from(limit.max(1)).unwrap_or(i64::MAX);
        let mut stmt = conn.prepare(
            r#"
            SELECT client_msg_id, target_device_id, channel, status,
                   error_code, error_message, attempt_count, next_retry_at, updated_at
            FROM message_outbox_targets
            WHERE status IN ('sending', 'pending')
              AND (next_retry_at IS NULL OR next_retry_at <= ?1)
            ORDER BY updated_at ASC
            LIMIT ?2
            "#,
        )?;
        let rows = stmt
            .query_map(params![now_ts, limit_i64], row_to_message_outbox_target)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Updates a single target's delivery status and atomically recomputes `overall_status`
    /// on the parent `message_outbox` row.
    pub fn update_outbox_target_status(
        &self,
        client_msg_id: &str,
        target_device_id: &str,
        status: &str,
        error_code: Option<&str>,
        error_message: Option<&str>,
        attempt_count: i64,
        next_retry_at: Option<i64>,
    ) -> Result<String> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = unix_timestamp_secs();

        tx.execute(
            "UPDATE message_outbox_targets
             SET status = ?1, error_code = ?2, error_message = ?3,
                 attempt_count = ?4, next_retry_at = ?5, updated_at = ?6
             WHERE client_msg_id = ?7 AND target_device_id = ?8",
            params![
                status,
                error_code,
                error_message,
                attempt_count,
                next_retry_at,
                now,
                client_msg_id,
                target_device_id,
            ],
        )?;

        let statuses: Vec<String> = {
            let mut stmt =
                tx.prepare("SELECT status FROM message_outbox_targets WHERE client_msg_id = ?1")?;
            let rows = stmt
                .query_map(params![client_msg_id], |row| row.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };

        let overall = compute_overall_outbox_status(&statuses);
        tx.execute(
            "UPDATE message_outbox
             SET overall_status = ?1, updated_at = ?2
             WHERE client_msg_id = ?3",
            params![overall, now, client_msg_id],
        )?;
        tx.commit()?;
        Ok(overall.to_string())
    }

    /// Resets only failed targets of `client_msg_id` back to `'sending'` for retry.
    pub fn reset_failed_outbox_targets_for_retry(&self, client_msg_id: &str) -> Result<usize> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = unix_timestamp_secs();

        let reset = tx.execute(
            "UPDATE message_outbox_targets
             SET status = 'sending', error_code = NULL, error_message = NULL,
                 next_retry_at = NULL, updated_at = ?1
             WHERE client_msg_id = ?2 AND status = 'failed'",
            params![now, client_msg_id],
        )?;
        if reset > 0 {
            tx.execute(
                "UPDATE message_outbox
                 SET overall_status = 'sending', updated_at = ?1
                 WHERE client_msg_id = ?2",
                params![now, client_msg_id],
            )?;
        }
        tx.commit()?;
        Ok(reset)
    }

    /// Checks whether `(sender_device_id, client_msg_id, receiver_device_id)` has already been
    /// processed. Returns `Ok(Some(existing_ack_json))` on duplicate, or inserts and returns `Ok(None)`.
    pub fn record_or_check_message_dedup(
        &self,
        sender_device_id: &str,
        client_msg_id: &str,
        receiver_device_id: &str,
        ack_json: &str,
    ) -> Result<Option<String>> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = tx
            .query_row(
                "SELECT ack_json FROM message_dedup_receipts
                 WHERE sender_device_id = ?1 AND client_msg_id = ?2 AND receiver_device_id = ?3",
                params![sender_device_id, client_msg_id, receiver_device_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            tx.commit()?;
            return Ok(Some(existing));
        }
        let now = unix_timestamp_secs();
        tx.execute(
            "INSERT INTO message_dedup_receipts (
                 sender_device_id, client_msg_id, receiver_device_id, ack_json, received_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                sender_device_id,
                client_msg_id,
                receiver_device_id,
                ack_json,
                now,
            ],
        )?;
        tx.commit()?;
        Ok(None)
    }

    // -------------------------------------------------------------------------
    // Legacy Sync Rules Migration (Idempotent, Never Guesses active_peer)
    // -------------------------------------------------------------------------

    /// Migrates legacy `sync_rules` into `device_groups`, `group_sync_rules` (strictly
    /// `diff_backup` mode — never enables mirror mode), and `rule_target_bindings`.
    ///
    /// Idempotent across repeated calls. Never guesses from `active_peer`: when a legacy
    /// rule targets `"default"` and cannot be uniquely resolved from historical `snapshots`,
    /// it is imported with `needs_binding_confirm = true` and a disabled pending binding.
    pub fn migrate_legacy_rules_to_groups(&self, local_device_id: &str) -> Result<usize> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = unix_timestamp_secs();

        struct LegacyRule {
            rule_id: String,
            local_dir: String,
            target_vault_id: String,
            wifi_only: bool,
            allowed_ssids: Option<String>,
            require_charging: bool,
            max_kbps: u32,
            enabled: bool,
        }

        let legacy_rules: Vec<LegacyRule> = {
            let mut stmt = tx.prepare(
                "SELECT rule_id, local_dir, target_vault_id, wifi_only,
                        allowed_ssids, require_charging, max_kbps, enabled
                 FROM sync_rules
                 WHERE rule_id NOT IN (SELECT rule_id FROM group_sync_rules)
                 ORDER BY rule_id ASC",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(LegacyRule {
                        rule_id: row.get(0)?,
                        local_dir: row.get(1)?,
                        target_vault_id: row.get(2)?,
                        wifi_only: row.get::<_, i64>(3)? != 0,
                        allowed_ssids: row.get(4)?,
                        require_charging: row.get::<_, i64>(5)? != 0,
                        max_kbps: row.get::<_, i64>(6)? as u32,
                        enabled: row.get::<_, i64>(7)? != 0,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };

        let mut migrated = 0usize;
        for rule in legacy_rules {
            // Attempt deterministic resolution solely from historical snapshots (never active_peer).
            let resolved: Option<(String, String)> = {
                let mut stmt = tx.prepare(
                    "SELECT DISTINCT device_id, vault_id
                     FROM snapshots
                     WHERE rule_id = ?1",
                )?;
                let pairs: Vec<(String, String)> = stmt
                    .query_map(params![rule.rule_id], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                if pairs.len() == 1 {
                    Some(pairs[0].clone())
                } else if rule.target_vault_id != "default" && !rule.target_vault_id.is_empty() {
                    let mut vstmt =
                        tx.prepare("SELECT DISTINCT device_id FROM snapshots WHERE vault_id = ?1")?;
                    let devs: Vec<String> = vstmt
                        .query_map(params![rule.target_vault_id], |row| row.get(0))?
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    if devs.len() == 1 {
                        Some((devs[0].clone(), rule.target_vault_id.clone()))
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            let (group_id, group_name, needs_confirm, target_dev, target_vault) = match resolved {
                Some((dev_id, vault_id)) => (
                    format!("migrated-{dev_id}"),
                    format!("Migrated Group ({dev_id})"),
                    false,
                    Some(dev_id),
                    Some(vault_id),
                ),
                None => {
                    let v = if rule.target_vault_id == "default" || rule.target_vault_id.is_empty()
                    {
                        None
                    } else {
                        Some(rule.target_vault_id.clone())
                    };
                    (
                        "migrated-unassigned".to_string(),
                        "Migrated Rules (Pending Binding)".to_string(),
                        true,
                        None,
                        v,
                    )
                }
            };

            // Ensure migration group exists.
            tx.execute(
                "INSERT OR IGNORE INTO device_groups (
                     group_id, name, coordinator_device_id, revision, created_at, updated_at, deleted_at
                 ) VALUES (?1, ?2, ?3, 1, ?4, ?4, NULL)",
                params![group_id, group_name, local_device_id, now],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO device_group_members (
                     group_id, device_id, role, added_at, removed_at
                 ) VALUES (?1, ?2, 'coordinator', ?3, NULL)",
                params![group_id, local_device_id, now],
            )?;
            if let Some(ref td) = target_dev {
                if td != local_device_id {
                    tx.execute(
                        "INSERT OR IGNORE INTO device_group_members (
                             group_id, device_id, role, added_at, removed_at
                         ) VALUES (?1, ?2, 'member', ?3, NULL)",
                        params![group_id, td, now],
                    )?;
                }
            }

            tx.execute(
                r#"
                INSERT INTO group_sync_rules (
                    rule_id, group_id, name, mode, source_device_id, source_root, source_root_id,
                    archive_retention_days, wifi_only, allowed_ssids, require_charging, max_kbps,
                    schedule_interval_secs, enabled, revision, group_revision, needs_binding_confirm,
                    created_at, updated_at
                ) VALUES (
                    ?1, ?2, ?3, 'diff_backup', ?4, ?5, NULL,
                    30, ?6, ?7, ?8, ?9,
                    0, ?10, 1, 1, ?11, ?12, ?12
                )
                "#,
                params![
                    rule.rule_id,
                    group_id,
                    rule.local_dir,
                    local_device_id,
                    rule.local_dir,
                    i64::from(rule.wifi_only),
                    rule.allowed_ssids,
                    i64::from(rule.require_charging),
                    i64::from(rule.max_kbps),
                    i64::from(rule.enabled && !needs_confirm),
                    i64::from(needs_confirm),
                    now,
                ],
            )?;

            let binding_id = format!("migrated-binding-{}", rule.rule_id);
            let binding_target_dev = target_dev.unwrap_or_default();
            let binding_status = if needs_confirm {
                "needs_binding_confirm"
            } else {
                "pending"
            };
            tx.execute(
                r#"
                INSERT INTO rule_target_bindings (
                    binding_id, rule_id, group_id, target_device_id, target_vault_id,
                    mirror_root_id, target_root, enabled, revoked_at, applied_revision,
                    last_source_gen, status, last_error, needs_binding_confirm,
                    created_at, updated_at
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, NULL, NULL, ?6, NULL, 0, 0, ?7, NULL, ?8, ?9, ?9
                )
                "#,
                params![
                    binding_id,
                    rule.rule_id,
                    group_id,
                    binding_target_dev,
                    target_vault,
                    i64::from(rule.enabled && !needs_confirm),
                    binding_status,
                    i64::from(needs_confirm),
                    now,
                ],
            )?;

            migrated += 1;
        }

        tx.commit()?;
        Ok(migrated)
    }
}

// -----------------------------------------------------------------------------
// Internal Helpers & Row Mappers
// -----------------------------------------------------------------------------

fn fetch_existing_catalog_entry(
    tx: &Transaction<'_>,
    binding_id: &str,
    path_key: &str,
) -> Result<Option<CatalogEntryRecord>> {
    let row = tx
        .query_row(
            r#"
            SELECT binding_id, path_key, display_path, entry_kind, version_id,
                   size, mtime_ns, content_hash, chunk_hashes_json,
                   source_deleted, source_deleted_at, updated_revision, updated_at
            FROM catalog_entries
            WHERE binding_id = ?1 AND path_key = ?2
            "#,
            params![binding_id, path_key],
            row_to_catalog_entry,
        )
        .optional()?;
    Ok(row)
}

fn archive_existing_entry(
    tx: &Transaction<'_>,
    ex: &CatalogEntryRecord,
    vault_id: &str,
    archive_reason: &str,
    operation_id: &str,
    archived_at: i64,
    expires_at: Option<i64>,
) -> Result<()> {
    let prev_state = if ex.source_deleted {
        "source_deleted"
    } else {
        "present"
    };
    tx.execute(
        r#"
        INSERT INTO catalog_versions (
            version_id, binding_id, vault_id, path_key, display_path, entry_kind,
            size, mtime_ns, content_hash, chunk_hashes_json, archive_reason,
            prev_state, operation_id, archived_at, expires_at, recycled_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, NULL)
        "#,
        params![
            ex.version_id,
            ex.binding_id,
            vault_id,
            ex.path_key,
            ex.display_path,
            ex.entry_kind,
            ex.size as i64,
            ex.mtime_ns,
            ex.content_hash,
            ex.chunk_hashes_json,
            archive_reason,
            prev_state,
            operation_id,
            archived_at,
            expires_at,
        ],
    )?;
    // Transfer chunk refs from ('current', version_id) -> ('version', version_id) with zero refcount churn.
    tx.execute(
        "UPDATE catalog_chunk_refs
         SET owner_kind = 'version'
         WHERE owner_kind = 'current' AND owner_id = ?1",
        params![ex.version_id],
    )?;
    Ok(())
}

fn attach_current_chunk_refs(
    tx: &Transaction<'_>,
    version_id: &str,
    vault_id: &str,
    chunk_hashes: &[[u8; 32]],
) -> Result<()> {
    for hash in chunk_hashes {
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO catalog_chunk_refs (owner_kind, owner_id, vault_id, chunk_hash)
             VALUES ('current', ?1, ?2, ?3)",
            params![version_id, vault_id, hash.as_slice()],
        )?;
        if inserted == 1 {
            let affected = tx.execute(
                "UPDATE chunks SET ref_count = ref_count + 1
                 WHERE vault_id = ?1 AND hash_blake3 = ?2",
                params![vault_id, hash.as_slice()],
            )?;
            if affected != 1 {
                return Err(LanVaultError::NotFound(format!(
                    "chunk {} in vault `{vault_id}` for catalog version `{version_id}`",
                    hex::encode(hash)
                )));
            }
        }
    }
    Ok(())
}

fn decode_optional_hash_hex(raw: Option<&str>) -> Result<Option<Vec<u8>>> {
    match raw {
        None => Ok(None),
        Some(h) => {
            let bytes = hex::decode(h).map_err(|e| {
                LanVaultError::InvalidInput(format!("invalid hex content_hash `{h}`: {e}"))
            })?;
            if bytes.len() != 32 {
                return Err(LanVaultError::InvalidHashLength {
                    expected: 32,
                    actual: bytes.len(),
                });
            }
            Ok(Some(bytes))
        }
    }
}

fn decode_chunk_hashes_hex(hashes_hex: &[String]) -> Result<Vec<[u8; 32]>> {
    let mut out = Vec::with_capacity(hashes_hex.len());
    for h in hashes_hex {
        let bytes = hex::decode(h).map_err(|e| {
            LanVaultError::InvalidInput(format!("invalid hex chunk hash `{h}`: {e}"))
        })?;
        let arr: [u8; 32] =
            bytes
                .try_into()
                .map_err(|v: Vec<u8>| LanVaultError::InvalidHashLength {
                    expected: 32,
                    actual: v.len(),
                })?;
        out.push(arr);
    }
    Ok(out)
}

fn compute_overall_outbox_status(statuses: &[String]) -> &'static str {
    if statuses.is_empty() {
        return "failed";
    }
    if statuses.iter().any(|s| s == "sending" || s == "pending") {
        return "sending";
    }
    let delivered = statuses
        .iter()
        .filter(|s| s.as_str() == "delivered")
        .count();
    let sent = statuses.iter().filter(|s| s.as_str() == "sent").count();
    let succeeded = delivered + sent;
    if delivered == statuses.len() {
        "delivered"
    } else if succeeded == statuses.len() {
        "sent"
    } else if succeeded > 0 {
        "partial_failed"
    } else {
        "failed"
    }
}

fn row_to_device_group(row: &rusqlite::Row<'_>) -> rusqlite::Result<DeviceGroupRecord> {
    Ok(DeviceGroupRecord {
        group_id: row.get(0)?,
        name: row.get(1)?,
        coordinator_device_id: row.get(2)?,
        revision: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        deleted_at: row.get(6)?,
    })
}

fn row_to_device_group_member(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<DeviceGroupMemberRecord> {
    Ok(DeviceGroupMemberRecord {
        group_id: row.get(0)?,
        device_id: row.get(1)?,
        role: row.get(2)?,
        added_at: row.get(3)?,
        removed_at: row.get(4)?,
    })
}

fn row_to_group_sync_rule(row: &rusqlite::Row<'_>) -> rusqlite::Result<GroupSyncRuleRecord> {
    let mode_str: String = row.get(3)?;
    let mode = SyncRuleMode::parse(&mode_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(GroupSyncRuleRecord {
        rule_id: row.get(0)?,
        group_id: row.get(1)?,
        name: row.get(2)?,
        mode,
        source_device_id: row.get(4)?,
        source_root: row.get(5)?,
        source_root_id: row.get(6)?,
        archive_retention_days: row.get::<_, i64>(7)?.max(0) as u32,
        wifi_only: row.get::<_, i64>(8)? != 0,
        allowed_ssids: row.get(9)?,
        require_charging: row.get::<_, i64>(10)? != 0,
        max_kbps: row.get::<_, i64>(11)?.max(0) as u32,
        schedule_interval_secs: row.get::<_, i64>(12)?.max(0) as u64,
        enabled: row.get::<_, i64>(13)? != 0,
        revision: row.get(14)?,
        group_revision: row.get(15)?,
        needs_binding_confirm: row.get::<_, i64>(16)? != 0,
        created_at: row.get(17)?,
        updated_at: row.get(18)?,
    })
}

fn row_to_rule_target_binding(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<RuleTargetBindingRecord> {
    Ok(RuleTargetBindingRecord {
        binding_id: row.get(0)?,
        rule_id: row.get(1)?,
        group_id: row.get(2)?,
        target_device_id: row.get(3)?,
        target_vault_id: row.get(4)?,
        mirror_root_id: row.get(5)?,
        target_root: row.get(6)?,
        enabled: row.get::<_, i64>(7)? != 0,
        revoked_at: row.get(8)?,
        applied_revision: row.get(9)?,
        last_source_gen: row.get(10)?,
        status: row.get(11)?,
        last_error: row.get(12)?,
        needs_binding_confirm: row.get::<_, i64>(13)? != 0,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
    })
}

fn row_to_mirror_root(row: &rusqlite::Row<'_>) -> rusqlite::Result<MirrorRootRegistration> {
    Ok(MirrorRootRegistration {
        mirror_root_id: row.get(0)?,
        device_id: row.get(1)?,
        canonical_root: row.get(2)?,
        root_identity: row.get(3)?,
        label: row.get(4)?,
        created_at: row.get(5)?,
        revoked_at: row.get(6)?,
    })
}

fn row_to_sync_lease(row: &rusqlite::Row<'_>) -> rusqlite::Result<SyncLeaseRecord> {
    Ok(SyncLeaseRecord {
        rule_id: row.get(0)?,
        binding_id: row.get(1)?,
        holder_device_id: row.get(2)?,
        fence_token: row.get(3)?,
        group_revision: row.get(4)?,
        rule_revision: row.get(5)?,
        acquired_at: row.get(6)?,
        expires_at: row.get(7)?,
    })
}

fn row_to_sync_operation(row: &rusqlite::Row<'_>) -> rusqlite::Result<SyncOperationRecord> {
    Ok(SyncOperationRecord {
        operation_id: row.get(0)?,
        rule_id: row.get(1)?,
        binding_id: row.get(2)?,
        payload_hash: row.get(3)?,
        status: row.get(4)?,
        response_json: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn row_to_catalog_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<CatalogEntryRecord> {
    Ok(CatalogEntryRecord {
        binding_id: row.get(0)?,
        path_key: row.get(1)?,
        display_path: row.get(2)?,
        entry_kind: row.get(3)?,
        version_id: row.get(4)?,
        size: row.get::<_, i64>(5)?.max(0) as u64,
        mtime_ns: row.get(6)?,
        content_hash: row.get(7)?,
        chunk_hashes_json: row.get(8)?,
        source_deleted: row.get::<_, i64>(9)? != 0,
        source_deleted_at: row.get(10)?,
        updated_revision: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

fn row_to_catalog_version(row: &rusqlite::Row<'_>) -> rusqlite::Result<CatalogVersionRecord> {
    Ok(CatalogVersionRecord {
        version_id: row.get(0)?,
        binding_id: row.get(1)?,
        vault_id: row.get(2)?,
        path_key: row.get(3)?,
        display_path: row.get(4)?,
        entry_kind: row.get(5)?,
        size: row.get::<_, i64>(6)?.max(0) as u64,
        mtime_ns: row.get(7)?,
        content_hash: row.get(8)?,
        chunk_hashes_json: row.get(9)?,
        archive_reason: row.get(10)?,
        prev_state: row.get(11)?,
        operation_id: row.get(12)?,
        archived_at: row.get(13)?,
        expires_at: row.get(14)?,
        recycled_at: row.get(15)?,
    })
}

fn row_to_mirror_journal(row: &rusqlite::Row<'_>) -> rusqlite::Result<MirrorJournalRecord> {
    Ok(MirrorJournalRecord {
        journal_id: row.get(0)?,
        operation_id: row.get(1)?,
        rule_id: row.get(2)?,
        binding_id: row.get(3)?,
        path_key: row.get(4)?,
        source_gen: row.get(5)?,
        base_revision: row.get(6)?,
        expected_prev_hash: row.get(7)?,
        archive_version_id: row.get(8)?,
        staging_path: row.get(9)?,
        phase: row.get(10)?,
        error_message: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

fn row_to_message_outbox(row: &rusqlite::Row<'_>) -> rusqlite::Result<MessageOutboxRecord> {
    Ok(MessageOutboxRecord {
        client_msg_id: row.get(0)?,
        local_seq: row.get(1)?,
        session_key: row.get(2)?,
        kind: row.get(3)?,
        title: row.get(4)?,
        content: row.get(5)?,
        file_path: row.get(6)?,
        file_hash_hex: row.get(7)?,
        size_bytes: row.get::<_, i64>(8)?.max(0) as u64,
        overall_status: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn row_to_message_outbox_target(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<MessageOutboxTargetRecord> {
    Ok(MessageOutboxTargetRecord {
        client_msg_id: row.get(0)?,
        target_device_id: row.get(1)?,
        channel: row.get(2)?,
        status: row.get(3)?,
        error_code: row.get(4)?,
        error_message: row.get(5)?,
        attempt_count: row.get(6)?,
        next_retry_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}
