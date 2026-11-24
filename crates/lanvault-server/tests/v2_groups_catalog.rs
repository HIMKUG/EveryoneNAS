//! Integration tests for LanVault v2 server-side features:
//! 1. 3-device group with 2 targets (1 offline catching up later)
//! 2. Peer revocation fencing out stale leases/commits + topology/mirror-root validation
//! 3. Diff backup 5 states (add, same, diff-content, source-deleted, reappear same/diff, idempotent retry)
//! 4. N-day mirror archive GC protecting diff_backup & source_deleted chunks
//! 5. Message dedup receipts, per-target online/offline/revoked status, WS broadcasts & clipboard history

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use lanvault_core::{
    blake3_hash, CatalogCommitRequest, CatalogCommitResponse, CatalogDeltaKind, CatalogDeltaOp,
    ChunkBatchCheckRequest, ChunkBatchCheckResponse, Database, PoolMode, ProtocolCapabilities,
    TrustedPeer,
};
use lanvault_server::gc::{GarbageCollector, GfsPolicy};
use lanvault_server::vault::VaultStore;
use lanvault_server::web::{AppState, WsServerEvent};
use lanvault_server::{serve_with_shutdown_and_state, ServerConfig};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;

fn unix_timestamp_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before Unix epoch")
        .as_secs() as i64
}

struct TestServer {
    root: PathBuf,
    addr: SocketAddr,
    state: Arc<AppState>,
    admin_cookie: String,
    default_vault_id: String,
    shutdown_tx: Option<oneshot::Sender<()>>,
    _server_task: tokio::task::JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        self._server_task.abort();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

async fn start_test_server(tag: &str) -> TestServer {
    let root = std::env::temp_dir().join(format!("lanvault-v2-{tag}-{}", uuid::Uuid::new_v4()));
    let vault_root = root.join("vault-primary");
    std::fs::create_dir_all(&vault_root).unwrap();

    let config = ServerConfig {
        bind_addr: "127.0.0.1".to_string(),
        port: 0,
        vault_root,
        default_quota_bytes: 128 * 1024 * 1024,
        web_password: "test-admin-password-123".to_string(),
        device_name: format!("TestServer-{tag}"),
        keep_alive_level: "TEST".to_string(),
    };

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let (addr_tx, addr_rx) = oneshot::channel();
    let (state_tx, state_rx) = oneshot::channel();

    let server_task = tokio::spawn(async move {
        let _ =
            serve_with_shutdown_and_state(config, shutdown_rx, Some(addr_tx), Some(state_tx)).await;
    });

    let addr = addr_rx.await.expect("server bound address");
    let state = state_rx.await.expect("server AppState");
    let admin_cookie = state.auth.issue_session();
    let default_vault_id = state
        .db
        .list_vaults()
        .unwrap()
        .into_iter()
        .next()
        .expect("default vault initialized")
        .vault_id;

    TestServer {
        root,
        addr,
        state,
        admin_cookie,
        default_vault_id,
        shutdown_tx: Some(shutdown_tx),
        _server_task: server_task,
    }
}

fn decode_chunked_body(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < raw.len() {
        let Some(rel_crlf) = raw[pos..].windows(2).position(|w| w == b"\r\n") else {
            break;
        };
        let size_line = std::str::from_utf8(&raw[pos..pos + rel_crlf]).unwrap_or("0");
        let size_hex = size_line.split(';').next().unwrap_or("0").trim();
        let chunk_len = usize::from_str_radix(size_hex, 16).unwrap_or(0);
        pos += rel_crlf + 2;
        if chunk_len == 0 {
            break;
        }
        let end = (pos + chunk_len).min(raw.len());
        out.extend_from_slice(&raw[pos..end]);
        pos = (end + 2).min(raw.len());
    }
    out
}

async fn http_raw(
    addr: SocketAddr,
    method: &str,
    path: &str,
    cookie: &str,
    body: Option<&[u8]>,
) -> (u16, String, Vec<u8>) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let body_bytes = body.unwrap_or(&[]);
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nCookie: lv_session={cookie}\r\n"
    );
    if body.is_some() {
        req.push_str("Content-Type: application/json\r\n");
    }
    req.push_str(&format!("Content-Length: {}\r\n\r\n", body_bytes.len()));
    stream.write_all(req.as_bytes()).await.unwrap();
    if !body_bytes.is_empty() {
        stream.write_all(body_bytes).await.unwrap();
    }
    stream.flush().await.unwrap();

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let header_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("valid HTTP response");
    let headers_str = String::from_utf8_lossy(&raw[..header_end]).into_owned();
    let status: u16 = headers_str
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("HTTP status code");
    let body_slice = &raw[header_end + 4..];
    let is_chunked = headers_str
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked");
    let decoded_body = if is_chunked {
        decode_chunked_body(body_slice)
    } else {
        body_slice.to_vec()
    };
    (status, headers_str, decoded_body)
}

async fn http_json(
    addr: SocketAddr,
    method: &str,
    path: &str,
    cookie: &str,
    payload: Option<Value>,
) -> (u16, Value) {
    let encoded = payload.map(|v| serde_json::to_vec(&v).unwrap());
    let (status, _, body) = http_raw(addr, method, path, cookie, encoded.as_deref()).await;
    let val = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or_else(|e| {
            panic!(
                "expected JSON body for {method} {path} (status {status}), got `{}`: {e}",
                String::from_utf8_lossy(&body)
            )
        })
    };
    (status, val)
}

fn register_trusted_peer(db: &Database, device_id: &str, name: &str) {
    db.upsert_trusted_peer(&TrustedPeer {
        device_id: device_id.to_string(),
        device_name: name.to_string(),
        os_type: "linux".to_string(),
        ed25519_pubkey: vec![1u8; 32],
        last_ip: Some("127.0.0.1".to_string()),
        last_port: Some(7842),
        auto_connect: true,
        paired_at: unix_timestamp_secs(),
        revoked_at: None,
    })
    .unwrap();
}

fn put_test_chunk(store: &VaultStore, vault_id: &str, payload: &[u8]) -> String {
    let hash = blake3_hash(payload);
    store
        .put_chunk(vault_id, &hash, payload.len() as u64, payload)
        .unwrap();
    hex::encode(hash)
}

#[tokio::test]
async fn three_device_group_with_two_targets_offline_catchup() {
    let srv = start_test_server("group-catchup").await;

    // 1. Capabilities endpoint
    let (status, caps_val) = http_json(
        srv.addr,
        "GET",
        "/api/capabilities",
        &srv.admin_cookie,
        None,
    )
    .await;
    assert_eq!(status, 200);
    let caps: ProtocolCapabilities = serde_json::from_value(caps_val).unwrap();
    assert_eq!(caps, ProtocolCapabilities::current());

    // Create second vault for target B
    let (vault_b, _) = srv
        .state
        .vault_store
        .create_vault(
            "Target B Vault".to_string(),
            srv.root.join("vault-b").to_string_lossy().into_owned(),
            None,
            PoolMode::Single,
            128 * 1024 * 1024,
            true,
            Vec::new(),
        )
        .unwrap();

    register_trusted_peer(&srv.state.db, "dev-src", "Source Laptop");
    register_trusted_peer(&srv.state.db, "dev-tgt-a", "NAS A");
    register_trusted_peer(&srv.state.db, "dev-tgt-b", "NAS B");

    let src_cookie = srv.state.auth.issue_peer_session("dev-src".to_string());

    // 2. Create 3-device group
    let (status, group_val) = http_json(
        srv.addr,
        "POST",
        "/api/groups",
        &srv.admin_cookie,
        Some(json!({
            "name": "Home 3-Device Group",
            "coordinator_device_id": "dev-src",
            "member_device_ids": ["dev-tgt-a", "dev-tgt-b"]
        })),
    )
    .await;
    assert_eq!(status, 201);
    let group_id = group_val["group_id"].as_str().unwrap().to_string();
    let group_rev = group_val["revision"].as_i64().unwrap();

    // 3. Create diff_backup rule + 2 target bindings
    let (status, rule_val) = http_json(
        srv.addr,
        "POST",
        &format!("/api/groups/{group_id}/rules"),
        &srv.admin_cookie,
        Some(json!({
            "name": "Docs Backup",
            "mode": "diff_backup",
            "source_device_id": "dev-src",
            "source_root": "/home/user/docs"
        })),
    )
    .await;
    assert_eq!(status, 201);
    let rule_id = rule_val["rule_id"].as_str().unwrap().to_string();
    let rule_rev = rule_val["revision"].as_i64().unwrap();

    let (status, b_a) = http_json(
        srv.addr,
        "POST",
        &format!("/api/rules/{rule_id}/bindings"),
        &srv.admin_cookie,
        Some(json!({
            "target_device_id": "dev-tgt-a",
            "target_vault_id": srv.default_vault_id
        })),
    )
    .await;
    assert_eq!(status, 201);
    let binding_a = b_a["binding_id"].as_str().unwrap().to_string();

    let (status, b_b) = http_json(
        srv.addr,
        "POST",
        &format!("/api/rules/{rule_id}/bindings"),
        &srv.admin_cookie,
        Some(json!({
            "target_device_id": "dev-tgt-b",
            "target_vault_id": vault_b.vault_id
        })),
    )
    .await;
    assert_eq!(status, 201);
    let binding_b = b_b["binding_id"].as_str().unwrap().to_string();

    // 4. Target A syncs while Target B is offline
    let payload = b"hello multi-target group sync";
    let chunk_hex = put_test_chunk(&srv.state.vault_store, &srv.default_vault_id, payload);
    let missing_hex = hex::encode(blake3_hash(b"not-uploaded-yet"));

    let (status, check_val) = http_json(
        srv.addr,
        "POST",
        &format!("/api/vaults/{}/chunks/check", srv.default_vault_id),
        &src_cookie,
        Some(
            serde_json::to_value(ChunkBatchCheckRequest {
                hashes_hex: vec![chunk_hex.clone(), missing_hex.clone()],
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, 200, "chunk check response: {check_val}");
    let check_resp: ChunkBatchCheckResponse = serde_json::from_value(check_val).unwrap();
    assert_eq!(check_resp.present, vec![chunk_hex.clone()]);
    assert_eq!(check_resp.missing, vec![missing_hex]);

    let (status, lease_a) = http_json(
        srv.addr,
        "POST",
        &format!("/api/bindings/{binding_a}/leases"),
        &src_cookie,
        Some(json!({
            "rule_id": rule_id,
            "group_revision": group_rev,
            "rule_revision": rule_rev,
            "ttl_secs": 60
        })),
    )
    .await;
    assert_eq!(status, 200);
    let fence_a = lease_a["fence_token"].as_i64().unwrap();

    let commit_a = CatalogCommitRequest {
        operation_id: "op-a-1".to_string(),
        rule_id: rule_id.clone(),
        group_revision: group_rev,
        rule_revision: rule_rev,
        fence_token: fence_a,
        base_revision: 0,
        source_gen: 1,
        ops: vec![CatalogDeltaOp {
            path_key: "docs/readme.md".to_string(),
            display_path: "docs/readme.md".to_string(),
            entry_kind: "file".to_string(),
            kind: CatalogDeltaKind::UpsertFile,
            size: payload.len() as u64,
            mtime_ns: 1_000,
            content_hash_hex: Some(chunk_hex.clone()),
            chunk_hashes_hex: vec![chunk_hex.clone()],
            archive_reason: None,
            expires_at: None,
        }],
    };
    let (status, resp_a_val) = http_json(
        srv.addr,
        "POST",
        &format!("/api/bindings/{binding_a}/catalog/commit"),
        &src_cookie,
        Some(serde_json::to_value(&commit_a).unwrap()),
    )
    .await;
    assert_eq!(status, 200);
    let resp_a: CatalogCommitResponse = serde_json::from_value(resp_a_val).unwrap();
    assert_eq!(resp_a.new_revision, 1);
    assert_eq!(resp_a.applied_ops, 1);

    // Target B still at revision 0 while offline
    let (status, cat_b_before) = http_json(
        srv.addr,
        "GET",
        &format!("/api/bindings/{binding_b}/catalog?limit=50"),
        &src_cookie,
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(cat_b_before["entries"].as_array().unwrap().len(), 0);

    // 5. Target B comes online later and catches up independently
    let chunk_hex_b = put_test_chunk(&srv.state.vault_store, &vault_b.vault_id, payload);
    assert_eq!(chunk_hex_b, chunk_hex);

    let (status, lease_b) = http_json(
        srv.addr,
        "POST",
        &format!("/api/bindings/{binding_b}/leases"),
        &src_cookie,
        Some(json!({
            "rule_id": rule_id,
            "group_revision": group_rev,
            "rule_revision": rule_rev,
            "ttl_secs": 60
        })),
    )
    .await;
    assert_eq!(status, 200);
    let fence_b = lease_b["fence_token"].as_i64().unwrap();

    let commit_b = CatalogCommitRequest {
        operation_id: "op-b-1".to_string(),
        rule_id: rule_id.clone(),
        group_revision: group_rev,
        rule_revision: rule_rev,
        fence_token: fence_b,
        base_revision: 0,
        source_gen: 1,
        ops: commit_a.ops.clone(),
    };
    let (status, resp_b_val) = http_json(
        srv.addr,
        "POST",
        &format!("/api/bindings/{binding_b}/catalog/commit"),
        &src_cookie,
        Some(serde_json::to_value(&commit_b).unwrap()),
    )
    .await;
    assert_eq!(status, 200);
    let resp_b: CatalogCommitResponse = serde_json::from_value(resp_b_val).unwrap();
    assert_eq!(resp_b.new_revision, 1);

    // Verify lost-response recovery endpoint returns committed operation
    let (status, op_rec) = http_json(
        srv.addr,
        "GET",
        &format!("/api/bindings/{binding_b}/catalog/operations/op-b-1"),
        &src_cookie,
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(op_rec["status"].as_str(), Some("committed"));
    assert_eq!(op_rec["response"]["new_revision"].as_i64(), Some(1));

    // Verify both bindings now hold docs/readme.md
    let (status, cat_b_after) = http_json(
        srv.addr,
        "GET",
        &format!("/api/bindings/{binding_b}/catalog?limit=50"),
        &src_cookie,
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(cat_b_after["entries"].as_array().unwrap().len(), 1);
    assert_eq!(
        cat_b_after["entries"][0]["path_key"].as_str(),
        Some("docs/readme.md")
    );
}

#[tokio::test]
async fn peer_revocation_fences_stale_leases_and_topology_conflicts_rejected() {
    let srv = start_test_server("revocation-fence").await;
    register_trusted_peer(&srv.state.db, "dev-alpha", "Alpha");
    register_trusted_peer(&srv.state.db, "dev-beta", "Beta");
    let alpha_cookie = srv.state.auth.issue_peer_session("dev-alpha".to_string());

    let (status, group_val) = http_json(
        srv.addr,
        "POST",
        "/api/groups",
        &srv.admin_cookie,
        Some(json!({
            "name": "Revocation Group",
            "coordinator_device_id": "dev-alpha",
            "member_device_ids": ["dev-beta"]
        })),
    )
    .await;
    assert_eq!(status, 201);
    let group_id = group_val["group_id"].as_str().unwrap().to_string();
    let group_rev = group_val["revision"].as_i64().unwrap();

    // Register mirror root for dev-beta and test unregistered/revoked rejection
    let (status, m_rule) = http_json(
        srv.addr,
        "POST",
        &format!("/api/groups/{group_id}/rules"),
        &srv.admin_cookie,
        Some(json!({
            "name": "Mirror A->B",
            "mode": "mirror_sync",
            "source_device_id": "dev-alpha",
            "source_root": "/sync/a"
        })),
    )
    .await;
    assert_eq!(status, 201);
    let mirror_rule_id = m_rule["rule_id"].as_str().unwrap().to_string();
    let mirror_rule_rev = m_rule["revision"].as_i64().unwrap();

    // Unregistered mirror_root_id must be rejected (400)
    let (status, _) = http_json(
        srv.addr,
        "POST",
        &format!("/api/rules/{mirror_rule_id}/bindings"),
        &srv.admin_cookie,
        Some(json!({
            "target_device_id": "dev-beta",
            "target_vault_id": srv.default_vault_id,
            "mirror_root_id": "nonexistent-root"
        })),
    )
    .await;
    assert_eq!(status, 400);

    // Register valid mirror roots on dev-beta and dev-alpha
    let (status, root_b) = http_json(
        srv.addr,
        "POST",
        "/api/mirror-roots/register",
        &srv.admin_cookie,
        Some(json!({
            "device_id": "dev-beta",
            "canonical_root": "/sync/b",
            "label": "Beta Mirror"
        })),
    )
    .await;
    assert_eq!(status, 201);
    let root_b_id = root_b["mirror_root_id"].as_str().unwrap().to_string();

    let (status, root_a) = http_json(
        srv.addr,
        "POST",
        "/api/mirror-roots/register",
        &srv.admin_cookie,
        Some(json!({
            "device_id": "dev-alpha",
            "canonical_root": "/sync/a",
            "label": "Alpha Mirror"
        })),
    )
    .await;
    assert_eq!(status, 201);
    let root_a_id = root_a["mirror_root_id"].as_str().unwrap().to_string();

    // Bind A -> B
    let (status, binding_ab) = http_json(
        srv.addr,
        "POST",
        &format!("/api/rules/{mirror_rule_id}/bindings"),
        &srv.admin_cookie,
        Some(json!({
            "target_device_id": "dev-beta",
            "target_vault_id": srv.default_vault_id,
            "mirror_root_id": root_b_id
        })),
    )
    .await;
    assert_eq!(status, 201);
    let binding_ab_id = binding_ab["binding_id"].as_str().unwrap().to_string();

    // Creating a reverse mirror rule B -> A and binding it to Alpha's /sync/a must trigger 409 topology conflict
    let (status, rev_rule) = http_json(
        srv.addr,
        "POST",
        &format!("/api/groups/{group_id}/rules"),
        &srv.admin_cookie,
        Some(json!({
            "name": "Mirror B->A",
            "mode": "mirror_sync",
            "source_device_id": "dev-beta",
            "source_root": "/sync/b"
        })),
    )
    .await;
    assert_eq!(status, 201);
    let rev_rule_id = rev_rule["rule_id"].as_str().unwrap().to_string();

    let (status, conflict_val) = http_json(
        srv.addr,
        "POST",
        &format!("/api/rules/{rev_rule_id}/bindings"),
        &srv.admin_cookie,
        Some(json!({
            "target_device_id": "dev-alpha",
            "target_vault_id": srv.default_vault_id,
            "mirror_root_id": root_a_id
        })),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(
        conflict_val["error"].as_str(),
        Some("rule_topology_conflict")
    );

    // Alpha acquires lease on binding A -> B
    let (status, lease_val) = http_json(
        srv.addr,
        "POST",
        &format!("/api/bindings/{binding_ab_id}/leases"),
        &alpha_cookie,
        Some(json!({
            "rule_id": mirror_rule_id,
            "group_revision": group_rev,
            "rule_revision": mirror_rule_rev,
            "ttl_secs": 120
        })),
    )
    .await;
    assert_eq!(status, 200);
    let stale_fence = lease_val["fence_token"].as_i64().unwrap();

    // Admin revokes dev-alpha via DELETE /api/peers/dev-alpha
    let (status, _) = http_json(
        srv.addr,
        "DELETE",
        "/api/peers/dev-alpha",
        &srv.admin_cookie,
        None,
    )
    .await;
    assert_eq!(status, 200);

    // Group membership & lease must immediately be invalidated
    assert!(!srv
        .state
        .db
        .is_active_group_member(&group_id, "dev-alpha")
        .unwrap());
    assert!(!srv
        .state
        .db
        .verify_sync_lease(
            &mirror_rule_id,
            &binding_ab_id,
            stale_fence,
            group_rev,
            mirror_rule_rev
        )
        .unwrap());

    // Stale commit using revoked peer cookie fails with 401
    let chunk_hex = put_test_chunk(
        &srv.state.vault_store,
        &srv.default_vault_id,
        b"stale-write",
    );
    let stale_commit = CatalogCommitRequest {
        operation_id: "op-stale-1".to_string(),
        rule_id: mirror_rule_id.clone(),
        group_revision: group_rev,
        rule_revision: mirror_rule_rev,
        fence_token: stale_fence,
        base_revision: 0,
        source_gen: 1,
        ops: vec![CatalogDeltaOp {
            path_key: "secret.txt".to_string(),
            display_path: "secret.txt".to_string(),
            entry_kind: "file".to_string(),
            kind: CatalogDeltaKind::UpsertFile,
            size: 11,
            mtime_ns: 100,
            content_hash_hex: Some(chunk_hex.clone()),
            chunk_hashes_hex: vec![chunk_hex],
            archive_reason: None,
            expires_at: None,
        }],
    };
    let (status, _) = http_json(
        srv.addr,
        "POST",
        &format!("/api/bindings/{binding_ab_id}/catalog/commit"),
        &alpha_cookie,
        Some(serde_json::to_value(&stale_commit).unwrap()),
    )
    .await;
    assert_eq!(status, 401);

    // Even with admin cookie, stale group_revision / invalidated lease is fenced out (409)
    let (status, _) = http_json(
        srv.addr,
        "POST",
        &format!("/api/bindings/{binding_ab_id}/catalog/commit"),
        &srv.admin_cookie,
        Some(serde_json::to_value(&stale_commit).unwrap()),
    )
    .await;
    assert_eq!(status, 409);
}

#[tokio::test]
async fn diff_backup_five_states_version_download_and_idempotent_retry() {
    let srv = start_test_server("diff-5-states").await;
    register_trusted_peer(&srv.state.db, "dev-src", "Source");
    register_trusted_peer(&srv.state.db, "dev-nas", "NAS");

    let (status, group_val) = http_json(
        srv.addr,
        "POST",
        "/api/groups",
        &srv.admin_cookie,
        Some(json!({
            "name": "Diff Group",
            "coordinator_device_id": "dev-src",
            "member_device_ids": ["dev-nas"]
        })),
    )
    .await;
    assert_eq!(status, 201);
    let group_id = group_val["group_id"].as_str().unwrap().to_string();
    let group_rev = group_val["revision"].as_i64().unwrap();

    let (status, rule_val) = http_json(
        srv.addr,
        "POST",
        &format!("/api/groups/{group_id}/rules"),
        &srv.admin_cookie,
        Some(json!({
            "name": "Photos Diff",
            "mode": "diff_backup",
            "source_device_id": "dev-src",
            "source_root": "/photos"
        })),
    )
    .await;
    assert_eq!(status, 201);
    let rule_id = rule_val["rule_id"].as_str().unwrap().to_string();
    let rule_rev = rule_val["revision"].as_i64().unwrap();

    let (status, bind_val) = http_json(
        srv.addr,
        "POST",
        &format!("/api/rules/{rule_id}/bindings"),
        &srv.admin_cookie,
        Some(json!({
            "target_device_id": "dev-nas",
            "target_vault_id": srv.default_vault_id
        })),
    )
    .await;
    assert_eq!(status, 201);
    let binding_id = bind_val["binding_id"].as_str().unwrap().to_string();

    let (status, lease_val) = http_json(
        srv.addr,
        "POST",
        &format!("/api/bindings/{binding_id}/leases"),
        &srv.admin_cookie,
        Some(json!({
            "rule_id": rule_id,
            "holder_device_id": "dev-src",
            "group_revision": group_rev,
            "rule_revision": rule_rev,
            "ttl_secs": 120
        })),
    )
    .await;
    assert_eq!(status, 200);
    let fence = lease_val["fence_token"].as_i64().unwrap();

    let v1_bytes = b"version-one-content";
    let v2_bytes = b"version-two-updated-content";
    let v3_bytes = b"version-three-reappeared-diff";
    let h1 = put_test_chunk(&srv.state.vault_store, &srv.default_vault_id, v1_bytes);
    let h2 = put_test_chunk(&srv.state.vault_store, &srv.default_vault_id, v2_bytes);
    let h3 = put_test_chunk(&srv.state.vault_store, &srv.default_vault_id, v3_bytes);

    let commit_url = format!("/api/bindings/{binding_id}/catalog/commit");

    // State 1: Add
    let (status, r1) = http_json(
        srv.addr,
        "POST",
        &commit_url,
        &srv.admin_cookie,
        Some(
            serde_json::to_value(CatalogCommitRequest {
                operation_id: "diff-op-1".to_string(),
                rule_id: rule_id.clone(),
                group_revision: group_rev,
                rule_revision: rule_rev,
                fence_token: fence,
                base_revision: 0,
                source_gen: 1,
                ops: vec![CatalogDeltaOp {
                    path_key: "notes/a.txt".to_string(),
                    display_path: "notes/a.txt".to_string(),
                    entry_kind: "file".to_string(),
                    kind: CatalogDeltaKind::UpsertFile,
                    size: v1_bytes.len() as u64,
                    mtime_ns: 100,
                    content_hash_hex: Some(h1.clone()),
                    chunk_hashes_hex: vec![h1.clone()],
                    archive_reason: None,
                    expires_at: None,
                }],
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(r1["new_revision"].as_i64(), Some(1));
    assert_eq!(r1["archived_versions"].as_u64(), Some(0));

    // State 2: Same content (mtime change only -> no archived version churn)
    let (status, r2) = http_json(
        srv.addr,
        "POST",
        &commit_url,
        &srv.admin_cookie,
        Some(
            serde_json::to_value(CatalogCommitRequest {
                operation_id: "diff-op-2".to_string(),
                rule_id: rule_id.clone(),
                group_revision: group_rev,
                rule_revision: rule_rev,
                fence_token: fence,
                base_revision: 1,
                source_gen: 2,
                ops: vec![CatalogDeltaOp {
                    path_key: "notes/a.txt".to_string(),
                    display_path: "notes/a.txt".to_string(),
                    entry_kind: "file".to_string(),
                    kind: CatalogDeltaKind::UpsertFile,
                    size: v1_bytes.len() as u64,
                    mtime_ns: 200,
                    content_hash_hex: Some(h1.clone()),
                    chunk_hashes_hex: vec![h1.clone()],
                    archive_reason: None,
                    expires_at: None,
                }],
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(r2["new_revision"].as_i64(), Some(2));
    assert_eq!(r2["archived_versions"].as_u64(), Some(0));

    // State 3: Diff-content (archives v1, promotes v2)
    let (status, r3) = http_json(
        srv.addr,
        "POST",
        &commit_url,
        &srv.admin_cookie,
        Some(
            serde_json::to_value(CatalogCommitRequest {
                operation_id: "diff-op-3".to_string(),
                rule_id: rule_id.clone(),
                group_revision: group_rev,
                rule_revision: rule_rev,
                fence_token: fence,
                base_revision: 2,
                source_gen: 3,
                ops: vec![CatalogDeltaOp {
                    path_key: "notes/a.txt".to_string(),
                    display_path: "notes/a.txt".to_string(),
                    entry_kind: "file".to_string(),
                    kind: CatalogDeltaKind::UpsertFile,
                    size: v2_bytes.len() as u64,
                    mtime_ns: 300,
                    content_hash_hex: Some(h2.clone()),
                    chunk_hashes_hex: vec![h2.clone()],
                    archive_reason: Some("diff_replaced".to_string()),
                    expires_at: None,
                }],
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(r3["new_revision"].as_i64(), Some(3));
    assert_eq!(r3["archived_versions"].as_u64(), Some(1));

    // Verify archived v1 can be listed and downloaded with u32be-aead-v1 framing
    let (status, ver_list) = http_json(
        srv.addr,
        "GET",
        &format!("/api/bindings/{binding_id}/versions?path_key=notes/a.txt&limit=10"),
        &srv.admin_cookie,
        None,
    )
    .await;
    assert_eq!(status, 200);
    let versions = ver_list["versions"].as_array().unwrap();
    assert_eq!(versions.len(), 1);
    let v1_version_id = versions[0]["version_id"].as_str().unwrap().to_string();

    let (dl_status, dl_headers, dl_body) = http_raw(
        srv.addr,
        "GET",
        &format!("/api/bindings/{binding_id}/versions/{v1_version_id}/download"),
        &srv.admin_cookie,
        None,
    )
    .await;
    assert_eq!(dl_status, 200);
    assert!(dl_headers
        .to_ascii_lowercase()
        .contains("x-lanvault-chunk-framing: u32be-aead-v1"));
    assert_eq!(dl_body.len(), 4 + v1_bytes.len());
    let framed_len = u32::from_be_bytes(dl_body[..4].try_into().unwrap()) as usize;
    assert_eq!(framed_len, v1_bytes.len());
    assert_eq!(&dl_body[4..], v1_bytes);

    // State 4: Source-deleted
    let (status, r4) = http_json(
        srv.addr,
        "POST",
        &commit_url,
        &srv.admin_cookie,
        Some(
            serde_json::to_value(CatalogCommitRequest {
                operation_id: "diff-op-4".to_string(),
                rule_id: rule_id.clone(),
                group_revision: group_rev,
                rule_revision: rule_rev,
                fence_token: fence,
                base_revision: 3,
                source_gen: 4,
                ops: vec![CatalogDeltaOp {
                    path_key: "notes/a.txt".to_string(),
                    display_path: "notes/a.txt".to_string(),
                    entry_kind: "file".to_string(),
                    kind: CatalogDeltaKind::MarkSourceDeleted,
                    size: 0,
                    mtime_ns: 0,
                    content_hash_hex: None,
                    chunk_hashes_hex: vec![],
                    archive_reason: None,
                    expires_at: None,
                }],
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(r4["marked_deleted"].as_u64(), Some(1));

    let (status, del_page) = http_json(
        srv.addr,
        "GET",
        &format!("/api/bindings/{binding_id}/catalog?source_deleted_only=true&limit=10"),
        &srv.admin_cookie,
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(del_page["entries"].as_array().unwrap().len(), 1);
    assert_eq!(
        del_page["entries"][0]["source_deleted"].as_bool(),
        Some(true)
    );

    // State 5a: Reappear with same content (clears source_deleted without archiving)
    let (status, r5a) = http_json(
        srv.addr,
        "POST",
        &commit_url,
        &srv.admin_cookie,
        Some(
            serde_json::to_value(CatalogCommitRequest {
                operation_id: "diff-op-5a".to_string(),
                rule_id: rule_id.clone(),
                group_revision: group_rev,
                rule_revision: rule_rev,
                fence_token: fence,
                base_revision: 4,
                source_gen: 5,
                ops: vec![CatalogDeltaOp {
                    path_key: "notes/a.txt".to_string(),
                    display_path: "notes/a.txt".to_string(),
                    entry_kind: "file".to_string(),
                    kind: CatalogDeltaKind::UpsertFile,
                    size: v2_bytes.len() as u64,
                    mtime_ns: 500,
                    content_hash_hex: Some(h2.clone()),
                    chunk_hashes_hex: vec![h2.clone()],
                    archive_reason: None,
                    expires_at: None,
                }],
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(r5a["cleared_deleted"].as_u64(), Some(1));
    assert_eq!(r5a["archived_versions"].as_u64(), Some(0));

    // Mark deleted again, then State 5b: Reappear with diff content (archives v2 + clears deleted)
    let (status, _) = http_json(
        srv.addr,
        "POST",
        &commit_url,
        &srv.admin_cookie,
        Some(
            serde_json::to_value(CatalogCommitRequest {
                operation_id: "diff-op-6".to_string(),
                rule_id: rule_id.clone(),
                group_revision: group_rev,
                rule_revision: rule_rev,
                fence_token: fence,
                base_revision: 5,
                source_gen: 6,
                ops: vec![CatalogDeltaOp {
                    path_key: "notes/a.txt".to_string(),
                    display_path: "notes/a.txt".to_string(),
                    entry_kind: "file".to_string(),
                    kind: CatalogDeltaKind::MarkSourceDeleted,
                    size: 0,
                    mtime_ns: 0,
                    content_hash_hex: None,
                    chunk_hashes_hex: vec![],
                    archive_reason: None,
                    expires_at: None,
                }],
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, 200);

    let reappear_diff_req = CatalogCommitRequest {
        operation_id: "diff-op-7".to_string(),
        rule_id: rule_id.clone(),
        group_revision: group_rev,
        rule_revision: rule_rev,
        fence_token: fence,
        base_revision: 6,
        source_gen: 7,
        ops: vec![CatalogDeltaOp {
            path_key: "notes/a.txt".to_string(),
            display_path: "notes/a.txt".to_string(),
            entry_kind: "file".to_string(),
            kind: CatalogDeltaKind::UpsertFile,
            size: v3_bytes.len() as u64,
            mtime_ns: 700,
            content_hash_hex: Some(h3.clone()),
            chunk_hashes_hex: vec![h3],
            archive_reason: Some("diff_reappeared".to_string()),
            expires_at: None,
        }],
    };
    let (status, r5b) = http_json(
        srv.addr,
        "POST",
        &commit_url,
        &srv.admin_cookie,
        Some(serde_json::to_value(&reappear_diff_req).unwrap()),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(r5b["new_revision"].as_i64(), Some(7));
    assert_eq!(r5b["cleared_deleted"].as_u64(), Some(1));
    assert_eq!(r5b["archived_versions"].as_u64(), Some(1));
    assert_eq!(r5b["deduplicated_replay"].as_bool(), Some(false));

    // Idempotent retry of op-7 returns exact same revision with deduplicated_replay = true
    let (status, r5b_retry) = http_json(
        srv.addr,
        "POST",
        &commit_url,
        &srv.admin_cookie,
        Some(serde_json::to_value(&reappear_diff_req).unwrap()),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(r5b_retry["new_revision"].as_i64(), Some(7));
    assert_eq!(r5b_retry["deduplicated_replay"].as_bool(), Some(true));
}

#[tokio::test]
async fn mirror_archive_nday_gc_protects_diff_backup_and_source_deleted_chunks() {
    let srv = start_test_server("gc-mirror-archive").await;
    register_trusted_peer(&srv.state.db, "dev-src", "Source");
    register_trusted_peer(&srv.state.db, "dev-tgt", "Target");

    let (status, group_val) = http_json(
        srv.addr,
        "POST",
        "/api/groups",
        &srv.admin_cookie,
        Some(json!({
            "name": "GC Protection Group",
            "coordinator_device_id": "dev-src",
            "member_device_ids": ["dev-tgt"]
        })),
    )
    .await;
    assert_eq!(status, 201);
    let group_id = group_val["group_id"].as_str().unwrap().to_string();
    let group_rev = group_val["revision"].as_i64().unwrap();

    // Rule 1: MirrorSync (7-day archive retention)
    let (status, root_val) = http_json(
        srv.addr,
        "POST",
        "/api/mirror-roots/register",
        &srv.admin_cookie,
        Some(json!({
            "device_id": "dev-tgt",
            "canonical_root": "/mirror/tgt",
            "label": "Mirror Root"
        })),
    )
    .await;
    assert_eq!(status, 201);
    let root_id = root_val["mirror_root_id"].as_str().unwrap().to_string();

    let (status, mirror_rule) = http_json(
        srv.addr,
        "POST",
        &format!("/api/groups/{group_id}/rules"),
        &srv.admin_cookie,
        Some(json!({
            "name": "Mirror 7d",
            "mode": "mirror_sync",
            "source_device_id": "dev-src",
            "source_root": "/src/mirror",
            "archive_retention_days": 7
        })),
    )
    .await;
    assert_eq!(status, 201);
    let mirror_rule_id = mirror_rule["rule_id"].as_str().unwrap().to_string();
    let mirror_rule_rev = mirror_rule["revision"].as_i64().unwrap();

    let (status, mirror_binding) = http_json(
        srv.addr,
        "POST",
        &format!("/api/rules/{mirror_rule_id}/bindings"),
        &srv.admin_cookie,
        Some(json!({
            "target_device_id": "dev-tgt",
            "target_vault_id": srv.default_vault_id,
            "mirror_root_id": root_id
        })),
    )
    .await;
    assert_eq!(status, 201);
    let mirror_bid = mirror_binding["binding_id"].as_str().unwrap().to_string();

    // Rule 2: DiffBackup (permanent versions & source_deleted entries)
    let (status, diff_rule) = http_json(
        srv.addr,
        "POST",
        &format!("/api/groups/{group_id}/rules"),
        &srv.admin_cookie,
        Some(json!({
            "name": "Diff Permanent",
            "mode": "diff_backup",
            "source_device_id": "dev-src",
            "source_root": "/src/diff"
        })),
    )
    .await;
    assert_eq!(status, 201);
    let diff_rule_id = diff_rule["rule_id"].as_str().unwrap().to_string();
    let diff_rule_rev = diff_rule["revision"].as_i64().unwrap();

    let (status, diff_binding) = http_json(
        srv.addr,
        "POST",
        &format!("/api/rules/{diff_rule_id}/bindings"),
        &srv.admin_cookie,
        Some(json!({
            "target_device_id": "dev-tgt",
            "target_vault_id": srv.default_vault_id
        })),
    )
    .await;
    assert_eq!(status, 201);
    let diff_bid = diff_binding["binding_id"].as_str().unwrap().to_string();

    let now = unix_timestamp_secs();
    let chunk_mirror_bytes = b"mirror-archived-chunk-to-expire";
    let chunk_diff_old_bytes = b"diff-backup-archived-chunk-forever";
    let chunk_diff_cur_bytes = b"diff-backup-source-deleted-chunk-forever";

    let h_mirror = put_test_chunk(
        &srv.state.vault_store,
        &srv.default_vault_id,
        chunk_mirror_bytes,
    );
    let h_diff_old = put_test_chunk(
        &srv.state.vault_store,
        &srv.default_vault_id,
        chunk_diff_old_bytes,
    );
    let h_diff_cur = put_test_chunk(
        &srv.state.vault_store,
        &srv.default_vault_id,
        chunk_diff_cur_bytes,
    );

    // Mirror: add file, then ArchiveAndRemove with expires_at = now + 7 days
    let lease_m = srv
        .state
        .db
        .acquire_or_renew_sync_lease(
            &mirror_rule_id,
            &mirror_bid,
            "dev-src",
            group_rev,
            mirror_rule_rev,
            60,
        )
        .unwrap();
    srv.state
        .db
        .commit_catalog_delta(
            &mirror_bid,
            &srv.default_vault_id,
            &CatalogCommitRequest {
                operation_id: "m-op-1".to_string(),
                rule_id: mirror_rule_id.clone(),
                group_revision: group_rev,
                rule_revision: mirror_rule_rev,
                fence_token: lease_m.fence_token,
                base_revision: 0,
                source_gen: 1,
                ops: vec![CatalogDeltaOp {
                    path_key: "temp.bin".to_string(),
                    display_path: "temp.bin".to_string(),
                    entry_kind: "file".to_string(),
                    kind: CatalogDeltaKind::UpsertFile,
                    size: chunk_mirror_bytes.len() as u64,
                    mtime_ns: 1,
                    content_hash_hex: Some(h_mirror.clone()),
                    chunk_hashes_hex: vec![h_mirror.clone()],
                    archive_reason: None,
                    expires_at: None,
                }],
            },
        )
        .unwrap();
    srv.state
        .db
        .commit_catalog_delta(
            &mirror_bid,
            &srv.default_vault_id,
            &CatalogCommitRequest {
                operation_id: "m-op-2".to_string(),
                rule_id: mirror_rule_id.clone(),
                group_revision: group_rev,
                rule_revision: mirror_rule_rev,
                fence_token: lease_m.fence_token,
                base_revision: 1,
                source_gen: 2,
                ops: vec![CatalogDeltaOp {
                    path_key: "temp.bin".to_string(),
                    display_path: "temp.bin".to_string(),
                    entry_kind: "file".to_string(),
                    kind: CatalogDeltaKind::ArchiveAndRemove,
                    size: 0,
                    mtime_ns: 0,
                    content_hash_hex: None,
                    chunk_hashes_hex: vec![],
                    archive_reason: Some("mirror_removed".to_string()),
                    expires_at: Some(now + 7 * 86_400),
                }],
            },
        )
        .unwrap();

    // DiffBackup: add v1, replace with v2 (v1 archived with expires_at=None), then MarkSourceDeleted on v2
    let lease_d = srv
        .state
        .db
        .acquire_or_renew_sync_lease(
            &diff_rule_id,
            &diff_bid,
            "dev-src",
            group_rev,
            diff_rule_rev,
            60,
        )
        .unwrap();
    srv.state
        .db
        .commit_catalog_delta(
            &diff_bid,
            &srv.default_vault_id,
            &CatalogCommitRequest {
                operation_id: "d-op-1".to_string(),
                rule_id: diff_rule_id.clone(),
                group_revision: group_rev,
                rule_revision: diff_rule_rev,
                fence_token: lease_d.fence_token,
                base_revision: 0,
                source_gen: 1,
                ops: vec![CatalogDeltaOp {
                    path_key: "keep.bin".to_string(),
                    display_path: "keep.bin".to_string(),
                    entry_kind: "file".to_string(),
                    kind: CatalogDeltaKind::UpsertFile,
                    size: chunk_diff_old_bytes.len() as u64,
                    mtime_ns: 1,
                    content_hash_hex: Some(h_diff_old.clone()),
                    chunk_hashes_hex: vec![h_diff_old.clone()],
                    archive_reason: None,
                    expires_at: None,
                }],
            },
        )
        .unwrap();
    srv.state
        .db
        .commit_catalog_delta(
            &diff_bid,
            &srv.default_vault_id,
            &CatalogCommitRequest {
                operation_id: "d-op-2".to_string(),
                rule_id: diff_rule_id.clone(),
                group_revision: group_rev,
                rule_revision: diff_rule_rev,
                fence_token: lease_d.fence_token,
                base_revision: 1,
                source_gen: 2,
                ops: vec![CatalogDeltaOp {
                    path_key: "keep.bin".to_string(),
                    display_path: "keep.bin".to_string(),
                    entry_kind: "file".to_string(),
                    kind: CatalogDeltaKind::UpsertFile,
                    size: chunk_diff_cur_bytes.len() as u64,
                    mtime_ns: 2,
                    content_hash_hex: Some(h_diff_cur.clone()),
                    chunk_hashes_hex: vec![h_diff_cur.clone()],
                    archive_reason: Some("diff_replaced".to_string()),
                    expires_at: None,
                }],
            },
        )
        .unwrap();
    srv.state
        .db
        .commit_catalog_delta(
            &diff_bid,
            &srv.default_vault_id,
            &CatalogCommitRequest {
                operation_id: "d-op-3".to_string(),
                rule_id: diff_rule_id.clone(),
                group_revision: group_rev,
                rule_revision: diff_rule_rev,
                fence_token: lease_d.fence_token,
                base_revision: 2,
                source_gen: 3,
                ops: vec![CatalogDeltaOp {
                    path_key: "keep.bin".to_string(),
                    display_path: "keep.bin".to_string(),
                    entry_kind: "file".to_string(),
                    kind: CatalogDeltaKind::MarkSourceDeleted,
                    size: 0,
                    mtime_ns: 0,
                    content_hash_hex: None,
                    chunk_hashes_hex: vec![],
                    archive_reason: None,
                    expires_at: None,
                }],
            },
        )
        .unwrap();

    // Run GC at now + 8 days (past 7-day mirror retention, lease expiry, and 300s orphan grace)
    let gc = GarbageCollector::new(
        Arc::clone(&srv.state.db),
        Arc::clone(&srv.state.vault_store),
    );
    let report = gc
        .run_gc(
            &srv.default_vault_id,
            now + 8 * 86_400,
            GfsPolicy::default(),
        )
        .unwrap();
    assert_eq!(report.deleted_chunks, 1);

    let mirror_hash = blake3_hash(chunk_mirror_bytes);
    let diff_old_hash = blake3_hash(chunk_diff_old_bytes);
    let diff_cur_hash = blake3_hash(chunk_diff_cur_bytes);

    assert!(
        !srv.state
            .vault_store
            .has_verified_chunk(&srv.default_vault_id, &mirror_hash)
            .unwrap(),
        "expired mirror archive chunk must be reclaimed by GC"
    );
    assert!(
        srv.state
            .vault_store
            .has_verified_chunk(&srv.default_vault_id, &diff_old_hash)
            .unwrap(),
        "diff_backup historical version chunk (expires_at IS NULL) must never be reclaimed"
    );
    assert!(
        srv.state
            .vault_store
            .has_verified_chunk(&srv.default_vault_id, &diff_cur_hash)
            .unwrap(),
        "source_deleted=1 current entry chunk must never be reclaimed"
    );
}

#[tokio::test]
async fn clipboard_message_dedup_receipts_and_history() {
    let srv = start_test_server("clipboard-dedup").await;
    register_trusted_peer(&srv.state.db, "peer-online", "Online Phone");
    register_trusted_peer(&srv.state.db, "peer-offline", "Offline Laptop");
    srv.state.touch_peer_lease("peer-online");

    let mut ws_rx = srv.state.ws_tx.subscribe();

    let payload = json!({
        "client_msg_id": "msg-dedup-001",
        "target_device_ids": ["peer-online", "peer-offline", "peer-revoked"],
        "kind": "text",
        "content": "hello idempotent clipboard"
    });

    let (status, first_resp) = http_json(
        srv.addr,
        "POST",
        "/api/clipboard",
        &srv.admin_cookie,
        Some(payload.clone()),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(first_resp["client_msg_id"].as_str(), Some("msg-dedup-001"));
    assert_eq!(first_resp["delivered_to"].as_u64(), Some(1));
    assert_eq!(first_resp["accepted_to"].as_u64(), Some(1));
    assert!(first_resp["server_ack_at"].as_i64().is_some());
    assert_eq!(
        first_resp["timestamp"].as_i64(),
        first_resp["server_ack_at"].as_i64()
    );

    let targets = first_resp["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 3);
    assert_eq!(targets[0]["target_device_id"].as_str(), Some("peer-online"));
    assert_eq!(targets[0]["status"].as_str(), Some("delivered"));
    assert_eq!(
        targets[1]["target_device_id"].as_str(),
        Some("peer-offline")
    );
    assert_eq!(targets[1]["status"].as_str(), Some("offline"));
    assert_eq!(
        targets[2]["target_device_id"].as_str(),
        Some("peer-revoked")
    );
    assert_eq!(targets[2]["status"].as_str(), Some("revoked"));

    // Verify WS received ClipboardReceived (with client_msg_id) and ClipboardStatus
    let ev1 = ws_rx.try_recv().unwrap();
    match ev1 {
        WsServerEvent::ClipboardReceived {
            client_msg_id,
            target_device_id,
            ..
        } => {
            assert_eq!(client_msg_id.as_deref(), Some("msg-dedup-001"));
            assert_eq!(target_device_id.as_deref(), Some("peer-online"));
        }
        other => panic!("expected ClipboardReceived, got {other:?}"),
    }
    let ev2 = ws_rx.try_recv().unwrap();
    match ev2 {
        WsServerEvent::ClipboardStatus {
            client_msg_id,
            delivered_to,
            accepted_to,
            targets,
            ..
        } => {
            assert_eq!(client_msg_id, "msg-dedup-001");
            assert_eq!(delivered_to, 1);
            assert_eq!(accepted_to, 1);
            assert_eq!(targets.len(), 3);
        }
        other => panic!("expected ClipboardStatus, got {other:?}"),
    }

    // Re-send same client_msg_id -> deduplicated, identical ack, zero extra WS broadcasts
    let (status, dup_resp) = http_json(
        srv.addr,
        "POST",
        "/api/clipboard",
        &srv.admin_cookie,
        Some(payload),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(dup_resp, first_resp);
    assert!(
        ws_rx.try_recv().is_err(),
        "duplicate client_msg_id must not re-broadcast WS events"
    );

    // Verify GET /api/clipboard/history returns recorded transfer
    let (status, hist) = http_json(
        srv.addr,
        "GET",
        "/api/clipboard/history?limit=10",
        &srv.admin_cookie,
        None,
    )
    .await;
    assert_eq!(status, 200);
    let transfers = hist["transfers"].as_array().unwrap();
    assert_eq!(transfers.len(), 1);
    assert_eq!(transfers[0]["kind"].as_str(), Some("text"));
    assert!(hist["outbox"].as_array().is_some());
}
