//! Strongly typed error hierarchy for LanVault core and server crates.
//!
//! Errors propagate upward without silent swallowing (`?`); external boundaries
//! map these variants into HTTP status codes, wire error frames, or FFI errors.

use thiserror::Error;

/// Unified error type across LanVault domain modules.
#[derive(Debug, Error)]
pub enum LanVaultError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("SQLite error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("JSON serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("cryptographic error: {0}")]
    Crypto(String),

    #[error("invalid Ed25519 signature")]
    InvalidSignature,

    #[error("invalid key length: expected {expected} bytes, got {actual}")]
    InvalidKeyLength { expected: usize, actual: usize },

    #[error("invalid BLAKE3 hash length: expected {expected} bytes, got {actual}")]
    InvalidHashLength { expected: usize, actual: usize },

    #[error("invalid FastCDC chunk parameters (min={min}, avg={avg}, max={max})")]
    InvalidChunkParams { min: u32, avg: u32, max: u32 },

    #[error("compression error: {0}")]
    Compression(String),

    #[error("LAN discovery error: {0}")]
    Discovery(String),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("protocol frame payload too large: {size} bytes (max {max})")]
    FrameTooLarge { size: usize, max: usize },

    #[error("unknown protocol frame type: 0x{0:02x}")]
    UnknownFrameType(u8),

    #[error("resource not found: {0}")]
    NotFound(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("vault quota exceeded: required {required} bytes, available {available} bytes")]
    QuotaExceeded { required: u64, available: u64 },

    #[error("corrupted chunk detected: {hash}")]
    CorruptedChunk { hash: String },

    #[error("integrity mismatch: expected {expected}, actual {actual}")]
    IntegrityMismatch { expected: String, actual: String },

    #[error("invalid internal state: {0}")]
    InvalidState(String),

    #[error("authentication error: {0}")]
    Auth(String),

    #[error("rate limited: retry after {retry_after_secs} seconds")]
    RateLimited { retry_after_secs: u64 },
}

/// Convenience result alias for LanVault operations.
pub type Result<T> = std::result::Result<T, LanVaultError>;
