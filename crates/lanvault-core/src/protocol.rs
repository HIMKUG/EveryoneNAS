//! Shared protocol domain types for LanVault HTTP/WebSocket transport.
//!
//! Legacy unused binary framing (`WireFrame` / `ProtocolMessage`) was removed during
//! Draft-to-Opus consolidation; all control and transfer paths use authenticated HTTP/WS.

use serde::{Deserialize, Serialize};

/// Payload kind for LAN clipboard and temporary file quick-transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardKind {
    Text,
    File,
}
