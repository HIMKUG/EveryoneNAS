//! Storage-hot-path regression tests.
//!
//! These cover the two invariants that a throughput optimization can silently break:
//! 1. A duplicate chunk may only be deduplicated when the on-disk copy is the *full*
//!    expected size (a truncated file left by a power loss must be rewritten).
//! 2. Truly identical chunks must still be deduplicated after content verification.

use std::sync::Arc;

use lanvault_core::{blake3_hash, Database, PoolMode};
use lanvault_server::vault::VaultStore;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "lanvault-vault-{tag}-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn store_with_vault(root: &std::path::Path) -> (Arc<Database>, VaultStore, String) {
    let db = Arc::new(Database::open(root.join("index.db")).unwrap());
    let store = VaultStore::new(Arc::clone(&db));
    let (vault, _volumes) = store
        .create_vault(
            "Test".to_string(),
            root.join("primary").to_string_lossy().into_owned(),
            None,
            PoolMode::Single,
            64 * 1024 * 1024,
            true,
            Vec::new(),
        )
        .unwrap();
    (db, store, vault.vault_id)
}

#[test]
fn identical_chunk_is_deduplicated_after_verification() {
    let root = temp_dir("dedup");
    let (_db, store, vault_id) = store_with_vault(&root);
    let payload = vec![7u8; 128 * 1024];
    let hash = blake3_hash(&payload);

    assert!(store.put_chunk(&vault_id, &hash, payload.len() as u64, &payload).unwrap());
    // 第二次上传必须通过内容校验并被判定为去重（返回 false）。
    assert!(!store.put_chunk(&vault_id, &hash, payload.len() as u64, &payload).unwrap());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn truncated_on_disk_chunk_is_not_deduplicated_away() {
    let root = temp_dir("truncated");
    let (_db, store, vault_id) = store_with_vault(&root);
    let payload = vec![9u8; 64 * 1024];
    let hash = blake3_hash(&payload);
    assert!(store.put_chunk(&vault_id, &hash, payload.len() as u64, &payload).unwrap());

    // 模拟断电后留下的半截文件：长度不再等于存储字节数。
    let hash_hex = hex::encode(hash);
    let chunk_path = root
        .join("primary")
        .join("chunks")
        .join(&hash_hex[..2])
        .join(&hash_hex);
    std::fs::write(&chunk_path, &payload[..payload.len() / 2]).unwrap();

    // 若去重只看"文件存在"，这里会错误返回 false 并永久丢失这个块。
    let is_new = store.put_chunk(&vault_id, &hash, payload.len() as u64, &payload).unwrap();
    assert!(is_new, "truncated chunk must be rewritten, not deduplicated");

    let restored = store.get_chunk(&vault_id, &hash).unwrap();
    assert_eq!(restored, payload, "restored chunk must match the original bytes");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn same_size_corruption_is_detected_repaired_and_does_not_consume_quota_twice() {
    let root = temp_dir("same-size");
    let (db, store, vault_id) = store_with_vault(&root);
    let payload = vec![3u8; 64 * 1024];
    let hash = blake3_hash(&payload);
    assert!(store
        .put_chunk(&vault_id, &hash, payload.len() as u64, &payload)
        .unwrap());

    let hash_hex = hex::encode(hash);
    let chunk_path = root
        .join("primary")
        .join("chunks")
        .join(&hash_hex[..2])
        .join(&hash_hex);
    std::fs::write(&chunk_path, vec![0u8; payload.len()]).unwrap();
    assert!(!store.resume_chunk(&vault_id, &hash).unwrap());
    assert!(store.validate_snapshot_chunks(&vault_id, &[hash]).is_err());

    let vault = db.get_vault(&vault_id).unwrap().unwrap();
    db.set_vault_used_bytes(&vault_id, vault.quota_bytes).unwrap();
    assert!(store
        .put_chunk(&vault_id, &hash, payload.len() as u64, &payload)
        .unwrap());
    assert!(store.resume_chunk(&vault_id, &hash).unwrap());
    assert!(store
        .validate_snapshot_chunks(&vault_id, &[hash])
        .is_ok());
    assert_eq!(
        db.get_vault(&vault_id).unwrap().unwrap().used_bytes,
        vault.quota_bytes
    );

    let _ = std::fs::remove_dir_all(&root);
}
