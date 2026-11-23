// 参考依据:
// - ARCHITECTURE_CONTRACTS.md 第 3.1 / 4 / 5 / 6 节
// - Apple HIG Motion / ConcentricRectangle / WCAG 2.2 AA

import 'dart:async';
import 'dart:io';

import 'package:material_ui/material_ui.dart';

import '../../bridge/lanvault_bridge.dart';
import '../../theme/neu_theme.dart';

/// 概览仪表盘：顶部三态运行模式切换器 + 按角色动态裁剪的客户端同步/重连区与服务端保活/Bit-Rot 状态区。
class DashboardScreen extends StatefulWidget {
  const DashboardScreen({super.key, required this.bridge});

  final LanVaultBridge bridge;

  @override
  State<DashboardScreen> createState() => _DashboardScreenState();
}

class _DashboardScreenState extends State<DashboardScreen> {
  KeepAliveLevel? _keepAliveLevel;
  SyncProgressEvent? _latestSync;
  StreamSubscription<SyncProgressEvent>? _syncSub;

  bool _switchingMode = false;
  bool _autoReconnectEnabled = true;
  bool _togglingReconnect = false;
  bool _restarting = false;
  bool _loadingLevel = false;

  String? _statusMessage;
  String? _errorMessage;

  @override
  void initState() {
    super.initState();
    _syncInitialRoleMode();
    if (widget.bridge.currentRoleMode.hasServer) {
      _loadKeepAliveLevel();
    }
    _subscribeTelemetry();
  }

  @override
  void dispose() {
    _syncSub?.cancel();
    super.dispose();
  }

  Future<void> _syncInitialRoleMode() async {
    try {
      final mode = await widget.bridge.getRoleMode();
      if (!mounted) return;
      if (mode.hasServer && _keepAliveLevel == null && !_loadingLevel) {
        _loadKeepAliveLevel();
      }
    } catch (err) {
      if (!mounted) return;
      setState(() => _errorMessage = '读取运行模式失败：$err');
    }
  }

  Future<void> _handleModeSelected(RoleMode target) async {
    if (_switchingMode || target == widget.bridge.currentRoleMode) return;

    setState(() {
      _switchingMode = true;
      _errorMessage = null;
      _statusMessage = null;
    });

    try {
      final applied = await widget.bridge.setRoleMode(target);
      if (!mounted) return;
      setState(() {
        _switchingMode = false;
        _statusMessage = '已切换至${applied.label}。';
      });
      if (applied.hasServer && _keepAliveLevel == null) {
        _loadKeepAliveLevel();
      }
    } catch (err) {
      if (!mounted) return;
      setState(() {
        _switchingMode = false;
        _errorMessage = '切换运行模式失败：$err';
      });
    }
  }

  Future<void> _loadKeepAliveLevel() async {
    setState(() {
      _loadingLevel = true;
      _errorMessage = null;
    });
    try {
      final level = await widget.bridge.detectKeepAliveLevel();
      if (!mounted) return;
      setState(() {
        _keepAliveLevel = level;
        _loadingLevel = false;
      });
    } catch (err) {
      if (!mounted) return;
      setState(() {
        _loadingLevel = false;
        _errorMessage = '保活等级探测失败：$err';
      });
    }
  }

  void _subscribeTelemetry() {
    try {
      _syncSub = widget.bridge.subscribeSyncProgress().listen(
        (event) {
          if (!mounted) return;
          setState(() => _latestSync = event);
        },
        onError: (Object err) {
          if (!mounted) return;
          setState(() => _errorMessage = '同步遥测流中断：$err');
        },
      );
    } catch (err) {
      _errorMessage = '未连接到本地 Rust 引擎：$err';
    }
  }

  Future<void> _toggleAutoReconnect() async {
    if (_togglingReconnect) return;
    final nextValue = !_autoReconnectEnabled;

    setState(() {
      _togglingReconnect = true;
      _errorMessage = null;
    });
    try {
      await widget.bridge.enableAutoReconnect(nextValue);
      if (!mounted) return;
      setState(() {
        _autoReconnectEnabled = nextValue;
        _togglingReconnect = false;
        _statusMessage = nextValue ? '已开启客户端自愈重连。' : '已暂停客户端自愈重连。';
      });
    } catch (err) {
      if (!mounted) return;
      setState(() {
        _togglingReconnect = false;
        _errorMessage = '更新重连策略失败：$err';
      });
    }
  }

  Future<void> _handleGracefulRestart() async {
    setState(() {
      _restarting = true;
      _errorMessage = null;
      _statusMessage = '正在停止新任务并刷盘 WAL（最多等待 3 秒）…';
    });
    try {
      final ok = await widget.bridge.executeGracefulRestart();
      if (!mounted) return;
      setState(() {
        _restarting = false;
        _statusMessage = ok ? '服务端已完成优雅重启。' : '重启指令已下发，但守护通道返回未就绪。';
      });
    } catch (err) {
      if (!mounted) return;
      setState(() {
        _restarting = false;
        _errorMessage = '优雅重启失败：$err';
      });
    }
  }

  Future<void> _requestAndroidBatteryExemption() async {
    try {
      final granted = await widget.bridge.requestBatteryExemption();
      if (!mounted) return;
      setState(() {
        _statusMessage = granted ? '已获得电池优化豁免。' : '请在系统设置中允许后台无限制运行。';
      });
    } catch (err) {
      if (!mounted) return;
      setState(() => _errorMessage = '请求电池优化豁免失败：$err');
    }
  }

  String _formatBytes(int bytes) {
    if (bytes >= 1024 * 1024 * 1024) {
      return '${(bytes / (1024 * 1024 * 1024)).toStringAsFixed(2)} GB';
    }
    if (bytes >= 1024 * 1024) {
      return '${(bytes / (1024 * 1024)).toStringAsFixed(2)} MB';
    }
    if (bytes >= 1024) {
      return '${(bytes / 1024).toStringAsFixed(1)} KB';
    }
    return '$bytes B';
  }

  String _formatThroughput(int bps) {
    if (bps <= 0) return '0 B/s';
    return '${_formatBytes(bps)}/s';
  }

  Color _statusColor(String status) {
    return switch (status.toLowerCase()) {
      'completed' || 'idle' || 'healthy' || 'up_to_date' => NeuTokens.healthy,
      'error' || 'failed' || 'corrupted' => NeuTokens.danger,
      _ => NeuTokens.accent,
    };
  }

  Color _bitRotColor(BitRotHealth health) {
    return switch (health) {
      BitRotHealth.healthy => NeuTokens.healthy,
      BitRotHealth.degraded => NeuTokens.warning,
      BitRotHealth.corrupted => NeuTokens.danger,
    };
  }

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    final sync = _latestSync;
    final syncStatus =
        sync == null || sync.status.isEmpty ? 'idle' : sync.status;
    final bitRot = sync?.bitRotHealth ?? BitRotHealth.healthy;

    return ValueListenableBuilder<RoleMode>(
      valueListenable: widget.bridge.roleModeNotifier,
      builder: (context, roleMode, _) {
        return SingleChildScrollView(
          padding: const EdgeInsets.all(NeuSpace.lg),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: <Widget>[
              Text('概览', style: textTheme.headlineSmall),
              const SizedBox(height: NeuSpace.md),
              NeuSegmentedControl<RoleMode>(
                semanticLabel: '运行模式切换',
                value: roleMode,
                enabled: !_switchingMode,
                onChanged: _handleModeSelected,
                segments: <NeuSegment<RoleMode>>[
                  for (final mode in RoleMode.values)
                    NeuSegment<RoleMode>(value: mode, label: mode.label),
                ],
              ),
              const SizedBox(height: NeuSpace.lg),
              if (_errorMessage != null) ...<Widget>[
                NeuCard(
                  inset: true,
                  child: Text(
                    _errorMessage!,
                    style:
                        textTheme.bodyMedium?.copyWith(color: NeuTokens.danger),
                  ),
                ),
                const SizedBox(height: NeuSpace.md),
              ],
              if (_statusMessage != null) ...<Widget>[
                NeuCard(
                  inset: true,
                  child: Text(
                    _statusMessage!,
                    style: textTheme.bodyMedium
                        ?.copyWith(color: NeuTokens.healthy),
                  ),
                ),
                const SizedBox(height: NeuSpace.md),
              ],
              if (roleMode.hasServer) ...<Widget>[
                _buildServerCard(context, bitRot, sync?.corruptedChunks ?? 0),
                if (roleMode.hasClient)
                  const SizedBox(height: NeuSpace.md),
              ],
              if (roleMode.hasClient)
                _buildClientCard(context, sync, syncStatus),
            ],
          ),
        );
      },
    );
  }

  Widget _buildServerCard(
    BuildContext context,
    BitRotHealth bitRot,
    int corruptedChunks,
  ) {
    final textTheme = Theme.of(context).textTheme;
    return NeuCard(
      semanticLabel: '服务端保活与 Bit-Rot 健康状态',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Text('服务端状态', style: textTheme.titleMedium),
          const SizedBox(height: NeuSpace.md),
          _MetricRow(
            label: '当前保活等级',
            value: _loadingLevel
                ? '探测中…'
                : (_keepAliveLevel?.displayLabel ?? '未知'),
            valueColor: NeuTokens.accent,
          ),
          const SizedBox(height: NeuSpace.sm),
          _MetricRow(
            label: '协议标识',
            value: _keepAliveLevel?.wireValue ?? '—',
          ),
          const SizedBox(height: NeuSpace.sm),
          _MetricRow(
            label: 'Bit-Rot 健康状态',
            value: corruptedChunks > 0
                ? '${bitRot.displayLabel}（$corruptedChunks 块异常）'
                : bitRot.displayLabel,
            valueColor: _bitRotColor(bitRot),
          ),
          const SizedBox(height: NeuSpace.md),
          Wrap(
            spacing: NeuSpace.sm,
            runSpacing: NeuSpace.sm,
            children: <Widget>[
              NeuButton(
                label: '一键优雅重启',
                icon: Icons.restart_alt,
                tone: NeuButtonTone.primary,
                loading: _restarting,
                onPressed: _handleGracefulRestart,
              ),
              NeuButton(
                label: '重新探测等级',
                icon: Icons.refresh,
                onPressed: _loadKeepAliveLevel,
              ),
              if (Platform.isAndroid)
                NeuButton(
                  label: '申请电池白名单',
                  icon: Icons.battery_saver_outlined,
                  onPressed: _requestAndroidBatteryExemption,
                ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _buildClientCard(
    BuildContext context,
    SyncProgressEvent? sync,
    String syncStatus,
  ) {
    final textTheme = Theme.of(context).textTheme;
    return NeuCard(
      semanticLabel: '客户端同步与重连状态',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Text('客户端同步状态', style: textTheme.titleMedium),
          const SizedBox(height: NeuSpace.md),
          _MetricRow(
            label: '同步状态',
            value: syncStatus,
            valueColor: _statusColor(syncStatus),
          ),
          const SizedBox(height: NeuSpace.sm),
          _MetricRow(
            label: '自愈重连',
            value: _autoReconnectEnabled ? '已开启（2s → 30s 退避）' : '已暂停',
            valueColor: _autoReconnectEnabled
                ? NeuTokens.healthy
                : NeuTokens.textSecondary,
          ),
          const SizedBox(height: NeuSpace.sm),
          _MetricRow(
            label: '实时同步速率',
            value: _formatThroughput(sync?.throughputBps ?? 0),
          ),
          const SizedBox(height: NeuSpace.sm),
          _MetricRow(
            label: '活跃规则 ID',
            value: sync == null || sync.ruleId.isEmpty ? '—' : sync.ruleId,
          ),
          const SizedBox(height: NeuSpace.sm),
          _MetricRow(
            label: '当前处理文件',
            value: sync == null || sync.filePath.isEmpty ? '空闲' : sync.filePath,
          ),
          const SizedBox(height: NeuSpace.sm),
          _MetricRow(
            label: '已处理数据量',
            value: _formatBytes(sync?.bytesProcessed ?? 0),
          ),
          const SizedBox(height: NeuSpace.sm),
          _MetricRow(
            label: '分块上传 / 秒传去重',
            value: sync == null
                ? '0 / 0'
                : '${sync.chunksUploaded} / ${sync.chunksDeduplicated}',
          ),
          const SizedBox(height: NeuSpace.md),
          NeuProgressBar(
            value: sync?.dedupRatio ?? 0.0,
            semanticLabel: '分块去重命中率',
          ),
          const SizedBox(height: NeuSpace.md),
          NeuButton(
            label: _autoReconnectEnabled ? '暂停自愈重连' : '开启自愈重连',
            icon: _autoReconnectEnabled ? Icons.link_off : Icons.link,
            loading: _togglingReconnect,
            onPressed: _toggleAutoReconnect,
          ),
        ],
      ),
    );
  }
}

class _MetricRow extends StatelessWidget {
  const _MetricRow({
    required this.label,
    required this.value,
    this.valueColor,
  });

  final String label;
  final String value;
  final Color? valueColor;

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    return Row(
      mainAxisAlignment: MainAxisAlignment.spaceBetween,
      children: <Widget>[
        Text(label, style: textTheme.bodySmall),
        const SizedBox(width: NeuSpace.md),
        Flexible(
          child: Text(
            value,
            textAlign: TextAlign.right,
            overflow: TextOverflow.ellipsis,
            style: textTheme.titleSmall?.copyWith(
              color: valueColor ?? NeuTokens.textPrimary,
            ),
          ),
        ),
      ],
    );
  }
}
