//! v2 Incremental Catalog, Chunk Batch Check, Fenced Sync Leases, and Version Download routes.

use super::*;
use lanvault_core::{
    CatalogCommitRequest, CatalogCommitResponse, CatalogDeltaKind, ChunkBatchCheckRequest,
    ChunkBatchCheckResponse,
};

const MAX_CHUNK_BATCH_CHECK: usize = 4096;

fn default_lease_ttl_secs() -> i64 {
    60
}

fn default_catalog_page_limit() -> usize {
    200
}

#[derive(Debug, Deserialize)]
pub(super) struct AcquireSyncLeaseRequest {
    rule_id: String,
    holder_device_id: Option<String>,
    group_revision: i64,
    rule_revision: i64,
    #[serde(default = "default_lease_ttl_secs")]
    ttl_secs: i64,
}

#[derive(Debug, Deserialize)]
pub(super) struct CatalogListQuery {
    after_path_key: Option<String>,
    #[serde(default = "default_catalog_page_limit")]
    limit: usize,
    #[serde(default)]
    source_deleted_only: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct CatalogVersionsQuery {
    path_key: Option<String>,
    after_archived_at: Option<i64>,
    after_version_id: Option<String>,
    #[serde(default = "default_catalog_page_limit")]
    limit: usize,
}

fn map_catalog_error(err: LanVaultError) -> Response {
    match err {
        LanVaultError::InvalidState(msg) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": msg })),
        )
            .into_response(),
        other => map_domain_error(other),
    }
}

pub(super) async fn handle_check_chunks(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(vault_id): Path<String>,
    Json(req): Json<ChunkBatchCheckRequest>,
) -> Response {
    if let Err(resp) = require_vault_access(&state, &session, &vault_id) {
        return resp;
    }
    if req.hashes_hex.len() > MAX_CHUNK_BATCH_CHECK {
        return map_domain_error(LanVaultError::InvalidInput(format!(
            "chunk batch check exceeds maximum of {MAX_CHUNK_BATCH_CHECK} hashes"
        )));
    }

    let mut parsed = Vec::with_capacity(req.hashes_hex.len());
    for h_hex in &req.hashes_hex {
        let decoded = match decode_hex_32(h_hex, "hashes_hex") {
            Ok(h) => h,
            Err(err) => return map_domain_error(err),
        };
        parsed.push((hex::encode(decoded), decoded));
    }

    let db = Arc::clone(&state.db);
    let vault_store = Arc::clone(&state.vault_store);
    let join_res =
        tokio::task::spawn_blocking(move || -> lanvault_core::Result<ChunkBatchCheckResponse> {
            if db.get_vault(&vault_id)?.is_none() {
                return Err(LanVaultError::NotFound(format!("vault {vault_id}")));
            }
            let mut present = Vec::new();
            let mut missing = Vec::new();
            for (normalized_hex, hash_bytes) in parsed {
                if vault_store.has_verified_chunk(&vault_id, &hash_bytes)? {
                    present.push(normalized_hex);
                } else {
                    missing.push(normalized_hex);
                }
            }
            Ok(ChunkBatchCheckResponse { present, missing })
        })
        .await;

    match join_res {
        Ok(Ok(resp)) => (StatusCode::OK, Json(resp)).into_response(),
        Ok(Err(err)) => map_catalog_error(err),
        Err(err) => {
            warn!("chunk batch check blocking task failed: {err}");
            map_domain_error(LanVaultError::InvalidState(
                "chunk_batch_check_failed".to_string(),
            ))
        }
    }
}

pub(super) async fn handle_acquire_sync_lease(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(binding_id): Path<String>,
    Json(req): Json<AcquireSyncLeaseRequest>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if req.rule_id.trim().is_empty() {
        return map_domain_error(LanVaultError::InvalidInput(
            "rule_id must not be empty".to_string(),
        ));
    }

    let holder_device_id = if session.is_admin {
        req.holder_device_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| state.device_id.clone())
    } else {
        let Some(dev_id) = session.device_id.clone() else {
            return unauthorized_response();
        };
        if let Some(requested) = req.holder_device_id.as_deref().map(str::trim) {
            if !requested.is_empty() && requested != dev_id {
                return (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({ "error": "peer_identity_mismatch" })),
                )
                    .into_response();
            }
        }
        dev_id
    };

    let binding = match state.db.get_rule_target_binding(&binding_id) {
        Ok(Some(b)) => b,
        Ok(None) => {
            return map_domain_error(LanVaultError::NotFound(format!(
                "rule_target_binding `{binding_id}`"
            )))
        }
        Err(err) => return map_domain_error(err),
    };
    let group = match require_group_access(&state.db, &session, &binding.group_id) {
        Ok(g) => g,
        Err(resp) => return resp,
    };
    if group.revision != req.group_revision {
        return map_catalog_error(LanVaultError::InvalidState(format!(
            "group `{}` revision mismatch (expected {}, got {})",
            group.group_id, group.revision, req.group_revision
        )));
    }

    let ttl_secs = req.ttl_secs.clamp(1, 600);
    match state.db.acquire_or_renew_sync_lease(
        req.rule_id.trim(),
        &binding_id,
        &holder_device_id,
        req.group_revision,
        req.rule_revision,
        ttl_secs,
    ) {
        Ok(lease) => (StatusCode::OK, Json(lease)).into_response(),
        Err(err) => map_catalog_error(err),
    }
}

pub(super) async fn handle_commit_catalog(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(binding_id): Path<String>,
    Json(req): Json<CatalogCommitRequest>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if req.operation_id.trim().is_empty() || req.rule_id.trim().is_empty() {
        return map_domain_error(LanVaultError::InvalidInput(
            "operation_id and rule_id must not be empty".to_string(),
        ));
    }

    // Parse all referenced chunk hashes at the HTTP boundary before entering blocking disk/DB work.
    let mut unique_hashes: HashSet<[u8; 32]> = HashSet::new();
    for op in &req.ops {
        if matches!(
            op.kind,
            CatalogDeltaKind::UpsertFile | CatalogDeltaKind::UpsertDir
        ) {
            for hash_hex in &op.chunk_hashes_hex {
                let decoded = match decode_hex_32(hash_hex, "chunk_hashes_hex") {
                    Ok(h) => h,
                    Err(err) => return map_domain_error(err),
                };
                unique_hashes.insert(decoded);
            }
        }
    }

    let binding = match state.db.get_rule_target_binding(&binding_id) {
        Ok(Some(b)) => b,
        Ok(None) => {
            return map_domain_error(LanVaultError::NotFound(format!(
                "rule_target_binding `{binding_id}`"
            )))
        }
        Err(err) => return map_domain_error(err),
    };
    let group = match require_group_access(&state.db, &session, &binding.group_id) {
        Ok(g) => g,
        Err(resp) => return resp,
    };
    if !binding.enabled || binding.revoked_at.is_some() || binding.rule_id != req.rule_id {
        return map_catalog_error(LanVaultError::InvalidState(format!(
            "binding `{binding_id}` is disabled, revoked, or mismatched rule_id"
        )));
    }
    if group.revision != req.group_revision {
        return map_catalog_error(LanVaultError::InvalidState(format!(
            "group `{}` revision mismatch (expected {}, got {})",
            group.group_id, group.revision, req.group_revision
        )));
    }
    let Some(vault_id) = binding.target_vault_id.clone() else {
        return map_catalog_error(LanVaultError::InvalidState(format!(
            "binding `{binding_id}` has no target_vault_id configured"
        )));
    };

    let _write_guard = ActiveChunkWriteGuard::new(Arc::clone(&state));
    let db = Arc::clone(&state.db);
    let vault_store = Arc::clone(&state.vault_store);
    let group_id = binding.group_id.clone();
    let rule_id = req.rule_id.clone();
    let bid = binding_id.clone();

    let join_res =
        tokio::task::spawn_blocking(move || -> lanvault_core::Result<CatalogCommitResponse> {
            // Verify all referenced chunks exist on disk BEFORE acquiring vault_store.with_write_lock
            // so disk reads never block concurrent chunk uploads.
            for hash in &unique_hashes {
                if !vault_store.has_verified_chunk(&vault_id, hash)? {
                    return Err(LanVaultError::NotFound(format!(
                        "chunk {} in vault {vault_id}",
                        hex::encode(hash)
                    )));
                }
            }

            vault_store.with_write_lock(|| db.commit_catalog_delta(&bid, &vault_id, &req))
        })
        .await;

    match join_res {
        Ok(Ok(resp)) => {
            if !resp.deduplicated_replay {
                let _ = state.ws_tx.send(WsServerEvent::GroupSyncUpdate {
                    group_id,
                    rule_id,
                    binding_id: resp.binding_id.clone(),
                    operation_id: resp.operation_id.clone(),
                    new_revision: resp.new_revision,
                    applied_ops: resp.applied_ops,
                    timestamp: unix_timestamp_secs(),
                });
            }
            (StatusCode::OK, Json(resp)).into_response()
        }
        Ok(Err(err)) => map_catalog_error(err),
        Err(err) => {
            warn!("catalog commit blocking task failed: {err}");
            map_domain_error(LanVaultError::InvalidState(
                "catalog_commit_failed".to_string(),
            ))
        }
    }
}

pub(super) async fn handle_get_catalog_operation(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path((binding_id, operation_id)): Path<(String, String)>,
) -> Response {
    let binding = match state.db.get_rule_target_binding(&binding_id) {
        Ok(Some(b)) => b,
        Ok(None) => {
            return map_domain_error(LanVaultError::NotFound(format!(
                "rule_target_binding `{binding_id}`"
            )))
        }
        Err(err) => return map_domain_error(err),
    };
    if let Err(resp) = require_group_access(&state.db, &session, &binding.group_id) {
        return resp;
    }

    let op = match state.db.get_sync_operation(&operation_id) {
        Ok(Some(op)) if op.binding_id == binding_id => op,
        Ok(_) => {
            return map_domain_error(LanVaultError::NotFound(format!(
                "sync_operation `{operation_id}` for binding `{binding_id}`"
            )))
        }
        Err(err) => return map_domain_error(err),
    };

    let response: CatalogCommitResponse = match serde_json::from_str(&op.response_json) {
        Ok(resp) => resp,
        Err(err) => return map_domain_error(err.into()),
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "operation_id": op.operation_id,
            "rule_id": op.rule_id,
            "binding_id": op.binding_id,
            "status": op.status,
            "created_at": op.created_at,
            "updated_at": op.updated_at,
            "response": response,
        })),
    )
        .into_response()
}

pub(super) async fn handle_list_catalog_entries(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(binding_id): Path<String>,
    Query(query): Query<CatalogListQuery>,
) -> Response {
    let binding = match state.db.get_rule_target_binding(&binding_id) {
        Ok(Some(b)) => b,
        Ok(None) => {
            return map_domain_error(LanVaultError::NotFound(format!(
                "rule_target_binding `{binding_id}`"
            )))
        }
        Err(err) => return map_domain_error(err),
    };
    if let Err(resp) = require_group_access(&state.db, &session, &binding.group_id) {
        return resp;
    }

    let limit = query.limit.clamp(1, 1000);
    let entries = match state.db.list_catalog_entries_page(
        &binding_id,
        query.after_path_key.as_deref(),
        limit,
        query.source_deleted_only,
    ) {
        Ok(entries) => entries,
        Err(err) => return map_domain_error(err),
    };
    let next_cursor = entries.last().map(|e| e.path_key.clone());

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "binding_id": binding_id,
            "entries": entries,
            "next_cursor": next_cursor,
        })),
    )
        .into_response()
}

pub(super) async fn handle_list_catalog_versions(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(binding_id): Path<String>,
    Query(query): Query<CatalogVersionsQuery>,
) -> Response {
    let binding = match state.db.get_rule_target_binding(&binding_id) {
        Ok(Some(b)) => b,
        Ok(None) => {
            return map_domain_error(LanVaultError::NotFound(format!(
                "rule_target_binding `{binding_id}`"
            )))
        }
        Err(err) => return map_domain_error(err),
    };
    if let Err(resp) = require_group_access(&state.db, &session, &binding.group_id) {
        return resp;
    }

    let limit = query.limit.clamp(1, 1000);
    let versions = match state.db.list_catalog_versions_page(
        &binding_id,
        query.path_key.as_deref(),
        query.after_archived_at,
        query.after_version_id.as_deref(),
        limit,
    ) {
        Ok(versions) => versions,
        Err(err) => return map_domain_error(err),
    };
    let next_after_archived_at = versions.last().map(|v| v.archived_at);
    let next_after_version_id = versions.last().map(|v| v.version_id.clone());

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "binding_id": binding_id,
            "versions": versions,
            "next_after_archived_at": next_after_archived_at,
            "next_after_version_id": next_after_version_id,
        })),
    )
        .into_response()
}

pub(super) async fn handle_download_catalog_version(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path((binding_id, version_id)): Path<(String, String)>,
) -> Response {
    let binding = match state.db.get_rule_target_binding(&binding_id) {
        Ok(Some(b)) => b,
        Ok(None) => {
            return map_domain_error(LanVaultError::NotFound(format!(
                "rule_target_binding `{binding_id}`"
            )))
        }
        Err(err) => return map_domain_error(err),
    };
    if let Err(resp) = require_group_access(&state.db, &session, &binding.group_id) {
        return resp;
    }

    let db = Arc::clone(&state.db);
    let vid = version_id.clone();
    let bid = binding_id.clone();

    let join_res = tokio::task::spawn_blocking(
        move || -> lanvault_core::Result<(String, String, Vec<[u8; 32]>, bool)> {
            let version = db
                .get_catalog_version(&vid)?
                .ok_or_else(|| LanVaultError::NotFound(format!("catalog_version `{vid}`")))?;
            if version.binding_id != bid {
                return Err(LanVaultError::NotFound(format!(
                    "catalog_version `{vid}` for binding `{bid}`"
                )));
            }
            if version.recycled_at.is_some() {
                return Err(LanVaultError::NotFound(format!(
                    "catalog_version `{vid}` has been recycled"
                )));
            }
            let vault = db
                .get_vault(&version.vault_id)?
                .ok_or_else(|| LanVaultError::NotFound(format!("vault {}", version.vault_id)))?;
            let hash_hexes: Vec<String> = serde_json::from_str(&version.chunk_hashes_json)?;
            let mut chunk_hashes = Vec::with_capacity(hash_hexes.len());
            for hash_hex in &hash_hexes {
                chunk_hashes.push(decode_hex_32(hash_hex, "catalog version chunk hash")?);
            }
            let filename = StdPath::new(&version.display_path)
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.replace(['"', '\r', '\n'], "_"))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "download.bin".to_string());
            Ok((version.vault_id, filename, chunk_hashes, vault.e2ee_enabled))
        },
    )
    .await;

    match join_res {
        Ok(Ok((vault_id, filename, chunk_hashes, e2ee_enabled))) => {
            let disposition = format!("attachment; filename=\"{filename}.lanvault-chunks\"");
            let stream_store = Arc::clone(&state.vault_store);
            let chunk_stream = futures_util::stream::try_unfold(
                (chunk_hashes.into_iter(), stream_store, vault_id),
                |(mut iter, store, vid)| async move {
                    let Some(hash) = iter.next() else {
                        return Ok::<_, std::io::Error>(None);
                    };
                    let store_bg = Arc::clone(&store);
                    let vid_bg = vid.clone();
                    let bytes =
                        tokio::task::spawn_blocking(move || store_bg.get_chunk(&vid_bg, &hash))
                            .await
                            .map_err(|e| {
                                std::io::Error::other(format!("chunk stream join error: {e}"))
                            })?
                            .map_err(|e| std::io::Error::other(e.to_string()))?;
                    let frame_len: u32 = bytes.len().try_into().map_err(|_| {
                        std::io::Error::other(format!(
                            "chunk exceeds u32 frame limit: {}",
                            bytes.len()
                        ))
                    })?;
                    let mut framed = Vec::with_capacity(4 + bytes.len());
                    framed.extend_from_slice(&frame_len.to_be_bytes());
                    framed.extend_from_slice(&bytes);
                    Ok(Some((axum::body::Bytes::from(framed), (iter, store, vid))))
                },
            );

            let mut resp = match axum::response::Response::builder()
                .status(StatusCode::OK)
                .body(axum::body::Body::from_stream(chunk_stream))
            {
                Ok(resp) => resp,
                Err(err) => {
                    warn!("failed to build streaming catalog version response: {err}");
                    return map_domain_error(LanVaultError::InvalidState(
                        "failed to build catalog version stream".to_string(),
                    ));
                }
            };
            resp.headers_mut().insert(
                CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            );
            if let Ok(val) = HeaderValue::from_str(&disposition) {
                resp.headers_mut().insert(CONTENT_DISPOSITION, val);
            }
            resp.headers_mut().insert(
                HeaderName::from_static("x-lanvault-encoded"),
                HeaderValue::from_static("true"),
            );
            resp.headers_mut().insert(
                HeaderName::from_static("x-lanvault-chunk-framing"),
                HeaderValue::from_static("u32be-aead-v1"),
            );
            if e2ee_enabled {
                resp.headers_mut().insert(
                    HeaderName::from_static("x-lanvault-e2ee"),
                    HeaderValue::from_static("true"),
                );
            }
            resp
        }
        Ok(Err(err)) => map_catalog_error(err),
        Err(err) => {
            warn!("catalog version download blocking task failed: {err}");
            map_domain_error(LanVaultError::InvalidState(
                "catalog_version_download_failed".to_string(),
            ))
        }
    }
}
