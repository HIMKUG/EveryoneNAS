// 参考依据:
// - ARCHITECTURE_CONTRACTS.md 第 4 节 (FFI 接口及 DTO)
// - ARCHITECTURE_CONTRACTS.md 第 5 节 (Android com.lanvault.app/keep_alive MethodChannel)
// - 严格遵循零过度防御原则：仅在 FFI / MethodChannel 外部边界解析与校验，内部直接传递强类型并向上抛出错误。

import 'dart:async';
import 'dart:convert';
import 'dart:ffi' as ffi;
import 'dart:io';
import 'dart:isolate';

import 'package:ffi/ffi.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'system_env_helper.dart';

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
  winService('WIN_SERVICE', 'Windows 桌面 / SCM 服务');

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
  healthy('healthy', '系统正常'),
  degraded('degraded', '状态降级'),
  corrupted('corrupted', '发现损坏块'),
  unknown('unknown', '未运行');

  const BitRotHealth(this.wireValue, this.displayLabel);

  final String wireValue;
  final String displayLabel;

  static BitRotHealth parseWireValue(String raw) {
    final normalized = raw.trim().toLowerCase();
    for (final item in BitRotHealth.values) {
      if (item.wireValue == normalized) return item;
    }
    if (normalized == 'stopped' || normalized == 'not_running') {
      return BitRotHealth.unknown;
    }
    throw FormatException('Unsupported bit_rot_health wire value: $raw');
  }
}

/// 对应 Rust `DiscoveredPeer` DTO。
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

/// 对应 Rust `SyncProgressEvent` DTO。
@immutable
class SyncProgressEvent {
  const SyncProgressEvent({
    required this.ruleId,
    required this.filePath,
    required this.bytesProcessed,
    required this.totalBytes,
    required this.chunksUploaded,
    required this.chunksDeduplicated,
    required this.status,
    this.throughputBps = 0,
    this.bitRotHealth,
    this.corruptedChunks = 0,
  });

  final String ruleId;
  final String filePath;
  final int bytesProcessed;
  final int totalBytes;
  final int chunksUploaded;
  final int chunksDeduplicated;
  final String status;
  final int throughputBps;
  final BitRotHealth? bitRotHealth;
  final int corruptedChunks;

  int get totalChunks => chunksUploaded + chunksDeduplicated;

  double get dedupRatio => totalChunks <= 0
      ? 0.0
      : (chunksDeduplicated / totalChunks).clamp(0.0, 1.0);

  double get progressRatio =>
      totalBytes <= 0 ? 0.0 : (bytesProcessed / totalBytes).clamp(0.0, 1.0);

  factory SyncProgressEvent.fromJson(Map<String, dynamic> json) {
    final rawBitRot = json['bit_rot_health'] as String?;

    return SyncProgressEvent(
      ruleId: json['rule_id'] as String,
      filePath: json['file_path'] as String,
      bytesProcessed: (json['bytes_processed'] as num).toInt(),
      totalBytes: json['total_bytes'] == null
          ? 0
          : (json['total_bytes'] as num).toInt(),
      chunksUploaded: (json['chunks_uploaded'] as num).toInt(),
      chunksDeduplicated: (json['chunks_deduplicated'] as num).toInt(),
      status: json['status'] as String,
      throughputBps: json['throughput_bps'] == null
          ? 0
          : (json['throughput_bps'] as num).toInt(),
      bitRotHealth: rawBitRot == null || rawBitRot.trim().isEmpty
          ? null
          : BitRotHealth.parseWireValue(rawBitRot),
      corruptedChunks: json['corrupted_chunks'] == null
          ? 0
          : (json['corrupted_chunks'] as num).toInt(),
    );
  }
}

/// 上次启动服务端配置快照。
@immutable
class LastServerConfig {
  const LastServerConfig({
    required this.vaultPath,
    required this.quotaGb,
    required this.port,
    this.webPassword,
  });

  final String vaultPath;
  final int quotaGb;
  final int port;
  final String? webPassword;

  factory LastServerConfig.fromRustJson(Map<String, dynamic> json) {
    return LastServerConfig(
      vaultPath: json['vault_path'] as String,
      quotaGb: (json['quota_gb'] as num).toInt(),
      port: (json['port'] as num).toInt(),
      webPassword: json['web_password'] as String?,
    );
  }
}

/// 最近系统活动条目（对齐 `get_system_overview.recent_activities`）。
@immutable
class RecentActivityModel {
  const RecentActivityModel({
    required this.id,
    required this.title,
    required this.detail,
    required this.kind,
    required this.timestamp,
  });

  final String id;
  final String title;
  final String detail;
  final String kind;
  final String timestamp;

  factory RecentActivityModel.fromJson(Map<String, dynamic> json) {
    return RecentActivityModel(
      id: json['id'] as String,
      title: json['title'] as String,
      detail: json['detail'] as String,
      kind: json['kind'] as String,
      timestamp: json['timestamp'].toString(),
    );
  }
}

/// 快照恢复结果模型（对齐 `restore_snapshot`）。
@immutable
class RestoreSnapshotResultModel {
  const RestoreSnapshotResultModel({
    required this.snapshotId,
    required this.restoredChunks,
    required this.restoredBytes,
    required this.outputPath,
  });

  final String snapshotId;
  final int restoredChunks;
  final int restoredBytes;
  final String outputPath;

  factory RestoreSnapshotResultModel.fromJson(Map<String, dynamic> json) {
    return RestoreSnapshotResultModel(
      snapshotId: json['snapshot_id'] as String,
      restoredChunks: (json['restored_chunks'] as num).toInt(),
      restoredBytes: (json['restored_bytes'] as num).toInt(),
      outputPath: json['output_path'] as String,
    );
  }
}

/// 持久化传输历史条目模型（对齐 `list_transfer_history`）。
@immutable
class TransferHistoryModel {
  const TransferHistoryModel({
    required this.id,
    required this.direction,
    required this.kind,
    required this.title,
    required this.subtitle,
    required this.peerName,
    this.content,
    required this.sizeBytes,
    required this.createdAtSecs,
  });

  final int id;
  final String direction;
  final String kind;
  final String title;
  final String subtitle;
  final String peerName;
  final String? content;
  final int sizeBytes;
  final int createdAtSecs;

  bool get isFile => kind == 'file';
  bool get isIncoming => direction == 'received';

  String get formattedTime {
    final dt = DateTime.fromMillisecondsSinceEpoch(createdAtSecs * 1000);
    return '${dt.month.toString().padLeft(2, '0')}-${dt.day.toString().padLeft(2, '0')} '
        '${dt.hour.toString().padLeft(2, '0')}:${dt.minute.toString().padLeft(2, '0')}';
  }

  factory TransferHistoryModel.fromJson(Map<String, dynamic> json) {
    return TransferHistoryModel(
      id: (json['id'] as num).toInt(),
      direction: json['direction'] as String,
      kind: json['kind'] as String,
      title: json['title'] as String,
      subtitle: json['subtitle'] as String,
      peerName: json['peer_name'] as String,
      content: json['content'] as String?,
      sizeBytes: (json['size_bytes'] as num).toInt(),
      createdAtSecs: (json['created_at'] as num).toInt(),
    );
  }
}

/// 全局系统总览模型（对齐 `get_system_overview`）。
@immutable
class SystemOverviewModel {
  const SystemOverviewModel({
    required this.serverRunning,
    required this.serverPort,
    required this.lanIp,
    required this.pairingPin,
    required this.pairingQrUri,
    required this.vaultUsedBytes,
    required this.vaultQuotaBytes,
    required this.vaultCount,
    required this.pairedPeerCount,
    required this.onlinePeerCount,
    required this.syncRuleCount,
    required this.bitRotHealth,
    required this.corruptedChunks,
    this.dedupRatio = 0.0,
    this.compressionRatio = 1.0,
    this.syncPaused = false,
    this.bitRotAutoScrub = true,
    required this.recentActivities,
  });

  final bool serverRunning;
  final int? serverPort;
  final String lanIp;
  final String pairingPin;
  final String pairingQrUri;
  final int vaultUsedBytes;
  final int vaultQuotaBytes;
  final int vaultCount;
  final int pairedPeerCount;
  final int onlinePeerCount;
  final int syncRuleCount;
  final BitRotHealth bitRotHealth;
  final int corruptedChunks;
  final double dedupRatio;
  final double compressionRatio;
  final bool syncPaused;
  final bool bitRotAutoScrub;
  final List<RecentActivityModel> recentActivities;

  double get storageRatio => vaultQuotaBytes <= 0
      ? 0.0
      : (vaultUsedBytes / vaultQuotaBytes).clamp(0.0, 1.0);

  factory SystemOverviewModel.fromJson(Map<String, dynamic> json) {
    final rawActivities = json['recent_activities'] as List<dynamic>;
    final rawHealth = json['bit_rot_health'] as String;
    return SystemOverviewModel(
      serverRunning: json['server_running'] as bool,
      serverPort: json['server_port'] == null
          ? null
          : (json['server_port'] as num).toInt(),
      lanIp: json['lan_ip'] as String,
      pairingPin: json['pairing_pin'] as String,
      pairingQrUri: json['pairing_qr_uri'] as String,
      vaultUsedBytes: (json['vault_used_bytes'] as num).toInt(),
      vaultQuotaBytes: (json['vault_quota_bytes'] as num).toInt(),
      vaultCount: (json['vault_count'] as num).toInt(),
      pairedPeerCount: (json['paired_peer_count'] as num).toInt(),
      onlinePeerCount: (json['online_peer_count'] as num).toInt(),
      syncRuleCount: (json['sync_rule_count'] as num).toInt(),
      bitRotHealth: BitRotHealth.parseWireValue(rawHealth),
      corruptedChunks: (json['corrupted_chunks'] as num).toInt(),
      dedupRatio: (json['dedup_ratio'] as num? ?? 0.0).toDouble(),
      compressionRatio: (json['compression_ratio'] as num? ?? 1.0).toDouble(),
      syncPaused: json['sync_paused'] as bool? ?? false,
      bitRotAutoScrub: json['bit_rot_auto_scrub'] as bool? ?? true,
      recentActivities: rawActivities
          .map((e) => RecentActivityModel.fromJson(e as Map<String, dynamic>))
          .toList(growable: false),
    );
  }
}

/// 存储池挂载卷模型（对齐 `list_vaults.vaults[].volumes[]`）。
@immutable
class VaultVolumeModel {
  const VaultVolumeModel({
    required this.volumeId,
    required this.mountPath,
    this.volumeGuid,
    required this.role,
    required this.isOnline,
  });

  final String volumeId;
  final String mountPath;
  final String? volumeGuid;
  final String role;
  final bool isOnline;

  factory VaultVolumeModel.fromJson(Map<String, dynamic> json) {
    return VaultVolumeModel(
      volumeId: json['volume_id'] as String,
      mountPath: json['mount_path'] as String,
      volumeGuid: json['volume_guid'] as String?,
      role: json['role'] as String,
      isOnline: json['is_online'] as bool,
    );
  }
}

/// 存储池摘要模型（对齐 `list_vaults` 与 `create_vault`）。
@immutable
class VaultSummaryModel {
  const VaultSummaryModel({
    required this.vaultId,
    required this.name,
    required this.poolMode,
    required this.quotaBytes,
    required this.usedBytes,
    required this.e2eeEnabled,
    required this.volumes,
  });

  final String vaultId;
  final String name;
  final String poolMode;
  final int quotaBytes;
  final int usedBytes;
  final bool e2eeEnabled;
  final List<VaultVolumeModel> volumes;

  double get usageRatio =>
      quotaBytes <= 0 ? 0.0 : (usedBytes / quotaBytes).clamp(0.0, 1.0);

  factory VaultSummaryModel.fromJson(Map<String, dynamic> json) {
    final rawVolumes = json['volumes'] as List<dynamic>;
    return VaultSummaryModel(
      vaultId: json['vault_id'] as String,
      name: json['name'] as String,
      poolMode: json['pool_mode'] as String,
      quotaBytes: (json['quota_bytes'] as num).toInt(),
      usedBytes: (json['used_bytes'] as num).toInt(),
      e2eeEnabled: json['e2ee_enabled'] as bool,
      volumes: rawVolumes
          .map((e) => VaultVolumeModel.fromJson(e as Map<String, dynamic>))
          .toList(growable: false),
    );
  }
}

/// 历史快照摘要模型（对齐 `list_snapshots`）。
@immutable
class SnapshotSummaryModel {
  const SnapshotSummaryModel({
    required this.snapshotId,
    required this.vaultId,
    required this.deviceId,
    required this.retentionTier,
    required this.fileCount,
    required this.totalBytes,
    required this.createdAt,
  });

  final String snapshotId;
  final String vaultId;
  final String deviceId;
  final String retentionTier;
  final int fileCount;
  final int totalBytes;
  final String createdAt;

  String get retentionLabel {
    return switch (retentionTier.toLowerCase()) {
      'daily' => '每日保留',
      'weekly' => '每周保留',
      'monthly' => '每月保留',
      'manual' => '手动快照',
      _ => retentionTier,
    };
  }

  factory SnapshotSummaryModel.fromJson(Map<String, dynamic> json) {
    return SnapshotSummaryModel(
      snapshotId: json['snapshot_id'] as String,
      vaultId: json['vault_id'] as String,
      deviceId: json['device_id'] as String,
      retentionTier: json['retention_tier'] as String,
      fileCount: (json['file_count'] as num).toInt(),
      totalBytes: (json['total_bytes'] as num).toInt(),
      createdAt: json['created_at'].toString(),
    );
  }
}

/// Bit-Rot 巡检结果模型（对齐 `trigger_bit_rot_scrub`）。
@immutable
class BitRotScrubResultModel {
  const BitRotScrubResultModel({
    required this.scrubId,
    required this.checkedChunks,
    required this.corruptedChunks,
    required this.healedChunks,
    required this.status,
  });

  final String scrubId;
  final int checkedChunks;
  final int corruptedChunks;
  final int healedChunks;
  final String status;

  factory BitRotScrubResultModel.fromJson(Map<String, dynamic> json) {
    return BitRotScrubResultModel(
      scrubId: json['scrub_id'] as String,
      checkedChunks: (json['checked_chunks'] as num).toInt(),
      corruptedChunks: (json['corrupted_chunks'] as num).toInt(),
      healedChunks: (json['healed_chunks'] as num).toInt(),
      status: json['status'] as String,
    );
  }
}

/// 本机配对信息模型（对齐 `get_pairing_info`）。
@immutable
class PairingInfoModel {
  const PairingInfoModel({
    required this.serverRunning,
    required this.lanIp,
    required this.port,
    required this.pairingPin,
    required this.pairingQrUri,
  });

  final bool serverRunning;
  final String lanIp;
  final int port;
  final String pairingPin;
  final String pairingQrUri;

  factory PairingInfoModel.fromJson(Map<String, dynamic> json) {
    return PairingInfoModel(
      serverRunning: json['server_running'] as bool,
      lanIp: json['lan_ip'] as String,
      port: (json['port'] as num).toInt(),
      pairingPin: json['pairing_pin'] as String,
      pairingQrUri: json['pairing_qr_uri'] as String,
    );
  }
}

/// 收到对端发起的入站配对请求（接收方弹窗直接同意或拒绝，无需手输 PIN）。
@immutable
class PendingPairRequestModel {
  const PendingPairRequestModel({
    required this.requestId,
    required this.deviceId,
    required this.deviceName,
    required this.osType,
    required this.fingerprint,
    required this.remoteIp,
    required this.createdAt,
    required this.expiresAt,
  });

  final String requestId;
  final String deviceId;
  final String deviceName;
  final String osType;
  final String fingerprint;
  final String remoteIp;
  final int createdAt;
  final int expiresAt;

  factory PendingPairRequestModel.fromJson(Map<String, dynamic> json) {
    return PendingPairRequestModel(
      requestId: json['request_id'] as String,
      deviceId: json['device_id'] as String,
      deviceName: json['device_name'] as String,
      osType: json['os_type'] as String,
      fingerprint: json['fingerprint'] as String? ?? '',
      remoteIp: json['remote_ip'] as String? ?? '',
      createdAt: (json['created_at'] as num?)?.toInt() ?? 0,
      expiresAt: (json['expires_at'] as num?)?.toInt() ?? 0,
    );
  }
}

/// Android 原生保活 MethodChannel 通道封装。
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
    bool skipNativeStart = false,
  }) async {
    if (!Platform.isAndroid) return true;
    final started = await _channel.invokeMethod<bool>(
      'startKeepAliveServer',
      <String, Object>{
        'vaultPath': vaultPath,
        'quotaGb': quotaGb,
        'port': port,
        'webPassword': webPassword,
        'skipNativeStart': skipNativeStart,
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
    final granted =
        await _channel.invokeMethod<bool>('requestBatteryExemption');
    if (granted == null) {
      throw StateError('requestBatteryExemption returned null.');
    }
    return granted;
  }

  Future<bool> requestStoragePermission() async {
    if (!Platform.isAndroid) return true;
    final granted =
        await _channel.invokeMethod<bool>('requestStoragePermission');
    if (granted == null) {
      throw StateError('requestStoragePermission returned null.');
    }
    return granted;
  }

  Future<bool> requestLocationPermission() async {
    if (!Platform.isAndroid) return true;
    final granted = await _channel.invokeMethod<bool>(
      'requestLocationPermission',
    );
    if (granted == null) {
      throw StateError('requestLocationPermission returned null.');
    }
    return granted;
  }

  Future<bool> requestNotificationPermission() async {
    if (!Platform.isAndroid) return true;
    final granted = await _channel.invokeMethod<bool>(
      'requestNotificationPermission',
    );
    if (granted == null) {
      throw StateError('requestNotificationPermission returned null.');
    }
    return granted;
  }

  Future<String?> pickDirectory() async {
    if (!Platform.isAndroid) return null;
    final picked = await _channel.invokeMethod<String>('pickDirectory');
    if (picked == null) return null;
    final trimmed = picked.trim();
    return trimmed.isEmpty ? null : trimmed;
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

  Future<LastServerConfig?> getLastServerConfig() async {
    if (!Platform.isAndroid) return null;
    final raw = await _channel.invokeMapMethod<String, Object?>(
      'getLastServerConfig',
    );
    if (raw == null) return null;
    return LastServerConfig(
      vaultPath: raw['vaultPath'] as String,
      quotaGb: (raw['quotaGb'] as num).toInt(),
      port: (raw['port'] as num).toInt(),
      webPassword: raw['webPassword'] as String?,
    );
  }
}

/// 统一 Debug 与性能探针日志条目。
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
      durationUs: json['duration_us'] == null
          ? null
          : (json['duration_us'] as num).toInt(),
      message: json['message'] as String,
      detail: json['detail'] as String?,
    );
  }
}

/// 持久化备份规则模型。
class SyncRuleModel {
  const SyncRuleModel({
    required this.ruleId,
    required this.localDir,
    required this.targetVaultId,
    required this.enabled,
    this.scheduleMode = 'realtime',
  });

  final String ruleId;
  final String localDir;
  final String targetVaultId;
  final bool enabled;
  final String scheduleMode;

  String get scheduleLabel {
    return switch (scheduleMode) {
      'daily' => '每日 · 增量',
      'manual' => '手动',
      _ => '实时 · 增量',
    };
  }

  SyncRuleModel copyWith({bool? enabled, String? scheduleMode}) {
    return SyncRuleModel(
      ruleId: ruleId,
      localDir: localDir,
      targetVaultId: targetVaultId,
      enabled: enabled ?? this.enabled,
      scheduleMode: scheduleMode ?? this.scheduleMode,
    );
  }

  factory SyncRuleModel.fromJson(Map<String, dynamic> json) {
    return SyncRuleModel(
      ruleId: json['rule_id'] as String,
      localDir: json['local_dir'] as String,
      targetVaultId: json['target_vault_id'] as String,
      enabled: json['enabled'] as bool,
      scheduleMode: json['schedule_mode'] as String? ?? 'realtime',
    );
  }
}

@immutable
class PairedPeerModel {
  const PairedPeerModel({
    required this.deviceId,
    required this.deviceName,
    required this.osType,
    this.ip,
    this.port,
    this.online = false,
    this.lastSeenSecs,
  });

  final String deviceId;
  final String deviceName;
  final String osType;
  final String? ip;
  final int? port;
  final bool online;
  final int? lastSeenSecs;

  String get presenceLabel {
    if (online) return '在线';
    final secs = lastSeenSecs;
    if (secs == null) return '离线';
    if (secs < 60) return '离线 · 刚刚';
    if (secs < 3600) return '离线 · ${secs ~/ 60} 分钟前';
    return '离线 · ${secs ~/ 3600} 小时前';
  }

  String get displayLabel {
    final name = deviceName.trim().isEmpty ? deviceId : deviceName.trim();
    if (ip != null && ip!.isNotEmpty) {
      return '$name · $ip';
    }
    return name;
  }

  factory PairedPeerModel.fromJson(Map<String, dynamic> json) {
    return PairedPeerModel(
      deviceId: json['device_id'] as String,
      deviceName: json['device_name'] as String,
      osType: json['os_type'] as String,
      ip: json['ip'] as String?,
      port: json['port'] == null ? null : (json['port'] as num).toInt(),
      online: json['online'] as bool? ?? false,
      lastSeenSecs: json['last_seen_secs'] == null
          ? null
          : (json['last_seen_secs'] as num).toInt(),
    );
  }
}

@immutable
class PowerNetworkPolicyModel {
  const PowerNetworkPolicyModel({
    required this.wifiOnly,
    required this.allowedSsids,
    required this.requireCharging,
    required this.maxKbps,
  });

  final bool wifiOnly;
  final List<String> allowedSsids;
  final bool requireCharging;
  final int maxKbps;

  factory PowerNetworkPolicyModel.fromJson(Map<String, dynamic> json) {
    final rawSsids =
        json['allowed_ssids'] as List<dynamic>? ?? const <dynamic>[];
    return PowerNetworkPolicyModel(
      wifiOnly: json['wifi_only'] as bool,
      allowedSsids: rawSsids.map((e) => e as String).toList(growable: false),
      requireCharging: json['require_charging'] as bool,
      maxKbps: (json['max_kbps'] as num).toInt(),
    );
  }
}

@immutable
class BackupStorageConfigModel {
  const BackupStorageConfigModel({
    required this.primaryDir,
    this.primaryRoot = '',
    this.fallbackDir,
    this.fallbackRoot,
  });

  /// 用户选择的目录（UI 中展示/编辑的路径）。
  final String primaryDir;

  /// 后端在所选目录下自动创建的隔离根 `<primaryDir>/everyonenasbackup`。
  final String primaryRoot;

  final String? fallbackDir;

  /// 备用盘的隔离根 `<fallbackDir>/everyonenasbackup`。
  final String? fallbackRoot;

  factory BackupStorageConfigModel.fromJson(Map<String, dynamic> json) {
    return BackupStorageConfigModel(
      primaryDir: json['primary_dir'] as String? ?? '',
      primaryRoot: json['primary_root'] as String? ?? '',
      fallbackDir: json['fallback_dir'] as String?,
      fallbackRoot: json['fallback_root'] as String?,
    );
  }
}

// =============================================================================
// v2 Models: Device Groups, Group Sync Rules, Catalog & Non-Blocking Outbox
// =============================================================================

/// 设备组记录（对齐 Rust `DeviceGroupRecord`）。
@immutable
class DeviceGroupModel {
  const DeviceGroupModel({
    required this.groupId,
    required this.name,
    required this.coordinatorDeviceId,
    required this.revision,
    required this.createdAt,
    required this.updatedAt,
    this.deletedAt,
  });

  final String groupId;
  final String name;
  final String coordinatorDeviceId;
  final int revision;
  final int createdAt;
  final int updatedAt;
  final int? deletedAt;

  factory DeviceGroupModel.fromJson(Map<String, dynamic> json) {
    return DeviceGroupModel(
      groupId: json['group_id'] as String,
      name: json['name'] as String,
      coordinatorDeviceId: json['coordinator_device_id'] as String,
      revision: (json['revision'] as num).toInt(),
      createdAt: (json['created_at'] as num).toInt(),
      updatedAt: (json['updated_at'] as num).toInt(),
      deletedAt: json['deleted_at'] == null
          ? null
          : (json['deleted_at'] as num).toInt(),
    );
  }
}

/// 设备组成员记录（对齐 Rust `DeviceGroupMemberRecord`）。
@immutable
class GroupMemberModel {
  const GroupMemberModel({
    required this.groupId,
    required this.deviceId,
    required this.role,
    required this.addedAt,
    this.removedAt,
  });

  final String groupId;
  final String deviceId;
  final String role;
  final int addedAt;
  final int? removedAt;

  bool get isActive => removedAt == null;
  bool get isCoordinator => role == 'coordinator';

  factory GroupMemberModel.fromJson(Map<String, dynamic> json) {
    return GroupMemberModel(
      groupId: json['group_id'] as String,
      deviceId: json['device_id'] as String,
      role: json['role'] as String,
      addedAt: (json['added_at'] as num).toInt(),
      removedAt: json['removed_at'] == null
          ? null
          : (json['removed_at'] as num).toInt(),
    );
  }
}

/// 设备组聚合视图（对齐 Rust `DeviceGroupView`）。
@immutable
class DeviceGroupViewModel {
  const DeviceGroupViewModel({
    required this.group,
    required this.members,
  });

  final DeviceGroupModel group;
  final List<GroupMemberModel> members;

  List<GroupMemberModel> get activeMembers =>
      members.where((m) => m.isActive).toList(growable: false);

  factory DeviceGroupViewModel.fromJson(Map<String, dynamic> json) {
    final rawMembers = json['members'] as List<dynamic>;
    return DeviceGroupViewModel(
      group: DeviceGroupModel.fromJson(json['group'] as Map<String, dynamic>),
      members: rawMembers
          .map((e) => GroupMemberModel.fromJson(e as Map<String, dynamic>))
          .toList(growable: false),
    );
  }
}

/// 设备组同步规则（对齐 Rust `GroupSyncRuleRecord`）。
@immutable
class GroupSyncRuleModel {
  const GroupSyncRuleModel({
    required this.ruleId,
    required this.groupId,
    required this.name,
    required this.mode,
    required this.sourceDeviceId,
    required this.sourceRoot,
    this.sourceRootId,
    required this.archiveRetentionDays,
    required this.wifiOnly,
    this.allowedSsids,
    required this.requireCharging,
    required this.maxKbps,
    required this.scheduleIntervalSecs,
    required this.enabled,
    required this.revision,
    required this.groupRevision,
    required this.needsBindingConfirm,
    required this.createdAt,
    required this.updatedAt,
  });

  final String ruleId;
  final String groupId;
  final String name;
  final String mode;
  final String sourceDeviceId;
  final String sourceRoot;
  final String? sourceRootId;
  final int archiveRetentionDays;
  final bool wifiOnly;
  final String? allowedSsids;
  final bool requireCharging;
  final int maxKbps;
  final int scheduleIntervalSecs;
  final bool enabled;
  final int revision;
  final int groupRevision;
  final bool needsBindingConfirm;
  final int createdAt;
  final int updatedAt;

  bool get isMirrorSync => mode == 'mirror_sync';
  String get modeLabel => isMirrorSync ? '镜像同步' : '差异备份';

  factory GroupSyncRuleModel.fromJson(Map<String, dynamic> json) {
    return GroupSyncRuleModel(
      ruleId: json['rule_id'] as String,
      groupId: json['group_id'] as String,
      name: json['name'] as String,
      mode: json['mode'] as String,
      sourceDeviceId: json['source_device_id'] as String,
      sourceRoot: json['source_root'] as String,
      sourceRootId: json['source_root_id'] as String?,
      archiveRetentionDays: (json['archive_retention_days'] as num).toInt(),
      wifiOnly: json['wifi_only'] as bool,
      allowedSsids: json['allowed_ssids'] as String?,
      requireCharging: json['require_charging'] as bool,
      maxKbps: (json['max_kbps'] as num).toInt(),
      scheduleIntervalSecs: (json['schedule_interval_secs'] as num).toInt(),
      enabled: json['enabled'] as bool,
      revision: (json['revision'] as num).toInt(),
      groupRevision: (json['group_revision'] as num).toInt(),
      needsBindingConfirm: json['needs_binding_confirm'] as bool,
      createdAt: (json['created_at'] as num).toInt(),
      updatedAt: (json['updated_at'] as num).toInt(),
    );
  }
}

/// 规则目标绑定记录（对齐 Rust `RuleTargetBindingRecord`）。
@immutable
class RuleTargetBindingModel {
  const RuleTargetBindingModel({
    required this.bindingId,
    required this.ruleId,
    required this.groupId,
    required this.targetDeviceId,
    this.targetVaultId,
    this.mirrorRootId,
    this.targetRoot,
    required this.enabled,
    this.revokedAt,
    required this.appliedRevision,
    required this.lastSourceGen,
    required this.status,
    this.lastError,
    required this.needsBindingConfirm,
    required this.createdAt,
    required this.updatedAt,
  });

  final String bindingId;
  final String ruleId;
  final String groupId;
  final String targetDeviceId;
  final String? targetVaultId;
  final String? mirrorRootId;
  final String? targetRoot;
  final bool enabled;
  final int? revokedAt;
  final int appliedRevision;
  final int lastSourceGen;
  final String status;
  final String? lastError;
  final bool needsBindingConfirm;
  final int createdAt;
  final int updatedAt;

  bool get isRevoked => revokedAt != null;

  factory RuleTargetBindingModel.fromJson(Map<String, dynamic> json) {
    return RuleTargetBindingModel(
      bindingId: json['binding_id'] as String,
      ruleId: json['rule_id'] as String,
      groupId: json['group_id'] as String,
      targetDeviceId: json['target_device_id'] as String,
      targetVaultId: json['target_vault_id'] as String?,
      mirrorRootId: json['mirror_root_id'] as String?,
      targetRoot: json['target_root'] as String?,
      enabled: json['enabled'] as bool,
      revokedAt: json['revoked_at'] == null
          ? null
          : (json['revoked_at'] as num).toInt(),
      appliedRevision: (json['applied_revision'] as num).toInt(),
      lastSourceGen: (json['last_source_gen'] as num).toInt(),
      status: json['status'] as String,
      lastError: json['last_error'] as String?,
      needsBindingConfirm: json['needs_binding_confirm'] as bool,
      createdAt: (json['created_at'] as num).toInt(),
      updatedAt: (json['updated_at'] as num).toInt(),
    );
  }
}

/// 组规则及其目标绑定聚合视图（对齐 Rust `GroupRuleView`）。
@immutable
class GroupRuleViewModel {
  const GroupRuleViewModel({
    required this.rule,
    required this.bindings,
  });

  final GroupSyncRuleModel rule;
  final List<RuleTargetBindingModel> bindings;

  factory GroupRuleViewModel.fromJson(Map<String, dynamic> json) {
    final rawBindings = json['bindings'] as List<dynamic>;
    return GroupRuleViewModel(
      rule: GroupSyncRuleModel.fromJson(json['rule'] as Map<String, dynamic>),
      bindings: rawBindings
          .map(
              (e) => RuleTargetBindingModel.fromJson(e as Map<String, dynamic>))
          .toList(growable: false),
    );
  }
}

/// 已登记的本机镜像根目录（对齐 Rust `MirrorRootRegistration`）。
@immutable
class MirrorRootModel {
  const MirrorRootModel({
    required this.mirrorRootId,
    required this.deviceId,
    required this.canonicalRoot,
    this.rootIdentity,
    required this.label,
    required this.createdAt,
    this.revokedAt,
  });

  final String mirrorRootId;
  final String deviceId;
  final String canonicalRoot;
  final String? rootIdentity;
  final String label;
  final int createdAt;
  final int? revokedAt;

  bool get isActive => revokedAt == null;

  factory MirrorRootModel.fromJson(Map<String, dynamic> json) {
    return MirrorRootModel(
      mirrorRootId: json['mirror_root_id'] as String,
      deviceId: json['device_id'] as String,
      canonicalRoot: json['canonical_root'] as String,
      rootIdentity: json['root_identity'] as String?,
      label: json['label'] as String,
      createdAt: (json['created_at'] as num).toInt(),
      revokedAt: json['revoked_at'] == null
          ? null
          : (json['revoked_at'] as num).toInt(),
    );
  }
}

/// 镜像/规则同步执行前变更预览（对齐 Rust `MirrorPreviewReport`）。
@immutable
class MirrorPreviewModel {
  const MirrorPreviewModel({
    required this.ruleId,
    required this.bindingId,
    required this.targetDeviceId,
    required this.addCount,
    required this.overwriteCount,
    required this.removeCount,
    required this.unchangedCount,
    required this.blockedCount,
    required this.conflicts,
  });

  final String ruleId;
  final String bindingId;
  final String targetDeviceId;
  final int addCount;
  final int overwriteCount;
  final int removeCount;
  final int unchangedCount;
  final int blockedCount;
  final List<String> conflicts;

  factory MirrorPreviewModel.fromJson(Map<String, dynamic> json) {
    final rawConflicts = json['conflicts'] as List<dynamic>;
    return MirrorPreviewModel(
      ruleId: json['rule_id'] as String,
      bindingId: json['binding_id'] as String,
      targetDeviceId: json['target_device_id'] as String,
      addCount: (json['add_count'] as num).toInt(),
      overwriteCount: (json['overwrite_count'] as num).toInt(),
      removeCount: (json['remove_count'] as num).toInt(),
      unchangedCount: (json['unchanged_count'] as num).toInt(),
      blockedCount: (json['blocked_count'] as num).toInt(),
      conflicts: rawConflicts.map((e) => e as String).toList(growable: false),
    );
  }
}

/// 单目标绑定同步执行结果（对齐 Rust `BindingSyncOutcome`）。
@immutable
class BindingSyncOutcomeModel {
  const BindingSyncOutcomeModel({
    required this.bindingId,
    required this.targetDeviceId,
    required this.status,
    this.error,
    this.diffReport,
    this.mirrorReport,
  });

  final String bindingId;
  final String targetDeviceId;
  final String status;
  final String? error;
  final Map<String, dynamic>? diffReport;
  final Map<String, dynamic>? mirrorReport;

  factory BindingSyncOutcomeModel.fromJson(Map<String, dynamic> json) {
    return BindingSyncOutcomeModel(
      bindingId: json['binding_id'] as String,
      targetDeviceId: json['target_device_id'] as String,
      status: json['status'] as String,
      error: json['error'] as String?,
      diffReport: json['diff_report'] as Map<String, dynamic>?,
      mirrorReport: json['mirror_report'] as Map<String, dynamic>?,
    );
  }
}

/// 组规则同步汇总结果（对齐 Rust `GroupRuleSyncOutcome`）。
@immutable
class GroupRuleSyncOutcomeModel {
  const GroupRuleSyncOutcomeModel({
    required this.ruleId,
    required this.sourceComplete,
    required this.scannedEntries,
    required this.blockedReasons,
    required this.volatilePaths,
    required this.bindings,
  });

  final String ruleId;
  final bool sourceComplete;
  final int scannedEntries;
  final List<String> blockedReasons;
  final List<String> volatilePaths;
  final List<BindingSyncOutcomeModel> bindings;

  factory GroupRuleSyncOutcomeModel.fromJson(Map<String, dynamic> json) {
    final rawBlocked = json['blocked_reasons'] as List<dynamic>;
    final rawVolatile = json['volatile_paths'] as List<dynamic>;
    final rawBindings = json['bindings'] as List<dynamic>;
    return GroupRuleSyncOutcomeModel(
      ruleId: json['rule_id'] as String,
      sourceComplete: json['source_complete'] as bool,
      scannedEntries: (json['scanned_entries'] as num).toInt(),
      blockedReasons:
          rawBlocked.map((e) => e as String).toList(growable: false),
      volatilePaths:
          rawVolatile.map((e) => e as String).toList(growable: false),
      bindings: rawBindings
          .map((e) =>
              BindingSyncOutcomeModel.fromJson(e as Map<String, dynamic>))
          .toList(growable: false),
    );
  }
}

/// 增量目录归档条目（对齐 Rust `CatalogEntryRecord`）。
@immutable
class CatalogEntryModel {
  const CatalogEntryModel({
    required this.bindingId,
    required this.pathKey,
    required this.displayPath,
    required this.entryKind,
    required this.versionId,
    required this.size,
    required this.mtimeNs,
    this.contentHash,
    required this.chunkHashesJson,
    required this.sourceDeleted,
    this.sourceDeletedAt,
    required this.updatedRevision,
    required this.updatedAt,
  });

  final String bindingId;
  final String pathKey;
  final String displayPath;
  final String entryKind;
  final String versionId;
  final int size;
  final int mtimeNs;
  final List<int>? contentHash;
  final String chunkHashesJson;
  final bool sourceDeleted;
  final int? sourceDeletedAt;
  final int updatedRevision;
  final int updatedAt;

  bool get isDir => entryKind == 'dir';

  factory CatalogEntryModel.fromJson(Map<String, dynamic> json) {
    final rawHash = json['content_hash'] as List<dynamic>?;
    return CatalogEntryModel(
      bindingId: json['binding_id'] as String,
      pathKey: json['path_key'] as String,
      displayPath: json['display_path'] as String,
      entryKind: json['entry_kind'] as String,
      versionId: json['version_id'] as String,
      size: (json['size'] as num).toInt(),
      mtimeNs: (json['mtime_ns'] as num).toInt(),
      contentHash:
          rawHash?.map((e) => (e as num).toInt()).toList(growable: false),
      chunkHashesJson: json['chunk_hashes_json'] as String,
      sourceDeleted: json['source_deleted'] as bool,
      sourceDeletedAt: json['source_deleted_at'] == null
          ? null
          : (json['source_deleted_at'] as num).toInt(),
      updatedRevision: (json['updated_revision'] as num).toInt(),
      updatedAt: (json['updated_at'] as num).toInt(),
    );
  }
}

/// 文件历史归档版本（对齐 Rust `CatalogVersionRecord`）。
@immutable
class CatalogVersionModel {
  const CatalogVersionModel({
    required this.versionId,
    required this.bindingId,
    required this.vaultId,
    required this.pathKey,
    required this.displayPath,
    required this.entryKind,
    required this.size,
    required this.mtimeNs,
    this.contentHash,
    required this.chunkHashesJson,
    required this.archiveReason,
    required this.prevState,
    required this.operationId,
    required this.archivedAt,
    this.expiresAt,
    this.recycledAt,
  });

  final String versionId;
  final String bindingId;
  final String vaultId;
  final String pathKey;
  final String displayPath;
  final String entryKind;
  final int size;
  final int mtimeNs;
  final List<int>? contentHash;
  final String chunkHashesJson;
  final String archiveReason;
  final String prevState;
  final String operationId;
  final int archivedAt;
  final int? expiresAt;
  final int? recycledAt;

  bool get isRecycled => recycledAt != null;

  factory CatalogVersionModel.fromJson(Map<String, dynamic> json) {
    final rawHash = json['content_hash'] as List<dynamic>?;
    return CatalogVersionModel(
      versionId: json['version_id'] as String,
      bindingId: json['binding_id'] as String,
      vaultId: json['vault_id'] as String,
      pathKey: json['path_key'] as String,
      displayPath: json['display_path'] as String,
      entryKind: json['entry_kind'] as String,
      size: (json['size'] as num).toInt(),
      mtimeNs: (json['mtime_ns'] as num).toInt(),
      contentHash:
          rawHash?.map((e) => (e as num).toInt()).toList(growable: false),
      chunkHashesJson: json['chunk_hashes_json'] as String,
      archiveReason: json['archive_reason'] as String,
      prevState: json['prev_state'] as String,
      operationId: json['operation_id'] as String,
      archivedAt: (json['archived_at'] as num).toInt(),
      expiresAt: json['expires_at'] == null
          ? null
          : (json['expires_at'] as num).toInt(),
      recycledAt: json['recycled_at'] == null
          ? null
          : (json['recycled_at'] as num).toInt(),
    );
  }
}

/// 归档版本恢复结果（对齐 Rust `RestoreCatalogVersionResult`）。
@immutable
class RestoreCatalogVersionResultModel {
  const RestoreCatalogVersionResultModel({
    required this.bindingId,
    required this.versionId,
    required this.pathKey,
    required this.restoredBytes,
    required this.outputPath,
  });

  final String bindingId;
  final String versionId;
  final String pathKey;
  final int restoredBytes;
  final String outputPath;

  factory RestoreCatalogVersionResultModel.fromJson(Map<String, dynamic> json) {
    return RestoreCatalogVersionResultModel(
      bindingId: json['binding_id'] as String,
      versionId: json['version_id'] as String,
      pathKey: json['path_key'] as String,
      restoredBytes: (json['restored_bytes'] as num).toInt(),
      outputPath: json['output_path'] as String,
    );
  }
}

/// 非阻塞发件箱单目标投递状态（对齐 Rust `MessageOutboxTargetRecord`）。
@immutable
class OutboxTargetStatusModel {
  const OutboxTargetStatusModel({
    required this.clientMsgId,
    required this.targetDeviceId,
    required this.channel,
    required this.status,
    this.errorCode,
    this.errorMessage,
    required this.attemptCount,
    this.nextRetryAt,
    required this.updatedAt,
  });

  final String clientMsgId;
  final String targetDeviceId;
  final String channel;
  final String status;
  final String? errorCode;
  final String? errorMessage;
  final int attemptCount;
  final int? nextRetryAt;
  final int updatedAt;

  factory OutboxTargetStatusModel.fromJson(Map<String, dynamic> json) {
    return OutboxTargetStatusModel(
      clientMsgId: json['client_msg_id'] as String,
      targetDeviceId: json['target_device_id'] as String,
      channel: json['channel'] as String,
      status: json['status'] as String,
      errorCode: json['error_code'] as String?,
      errorMessage: json['error_message'] as String?,
      attemptCount: (json['attempt_count'] as num).toInt(),
      nextRetryAt: json['next_retry_at'] == null
          ? null
          : (json['next_retry_at'] as num).toInt(),
      updatedAt: (json['updated_at'] as num).toInt(),
    );
  }
}

/// 非阻塞发件箱消息视图（对齐 Rust `MessageOutboxView`）。
@immutable
class OutboxMessageViewModel {
  const OutboxMessageViewModel({
    required this.clientMsgId,
    required this.localSeq,
    required this.sessionKey,
    required this.kind,
    required this.title,
    this.content,
    this.filePath,
    this.fileHashHex,
    required this.sizeBytes,
    required this.overallStatus,
    required this.createdAt,
    required this.updatedAt,
    required this.targets,
  });

  final String clientMsgId;
  final int localSeq;
  final String sessionKey;
  final String kind;
  final String title;
  final String? content;
  final String? filePath;
  final String? fileHashHex;
  final int sizeBytes;
  final String overallStatus;
  final int createdAt;
  final int updatedAt;
  final List<OutboxTargetStatusModel> targets;

  bool get isFile => kind == 'file';

  bool get canRetry =>
      overallStatus == 'failed' || overallStatus == 'partial_failed';

  String get formattedTime {
    final dt = DateTime.fromMillisecondsSinceEpoch(createdAt * 1000);
    return '${dt.month.toString().padLeft(2, '0')}-${dt.day.toString().padLeft(2, '0')} '
        '${dt.hour.toString().padLeft(2, '0')}:${dt.minute.toString().padLeft(2, '0')}';
  }

  factory OutboxMessageViewModel.fromJson(Map<String, dynamic> json) {
    final msg = json['message'] as Map<String, dynamic>;
    final rawTargets = json['targets'] as List<dynamic>;
    return OutboxMessageViewModel(
      clientMsgId: msg['client_msg_id'] as String,
      localSeq: (msg['local_seq'] as num).toInt(),
      sessionKey: msg['session_key'] as String,
      kind: msg['kind'] as String,
      title: msg['title'] as String,
      content: msg['content'] as String?,
      filePath: msg['file_path'] as String?,
      fileHashHex: msg['file_hash_hex'] as String?,
      sizeBytes: (msg['size_bytes'] as num).toInt(),
      overallStatus: msg['overall_status'] as String,
      createdAt: (msg['created_at'] as num).toInt(),
      updatedAt: (msg['updated_at'] as num).toInt(),
      targets: rawTargets
          .map((e) =>
              OutboxTargetStatusModel.fromJson(e as Map<String, dynamic>))
          .toList(growable: false),
    );
  }
}

/// 严格对齐 `crates/lanvault-ffi/src/api.rs` 导出函数的抽象契约。
abstract class LanVaultRustFfi {
  Stream<DiscoveredPeer> startMdnsScan();
  Future<String> pairWithPin({
    required String ip,
    required int port,
    required String pin,
  });
  Future<String> requestPairing({
    required String ip,
    required int port,
  });
  Future<List<PendingPairRequestModel>> listPendingPairRequests();
  Future<bool> respondPairRequest({
    required String requestId,
    required bool accept,
  });
  Future<NetworkScanSnapshot> scanWindowsNetworks();
  Future<void> enableAutoReconnect(bool enabled);
  Future<int> startLocalServer({
    required String vaultPath,
    required int quotaGb,
    required String webPassword,
    int? port,
  });
  Future<void> prepareGracefulRestart();
  Future<LastServerConfig?> getLastServerConfig();
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
  Future<List<PairedPeerModel>> listPairedPeers();
  Future<PowerNetworkPolicyModel> getPowerNetworkPolicy();
  Future<bool> getAutoReconnect();
  Future<BackupStorageConfigModel> getBackupStorageConfig();
  Future<BackupStorageConfigModel> setBackupStorageConfig({
    required String primaryDir,
    String? fallbackDir,
  });
  Future<String?> getDebugLogPath();
  Future<Map<String, dynamic>> runSelfDiagnostics();

  // --- 新增 9 个 WebUI 端内整合 FFI 接口 ---
  Future<SystemOverviewModel> getSystemOverview();
  Future<List<VaultSummaryModel>> listVaults();
  Future<VaultSummaryModel> createVault({
    required String name,
    required String mountPath,
    required String poolMode,
    required int quotaGb,
    required bool e2eeEnabled,
    String? extraMountPath,
  });
  Future<List<SnapshotSummaryModel>> listSnapshots({String? vaultId});
  Future<BitRotScrubResultModel> triggerBitRotScrub();
  Future<bool> revokePairedPeer(String deviceId);
  Future<PairingInfoModel> getPairingInfo();
  Future<bool> toggleSyncRule({
    required String ruleId,
    required bool enabled,
  });
  Future<bool> pushClipboardFile({
    String? targetDeviceId,
    required String filePath,
  });

  Future<RestoreSnapshotResultModel> restoreSnapshot({
    required String snapshotId,
    required String targetDir,
  });

  Future<List<TransferHistoryModel>> listTransferHistory({int limit = 50});

  Future<bool> setSyncPaused(bool paused);

  Future<bool> setFastBackupMode(bool enabled);

  Future<bool> getSyncPaused();

  Future<bool> setBitRotAutoScrub(bool enabled);

  Future<bool> getBitRotAutoScrub();

  Future<String> setSyncRuleSchedule({
    required String ruleId,
    required String scheduleMode,
  });

  /// 启动时自动拉起上次配置好的本地存储服务；返回监听端口，或 null（尚未配置/已关闭）。
  Future<int?> autostartLocalServer();

  /// 是否在每次启动时自动开启存储服务。
  Future<bool> getAutostartServer();

  Future<bool> setAutostartServer(bool enabled);

  // --- v2 Device Groups, Group Sync Rules, Catalog & Non-Blocking Outbox ---
  Future<List<DeviceGroupViewModel>> listDeviceGroups();
  Future<DeviceGroupModel> createDeviceGroup(String name);
  Future<DeviceGroupModel> renameDeviceGroup({
    required String groupId,
    required String name,
  });
  Future<void> deleteDeviceGroup(String groupId);
  Future<void> addGroupMember({
    required String groupId,
    required String deviceId,
    String? role,
  });
  Future<void> removeGroupMember({
    required String groupId,
    required String deviceId,
  });
  Future<List<GroupRuleViewModel>> listGroupRules({String? groupId});
  Future<GroupSyncRuleModel> upsertGroupRule({
    String? ruleId,
    required String groupId,
    required String name,
    required String mode,
    String? sourceDeviceId,
    required String sourceRoot,
    String? sourceRootId,
    int archiveRetentionDays = 30,
    bool wifiOnly = true,
    String? allowedSsids,
    bool requireCharging = false,
    int maxKbps = 0,
    int scheduleIntervalSecs = 0,
    bool enabled = true,
  });
  Future<void> deleteGroupRule(String ruleId);
  Future<void> toggleGroupRule({
    required String ruleId,
    required bool enabled,
  });
  Future<RuleTargetBindingModel> upsertRuleTargetBinding({
    String? bindingId,
    required String ruleId,
    required String targetDeviceId,
    String? targetVaultId,
    String? mirrorRootId,
    String? targetRoot,
    bool enabled = true,
  });
  Future<void> disableRuleTargetBinding(String bindingId);
  Future<MirrorRootModel> registerMirrorRoot({
    required String canonicalPath,
    String displayName = '',
  });
  Future<List<MirrorRootModel>> listMirrorRoots();
  Future<void> revokeMirrorRoot(String rootId);
  Future<List<MirrorPreviewModel>> previewGroupRule(String ruleId);
  Future<GroupRuleSyncOutcomeModel> triggerGroupRuleSync(String ruleId);
  Future<List<CatalogEntryModel>> listBindingCatalog({
    required String bindingId,
    String? afterPathKey,
    int? limit,
    bool sourceDeletedOnly = false,
  });
  Future<List<CatalogVersionModel>> listBindingVersions({
    required String bindingId,
    String? pathKey,
    int? afterArchivedAt,
    String? afterVersionId,
    int? limit,
  });
  Future<RestoreCatalogVersionResultModel> restoreCatalogVersion({
    required String bindingId,
    required String versionId,
    required String targetDir,
  });
  Future<OutboxMessageViewModel> enqueueMessageText({
    required List<String> targetDeviceIds,
    required String text,
  });
  Future<OutboxMessageViewModel> enqueueMessageFile({
    required List<String> targetDeviceIds,
    required String filePath,
  });
  Future<int> retryOutboxMessage(String clientMsgId);
  Future<List<OutboxMessageViewModel>> listOutboxMessages({int? limit});
}

// --- Native C-ABI 符号签名，与 lanvault_ffi 共享库对齐 ---
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
typedef _CDecodeWoff2Native = ffi.Int32 Function(
  ffi.Pointer<ffi.Uint8> woff2Ptr,
  ffi.Size woff2Len,
  ffi.Pointer<ffi.Pointer<ffi.Uint8>> outPtr,
  ffi.Pointer<ffi.Size> outLen,
);
typedef _CDecodeWoff2Dart = int Function(
  ffi.Pointer<ffi.Uint8> woff2Ptr,
  int woff2Len,
  ffi.Pointer<ffi.Pointer<ffi.Uint8>> outPtr,
  ffi.Pointer<ffi.Size> outLen,
);
typedef _CFreeBytesNative = ffi.Void Function(
  ffi.Pointer<ffi.Uint8> ptr,
  ffi.Size len,
);
typedef _CFreeBytesDart = void Function(ffi.Pointer<ffi.Uint8> ptr, int len);

Uint8List _decodeWoff2InWorker(Uint8List woff2Bytes) {
  final lib = _openLanVaultLibrary();
  final decodeFn = lib.lookupFunction<_CDecodeWoff2Native, _CDecodeWoff2Dart>(
    'lanvault_ffi_decode_woff2',
  );
  final freeBytes = lib.lookupFunction<_CFreeBytesNative, _CFreeBytesDart>(
    'lanvault_ffi_free_bytes',
  );
  final inPtr = calloc<ffi.Uint8>(woff2Bytes.length);
  final outPtrPtr = calloc<ffi.Pointer<ffi.Uint8>>();
  final outLenPtr = calloc<ffi.Size>();
  try {
    inPtr.asTypedList(woff2Bytes.length).setAll(0, woff2Bytes);
    final rc = decodeFn(inPtr, woff2Bytes.length, outPtrPtr, outLenPtr);
    if (rc != 0) {
      throw StateError('WOFF2 decode failed with code $rc');
    }
    final outPtr = outPtrPtr.value;
    final outLen = outLenPtr.value;
    if (outPtr == ffi.nullptr || outLen == 0) {
      throw StateError('WOFF2 decode returned empty buffer');
    }
    try {
      return Uint8List.fromList(outPtr.asTypedList(outLen));
    } finally {
      freeBytes(outPtr, outLen);
    }
  } finally {
    calloc.free(inPtr);
    calloc.free(outPtrPtr);
    calloc.free(outLenPtr);
  }
}

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
    if (resultPtr == ffi.nullptr) {
      throw StateError('FFI $fnName returned null pointer.');
    }
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

/// 常驻的 Rust 调用工作 Isolate。
///
/// 原来的实现是每次 FFI 调用都 `Isolate.run(...)`：一次调用就要新建 isolate、重新
/// `dlopen` 动态库、查符号表，然后销毁。UI 里同步进度每秒轮询、配对请求每 2 秒轮询、
/// 环境每 15 秒刷新，加起来每秒都在重复这套开销，还会和真正的传输抢 CPU。
///
/// 这里改为进程内一个长驻 worker：通过 `SendPort` 串行派发请求，`dlopen` 与符号查找
/// 只做一次。Rust 侧本来就是多线程异步运行时，真正并行的是 Rust 内部任务，
/// 而不是 Dart isolate 的数量；串行派发反而消除了多个 isolate 同时抢全局状态锁的问题。
class _FfiWorker {
  _FfiWorker._(this._sendPort);

  final SendPort _sendPort;
  final Map<int, Completer<Map<String, dynamic>>> _pending =
      <int, Completer<Map<String, dynamic>>>{};
  int _nextRequestId = 1;
  ReceivePort? _replyPort;
  bool _dead = false;

  static _FfiWorker? _instance;
  static Future<_FfiWorker>? _starting;
  static _FfiWorker? _transferInstance;
  static Future<_FfiWorker>? _transferStarting;

  static const Set<String> _heavyFfiOps = <String>{
    'trigger_incremental_scan',
    'push_clipboard_file',
    'restore_snapshot',
    'trigger_bit_rot_scrub',
  };

  /// 获取（必要时启动）常驻 worker：控制通道与长耗时传输通道隔离，避免传输阻塞进度轮询与 UI 响应。
  static Future<_FfiWorker> acquire({bool heavy = false}) {
    if (heavy) {
      final existing = _transferInstance;
      if (existing != null && !existing._dead) {
        return Future<_FfiWorker>.value(existing);
      }
      final inFlight = _transferStarting;
      if (inFlight != null) return inFlight;
      final future = _FfiWorker._spawn(isTransfer: true).whenComplete(() {
        if (_transferStarting != null && _transferInstance == null) {
          _transferStarting = null;
        }
      });
      _transferStarting = future;
      return future;
    }
    final existing = _instance;
    if (existing != null && !existing._dead) {
      return Future<_FfiWorker>.value(existing);
    }
    final inFlight = _starting;
    if (inFlight != null) return inFlight;
    // 失败必须清掉 `_starting`，否则后续调用会永远 await 一个已失败的 Future。
    final future = _FfiWorker._spawn(isTransfer: false).whenComplete(() {
      if (_starting != null && _instance == null) _starting = null;
    });
    _starting = future;
    return future;
  }

  static Future<_FfiWorker> _spawn({bool isTransfer = false}) async {
    final replyPort = ReceivePort();
    final hostPort = replyPort.sendPort;
    final handshake = Completer<SendPort>();

    await Isolate.spawn<_WorkerBootstrap>(
      _ffiWorkerMain,
      _WorkerBootstrap(hostPort),
      debugName: 'everyonenas-ffi-worker',
    );

    late _FfiWorker worker;
    replyPort.listen((Object? message) {
      if (message is SendPort) {
        if (!handshake.isCompleted) handshake.complete(message);
        return;
      }
      if (message is _WorkerReply) {
        final completer = worker._pending.remove(message.requestId);
        if (completer != null && !completer.isCompleted) {
          if (message.error != null) {
            completer.completeError(StateError(message.error!));
          } else {
            completer.complete(message.payload ?? <String, dynamic>{});
          }
        }
        return;
      }
      // isolate 退出或异常：标记死亡，后续调用会重建 worker。
      worker._markDead();
    });

    final sendPort = await handshake.future.timeout(
      const Duration(seconds: 10),
      onTimeout: () {
        replyPort.close();
        throw StateError('FFI worker isolate failed to start');
      },
    );
    worker = _FfiWorker._(sendPort).._replyPort = replyPort;
    if (isTransfer) {
      _transferInstance = worker;
      _transferStarting = null;
    } else {
      _instance = worker;
      _starting = null;
    }
    return worker;
  }

  void _markDead() {
    _dead = true;
    if (identical(_instance, this)) _instance = null;
    _starting = null;
    _replyPort?.close();
    _replyPort = null;
    for (final completer in _pending.values) {
      if (!completer.isCompleted) {
        completer.completeError(StateError('FFI worker isolate exited'));
      }
    }
    _pending.clear();
  }

  /// 派发一次调用；由 [_FfiWorker] 内部串行化。
  Future<Map<String, dynamic>> call(String fnName, String payloadJson) {
    if (_dead) {
      return Future<Map<String, dynamic>>.error(
        StateError('FFI worker isolate is not running'),
      );
    }
    final id = _nextRequestId++;
    final completer = Completer<Map<String, dynamic>>();
    _pending[id] = completer;
    _sendPort.send(_WorkerRequest(id, fnName, payloadJson));
    return completer.future;
  }
}

class _WorkerBootstrap {
  const _WorkerBootstrap(this.replyPort);
  final SendPort replyPort;
}

class _WorkerRequest {
  const _WorkerRequest(this.requestId, this.fnName, this.payloadJson);
  final int requestId;
  final String fnName;
  final String payloadJson;
}

class _WorkerReply {
  const _WorkerReply(this.requestId, this.payload, this.error);
  final int requestId;
  final Map<String, dynamic>? payload;
  final String? error;
}

/// 常驻 worker 的主循环：只在这里做一次库打开与符号查找。
void _ffiWorkerMain(_WorkerBootstrap bootstrap) {
  final requests = ReceivePort();
  ffi.DynamicLibrary? library;
  _CInvokeJsonDart? invokeJson;
  _CFreeStringDart? freeString;

  void ensureLibrary() {
    if (library != null) return;
    final opened = _openLanVaultLibrary();
    library = opened;
    invokeJson = opened.lookupFunction<_CInvokeJsonNative, _CInvokeJsonDart>(
      'lanvault_ffi_invoke_json',
    );
    freeString = opened.lookupFunction<_CFreeStringNative, _CFreeStringDart>(
      'lanvault_ffi_free_string',
    );
  }

  bootstrap.replyPort.send(requests.sendPort);
  requests.listen((Object? message) {
    if (message is! _WorkerRequest) return;
    ffi.Pointer<Utf8>? fnPtr;
    ffi.Pointer<Utf8>? payloadPtr;
    ffi.Pointer<Utf8>? resultPtr;
    try {
      ensureLibrary();
      fnPtr = message.fnName.toNativeUtf8();
      payloadPtr = message.payloadJson.toNativeUtf8();
      resultPtr = invokeJson!(fnPtr, payloadPtr);
      if (resultPtr == ffi.nullptr) {
        throw StateError('FFI ${message.fnName} returned null pointer.');
      }
      final raw = resultPtr.toDartString();
      final decoded = jsonDecode(raw) as Map<String, dynamic>;
      if (decoded['ok'] != true) {
        throw StateError('FFI ${message.fnName} failed: ${decoded['error']}');
      }
      bootstrap.replyPort.send(_WorkerReply(message.requestId, decoded, null));
    } catch (error) {
      bootstrap.replyPort
          .send(_WorkerReply(message.requestId, null, error.toString()));
    } finally {
      if (fnPtr != null) calloc.free(fnPtr);
      if (payloadPtr != null) calloc.free(payloadPtr);
      if (resultPtr != null && resultPtr != ffi.nullptr) {
        freeString!(resultPtr);
      }
    }
  });
}

class NativeLanVaultRustFfi implements LanVaultRustFfi {
  NativeLanVaultRustFfi();

  /// 常驻 worker 不可用（例如被系统回收）时的一次性兜底。
  Future<Map<String, dynamic>> _callViaOneShotIsolate(
    String fnName,
    String encoded,
  ) {
    return Isolate.run(() => _invokeJsonInWorker((fnName, encoded)));
  }

  Future<Map<String, dynamic>> _callAsync(
    String fnName,
    Map<String, Object?> payload,
  ) async {
    final encoded = jsonEncode(payload);
    try {
      final worker = await _FfiWorker.acquire(
          heavy: _FfiWorker._heavyFfiOps.contains(fnName));
      return await worker.call(fnName, encoded);
    } catch (error) {
      // 只有"worker 起不来"才回退；Rust 侧返回的业务错误必须原样抛给调用方，
      // 否则会用一次额外 isolate 重跑同一个失败调用（例如密码错的登录）。
      final message = error.toString();
      final workerInfraFailure = message.contains('worker isolate') ||
          message.contains('DynamicLibrary') ||
          message.contains('Failed to load dynamic library');
      if (!workerInfraFailure) rethrow;
      return _callViaOneShotIsolate(fnName, encoded);
    }
  }

  @override
  Stream<DiscoveredPeer> startMdnsScan() {
    late final StreamController<DiscoveredPeer> controller;
    Timer? timer;
    var inFlight = false;

    Future<void> pollOnce() async {
      if (inFlight || controller.isClosed) return;
      inFlight = true;
      try {
        final resp =
            await _callAsync('start_mdns_scan', const <String, Object?>{});
        if (controller.isClosed) return;
        final list = resp['peers'] as List<dynamic>;
        for (final item in list) {
          controller.add(DiscoveredPeer.fromJson(item as Map<String, dynamic>));
        }
      } catch (err, st) {
        if (!controller.isClosed) {
          controller.addError(err, st);
        }
      } finally {
        inFlight = false;
      }
    }

    controller = StreamController<DiscoveredPeer>(
      onListen: () {
        unawaited(pollOnce());
        timer = Timer.periodic(
          const Duration(seconds: 2),
          (_) => unawaited(pollOnce()),
        );
      },
      onCancel: () async {
        timer?.cancel();
        timer = null;
        if (!controller.isClosed) {
          await controller.close();
        }
      },
    );
    return controller.stream;
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
  Future<String> requestPairing({
    required String ip,
    required int port,
  }) async {
    final resp = await _callAsync('request_pairing', <String, Object?>{
      'ip': ip,
      'port': port,
    });
    return resp['device_id'] as String;
  }

  @override
  Future<List<PendingPairRequestModel>> listPendingPairRequests() async {
    final resp = await _callAsync(
      'list_pending_pair_requests',
      const <String, Object?>{},
    );
    final list = resp['requests'] as List<dynamic>? ?? const <dynamic>[];
    return list
        .map(
          (item) =>
              PendingPairRequestModel.fromJson(item as Map<String, dynamic>),
        )
        .toList(growable: false);
  }

  @override
  Future<bool> respondPairRequest({
    required String requestId,
    required bool accept,
  }) async {
    final resp = await _callAsync('respond_pair_request', <String, Object?>{
      'request_id': requestId,
      'accept': accept,
    });
    return resp['accepted'] as bool? ?? accept;
  }

  @override
  Future<NetworkScanSnapshot> scanWindowsNetworks() async {
    final resp = await _callAsync(
      'scan_windows_networks',
      const <String, Object?>{},
    );
    final ssids =
        (resp['available_ssids'] as List<dynamic>? ?? const <dynamic>[])
            .map((e) => e.toString())
            .toList(growable: false);
    return NetworkScanSnapshot(
      hasWifiAdapter: resp['has_wifi_adapter'] as bool? ?? false,
      isWiredEthernet: resp['is_wired_ethernet'] as bool? ?? false,
      currentSsid: resp['current_ssid'] as String?,
      availableSsids: ssids,
    );
  }

  @override
  Future<void> enableAutoReconnect(bool enabled) async {
    await _callAsync(
      'enable_auto_reconnect',
      <String, Object?>{'enabled': enabled},
    );
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
  Future<LastServerConfig?> getLastServerConfig() async {
    final resp = await _callAsync(
      'get_last_server_config',
      const <String, Object?>{},
    );
    final raw = resp.containsKey('config') ? resp['config'] : resp;
    if (raw == null) return null;
    if (raw is Map<String, dynamic> && raw['vault_path'] is String) {
      return LastServerConfig.fromRustJson(raw);
    }
    return null;
  }

  @override
  Stream<SyncProgressEvent> subscribeSyncProgress() {
    late final StreamController<SyncProgressEvent> controller;
    Timer? timer;
    var inFlight = false;

    Future<void> pollOnce() async {
      if (inFlight || controller.isClosed) return;
      inFlight = true;
      try {
        final resp = await _callAsync(
          'subscribe_sync_progress',
          const <String, Object?>{},
        );
        if (controller.isClosed) return;
        controller.add(
          SyncProgressEvent.fromJson(resp['event'] as Map<String, dynamic>),
        );
      } catch (err, st) {
        if (!controller.isClosed) {
          controller.addError(err, st);
        }
      } finally {
        inFlight = false;
      }
    }

    controller = StreamController<SyncProgressEvent>(
      onListen: () {
        unawaited(pollOnce());
        timer = Timer.periodic(
          const Duration(seconds: 1),
          (_) => unawaited(pollOnce()),
        );
      },
      onCancel: () async {
        timer?.cancel();
        timer = null;
        if (!controller.isClosed) {
          await controller.close();
        }
      },
    );
    return controller.stream;
  }

  @override
  Future<void> triggerIncrementalScan(String ruleId) async {
    await _callAsync(
      'trigger_incremental_scan',
      <String, Object?>{'rule_id': ruleId},
    );
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

  @override
  Future<List<PairedPeerModel>> listPairedPeers() async {
    final resp =
        await _callAsync('list_paired_peers', const <String, Object?>{});
    final list = resp['peers'] as List<dynamic>;
    return list
        .map((e) => PairedPeerModel.fromJson(e as Map<String, dynamic>))
        .toList(growable: false);
  }

  @override
  Future<PowerNetworkPolicyModel> getPowerNetworkPolicy() async {
    final resp = await _callAsync(
      'get_power_network_policy',
      const <String, Object?>{},
    );
    return PowerNetworkPolicyModel.fromJson(
      resp['policy'] as Map<String, dynamic>,
    );
  }

  @override
  Future<bool> getAutoReconnect() async {
    final resp =
        await _callAsync('get_auto_reconnect', const <String, Object?>{});
    return resp['enabled'] as bool;
  }

  @override
  Future<BackupStorageConfigModel> getBackupStorageConfig() async {
    final resp = await _callAsync(
      'get_backup_storage_config',
      const <String, Object?>{},
    );
    return BackupStorageConfigModel.fromJson(
      resp['config'] as Map<String, dynamic>,
    );
  }

  @override
  Future<BackupStorageConfigModel> setBackupStorageConfig({
    required String primaryDir,
    String? fallbackDir,
  }) async {
    final resp = await _callAsync(
      'set_backup_storage_config',
      <String, Object?>{
        'primary_dir': primaryDir,
        'fallback_dir': fallbackDir,
      },
    );
    return BackupStorageConfigModel.fromJson(
      resp['config'] as Map<String, dynamic>,
    );
  }

  @override
  Future<String?> getDebugLogPath() async {
    final resp =
        await _callAsync('get_debug_log_path', const <String, Object?>{});
    return resp['log_path'] as String?;
  }

  @override
  Future<Map<String, dynamic>> runSelfDiagnostics() async {
    final resp =
        await _callAsync('run_self_diagnostics', const <String, Object?>{});
    return resp['report'] as Map<String, dynamic>;
  }

  // --- 新增 9 个 WebUI 端内整合 FFI 接口实现 ---

  @override
  Future<SystemOverviewModel> getSystemOverview() async {
    final resp = await _callAsync(
      'get_system_overview',
      const <String, Object?>{},
    );
    final raw = resp.containsKey('overview')
        ? resp['overview'] as Map<String, dynamic>
        : resp;
    return SystemOverviewModel.fromJson(raw);
  }

  @override
  Future<List<VaultSummaryModel>> listVaults() async {
    final resp = await _callAsync('list_vaults', const <String, Object?>{});
    final list = resp['vaults'] as List<dynamic>;
    return list
        .map((e) => VaultSummaryModel.fromJson(e as Map<String, dynamic>))
        .toList(growable: false);
  }

  @override
  Future<VaultSummaryModel> createVault({
    required String name,
    required String mountPath,
    required String poolMode,
    required int quotaGb,
    required bool e2eeEnabled,
    String? extraMountPath,
  }) async {
    final resp = await _callAsync('create_vault', <String, Object?>{
      'name': name,
      'mount_path': mountPath,
      'pool_mode': poolMode,
      'quota_gb': quotaGb,
      'e2ee_enabled': e2eeEnabled,
      'extra_mount_path': extraMountPath,
    });
    final raw = resp.containsKey('vault')
        ? resp['vault'] as Map<String, dynamic>
        : resp;
    return VaultSummaryModel.fromJson(raw);
  }

  @override
  Future<List<SnapshotSummaryModel>> listSnapshots({String? vaultId}) async {
    final resp = await _callAsync('list_snapshots', <String, Object?>{
      'vault_id': vaultId,
    });
    final list = resp['snapshots'] as List<dynamic>;
    return list
        .map((e) => SnapshotSummaryModel.fromJson(e as Map<String, dynamic>))
        .toList(growable: false);
  }

  @override
  Future<BitRotScrubResultModel> triggerBitRotScrub() async {
    final resp = await _callAsync(
      'trigger_bit_rot_scrub',
      const <String, Object?>{},
    );
    final raw = resp.containsKey('scrub')
        ? resp['scrub'] as Map<String, dynamic>
        : resp;
    return BitRotScrubResultModel.fromJson(raw);
  }

  @override
  Future<bool> revokePairedPeer(String deviceId) async {
    final resp = await _callAsync('revoke_paired_peer', <String, Object?>{
      'device_id': deviceId,
    });
    return resp['revoked'] as bool;
  }

  @override
  Future<PairingInfoModel> getPairingInfo() async {
    final resp = await _callAsync(
      'get_pairing_info',
      const <String, Object?>{},
    );
    final raw = resp.containsKey('pairing_info')
        ? resp['pairing_info'] as Map<String, dynamic>
        : resp;
    return PairingInfoModel.fromJson(raw);
  }

  @override
  Future<bool> toggleSyncRule({
    required String ruleId,
    required bool enabled,
  }) async {
    final resp = await _callAsync('toggle_sync_rule', <String, Object?>{
      'rule_id': ruleId,
      'enabled': enabled,
    });
    return resp['enabled'] as bool;
  }

  @override
  Future<bool> pushClipboardFile({
    String? targetDeviceId,
    required String filePath,
  }) async {
    final resp = await _callAsync('push_clipboard_file', <String, Object?>{
      'target_device_id': targetDeviceId,
      'file_path': filePath,
    });
    return resp['sent'] as bool;
  }

  @override
  Future<RestoreSnapshotResultModel> restoreSnapshot({
    required String snapshotId,
    required String targetDir,
  }) async {
    final resp = await _callAsync('restore_snapshot', <String, Object?>{
      'snapshot_id': snapshotId,
      'target_dir': targetDir,
    });
    return RestoreSnapshotResultModel.fromJson(resp);
  }

  @override
  Future<List<TransferHistoryModel>> listTransferHistory(
      {int limit = 50}) async {
    final resp = await _callAsync('list_transfer_history', <String, Object?>{
      'limit': limit,
    });
    final raw = resp['records'] as List<dynamic>? ?? const <dynamic>[];
    return raw
        .map((e) => TransferHistoryModel.fromJson(e as Map<String, dynamic>))
        .toList(growable: false);
  }

  @override
  Future<bool> setSyncPaused(bool paused) async {
    final resp = await _callAsync('set_sync_paused', <String, Object?>{
      'paused': paused,
    });
    return resp['paused'] as bool? ?? paused;
  }

  @override
  Future<bool> setFastBackupMode(bool enabled) async {
    final resp = await _callAsync('set_fast_backup_mode', <String, Object?>{
      'enabled': enabled,
    });
    return resp['enabled'] as bool? ?? enabled;
  }

  @override
  Future<bool> getSyncPaused() async {
    final resp = await _callAsync('get_sync_paused', const <String, Object?>{});
    return resp['paused'] as bool? ?? false;
  }

  @override
  Future<bool> setBitRotAutoScrub(bool enabled) async {
    final resp = await _callAsync('set_bit_rot_auto_scrub', <String, Object?>{
      'enabled': enabled,
    });
    return resp['enabled'] as bool? ?? enabled;
  }

  @override
  Future<bool> getBitRotAutoScrub() async {
    final resp =
        await _callAsync('get_bit_rot_auto_scrub', const <String, Object?>{});
    return resp['enabled'] as bool? ?? true;
  }

  @override
  Future<String> setSyncRuleSchedule({
    required String ruleId,
    required String scheduleMode,
  }) async {
    final resp = await _callAsync('set_sync_rule_schedule', <String, Object?>{
      'rule_id': ruleId,
      'schedule_mode': scheduleMode,
    });
    return resp['schedule_mode'] as String? ?? scheduleMode;
  }

  @override
  Future<int?> autostartLocalServer() async {
    final resp = await _callAsync(
      'autostart_local_server',
      const <String, Object?>{},
    );
    return (resp['port'] as num?)?.toInt();
  }

  @override
  Future<bool> getAutostartServer() async {
    final resp =
        await _callAsync('get_autostart_server', const <String, Object?>{});
    return resp['enabled'] as bool? ?? true;
  }

  @override
  Future<bool> setAutostartServer(bool enabled) async {
    final resp = await _callAsync(
      'set_autostart_server',
      <String, Object?>{'enabled': enabled},
    );
    return resp['enabled'] as bool? ?? enabled;
  }

  // --- v2 Device Groups, Group Sync Rules, Catalog & Non-Blocking Outbox ---

  @override
  Future<List<DeviceGroupViewModel>> listDeviceGroups() async {
    final resp =
        await _callAsync('list_device_groups', const <String, Object?>{});
    final list = resp['groups'] as List<dynamic>;
    return list
        .map((e) => DeviceGroupViewModel.fromJson(e as Map<String, dynamic>))
        .toList(growable: false);
  }

  @override
  Future<DeviceGroupModel> createDeviceGroup(String name) async {
    final resp = await _callAsync('create_device_group', <String, Object?>{
      'name': name,
    });
    return DeviceGroupModel.fromJson(resp['group'] as Map<String, dynamic>);
  }

  @override
  Future<DeviceGroupModel> renameDeviceGroup({
    required String groupId,
    required String name,
  }) async {
    final resp = await _callAsync('rename_device_group', <String, Object?>{
      'group_id': groupId,
      'name': name,
    });
    return DeviceGroupModel.fromJson(resp['group'] as Map<String, dynamic>);
  }

  @override
  Future<void> deleteDeviceGroup(String groupId) async {
    await _callAsync('delete_device_group', <String, Object?>{
      'group_id': groupId,
    });
  }

  @override
  Future<void> addGroupMember({
    required String groupId,
    required String deviceId,
    String? role,
  }) async {
    await _callAsync('add_group_member', <String, Object?>{
      'group_id': groupId,
      'device_id': deviceId,
      'role': role,
    });
  }

  @override
  Future<void> removeGroupMember({
    required String groupId,
    required String deviceId,
  }) async {
    await _callAsync('remove_group_member', <String, Object?>{
      'group_id': groupId,
      'device_id': deviceId,
    });
  }

  @override
  Future<List<GroupRuleViewModel>> listGroupRules({String? groupId}) async {
    final resp = await _callAsync('list_group_rules', <String, Object?>{
      'group_id': groupId,
    });
    final list = resp['rules'] as List<dynamic>;
    return list
        .map((e) => GroupRuleViewModel.fromJson(e as Map<String, dynamic>))
        .toList(growable: false);
  }

  @override
  Future<GroupSyncRuleModel> upsertGroupRule({
    String? ruleId,
    required String groupId,
    required String name,
    required String mode,
    String? sourceDeviceId,
    required String sourceRoot,
    String? sourceRootId,
    int archiveRetentionDays = 30,
    bool wifiOnly = true,
    String? allowedSsids,
    bool requireCharging = false,
    int maxKbps = 0,
    int scheduleIntervalSecs = 0,
    bool enabled = true,
  }) async {
    final resp = await _callAsync('upsert_group_rule', <String, Object?>{
      'rule_id': ruleId,
      'group_id': groupId,
      'name': name,
      'mode': mode,
      'source_device_id': sourceDeviceId,
      'source_root': sourceRoot,
      'source_root_id': sourceRootId,
      'archive_retention_days': archiveRetentionDays,
      'wifi_only': wifiOnly,
      'allowed_ssids': allowedSsids,
      'require_charging': requireCharging,
      'max_kbps': maxKbps,
      'schedule_interval_secs': scheduleIntervalSecs,
      'enabled': enabled,
    });
    return GroupSyncRuleModel.fromJson(resp['rule'] as Map<String, dynamic>);
  }

  @override
  Future<void> deleteGroupRule(String ruleId) async {
    await _callAsync('delete_group_rule', <String, Object?>{
      'rule_id': ruleId,
    });
  }

  @override
  Future<void> toggleGroupRule({
    required String ruleId,
    required bool enabled,
  }) async {
    await _callAsync('toggle_group_rule', <String, Object?>{
      'rule_id': ruleId,
      'enabled': enabled,
    });
  }

  @override
  Future<RuleTargetBindingModel> upsertRuleTargetBinding({
    String? bindingId,
    required String ruleId,
    required String targetDeviceId,
    String? targetVaultId,
    String? mirrorRootId,
    String? targetRoot,
    bool enabled = true,
  }) async {
    final resp =
        await _callAsync('upsert_rule_target_binding', <String, Object?>{
      'binding_id': bindingId,
      'rule_id': ruleId,
      'target_device_id': targetDeviceId,
      'target_vault_id': targetVaultId,
      'mirror_root_id': mirrorRootId,
      'target_root': targetRoot,
      'enabled': enabled,
    });
    return RuleTargetBindingModel.fromJson(
      resp['binding'] as Map<String, dynamic>,
    );
  }

  @override
  Future<void> disableRuleTargetBinding(String bindingId) async {
    await _callAsync('disable_rule_target_binding', <String, Object?>{
      'binding_id': bindingId,
    });
  }

  @override
  Future<MirrorRootModel> registerMirrorRoot({
    required String canonicalPath,
    String displayName = '',
  }) async {
    final resp = await _callAsync('register_mirror_root', <String, Object?>{
      'canonical_path': canonicalPath,
      'display_name': displayName,
    });
    return MirrorRootModel.fromJson(
      resp['mirror_root'] as Map<String, dynamic>,
    );
  }

  @override
  Future<List<MirrorRootModel>> listMirrorRoots() async {
    final resp =
        await _callAsync('list_mirror_roots', const <String, Object?>{});
    final list = resp['mirror_roots'] as List<dynamic>;
    return list
        .map((e) => MirrorRootModel.fromJson(e as Map<String, dynamic>))
        .toList(growable: false);
  }

  @override
  Future<void> revokeMirrorRoot(String rootId) async {
    await _callAsync('revoke_mirror_root', <String, Object?>{
      'root_id': rootId,
    });
  }

  @override
  Future<List<MirrorPreviewModel>> previewGroupRule(String ruleId) async {
    final resp = await _callAsync('preview_group_rule', <String, Object?>{
      'rule_id': ruleId,
    });
    final list = resp['previews'] as List<dynamic>;
    return list
        .map((e) => MirrorPreviewModel.fromJson(e as Map<String, dynamic>))
        .toList(growable: false);
  }

  @override
  Future<GroupRuleSyncOutcomeModel> triggerGroupRuleSync(String ruleId) async {
    final resp = await _callAsync('trigger_group_rule_sync', <String, Object?>{
      'rule_id': ruleId,
    });
    return GroupRuleSyncOutcomeModel.fromJson(
      resp['outcome'] as Map<String, dynamic>,
    );
  }

  @override
  Future<List<CatalogEntryModel>> listBindingCatalog({
    required String bindingId,
    String? afterPathKey,
    int? limit,
    bool sourceDeletedOnly = false,
  }) async {
    final resp = await _callAsync('list_binding_catalog', <String, Object?>{
      'binding_id': bindingId,
      'after_path_key': afterPathKey,
      'limit': limit,
      'source_deleted_only': sourceDeletedOnly,
    });
    final list = resp['entries'] as List<dynamic>;
    return list
        .map((e) => CatalogEntryModel.fromJson(e as Map<String, dynamic>))
        .toList(growable: false);
  }

  @override
  Future<List<CatalogVersionModel>> listBindingVersions({
    required String bindingId,
    String? pathKey,
    int? afterArchivedAt,
    String? afterVersionId,
    int? limit,
  }) async {
    final resp = await _callAsync('list_binding_versions', <String, Object?>{
      'binding_id': bindingId,
      'path_key': pathKey,
      'after_archived_at': afterArchivedAt,
      'after_version_id': afterVersionId,
      'limit': limit,
    });
    final list = resp['versions'] as List<dynamic>;
    return list
        .map((e) => CatalogVersionModel.fromJson(e as Map<String, dynamic>))
        .toList(growable: false);
  }

  @override
  Future<RestoreCatalogVersionResultModel> restoreCatalogVersion({
    required String bindingId,
    required String versionId,
    required String targetDir,
  }) async {
    final resp = await _callAsync('restore_catalog_version', <String, Object?>{
      'binding_id': bindingId,
      'version_id': versionId,
      'target_dir': targetDir,
    });
    return RestoreCatalogVersionResultModel.fromJson(
      resp['result'] as Map<String, dynamic>,
    );
  }

  @override
  Future<OutboxMessageViewModel> enqueueMessageText({
    required List<String> targetDeviceIds,
    required String text,
  }) async {
    final resp = await _callAsync('enqueue_message_text', <String, Object?>{
      'target_device_ids': targetDeviceIds,
      'text': text,
    });
    return OutboxMessageViewModel.fromJson(
      resp['outbox'] as Map<String, dynamic>,
    );
  }

  @override
  Future<OutboxMessageViewModel> enqueueMessageFile({
    required List<String> targetDeviceIds,
    required String filePath,
  }) async {
    final resp = await _callAsync('enqueue_message_file', <String, Object?>{
      'target_device_ids': targetDeviceIds,
      'file_path': filePath,
    });
    return OutboxMessageViewModel.fromJson(
      resp['outbox'] as Map<String, dynamic>,
    );
  }

  @override
  Future<int> retryOutboxMessage(String clientMsgId) async {
    final resp = await _callAsync('retry_outbox_message', <String, Object?>{
      'client_msg_id': clientMsgId,
    });
    return (resp['reset_targets'] as num).toInt();
  }

  @override
  Future<List<OutboxMessageViewModel>> listOutboxMessages({int? limit}) async {
    final resp = await _callAsync('list_outbox_messages', <String, Object?>{
      'limit': limit,
    });
    final list = resp['messages'] as List<dynamic>;
    return list
        .map((e) => OutboxMessageViewModel.fromJson(e as Map<String, dynamic>))
        .toList(growable: false);
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
    SystemEnvHelper.windowsNetworkScanner ??= () => _ffi.scanWindowsNetworks();
    _environmentTimer = Timer.periodic(
      const Duration(seconds: 15),
      (_) => unawaited(_refreshDeviceEnvironment()),
    );
    _pairRequestTimer = Timer.periodic(
      const Duration(seconds: 2),
      (_) => unawaited(_pollIncomingPairRequests()),
    );
    unawaited(_refreshDeviceEnvironment());
    unawaited(_syncInitialServerStatus());
  }

  static const String appVersion = '0.2.0';
  static const int defaultServerPort = 7842;

  LanVaultRustFfi? _rustFfi;
  final AndroidKeepAliveChannel _keepAliveChannel;
  Timer? _environmentTimer;
  Timer? _pairRequestTimer;
  bool _envRefreshInFlight = false;
  bool _pairPollInFlight = false;
  bool _disposed = false;
  ({
    String vaultPath,
    int quotaGb,
    String webPassword,
    int port,
    bool elevated
  })? _lastServerConfig;

  /// 运行模式单一真相源，供 Shell 导航栏、Dashboard 与 Settings 实时联动。
  final ValueNotifier<RoleMode> roleModeNotifier;

  /// 本地存储服务运行状态单一真相源。
  final ValueNotifier<bool> serverRunningNotifier = ValueNotifier<bool>(false);

  /// 等待用户确认的入站配对请求列表。
  final ValueNotifier<List<PendingPairRequestModel>>
      pendingPairRequestsNotifier =
      ValueNotifier<List<PendingPairRequestModel>>(
    const <PendingPairRequestModel>[],
  );

  /// 已配对设备列表状态源，配对/解绑成功后自动刷新。
  final ValueNotifier<List<PairedPeerModel>> pairedPeersNotifier =
      ValueNotifier<List<PairedPeerModel>>(const <PairedPeerModel>[]);

  /// 断线自动重连开关状态源。
  final ValueNotifier<bool> autoReconnectNotifier = ValueNotifier<bool>(true);
  final ValueNotifier<int> serverStateVersion = ValueNotifier<int>(0);
  final ValueNotifier<String?> selectedPeerIdNotifier =
      ValueNotifier<String?>(null);
  final ValueNotifier<bool> fastBackupModeNotifier = ValueNotifier<bool>(false);

  PairedPeerModel? get selectedPeer {
    final peers = pairedPeersNotifier.value;
    if (peers.isEmpty) return null;
    final currentId = selectedPeerIdNotifier.value;
    if (currentId != null) {
      for (final p in peers) {
        if (p.deviceId == currentId) return p;
      }
    }
    for (final p in peers) {
      if (p.online) return p;
    }
    return peers.first;
  }

  void selectPeer(String? deviceId) {
    if (_disposed) return;
    if (selectedPeerIdNotifier.value != deviceId) {
      selectedPeerIdNotifier.value = deviceId;
    }
  }

  /// 将后端原始时间戳（秒/毫秒/ISO 字符串）格式化为简洁可读时间。
  static String formatRelativeTime(String raw) {
    final trimmed = raw.trim();
    if (trimmed.isEmpty) return '';
    DateTime? dt;
    final parsedInt = int.tryParse(trimmed);
    if (parsedInt != null) {
      final ms = parsedInt > 100000000000 ? parsedInt : parsedInt * 1000;
      dt = DateTime.fromMillisecondsSinceEpoch(ms).toLocal();
    } else {
      dt = DateTime.tryParse(trimmed)?.toLocal();
    }
    if (dt == null) return trimmed;
    final now = DateTime.now();
    final hh = dt.hour.toString().padLeft(2, '0');
    final mm = dt.minute.toString().padLeft(2, '0');
    if (dt.year == now.year && dt.month == now.month && dt.day == now.day) {
      return '$hh:$mm';
    }
    final mo = dt.month.toString().padLeft(2, '0');
    final dd = dt.day.toString().padLeft(2, '0');
    return '$mo-$dd $hh:$mm';
  }

  /// 将后端同步阻塞/状态英文短语翻译为自然中文。
  static String formatSyncBlockedReason(String rawStatus) {
    final lower = rawStatus.toLowerCase();
    if (lower.contains('notonwifi')) return '已暂停 · 等待连接 Wi-Fi';
    if (lower.contains('notcharging')) return '已暂停 · 等待连接电源';
    if (lower.contains('ssid')) return '已暂停 · 未连接到允许的家庭网络';
    if (lower.contains('paused')) return '已暂停自动备份';
    if (lower.contains('blocked')) return '已暂停 · 等待网络或电源条件满足';
    if (lower.contains('error') || lower.contains('fail'))
      return '同步遇到异常，正在等待重试';
    if (lower.contains('running') ||
        lower.contains('uploading') ||
        lower.contains('syncing')) {
      return '正在备份中';
    }
    if (lower.contains('completed') || lower.contains('done')) return '备份已完成';
    return '空闲待命';
  }

  Stream<SyncProgressEvent>? _sharedSyncStream;
  StreamSubscription<SyncProgressEvent>? _sharedSyncSub;
  final StreamController<SyncProgressEvent> _syncBroadcastController =
      StreamController<SyncProgressEvent>.broadcast();

  /// 将捆绑 `.woff2` 字体流通过 Rust 原生 Brotli 解码为 SFNT 字节供 Flutter `FontLoader` 加载。
  static Future<Uint8List> decodeWoff2ToSfnt(Uint8List woff2Bytes) {
    return Isolate.run(() => _decodeWoff2InWorker(woff2Bytes));
  }

  void _notifyStateChanged() {
    if (_disposed) return;
    serverStateVersion.value++;
  }

  Future<void> _syncInitialServerStatus() async {
    try {
      // 已配置过的机器在这里自动拉起存储服务：用户不必每次手动开启。
      final restoredPort = await _ffi.autostartLocalServer();
      if (_disposed) return;
      if (restoredPort != null) {
        serverRunningNotifier.value = true;
        _notifyStateChanged();
      }
      final overview = await _ffi.getSystemOverview();
      if (_disposed) return;
      serverRunningNotifier.value = overview.serverRunning;
    } catch (_) {
      // 自动启动失败不阻塞 UI；用户仍可在"存储"页手动启动。
    }
  }

  /// 打开/关闭"随软件启动自动开启存储服务"。
  Future<bool> getAutostartServer() => _ffi.getAutostartServer();

  Future<bool> setAutostartServer(bool enabled) async {
    final applied = await _ffi.setAutostartServer(enabled);
    if (!_disposed) _notifyStateChanged();
    return applied;
  }

  /// 供"存储"页在用户手动启动失败后重试自动恢复。
  Future<int?> autostartLocalServer() async {
    final port = await _ffi.autostartLocalServer();
    if (port != null && !_disposed) {
      serverRunningNotifier.value = true;
      _notifyStateChanged();
    }
    return port;
  }

  Future<void> _pollIncomingPairRequests() async {
    if (_disposed || _pairPollInFlight || !roleModeNotifier.value.hasServer) {
      return;
    }
    _pairPollInFlight = true;
    try {
      final requests = await _ffi.listPendingPairRequests();
      if (_disposed) return;
      pendingPairRequestsNotifier.value = requests;
    } catch (_) {
      // Ignore transient poll errors while server is stopped or restarting.
    } finally {
      _pairPollInFlight = false;
    }
  }

  /// 将底层 Rust/FFI 异常转换为通俗易懂的中文交互提示。
  static String formatUserError(Object error) {
    var msg = error.toString().trim();
    for (final prefix in const <String>[
      'Bad state: ',
      'StateError: ',
      'Exception: ',
      'Invalid argument(s): ',
      'RangeError: ',
      'FormatException: ',
    ]) {
      if (msg.startsWith(prefix)) {
        msg = msg.substring(prefix.length).trim();
      }
    }
    final colonIdx = msg.indexOf(' failed: ');
    if (msg.startsWith('FFI ') && colonIdx > 0) {
      msg = msg.substring(colonIdx + ' failed: '.length).trim();
    }
    final lower = msg.toLowerCase();
    if (lower.contains('cannot start local server in client_only mode') ||
        lower.contains('cannot start local server in `client_only` mode')) {
      return '当前处于“仅客户端”模式，请先切换为“双端模式”或“仅服务端”后再启动存储服务';
    }
    if (lower.contains('local server is not running') ||
        lower.contains('local server database is not initialized')) {
      return '请先启动本机存储服务后再新建存储池';
    }
    if (lower.contains('mount path overlaps existing volume')) {
      return '所选挂载目录与已有存储池目录存在包含或重叠，请新建子目录或选择其他独立路径';
    }
    if (lower.contains('vault name must not be empty')) {
      return '请输入存储池名称';
    }
    if (lower.contains('peer rejected pairing request')) {
      return '对方已拒绝本次配对请求';
    }
    if (lower.contains('pairing request timed out') ||
        lower.contains('pairing request expired')) {
      return '配对请求已超时，请确认对方设备保持在线后重试';
    }
    if (lower.contains('too_many_pending_pair_requests')) {
      return '对方待确认配对请求过多，请稍后再试';
    }
    if (lower.contains('invalid password') || lower.contains('locked_out')) {
      return '管理密码不正确或验证次数过多，请稍后再试';
    }
    if (lower.contains('connection refused') || lower.contains('timed out')) {
      return '无法连接目标设备，请确认两台设备在同一局域网且服务端已启动';
    }
    if (lower.contains('no paired peer session')) {
      return '当前尚未连接已配对设备，请先在“设备”页完成配对';
    }
    if (lower.contains('address already in use')) {
      return '端口 $defaultServerPort 已被占用，请检查是否已有后台实例正在运行';
    }
    return msg.isEmpty ? '操作未完成，请稍后重试' : msg;
  }

  RoleMode get currentRoleMode => roleModeNotifier.value;

  LanVaultRustFfi get _ffi => _rustFfi ??= NativeLanVaultRustFfi();

  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _environmentTimer?.cancel();
    _environmentTimer = null;
    _pairRequestTimer?.cancel();
    _pairRequestTimer = null;
    unawaited(_sharedSyncSub?.cancel());
    _sharedSyncSub = null;
    unawaited(_syncBroadcastController.close());
    roleModeNotifier.dispose();
    serverRunningNotifier.dispose();
    pendingPairRequestsNotifier.dispose();
    pairedPeersNotifier.dispose();
    autoReconnectNotifier.dispose();
    SystemEnvHelper.windowsNetworkScanner = null;
    serverStateVersion.dispose();
    selectedPeerIdNotifier.dispose();
    fastBackupModeNotifier.dispose();
  }

  Future<({bool onWifi, String? currentSsid, bool isCharging})>
      _readPlatformDeviceEnvironment() async {
    if (Platform.isWindows) {
      final scan = await _ffi.scanWindowsNetworks();
      return (
        onWifi: scan.hasWifiAdapter && scan.currentSsid != null,
        currentSsid: scan.currentSsid,
        isCharging: true,
      );
    }
    return _keepAliveChannel.getDeviceEnvironment();
  }

  Future<void> _refreshDeviceEnvironment() async {
    if (_disposed || _envRefreshInFlight) return;
    _envRefreshInFlight = true;
    try {
      final environment = await _readPlatformDeviceEnvironment();
      if (_disposed) return;
      await _ffi.updateDeviceEnvironment(
        onWifi: environment.onWifi,
        currentSsid: environment.currentSsid,
        isCharging: environment.isCharging,
      );
    } catch (err, st) {
      debugPrint('EveryoneNAS _refreshDeviceEnvironment failed: $err');
      if (!_disposed) {
        unawaited(
          _ffi
              .reportClientLog(
                level: 'WARN',
                source: 'bridge.device_env',
                message: '刷新设备网络环境失败: $err',
                detail: st.toString(),
              )
              .catchError((Object _) {}),
        );
      }
    } finally {
      _envRefreshInFlight = false;
    }
  }

  // --- 运行模式管理（三态切换） ---

  Future<RoleMode> getRoleMode() async {
    final mode = await _ffi.getRoleMode();
    if (!_disposed) {
      roleModeNotifier.value = mode;
    }
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
        if (!_disposed) {
          roleModeNotifier.value = reverted;
        }
        throw StateError('后台服务未能停止，运行模式已回滚到 ${reverted.wireValue}。');
      }
    } else if (Platform.isAndroid && !previous.hasServer && applied.hasServer) {
      await _keepAliveChannel.detectKeepAliveLevel();
    }

    if (!_disposed) {
      roleModeNotifier.value = applied;
      if (!applied.hasServer) {
        serverRunningNotifier.value = false;
      }
      _notifyStateChanged();
    }
    return applied;
  }

  // --- 核心 FFI 接口 ---

  Stream<DiscoveredPeer> startMdnsScan() => _ffi.startMdnsScan();

  Future<String> pairWithPin({
    required String ip,
    required int port,
    required String pin,
  }) async {
    if (!RegExp(r'^\d{6}$').hasMatch(pin)) {
      throw ArgumentError.value(pin, 'pin', 'PIN 必须为 6 位数字');
    }
    if (port <= 0 || port > 65535) {
      throw RangeError.range(port, 1, 65535, 'port');
    }
    final pairedId = await _ffi.pairWithPin(ip: ip, port: port, pin: pin);
    unawaited(refreshPairedPeers()
        .catchError((Object _) => const <PairedPeerModel>[]));
    return pairedId;
  }

  Future<String> requestPairing({
    required String ip,
    required int port,
  }) async {
    if (ip.trim().isEmpty) {
      throw ArgumentError.value(ip, 'ip', '对端 IP 地址不能为空');
    }
    if (port <= 0 || port > 65535) {
      throw RangeError.range(port, 1, 65535, 'port');
    }
    final pairedId = await _ffi.requestPairing(ip: ip.trim(), port: port);
    unawaited(refreshPairedPeers()
        .catchError((Object _) => const <PairedPeerModel>[]));
    _notifyStateChanged();
    return pairedId;
  }

  Future<List<PendingPairRequestModel>> listPendingPairRequests() async {
    final list = await _ffi.listPendingPairRequests();
    if (!_disposed) {
      pendingPairRequestsNotifier.value = list;
    }
    return list;
  }

  Future<bool> respondPairRequest({
    required String requestId,
    required bool accept,
  }) async {
    final accepted = await _ffi.respondPairRequest(
      requestId: requestId,
      accept: accept,
    );
    if (!_disposed) {
      pendingPairRequestsNotifier.value = pendingPairRequestsNotifier.value
          .where((item) => item.requestId != requestId)
          .toList(growable: false);
      if (accepted) {
        unawaited(refreshPairedPeers()
            .catchError((Object _) => const <PairedPeerModel>[]));
        _notifyStateChanged();
      }
    }
    return accepted;
  }

  Future<void> enableAutoReconnect(bool enabled) async {
    await _ffi.enableAutoReconnect(enabled);
    if (!_disposed) {
      autoReconnectNotifier.value = enabled;
    }
  }

  Future<bool> getAutoReconnect() async {
    final enabled = await _ffi.getAutoReconnect();
    if (!_disposed) {
      autoReconnectNotifier.value = enabled;
    }
    return enabled;
  }

  Future<List<PairedPeerModel>> refreshPairedPeers() async {
    final peers = await _ffi.listPairedPeers();
    if (!_disposed) {
      pairedPeersNotifier.value = peers;
      final currentId = selectedPeerIdNotifier.value;
      final exists =
          currentId != null && peers.any((p) => p.deviceId == currentId);
      if (!exists) {
        PairedPeerModel? fallback;
        for (final p in peers) {
          if (p.online) {
            fallback = p;
            break;
          }
        }
        fallback ??= peers.isNotEmpty ? peers.first : null;
        selectedPeerIdNotifier.value = fallback?.deviceId;
      }
    }
    return peers;
  }

  Future<PowerNetworkPolicyModel> getPowerNetworkPolicy() =>
      _ffi.getPowerNetworkPolicy();

  Future<BackupStorageConfigModel> getBackupStorageConfig() =>
      _ffi.getBackupStorageConfig();

  Future<BackupStorageConfigModel> setBackupStorageConfig({
    required String primaryDir,
    String? fallbackDir,
  }) =>
      _ffi.setBackupStorageConfig(
        primaryDir: primaryDir,
        fallbackDir: fallbackDir,
      );

  Future<({bool onWifi, String? currentSsid, bool isCharging})>
      getDeviceEnvironment() => _readPlatformDeviceEnvironment();

  Future<LastServerConfig?> getLastServerConfig() async {
    if (Platform.isAndroid) {
      final androidSaved = await _keepAliveChannel.getLastServerConfig();
      if (androidSaved != null) {
        return androidSaved;
      }
    }
    return _ffi.getLastServerConfig();
  }

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
    if (port <= 0 || port > 65535) {
      throw RangeError.range(port, 1, 65535, 'port');
    }
    if (webPassword.length < 8) {
      throw ArgumentError.value(webPassword, 'webPassword', 'Web 管理密码至少 8 位');
    }
    final environment = await _readPlatformDeviceEnvironment();
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
        if (!_disposed) {
          serverRunningNotifier.value = true;
          _notifyStateChanged();
        }
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
        skipNativeStart: true,
      );
      _lastServerConfig = (
        vaultPath: trimmedPath,
        quotaGb: quotaGb,
        webPassword: webPassword,
        port: boundPort,
        elevated: false,
      );
      if (!_disposed) {
        serverRunningNotifier.value = true;
        _notifyStateChanged();
      }
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
    if (!_disposed) {
      serverRunningNotifier.value = true;
      _notifyStateChanged();
    }
    return boundPort;
  }

  Future<void> prepareGracefulRestart() => _ffi.prepareGracefulRestart();

  Stream<SyncProgressEvent> subscribeSyncProgress() {
    if (_sharedSyncStream == null) {
      _sharedSyncSub = _ffi.subscribeSyncProgress().listen(
        (ev) {
          if (!_syncBroadcastController.isClosed) {
            _syncBroadcastController.add(ev);
          }
        },
        onError: (Object err, StackTrace st) {
          if (!_syncBroadcastController.isClosed) {
            _syncBroadcastController.addError(err, st);
          }
        },
      );
      _sharedSyncStream = _syncBroadcastController.stream;
    }
    return _sharedSyncStream!;
  }

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
      targetVaultId:
          targetVaultId.trim().isEmpty ? 'default' : targetVaultId.trim(),
      enabled: enabled,
    );
  }

  Future<bool> deleteSyncRule(String ruleId) async {
    if (ruleId.trim().isEmpty) {
      throw ArgumentError.value(ruleId, 'ruleId', '规则 ID 不能为空');
    }
    final res = await _ffi.deleteSyncRule(ruleId.trim());
    _notifyStateChanged();
    return res;
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
    if (maxKbps < 0 || maxKbps > 0xFFFFFFFF) {
      throw RangeError.range(maxKbps, 0, 0xFFFFFFFF, 'maxKbps');
    }
    final environment = await _readPlatformDeviceEnvironment();
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

  // --- 新增 9 个 WebUI 端内整合能力 ---

  Future<SystemOverviewModel> getSystemOverview() => _ffi.getSystemOverview();

  Future<List<VaultSummaryModel>> listVaults() => _ffi.listVaults();

  Future<VaultSummaryModel> createVault({
    required String name,
    required String mountPath,
    required String poolMode,
    required int quotaGb,
    required bool e2eeEnabled,
    String? extraMountPath,
  }) async {
    if (name.trim().isEmpty) {
      throw ArgumentError.value(name, 'name', '存储池名称不能为空');
    }
    if (mountPath.trim().isEmpty) {
      throw ArgumentError.value(mountPath, 'mountPath', '挂载路径不能为空');
    }
    if (quotaGb <= 0) {
      throw RangeError.value(quotaGb, 'quotaGb', '存储配额必须大于 0 GB');
    }
    final created = await _ffi.createVault(
      name: name.trim(),
      mountPath: mountPath.trim(),
      poolMode: poolMode.trim(),
      quotaGb: quotaGb,
      e2eeEnabled: e2eeEnabled,
      extraMountPath: extraMountPath != null && extraMountPath.trim().isNotEmpty
          ? extraMountPath.trim()
          : null,
    );
    return created;
  }

  Future<List<SnapshotSummaryModel>> listSnapshots({String? vaultId}) =>
      _ffi.listSnapshots(vaultId: vaultId);

  Future<BitRotScrubResultModel> triggerBitRotScrub() =>
      _ffi.triggerBitRotScrub();

  Future<bool> revokePairedPeer(String deviceId) async {
    if (deviceId.trim().isEmpty) {
      throw ArgumentError.value(deviceId, 'deviceId', '设备 ID 不能为空');
    }
    final revoked = await _ffi.revokePairedPeer(deviceId.trim());
    if (revoked) {
      unawaited(refreshPairedPeers()
          .catchError((Object _) => const <PairedPeerModel>[]));
      _notifyStateChanged();
    }
    return revoked;
  }

  Future<PairingInfoModel> getPairingInfo() => _ffi.getPairingInfo();

  Future<bool> toggleSyncRule({
    required String ruleId,
    required bool enabled,
  }) {
    if (ruleId.trim().isEmpty) {
      throw ArgumentError.value(ruleId, 'ruleId', '规则 ID 不能为空');
    }
    return _ffi.toggleSyncRule(ruleId: ruleId.trim(), enabled: enabled);
  }

  Future<bool> pushClipboardFile({
    String? targetDeviceId,
    required String filePath,
  }) {
    final trimmed = filePath.trim();
    if (trimmed.isEmpty) {
      throw ArgumentError.value(filePath, 'filePath', '文件路径不能为空');
    }
    return _ffi.pushClipboardFile(
      targetDeviceId: targetDeviceId,
      filePath: trimmed,
    );
  }

  Future<RestoreSnapshotResultModel> restoreSnapshot({
    required String snapshotId,
    required String targetDir,
  }) {
    if (snapshotId.trim().isEmpty || targetDir.trim().isEmpty) {
      throw ArgumentError('snapshotId 与 targetDir 不能为空');
    }
    return _ffi.restoreSnapshot(
      snapshotId: snapshotId.trim(),
      targetDir: targetDir.trim(),
    );
  }

  Future<List<TransferHistoryModel>> listTransferHistory({int limit = 50}) =>
      _ffi.listTransferHistory(limit: limit);

  Future<bool> setSyncPaused(bool paused) => _ffi.setSyncPaused(paused);

  Future<bool> setFastBackupMode(bool enabled) async {
    if (_disposed) return fastBackupModeNotifier.value;
    final prev = fastBackupModeNotifier.value;
    fastBackupModeNotifier.value = enabled;
    try {
      final applied = await _ffi.setFastBackupMode(enabled);
      if (!_disposed) fastBackupModeNotifier.value = applied;
      return applied;
    } catch (err) {
      if (!_disposed) fastBackupModeNotifier.value = prev;
      rethrow;
    }
  }

  Future<bool> getSyncPaused() => _ffi.getSyncPaused();

  Future<bool> setBitRotAutoScrub(bool enabled) =>
      _ffi.setBitRotAutoScrub(enabled);

  Future<bool> getBitRotAutoScrub() => _ffi.getBitRotAutoScrub();

  Future<String> setSyncRuleSchedule({
    required String ruleId,
    required String scheduleMode,
  }) =>
      _ffi.setSyncRuleSchedule(
        ruleId: ruleId.trim(),
        scheduleMode: scheduleMode.trim(),
      );

  // --- 双端服务生命周期与权限通道 ---

  Future<KeepAliveLevel> detectKeepAliveLevel() =>
      _keepAliveChannel.detectKeepAliveLevel();

  Future<bool> stopKeepAliveServer() async {
    if (Platform.isAndroid) {
      final stopped = await _keepAliveChannel.stopKeepAliveServer();
      if (stopped && !_disposed) {
        _lastServerConfig = null;
        serverRunningNotifier.value = false;
        _notifyStateChanged();
      }
      return stopped;
    }
    await _ffi.prepareGracefulRestart();
    if (!_disposed) {
      _lastServerConfig = null;
      serverRunningNotifier.value = false;
      _notifyStateChanged();
    }
    return true;
  }

  Future<bool> stopLocalServer() => stopKeepAliveServer();

  Future<bool> executeGracefulRestart() async {
    var config = _lastServerConfig;
    if (config == null) {
      final saved = await getLastServerConfig();
      if (saved != null) {
        final elevated = Platform.isAndroid
            ? (await _keepAliveChannel.detectKeepAliveLevel())
                .isElevatedAndroidDaemon
            : false;
        config = (
          vaultPath: saved.vaultPath,
          quotaGb: saved.quotaGb,
          webPassword: saved.webPassword ?? '',
          port: saved.port,
          elevated: elevated,
        );
      }
    }
    if (config == null || (!config.elevated && config.webPassword.length < 8)) {
      throw StateError('请先在“存储”页输入管理密码并启动服务后再重启。');
    }
    if (config.elevated) {
      final restarted = await _keepAliveChannel.gracefulRestartServer();
      if (restarted && !_disposed) {
        serverRunningNotifier.value = true;
        _notifyStateChanged();
      }
      return restarted;
    }
    await _ffi.prepareGracefulRestart();
    final boundPort = await _ffi.startLocalServer(
      vaultPath: config.vaultPath,
      quotaGb: config.quotaGb,
      webPassword: config.webPassword,
      port: config.port,
    );
    if (Platform.isAndroid) {
      await _keepAliveChannel.startKeepAliveServer(
        vaultPath: config.vaultPath,
        quotaGb: config.quotaGb,
        port: boundPort,
        webPassword: config.webPassword,
        skipNativeStart: true,
      );
    }
    _lastServerConfig = (
      vaultPath: config.vaultPath,
      quotaGb: config.quotaGb,
      webPassword: config.webPassword,
      port: boundPort,
      elevated: false,
    );
    if (!_disposed) {
      serverRunningNotifier.value = true;
      _notifyStateChanged();
    }
    return true;
  }

  Future<bool> requestBatteryExemption() =>
      _keepAliveChannel.requestBatteryExemption();

  Future<bool> requestStoragePermission() =>
      _keepAliveChannel.requestStoragePermission();

  Future<bool> requestLocationPermission() =>
      _keepAliveChannel.requestLocationPermission();

  Future<bool> requestNotificationPermission() =>
      _keepAliveChannel.requestNotificationPermission();

  Future<String?> pickDirectory({String description = '选择文件夹'}) =>
      SystemEnvHelper.pickDirectory(description: description);

  Future<String?> pickFile({String title = '选择要发送的文件'}) =>
      SystemEnvHelper.pickFile(title: title);

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

  Future<String?> getDebugLogPath() => _ffi.getDebugLogPath();

  Future<Map<String, dynamic>> runSelfDiagnostics() =>
      _ffi.runSelfDiagnostics();

  // --- v2 设备组、组规则、目录归档与非阻塞发件箱 ---

  Future<List<DeviceGroupViewModel>> listDeviceGroups() =>
      _ffi.listDeviceGroups();

  Future<DeviceGroupModel> createDeviceGroup(String name) async {
    final trimmed = name.trim();
    if (trimmed.isEmpty) {
      throw ArgumentError.value(name, 'name', '设备组名称不能为空');
    }
    final created = await _ffi.createDeviceGroup(trimmed);
    _notifyStateChanged();
    return created;
  }

  Future<DeviceGroupModel> renameDeviceGroup({
    required String groupId,
    required String name,
  }) async {
    final gid = groupId.trim();
    final trimmed = name.trim();
    if (gid.isEmpty || trimmed.isEmpty) {
      throw ArgumentError('设备组 ID 与新名称不能为空');
    }
    final renamed = await _ffi.renameDeviceGroup(groupId: gid, name: trimmed);
    _notifyStateChanged();
    return renamed;
  }

  Future<void> deleteDeviceGroup(String groupId) async {
    final gid = groupId.trim();
    if (gid.isEmpty) {
      throw ArgumentError.value(groupId, 'groupId', '设备组 ID 不能为空');
    }
    await _ffi.deleteDeviceGroup(gid);
    _notifyStateChanged();
  }

  Future<void> addGroupMember({
    required String groupId,
    required String deviceId,
    String? role,
  }) async {
    final gid = groupId.trim();
    final did = deviceId.trim();
    if (gid.isEmpty || did.isEmpty) {
      throw ArgumentError('设备组 ID 与设备 ID 不能为空');
    }
    await _ffi.addGroupMember(groupId: gid, deviceId: did, role: role);
    _notifyStateChanged();
  }

  Future<void> removeGroupMember({
    required String groupId,
    required String deviceId,
  }) async {
    final gid = groupId.trim();
    final did = deviceId.trim();
    if (gid.isEmpty || did.isEmpty) {
      throw ArgumentError('设备组 ID 与设备 ID 不能为空');
    }
    await _ffi.removeGroupMember(groupId: gid, deviceId: did);
    _notifyStateChanged();
  }

  Future<List<GroupRuleViewModel>> listGroupRules({String? groupId}) =>
      _ffi.listGroupRules(groupId: groupId);

  Future<GroupSyncRuleModel> upsertGroupRule({
    String? ruleId,
    required String groupId,
    required String name,
    required String mode,
    String? sourceDeviceId,
    required String sourceRoot,
    String? sourceRootId,
    int archiveRetentionDays = 30,
    bool wifiOnly = true,
    String? allowedSsids,
    bool requireCharging = false,
    int maxKbps = 0,
    int scheduleIntervalSecs = 0,
    bool enabled = true,
  }) async {
    if (groupId.trim().isEmpty || sourceRoot.trim().isEmpty) {
      throw ArgumentError('设备组与源目录均不能为空');
    }
    final saved = await _ffi.upsertGroupRule(
      ruleId: ruleId?.trim(),
      groupId: groupId.trim(),
      name: name.trim(),
      mode: mode.trim(),
      sourceDeviceId: sourceDeviceId?.trim(),
      sourceRoot: sourceRoot.trim(),
      sourceRootId: sourceRootId?.trim(),
      archiveRetentionDays: archiveRetentionDays,
      wifiOnly: wifiOnly,
      allowedSsids: allowedSsids,
      requireCharging: requireCharging,
      maxKbps: maxKbps,
      scheduleIntervalSecs: scheduleIntervalSecs,
      enabled: enabled,
    );
    _notifyStateChanged();
    return saved;
  }

  Future<void> deleteGroupRule(String ruleId) async {
    final rid = ruleId.trim();
    if (rid.isEmpty) {
      throw ArgumentError.value(ruleId, 'ruleId', '规则 ID 不能为空');
    }
    await _ffi.deleteGroupRule(rid);
    _notifyStateChanged();
  }

  Future<void> toggleGroupRule({
    required String ruleId,
    required bool enabled,
  }) async {
    final rid = ruleId.trim();
    if (rid.isEmpty) {
      throw ArgumentError.value(ruleId, 'ruleId', '规则 ID 不能为空');
    }
    await _ffi.toggleGroupRule(ruleId: rid, enabled: enabled);
    _notifyStateChanged();
  }

  Future<RuleTargetBindingModel> upsertRuleTargetBinding({
    String? bindingId,
    required String ruleId,
    required String targetDeviceId,
    String? targetVaultId,
    String? mirrorRootId,
    String? targetRoot,
    bool enabled = true,
  }) async {
    if (ruleId.trim().isEmpty || targetDeviceId.trim().isEmpty) {
      throw ArgumentError('规则 ID 与目标设备不能为空');
    }
    final binding = await _ffi.upsertRuleTargetBinding(
      bindingId: bindingId?.trim(),
      ruleId: ruleId.trim(),
      targetDeviceId: targetDeviceId.trim(),
      targetVaultId: targetVaultId?.trim(),
      mirrorRootId: mirrorRootId?.trim(),
      targetRoot: targetRoot?.trim(),
      enabled: enabled,
    );
    _notifyStateChanged();
    return binding;
  }

  Future<void> disableRuleTargetBinding(String bindingId) async {
    final bid = bindingId.trim();
    if (bid.isEmpty) {
      throw ArgumentError.value(bindingId, 'bindingId', '绑定 ID 不能为空');
    }
    await _ffi.disableRuleTargetBinding(bid);
    _notifyStateChanged();
  }

  Future<MirrorRootModel> registerMirrorRoot({
    required String canonicalPath,
    String displayName = '',
  }) async {
    final path = canonicalPath.trim();
    if (path.isEmpty) {
      throw ArgumentError.value(canonicalPath, 'canonicalPath', '镜像目录路径不能为空');
    }
    final root = await _ffi.registerMirrorRoot(
      canonicalPath: path,
      displayName: displayName.trim(),
    );
    _notifyStateChanged();
    return root;
  }

  Future<List<MirrorRootModel>> listMirrorRoots() => _ffi.listMirrorRoots();

  Future<void> revokeMirrorRoot(String rootId) async {
    final rid = rootId.trim();
    if (rid.isEmpty) {
      throw ArgumentError.value(rootId, 'rootId', '镜像根目录 ID 不能为空');
    }
    await _ffi.revokeMirrorRoot(rid);
    _notifyStateChanged();
  }

  Future<List<MirrorPreviewModel>> previewGroupRule(String ruleId) {
    final rid = ruleId.trim();
    if (rid.isEmpty) {
      throw ArgumentError.value(ruleId, 'ruleId', '规则 ID 不能为空');
    }
    return _ffi.previewGroupRule(rid);
  }

  Future<GroupRuleSyncOutcomeModel> triggerGroupRuleSync(String ruleId) {
    final rid = ruleId.trim();
    if (rid.isEmpty) {
      throw ArgumentError.value(ruleId, 'ruleId', '规则 ID 不能为空');
    }
    return _ffi.triggerGroupRuleSync(rid);
  }

  Future<List<CatalogEntryModel>> listBindingCatalog({
    required String bindingId,
    String? afterPathKey,
    int? limit,
    bool sourceDeletedOnly = false,
  }) {
    final bid = bindingId.trim();
    if (bid.isEmpty) {
      throw ArgumentError.value(bindingId, 'bindingId', '绑定 ID 不能为空');
    }
    return _ffi.listBindingCatalog(
      bindingId: bid,
      afterPathKey: afterPathKey,
      limit: limit,
      sourceDeletedOnly: sourceDeletedOnly,
    );
  }

  Future<List<CatalogVersionModel>> listBindingVersions({
    required String bindingId,
    String? pathKey,
    int? afterArchivedAt,
    String? afterVersionId,
    int? limit,
  }) {
    final bid = bindingId.trim();
    if (bid.isEmpty) {
      throw ArgumentError.value(bindingId, 'bindingId', '绑定 ID 不能为空');
    }
    return _ffi.listBindingVersions(
      bindingId: bid,
      pathKey: pathKey,
      afterArchivedAt: afterArchivedAt,
      afterVersionId: afterVersionId,
      limit: limit,
    );
  }

  Future<RestoreCatalogVersionResultModel> restoreCatalogVersion({
    required String bindingId,
    required String versionId,
    required String targetDir,
  }) {
    if (bindingId.trim().isEmpty ||
        versionId.trim().isEmpty ||
        targetDir.trim().isEmpty) {
      throw ArgumentError('绑定 ID、版本 ID 与恢复目标目录均不能为空');
    }
    return _ffi.restoreCatalogVersion(
      bindingId: bindingId.trim(),
      versionId: versionId.trim(),
      targetDir: targetDir.trim(),
    );
  }

  Future<OutboxMessageViewModel> enqueueMessageText({
    required List<String> targetDeviceIds,
    required String text,
  }) {
    if (targetDeviceIds.isEmpty) {
      throw ArgumentError.value(
          targetDeviceIds, 'targetDeviceIds', '至少选择一个目标设备');
    }
    if (text.isEmpty) {
      throw ArgumentError.value(text, 'text', '发送内容不能为空');
    }
    return _ffi.enqueueMessageText(
      targetDeviceIds: targetDeviceIds,
      text: text,
    );
  }

  Future<OutboxMessageViewModel> enqueueMessageFile({
    required List<String> targetDeviceIds,
    required String filePath,
  }) {
    if (targetDeviceIds.isEmpty) {
      throw ArgumentError.value(
          targetDeviceIds, 'targetDeviceIds', '至少选择一个目标设备');
    }
    final trimmed = filePath.trim();
    if (trimmed.isEmpty) {
      throw ArgumentError.value(filePath, 'filePath', '文件路径不能为空');
    }
    return _ffi.enqueueMessageFile(
      targetDeviceIds: targetDeviceIds,
      filePath: trimmed,
    );
  }

  Future<int> retryOutboxMessage(String clientMsgId) {
    final id = clientMsgId.trim();
    if (id.isEmpty) {
      throw ArgumentError.value(clientMsgId, 'clientMsgId', '消息 ID 不能为空');
    }
    return _ffi.retryOutboxMessage(id);
  }

  Future<List<OutboxMessageViewModel>> listOutboxMessages({int? limit}) =>
      _ffi.listOutboxMessages(limit: limit);
}
