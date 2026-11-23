import 'dart:io';

import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import '../../bridge/lanvault_bridge.dart';
import '../../bridge/system_env_helper.dart';
import '../../theme/neu_theme.dart';
import '../../theme/neu_toast.dart';

enum _SettingsSection {
  general('通用', Icons.tune_rounded),
  security('安全与加密', Icons.verified_user_outlined),
  backupPolicy('备份策略', Icons.event_repeat_rounded),
  networkService('网络与服务', Icons.lan_outlined),
  diagnostics('诊断与日志', Icons.terminal_rounded),
  about('关于', Icons.info_outline_rounded);

  const _SettingsSection(this.label, this.icon);

  final String label;
  final IconData icon;
}

/// 7. 设置页：
/// 桌面端左侧二级菜单 + 右侧详情面板；移动端顶部横向分类胶囊切换。
/// 完整整合三态运行模式、安全加密与密钥备份、网络限速与多 SSID 白名单、
/// 服务端运行管理、Debug 探针日志控制台与品牌“关于”页。
class SettingsScreen extends StatefulWidget {
  const SettingsScreen({super.key, required this.bridge});

  final LanVaultBridge bridge;

  @override
  State<SettingsScreen> createState() => _SettingsScreenState();
}

class _SettingsScreenState extends State<SettingsScreen> {
  static const String _anyNetworkSentinel = '__any_network__';

  final TextEditingController _maxKbpsController =
      TextEditingController(text: '0');
  final TextEditingController _keyPassphraseController =
      TextEditingController();

  _SettingsSection _activeSection = _SettingsSection.general;

  bool _loading = true;
  bool _switchingMode = false;
  bool _savingPolicy = false;
  bool _scanningWifi = false;
  bool _scrubbing = false;
  bool _restarting = false;
  bool _exportingKey = false;

  bool _wifiOnly = true;
  bool _requireCharging = false;
  bool _debugMode = false;
  bool _bitRotAutoScrub = true;
  bool _loadingLogs = false;
  bool _runningDiagnostics = false;
  String _logFilter = 'ALL';
  String? _debugLogPath;
  List<DebugLogEntryModel> _debugLogs = const <DebugLogEntryModel>[];

  List<String> _allowedSsids = <String>[];
  String _selectedWifiDropdown = _anyNetworkSentinel;
  NetworkScanSnapshot? _networkSnapshot;
  SystemOverviewModel? _overview;
  KeepAliveLevel? _keepAliveLevel;
  BitRotScrubResultModel? _lastScrub;

  @override
  void initState() {
    super.initState();
    widget.bridge.serverStateVersion.addListener(_loadSettings);
    _loadSettings();
  }

  @override
  void didUpdateWidget(covariant SettingsScreen oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.bridge != widget.bridge) {
      oldWidget.bridge.serverStateVersion.removeListener(_loadSettings);
      widget.bridge.serverStateVersion.addListener(_loadSettings);
      _loadSettings();
    }
  }

  @override
  void dispose() {
    widget.bridge.serverStateVersion.removeListener(_loadSettings);
    _maxKbpsController.dispose();
    _keyPassphraseController.dispose();
    super.dispose();
  }

  Future<void> _loadSettings() async {
    setState(() => _loading = true);
    try {
      final results = await Future.wait<Object>(<Future<Object>>[
        widget.bridge.getRoleMode(),
        widget.bridge.getAutoReconnect(),
        widget.bridge.getPowerNetworkPolicy(),
        widget.bridge.getDebugMode(),
        widget.bridge.getSystemOverview(),
      ]);
      final policy = results[2] as PowerNetworkPolicyModel;
      final debugEnabled = results[3] as bool;
      final overview = results[4] as SystemOverviewModel;
      final env = await widget.bridge.getDeviceEnvironment();
      final scan = await SystemEnvHelper.scanNetworks(
        fallbackCurrentSsid: env.currentSsid,
      );
      final level = await widget.bridge.detectKeepAliveLevel();

      String? logPath;
      List<DebugLogEntryModel> logs = const <DebugLogEntryModel>[];
      if (debugEnabled) {
        logPath = await widget.bridge.getDebugLogPath();
        logs = await widget.bridge.getDebugLogs(limit: 120);
      }

      if (!mounted) return;
      final preservedSsids = policy.allowedSsids
          .map((s) => s.trim())
          .where((s) => s.isNotEmpty)
          .toSet()
          .toList(growable: true);
      setState(() {
        _wifiOnly = Platform.isAndroid ? policy.wifiOnly : false;
        _requireCharging = Platform.isAndroid ? policy.requireCharging : false;
        _maxKbpsController.text = policy.maxKbps.toString();
        _allowedSsids = preservedSsids;
        _selectedWifiDropdown = preservedSsids.isEmpty
            ? _anyNetworkSentinel
            : preservedSsids.first;
        _debugMode = debugEnabled;
        _debugLogPath = logPath;
        _debugLogs = logs.reversed.toList(growable: false);
        _networkSnapshot = scan;
        _overview = overview;
        _bitRotAutoScrub = overview.bitRotAutoScrub;
        _keepAliveLevel = level;
        _loading = false;
      });
    } catch (err) {
      if (!mounted) return;
      setState(() => _loading = false);
      NeuToast.error(context, '读取设置失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _rescanNetworks() async {
    setState(() => _scanningWifi = true);
    try {
      final env = await widget.bridge.getDeviceEnvironment();
      final scan = await SystemEnvHelper.scanNetworks(
        fallbackCurrentSsid: env.currentSsid,
      );
      if (!mounted) return;
      setState(() {
        _networkSnapshot = scan;
        _scanningWifi = false;
      });
      if (!scan.hasWifiAdapter || scan.availableSsids.isEmpty) {
        NeuToast.info(context, '当前未扫描到可用 Wi-Fi（有线网线连接可直接同步）');
      } else {
        NeuToast.success(context, '已扫描到 ${scan.availableSsids.length} 个 Wi-Fi 网络');
      }
    } catch (err) {
      if (!mounted) return;
      setState(() => _scanningWifi = false);
      NeuToast.error(context, '扫描网络失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _requestLocationPermission() async {
    try {
      final granted = await widget.bridge.requestLocationPermission();
      if (!mounted) return;
      if (granted) {
        NeuToast.success(context, '已获得定位权限，正在刷新当前 Wi-Fi SSID');
        await _rescanNetworks();
      } else {
        NeuToast.info(context, '未授予定位权限，系统将无法读取当前 Wi-Fi 名称');
      }
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, '请求定位权限失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _requestNotificationPermission() async {
    try {
      final granted = await widget.bridge.requestNotificationPermission();
      if (!mounted) return;
      if (granted) {
        NeuToast.success(context, '已获得通知权限');
      } else {
        NeuToast.info(context, '请在系统设置中允许通知权限以保持后台服务稳定');
      }
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, '请求通知权限失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  void _handleWifiSelected(String? val) {
    if (val == null) return;
    setState(() {
      _selectedWifiDropdown = val;
      if (val == _anyNetworkSentinel) {
        _allowedSsids = <String>[];
      } else if (!_allowedSsids.contains(val)) {
        _allowedSsids = <String>[..._allowedSsids, val];
      }
    });
  }

  void _removeAllowedSsid(String ssid) {
    setState(() {
      _allowedSsids = _allowedSsids.where((s) => s != ssid).toList();
      if (_allowedSsids.isEmpty) {
        _selectedWifiDropdown = _anyNetworkSentinel;
      } else if (_selectedWifiDropdown == ssid) {
        _selectedWifiDropdown = _allowedSsids.first;
      }
    });
  }

  Future<void> _handleModeSelected(RoleMode target) async {
    if (_switchingMode || target == widget.bridge.currentRoleMode) return;
    setState(() => _switchingMode = true);
    try {
      final applied = await widget.bridge.setRoleMode(target);
      if (!mounted) return;
      setState(() => _switchingMode = false);
      NeuToast.success(context, '已切换至${applied.label}');
    } catch (err) {
      if (!mounted) return;
      setState(() => _switchingMode = false);
      NeuToast.error(context, '切换运行模式失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _toggleAutoReconnect(bool enabled) async {
    try {
      await widget.bridge.enableAutoReconnect(enabled);
      if (!mounted) return;
      NeuToast.success(context, enabled ? '已开启断线自动重连' : '已关闭断线自动重连');
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, '更新自动重连失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _saveSyncPolicy() async {
    final maxKbps = int.tryParse(_maxKbpsController.text.trim());
    if (maxKbps == null || maxKbps < 0) {
      NeuToast.warning(context, '上传限速必须为大于或等于 0 的整数（0 表示不限速）');
      return;
    }

    setState(() => _savingPolicy = true);
    try {
      await widget.bridge.updatePowerNetworkPolicy(
        wifiOnly: Platform.isAndroid ? _wifiOnly : false,
        allowedSsids: List<String>.unmodifiable(_allowedSsids),
        requireCharging: Platform.isAndroid ? _requireCharging : false,
        maxKbps: maxKbps,
      );
      if (!mounted) return;
      setState(() => _savingPolicy = false);
      NeuToast.success(context, '同步与网络策略已保存');
    } catch (err) {
      if (!mounted) return;
      setState(() => _savingPolicy = false);
      NeuToast.error(context, '保存策略失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _toggleBitRotAutoScrub(bool enabled) async {
    try {
      final applied = await widget.bridge.setBitRotAutoScrub(enabled);
      if (!mounted) return;
      setState(() => _bitRotAutoScrub = applied);
      NeuToast.success(
        context,
        applied ? '已开启 Bit-Rot 定时自动巡检' : '已关闭定时自动巡检',
      );
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, '切换自动巡检失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _triggerQuickScrub() async {
    setState(() => _scrubbing = true);
    try {
      final res = await widget.bridge.triggerBitRotScrub();
      if (!mounted) return;
      setState(() {
        _lastScrub = res;
        _scrubbing = false;
      });
      NeuToast.success(
        context,
        '校验完成：已检 ${res.checkedChunks} 块，损坏 ${res.corruptedChunks} 块，自愈 ${res.healedChunks} 块',
      );
    } catch (err) {
      if (!mounted) return;
      setState(() => _scrubbing = false);
      NeuToast.error(context, '完整性校验失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _exportKeyBackup() async {
    final pass = _keyPassphraseController.text;
    if (pass.length < 12) {
      NeuToast.warning(context, '密钥备份口令至少需要 12 位字符');
      return;
    }
    setState(() => _exportingKey = true);
    try {
      final backupJson = await widget.bridge.exportVaultKeyBackup(pass);
      await Clipboard.setData(ClipboardData(text: backupJson));
      if (!mounted) return;
      _keyPassphraseController.clear();
      setState(() => _exportingKey = false);
      NeuToast.success(context, '加密密钥备份已导出并复制到剪贴板，请妥善保存');
    } catch (err) {
      if (!mounted) return;
      setState(() => _exportingKey = false);
      NeuToast.error(context, '导出密钥备份失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _restartServer() async {
    setState(() => _restarting = true);
    try {
      final ok = await widget.bridge.executeGracefulRestart();
      final overview = await widget.bridge.getSystemOverview();
      if (!mounted) return;
      setState(() {
        _overview = overview;
        _restarting = false;
      });
      NeuToast.success(context, ok ? '服务端已完成优雅重启' : '已下发重启指令');
    } catch (err) {
      if (!mounted) return;
      setState(() => _restarting = false);
      NeuToast.error(context, '重启服务失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _toggleDebugMode(bool enabled) async {
    try {
      final applied = await widget.bridge.setDebugMode(enabled);
      String? logPath;
      List<DebugLogEntryModel> logs = const <DebugLogEntryModel>[];
      if (applied) {
        logPath = await widget.bridge.getDebugLogPath();
        logs = await widget.bridge.getDebugLogs(limit: 120);
      }
      if (!mounted) return;
      setState(() {
        _debugMode = applied;
        _debugLogPath = logPath;
        _debugLogs = logs.reversed.toList(growable: false);
      });
      NeuToast.info(
        context,
        applied ? '已开启 Debug 模式，探针日志将实时写入磁盘' : '已关闭 Debug 模式',
      );
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, '切换 Debug 模式失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _refreshDebugLogs() async {
    setState(() => _loadingLogs = true);
    try {
      final logPath = await widget.bridge.getDebugLogPath();
      final logs = await widget.bridge.getDebugLogs(limit: 150);
      if (!mounted) return;
      setState(() {
        _debugLogPath = logPath;
        _debugLogs = logs.reversed.toList(growable: false);
        _loadingLogs = false;
      });
    } catch (err) {
      if (!mounted) return;
      setState(() => _loadingLogs = false);
      NeuToast.error(context, '刷新日志失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _clearDebugLogs() async {
    try {
      await widget.bridge.clearDebugLogs();
      if (!mounted) return;
      setState(() => _debugLogs = const <DebugLogEntryModel>[]);
      NeuToast.info(context, '已清空内存与磁盘 Debug 日志');
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, '清空日志失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _runSelfDiagnostics() async {
    setState(() => _runningDiagnostics = true);
    try {
      final report = await widget.bridge.runSelfDiagnostics();
      final logs = await widget.bridge.getDebugLogs(limit: 150);
      if (!mounted) return;
      setState(() {
        _runningDiagnostics = false;
        _debugLogs = logs.reversed.toList(growable: false);
        _debugLogPath = report['log_file_path'] as String? ?? _debugLogPath;
      });
      final checksPassed = report['checks_passed'];
      final issues = report['issues'];
      final version = report['version'] ?? LanVaultBridge.appVersion;
      final summary = checksPassed != null
          ? '自检完成（v$version，通过：$checksPassed，异常：${issues is List ? issues.length : 0}）'
          : '探针自检通过（模式：${report['role_mode']}，任务数：${report['sync_rule_count']}）';
      NeuToast.success(context, summary);
    } catch (err) {
      if (!mounted) return;
      setState(() => _runningDiagnostics = false);
      NeuToast.error(context, '自检失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _copyLogPath() async {
    final path = _debugLogPath;
    if (path == null || path.isEmpty) {
      NeuToast.warning(context, '当前尚未生成日志路径');
      return;
    }
    await Clipboard.setData(ClipboardData(text: path));
    if (!mounted) return;
    NeuToast.success(context, '已复制日志文件路径');
  }

  Future<void> _openLogFolder() async {
    final path = _debugLogPath;
    if (path == null || path.isEmpty) return;
    if (Platform.isWindows) {
      try {
        await Process.run('explorer.exe', <String>['/select,$path']);
      } catch (err) {
        if (!mounted) return;
        NeuToast.error(context, '打开日志目录失败：${LanVaultBridge.formatUserError(err)}');
      }
    }
  }

  String _formatLogTime(int timestampMs) {
    final dt = DateTime.fromMillisecondsSinceEpoch(timestampMs);
    return '${dt.hour.toString().padLeft(2, '0')}:'
        '${dt.minute.toString().padLeft(2, '0')}:'
        '${dt.second.toString().padLeft(2, '0')}.'
        '${dt.millisecond.toString().padLeft(3, '0')}';
  }

  Color _levelColor(String level) {
    return switch (level.toUpperCase()) {
      'FATAL' || 'ERROR' => NeuTokens.danger,
      'WARN' => NeuTokens.warning,
      'PROBE' => NeuTokens.accent,
      _ => NeuTokens.healthy,
    };
  }

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    final isWide =
        MediaQuery.sizeOf(context).width >= NeuTokens.breakpointWide;

    return ValueListenableBuilder<RoleMode>(
      valueListenable: widget.bridge.roleModeNotifier,
      builder: (context, roleMode, _) {
        final detailPanel = NeuMotionSwitcher(
          child: KeyedSubtree(
            key: ValueKey<_SettingsSection>(_activeSection),
            child: _buildActiveSection(context, roleMode, isWide),
          ),
        );

        if (isWide) {
          return Padding(
            padding: const EdgeInsets.all(NeuSpace.lg),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: <Widget>[
                Text('设置', style: textTheme.headlineSmall),
                const SizedBox(height: 4),
                Text('系统配置、服务与安全', style: textTheme.bodySmall),
                const SizedBox(height: 20),
                Expanded(
                  child: Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: <Widget>[
                      SizedBox(
                        width: 196,
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.stretch,
                          children: <Widget>[
                            for (final sec in _SettingsSection.values) ...<Widget>[
                              _SettingsMenuTile(
                                section: sec,
                                selected: sec == _activeSection,
                                onTap: () =>
                                    setState(() => _activeSection = sec),
                              ),
                              const SizedBox(height: 4),
                            ],
                          ],
                        ),
                      ),
                      const SizedBox(width: NeuSpace.lg),
                      Expanded(
                        child: SingleChildScrollView(child: detailPanel),
                      ),
                    ],
                  ),
                ),
              ],
            ),
          );
        }

        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: <Widget>[
            Padding(
              padding: const EdgeInsets.symmetric(
                horizontal: NeuSpace.md,
                vertical: 6,
              ),
              child: Wrap(
                spacing: 8,
                runSpacing: 8,
                children: <Widget>[
                  for (final sec in _SettingsSection.values)
                    Semantics(
                      button: true,
                      selected: sec == _activeSection,
                      label: sec.label,
                      child: InkWell(
                        onTap: () => setState(() => _activeSection = sec),
                        borderRadius: const BorderRadius.all(
                          Radius.circular(NeuRadius.pill),
                        ),
                        child: AnimatedContainer(
                          duration: NeuMotion.resolveState(context),
                          constraints: const BoxConstraints(minHeight: 38),
                          padding: const EdgeInsets.symmetric(
                            horizontal: 13,
                            vertical: 8,
                          ),
                          decoration: BoxDecoration(
                            color: sec == _activeSection
                                ? null
                                : NeuTokens.surface,
                            gradient: sec == _activeSection
                                ? NeuTokens.primaryGradient
                                : null,
                            borderRadius: const BorderRadius.all(
                              Radius.circular(NeuRadius.pill),
                            ),
                            border: sec == _activeSection
                                ? null
                                : Border.all(color: NeuTokens.hairlineBorder),
                          ),
                          child: Row(
                            mainAxisSize: MainAxisSize.min,
                            children: <Widget>[
                              Icon(
                                sec.icon,
                                size: 15,
                                color: sec == _activeSection
                                    ? Colors.white
                                    : NeuTokens.textSecondary,
                              ),
                              const SizedBox(width: 6),
                              Text(
                                sec.label,
                                style: TextStyle(
                                  fontSize: 13,
                                  fontWeight: FontWeight.w600,
                                  color: sec == _activeSection
                                      ? Colors.white
                                      : NeuTokens.textPrimary,
                                ),
                              ),
                            ],
                          ),
                        ),
                      ),
                    ),
                ],
              ),
            ),
            Expanded(
              child: SingleChildScrollView(
                padding: const EdgeInsets.all(NeuSpace.md),
                child: detailPanel,
              ),
            ),
          ],
        );
      },
    );
  }

  Widget _buildActiveSection(
    BuildContext context,
    RoleMode roleMode,
    bool isWide,
  ) {
    return switch (_activeSection) {
      _SettingsSection.general => _buildGeneralSection(context, roleMode),
      _SettingsSection.security => _buildSecuritySection(context, isWide),
      _SettingsSection.backupPolicy => _buildBackupPolicySection(context, isWide),
      _SettingsSection.networkService => _buildNetworkServiceSection(context),
      _SettingsSection.diagnostics => _buildDiagnosticsSection(context),
      _SettingsSection.about => _buildAboutSection(context),
    };
  }

  Widget _buildGeneralSection(BuildContext context, RoleMode roleMode) {
    final textTheme = Theme.of(context).textTheme;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: <Widget>[
        NeuCard(
          padding: const EdgeInsets.all(20),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: <Widget>[
              Text('节点运行模式', style: textTheme.titleMedium),
              const SizedBox(height: 4),
              Text(
                '按需切换为仅备份客户端、仅存储服务端或双端同时运行',
                style: textTheme.bodySmall,
              ),
              const SizedBox(height: 14),
              NeuSegmentedControl<RoleMode>(
                semanticLabel: '运行模式切换',
                value: roleMode,
                enabled: !_switchingMode && !_loading,
                onChanged: _handleModeSelected,
                segments: <NeuSegment<RoleMode>>[
                  for (final mode in RoleMode.values)
                    NeuSegment<RoleMode>(value: mode, label: mode.label),
                ],
              ),
            ],
          ),
        ),
        const SizedBox(height: NeuSpace.md),
        NeuCard(
          padding: const EdgeInsets.all(20),
          child: ValueListenableBuilder<bool>(
            valueListenable: widget.bridge.autoReconnectNotifier,
            builder: (context, autoReconnect, _) {
              return NeuSwitch(
                title: '断线自动重连',
                subtitle: '局域网恢复后自动按 2s / 5s / 15s / 30s 退避重连已配对设备',
                value: autoReconnect,
                onChanged: _loading ? null : _toggleAutoReconnect,
              );
            },
          ),
        ),
        if (Platform.isAndroid) ...<Widget>[
          const SizedBox(height: NeuSpace.md),
          NeuCard(
            padding: const EdgeInsets.all(20),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: <Widget>[
                Text('Android 系统权限', style: textTheme.titleMedium),
                const SizedBox(height: 4),
                Text(
                  '授予 Wi-Fi 定位与通知权限，保障后台保活服务稳定运行与家庭网络 SSID 识别。',
                  style: textTheme.bodySmall,
                ),
                const SizedBox(height: 14),
                Wrap(
                  spacing: NeuSpace.sm,
                  runSpacing: NeuSpace.sm,
                  children: <Widget>[
                    NeuButton(
                      label: '授权 Wi-Fi 定位',
                      icon: Icons.location_on_outlined,
                      onPressed: _requestLocationPermission,
                    ),
                    NeuButton(
                      label: '授权通知权限',
                      icon: Icons.notifications_active_outlined,
                      onPressed: _requestNotificationPermission,
                    ),
                  ],
                ),
              ],
            ),
          ),
        ],
      ],
    );
  }

  Widget _buildSecuritySection(BuildContext context, bool isWide) {
    final textTheme = Theme.of(context).textTheme;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: <Widget>[
        NeuCard(
          padding: const EdgeInsets.all(20),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: <Widget>[
              Text('安全与加密', style: textTheme.titleMedium),
              const SizedBox(height: 14),
              const _SecurityRow(
                title: '零知识加密',
                subtitle: '文件在客户端加密后分块上传，服务端仅存密文',
                status: '已启用',
                tone: NeuStatusTone.healthy,
              ),
              const Padding(
                padding: EdgeInsets.symmetric(vertical: 12),
                child: Divider(height: 1),
              ),
              NeuSwitch(
                title: '数据完整性自动校验 (Bit-Rot)',
                subtitle: _lastScrub == null
                    ? '定期巡检数据块哈希并利用冗余副本自愈损坏块'
                    : '上次校验 ${_lastScrub!.checkedChunks} 块 · 损坏 ${_lastScrub!.corruptedChunks} 块',
                value: _bitRotAutoScrub,
                onChanged: _loading ? null : _toggleBitRotAutoScrub,
              ),
              const SizedBox(height: 10),
              Align(
                alignment: Alignment.centerRight,
                child: NeuButton(
                  label: '立即校验',
                  icon: Icons.fact_check_outlined,
                  tone: NeuButtonTone.primary,
                  loading: _scrubbing,
                  onPressed: _triggerQuickScrub,
                ),
              ),
              const Padding(
                padding: EdgeInsets.symmetric(vertical: 12),
                child: Divider(height: 1),
              ),
              const _SecurityRow(
                title: '传输加密 (TLS)',
                subtitle: '局域网内双向证书校验与端到端加密传输',
                status: '强制启用',
                tone: NeuStatusTone.healthy,
              ),
            ],
          ),
        ),
        const SizedBox(height: NeuSpace.md),
        NeuCard(
          padding: const EdgeInsets.all(20),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: <Widget>[
              Text('主密钥加密备份', style: textTheme.titleMedium),
              const SizedBox(height: 4),
              Text(
                '使用至少 12 位高强度口令导出 Vault 主密钥加密备份，换机时可用于离线恢复数据。',
                style: textTheme.bodySmall,
              ),
              const SizedBox(height: 12),
              if (isWide)
                Row(
                  crossAxisAlignment: CrossAxisAlignment.end,
                  children: <Widget>[
                    Expanded(
                      child: NeuInsetField(
                        controller: _keyPassphraseController,
                        label: '备份加密口令（至少 12 位）',
                        hint: '输入高强度口令',
                        obscureText: true,
                        prefixIcon: Icons.key_rounded,
                      ),
                    ),
                    const SizedBox(width: NeuSpace.sm),
                    NeuButton(
                      label: '导出备份',
                      icon: Icons.download_rounded,
                      tone: NeuButtonTone.primary,
                      loading: _exportingKey,
                      onPressed: _exportKeyBackup,
                    ),
                  ],
                )
              else
                Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: <Widget>[
                    NeuInsetField(
                      controller: _keyPassphraseController,
                      label: '备份加密口令（至少 12 位）',
                      hint: '输入高强度口令',
                      obscureText: true,
                      prefixIcon: Icons.key_rounded,
                    ),
                    const SizedBox(height: NeuSpace.sm),
                    NeuButton(
                      label: '导出备份',
                      icon: Icons.download_rounded,
                      tone: NeuButtonTone.primary,
                      loading: _exportingKey,
                      expanded: true,
                      onPressed: _exportKeyBackup,
                    ),
                  ],
                ),
            ],
          ),
        ),
      ],
    );
  }

  Widget _buildBackupPolicySection(BuildContext context, bool isWide) {
    final textTheme = Theme.of(context).textTheme;
    final scan = _networkSnapshot;
    final scannedSsids = <String>{
      ...?scan?.availableSsids,
      ..._allowedSsids,
    }.toList(growable: false)
      ..sort();

    final wifiOptions = <NeuDropdownOption<String>>[
      const NeuDropdownOption<String>(
        value: _anyNetworkSentinel,
        label: '不限制网络（包含网线与任意 Wi-Fi）',
      ),
      for (final ssid in scannedSsids)
        NeuDropdownOption<String>(
          value: ssid,
          label: ssid,
          subtitle: scan?.currentSsid == ssid ? '当前已连' : null,
        ),
    ];

    final dropdownValue =
        wifiOptions.any((o) => o.value == _selectedWifiDropdown)
            ? _selectedWifiDropdown
            : _anyNetworkSentinel;

    return NeuCard(
      padding: const EdgeInsets.all(20),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Text('备份与网络限速策略', style: textTheme.titleMedium),
          const SizedBox(height: 14),
          if (Platform.isAndroid) ...<Widget>[
            NeuSwitch(
              title: '仅 Wi-Fi 网络下执行同步',
              subtitle: '避免消耗手机蜂窝流量',
              value: _wifiOnly,
              onChanged: (v) => setState(() => _wifiOnly = v),
            ),
            const SizedBox(height: 8),
            NeuSwitch(
              title: '仅充电时全速上传',
              subtitle: '移动端未充电时降低后台耗电',
              value: _requireCharging,
              onChanged: (v) => setState(() => _requireCharging = v),
            ),
            const SizedBox(height: 14),
          ],
          if (!Platform.isAndroid &&
              scan != null &&
              scan.isWiredEthernet) ...<Widget>[
            Text(
              scan.hasWifiAdapter
                  ? '当前未连接 Wi-Fi（可能正通过网线连接），默认不限制网络即可直接同步。'
                  : '当前电脑未检测到无线网卡（有线网线连接），无需限制 Wi-Fi。',
              style: textTheme.bodySmall,
            ),
            const SizedBox(height: 10),
          ],
          if (isWide)
            Row(
              crossAxisAlignment: CrossAxisAlignment.end,
              children: <Widget>[
                Expanded(
                  child: NeuDropdown<String>(
                    label: '允许同步的家庭网络（支持多选添加）',
                    emptyHint: '不限制网络',
                    prefixIcon: Icons.wifi_rounded,
                    options: wifiOptions,
                    value: dropdownValue,
                    onChanged: _handleWifiSelected,
                  ),
                ),
                const SizedBox(width: NeuSpace.sm),
                NeuButton(
                  label: '扫描',
                  icon: Icons.refresh_rounded,
                  loading: _scanningWifi,
                  onPressed: _rescanNetworks,
                ),
              ],
            )
          else ...<Widget>[
            NeuDropdown<String>(
              label: '允许同步的家庭网络（支持多选添加）',
              emptyHint: '不限制网络',
              prefixIcon: Icons.wifi_rounded,
              options: wifiOptions,
              value: dropdownValue,
              onChanged: _handleWifiSelected,
            ),
            const SizedBox(height: NeuSpace.sm),
            NeuButton(
              label: '扫描周边 Wi-Fi',
              icon: Icons.refresh_rounded,
              loading: _scanningWifi,
              expanded: true,
              onPressed: _rescanNetworks,
            ),
          ],
          if (_allowedSsids.isNotEmpty) ...<Widget>[
            const SizedBox(height: 10),
            Wrap(
              spacing: 8,
              runSpacing: 8,
              children: <Widget>[
                for (final ssid in _allowedSsids)
                  Container(
                    key: ValueKey<String>('ssid_$ssid'),
                    padding: const EdgeInsets.only(left: 12, right: 2),
                    decoration: BoxDecoration(
                      color: NeuTokens.accentSoft,
                      borderRadius: const BorderRadius.all(
                        Radius.circular(NeuRadius.pill),
                      ),
                    ),
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: <Widget>[
                        Text(
                          ssid,
                          style: textTheme.labelMedium?.copyWith(
                            color: NeuTokens.accent,
                          ),
                        ),
                        const SizedBox(width: 2),
                        Semantics(
                          button: true,
                          label: '移除允许的 Wi-Fi $ssid',
                          child: InkWell(
                            borderRadius: const BorderRadius.all(
                              Radius.circular(NeuRadius.pill),
                            ),
                            onTap: () => _removeAllowedSsid(ssid),
                            child: const SizedBox(
                              width: 44,
                              height: 44,
                              child: Icon(
                                Icons.close_rounded,
                                size: 16,
                                color: NeuTokens.accent,
                              ),
                            ),
                          ),
                        ),
                      ],
                    ),
                  ),
              ],
            ),
          ],
          const SizedBox(height: 14),
          NeuInsetField(
            controller: _maxKbpsController,
            label: '上传限速（KB/s，0 为不限速）',
            hint: '0',
            keyboardType: TextInputType.number,
            prefixIcon: Icons.speed_rounded,
          ),
          const SizedBox(height: 16),
          NeuButton(
            label: '保存备份与网络策略',
            icon: Icons.check_rounded,
            tone: NeuButtonTone.primary,
            loading: _savingPolicy,
            onPressed: _saveSyncPolicy,
          ),
        ],
      ),
    );
  }

  Widget _buildNetworkServiceSection(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    final overview = _overview;
    return NeuCard(
      padding: const EdgeInsets.all(20),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Text('网络与服务状态', style: textTheme.titleMedium),
          const SizedBox(height: 14),
          NeuKeyValueRow(
            label: '本机局域网 IP',
            value: overview == null || overview.lanIp.isEmpty
                ? '未获取'
                : overview.lanIp,
          ),
          const Divider(height: 1),
          NeuKeyValueRow(
            label: '本地服务端口',
            value: overview?.serverRunning == true
                ? '运行中 · :${overview?.serverPort ?? LanVaultBridge.defaultServerPort}'
                : '未启动',
          ),
          const Divider(height: 1),
          NeuKeyValueRow(
            label: '后台保活等级',
            value: _keepAliveLevel?.displayLabel ?? '检测中',
          ),
          const SizedBox(height: 18),
          Wrap(
            spacing: NeuSpace.sm,
            runSpacing: NeuSpace.sm,
            children: <Widget>[
              NeuButton(
                label: '优雅重启服务端',
                icon: Icons.restart_alt_rounded,
                tone: NeuButtonTone.primary,
                loading: _restarting,
                onPressed: _restartServer,
              ),
              NeuButton(
                label: '刷新状态',
                icon: Icons.refresh_rounded,
                onPressed: _loadSettings,
              ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _buildDiagnosticsSection(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    final filteredLogs = _logFilter == 'ALL'
        ? _debugLogs
        : _debugLogs
            .where((e) => e.level.toUpperCase() == _logFilter)
            .toList(growable: false);

    return NeuCard(
      padding: const EdgeInsets.all(20),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          NeuSwitch(
            title: 'Debug 调试与性能探针模式',
            subtitle: '开启后启用全链路微秒级探针，并实时写入磁盘 JSONL 日志文件',
            value: _debugMode,
            onChanged: _loading ? null : _toggleDebugMode,
          ),
          const SizedBox(height: 14),
          Wrap(
            spacing: NeuSpace.sm,
            runSpacing: NeuSpace.sm,
            children: <Widget>[
              NeuButton(
                label: '一键探针自检',
                icon: Icons.health_and_safety_outlined,
                tone: NeuButtonTone.primary,
                loading: _runningDiagnostics,
                onPressed: _runSelfDiagnostics,
              ),
              if (_debugMode) ...<Widget>[
                NeuButton(
                  label: '刷新日志',
                  icon: Icons.refresh_rounded,
                  loading: _loadingLogs,
                  onPressed: _refreshDebugLogs,
                ),
                NeuButton(
                  label: '复制日志路径',
                  icon: Icons.copy_rounded,
                  onPressed: _copyLogPath,
                ),
                if (Platform.isWindows)
                  NeuButton(
                    label: '打开日志目录',
                    icon: Icons.folder_open_rounded,
                    onPressed: _openLogFolder,
                  ),
                NeuButton(
                  label: '清空日志',
                  icon: Icons.delete_sweep_outlined,
                  onPressed: _clearDebugLogs,
                ),
              ],
            ],
          ),
          if (_debugMode) ...<Widget>[
            const SizedBox(height: 14),
            NeuCard(
              inset: true,
              padding: const EdgeInsets.all(12),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: <Widget>[
                  Text('日志文件位置（JSONL 格式，10 MB 自动轮转）',
                      style: textTheme.labelMedium),
                  const SizedBox(height: 4),
                  SelectableText(
                    _debugLogPath ?? '正在初始化日志路径…',
                    style: textTheme.bodySmall?.copyWith(
                      fontFamily: 'monospace',
                      color: NeuTokens.textPrimary,
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(height: 12),
            NeuSegmentedControl<String>(
              semanticLabel: '日志级别过滤',
              value: _logFilter,
              onChanged: (val) => setState(() => _logFilter = val),
              segments: const <NeuSegment<String>>[
                NeuSegment<String>(value: 'ALL', label: '全部'),
                NeuSegment<String>(value: 'PROBE', label: '探针'),
                NeuSegment<String>(value: 'ERROR', label: '错误'),
                NeuSegment<String>(value: 'DEBUG', label: '调试'),
              ],
            ),
            const SizedBox(height: 10),
            NeuCard(
              inset: true,
              padding: const EdgeInsets.all(12),
              child: SizedBox(
                height: 250,
                child: filteredLogs.isEmpty
                    ? Center(
                        child: Text(
                          '暂无匹配的日志记录，点击“一键探针自检”即可生成诊断日志。',
                          style: textTheme.bodySmall,
                        ),
                      )
                    : ListView.separated(
                        itemCount: filteredLogs.length,
                        separatorBuilder: (_, __) => const Divider(height: 10),
                        itemBuilder: (context, index) {
                          final item = filteredLogs[index];
                          final durText = item.durationUs == null
                              ? ''
                              : ' · ${item.durationUs} us';
                          final probeText =
                              item.probe == null ? '' : ' [${item.probe}]';
                          return Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: <Widget>[
                              Row(
                                children: <Widget>[
                                  Text(
                                    item.level,
                                    style: textTheme.labelMedium?.copyWith(
                                      color: _levelColor(item.level),
                                      fontFamily: 'monospace',
                                    ),
                                  ),
                                  const SizedBox(width: 8),
                                  Expanded(
                                    child: Text(
                                      '${_formatLogTime(item.timestampMs)} · ${item.source}$probeText$durText',
                                      overflow: TextOverflow.ellipsis,
                                      style: textTheme.bodySmall?.copyWith(
                                        fontFamily: 'monospace',
                                        fontSize: 12,
                                      ),
                                    ),
                                  ),
                                ],
                              ),
                              const SizedBox(height: 2),
                              Text(
                                item.message,
                                style: textTheme.bodySmall?.copyWith(
                                  color: NeuTokens.textPrimary,
                                  fontFamily: 'monospace',
                                  fontSize: 12.5,
                                ),
                              ),
                            ],
                          );
                        },
                      ),
              ),
            ),
          ],
        ],
      ),
    );
  }

  Widget _buildAboutSection(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    return NeuCard(
      padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 32),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.center,
        children: <Widget>[
          const BrandLogoFull(height: 54),
          const SizedBox(height: 14),
          NeuStatusBadge(
            label: 'EveryoneNAS v${LanVaultBridge.appVersion}',
            tone: NeuStatusTone.accent,
            showDot: false,
          ),
          const SizedBox(height: 14),
          Text(
            '局域网私有云 · 零知识加密 · 增量备份 · 数据自愈 · 跨端协同',
            textAlign: TextAlign.center,
            style: textTheme.bodyMedium?.copyWith(
              color: NeuTokens.textSecondary,
            ),
          ),
          const SizedBox(height: 20),
          const Divider(height: 1),
          const SizedBox(height: 8),
          const NeuKeyValueRow(
            label: '核心引擎',
            value: 'Rust Native FFI (FastCDC + Zstd + BLAKE3)',
          ),
          const NeuKeyValueRow(
            label: '快照策略',
            value: 'GFS 多级保留 (7日 / 4周 / 12月)',
          ),
          const NeuKeyValueRow(
            label: '支持平台',
            value: 'Windows 桌面端 & Android 移动端',
          ),
        ],
      ),
    );
  }
}

class _SettingsMenuTile extends StatelessWidget {
  const _SettingsMenuTile({
    required this.section,
    required this.selected,
    required this.onTap,
  });

  final _SettingsSection section;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: onTap,
        child: AnimatedContainer(
          duration: NeuMotion.resolveState(context),
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
          decoration: BoxDecoration(
            color: selected ? NeuTokens.accentSoft : Colors.transparent,
            borderRadius: const BorderRadius.all(Radius.circular(10)),
          ),
          child: Row(
            children: <Widget>[
              Icon(
                section.icon,
                size: 18,
                color: selected ? NeuTokens.accent : NeuTokens.textSecondary,
              ),
              const SizedBox(width: 10),
              Text(
                section.label,
                style: Theme.of(context).textTheme.labelLarge?.copyWith(
                      color:
                          selected ? NeuTokens.accent : NeuTokens.textPrimary,
                      fontWeight: selected ? FontWeight.w700 : FontWeight.w500,
                    ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _SecurityRow extends StatelessWidget {
  const _SecurityRow({
    required this.title,
    required this.subtitle,
    required this.status,
    required this.tone,
  });

  final String title;
  final String subtitle;
  final String status;
  final NeuStatusTone tone;

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    return Row(
      children: <Widget>[
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: <Widget>[
              Text(title, style: textTheme.titleSmall),
              const SizedBox(height: 2),
              Text(subtitle, style: textTheme.bodySmall),
            ],
          ),
        ),
        const SizedBox(width: 12),
        NeuStatusBadge(label: status, tone: tone),
      ],
    );
  }
}
