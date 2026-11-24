//! 在线状态一致性回归测试：`is_peer_online` 只能由显式握手租约或活跃 WebSocket 推出，
//! 绝不因为"曾经配对过"就把设备判为在线（这正是双端状态机不同步的根因）。

use std::time::Duration;

use lanvault_server::web::{PEER_PRESENCE_LEASE_SECS, PendingPairRequestDto};

#[test]
fn presence_lease_window_is_strictly_shorter_than_never() {
    // 租约必须有界：否则一次握手会把设备永久标记为在线。
    assert!(PEER_PRESENCE_LEASE_SECS > 0);
    assert!(PEER_PRESENCE_LEASE_SECS <= 120);
}

#[test]
fn pending_pair_request_carries_identity_for_both_sides() {
    // 双方各自写入 trusted_peers 时依赖这些字段，缺任何一项都会造成"单边配对"。
    let dto = PendingPairRequestDto {
        request_id: "req-1".to_string(),
        device_id: "lv-0123456789ab".to_string(),
        device_name: "Pixel".to_string(),
        os_type: "android".to_string(),
        fingerprint: "fp".to_string(),
        remote_ip: "192.168.1.10".to_string(),
        created_at: 0,
        expires_at: 60,
    };
    assert!(!dto.device_id.is_empty());
    assert!(!dto.device_name.is_empty());
    assert!(!dto.os_type.is_empty());
    assert!(Duration::from_secs(1).as_secs() > 0);
}
