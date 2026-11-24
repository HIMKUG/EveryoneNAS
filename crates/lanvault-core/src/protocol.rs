//! Shared protocol domain types for LanVault HTTP/WebSocket transport.
//!
//! Legacy unused binary framing (`WireFrame` / `ProtocolMessage`) was removed during
//! Draft-to-Opus consolidation; all control and transfer paths use authenticated HTTP/WS.

use serde::{Deserialize, Serialize};

use crate::error::{LanVaultError, Result};

pub const CLIPBOARD_AAD: &[u8] = b"lanvault.clipboard.v1";

/// Payload kind for LAN clipboard and temporary file quick-transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardKind {
    Text,
    File,
}

pub fn pairing_identity_message(
    pin: &str,
    client_ed25519_pubkey: &[u8],
    client_x25519_pubkey: &[u8],
    device_name: &str,
    os_type: &str,
) -> Vec<u8> {
    let fields = [
        b"lanvault.pair.identity.v1".as_slice(),
        pin.as_bytes(),
        client_ed25519_pubkey,
        client_x25519_pubkey,
        device_name.as_bytes(),
        os_type.as_bytes(),
    ];
    let mut message = Vec::new();
    for field in fields {
        message.extend_from_slice(&(field.len() as u64).to_le_bytes());
        message.extend_from_slice(field);
    }
    message
}

pub fn pairing_response_message(
    pin: &str,
    client_ed25519_pubkey: &[u8],
    client_x25519_pubkey: &[u8],
    server_ed25519_pubkey: &[u8],
    server_x25519_pubkey: &[u8],
    server_device_id: &str,
    clipboard_key_envelope: &[u8],
) -> Vec<u8> {
    let fields = [
        b"lanvault.pair.response.v1".as_slice(),
        pin.as_bytes(),
        client_ed25519_pubkey,
        client_x25519_pubkey,
        server_ed25519_pubkey,
        server_x25519_pubkey,
        server_device_id.as_bytes(),
        clipboard_key_envelope,
    ];
    let mut message = Vec::new();
    for field in fields {
        message.extend_from_slice(&(field.len() as u64).to_le_bytes());
        message.extend_from_slice(field);
    }
    message
}

/// Canonical length-prefixed transcript signed by a paired device to renew its session.
/// Binds the signature to the target server instance (`server_device_id`), the client device
/// (`client_device_id`), the single-use server-issued `challenge`, and the renewal purpose.
pub fn session_renew_message(
    server_device_id: &str,
    client_device_id: &str,
    challenge: &str,
) -> Vec<u8> {
    let fields = [
        b"lanvault.session.renew.v1".as_slice(),
        server_device_id.as_bytes(),
        client_device_id.as_bytes(),
        challenge.as_bytes(),
    ];
    let mut message = Vec::new();
    for field in fields {
        message.extend_from_slice(&(field.len() as u64).to_le_bytes());
        message.extend_from_slice(field);
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_renew_message_is_domain_separated_and_bound() {
        let a = session_renew_message("lv-server1", "lv-client1", "chal-1");
        let b = session_renew_message("lv-server2", "lv-client1", "chal-1");
        let c = session_renew_message("lv-server1", "lv-client2", "chal-1");
        let d = session_renew_message("lv-server1", "lv-client1", "chal-2");
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
    }
}
// -----------------------------------------------------------------------------
// v2 Protocol Capabilities, Incremental Catalog, Mirror Sync & Messaging DTOs
// -----------------------------------------------------------------------------

/// Negotiated v2 feature capabilities exchanged between LanVault peers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolCapabilities {
    pub protocol_version: u32,
    pub device_groups: bool,
    pub incremental_catalog: bool,
    pub mirror_sync: bool,
    pub message_idempotency: bool,
}

impl ProtocolCapabilities {
    pub const fn current() -> Self {
        Self {
            protocol_version: 2,
            device_groups: true,
            incremental_catalog: true,
            mirror_sync: true,
            message_idempotency: true,
        }
    }
}

/// Execution mode for a group sync rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncRuleMode {
    DiffBackup,
    MirrorSync,
}

impl SyncRuleMode {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::DiffBackup => "diff_backup",
            Self::MirrorSync => "mirror_sync",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "diff_backup" => Ok(Self::DiffBackup),
            "mirror_sync" => Ok(Self::MirrorSync),
            other => Err(LanVaultError::InvalidInput(format!(
                "invalid sync rule mode: `{other}`"
            ))),
        }
    }
}

/// Atomic mutation kind inside an incremental catalog commit batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogDeltaKind {
    UpsertFile,
    UpsertDir,
    MarkSourceDeleted,
    ClearSourceDeleted,
    ArchiveAndRemove,
    MetadataOnly,
}

/// Single path operation within a `CatalogCommitRequest`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogDeltaOp {
    pub path_key: String,
    pub display_path: String,
    pub entry_kind: String,
    pub kind: CatalogDeltaKind,
    pub size: u64,
    pub mtime_ns: i64,
    pub content_hash_hex: Option<String>,
    pub chunk_hashes_hex: Vec<String>,
    pub archive_reason: Option<String>,
    pub expires_at: Option<i64>,
}

/// Idempotent incremental catalog commit request guarded by revision and fence token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogCommitRequest {
    pub operation_id: String,
    pub rule_id: String,
    pub group_revision: i64,
    pub rule_revision: i64,
    pub fence_token: i64,
    pub base_revision: i64,
    pub source_gen: i64,
    pub ops: Vec<CatalogDeltaOp>,
}

/// Result of applying (or replaying) a `CatalogCommitRequest`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogCommitResponse {
    pub operation_id: String,
    pub binding_id: String,
    pub new_revision: i64,
    pub applied_ops: usize,
    pub archived_versions: usize,
    pub marked_deleted: usize,
    pub cleared_deleted: usize,
    pub deduplicated_replay: bool,
}

/// Request to check which content-addressed chunks already exist in a target vault.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkBatchCheckRequest {
    pub hashes_hex: Vec<String>,
}

/// Partitioned presence result for `ChunkBatchCheckRequest`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkBatchCheckResponse {
    pub present: Vec<String>,
    pub missing: Vec<String>,
}

/// Dry-run preview summary before enabling or modifying a mirror sync target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MirrorPreviewReport {
    pub rule_id: String,
    pub binding_id: String,
    pub target_device_id: String,
    pub add_count: usize,
    pub overwrite_count: usize,
    pub remove_count: usize,
    pub unchanged_count: usize,
    pub blocked_count: usize,
    pub conflicts: Vec<String>,
}

/// Per-target delivery/acceptance status for a multi-target message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageTargetAck {
    pub target_device_id: String,
    pub status: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

/// v2 idempotent message send response carrying per-target statuses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageSendResponseV2 {
    pub client_msg_id: String,
    pub delivered_to: usize,
    pub accepted_to: usize,
    pub server_ack_at: i64,
    pub targets: Vec<MessageTargetAck>,
}
