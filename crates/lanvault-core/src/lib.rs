//! `lanvault-core`: shared domain primitives, cryptography, FastCDC chunking,
//! SQLite WAL persistence, mDNS+UDP discovery, and wire protocol frames.

use serde::{Deserialize, Serialize};

pub mod chunker;
pub mod crypto;
pub mod db;
pub mod debug_telemetry;
pub mod discovery;
pub mod error;
pub mod protocol;

pub use chunker::{
    blake3_hash, blake3_hash_reader, chunk_bytes, chunk_stream, compress_zstd, decompress_zstd,
    pack_chunk, pack_chunk_with_context, unpack_chunk, ChunkDescriptor, ChunkParams, PackedChunk, CHUNK_AVG_SIZE,
    CHUNK_MAX_SIZE, CHUNK_MIN_SIZE, DEFAULT_ZSTD_LEVEL,
};
pub use crypto::{
    compute_pin_mac, constant_time_eq, decrypt_aead, encrypt_aead, encrypt_aead_deterministic, ChunkSealContext,
    public_key_fingerprint, verify_ed25519, verify_pin_mac, DeviceIdentity, SessionKeyExchange,
    SymmetricKey,
};
pub use db::{
    ChunkRecord, Database, LocalFileStateRecord, PoolMode, RetentionTier, ScrubStatus,
    SnapshotRecord, SyncRuleRecord, TrustedPeer, VaultRecord, VaultVolumeRecord, VolumeRole,
    WalCheckpointReport,
};
pub use discovery::{
    DiscoveredPeerInfo, DiscoveryAnnouncement, DiscoveryBrowser, MdnsRegistrar, MDNS_SERVICE_TYPE,
    UDP_FALLBACK_PORT,
};
pub use protocol::{
    pairing_identity_message, pairing_response_message, CLIPBOARD_AAD,
};
pub use debug_telemetry::{
    clear_debug_logs, configure_debug_log_file, get_debug_logs, install_rust_panic_hook,
    is_debug_enabled, record_debug, record_event_always, record_frontend_log, set_debug_mode,
    DebugLogEntry, ProbeGuard,
};
pub use error::{LanVaultError, Result};
pub use protocol::ClipboardKind;

/// 统一软件三态运行模式：`client_only`（仅客户端）、`server_only`（仅服务端）、`dual`（双端模式）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleMode {
    ClientOnly,
    ServerOnly,
    #[default]
    Dual,
}

impl RoleMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClientOnly => "client_only",
            Self::ServerOnly => "server_only",
            Self::Dual => "dual",
        }
    }
}

impl std::fmt::Display for RoleMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 仅在外部输入边界解析并校验三态运行模式字符串。
pub fn parse_role_mode(raw: &str) -> Result<RoleMode> {
    match raw.trim() {
        "client_only" => Ok(RoleMode::ClientOnly),
        "server_only" => Ok(RoleMode::ServerOnly),
        "dual" => Ok(RoleMode::Dual),
        other => Err(LanVaultError::InvalidInput(format!(
            "unsupported role_mode `{other}`: expected `client_only`, `server_only`, or `dual`"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_role_mode, RoleMode};

    #[test]
    fn parse_role_mode_accepts_valid_snake_case_values() {
        assert_eq!(parse_role_mode("client_only").unwrap(), RoleMode::ClientOnly);
        assert_eq!(parse_role_mode("server_only").unwrap(), RoleMode::ServerOnly);
        assert_eq!(parse_role_mode("dual").unwrap(), RoleMode::Dual);
        assert_eq!(parse_role_mode("  dual  ").unwrap(), RoleMode::Dual);
    }

    #[test]
    fn parse_role_mode_rejects_invalid_values() {
        assert!(parse_role_mode("").is_err());
        assert!(parse_role_mode("hybrid").is_err());
        assert!(parse_role_mode("Dual").is_err());
    }

    #[test]
    fn role_mode_serde_roundtrip() {
        assert_eq!(
            serde_json::to_string(&RoleMode::ClientOnly).unwrap(),
            "\"client_only\""
        );
        assert_eq!(
            serde_json::from_str::<RoleMode>("\"server_only\"").unwrap(),
            RoleMode::ServerOnly
        );
        assert_eq!(RoleMode::default(), RoleMode::Dual);
    }
}
