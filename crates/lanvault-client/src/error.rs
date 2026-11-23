//! `lanvault-client` 统一错误类型（fail-fast，严禁内部吞错）。

use std::path::PathBuf;

use thiserror::Error;

use crate::sync_engine::PolicyBlockReason;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("invalid path `{path}`: {reason}")]
    InvalidPath { path: PathBuf, reason: String },

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("filesystem watcher error: {0}")]
    Notify(#[from] notify::Error),

    #[error("core error: {0}")]
    Core(#[from] lanvault_core::LanVaultError),

    #[error("HTTP transport error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("remote endpoint `{url}` returned HTTP {status}")]
    RemoteStatus { url: String, status: u16 },

    #[error("remote rejected upload with HTTP {status}: {body}")]
    RemoteRejected { status: u16, body: String },

    #[error("cryptographic error: {0}")]
    Crypto(String),

    #[error("sync blocked by power/network policy: {0:?}")]
    PolicyBlocked(PolicyBlockReason),
}
