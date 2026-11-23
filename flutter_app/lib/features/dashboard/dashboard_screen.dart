import 'dart:async';
import 'dart:io';

import 'package:material_ui/material_ui.dart';

import '../../bridge/lanvault_bridge.dart';
import '../../bridge/system_env_helper.dart';
import '../../theme/neu_theme.dart';
import '../../theme/neu_toast.dart';

/// 1. 总览页：
/// 还原设计稿顶行 3 张 KPI 卡（系统健康绿盾牌卡、在线设备、已用空间）与
/// 下方左右双栏（左：存储使用环形图与存储池分项；右：最近活动流与服务端快捷控制）。
class DashboardScreen extends StatefulWidget {
  const DashboardScreen({
    super.key,
    required this.bridge,
    this.onOpenSnapshots,
  });

  final LanVaultBridge bridge;
  final VoidCallback? onOpenSnapshots;

  @override
  State<DashboardScreen> createState() => _DashboardScreenState();
}

class _DashboardScreenState extends State<DashboardScreen> {
  static const Duration _syncErrorToastCooldown = Duration(seconds: 15);
  static const List<Color> _vaultLegendColors = <Color>[
    NeuTokens.accent,
    NeuTokens.chartSky,
    NeuTokens.chartIndigo,
    NeuTokens.healthy,
  ];

  SystemOverviewModel? _overview;
  List<VaultSummaryModel> _vaults = const <VaultSummaryModel>[];
  KeepAliveLevel? _keepAliveLevel;
  SyncProgressEvent? _latestSync;
  StreamSubscription<SyncProgressEvent>? _syncSub;
  DateTime? _lastSyncErrorToastAt;

  bool _loading = true;
  bool _restarting = false;
  bool _stopping = false;

  @override
  void initState() {
    super.initState();
    widget.bridge.roleModeNotifier.addListener(_handleRoleModeChanged);
    _refreshAll();
    _subscribeTelemetry();
  }

  @override
  void didUpdateWidget(covariant DashboardScreen oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.bridge != widget.bridge) {
      oldWidget.bridge.roleModeNotifier.removeListener(_handleRoleModeChanged);
      widget.bridge.roleModeNotifier.addListener(_handleRoleModeChanged);
      _syncSub?.cancel();
      _subscribeTelemetry();
      _refreshAll();
    }
  }

  @override
  void dispose() {
    widget.bridge.roleModeNotifier.removeListener(_handleRoleModeChanged);
    _syncSub?.cancel();
    super.dispose();
  }

  void _handleRoleModeChanged() {
    if (!mounted) return;
    _refreshAll();
  }

  Future<void> _refreshAll() async {
    setState(() => _loading = true);
    try {
      final results = await Future.wait<Object>(<Future<Object>>[
        widget.bridge.getSystemOverview(),
        widget.bridge.listVaults(),
        widget.bridge.getRoleMode(),
        widget.bridge.getAutoReconnect(),
      ]);
      final overview = results[0] as SystemOverviewModel;
      final vaults = results[1] as List<VaultSummaryModel>;
      final mode = results[2] as RoleMode;

      KeepAliveLevel? level;
      if (mode.hasServer) {
        level = await widget.bridge.detectKeepAliveLevel();
      }
      if (!mounted) return;
      setState(() {
        _overview = overview;
        _vaults = vaults;
        _keepAliveLevel = level;
        _loading = false;
      });
    } catch (err) {
      if (!mounted) return;
      setState(() => _loading = false);
      NeuToast.error(context, '加载系统总览失败：$err');
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
          final now = DateTime.now();
          if (_lastSyncErrorToastAt == null ||
              now.difference(_lastSyncErrorToastAt!) >=
                  _syncErrorToastCooldown) {
            _lastSyncErrorToastAt = now;
            NeuToast.error(context, '同步状态更新中断：$err');
          }
        },
      );
    } catch (err) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) {
          NeuToast.error(context, '未连接到本地引擎：$err');
        }
      });
    }
  }

  Future<void> _handleGracefulRestart() async {
    setState(() => _restarting = true);
    NeuToast.info(context, '正在安全重启服务，请稍候…');
    try {
      final ok = await widget.bridge.executeGracefulRestart();
      await _refreshAll();
      if (!mounted) return;
      setState(() => _restarting = false);
      if (ok) {
        NeuToast.success(context, '服务端已完成重启');
      } else {
        NeuToast.warning(context, '重启指令已下发，但守护进程尚未就绪');
      }
    } catch (err) {
      if (!mounted) return;
      setState(() => _restarting = false);
      NeuToast.error(context, '重启失败：$err');
    }
  }

  Future<void> _handleStopServer() async {
    setState(() => _stopping = true);
    try {
      final stopped = await widget.bridge.stopLocalServer();
      await _refreshAll();
      if (!mounted) return;
      setState(() => _stopping = false);
      if (stopped) {
        NeuToast.success(context, '本地服务端已停止');
      } else {
        NeuToast.warning(context, '未能停止本地服务端，请检查守护进程状态');
      }
    } catch (err) {
      if (!mounted) return;
      setState(() => _stopping = false);
      NeuToast.error(context, '停止服务端失败：$err');
    }
  }

  Future<void> _requestAndroidBatteryExemption() async {
    try {
      final granted = await widget.bridge.requestBatteryExemption();
      if (!mounted) return;
      if (granted) {
        NeuToast.success(context, '已获得电池优化豁免');
      } else {
        NeuToast.info(context, '请在系统设置中允许后台无限制运行');
      }
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, '请求电池优化豁免失败：$err');
    }
  }

  (String title, String subtitle, Color accent, Color softBg, IconData icon)
      _resolveHealthVisual(SystemOverviewModel? overview) {
    if (overview == null) {
      return (
        _loading ? '正在检测…' : '等待连接',
        '正在读取本地引擎状态',
        NeuTokens.textSecondary,
        NeuTokens.insetSurface,
        Icons.shield_outlined,
      );
    }
    if (!overview.serverRunning) {
      return (
        '服务未启动',
        '前往“存储”页启动本机存储服务',
        NeuTokens.textSecondary,
        NeuTokens.insetSurface,
        Icons.shield_outlined,
      );
    }
    final portSuffix =
        overview.serverPort != null ? ' · 端口 :${overview.serverPort}' : '';
    return switch (overview.bitRotHealth) {
      BitRotHealth.healthy => (
          '系统正常',
          '所有服务运行正常$portSuffix',
          NeuTokens.healthy,
          NeuTokens.healthySoft,
          Icons.verified_user_rounded,
        ),
      BitRotHealth.degraded => (
          '状态降级',
          '部分卷处于降级状态$portSuffix',
          NeuTokens.warning,
          NeuTokens.folderGoldSoft,
          Icons.gpp_maybe_rounded,
        ),
      BitRotHealth.corrupted => (
          '发现损坏块',
          '检测到 ${overview.corruptedChunks} 个异常数据块，建议立即校验修复',
          NeuTokens.danger,
          NeuTokens.dangerSoft,
          Icons.gpp_bad_rounded,
        ),
      BitRotHealth.unknown => (
          '系统正常',
          '服务端运行中$portSuffix · 尚待首次巡检',
          NeuTokens.healthy,
          NeuTokens.healthySoft,
          Icons.verified_user_rounded,
        ),
    };
  }

  (IconData, Color, Color) _activityVisual(String kind) {
    return switch (kind.toLowerCase()) {
      'backup' || 'sync' => (
          Icons.check_circle_rounded,
          NeuTokens.healthy,
          NeuTokens.healthySoft,
        ),
      'device' || 'pairing' => (
          Icons.devices_rounded,
          NeuTokens.accent,
          NeuTokens.accentSoft,
        ),
      'scrub' || 'verify' => (
          Icons.fact_check_rounded,
          NeuTokens.chartIndigo,
          NeuTokens.chartIndigoSoft,
        ),
      'error' || 'warning' => (
          Icons.warning_amber_rounded,
          NeuTokens.warning,
          NeuTokens.folderGoldSoft,
        ),
      _ => (
          Icons.bolt_rounded,
          NeuTokens.accent,
          NeuTokens.accentSoft,
        ),
    };
  }

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    final isWide = MediaQuery.sizeOf(context).width >= 840;
    final overview = _overview;
    final sync = _latestSync;
    final isSyncActive = sync != null &&
        sync.status.isNotEmpty &&
        sync.status.toLowerCase() != 'idle' &&
        sync.status.toLowerCase() != 'up_to_date';

    return ValueListenableBuilder<RoleMode>(
      valueListenable: widget.bridge.roleModeNotifier,
      builder: (context, roleMode, _) {
        return SingleChildScrollView(
          padding: EdgeInsets.all(isWide ? NeuSpace.lg : NeuSpace.md),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: <Widget>[
              if (isWide) ...<Widget>[
                Row(
                  children: <Widget>[
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: <Widget>[
                          Text('总览', style: textTheme.headlineSmall),
                          const SizedBox(height: 4),
                          Text(
                            '欢迎回来，数据正在安全守护中',
                            style: textTheme.bodySmall,
                          ),
                        ],
                      ),
                    ),
                    NeuStatusBadge(
                      label: roleMode.label,
                      tone: NeuStatusTone.accent,
                    ),
                    const SizedBox(width: NeuSpace.sm),
                    NeuButton(
                      label: '刷新',
                      icon: Icons.refresh_rounded,
                      loading: _loading,
                      onPressed: _refreshAll,
                    ),
                  ],
                ),
                const SizedBox(height: 20),
              ],
              if (isWide) ...<Widget>[
                _buildKpiRow(context, isWide, overview),
                if (isSyncActive) ...<Widget>[
                  const SizedBox(height: NeuSpace.md),
                  _buildLiveSyncBanner(context, sync),
                ],
                const SizedBox(height: NeuSpace.md),
                Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: <Widget>[
                    Expanded(
                      flex: 11,
                      child: _buildStorageUsageCard(context, overview),
                    ),
                    const SizedBox(width: NeuSpace.md),
                    Expanded(
                      flex: 10,
                      child: _buildRecentActivitiesCard(
                        context,
                        overview,
                        roleMode,
                      ),
                    ),
                  ],
                ),
              ] else ...<Widget>[
                _buildHealthCard(context, overview),
                if (isSyncActive) ...<Widget>[
                  const SizedBox(height: 12),
                  _buildLiveSyncBanner(context, sync),
                ],
                const SizedBox(height: 12),
                _buildStorageUsageCard(context, overview),
                const SizedBox(height: 12),
                _buildMobileCompactKpiRow(context, overview),
                const SizedBox(height: 12),
                _buildMobileDataHealthCard(context, overview),
                const SizedBox(height: 12),
                _buildRecentActivitiesCard(context, overview, roleMode),
              ],
            ],
          ),
        );
      },
    );
  }

  Widget _buildHealthCard(
    BuildContext context,
    SystemOverviewModel? overview,
  ) {
    final (healthTitle, healthSub, healthColor, healthBg, healthIcon) =
        _resolveHealthVisual(overview);
    return NeuCard(
      backgroundColor: healthBg,
      borderColor: healthColor.withValues(alpha: 0.22),
      padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 18),
      child: Row(
        children: <Widget>[
          Container(
            width: 44,
            height: 44,
            decoration: BoxDecoration(
              color: healthColor.withValues(alpha: 0.14),
              borderRadius: const BorderRadius.all(Radius.circular(14)),
            ),
            child: Icon(healthIcon, size: 26, color: healthColor),
          ),
          const SizedBox(width: 14),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: <Widget>[
                Text(
                  healthTitle,
                  style: Theme.of(context).textTheme.titleMedium?.copyWith(
                        fontSize: 17,
                        fontWeight: FontWeight.w700,
                      ),
                ),
                const SizedBox(height: 3),
                Text(
                  healthSub,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: Theme.of(context).textTheme.bodySmall,
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildMobileCompactKpiRow(
    BuildContext context,
    SystemOverviewModel? overview,
  ) {
    return Row(
      children: <Widget>[
        Expanded(
          child: NeuCard(
            padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 14),
            child: Row(
              children: <Widget>[
                Container(
                  width: 36,
                  height: 36,
                  decoration: const BoxDecoration(
                    color: NeuTokens.accentSoft,
                    borderRadius: BorderRadius.all(Radius.circular(10)),
                  ),
                  child: const Icon(
                    Icons.devices_rounded,
                    size: 19,
                    color: NeuTokens.accent,
                  ),
                ),
                const SizedBox(width: 10),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: <Widget>[
                      Text(
                        '${overview?.onlinePeerCount ?? 0}',
                        style: Theme.of(context).textTheme.titleLarge?.copyWith(
                              fontWeight: FontWeight.w700,
                              fontFeatures: const <FontFeature>[
                                FontFeature.tabularFigures(),
                              ],
                            ),
                      ),
                      Text(
                        '在线设备',
                        overflow: TextOverflow.ellipsis,
                        style: Theme.of(context).textTheme.bodySmall,
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ),
        const SizedBox(width: 12),
        Expanded(
          child: NeuCard(
            padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 14),
            child: Row(
              children: <Widget>[
                Container(
                  width: 36,
                  height: 36,
                  decoration: const BoxDecoration(
                    color: NeuTokens.accentSoft,
                    borderRadius: BorderRadius.all(Radius.circular(10)),
                  ),
                  child: const Icon(
                    Icons.cloud_sync_rounded,
                    size: 19,
                    color: NeuTokens.accent,
                  ),
                ),
                const SizedBox(width: 10),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: <Widget>[
                      Text(
                        '${overview?.syncRuleCount ?? 0}',
                        style: Theme.of(context).textTheme.titleLarge?.copyWith(
                              fontWeight: FontWeight.w700,
                              fontFeatures: const <FontFeature>[
                                FontFeature.tabularFigures(),
                              ],
                            ),
                      ),
                      Text(
                        '备份任务',
                        overflow: TextOverflow.ellipsis,
                        style: Theme.of(context).textTheme.bodySmall,
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ),
      ],
    );
  }

  Widget _buildMobileDataHealthCard(
    BuildContext context,
    SystemOverviewModel? overview,
  ) {
    final (_, _, healthColor, healthBg, _) = _resolveHealthVisual(overview);
    final summary = overview == null
        ? '点击查看快照与完整性校验'
        : (overview.corruptedChunks > 0
            ? '检测到 ${overview.corruptedChunks} 个异常块 · 点击修复'
            : '最近巡检正常 · 点击进入快照与校验');
    return NeuCard(
      onTap: widget.onOpenSnapshots,
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
        child: Row(
          children: <Widget>[
            Container(
              width: 38,
              height: 38,
              decoration: BoxDecoration(
                color: healthBg,
                borderRadius: const BorderRadius.all(Radius.circular(10)),
              ),
              child: Icon(
                Icons.health_and_safety_rounded,
                size: 20,
                color: healthColor,
              ),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: <Widget>[
                  Text('数据健康', style: Theme.of(context).textTheme.titleSmall),
                  const SizedBox(height: 2),
                  Text(
                    summary,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                ],
              ),
            ),
            const Icon(
              Icons.chevron_right_rounded,
              color: NeuTokens.textSecondary,
            ),
          ],
        ),
      );
  }

  Widget _buildKpiRow(
    BuildContext context,
    bool isWide,
    SystemOverviewModel? overview,
  ) {
    final healthCard = _buildHealthCard(context, overview);
    final devicesCard = NeuCard(
      padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 18),
      child: Row(
        children: <Widget>[
          Container(
            width: 42,
            height: 42,
            decoration: const BoxDecoration(
              color: NeuTokens.accentSoft,
              borderRadius: BorderRadius.all(Radius.circular(12)),
            ),
            child: const Icon(
              Icons.devices_rounded,
              size: 22,
              color: NeuTokens.accent,
            ),
          ),
          const SizedBox(width: 14),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: <Widget>[
                Text(
                  '${overview?.onlinePeerCount ?? 0}',
                  style: Theme.of(context).textTheme.headlineSmall?.copyWith(
                        fontSize: 22,
                        fontFeatures: const <FontFeature>[
                          FontFeature.tabularFigures(),
                        ],
                      ),
                ),
                const SizedBox(height: 2),
                Text(
                  '在线设备 · 共 ${overview?.pairedPeerCount ?? 0} 台',
                  overflow: TextOverflow.ellipsis,
                  style: Theme.of(context).textTheme.bodySmall,
                ),
              ],
            ),
          ),
        ],
      ),
    );

    final spaceCard = NeuCard(
      padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 18),
      child: Row(
        children: <Widget>[
          Container(
            width: 42,
            height: 42,
            decoration: const BoxDecoration(
              color: NeuTokens.accentSoft,
              borderRadius: BorderRadius.all(Radius.circular(12)),
            ),
            child: const Icon(
              Icons.layers_rounded,
              size: 22,
              color: NeuTokens.accent,
            ),
          ),
          const SizedBox(width: 14),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: <Widget>[
                Text(
                  SystemEnvHelper.formatBytes(overview?.vaultUsedBytes ?? 0),
                  overflow: TextOverflow.ellipsis,
                  style: Theme.of(context).textTheme.headlineSmall?.copyWith(
                        fontSize: 22,
                        fontFeatures: const <FontFeature>[
                          FontFeature.tabularFigures(),
                        ],
                      ),
                ),
                const SizedBox(height: 2),
                Text(
                  '已用空间 · ${overview?.syncRuleCount ?? 0} 个备份任务',
                  overflow: TextOverflow.ellipsis,
                  style: Theme.of(context).textTheme.bodySmall,
                ),
              ],
            ),
          ),
        ],
      ),
    );

    if (isWide) {
      return Row(
        children: <Widget>[
          Expanded(flex: 12, child: healthCard),
          const SizedBox(width: NeuSpace.md),
          Expanded(flex: 9, child: devicesCard),
          const SizedBox(width: NeuSpace.md),
          Expanded(flex: 10, child: spaceCard),
        ],
      );
    }

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: <Widget>[
        healthCard,
        const SizedBox(height: 12),
        Row(
          children: <Widget>[
            Expanded(child: devicesCard),
            const SizedBox(width: 12),
            Expanded(child: spaceCard),
          ],
        ),
      ],
    );
  }

  Widget _buildLiveSyncBanner(BuildContext context, SyncProgressEvent sync) {
    final throughput = sync.throughputBps > 0
        ? '${SystemEnvHelper.formatBytes(sync.throughputBps)}/s'
        : '计算速率中';
    return NeuCard(
      backgroundColor: NeuTokens.accentSoft,
      borderColor: NeuTokens.accent.withValues(alpha: 0.25),
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Row(
            children: <Widget>[
              const Icon(
                Icons.sync_rounded,
                size: 18,
                color: NeuTokens.accent,
              ),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  '正在同步：${sync.filePath.isEmpty ? sync.ruleId : SystemEnvHelper.folderDisplayName(sync.filePath)}',
                  overflow: TextOverflow.ellipsis,
                  style: Theme.of(context).textTheme.titleSmall?.copyWith(
                        color: NeuTokens.accent,
                      ),
                ),
              ),
              Text(
                throughput,
                style: Theme.of(context).textTheme.labelMedium?.copyWith(
                      color: NeuTokens.accent,
                    ),
              ),
            ],
          ),
          const SizedBox(height: 8),
          NeuProgressBar(
            value: sync.progressRatio,
            height: 7,
            semanticLabel: '当前同步进度',
          ),
        ],
      ),
    );
  }

  Widget _buildStorageUsageCard(
    BuildContext context,
    SystemOverviewModel? overview,
  ) {
    final textTheme = Theme.of(context).textTheme;
    final usedStr = SystemEnvHelper.formatBytes(overview?.vaultUsedBytes ?? 0);
    final quotaStr =
        SystemEnvHelper.formatBytes(overview?.vaultQuotaBytes ?? 0);

    return NeuCard(
      padding: const EdgeInsets.all(20),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: <Widget>[
              Text('存储使用', style: textTheme.titleMedium),
              Text(
                '$usedStr / $quotaStr',
                style: textTheme.bodySmall?.copyWith(
                  fontWeight: FontWeight.w600,
                  fontFeatures: const <FontFeature>[
                    FontFeature.tabularFigures(),
                  ],
                ),
              ),
            ],
          ),
          const SizedBox(height: 18),
          Row(
            crossAxisAlignment: CrossAxisAlignment.center,
            children: <Widget>[
              StorageDonutRing(
                ratio: overview?.storageRatio ?? 0.0,
                size: 112,
                strokeWidth: 11,
                centerLabel: '已使用',
              ),
              const SizedBox(width: 22),
              Expanded(
                child: _vaults.isEmpty
                    ? Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: <Widget>[
                          Text(
                            '暂无存储池',
                            style: textTheme.titleSmall,
                          ),
                          const SizedBox(height: 4),
                          Text(
                            '前往“存储”页创建或挂载您的本地加密存储池。',
                            style: textTheme.bodySmall,
                          ),
                        ],
                      )
                    : Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: <Widget>[
                          for (var i = 0; i < _vaults.length; i++) ...<Widget>[
                            Row(
                              children: <Widget>[
                                Container(
                                  width: 8,
                                  height: 8,
                                  decoration: BoxDecoration(
                                    color: _vaultLegendColors[
                                        i % _vaultLegendColors.length],
                                    shape: BoxShape.circle,
                                  ),
                                ),
                                const SizedBox(width: 8),
                                Expanded(
                                  child: Text(
                                    _vaults[i].name,
                                    overflow: TextOverflow.ellipsis,
                                    style: textTheme.bodyMedium?.copyWith(
                                      fontSize: 13.5,
                                    ),
                                  ),
                                ),
                                const SizedBox(width: 8),
                                Text(
                                  SystemEnvHelper.formatBytes(
                                    _vaults[i].usedBytes,
                                  ),
                                  style: textTheme.labelMedium?.copyWith(
                                    color: NeuTokens.textPrimary,
                                    fontFeatures: const <FontFeature>[
                                      FontFeature.tabularFigures(),
                                    ],
                                  ),
                                ),
                              ],
                            ),
                            if (i < _vaults.length - 1)
                              const SizedBox(height: 10),
                          ],
                        ],
                      ),
              ),
            ],
          ),
          if (_keepAliveLevel != null) ...<Widget>[
            const SizedBox(height: 18),
            Container(
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 9),
              decoration: BoxDecoration(
                color: NeuTokens.insetSurface,
                borderRadius: const BorderRadius.all(Radius.circular(10)),
              ),
              child: Row(
                children: <Widget>[
                  const Icon(
                    Icons.memory_rounded,
                    size: 16,
                    color: NeuTokens.accent,
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Text(
                      '保活等级：${_keepAliveLevel!.displayLabel}',
                      overflow: TextOverflow.ellipsis,
                      style: textTheme.bodySmall?.copyWith(
                        color: NeuTokens.textPrimary,
                        fontWeight: FontWeight.w500,
                      ),
                    ),
                  ),
                  if (overview != null && overview.lanIp.isNotEmpty)
                    Text(
                      overview.lanIp,
                      style: textTheme.labelMedium?.copyWith(
                        color: NeuTokens.textSecondary,
                      ),
                    ),
                ],
              ),
            ),
          ],
        ],
      ),
    );
  }

  Widget _buildRecentActivitiesCard(
    BuildContext context,
    SystemOverviewModel? overview,
    RoleMode roleMode,
  ) {
    final textTheme = Theme.of(context).textTheme;
    final activities = overview?.recentActivities ?? const <RecentActivityModel>[];

    return NeuCard(
      padding: const EdgeInsets.all(20),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: <Widget>[
              Text('最近活动', style: textTheme.titleMedium),
              Text(
                '${activities.length} 条记录',
                style: textTheme.labelMedium,
              ),
            ],
          ),
          const SizedBox(height: 14),
          if (activities.isEmpty)
            Container(
              width: double.infinity,
              padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 20),
              decoration: BoxDecoration(
                color: NeuTokens.insetSurface,
                borderRadius: const BorderRadius.all(Radius.circular(12)),
              ),
              child: Text(
                '暂无最近活动记录。开始备份、配对设备或执行完整性校验后将在此实时显示。',
                style: textTheme.bodySmall,
              ),
            )
          else
            for (var i = 0; i < activities.length; i++) ...<Widget>[
              Builder(
                builder: (context) {
                  final item = activities[i];
                  final (icon, fg, bg) = _activityVisual(item.kind);
                  return Row(
                    crossAxisAlignment: CrossAxisAlignment.center,
                    children: <Widget>[
                      Container(
                        width: 32,
                        height: 32,
                        decoration: BoxDecoration(
                          color: bg,
                          borderRadius:
                              const BorderRadius.all(Radius.circular(10)),
                        ),
                        child: Icon(icon, size: 17, color: fg),
                      ),
                      const SizedBox(width: 12),
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: <Widget>[
                            Text(
                              item.title,
                              overflow: TextOverflow.ellipsis,
                              style: textTheme.titleSmall,
                            ),
                            if (item.detail.isNotEmpty)
                              Text(
                                item.detail,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: textTheme.bodySmall,
                              ),
                          ],
                        ),
                      ),
                      const SizedBox(width: 8),
                      Text(
                        item.timestamp,
                        style: textTheme.labelMedium?.copyWith(
                          fontFeatures: const <FontFeature>[
                            FontFeature.tabularFigures(),
                          ],
                        ),
                      ),
                    ],
                  );
                },
              ),
              if (i < activities.length - 1)
                const Padding(
                  padding: EdgeInsets.symmetric(vertical: 10),
                  child: Divider(height: 1),
                ),
            ],
          if (roleMode.hasServer) ...<Widget>[
            const SizedBox(height: 18),
            const Divider(height: 1),
            const SizedBox(height: 14),
            Wrap(
              spacing: NeuSpace.sm,
              runSpacing: NeuSpace.sm,
              children: <Widget>[
                NeuButton(
                  label: '重启服务端',
                  icon: Icons.restart_alt_rounded,
                  tone: NeuButtonTone.primary,
                  loading: _restarting,
                  onPressed: _handleGracefulRestart,
                ),
                NeuButton(
                  label: '停止服务端',
                  icon: Icons.stop_circle_outlined,
                  tone: NeuButtonTone.danger,
                  loading: _stopping,
                  onPressed:
                      overview?.serverRunning == true ? _handleStopServer : null,
                ),
                if (Platform.isAndroid)
                  NeuButton(
                    label: '电池白名单',
                    icon: Icons.battery_saver_outlined,
                    onPressed: _requestAndroidBatteryExemption,
                  ),
              ],
            ),
          ],
        ],
      ),
    );
  }
}
