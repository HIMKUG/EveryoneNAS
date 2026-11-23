//! Shared protocol domain types for LanVault HTTP/WebSocket transport.
//!
//! Legacy unused binary framing (`WireFrame` / `ProtocolMessage`) was removed during
//! Draft-to-Opus consolidation; all control and transfer paths use authenticated HTTP/WS.

use serde::{Deserialize, Serialize};

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
