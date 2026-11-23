# LanVault Draft-to-Opus 重构交接单 (`REFACTOR_HANDOFF.md`)

- **回滚检查点标签**：`pre-draft-to-opus-refactor`
- **重构工作分支**：`refactor/draft-to-opus`
- **执行范围**：全量落地体检报告中扫出的全部 7 项建议修复项（含 2 项高风险高收益项、1 项中风险项与 4 项低风险项）。

## 已完成分批提交清单

1. **死代码清理与认证缓存上限（Tranche 1 & 2A）**
   - `crates/lanvault-core/src/protocol.rs`、`lib.rs`：删除全仓零调用的 17 种二进制帧死代码，仅保留现行 HTTP/WS 使用的 `ClipboardKind`。
   - `crates/lanvault-server/src/auth.rs`：新增 `prune_expired()` 及容量上限（`MAX_SESSIONS = 256`、`MAX_IP_LOCKS = 4096`），杜绝长期常驻或局域网扫描导致内存无限增长。
2. **大文件快照流式下载与 Web 路由模块化（Tranche 2B）**
   - `crates/lanvault-server/src/web/mod.rs`、`web/debug_routes.rs`：`handle_snapshot_file_download` 改为 `futures_util::stream::try_unfold` + `axum::body::Body::from_stream` 逐块流式输出，消除 4GB+ 大文件整块缓冲 OOM；将调试接口拆至 `web/debug_routes.rs`。
3. **去重缓存上限、批量落库、重连抖动与 JNI 高熵密码（Tranche 3）**
   - `crates/lanvault-client/src/sync_engine.rs`：`known_remote_chunks` 增加 `MAX_KNOWN_REMOTE_CHUNKS = 100_000` 上限；`sync_diff` 改为单事务批量调用 `db.upsert_local_file_states_batch`。
   - `crates/lanvault-client/src/reconnect.rs`：新增 `backoff_delay_with_jitter`（在 `2s/5s/15s/30s` 基础上叠加 ±15% 随机抖动防止多机惊群）。
   - `crates/lanvault-ffi/src/api.rs`、`c_abi.rs`：移除 Android JNI 弱默认密码兜底，改为首次生成 24 位高熵随机密码并持久化于 `client_state.db`。
4. **Web 二维码拆模块与发布运维文档（Tranche 4）**
   - `web-ui/js/qr.js`、`app.js`、`index.html`：将标准 ISO/IEC 18004 二维码编码器独立拆出为 `web-ui/js/qr.js`（全量 JS 通过 `node --check`）。
   - 新增 `README.md`、`.env.example`、`.gitignore` 与 `flutter_app/android/key.properties.example`。

## 回滚与本地验收命令

- **一键回滚至重构前**：`git reset --hard pre-draft-to-opus-refactor`
- **本地全量编译与单测验收**：
  ```powershell
  cargo test --workspace
  cd flutter_app && flutter analyze
  .\scripts\build_windows_release.ps1
  .\scripts\build_android_release.ps1
  ```
