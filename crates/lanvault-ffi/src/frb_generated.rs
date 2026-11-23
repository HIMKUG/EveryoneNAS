//! `flutter_rust_bridge` v2 `StreamSink` 轻量桥接占位定义。
//!
//! 执行 `flutter_rust_bridge_codegen generate` 时会自动覆盖生成完整胶水代码；
//! 此文件确保在运行代码生成前 `lanvault-ffi` 即可独立通过 `cargo check`。
//!
//! 参考依据 (`vendor_refs/rust-flutter-bridge`):
//! - `frb_rust/src/stream/stream_sink.rs`: `StreamSinkBase<T, SseCodec>`。
//! - `frb_rust/src/codec/sse.rs`: `SseCodec::encode(Rust2DartAction::Success, ...)`。

use std::io::Write;

use flutter_rust_bridge::for_generated::{Rust2DartAction, SseCodec, StreamSinkBase};
use flutter_rust_bridge::Rust2DartSendError;

/// 与 `flutter_rust_bridge_codegen` v2 生成签名一致的流式下沉句柄。
#[derive(Clone)]
pub struct StreamSink<T, Rust2DartCodec: flutter_rust_bridge::BaseCodec = SseCodec> {
    base: StreamSinkBase<T, Rust2DartCodec>,
}

impl<T> StreamSink<T, SseCodec>
where
    T: serde::Serialize,
{
    pub fn from_base(base: StreamSinkBase<T, SseCodec>) -> Self {
        Self { base }
    }

    /// 向 Dart `Stream<T>` 推送一条序列化事件。
    pub fn add(&self, value: T) -> Result<(), Rust2DartSendError> {
        let payload = serde_json::to_vec(&value).expect("FFI DTO serialization must succeed");
        let msg = SseCodec::encode(Rust2DartAction::Success, |serializer| {
            serializer
                .cursor
                .write_all(&payload)
                .expect("in-memory SSE cursor write cannot fail");
        });
        self.base.add_raw(msg)
    }
}
