//! `lanvault-client`: LanVault 跨平台增量备份客户端引擎。

pub mod clipboard;
pub mod error;
pub mod reconnect;
pub mod scanner;
pub mod sync_engine;
pub mod watcher;

pub use clipboard::{
    ClipboardClient, ClipboardPushRequest, ClipboardPushResponse, DecryptedClipboardItem,
    MAX_CLIPBOARD_JSON_BYTES, MAX_TEMP_FILE_BYTES,
};
pub use error::ClientError;
pub use reconnect::{
    backoff_delay_for_attempt, ReconnectController, ReconnectEvent, ReconnectState,
    ReconnectTarget, BACKOFF_SCHEDULE_SECS,
};
pub use scanner::{
    delete_file_states, inspect_single_file, reconcile_directory, upsert_file_state, DiffResult,
    FileMetadataSnapshot, StoredFileState,
};
pub use sync_engine::{
    DeviceEnvironment, PolicyBlockReason, PowerNetworkPolicy, SyncEngine, SyncProgress,
    SyncSummary, TokenBucket,
};
pub use watcher::{
    canonicalize_watch_root, DirectoryWatcher, RescanReason, WatchEvent, DEBOUNCE_WINDOW,
    WIN32_ERROR_NOTIFY_ENUM_DIR,
};
