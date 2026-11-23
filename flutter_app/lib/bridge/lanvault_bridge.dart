// 参考依据:
// - ARCHITECTURE_CONTRACTS.md 第 4 节 (flutter_rust_bridge FFI 接口及 DTO)
// - ARCHITECTURE_CONTRACTS.md 第 5 节 (Android com.lanvault.app/keep_alive MethodChannel)
// - 严格遵循无过度防御原则：仅在 FFI / MethodChannel 外部边界解析与校验，内部直接传递强类型并向上抛出错误。

import 'dart:async';
import 'dart:convert';
import 'dart:ffi' as ffi;
import 'dart:io';
import 'dart:isolate';

import 'package:ffi/ffi.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// 节点运行模式（仅客户端 / 仅服务端 / 双端模式）。
enum RoleMode {
  clientOnly('client_only', '仅客户端'),
  serverOnly('server_only', '仅服务端'),
  dual('dual', '双端模式');

  const RoleMode(this.wireValue, this.label);

  final String wireValue;
  final String label;

  bool get hasClient => this != RoleMode.serverOnly;
  bool get hasServer => this != RoleMode.clientOnly;

  static RoleMode fromWire(String raw) {
    for (final mode in RoleMode.values) {
      if (mode.wireValue == raw) return mode;
    }
    throw FormatException('Unsupported role_mode wire value: $raw');
  }
}

/// 保活等级枚举，严格对齐 ARCHITECTURE_CONTRACTS.md 第 3.1 / 5 节。
enum KeepAliveLevel {
  level3MagiskDaemon('LEVEL_3_MAGISK_DAEMON', 'Level 3 · Magisk 静态守护'),
  level2RootLibsu('LEVEL_2_ROOT_LIBSU', 'Level 2 · Root Daemon (libsu)'),
  level1Shizuku('LEVEL_1_SHIZUKU', 'Level 1 · Shizuku (UID 2000)'),
  level0Foreground('LEVEL_0_FOREGROUND', 'Level 0 · 标准前台服务'),
  winService('WIN_SERVICE', 'Windows SCM Session 0 服务');

  const KeepAliveLevel(this.wireValue, this.displayLabel);

  final String wireValue;
  final String displayLabel;

  bool get isElevatedAndroidDaemon =>
      this == KeepAliveLevel.level3MagiskDaemon ||
      this == KeepAliveLevel.level2RootLibsu ||
      this == KeepAliveLevel.level1Shizuku;

  static KeepAliveLevel parseWireValue(String raw) {
    for (final level in KeepAliveLevel.values) {
      if (level.wireValue == raw) return level;
    }
    throw FormatException('Unsupported keep_alive_level wire value: $raw');
  }
}

/// Bit-Rot 健康状态枚举，严格对齐 ARCHITECTURE_CONTRACTS.md 第 3.1 节。
enum BitRotHealth {
  healthy('healthy', '健康'),
  degraded('degraded', '降级'),
  corrupted('corrupted', '发现损坏块');

  const BitRotHealth(this.wireValue, this.displayLabel);

  final String wireValue;
  final String displayLabel;

  static BitRotHealth parseWireValue(String raw) {
    for (final item in BitRotHealth.values) {
      if (item.wireValue == raw) return item;
    }
    throw FormatException('Unsupported bit_rot_health wire value: $raw');
  }
}

/// 对应 Rust `DiscoveredPeer` DTO（ARCHITECTURE_CONTRACTS.md 第 4 节）。
@immutable
class DiscoveredPeer {
  const DiscoveredPeer({
    required this.deviceId,
    required this.deviceName,
    required this.osType,
    required this.ip,
    required this.port,
    required this.fingerprint,
  });

  final String deviceId;
  final String deviceName;
  final String osType;
  final String ip;
  final int port;
  final String fingerprint;

  factory DiscoveredPeer.fromJson(Map<String, dynamic> json) {
    return DiscoveredPeer(
      deviceId: json['device_id'] as String,
      deviceName: json['device_name'] as String,
      osType: json['os_type'] as String,
      ip: json['ip'] as String,
      port: (json['port'] as num).toInt(),
      fingerprint: json['fingerprint'] as String,
    );
  }
}

/// 对应 Rust `SyncProgressEvent` DTO（严格对齐 Rust 真实字段）。
@immutable
class SyncProgressEvent {
  const SyncProgressEvent({
    required this.ruleId,
    required this.filePath,
    required this.bytesProcessed,
    required this.chunksUploaded,
    required this.chunksDeduplicated,
    required this.status,
    this.throughputBps = 0,
    this.bitRotHealth = BitRotHealth.healthy,
    this.corruptedChunks = 0,
  });

  final String ruleId;
  final String filePath;
  final int bytesProcessed;
  final int chunksUploaded;
  final int chunksDeduplicated;
  final String status;
  final int throughputBps;
  final BitRotHealth bitRotHealth;
  final int corruptedChunks;

  int get totalChunks => chunksUploaded + chunksDeduplicated;

  double get dedupRatio => totalChunks <= 0
      ? 0.0
      : (chunksDeduplicated / totalChunks).clamp(0.0, 1.0);

  factory SyncProgressEvent.fromJson(Map<String, dynamic> json) {
    final filePath = json.containsKey('current_file')
        ? json['current_file'] as String
        : json['file_path'] as String;
    final rawBitRot = json['bit_rot_health'] as String?;

    return SyncProgressEvent(
      ruleId: json['rule_id'] as String,
      filePath: filePath,
      bytesProcessed: (json['bytes_processed'] as num).toInt(),
      chunksUploaded: (json['chunks_uploaded'] as num).toInt(),
      chunksDeduplicated: (json['chunks_deduplicated'] as num).toInt(),
      status: json['status'] as String,
      throughputBps: json['throughput_bps'] == null
          ? 0
          : (json['throughput_bps'] as num).toInt(),
      bitRotHealth: rawBitRot == null
          ? BitRotHealth.healthy
          : BitRotHealth.parseWireValue(rawBitRot),
      corruptedChunks: json['corrupted_chunks'] == null
          ? 0
          : (json['corrupted_chunks'] as num).toInt(),
    );
  }
}

/// Android 原生保活 MethodChannel 通道封装（ARCHITECTURE_CONTRACTS.md 第 5 节）。
class AndroidKeepAliveChannel {
  const AndroidKeepAliveChannel({
    MethodChannel channel = const MethodChannel(channelName),
  }) : _channel = channel;

  static const String channelName = 'com.lanvault.app/keep_alive';

  final MethodChannel _channel;

  Future<KeepAliveLevel> detectKeepAliveLevel() async {
    if (!Platform.isAndroid) {
      return KeepAliveLevel.winService;
    }
    final raw = await _channel.invokeMethod<String>('detectKeepAliveLevel');
    if (raw == null || raw.isEmpty) {
      throw StateError('detectKeepAliveLevel returned empty response.');
    }
    return KeepAliveLevel.parseWireValue(raw);
  }

  Future<bool> startKeepAliveServer({
    required String vaultPath,
    required int quotaGb,
    required int port,
    required String webPassword,
  }) async {
    if (!Platform.isAndroid) return true;
    final started = await _channel.invokeMethod<bool>(
      'startKeepAliveServer',
      <String, Object>{
        'vaultPath': vaultPath,
        'quotaGb': quotaGb,
        'port': port,
        'webPassword': webPassword,
      },
    );
    if (started == null) {
      throw StateError('startKeepAliveServer returned null.');
    }
    return started;
  }

  Future<bool> stopKeepAliveServer() async {
    if (!Platform.isAndroid) return true;
    final stopped = await _channel.invokeMethod<bool>('stopKeepAliveServer');
    if (stopped == null) {
      throw StateError('stopKeepAliveServer returned null.');
    }
    return stopped;
  }

  Future<bool> gracefulRestartServer() async {
    if (!Platform.isAndroid) return true;
    final ok = await _channel.invokeMethod<bool>('gracefulRestartServer');
    if (ok == null) {
      throw StateError('gracefulRestartServer returned null.');
    }
    return ok;
  }

  Future<bool> requestBatteryExemption() async {
    if (!Platform.isAndroid) return true;
    final granted = await _channel.invokeMethod<bool>('requestBatteryExemption');
    if (granted == null) {
      throw StateError('requestBatteryExemption returned null.');
    }
    return granted;
  }

  Future<bool> requestStoragePermission() async {
    if (!Platform.isAndroid) return true;
    final granted = await _channel.invokeMethod<bool>('requestStoragePermission');
    if (granted == null) {
      throw StateError('requestStoragePermission returned null.');
    }
    return granted;
  }

  Future<({bool onWifi, String? currentSsid, bool isCharging})>
      getDeviceEnvironment() async {
    if (!Platform.isAndroid) {
      return (onWifi: true, currentSsid: null, isCharging: true);
    }
    final raw = await _channel.invokeMapMethod<String, Object?>(
      'getDeviceEnvironment',
    );
    if (raw == null) {
      throw StateError('getDeviceEnvironment returned null.');
    }
    return (
      onWifi: raw['onWifi'] as bool? ?? false,
      currentSsid: raw['currentSsid'] as String?,
      isCharging: raw['isCharging'] as bool? ?? false,
    );
  }

  Future<({String vaultPath, int quotaGb, int port, String webPassword})?>
      getLastServerConfig() async {
    if (!Platform.isAndroid) return null;
    final raw = await _channel.invokeMapMethod<String, Object?>(
      'getLastServerConfig',
    );
    if (raw == null) return null;
    return (
      vaultPath: raw['vaultPath'] as String,
      quotaGb: (raw['quotaGb'] as num).toInt(),
      port: (raw['port'] as num).toInt(),
      webPassword: raw['webPassword'] as String,
    );
  }
}

/// 严格对齐 `crates/lanvault-ffi/src/api.rs` 导出函数的抽象契约。
/// 持久化备份规则模型（对应 SQLite `sync_rules` 表）。
/// 统一 Debug 与性能探针日志条目（聚合 Rust 原生错误/Panic、探针耗时与前端异常）。
class DebugLogEntryModel {
  const DebugLogEntryModel({
    required this.seq,
    required this.timestampMs,
    required this.level,
    required this.source,
    this.probe,
    this.durationUs,
    required this.message,
    this.detail,
  });

  final int seq;
  final int timestampMs;
  final String level;
  final String source;
  final String? probe;
  final int? durationUs;
  final String message;
  final String? detail;

  factory DebugLogEntryModel.fromJson(Map<String, dynamic> json) {
    return DebugLogEntryModel(
      seq: (json['seq'] as num).toInt(),
      timestampMs: (json['timestamp_ms'] as num).toInt(),
      level: json['level'] as String,
      source: json['source'] as String,
      probe: json['probe'] as String?,
      durationUs: json['duration_us'] == null ? null : (json['duration_us'] as num).toInt(),
      message: json['message'] as String,
      detail: json['detail'] as String?,
    );
  }
}

class SyncRuleModel {
  const SyncRuleModel({
    required this.ruleId,
    required this.localDir,
    required this.targetVaultId,
    required this.enabled,
  });

  final String ruleId;
  final String localDir;
  final String targetVaultId;
  final bool enabled;

  factory SyncRuleModel.fromJson(Map<String, dynamic> json) {
    return SyncRuleModel(
      ruleId: json['rule_id'] as String,
      localDir: json['local_dir'] as String,
      targetVaultId: json['target_vault_id'] as String,
      enabled: json['enabled'] as bool,
    );
  }
}

abstract class LanVaultRustFfi {
  Stream<DiscoveredPeer> startMdnsScan();
  Future<String> pairWithPin({
    required String ip,
    required int port,
    required String pin,
  });
  Future<void> enableAutoReconnect(bool enabled);
  Future<int> startLocalServer({
    required String vaultPath,
    required int quotaGb,
    required String webPassword,
    int? port,
  });
  Future<void> prepareGracefulRestart();
  Stream<SyncProgressEvent> subscribeSyncProgress();
  Future<void> triggerIncrementalScan(String ruleId);
  Future<void> updatePowerNetworkPolicy({
    required bool wifiOnly,
    required List<String> allowedSsids,
    required bool requireCharging,
    required int maxKbps,
  });
  Future<void> updateDeviceEnvironment({
    required bool onWifi,
    String? currentSsid,
    required bool isCharging,
  });
  Future<void> pushClipboardText({
    String? targetDeviceId,
    required String text,
  });
  Future<RoleMode> getRoleMode();
  Future<RoleMode> setRoleMode(RoleMode mode);
  Future<List<SyncRuleModel>> listSyncRules();
  Future<SyncRuleModel> upsertSyncRule({
    required String ruleId,
    required String localDir,
    String targetVaultId = 'default',
    bool enabled = true,
  });
  Future<bool> deleteSyncRule(String ruleId);
  Future<String> exportVaultKeyBackup(String passphrase);
  Future<void> importVaultKeyBackup(String passphrase, String backupJson);
  Future<bool> setDebugMode(bool enabled);
  Future<bool> getDebugMode();
  Future<List<DebugLogEntryModel>> getDebugLogs({int limit = 200});
  Future<void> clearDebugLogs();
  Future<void> reportClientLog({
    required String level,
    required String source,
    required String message,
    String? detail,
  });
}

// --- Native C-ABI 符号签名，与 flutter_rust_bridge / lanvault_ffi 共享库对齐 ---
typedef _CInvokeJsonNative = ffi.Pointer<Utf8> Function(
  ffi.Pointer<Utf8> fnName,
  ffi.Pointer<Utf8> payloadJson,
);
typedef _CInvokeJsonDart = ffi.Pointer<Utf8> Function(
  ffi.Pointer<Utf8> fnName,
  ffi.Pointer<Utf8> payloadJson,
);
typedef _CFreeStringNative = ffi.Void Function(ffi.Pointer<Utf8> ptr);
typedef _CFreeStringDart = void Function(ffi.Pointer<Utf8> ptr);

/// 基于 `liblanvault_ffi` 动态库（Windows: `lanvault_ffi.dll`, Android: `liblanvault_ffi.so`）
/// 的 FFI 绑定实现；在边界解码 JSON 响应并立即将 Rust 错误向上抛出。
ffi.DynamicLibrary _openLanVaultLibrary() {
  if (Platform.isWindows) {
    return ffi.DynamicLibrary.open('lanvault_ffi.dll');
  }
  if (Platform.isAndroid || Platform.isLinux) {
    return ffi.DynamicLibrary.open('liblanvault_ffi.so');
  }
  if (Platform.isMacOS) {
    return ffi.DynamicLibrary.open('liblanvault_ffi.dylib');
  }
  throw UnsupportedError('Unsupported platform: ${Platform.operatingSystem}');
}

/// 在后台 Worker Isolate 执行阻塞型 Rust C-ABI 调用，彻底避免阻塞 120Hz Flutter UI 主线程。
Map<String, dynamic> _invokeJsonInWorker((String, String) args) {
  final (fnName, payloadJson) = args;
  final lib = _openLanVaultLibrary();
  final invokeJson = lib.lookupFunction<_CInvokeJsonNative, _CInvokeJsonDart>(
    'lanvault_ffi_invoke_json',
  );
  final freeString = lib.lookupFunction<_CFreeStringNative, _CFreeStringDart>(
    'lanvault_ffi_free_string',
  );
  final fnPtr = fnName.toNativeUtf8();
  final payloadPtr = payloadJson.toNativeUtf8();
  ffi.Pointer<Utf8>? resultPtr;
  try {
    resultPtr = invokeJson(fnPtr, payloadPtr);
    final raw = resultPtr.toDartString();
    final decoded = jsonDecode(raw) as Map<String, dynamic>;
    if (decoded['ok'] != true) {
      throw StateError('FFI $fnName failed: ${decoded['error']}');
    }
    return decoded;
  } finally {
    calloc.free(fnPtr);
    calloc.free(payloadPtr);
    if (resultPtr != null && resultPtr != ffi.nullptr) {
      freeString(resultPtr);
    }
  }
}

class NativeLanVaultRustFfi implements LanVaultRustFfi {
  NativeLanVaultRustFfi() : _lib = _openLibrary() {
    _invokeJson = _lib.lookupFunction<_CInvokeJsonNative, _CInvokeJsonDart>(
      'lanvault_ffi_invoke_json',
    );
    _freeString = _lib.lookupFunction<_CFreeStringNative, _CFreeStringDart>(
      'lanvault_ffi_free_string',
    );
  }

  final ffi.DynamicLibrary _lib;
  late final _CInvokeJsonDart _invokeJson;
  late final _CFreeStringDart _freeString;

  static ffi.DynamicLibrary _openLibrary() {
    if (Platform.isWindows) {
      return ffi.DynamicLibrary.open('lanvault_ffi.dll');
    }
    if (Platform.isAndroid || Platform.isLinux) {
      return ffi.DynamicLibrary.open('liblanvault_ffi.so');
    }
    if (Platform.isMacOS) {
      return ffi.DynamicLibrary.open('liblanvault_ffi.dylib');
    }
    throw UnsupportedError('Unsupported platform: ${Platform.operatingSystem}');
  }

  Map<String, dynamic> _callSync(String fnName, Map<String, Object?> payload) {
    final fnPtr = fnName.toNativeUtf8();
    final payloadPtr = jsonEncode(payload).toNativeUtf8();
    ffi.Pointer<Utf8>? resultPtr;
    try {
      resultPtr = _invokeJson(fnPtr, payloadPtr);
      final raw = resultPtr.toDartString();
      final decoded = jsonDecode(raw) as Map<String, dynamic>;
      if (decoded['ok'] != true) {
        throw StateError('FFI $fnName failed: ${decoded['error']}');
      }
      return decoded;
    } finally {
      calloc.free(fnPtr);
      calloc.free(payloadPtr);
      if (resultPtr != null && resultPtr != ffi.nullptr) {
        _freeString(resultPtr);
      }
    }
  }

  Future<Map<String, dynamic>> _callAsync(
    String fnName,
    Map<String, Object?> payload,
  ) {
    final encoded = jsonEncode(payload);
    return Isolate.run(() => _invokeJsonInWorker((fnName, encoded)));
  }

  @override
  Stream<DiscoveredPeer> startMdnsScan() {
    return Stream<List<DiscoveredPeer>>.periodic(
      const Duration(seconds: 2),
      (_) {
        final resp = _callSync('start_mdns_scan', const <String, Object?>{});
        final list = resp['peers'] as List<dynamic>;
        return list
            .map((e) => DiscoveredPeer.fromJson(e as Map<String, dynamic>))
            .toList(growable: false);
      },
    ).expand((peers) => peers);
  }

  @override
  Future<String> pairWithPin({
    required String ip,
    required int port,
    required String pin,
  }) async {
    final resp = await _callAsync('pair_with_pin', <String, Object?>{
      'ip': ip,
      'port': port,
      'pin': pin,
    });
    return resp['device_id'] as String;
  }

  @override
  Future<void> enableAutoReconnect(bool enabled) async {
    await _callAsync('enable_auto_reconnect', <String, Object?>{'enabled': enabled});
  }

  @override
  Future<int> startLocalServer({
    required String vaultPath,
    required int quotaGb,
    required String webPassword,
    int? port,
  }) async {
    final resp = await _callAsync('start_local_server', <String, Object?>{
      'vault_path': vaultPath,
      'quota_gb': quotaGb,
      'web_password': webPassword,
      'port': port,
    });
    return (resp['port'] as num).toInt();
  }

  @override
  Future<void> prepareGracefulRestart() async {
    await _callAsync('prepare_graceful_restart', const <String, Object?>{});
  }

  @override
  Stream<SyncProgressEvent> subscribeSyncProgress() {
    return Stream<SyncProgressEvent>.periodic(
      const Duration(seconds: 1),
      (_) {
        final resp = _callSync(
          'subscribe_sync_progress',
          const <String, Object?>{},
        );
        return SyncProgressEvent.fromJson(resp['event'] as Map<String, dynamic>);
      },
    );
  }

  @override
  Future<void> triggerIncrementalScan(String ruleId) async {
    await _callAsync('trigger_incremental_scan', <String, Object?>{'rule_id': ruleId});
  }

  @override
  Future<void> updatePowerNetworkPolicy({
    required bool wifiOnly,
    required List<String> allowedSsids,
    required bool requireCharging,
    required int maxKbps,
  }) async {
    await _callAsync('update_power_network_policy', <String, Object?>{
      'wifi_only': wifiOnly,
      'allowed_ssids': allowedSsids,
      'require_charging': requireCharging,
      'max_kbps': maxKbps,
    });
  }

  @override
  Future<void> updateDeviceEnvironment({
    required bool onWifi,
    String? currentSsid,
    required bool isCharging,
  }) async {
    await _callAsync('update_device_environment', <String, Object?>{
      'on_wifi': onWifi,
      'current_ssid': currentSsid,
      'is_charging': isCharging,
    });
  }

  @override
  Future<void> pushClipboardText({
    String? targetDeviceId,
    required String text,
  }) async {
    await _callAsync('push_clipboard_text', <String, Object?>{
      'target_device_id': targetDeviceId,
      'text': text,
    });
  }

  @override
  Future<RoleMode> getRoleMode() async {
    final resp = await _callAsync('get_role_mode', const <String, Object?>{});
    return RoleMode.fromWire(resp['role_mode'] as String);
  }

  @override
  Future<RoleMode> setRoleMode(RoleMode mode) async {
    final resp = await _callAsync('set_role_mode', <String, Object?>{
      'mode': mode.wireValue,
    });
    return RoleMode.fromWire(resp['role_mode'] as String);
  }

  @override
  Future<List<SyncRuleModel>> listSyncRules() async {
    final resp = await _callAsync('list_sync_rules', const <String, Object?>{});
    final list = resp['rules'] as List<dynamic>;
    return list
        .map((e) => SyncRuleModel.fromJson(e as Map<String, dynamic>))
        .toList(growable: false);
  }

  @override
  Future<SyncRuleModel> upsertSyncRule({
    required String ruleId,
    required String localDir,
    String targetVaultId = 'default',
    bool enabled = true,
  }) async {
    final resp = await _callAsync('upsert_sync_rule', <String, Object?>{
      'rule_id': ruleId,
      'local_dir': localDir,
      'target_vault_id': targetVaultId,
      'enabled': enabled,
    });
    return SyncRuleModel.fromJson(resp['rule'] as Map<String, dynamic>);
  }

  @override
  Future<bool> deleteSyncRule(String ruleId) async {
    final resp = await _callAsync('delete_sync_rule', <String, Object?>{
      'rule_id': ruleId,
    });
    return resp['deleted'] as bool;
  }

  @override
  Future<String> exportVaultKeyBackup(String passphrase) async {
    final resp = await _callAsync('export_vault_key_backup', <String, Object?>{
      'passphrase': passphrase,
    });
    return resp['backup'] as String;
  }

  @override
  Future<void> importVaultKeyBackup(
    String passphrase,
    String backupJson,
  ) async {
    await _callAsync('import_vault_key_backup', <String, Object?>{
      'passphrase': passphrase,
      'backup_json': backupJson,
    });
  }

  @override
  Future<bool> setDebugMode(bool enabled) async {
    final resp = await _callAsync('set_debug_mode', <String, Object?>{
      'enabled': enabled,
    });
    return resp['debug_enabled'] as bool;
  }

  @override
  Future<bool> getDebugMode() async {
    final resp = await _callAsync('get_debug_mode', const <String, Object?>{});
    return resp['debug_enabled'] as bool;
  }

  @override
  Future<List<DebugLogEntryModel>> getDebugLogs({int limit = 200}) async {
    final resp = await _callAsync('get_debug_logs', <String, Object?>{
      'limit': limit,
    });
    final list = resp['entries'] as List<dynamic>;
    return list
        .map((e) => DebugLogEntryModel.fromJson(e as Map<String, dynamic>))
        .toList(growable: false);
  }

  @override
  Future<void> clearDebugLogs() async {
    await _callAsync('clear_debug_logs', const <String, Object?>{});
  }

  @override
  Future<void> reportClientLog({
    required String level,
    required String source,
    required String message,
    String? detail,
  }) async {
    await _callAsync('report_client_log', <String, Object?>{
      'level': level,
      'source': source,
      'message': message,
      'detail': detail,
    });
  }
}

/// 统一桥接门面：协调 Rust FFI 与 Android MethodChannel，
/// 并通过 [roleModeNotifier] 暴露全局单一真相源的运行模式状态。
class LanVaultBridge {
  LanVaultBridge({
    LanVaultRustFfi? rustFfi,
    AndroidKeepAliveChannel? keepAliveChannel,
    RoleMode initialRoleMode = RoleMode.dual,
  })  : _rustFfi = rustFfi,
        _keepAliveChannel = keepAliveChannel ?? const AndroidKeepAliveChannel(),
        roleModeNotifier = ValueNotifier<RoleMode>(initialRoleMode) {
    _environmentTimer = Timer.periodic(
      const Duration(seconds: 15),
      (_) => _refreshDeviceEnvironment(),
    );
    unawaited(_refreshDeviceEnvironment());
  }

  static const int defaultServerPort = 7842;

  LanVaultRustFfi? _rustFfi;
  final AndroidKeepAliveChannel _keepAliveChannel;
  Timer? _environmentTimer;
  ({String vaultPath, int quotaGb, String webPassword, int port, bool elevated})?
      _lastServerConfig;

  /// 运行模式单一真相源，供 Shell 导航栏与 Dashboard 实时联动。
  final ValueNotifier<RoleMode> roleModeNotifier;

  RoleMode get currentRoleMode => roleModeNotifier.value;

  LanVaultRustFfi get _ffi => _rustFfi ??= NativeLanVaultRustFfi();

  void dispose() {
    _environmentTimer?.cancel();
    roleModeNotifier.dispose();
  }

  Future<void> _refreshDeviceEnvironment() async {
    try {
      final environment = await _keepAliveChannel.getDeviceEnvironment();
      await _ffi.updateDeviceEnvironment(
        onWifi: environment.onWifi,
        currentSsid: environment.currentSsid,
        isCharging: environment.isCharging,
      );
    } catch (_) {
      // A subsequent periodic refresh retries transient platform failures.
    }
  }

  // --- 运行模式管理（三态切换） ---

  Future<RoleMode> getRoleMode() async {
    final mode = await _ffi.getRoleMode();
    roleModeNotifier.value = mode;
    return mode;
  }

  Future<RoleMode> setRoleMode(RoleMode mode) async {
    final previous = roleModeNotifier.value;
    final applied = await _ffi.setRoleMode(mode);

    if (Platform.isAndroid && previous.hasServer && !applied.hasServer) {
      final stopped = await _keepAliveChannel.stopKeepAliveServer();
      if (!stopped) {
        final config = _lastServerConfig;
        if (config != null && !config.elevated) {
          await _ffi.startLocalServer(
            vaultPath: config.vaultPath,
            quotaGb: config.quotaGb,
            webPassword: config.webPassword,
            port: config.port,
          );
        }
        final reverted = await _ffi.setRoleMode(previous);
        roleModeNotifier.value = reverted;
        throw StateError('后台服务未能停止，运行模式已回滚到 ${reverted.wireValue}。');
      }
    }

    roleModeNotifier.value = applied;
    return applied;
  }

  // --- 1..9: ARCHITECTURE_CONTRACTS.md 第 4 节 FFI 接口 ---

  Stream<DiscoveredPeer> startMdnsScan() => _ffi.startMdnsScan();

  Future<String> pairWithPin({
    required String ip,
    required int port,
    required String pin,
  }) {
    if (!RegExp(r'^\d{6}$').hasMatch(pin)) {
      throw ArgumentError.value(pin, 'pin', 'PIN 必须为 6 位数字');
    }
    if (port <= 0 || port > 65535) {
      throw RangeError.range(port, 1, 65535, 'port');
    }
    return _ffi.pairWithPin(ip: ip, port: port, pin: pin);
  }

  Future<void> enableAutoReconnect(bool enabled) =>
      _ffi.enableAutoReconnect(enabled);

  Future<int> startLocalServer({
    required String vaultPath,
    required int quotaGb,
    required String webPassword,
    int port = defaultServerPort,
  }) async {
    final trimmedPath = vaultPath.trim();
    if (trimmedPath.isEmpty) {
      throw ArgumentError.value(vaultPath, 'vaultPath', '存储路径不能为空');
    }
    if (quotaGb <= 0) {
      throw RangeError.value(quotaGb, 'quotaGb', '存储配额必须大于 0 GB');
    }
    if (webPassword.length < 8) {
      throw ArgumentError.value(webPassword, 'webPassword', 'Web 管理密码至少 8 位');
    }
    final environment = await _keepAliveChannel.getDeviceEnvironment();
    await _ffi.updateDeviceEnvironment(
      onWifi: environment.onWifi,
      currentSsid: environment.currentSsid,
      isCharging: environment.isCharging,
    );

    if (Platform.isAndroid) {
      final level = await _keepAliveChannel.detectKeepAliveLevel();
      if (level.isElevatedAndroidDaemon) {
        final started = await _keepAliveChannel.startKeepAliveServer(
          vaultPath: trimmedPath,
          quotaGb: quotaGb,
          port: port,
          webPassword: webPassword,
        );
        if (!started) {
          throw StateError('提权守护进程启动失败（${level.wireValue}）');
        }
        _lastServerConfig = (
          vaultPath: trimmedPath,
          quotaGb: quotaGb,
          webPassword: webPassword,
          port: port,
          elevated: true,
        );
        return port;
      }

      final boundPort = await _ffi.startLocalServer(
        vaultPath: trimmedPath,
        quotaGb: quotaGb,
        webPassword: webPassword,
        port: port,
      );
      await _keepAliveChannel.startKeepAliveServer(
        vaultPath: trimmedPath,
        quotaGb: quotaGb,
        port: boundPort,
        webPassword: webPassword,
      );
      _lastServerConfig = (
        vaultPath: trimmedPath,
        quotaGb: quotaGb,
        webPassword: webPassword,
        port: boundPort,
        elevated: false,
      );
      return boundPort;
    }

    final boundPort = await _ffi.startLocalServer(
      vaultPath: trimmedPath,
      quotaGb: quotaGb,
      webPassword: webPassword,
      port: port,
    );
    _lastServerConfig = (
      vaultPath: trimmedPath,
      quotaGb: quotaGb,
      webPassword: webPassword,
      port: boundPort,
      elevated: false,
    );
    return boundPort;
  }

  Future<void> prepareGracefulRestart() => _ffi.prepareGracefulRestart();

  Stream<SyncProgressEvent> subscribeSyncProgress() =>
      _ffi.subscribeSyncProgress();

  Future<void> triggerIncrementalScan(String ruleId) {
    if (ruleId.trim().isEmpty) {
      throw ArgumentError.value(ruleId, 'ruleId', '规则 ID 不能为空');
    }
    return _ffi.triggerIncrementalScan(ruleId.trim());
  }

  Future<List<SyncRuleModel>> listSyncRules() => _ffi.listSyncRules();

  Future<SyncRuleModel> upsertSyncRule({
    required String ruleId,
    required String localDir,
    String targetVaultId = 'default',
    bool enabled = true,
  }) {
    if (ruleId.trim().isEmpty || localDir.trim().isEmpty) {
      throw ArgumentError('规则 ID 与本地目录均不能为空');
    }
    return _ffi.upsertSyncRule(
      ruleId: ruleId.trim(),
      localDir: localDir.trim(),
      targetVaultId: targetVaultId.trim().isEmpty ? 'default' : targetVaultId.trim(),
      enabled: enabled,
    );
  }

  Future<bool> deleteSyncRule(String ruleId) {
    if (ruleId.trim().isEmpty) {
      throw ArgumentError.value(ruleId, 'ruleId', '规则 ID 不能为空');
    }
    return _ffi.deleteSyncRule(ruleId.trim());
  }

  Future<String> exportVaultKeyBackup(String passphrase) {
    if (passphrase.length < 12) {
      throw ArgumentError.value(passphrase, 'passphrase', '密钥备份口令至少 12 位');
    }
    return _ffi.exportVaultKeyBackup(passphrase);
  }

  Future<void> importVaultKeyBackup(String passphrase, String backupJson) {
    if (passphrase.length < 12 || backupJson.trim().isEmpty) {
      throw ArgumentError('密钥备份口令或备份内容无效');
    }
    return _ffi.importVaultKeyBackup(passphrase, backupJson.trim());
  }

  Future<void> updatePowerNetworkPolicy({
    required bool wifiOnly,
    required List<String> allowedSsids,
    required bool requireCharging,
    required int maxKbps,
  }) async {
    if (maxKbps < 0) {
      throw RangeError.value(maxKbps, 'maxKbps', '限速值不能为负数');
    }
    final environment = await _keepAliveChannel.getDeviceEnvironment();
    await _ffi.updateDeviceEnvironment(
      onWifi: environment.onWifi,
      currentSsid: environment.currentSsid,
      isCharging: environment.isCharging,
    );
    await _ffi.updatePowerNetworkPolicy(
      wifiOnly: wifiOnly,
      allowedSsids: allowedSsids,
      requireCharging: requireCharging,
      maxKbps: maxKbps,
    );
  }

  Future<void> pushClipboardText({
    String? targetDeviceId,
    required String text,
  }) {
    if (text.isEmpty) {
      throw ArgumentError.value(text, 'text', '剪贴板内容不能为空');
    }
    return _ffi.pushClipboardText(
      targetDeviceId: targetDeviceId,
      text: text,
    );
  }

  // --- ARCHITECTURE_CONTRACTS.md 第 5 节 Android 保活通道 ---

  Future<KeepAliveLevel> detectKeepAliveLevel() =>
      _keepAliveChannel.detectKeepAliveLevel();

  Future<bool> stopKeepAliveServer() async {
    final config = _lastServerConfig;
    if (config != null && !config.elevated) {
      await _ffi.prepareGracefulRestart();
    }
    final stopped = await _keepAliveChannel.stopKeepAliveServer();
    if (stopped) {
      _lastServerConfig = null;
    }
    return stopped;
  }

  /// 完整优雅重启流程：先通知 Rust 停接新任务与 WAL 刷盘，再通过 Android 提权通道拉起。
  Future<bool> executeGracefulRestart() async {
    var config = _lastServerConfig;
    if (config == null && Platform.isAndroid) {
      final saved = await _keepAliveChannel.getLastServerConfig();
      final level = await _keepAliveChannel.detectKeepAliveLevel();
      if (saved != null) {
        config = (
          vaultPath: saved.vaultPath,
          quotaGb: saved.quotaGb,
          webPassword: saved.webPassword,
          port: saved.port,
          elevated: level.isElevatedAndroidDaemon,
        );
      }
    }
    if (config == null) {
      throw StateError('服务器尚未启动，无法执行优雅重启。');
    }
    if (config.elevated) {
      return _keepAliveChannel.gracefulRestartServer();
    }
    await _ffi.prepareGracefulRestart();
    final boundPort = await _ffi.startLocalServer(
      vaultPath: config.vaultPath,
      quotaGb: config.quotaGb,
      webPassword: config.webPassword,
      port: config.port,
    );
    _lastServerConfig = (
      vaultPath: config.vaultPath,
      quotaGb: config.quotaGb,
      webPassword: config.webPassword,
      port: boundPort,
      elevated: false,
    );
    return true;
  }

  Future<bool> requestBatteryExemption() =>
      _keepAliveChannel.requestBatteryExemption();

  Future<bool> requestStoragePermission() =>
      _keepAliveChannel.requestStoragePermission();

  // --- 统一 Debug 开关、性能探针日志与前端异常上报 ---

  Future<bool> setDebugMode(bool enabled) => _ffi.setDebugMode(enabled);
  Future<bool> getDebugMode() => _ffi.getDebugMode();
  Future<List<DebugLogEntryModel>> getDebugLogs({int limit = 200}) =>
      _ffi.getDebugLogs(limit: limit);
  Future<void> clearDebugLogs() => _ffi.clearDebugLogs();
  Future<void> reportClientLog({
    required String level,
    required String source,
    required String message,
    String? detail,
  }) =>
      _ffi.reportClientLog(
        level: level,
        source: source,
        message: message,
        detail: detail,
      );
}
