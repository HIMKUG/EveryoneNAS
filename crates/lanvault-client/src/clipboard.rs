//! 复用加密信道的局域网剪贴板双向同步与临时文件快传客户端。
//!
//! 对齐 `ARCHITECTURE_CONTRACTS.md` §3.1 路由 11 (`POST /api/clipboard`)：
//! `{ "target_device_id": string|null, "kind": "text|file", "content": string, "filename": string|null }`。

use std::path::Path;
use std::time::Duration;

use lanvault_core::{decrypt_aead, encrypt_aead, SymmetricKey};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::ClientError;
use lanvault_core::CLIPBOARD_AAD;

/// 单次剪贴板/快传 JSON 请求体上限（12 MiB，匹配服务端 16 MiB 帧与请求体限制）。
pub const MAX_CLIPBOARD_JSON_BYTES: usize = 12 * 1024 * 1024;

/// 临时文件快传原始字节上限（5 MiB，经十六进制编码与 AEAD 封装后严格不超过 12 MiB JSON 上限）。
pub const MAX_TEMP_FILE_BYTES: u64 = 5 * 1024 * 1024;

/// 对齐契约 `POST /api/clipboard` 请求载荷。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardPushRequest {
    pub target_device_id: Option<String>,
    pub kind: String,
    pub content: String,
    pub filename: Option<String>,
}

/// 对齐契约 `POST /api/clipboard` 响应载荷。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardPushResponse {
    pub delivered_to: usize,
    pub timestamp: i64,
}

/// 经 `lanvault_core::encrypt_aead` 封装的 XChaCha20-Poly1305 加密信封（不含明文哈希，杜绝明文指纹泄露）。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct EncryptedEnvelope {
    /// `[24-byte XNonce || ciphertext || 16-byte Poly1305 tag]` 十六进制编码。
    wire_hex: String,
}

/// 解密后的剪贴板/快传载荷。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecryptedClipboardItem {
    Text(String),
    File { filename: String, bytes: Vec<u8> },
}

/// 局域网加密剪贴板与临时快传客户端。
pub struct ClipboardClient {
    http: Client,
    endpoint_base_url: String,
    session_cookie: Option<String>,
    channel_key: SymmetricKey,
}

impl ClipboardClient {
    pub fn new(
        endpoint_base_url: String,
        session_cookie: Option<String>,
        channel_key: SymmetricKey,
    ) -> Result<Self, ClientError> {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self {
            http,
            endpoint_base_url: endpoint_base_url.trim_end_matches('/').to_string(),
            session_cookie,
            channel_key,
        })
    }

    /// 推送加密剪贴板文本至指定已配对设备（`None` 表示广播至所有在线设备）。
    pub async fn push_text(
        &self,
        target_device_id: Option<String>,
        text: &str,
    ) -> Result<ClipboardPushResponse, ClientError> {
        let envelope = seal_payload(text.as_bytes(), &self.channel_key)?;
        let content = serde_json::to_string(&envelope)?;
        self.send_request(ClipboardPushRequest {
            target_device_id,
            kind: "text".to_string(),
            content,
            filename: None,
        })
        .await
    }

    /// 推送临时文件至指定已配对设备（校验大小边界后加密发送）。
    pub async fn push_temp_file(
        &self,
        target_device_id: Option<String>,
        file_path: &Path,
    ) -> Result<ClipboardPushResponse, ClientError> {
        let meta = tokio::fs::metadata(file_path).await?;
        if !meta.is_file() {
            return Err(ClientError::InvalidPath {
                path: file_path.to_path_buf(),
                reason: "temp transfer target must be a regular file".to_string(),
            });
        }
        if meta.len() > MAX_TEMP_FILE_BYTES {
            return Err(ClientError::InvalidInput(format!(
                "temp file exceeds limit ({} > {} bytes)",
                meta.len(),
                MAX_TEMP_FILE_BYTES
            )));
        }

        let filename = file_path
            .file_name()
            .ok_or_else(|| ClientError::InvalidPath {
                path: file_path.to_path_buf(),
                reason: "missing file name".to_string(),
            })?
            .to_string_lossy()
            .into_owned();

        let bytes = tokio::fs::read(file_path).await?;
        let envelope = seal_payload(&bytes, &self.channel_key)?;
        let content = serde_json::to_string(&envelope)?;

        info!(filename = %filename, bytes = bytes.len(), "pushing encrypted temp file");
        self.send_request(ClipboardPushRequest {
            target_device_id,
            kind: "file".to_string(),
            content,
            filename: Some(filename),
        })
        .await
    }

    /// 解密并校验远端通过 WebSocket `clipboard_received` 推送的加密信封。
    pub fn receive_item(
        &self,
        kind: &str,
        content: &str,
        filename: Option<String>,
    ) -> Result<DecryptedClipboardItem, ClientError> {
        let envelope: EncryptedEnvelope = serde_json::from_str(content)?;
        let plaintext = open_payload(&envelope, &self.channel_key)?;

        match kind {
            "text" => {
                let text = String::from_utf8(plaintext)
                    .map_err(|e| ClientError::InvalidInput(format!("invalid UTF-8 text: {e}")))?;
                Ok(DecryptedClipboardItem::Text(text))
            }
            "file" => {
                let raw_name = filename.ok_or_else(|| {
                    ClientError::InvalidInput("file clipboard item missing filename".to_string())
                })?;
                let safe_name = sanitize_received_filename(&raw_name)?;
                Ok(DecryptedClipboardItem::File {
                    filename: safe_name,
                    bytes: plaintext,
                })
            }
            other => Err(ClientError::InvalidInput(format!(
                "unsupported clipboard kind: {other}"
            ))),
        }
    }

    async fn send_request(
        &self,
        payload: ClipboardPushRequest,
    ) -> Result<ClipboardPushResponse, ClientError> {
        let body = serde_json::to_vec(&payload)?;
        if body.len() > MAX_CLIPBOARD_JSON_BYTES {
            return Err(ClientError::InvalidInput(format!(
                "clipboard JSON payload exceeds limit ({} > {} bytes)",
                body.len(),
                MAX_CLIPBOARD_JSON_BYTES
            )));
        }

        let url = format!("{}/api/clipboard", self.endpoint_base_url);
        let mut req = self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .body(body);
        if let Some(cookie) = &self.session_cookie {
            req = req.header("Cookie", format!("lv_session={cookie}"));
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(ClientError::RemoteStatus {
                url,
                status: resp.status().as_u16(),
            });
        }
        let parsed = resp.json::<ClipboardPushResponse>().await?;
        Ok(parsed)
    }
}

fn seal_payload(plaintext: &[u8], key: &SymmetricKey) -> Result<EncryptedEnvelope, ClientError> {
    let wire = encrypt_aead(key, plaintext, CLIPBOARD_AAD)?;
    Ok(EncryptedEnvelope {
        wire_hex: hex::encode(wire),
    })
}

fn open_payload(envelope: &EncryptedEnvelope, key: &SymmetricKey) -> Result<Vec<u8>, ClientError> {
    let wire = hex::decode(&envelope.wire_hex)
        .map_err(|e| ClientError::Crypto(format!("invalid clipboard wire hex: {e}")))?;
    let plaintext = decrypt_aead(key, &wire, CLIPBOARD_AAD)?;
    Ok(plaintext)
}

fn sanitize_received_filename(raw: &str) -> Result<String, ClientError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.contains('\0') {
        return Err(ClientError::InvalidInput(
            "clipboard filename must not be empty or contain NUL".to_string(),
        ));
    }
    let basename = trimmed
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim();
    if basename.is_empty()
        || basename == "."
        || basename == ".."
        || basename.chars().any(|c| c.is_control())
    {
        return Err(ClientError::InvalidInput(format!(
            "unsafe clipboard filename `{raw}`"
        )));
    }
    Ok(basename.to_string())
}


/// 工业级流式分片直传客户端（突破 5 MiB JSON 限制，支持任意大文件 + 断点续传 + 分片 BLAKE3 校验）。
pub struct ResumableFileTransferClient {
    http: Client,
    endpoint_base_url: String,
    session_cookie: Option<String>,
}

impl ResumableFileTransferClient {
    pub fn new(endpoint_base_url: String, session_cookie: Option<String>) -> Result<Self, ClientError> {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .read_timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(8)
            .tcp_nodelay(true)
            .build()?;
        Ok(Self {
            http,
            endpoint_base_url: endpoint_base_url.trim_end_matches('/').to_string(),
            session_cookie,
        })
    }

    pub async fn send_file_resumable(&self, file_path: &Path) -> Result<(), ClientError> {
        use std::io::{Read, Seek, SeekFrom};
        const PART_SIZE: u64 = 2 * 1024 * 1024;

        let path_owned = file_path.to_path_buf();
        let (filename, total_size, file_blake3_hex) = tokio::task::spawn_blocking(move || -> std::io::Result<(String, u64, String)> {
            let meta = std::fs::metadata(&path_owned)?;
            let total_size = meta.len();
            let filename = path_owned
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string();
            let mut file = std::fs::File::open(&path_owned)?;
            let hash = lanvault_core::blake3_hash_reader(&mut file)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            Ok((filename, total_size, hex::encode(hash)))
        })
        .await
        .map_err(|e| ClientError::Io(std::io::Error::other(e.to_string())))??;

        let id_seed = format!("{filename}:{total_size}:{file_blake3_hex}");
        let transfer_id = hex::encode(&lanvault_core::blake3_hash(id_seed.as_bytes())[..16]);

        let init_url = format!("{}/api/transfers/init", self.endpoint_base_url);
        let mut init_req = self.http.post(&init_url).json(&serde_json::json!({
            "transfer_id": transfer_id,
            "filename": filename,
            "total_size": total_size,
            "chunk_size": PART_SIZE,
            "file_blake3_hex": file_blake3_hex,
        }));
        if let Some(c) = &self.session_cookie {
            init_req = init_req.header("Cookie", format!("lv_session={c}"));
        }
        let init_resp = init_req.send().await?;
        if !init_resp.status().is_success() {
            let status = init_resp.status().as_u16();
            let body = init_resp.text().await.unwrap_or_default();
            return Err(ClientError::RemoteRejected { status, body });
        }
        #[derive(Deserialize)]
        struct InitResp {
            total_parts: u64,
            received_parts: Vec<u64>,
        }
        let init_data: InitResp = init_resp.json().await?;
        let received_set: std::collections::HashSet<u64> =
            init_data.received_parts.into_iter().collect();

        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
        let mut tasks = tokio::task::JoinSet::new();

        for part_idx in 0..init_data.total_parts {
            if received_set.contains(&part_idx) {
                continue;
            }
            let permit = sem
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| ClientError::Io(std::io::Error::other("transfer semaphore closed")))?;
            let path_for_part = file_path.to_path_buf();
            let http = self.http.clone();
            let base = self.endpoint_base_url.clone();
            let cookie = self.session_cookie.clone();
            let tid = transfer_id.clone();

            tasks.spawn(async move {
                let _permit = permit;
                let offset = part_idx.saturating_mul(PART_SIZE);
                let to_read = if total_size == 0 {
                    0
                } else {
                    PART_SIZE.min(total_size.saturating_sub(offset)) as usize
                };
                let bytes = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
                    let mut buf = vec![0u8; to_read];
                    if to_read > 0 {
                        let mut f = std::fs::File::open(&path_for_part)?;
                        f.seek(SeekFrom::Start(offset))?;
                        f.read_exact(&mut buf)?;
                    }
                    Ok(buf)
                })
                .await
                .map_err(|e| ClientError::Io(std::io::Error::other(e.to_string())))??;

                let part_hash = hex::encode(lanvault_core::blake3_hash(&bytes));
                let part_url = format!("{base}/api/transfers/{tid}/parts/{part_idx}");
                let max_attempts = 4u32;
                let mut attempt = 0u32;
                loop {
                    attempt += 1;
                    let mut req = http
                        .put(&part_url)
                        .header("Content-Type", "application/octet-stream")
                        .header("X-LanVault-Part-Blake3", &part_hash)
                        .body(bytes.clone());
                    if let Some(ref c) = cookie {
                        req = req.header("Cookie", format!("lv_session={c}"));
                    }
                    match req.send().await {
                        Ok(resp) if resp.status().is_success() => return Ok::<(), ClientError>(()),
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if matches!(status, 408 | 429 | 500 | 502 | 503 | 504) && attempt < max_attempts {
                                tokio::time::sleep(Duration::from_millis(400u64 << (attempt - 1))).await;
                                continue;
                            }
                            let body = resp.text().await.unwrap_or_default();
                            return Err(ClientError::RemoteRejected { status, body });
                        }
                        Err(err) if attempt < max_attempts => {
                            tokio::time::sleep(Duration::from_millis(400u64 << (attempt - 1))).await;
                        }
                        Err(err) => return Err(err.into()),
                    }
                }
            });
        }

        while let Some(res) = tasks.join_next().await {
            res.map_err(|e| ClientError::Io(std::io::Error::other(e.to_string())))??;
        }

        let commit_url = format!("{}/api/transfers/{}/commit", self.endpoint_base_url, transfer_id);
        let mut commit_req = self.http.post(&commit_url);
        if let Some(c) = &self.session_cookie {
            commit_req = commit_req.header("Cookie", format!("lv_session={c}"));
        }
        let commit_resp = commit_req.send().await?;
        if !commit_resp.status().is_success() {
            let status = commit_resp.status().as_u16();
            let body = commit_resp.text().await.unwrap_or_default();
            return Err(ClientError::RemoteRejected { status, body });
        }
        Ok(())
    }
}
