//! `lanvault-client`: LanVault 跨平台增量备份与镜像同步客户端引擎。

pub mod clipboard;
pub mod error;
pub mod mirror_engine;
pub mod outbox;
pub mod reconnect;
pub mod scanner;
pub mod sync_engine;
pub mod transport;
pub mod watcher;

pub use clipboard::{
    ClipboardClient, ClipboardPushRequest, ClipboardPushResponse, DecryptedClipboardItem,
    ResumableFileTransferClient, MAX_CLIPBOARD_JSON_BYTES, MAX_TEMP_FILE_BYTES,
};
pub use error::ClientError;
pub use mirror_engine::{
    execute_mirror_sync, preview_mirror_sync, recover_mirror_journal, MirrorApplyReport,
    MirrorFaultStage, MirrorRecoveryReport, MIRROR_STAGING_DIR,
};
pub use outbox::{
    dispatch_pending_outbox, dispatch_pending_outbox_with_config, enqueue_file_message,
    enqueue_text_message, retry_failed_outbox_message, FrozenOutboxTarget, OutboxDispatchConfig,
    OutboxDispatchReport, OUTBOX_BACKOFF_SECS, OUTBOX_MAX_TRANSIENT_RETRIES,
};
pub use reconnect::{
    backoff_delay_for_attempt, ReconnectController, ReconnectEvent, ReconnectState,
    ReconnectTarget, BACKOFF_SCHEDULE_SECS,
};
pub use scanner::{
    delete_file_states, inspect_single_file, reconcile_directory, scan_source_generation,
    upsert_file_state, DiffResult, FileMetadataSnapshot, ScannedEntry, ScannedEntryKind,
    SourceGeneration, StoredFileState,
};
pub use sync_engine::{
    compute_two_pointer_merge, BindingSyncReport, DeviceEnvironment, MergeAction, MergePlan,
    PolicyBlockReason, PowerNetworkPolicy, SyncEngine, SyncProgress, SyncSummary, TokenBucket,
};
pub use watcher::{
    canonicalize_watch_root, DirectoryWatcher, RescanReason, WatchEvent, DEBOUNCE_WINDOW,
    WIN32_ERROR_NOTIFY_ENUM_DIR,
};
