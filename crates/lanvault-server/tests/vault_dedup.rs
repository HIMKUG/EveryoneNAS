//! Storage-hot-path regression tests.
//!
//! Reuse only verified content. Missing, truncated and same-size damaged chunks
//! must remain repairable without consuming quota twice.

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
fn identical_chunk_is_deduplicated() {
    let root = temp_dir("dedup");
    let (_db, store, vault_id) = store_with_vault(&root);
    let payload = vec![7u8; 128 * 1024];
    let hash = blake3_hash(&payload);

    assert!(store.put_chunk(&vault_id, &hash, payload.len() as u64, &payload).unwrap());
    assert!(!store.put_chunk(&vault_id, &hash, payload.len() as u64, &payload).unwrap());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn interrupted_upload_reuses_verified_chunks_and_repairs_missing_or_damaged_files() {
    let root = temp_dir("resume");
    let (db, store, vault_id) = store_with_vault(&root);
    let payload = vec![7; 128 * 1024];
    let size = payload.len() as u64;
    let hash = blake3_hash(&payload);
    assert!(store.put_chunk(&vault_id, &hash, size, &payload).unwrap());
    assert!(store.resume_chunk(&vault_id, &hash).unwrap());

    let path = lanvault_server::vault::chunk_file_path(&root.join("primary"), &hex::encode(hash));
    let vault = db.get_vault(&vault_id).unwrap().unwrap();
    std::fs::write(&path, vec![8; payload.len()]).unwrap();
    assert!(!store.resume_chunk(&vault_id, &hash).unwrap());
    assert!(store.validate_snapshot_chunks(&vault, &[hash]).is_err());
    assert!(store.put_chunk(&vault_id, &hash, size, &payload).unwrap());
    assert!(store.validate_snapshot_chunks(&vault, &[hash]).is_ok());

    std::fs::rename(&path, root.join("displaced")).unwrap();
    assert!(!store.resume_chunk(&vault_id, &hash).unwrap());
    assert!(store.put_chunk(&vault_id, &hash, size, &payload).unwrap());
    assert_eq!(db.get_vault(&vault_id).unwrap().unwrap().used_bytes, size);
    drop(store);
    drop(db);
    let _ = std::fs::remove_dir_all(root);
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
