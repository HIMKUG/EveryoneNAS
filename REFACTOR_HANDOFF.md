# Refactoring Handoff Report (`REFACTOR_HANDOFF.md`)

EveryoneNAS full-spectrum audit & autonomous refactoring handoff following the `draft-to-opus` workflow.

## 1. Safety & Rollback Anchor

- **Original Backup Branch / Tag**: `main` at `19ec7f8`, annotated tag `pre-audit-20261124`
- **Active Refactor Branch**: `refactor/audit-fixes`
- **One-Command Full Rollback**: `git reset --hard pre-audit-20261124`
- **Commits Applied (One Per Tranche)**:
  - `19ec7f8` — Android R8 `RoomDatabase` Keep rule fix (`proguard-rules.pro`, on `main` / tag anchor)
  - `c8814b4` — Tranche 1: `lanvault-core` + `lanvault-win-svc` (`woff2.rs`, `db.rs` pagination/grace-period/volumes, `debug_telemetry.rs`, `firewall.rs` `CREATE_NO_WINDOW`)
  - `0c5408c` — Tranche 2: `lanvault-server` + `lanvault-client` (interactive pairing queue/WS/routes, GC 300s orphan grace, RAID1 self-heal, `write_lock` unlock, auth re-pair fix, N+1 removal, `sync_engine` resilience)
  - `7b6a232` — Tranche 3: `lanvault-ffi` (`request_pairing` / `list_pending_pair_requests` / `respond_pair_request`, `scan_windows_networks` + `CREATE_NO_WINDOW`, manifest `restore_snapshot`, `spawn_blocking`, `lanvault_ffi_decode_woff2`)
  - `814f5a9` — Tranche 4: Platform runners + WebUI + Audit Report (Win32 COM `IFileOpenDialog`, Android `LaunchTheme`/`NormalTheme`, WOFF2 bundled fonts, `docs/audit-report.html`)
  - `Tranche 5 (HEAD)` — Flutter UI/Theme/Bridge + Win32 UTF-8 fix + `flutter_app/test/widget_test.dart` + `REFACTOR_HANDOFF.md`

## 2. Module-by-Module Before vs. After

| Module / UI Category | Action Taken | Before (Draft / Patched State) | After (Clean State) | Locked External Contract |
| :--- | :--- | :--- | :--- | :--- |
| `crates/lanvault-core` (`woff2.rs`, `db.rs`, `lib.rs`, `debug_telemetry.rs`) | `Consolidate` | Missing WOFF2 decoding, missing `revoked_sessions` expiry index & `chunks.created_at`, N+1 vault volume fetches, uncached telemetry JSON byte size | Added `decode_woff2_to_sfnt` via `brotli-decompressor`; added SQL pagination, orphan grace query, bulk `list_all_volumes()`, `.max(0) as u64` guards | `db.rs` schema backward-compatible (`ALTER TABLE`/`CREATE INDEX IF NOT EXISTS`); `crypto.rs` & `chunker.rs` untouched (exempt) |
| `crates/lanvault-win-svc` (`firewall.rs`, `main.rs`) | `Prune` | `netsh` spawned flashing console windows; volume state loaded via N+1 loop | Applied `CREATE_NO_WINDOW` (`0x0800_0000`) to all `netsh` invocations; switched to `list_all_volumes()` | Service control commands and firewall rule names unchanged |
| `crates/lanvault-server` (`vault.rs`, `auth.rs`, `gc.rs`, `web/mod.rs`, `web/debug_routes.rs`) | `Rewrite` (targeted paths) | No interactive two-way pairing approval; re-pairing stayed locked out in `revoked_devices`; GC lacked grace period; `has_verified_chunk` inside `write_lock`; self-overlapping extra volumes unchecked | Added `pair-request`/`pair-status`/`pair-requests`/`respond` + `WsServerEvent::PairingRequested`; `unrevoke_device`; 300s `ORPHAN_GRACE_SECS`; moved reads out of `write_lock`; self-overlap check + `ActiveChunkWriteGuard` + `spawn_blocking` | Existing `/api/*` endpoints and JSON DTO shapes preserved; admin-only WS events scoped |
| `crates/lanvault-client` (`clipboard.rs`, `sync_engine.rs`) | `Consolidate` | Missing HTTP `connect_timeout`; bare `expect()`; full `known_remote_chunks.clear()` on cap | Added `connect_timeout`, `sanitize_received_filename`, 25% gradual LRU-style eviction, `abort_all()` on upload failure | Sync rule scheduling & FastCDC chunk upload protocol preserved |
| `crates/lanvault-ffi` (`api.rs`, `c_abi.rs`) | `Consolidate` | `ipconfig` & PowerShell network scans flashed consoles; `restore_snapshot` concatenated raw deduped chunks; per-call `reqwest::Client`; manual-only schedule ignored | Added shared `http_client`, `request_pairing`, `list_pending_pair_requests`, `respond_pair_request`, `scan_windows_networks` (`CREATE_NO_WINDOW`), manifest tree `restore_snapshot`, `lanvault_ffi_decode_woff2`/`lanvault_ffi_free_bytes` | Existing `lanvault_ffi_invoke_json` + `lanvault_ffi_free_string` C-ABI signatures unchanged |
| `flutter_app/windows/runner` (`flutter_window.cpp`, `CMakeLists.txt`) | `Rewrite` | Directory/file picking invoked PowerShell WinForms (~several seconds stall) | Native Win32 COM `IFileOpenDialog` (`pickDirectory` / `pickFile` on `com.lanvault.app/keep_alive`) with `/utf-8` + `\uXXXX` literals and `ComPtrGuard` | `MethodChannel('com.lanvault.app/keep_alive')` returns UTF-8 path string or `null` on cancel |
| `flutter_app/android` (`AndroidManifest.xml`, `styles.xml`, `proguard-rules.pro`) | `Prune` | Black native ActionBar above Flutter canvas; R8 stripped `RoomDatabase` constructors | Restored `@style/LaunchTheme` + `NormalTheme` (`Theme.Light.NoTitleBar`); kept `RoomDatabase` constructors under R8 full mode | Android KeepAlive foreground service & Package ID `com.lanvault.app` preserved |
| Fonts (`SourceHanSansSC-Regular.woff2`, `web-ui`, `pubspec.yaml`, `main.dart`) | `Consolidate` | Mixed system fallback fonts; user requested compressed `SourceHanSansSC-Regular-2.otf` | Compressed `16,437,608 B` -> `11,428,300 B` WOFF2 (69.5%); wired `@font-face` in WebUI; bundled & decoded via FFI + `FontLoader('SourceHanSansSC')` in Flutter | Graceful `try/catch` fallback when native library is absent in unit tests |
| `flutter_app/lib/bridge` (`lanvault_bridge.dart`, `system_env_helper.dart`) | `Consolidate` | Dart spawned `powershell.exe`/`cmd.exe` on 15s polling and file picking; Windows/Android elevated server start skipped `_notifyStateChanged()` | Zero `Process.run` calls for picker/Wi-Fi scan; 2s incoming-pair poll -> `pendingPairRequestsNotifier`; shared sync broadcast stream; `_disposed` guards | Public `LanVaultBridge` / `SystemEnvHelper` APIs preserved; added interactive pairing & WOFF2 decode helpers |
| `flutter_app/lib/theme` + `main.dart` + 7 `features/*` | `Consolidate` | 4 custom Dialog implementations; backup task vertical 1-char squeeze on mobile; clipped settings tabs & 15px close hitbox; `+ 新建存储池` active when service stopped | Unified `NeuDialog`/`NeuIconBadge`/`NeuKeyValueRow`/`NeuEmptyState`; global interactive incoming pairing confirmation dialog; dark `#1E293B` `disabledLook` + Toast on stopped service; responsive mobile stacking & `ValueKey`s | Visual tokens converged in `NeuTokens`, `NeuMotion`, and `NeuRadius` |

## 3. Verification Results

- **Rust Unit Tests** (`CARGO_TARGET_DIR=H:\cargo_target_nas cargo test --workspace`): **16 passed, 0 failed** (`lanvault-core`: 12, `lanvault-server`: 3, `lanvault-client`: 1)
- **Flutter Static Analysis** (`flutter analyze`): **0 issues found**
- **Flutter Widget Tests** (`flutter test`): **5 passed, 0 failed** (`formatUserError` mapping, `NeuButton` `disabledLook`, `VaultScreen` service gating Toast, `BackupScreen` 360px narrow layout single-line rendering, `LanVaultShell` global incoming pairing dialog acceptance)
- **Windows Desktop Release Build** (`cargo build --release -p lanvault-ffi` + `flutter build windows --release`): Built `flutter_app\build\windows\x64\runner\Release\lanvault_app.exe` (exit code 0, `/W4 /WX`)
- **Android ARM64 Release APK Build** (`cargo ndk -t arm64-v8a -P 26 --lib -p lanvault-ffi` + `flutter build apk --release --target-platform android-arm64 --obfuscate --split-debug-info=build/symbols/android`): Built `flutter_app\build\app\outputs\flutter-apk\app-release.apk` (`47.4MB`, exit code 0)
- **Physical Android UI Verification**: Not re-run on phone after final refactor because user unplugged the Android test device prior to implementation; verified via 360px widget tests and Release APK compilation instead.

## 4. Guide for Follow-Up AI (If Anything Breaks in Manual Testing)

If a regression appears during manual testing, pass this file to a follow-up AI with the failing symptom and affected module name from Section 2. Fix only against the locked contract without re-introducing global patches:
- Interactive pairing state is driven by `POST /api/peers/pair-request`, `POST /api/peers/pair-requests/{request_id}/respond`, and `LanVaultBridge.pendingPairRequestsNotifier` -> `LanVaultShell._showIncomingPairDialog`.
- Windows native picker runs through `MethodChannel("com.lanvault.app/keep_alive")` in `windows/runner/flutter_window.cpp` (`ShowNativeItemDialog`); keep UTF-16 literals ASCII-escaped or `/utf-8` enabled on MSVC.
- Vault creation gating uses `NeuButton.disabledLook` bound to `_runningPort == null && !bridge.serverRunningNotifier.value` in `VaultScreen`.
