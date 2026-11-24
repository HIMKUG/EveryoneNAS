//! Integration & unit tests for v2 client-side scanner, two-pointer merge,
//! crash-safe journaled mirror sync (with 3-stage fault injection & recovery),
//! external edit conflict protection, and persistent message outbox.

use std::fs;
use std::path::{Path, PathBuf};

use lanvault_client::mirror_engine::{
    execute_mirror_sync, preview_mirror_sync, recover_mirror_journal, MirrorFaultStage,
};
use lanvault_client::outbox::{
    enqueue_file_message, enqueue_text_message, retry_failed_outbox_message, FrozenOutboxTarget,
};
use lanvault_client::scanner::{scan_source_generation, ScannedEntry, ScannedEntryKind};
use lanvault_client::sync_engine::{compute_two_pointer_merge, MergeAction};
use lanvault_core::{
    blake3_hash, CatalogCommitRequest, CatalogDeltaKind, CatalogDeltaOp, CatalogEntryRecord,
    Database, DeviceGroupRecord, GroupSyncRuleRecord, RuleTargetBindingRecord, SymmetricKey,
    SyncRuleMode,
};
use uuid::Uuid;

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(format!("lanvault_{prefix}_{}", Uuid::new_v4()));
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn make_rule_and_binding(
    mode: SyncRuleMode,
    source_root: &Path,
    target_root: &Path,
) -> (
    DeviceGroupRecord,
    GroupSyncRuleRecord,
    RuleTargetBindingRecord,
) {
    let now = 1_700_000_000i64;
    let group = DeviceGroupRecord {
        group_id: "grp-1".to_string(),
        name: "Test Group".to_string(),
        coordinator_device_id: "dev-src".to_string(),
        revision: 1,
        created_at: now,
        updated_at: now,
        deleted_at: None,
    };
    let rule = GroupSyncRuleRecord {
        rule_id: "rule-1".to_string(),
        group_id: "grp-1".to_string(),
        name: "Mirror Rule".to_string(),
        mode,
        source_device_id: "dev-src".to_string(),
        source_root: source_root.to_string_lossy().into_owned(),
        source_root_id: None,
        archive_retention_days: 30,
        wifi_only: false,
        allowed_ssids: None,
        require_charging: false,
        max_kbps: 0,
        schedule_interval_secs: 0,
        enabled: true,
        revision: 1,
        group_revision: 1,
        needs_binding_confirm: false,
        created_at: now,
        updated_at: now,
    };
    let binding = RuleTargetBindingRecord {
        binding_id: "bind-1".to_string(),
        rule_id: "rule-1".to_string(),
        group_id: "grp-1".to_string(),
        target_device_id: "dev-tgt".to_string(),
        target_vault_id: Some("vault-1".to_string()),
        mirror_root_id: Some("mroot-1".to_string()),
        target_root: Some(target_root.to_string_lossy().into_owned()),
        enabled: true,
        revoked_at: None,
        applied_revision: 0,
        last_source_gen: 0,
        status: "ready".to_string(),
        last_error: None,
        needs_binding_confirm: false,
        created_at: now,
        updated_at: now,
    };
    (group, rule, binding)
}

fn make_scanned_file(path_key: &str, content: &[u8], mtime_ns: i64) -> ScannedEntry {
    ScannedEntry {
        path_key: path_key.to_string(),
        display_path: path_key.to_string(),
        abs_path: PathBuf::from(path_key),
        kind: ScannedEntryKind::File,
        size: content.len() as u64,
        mtime_ns,
        file_id: Some(1),
        content_hash: Some(blake3_hash(content)),
    }
}

fn make_scanned_dir(path_key: &str) -> ScannedEntry {
    ScannedEntry {
        path_key: path_key.to_string(),
        display_path: path_key.to_string(),
        abs_path: PathBuf::from(path_key),
        kind: ScannedEntryKind::Dir,
        size: 0,
        mtime_ns: 100,
        file_id: Some(2),
        content_hash: None,
    }
}

fn make_catalog_entry(
    path_key: &str,
    entry_kind: &str,
    content: Option<&[u8]>,
    mtime_ns: i64,
    source_deleted: bool,
) -> CatalogEntryRecord {
    CatalogEntryRecord {
        binding_id: "bind-1".to_string(),
        path_key: path_key.to_string(),
        display_path: path_key.to_string(),
        entry_kind: entry_kind.to_string(),
        version_id: format!("ver-{path_key}"),
        size: content.map(|b| b.len() as u64).unwrap_or(0),
        mtime_ns,
        content_hash: content.map(|b| blake3_hash(b).to_vec()),
        chunk_hashes_json: "[]".to_string(),
        source_deleted,
        source_deleted_at: if source_deleted {
            Some(1_700_000_000)
        } else {
            None
        },
        updated_revision: 1,
        updated_at: 1_700_000_000,
    }
}

#[test]
fn scan_source_generation_handles_complete_symlinks_and_collisions() {
    let dir = TempDirGuard::new("scan");
    let root = dir.path().join("src");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("a.txt"), b"hello").unwrap();
    fs::write(root.join("sub/b.txt"), b"world").unwrap();

    let gen = scan_source_generation(&root, &[]).unwrap();
    assert!(gen.complete);
    assert!(gen.blocked_reasons.is_empty());
    assert!(gen.volatile_paths.is_empty());
    assert!(gen.collisions.is_empty());
    assert_eq!(gen.entries.len(), 3);
    assert_eq!(gen.entries[0].path_key, "a.txt");
    assert_eq!(gen.entries[0].content_hash, Some(blake3_hash(b"hello")));
    assert_eq!(gen.entries[1].path_key, "sub");
    assert_eq!(gen.entries[1].kind, ScannedEntryKind::Dir);
    assert_eq!(gen.entries[2].path_key, "sub/b.txt");

    // Symlink root must be rejected outright.
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let link_root = dir.path().join("link_root");
        symlink(&root, &link_root).unwrap();
        assert!(scan_source_generation(&link_root, &[]).is_err());

        // Symlink inside directory must be skipped and recorded in blocked_reasons.
        symlink(root.join("a.txt"), root.join("sym.txt")).unwrap();
        let gen_with_sym = scan_source_generation(&root, &[]).unwrap();
        assert!(gen_with_sym
            .blocked_reasons
            .iter()
            .any(|r| r.contains("sym.txt")));
        assert!(!gen_with_sym.entries.iter().any(|e| e.path_key == "sym.txt"));

        // Case-fold collision detection (on case-sensitive Unix FS).
        fs::write(root.join("Case.txt"), b"upper").unwrap();
        fs::write(root.join("case.txt"), b"lower").unwrap();
        let gen_col = scan_source_generation(&root, &[]).unwrap();
        if gen_col
            .entries
            .iter()
            .filter(|e| e.path_key.eq_ignore_ascii_case("case.txt"))
            .count()
            == 2
        {
            assert!(gen_col
                .collisions
                .iter()
                .any(|c| c.kind == PathCollisionKind::CaseFoldCollision));
        }
    }
}

#[test]
fn two_pointer_merge_covers_all_five_states_and_guards() {
    let source = vec![
        make_scanned_file("added.txt", b"new", 100),
        make_scanned_file("changed_same_size_mtime.txt", b"BBBB", 500),
        make_scanned_file("resurrected.txt", b"same", 200),
        make_scanned_dir("swap_kind"),
        make_scanned_file("unchanged.txt", b"steady", 300),
    ];

    let target = vec![
        make_catalog_entry("already_deleted.txt", "file", Some(b"old"), 100, true),
        make_catalog_entry(
            "changed_same_size_mtime.txt",
            "file",
            Some(b"AAAA"),
            500,
            false,
        ),
        make_catalog_entry("removed.txt", "file", Some(b"gone"), 100, false),
        make_catalog_entry("resurrected.txt", "file", Some(b"same"), 200, true),
        make_catalog_entry("swap_kind", "file", Some(b"was-file"), 100, false),
        make_catalog_entry("unchanged.txt", "file", Some(b"steady"), 300, false),
    ];

    // 1. DiffBackup with complete scan:
    let diff_plan = compute_two_pointer_merge(&source, &target, SyncRuleMode::DiffBackup, true);
    assert_eq!(diff_plan.blocked_deletions, 0);
    // unchanged.txt + already_deleted.txt (skipped without refreshing source_deleted_at!)
    assert_eq!(diff_plan.unchanged_count, 2);
    assert!(diff_plan
        .actions
        .iter()
        .any(|a| matches!(a, MergeAction::Add(e) if e.path_key == "added.txt")));
    assert!(diff_plan.actions.iter().any(|a| matches!(
        a,
        MergeAction::Update { source, type_swap: false, .. }
            if source.path_key == "changed_same_size_mtime.txt"
    )));
    assert!(diff_plan.actions.iter().any(|a| matches!(
        a,
        MergeAction::MarkSourceDeleted { path_key, .. } if path_key == "removed.txt"
    )));
    assert!(!diff_plan.actions.iter().any(|a| matches!(
        a,
        MergeAction::MarkSourceDeleted { path_key, .. } if path_key == "already_deleted.txt"
    )));
    assert!(diff_plan.actions.iter().any(|a| matches!(
        a,
        MergeAction::ClearSourceDeleted { source } if source.path_key == "resurrected.txt"
    )));
    assert!(diff_plan.actions.iter().any(|a| matches!(
        a,
        MergeAction::Update { source, type_swap: true, .. } if source.path_key == "swap_kind"
    )));

    // 2. Incomplete scan MUST block deletions (both DiffBackup and MirrorSync):
    let incomplete_plan =
        compute_two_pointer_merge(&source, &target, SyncRuleMode::MirrorSync, false);
    assert_eq!(incomplete_plan.blocked_deletions, 2);
    assert!(!incomplete_plan
        .actions
        .iter()
        .any(|a| matches!(a, MergeAction::ArchiveAndRemove { .. })));

    // 3. MirrorSync with complete scan emits ArchiveAndRemove for target-only entries:
    let mirror_plan = compute_two_pointer_merge(&source, &target, SyncRuleMode::MirrorSync, true);
    assert_eq!(
        mirror_plan
            .actions
            .iter()
            .filter(|a| matches!(a, MergeAction::ArchiveAndRemove { .. }))
            .count(),
        2
    );

    // 4. Zero-change rerun produces empty plan:
    let identical_src = vec![make_scanned_file("ok.txt", b"data", 10)];
    let identical_tgt = vec![make_catalog_entry(
        "ok.txt",
        "file",
        Some(b"data"),
        10,
        false,
    )];
    let zero_plan = compute_two_pointer_merge(
        &identical_src,
        &identical_tgt,
        SyncRuleMode::DiffBackup,
        true,
    );
    assert!(zero_plan.is_empty());
    assert_eq!(zero_plan.unchanged_count, 1);
}

#[test]
fn mirror_sync_three_stage_crash_recovery_and_external_edit_protection() {
    let dir = TempDirGuard::new("mirror");
    let src_root = dir.path().join("source");
    let tgt_root = dir.path().join("target");
    let arc_root = dir.path().join("archive");
    fs::create_dir_all(&src_root).unwrap();
    fs::create_dir_all(&tgt_root).unwrap();
    fs::create_dir_all(&arc_root).unwrap();

    fs::write(src_root.join("doc.txt"), b"version-2").unwrap();
    fs::write(tgt_root.join("doc.txt"), b"version-1").unwrap();

    let db = Database::open_in_memory().unwrap();
    let (group, rule, binding) =
        make_rule_and_binding(SyncRuleMode::MirrorSync, &src_root, &tgt_root);
    db.create_device_group(&group).unwrap();
    db.upsert_group_sync_rule(&rule).unwrap();
    db.upsert_rule_target_binding(&binding).unwrap();

    let source_gen = scan_source_generation(&src_root, &[]).unwrap();
    let preview = preview_mirror_sync(
        &source_gen,
        &tgt_root,
        &rule.rule_id,
        &binding.binding_id,
        &binding.target_device_id,
    )
    .unwrap();
    assert_eq!(preview.overwrite_count, 1);
    assert_eq!(preview.add_count, 0);

    // Stage 1 fault: BeforeArchive -> target untouched, recovery commits journal cleanly.
    assert!(execute_mirror_sync(
        &db,
        &source_gen,
        &tgt_root,
        &arc_root,
        &rule,
        &binding,
        "op-stage1",
        Some(MirrorFaultStage::BeforeArchive),
    )
    .is_err());
    assert_eq!(fs::read(tgt_root.join("doc.txt")).unwrap(), b"version-1");
    let rec1 = recover_mirror_journal(&db, &tgt_root, &arc_root, &binding.binding_id).unwrap();
    assert_eq!(rec1.inspected, 1);
    assert_eq!(rec1.committed, 1);
    assert!(db
        .list_unfinished_mirror_journals(Some(&binding.binding_id))
        .unwrap()
        .is_empty());

    // Stage 2 fault: AfterArchiveBeforeApply -> archive exists on disk & in catalog_versions, target untouched.
    assert!(execute_mirror_sync(
        &db,
        &source_gen,
        &tgt_root,
        &arc_root,
        &rule,
        &binding,
        "op-stage2",
        Some(MirrorFaultStage::AfterArchiveBeforeApply),
    )
    .is_err());
    assert_eq!(fs::read(tgt_root.join("doc.txt")).unwrap(), b"version-1");
    let versions = db
        .list_catalog_versions_page(&binding.binding_id, Some("doc.txt"), None, None, 10)
        .unwrap();
    assert_eq!(versions.len(), 1);
    let rec2 = recover_mirror_journal(&db, &tgt_root, &arc_root, &binding.binding_id).unwrap();
    assert_eq!(rec2.committed, 1);
    assert_eq!(rec2.conflict_blocked, 0);

    // Stage 3 fault: AfterApplyBeforeCommit -> target already has version-2, recovery verifies and commits.
    assert!(execute_mirror_sync(
        &db,
        &source_gen,
        &tgt_root,
        &arc_root,
        &rule,
        &binding,
        "op-stage3",
        Some(MirrorFaultStage::AfterApplyBeforeCommit),
    )
    .is_err());
    assert_eq!(fs::read(tgt_root.join("doc.txt")).unwrap(), b"version-2");
    let rec3 = recover_mirror_journal(&db, &tgt_root, &arc_root, &binding.binding_id).unwrap();
    assert_eq!(rec3.committed, 1);
    assert_eq!(rec3.conflict_blocked, 0);

    // External edit conflict protection: record baseline "version-2" in catalog_entries,
    // externally modify target to "user-local-edit", update source to "version-3", and run mirror sync.
    let lease = db
        .acquire_or_renew_sync_lease(
            &rule.rule_id,
            &binding.binding_id,
            "dev-src",
            rule.group_revision,
            rule.revision,
            120,
        )
        .unwrap();
    db.commit_catalog_delta(
        &binding.binding_id,
        "vault-1",
        &CatalogCommitRequest {
            operation_id: "op-baseline".to_string(),
            rule_id: rule.rule_id.clone(),
            group_revision: lease.group_revision,
            rule_revision: lease.rule_revision,
            fence_token: lease.fence_token,
            base_revision: 0,
            source_gen: 1,
            ops: vec![CatalogDeltaOp {
                path_key: "doc.txt".to_string(),
                display_path: "doc.txt".to_string(),
                entry_kind: "file".to_string(),
                kind: CatalogDeltaKind::UpsertFile,
                size: 9,
                mtime_ns: 100,
                content_hash_hex: Some(hex::encode(blake3_hash(b"version-2"))),
                chunk_hashes_hex: Vec::new(),
                archive_reason: None,
                expires_at: None,
            }],
        },
    )
    .unwrap();

    fs::write(tgt_root.join("doc.txt"), b"user-local-edit").unwrap();
    fs::write(src_root.join("doc.txt"), b"version-3").unwrap();
    let gen_v3 = scan_source_generation(&src_root, &[]).unwrap();

    let conflict_report = execute_mirror_sync(
        &db,
        &gen_v3,
        &tgt_root,
        &arc_root,
        &rule,
        &binding,
        "op-conflict",
        None,
    )
    .unwrap();
    assert_eq!(
        conflict_report.conflict_blocked,
        vec!["doc.txt".to_string()]
    );
    assert_eq!(
        fs::read(tgt_root.join("doc.txt")).unwrap(),
        b"user-local-edit",
        "externally edited target file must be preserved"
    );
}

#[test]
fn outbox_enqueue_partial_failure_and_retry_failed_only() {
    let dir = TempDirGuard::new("outbox");
    let staging = dir.path().join("staging");
    let sample_file = dir.path().join("notes.txt");
    fs::write(&sample_file, b"important attachment").unwrap();

    let db = Database::open_in_memory().unwrap();
    let targets = vec![
        FrozenOutboxTarget {
            device_id: "dev-a".to_string(),
            device_name: "Device A".to_string(),
            endpoint_base_url: "http://127.0.0.1:18081".to_string(),
            session_cookie: Some("tok-a".to_string()),
            channel_key: SymmetricKey::generate(),
        },
        FrozenOutboxTarget {
            device_id: "dev-b".to_string(),
            device_name: "Device B".to_string(),
            endpoint_base_url: "http://127.0.0.1:18082".to_string(),
            session_cookie: Some("tok-b".to_string()),
            channel_key: SymmetricKey::generate(),
        },
    ];

    let text_msg = enqueue_text_message(&db, "conv-1", &targets, "hello lanvault").unwrap();
    assert_eq!(text_msg.local_seq, 1);
    assert_eq!(text_msg.overall_status, "sending");

    let file_msg = enqueue_file_message(&db, &staging, "conv-1", &targets, &sample_file).unwrap();
    assert_eq!(file_msg.local_seq, 2);
    assert_eq!(
        file_msg.file_hash_hex.as_deref(),
        Some(hex::encode(blake3_hash(b"important attachment")).as_str())
    );
    assert!(Path::new(file_msg.file_path.as_deref().unwrap()).is_file());

    // Simulate partial failure on text_msg: dev-a succeeds, dev-b fails permanently.
    db.update_outbox_target_status(
        &text_msg.client_msg_id,
        "dev-a",
        "sent",
        None,
        None,
        1,
        None,
    )
    .unwrap();
    let overall = db
        .update_outbox_target_status(
            &text_msg.client_msg_id,
            "dev-b",
            "failed",
            Some("http_403"),
            Some("forbidden"),
            1,
            None,
        )
        .unwrap();
    assert_eq!(overall, "partial_failed");

    // Retrying failed targets MUST reset ONLY dev-b (count == 1), leaving dev-a sent!
    let reset_count = retry_failed_outbox_message(&db, &text_msg.client_msg_id).unwrap();
    assert_eq!(reset_count, 1);

    let pending = db.list_pending_outbox_targets(i64::MAX, 100).unwrap();
    let text_pending: Vec<&str> = pending
        .iter()
        .filter(|t| t.client_msg_id == text_msg.client_msg_id)
        .map(|t| t.target_device_id.as_str())
        .collect();
    assert_eq!(text_pending, vec!["dev-b"]);
}
