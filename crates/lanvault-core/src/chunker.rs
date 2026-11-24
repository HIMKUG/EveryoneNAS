//! Content-Defined Chunking (FastCDC), BLAKE3 hashing, and zstd compression.
//!
//! Reference implementation: `vendor_refs/rust-ffsync-cdc/src/chunking/{chunker.rs, hasher.rs}`.
//! Adapted to LanVault's 1 MiB average chunk target (256 KiB min, 4 MiB max) with
//! strict parameter boundary validation before entering `fastcdc::v2020`.

use std::io::Read;

use fastcdc::v2020::{
    FastCDC, StreamCDC, AVERAGE_MAX, AVERAGE_MIN, MAXIMUM_MAX, MAXIMUM_MIN, MINIMUM_MAX,
    MINIMUM_MIN,
};

use crate::crypto::{decrypt_aead, encrypt_aead_deterministic, ChunkSealContext, SymmetricKey};
use crate::error::{LanVaultError, Result};

/// Minimum FastCDC chunk size (256 KiB).
pub const CHUNK_MIN_SIZE: u32 = 256 * 1024;
/// Average FastCDC chunk size (1 MiB).
pub const CHUNK_AVG_SIZE: u32 = 1024 * 1024;
/// Maximum FastCDC chunk size (4 MiB).
pub const CHUNK_MAX_SIZE: u32 = 4 * 1024 * 1024;
/// Default zstd compression level balancing LAN throughput and ratio.
pub const DEFAULT_ZSTD_LEVEL: i32 = 3;

/// Validated FastCDC chunking parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkParams {
    pub min_size: u32,
    pub avg_size: u32,
    pub max_size: u32,
}

impl Default for ChunkParams {
    fn default() -> Self {
        Self {
            min_size: CHUNK_MIN_SIZE,
            avg_size: CHUNK_AVG_SIZE,
            max_size: CHUNK_MAX_SIZE,
        }
    }
}

impl ChunkParams {
    /// Validates and constructs FastCDC parameters at the external boundary.
    ///
    /// Mirrors `sane_cdc_params` in `vendor_refs/rust-ffsync-cdc/src/chunking/chunker.rs`
    /// so release builds never index past fastcdc's mask tables.
    pub fn new(min_size: u32, avg_size: u32, max_size: u32) -> Result<Self> {
        let valid = min_size <= avg_size
            && avg_size <= max_size
            && (MINIMUM_MIN..=MINIMUM_MAX).contains(&min_size)
            && (AVERAGE_MIN..=AVERAGE_MAX).contains(&avg_size)
            && (MAXIMUM_MIN..=MAXIMUM_MAX).contains(&max_size)
            && min_size.is_multiple_of(2)
            && avg_size.is_multiple_of(2)
            && max_size.is_multiple_of(2);

        if !valid {
            return Err(LanVaultError::InvalidChunkParams {
                min: min_size,
                avg: avg_size,
                max: max_size,
            });
        }

        Ok(Self {
            min_size,
            avg_size,
            max_size,
        })
    }
}

/// Metadata for a single content-defined chunk boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkDescriptor {
    pub offset: u64,
    pub length: usize,
    pub raw_hash: [u8; 32],
}

/// A compressed and optionally AEAD-encrypted chunk ready for content-addressed storage.
#[derive(Debug, Clone)]
pub struct PackedChunk {
    /// BLAKE3 hash of `stored_bytes` (the content-addressed storage key on the server).
    pub stored_hash: [u8; 32],
    /// Original uncompressed plaintext length in bytes.
    pub raw_size: usize,
    /// Final stored bytes (zstd compressed, and XChaCha20-Poly1305 encrypted if E2EE is enabled).
    pub stored_bytes: Vec<u8>,
}

/// Computes the 32-byte BLAKE3 hash of an in-memory slice.
pub fn blake3_hash(data: &[u8]) -> [u8; 32] {
    *blake3::hash(data).as_bytes()
}

/// Computes the 32-byte BLAKE3 hash of a stream using `std::io::copy`
/// (following `vendor_refs/rust-ffsync-cdc/src/chunking/hasher.rs`).
pub fn blake3_hash_reader<R: Read>(reader: &mut R) -> Result<[u8; 32]> {
    let mut hasher = blake3::Hasher::new();
    std::io::copy(reader, &mut hasher)?;
    Ok(*hasher.finalize().as_bytes())
}

/// Splits an in-memory byte slice using FastCDC and computes BLAKE3 hashes per chunk.
pub fn chunk_bytes(data: &[u8], params: ChunkParams) -> Vec<ChunkDescriptor> {
    if data.is_empty() {
        return Vec::new();
    }
    FastCDC::new(data, params.min_size, params.avg_size, params.max_size)
        .map(|entry| {
            let slice = &data[entry.offset..entry.offset + entry.length];
            ChunkDescriptor {
                offset: entry.offset as u64,
                length: entry.length,
                raw_hash: blake3_hash(slice),
            }
        })
        .collect()
}

/// Streams a `Read` source through `StreamCDC`, invoking `on_chunk` for each chunk.
pub fn chunk_stream<R, F>(reader: R, params: ChunkParams, mut on_chunk: F) -> Result<u64>
where
    R: Read,
    F: FnMut(ChunkDescriptor, Vec<u8>) -> Result<()>,
{
    let cdc = StreamCDC::new(reader, params.min_size, params.avg_size, params.max_size);
    let mut total_bytes = 0u64;
    for chunk_res in cdc {
        let chunk = chunk_res.map_err(|e| LanVaultError::Io(std::io::Error::other(e)))?;
        let raw_hash = blake3_hash(&chunk.data);
        let desc = ChunkDescriptor {
            offset: chunk.offset,
            length: chunk.length,
            raw_hash,
        };
        total_bytes += chunk.length as u64;
        on_chunk(desc, chunk.data)?;
    }
    Ok(total_bytes)
}

/// Compresses raw chunk bytes with zstd at the given compression level.
pub fn compress_zstd(data: &[u8], level: i32) -> Result<Vec<u8>> {
    zstd::bulk::compress(data, level)
        .map_err(|e| LanVaultError::Compression(format!("zstd compress failed: {e}")))
}

/// Decompresses zstd-encoded bytes while enforcing a strict upper bound to guard against bombs.
pub fn decompress_zstd(compressed: &[u8], max_decompressed_size: usize) -> Result<Vec<u8>> {
    let decoder = zstd::stream::Decoder::new(compressed)
        .map_err(|e| LanVaultError::Compression(format!("zstd decoder init failed: {e}")))?;
    let mut limited = decoder.take((max_decompressed_size as u64) + 1);
    let mut out = Vec::new();
    limited
        .read_to_end(&mut out)
        .map_err(|e| LanVaultError::Compression(format!("zstd decompress failed: {e}")))?;

    if out.len() > max_decompressed_size {
        return Err(LanVaultError::Compression(format!(
            "decompressed chunk exceeded limit of {max_decompressed_size} bytes"
        )));
    }
    Ok(out)
}

/// Packs a raw chunk: zstd compress -> optional deterministic XChaCha20-Poly1305 E2EE -> BLAKE3 stored hash.
pub fn pack_chunk(
    plaintext: &[u8],
    e2ee_key: Option<&SymmetricKey>,
    aad: &[u8],
    zstd_level: i32,
) -> Result<PackedChunk> {
    let compressed = compress_zstd(plaintext, zstd_level)?;
    let stored_bytes = match e2ee_key {
        Some(key) => encrypt_aead_deterministic(key, &compressed, aad)?,
        None => compressed,
    };
    let stored_hash = blake3_hash(&stored_bytes);
    Ok(PackedChunk {
        stored_hash,
        raw_size: plaintext.len(),
        stored_bytes,
    })
}

/// High-throughput chunk packer using a pre-initialized `ChunkSealContext`
/// (eliminates per-chunk KDF, cipher schedule setup, and AAD prefix re-hashing).
pub fn pack_chunk_with_context(
    plaintext: &[u8],
    seal_ctx: Option<&ChunkSealContext>,
    zstd_level: i32,
) -> Result<PackedChunk> {
    let mut probe =
        crate::debug_telemetry::ProbeGuard::start("core.chunker", "chunker.pack_chunk", None);
    probe.add_bytes(plaintext.len() as u64);
    let compressed = compress_zstd(plaintext, zstd_level)?;
    let stored_bytes = match seal_ctx {
        Some(ctx) => ctx.encrypt_deterministic(&compressed)?,
        None => compressed,
    };
    let stored_hash = blake3_hash(&stored_bytes);
    Ok(PackedChunk {
        stored_hash,
        raw_size: plaintext.len(),
        stored_bytes,
    })
}

/// Verifies stored BLAKE3 integrity, decrypts if E2EE is active, and zstd-decompresses the chunk.

/// 块压缩编码类型：`Zstd` 为默认压缩，`Raw` 跳过压缩（用于高熵媒体或快速模式）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkCompression {
    Zstd,
    Raw,
}

impl ChunkCompression {
    pub fn wire_encoding(self) -> &'static str {
        match self {
            ChunkCompression::Zstd => "xchacha20poly1305+zstd",
            ChunkCompression::Raw => "xchacha20poly1305+raw",
        }
    }

    pub fn db_tag(self) -> &'static str {
        match self {
            ChunkCompression::Zstd => "zstd",
            ChunkCompression::Raw => "raw",
        }
    }

    pub fn from_db_tag(tag: &str) -> Self {
        if tag.eq_ignore_ascii_case("raw") {
            ChunkCompression::Raw
        } else {
            ChunkCompression::Zstd
        }
    }
}

/// 高熵已压缩格式黑名单：这类文件再跑 zstd 几乎零收益却白耗 CPU。
pub fn is_incompressible_extension(path: &std::path::Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "jpg"
            | "jpeg"
            | "heic"
            | "heif"
            | "png"
            | "webp"
            | "avif"
            | "gif"
            | "mp4"
            | "mov"
            | "mkv"
            | "avi"
            | "webm"
            | "m4v"
            | "mp3"
            | "aac"
            | "flac"
            | "ogg"
            | "m4a"
            | "zip"
            | "7z"
            | "rar"
            | "gz"
            | "xz"
            | "zst"
            | "bz2"
            | "apk"
            | "ipa"
    )
}

/// 自适应打包：根据 `force_raw` 与前 4 KiB 试压收益自动决定走 `Zstd` 还是 `Raw`。
pub fn pack_chunk_adaptive_with_context(
    plaintext: &[u8],
    seal_ctx: Option<&ChunkSealContext>,
    zstd_level: i32,
    force_raw: bool,
) -> Result<(PackedChunk, ChunkCompression)> {
    let mut use_raw = force_raw;
    if !use_raw && plaintext.len() >= 4096 {
        if let Ok(sample) = compress_zstd(&plaintext[..4096], 1) {
            if sample.len() * 100 > 4096 * 92 {
                use_raw = true;
            }
        }
    }
    let (payload, compression) = if use_raw {
        (plaintext.to_vec(), ChunkCompression::Raw)
    } else {
        (
            compress_zstd(plaintext, zstd_level)?,
            ChunkCompression::Zstd,
        )
    };
    let stored_bytes = match seal_ctx {
        Some(ctx) => ctx.encrypt_deterministic(&payload)?,
        None => payload,
    };
    let stored_hash = blake3_hash(&stored_bytes);
    Ok((
        PackedChunk {
            stored_hash,
            raw_size: plaintext.len(),
            stored_bytes,
        },
        compression,
    ))
}

/// 按存储压缩标记解包数据块。
pub fn unpack_chunk_with_compression(
    stored_bytes: &[u8],
    expected_stored_hash: &[u8; 32],
    e2ee_key: Option<&SymmetricKey>,
    aad: &[u8],
    max_raw_size: usize,
    compression: ChunkCompression,
) -> Result<Vec<u8>> {
    let actual_hash = blake3_hash(stored_bytes);
    if actual_hash != *expected_stored_hash {
        return Err(LanVaultError::CorruptedChunk {
            hash: hex::encode(expected_stored_hash),
        });
    }
    let inner = match e2ee_key {
        Some(key) => decrypt_aead(key, stored_bytes, aad)?,
        None => stored_bytes.to_vec(),
    };
    match compression {
        ChunkCompression::Raw => {
            if inner.len() > max_raw_size {
                return Err(LanVaultError::Compression(format!(
                    "raw chunk exceeded limit of {max_raw_size} bytes"
                )));
            }
            Ok(inner)
        }
        ChunkCompression::Zstd => decompress_zstd(&inner, max_raw_size),
    }
}

pub fn unpack_chunk(
    stored_bytes: &[u8],
    expected_stored_hash: &[u8; 32],
    e2ee_key: Option<&SymmetricKey>,
    aad: &[u8],
    max_raw_size: usize,
) -> Result<Vec<u8>> {
    let actual_hash = blake3_hash(stored_bytes);
    if actual_hash != *expected_stored_hash {
        return Err(LanVaultError::CorruptedChunk {
            hash: hex::encode(expected_stored_hash),
        });
    }
    let compressed = match e2ee_key {
        Some(key) => decrypt_aead(key, stored_bytes, aad)?,
        None => stored_bytes.to_vec(),
    };
    decompress_zstd(&compressed, max_raw_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_chunk_is_deterministic_for_zero_knowledge_dedup() {
        let master = SymmetricKey::generate();
        let vault_id = "vault-alpha";
        let vault_key = master.derive_vault_chunk_key(vault_id);
        let payload = b"repeatable content-defined chunk payload for dedup";

        let p1 = pack_chunk(
            payload,
            Some(&vault_key),
            vault_id.as_bytes(),
            DEFAULT_ZSTD_LEVEL,
        )
        .unwrap();
        let p2 = pack_chunk(
            payload,
            Some(&vault_key),
            vault_id.as_bytes(),
            DEFAULT_ZSTD_LEVEL,
        )
        .unwrap();
        assert_eq!(p1.stored_hash, p2.stored_hash);
        assert_eq!(p1.stored_bytes, p2.stored_bytes);

        let restored = unpack_chunk(
            &p1.stored_bytes,
            &p1.stored_hash,
            Some(&vault_key),
            vault_id.as_bytes(),
            CHUNK_MAX_SIZE as usize,
        )
        .unwrap();
        assert_eq!(restored, payload);

        assert!(unpack_chunk(
            &p1.stored_bytes,
            &p1.stored_hash,
            Some(&vault_key),
            b"vault-beta",
            CHUNK_MAX_SIZE as usize,
        )
        .is_err());
    }
}
