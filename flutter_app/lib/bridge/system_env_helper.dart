import 'dart:io';

import 'package:flutter/services.dart';

/// 平台网络扫描结果（区分有线连接与 Wi-Fi 列表）。
class NetworkScanSnapshot {
  const NetworkScanSnapshot({
    required this.hasWifiAdapter,
    required this.isWiredEthernet,
    required this.currentSsid,
    required this.availableSsids,
  });

  final bool hasWifiAdapter;
  final bool isWiredEthernet;
  final String? currentSsid;
  final List<String> availableSsids;
}

/// 平台级系统辅助工具：
/// - Windows 原生 COM IFileOpenDialog 目录/文件选择器与 Android SAF 选择器（零 PowerShell 启动耗时与弹窗）
/// - 盘符/卷根提取（与 Rust `normalized_drive_key()` 严格对齐）
/// - Windows Wi-Fi 列表扫描与有线网络检测（通过 Rust FFI + CREATE_NO_WINDOW 隐藏控制台）
/// - 统一字节容量格式化
abstract final class SystemEnvHelper {
  static const MethodChannel _keepAliveChannel =
      MethodChannel('com.lanvault.app/keep_alive');

  /// 由 [LanVaultBridge] 注入的 Windows 无窗口网络扫描器，避免 Dart 直接拉起 cmd/netsh 闪窗。
  static Future<NetworkScanSnapshot> Function()? windowsNetworkScanner;

  /// 统一字节大小格式化（B / KB / MB / GB / TB）。
  static String formatBytes(int bytes) {
    if (bytes <= 0) return '0 B';
    const kb = 1024.0;
    const mb = kb * 1024.0;
    const gb = mb * 1024.0;
    const tb = gb * 1024.0;
    if (bytes >= tb) {
      return '${(bytes / tb).toStringAsFixed(2)} TB';
    }
    if (bytes >= gb) {
      return '${(bytes / gb).toStringAsFixed(2)} GB';
    }
    if (bytes >= mb) {
      return '${(bytes / mb).toStringAsFixed(1)} MB';
    }
    if (bytes >= kb) {
      return '${(bytes / kb).toStringAsFixed(1)} KB';
    }
    return '$bytes B';
  }

  /// 提取路径对应的存储卷标识（与 Rust `normalized_drive_key()` 100% 对齐）。
  static String? extractDriveKey(String rawPath) {
    var trimmed = rawPath.trim();
    if (trimmed.isEmpty) return null;

    if (trimmed.startsWith(r'\\?\UNC\') || trimmed.startsWith(r'\\?\unc\')) {
      trimmed = r'\\' + trimmed.substring(8);
    } else if (trimmed.startsWith(r'\\?\')) {
      trimmed = trimmed.substring(4);
    }

    final winDrive = RegExp(r'^([a-zA-Z]):').firstMatch(trimmed);
    if (winDrive != null) {
      return '${winDrive.group(1)!.toUpperCase()}:';
    }

    if (trimmed.startsWith(r'\\')) {
      final parts = trimmed
          .substring(2)
          .split('\\')
          .where((p) => p.isNotEmpty)
          .toList(growable: false);
      if (parts.length >= 2) {
        return r'\\' '${parts[0].toLowerCase()}\\${parts[1].toLowerCase()}';
      }
    }

    if (trimmed.startsWith('/')) {
      final parts = trimmed
          .split('/')
          .where((p) => p.isNotEmpty)
          .toList(growable: false);
      if (parts.isEmpty) return '/';
      if (parts.first == 'storage') {
        if (parts.length >= 3 && parts[1] == 'emulated') {
          return '/storage/emulated/${parts[2]}';
        }
        if (parts.length >= 2) {
          return '/storage/${parts[1]}';
        }
      }
      return '/${parts.first}';
    }

    return null;
  }

  /// 提取路径末级目录或文件名，作为简洁展示标题。
  static String folderDisplayName(String rawPath) {
    final trimmed = rawPath.trim().replaceAll(RegExp(r'[\\/]+$'), '');
    if (trimmed.isEmpty) return rawPath;
    final segments = trimmed.split(RegExp(r'[\\/]'));
    final last = segments.lastWhere((s) => s.isNotEmpty, orElse: () => trimmed);
    return last;
  }

  /// 调用平台原生文件夹选择器：
  /// - Windows：通过 C++ Runner COM `IFileOpenDialog` MethodChannel（毫秒级弹出，无 PowerShell）
  /// - Android：通过 `com.lanvault.app/keep_alive` 的 `pickDirectory` MethodChannel（SAF）
  static Future<String?> pickDirectory({String description = '选择文件夹'}) async {
    if (!Platform.isAndroid && !Platform.isWindows) return null;
    final picked = await _keepAliveChannel.invokeMethod<String>(
      'pickDirectory',
      Platform.isWindows ? <String, Object>{'title': description} : null,
    );
    if (picked == null) return null;
    final trimmed = picked.trim();
    return trimmed.isEmpty ? null : trimmed;
  }

  /// 调用平台原生文件选择器（Windows 使用原生 COM `IFileOpenDialog`，Android 使用 SAF）。
  static Future<String?> pickFile({String title = '选择要发送的文件'}) async {
    if (!Platform.isAndroid && !Platform.isWindows) return null;
    final picked = await _keepAliveChannel.invokeMethod<String>(
      'pickFile',
      Platform.isWindows ? <String, Object>{'title': title} : null,
    );
    if (picked == null) return null;
    final trimmed = picked.trim();
    return trimmed.isEmpty ? null : trimmed;
  }

  /// 扫描当前可见 Wi-Fi 网络并判断是否通过网线连接。
  static Future<NetworkScanSnapshot> scanNetworks({
    String? fallbackCurrentSsid,
  }) async {
    if (!Platform.isWindows) {
      final list = <String>[
        if (fallbackCurrentSsid != null && fallbackCurrentSsid.trim().isNotEmpty)
          fallbackCurrentSsid.trim(),
      ];
      return NetworkScanSnapshot(
        hasWifiAdapter: true,
        isWiredEthernet: false,
        currentSsid: fallbackCurrentSsid,
        availableSsids: list,
      );
    }

    final scanner = windowsNetworkScanner;
    if (scanner != null) {
      return scanner();
    }

    return NetworkScanSnapshot(
      hasWifiAdapter: false,
      isWiredEthernet: true,
      currentSsid: fallbackCurrentSsid,
      availableSsids: const <String>[],
    );
  }
}
