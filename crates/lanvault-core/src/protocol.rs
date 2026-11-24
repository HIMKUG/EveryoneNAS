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
