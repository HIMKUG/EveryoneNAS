import 'dart:convert';
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
/// - Windows 原生目录/文件选择器与 Android SAF 目录选择器
/// - 盘符/卷根提取（与 Rust `normalized_drive_key()` 严格对齐）
/// - Windows Wi-Fi 列表扫描与有线网络检测
/// - 统一字节容量格式化
abstract final class SystemEnvHelper {
  static const MethodChannel _keepAliveChannel =
      MethodChannel('com.lanvault.app/keep_alive');
  static const Utf8Codec _lenientUtf8 = Utf8Codec(allowMalformed: true);

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

  /// 提取路径对应的存储卷标识（与 Rust `normalized_drive_key()` 100% 对齐）：
  /// - Windows：剥离 `\\?\` 后提取大写盘符 `C:`，或 UNC `\\host\share`
  /// - Android：`/storage/emulated/<user>/...` -> `/storage/emulated/<user>`；
  ///   `/storage/<UUID>/...` -> `/storage/<UUID>`
  /// - 其他 Unix：取顶层挂载点 `/<first>`
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
  /// - Windows：通过 PowerShell FolderBrowserDialog（显式 UTF-8 编码避免中文路径乱码）
  /// - Android：通过 `com.lanvault.app/keep_alive` 的 `pickDirectory` MethodChannel（SAF）
  static Future<String?> pickDirectory({String description = '选择文件夹'}) async {
    if (Platform.isAndroid) {
      final picked = await _keepAliveChannel.invokeMethod<String>(
        'pickDirectory',
      );
      if (picked == null) return null;
      final trimmed = picked.trim();
      return trimmed.isEmpty ? null : trimmed;
    }

    if (!Platform.isWindows) return null;
    final escapedDesc = description.replaceAll("'", "''");
    final script = '''
Add-Type -AssemblyName System.Windows.Forms
\$dialog = New-Object System.Windows.Forms.FolderBrowserDialog
\$dialog.Description = '$escapedDesc'
\$dialog.ShowNewFolderButton = \$true
if (\$dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
  [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
  Write-Output \$dialog.SelectedPath
}
''';
    final result = await Process.run(
      'powershell',
      <String>['-NoProfile', '-STA', '-Command', script],
      stdoutEncoding: _lenientUtf8,
      stderrEncoding: _lenientUtf8,
    );
    if (result.exitCode != 0) {
      final errText = (result.stderr as String).trim();
      throw StateError(
        'Windows 目录选择器执行失败（退出码 ${result.exitCode}）${errText.isEmpty ? '' : '：$errText'}',
      );
    }
    final selected = (result.stdout as String).trim();
    return selected.isEmpty ? null : selected;
  }

  /// 调用平台原生文件选择器（Windows 使用 OpenFileDialog，Android 使用 SAF ACTION_OPEN_DOCUMENT）。
  static Future<String?> pickFile({String title = '选择要发送的文件'}) async {
    if (Platform.isAndroid) {
      final picked = await _keepAliveChannel.invokeMethod<String>('pickFile');
      if (picked == null) return null;
      final trimmed = picked.trim();
      return trimmed.isEmpty ? null : trimmed;
    }
    if (!Platform.isWindows) return null;
    final escapedTitle = title.replaceAll("'", "''");
    final script = '''
Add-Type -AssemblyName System.Windows.Forms
\$dialog = New-Object System.Windows.Forms.OpenFileDialog
\$dialog.Title = '$escapedTitle'
\$dialog.Filter = 'All Files (*.*)|*.*'
\$dialog.Multiselect = \$false
if (\$dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
  [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
  Write-Output \$dialog.FileName
}
''';
    final result = await Process.run(
      'powershell',
      <String>['-NoProfile', '-STA', '-Command', script],
      stdoutEncoding: _lenientUtf8,
      stderrEncoding: _lenientUtf8,
    );
    if (result.exitCode != 0) {
      final errText = (result.stderr as String).trim();
      throw StateError(
        'Windows 文件选择器执行失败（退出码 ${result.exitCode}）${errText.isEmpty ? '' : '：$errText'}',
      );
    }
    final selected = (result.stdout as String).trim();
    return selected.isEmpty ? null : selected;
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

    String? currentSsid;
    var hasWifiAdapter = false;
    final ssids = <String>{};

    final ifaceRes = await Process.run(
      'cmd',
      <String>['/c', 'chcp 65001 >nul && netsh wlan show interfaces'],
      stdoutEncoding: _lenientUtf8,
      stderrEncoding: _lenientUtf8,
    );
    final ifaceOut = ifaceRes.stdout as String;
    final ifaceErr = (ifaceRes.stderr as String).trim();

    if (ifaceRes.exitCode != 0) {
      final combined = '$ifaceOut\n$ifaceErr';
      final noWirelessInterface =
          combined.contains('There is no wireless interface') ||
              combined.contains('系统上没有无线接口');
      if (!noWirelessInterface) {
        throw StateError(
          'netsh wlan show interfaces 执行失败（退出码 ${ifaceRes.exitCode}）：'
          '${ifaceErr.isNotEmpty ? ifaceErr : ifaceOut.trim()}',
        );
      }
    } else {
      if (!ifaceOut.contains('There is no wireless interface') &&
          !ifaceOut.contains('系统上没有无线接口')) {
        hasWifiAdapter = true;
      }
      for (final line in ifaceOut.split(RegExp(r'\r?\n'))) {
        final match = RegExp(r'^\s*SSID\s*:\s*(.+)$').firstMatch(line);
        if (match != null) {
          final val = match.group(1)!.trim();
          if (val.isNotEmpty) {
            currentSsid = val;
            ssids.add(val);
          }
        }
      }
    }

    if (hasWifiAdapter) {
      final netRes = await Process.run(
        'cmd',
        <String>['/c', 'chcp 65001 >nul && netsh wlan show networks'],
        stdoutEncoding: _lenientUtf8,
        stderrEncoding: _lenientUtf8,
      );
      if (netRes.exitCode == 0) {
        final out = netRes.stdout as String;
        for (final line in out.split(RegExp(r'\r?\n'))) {
          final match = RegExp(r'^\s*SSID\s+\d+\s*:\s*(.+)$').firstMatch(line);
          if (match != null) {
            final val = match.group(1)!.trim();
            if (val.isNotEmpty) {
              ssids.add(val);
            }
          }
        }
      }
    }

    final isWiredEthernet = !hasWifiAdapter || currentSsid == null;
    final sorted = ssids.toList(growable: false)..sort();

    return NetworkScanSnapshot(
      hasWifiAdapter: hasWifiAdapter,
      isWiredEthernet: isWiredEthernet,
      currentSsid: currentSsid,
      availableSsids: sorted,
    );
  }
}
