//! 复用加密信道的局域网剪贴板双向同步与临时文件快传客户端。
//!
//! 对齐 `ARCHITECTURE_CONTRACTS.md` §3.1 路由 11 (`POST /api/clipboard`)：
//! `{ "target_device_id": string|null, "kind": "text|file", "content": string, "filename": string|null }`。

use std::fs;
use std::path::Path;
use std::time::Duration;

use lanvault_core::{decrypt_aead, encrypt_aead, SymmetricKey};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::ClientError;

/// 单次剪贴板/快传 JSON 请求体上限（12 MiB，匹配服务端 16 MiB 帧与请求体限制）。
pub const MAX_CLIPBOARD_JSON_BYTES: usize = 12 * 1024 * 1024;

/// 临时文件快传原始字节上限（5 MiB，经十六进制编码与 AEAD 封装后严格不超过 12 MiB JSON 上限）。
pub const MAX_TEMP_FILE_BYTES: u64 = 5 * 1024 * 1024;

const CLIPBOARD_AAD: &[u8] = b"lanvault.clipboard.v1";

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
        let meta = fs::metadata(file_path)?;
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

        let bytes = fs::read(file_path)?;
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
                let name = filename.ok_or_else(|| {
                    ClientError::InvalidInput("file clipboard item missing filename".to_string())
                })?;
                Ok(DecryptedClipboardItem::File {
                    filename: name,
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
