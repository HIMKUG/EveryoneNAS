import 'dart:async';
import 'dart:io';
import 'dart:math' as math;

import 'package:material_ui/material_ui.dart';

import '../../bridge/lanvault_bridge.dart';
import '../../bridge/system_env_helper.dart';
import '../../theme/neu_theme.dart';
import '../../theme/neu_toast.dart';

/// 总览页（重构版）：
/// - 唯一重心：一眼确认系统是否正常、空间剩多少、最近发生了什么。
/// - 彻底移除“最近活动”底部的重启/停止服务端/电池白名单堆砌按钮与截断的 Level 0/虚拟 IP 条。
/// - 在线设备数与 pairedPeersNotifier 实时对齐，时间戳格式化为可读时间。
class DashboardScreen extends StatefulWidget {
  const DashboardScreen({
    super.key,
    required this.bridge,
    this.onOpenSnapshots,
    this.onOpenVault,
    this.onOpenPairing,
    this.onOpenSettings,
  });

  final LanVaultBridge bridge;
  final VoidCallback? onOpenSnapshots;
  final VoidCallback? onOpenVault;
  final VoidCallback? onOpenPairing;
  final VoidCallback? onOpenSettings;

  @override
  State<DashboardScreen> createState() => _DashboardScreenState();
}

class _DashboardScreenState extends State<DashboardScreen> with WidgetsBindingObserver {
  SystemOverviewModel? _overview;
  List<VaultSummaryModel> _vaults = const <VaultSummaryModel>[];
  SyncProgressEvent? _lastSync;
  StreamSubscription<SyncProgressEvent>? _syncSub;
  Timer? _pollTimer;
  bool _loading = true;
  bool _loadFailed = false;
  bool _refreshing = false;
  bool _batteryBannerDismissed = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _syncSub = widget.bridge.subscribeSyncProgress().listen((event) {
      if (!mounted) return;
      setState(() => _lastSync = event);
    });
    widget.bridge.serverStateVersion.addListener(_onExternalStateChange);
    unawaited(_refreshAll(silent: false));
    _pollTimer = Timer.periodic(
      const Duration(seconds: 6),
      (_) => unawaited(_refreshAll(silent: true)),
    );
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    // 轮询在前后台之间必须停一会：此前退到后台仍按 6s 间隔打 FFI。
    if (state == AppLifecycleState.resumed) {
      _pollTimer ??= Timer.periodic(
        const Duration(seconds: 6),
        (_) => unawaited(_refreshAll(silent: true)),
      );
      unawaited(_refreshAll(silent: true));
    } else {
      _pollTimer?.cancel();
      _pollTimer = null;
    }
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _pollTimer?.cancel();
    _syncSub?.cancel();
    widget.bridge.serverStateVersion.removeListener(_onExternalStateChange);
    super.dispose();
  }

  void _onExternalStateChange() {
    if (!mounted) return;
    unawaited(_refreshAll(silent: true));
  }

  Future<void> _refreshAll({required bool silent}) async {
    if (_refreshing) return;
    _refreshing = true;
    if (!silent && mounted) setState(() => _loading = true);
    try {
      final results = await Future.wait<Object>(<Future<Object>>[
        widget.bridge.getSystemOverview(),
        widget.bridge.listVaults(),
      ]);
      if (!mounted) return;
      setState(() {
        _overview = results[0] as SystemOverviewModel;
        _vaults = results[1] as List<VaultSummaryModel>;
        _loading = false;
        _loadFailed = false;
      });
    } catch (err) {
      if (!mounted) return;
      setState(() {
        _loading = false;
        // 首次加载失败时 _overview 仍为 null，不能把它当成「服务未启动」。
        _loadFailed = _overview == null;
      });
      if (!silent) {
        NeuToast.error(context, LanVaultBridge.formatUserError(err));
      }
    } finally {
      _refreshing = false;
    }
  }

  Future<void> _requestBatteryWhitelist() async {
    try {
      final granted = await widget.bridge.requestBatteryExemption();
      if (!mounted) return;
      if (granted) {
        setState(() => _batteryBannerDismissed = true);
        NeuToast.success(context, '已开启后台电池白名单');
      } else {
        NeuToast.info(context, '请在系统设置中允许 EveryoneNAS 后台无限制运行');
      }
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, LanVaultBridge.formatUserError(err));
    }
  }

  bool _isSyncActive(SyncProgressEvent? sync) {
    if (sync == null) return false;
    final st = sync.status.toLowerCase();
    return st.contains('running') ||
        st.contains('uploading') ||
        st.contains('syncing');
  }

  @override
  Widget build(BuildContext context) {
    final isWide =
        MediaQuery.sizeOf(context).width >= NeuTokens.breakpointWide;

    return ValueListenableBuilder<List<PairedPeerModel>>(
      valueListenable: widget.bridge.pairedPeersNotifier,
      builder: (context, pairedPeers, _) {
        final overview = _overview;
        final liveOnlineCount = math.max(
          overview?.onlinePeerCount ?? 0,
          pairedPeers.where((p) => p.online).length,
        );
        final totalPeers = math.max(
          overview?.pairedPeerCount ?? 0,
          pairedPeers.length,
        );

        return RefreshIndicator(
          onRefresh: () => _refreshAll(silent: false),
          child: SingleChildScrollView(
            physics: const AlwaysScrollableScrollPhysics(),
            padding: EdgeInsets.all(isWide ? NeuSpace.lg : NeuSpace.md),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: <Widget>[
                if (isWide) ...<Widget>[
                  Row(
                    children: <Widget>[
                      Text(
                        '总览',
                        style: Theme.of(context)
                            .textTheme
                            .headlineSmall
                            ?.copyWith(fontWeight: FontWeight.w800),
                      ),
                      const Spacer(),
                      NeuButton(
                        label: '刷新',
                        icon: Icons.refresh_rounded,
                        loading: _loading && overview != null,
                        onPressed: () => _refreshAll(silent: false),
                      ),
                    ],
                  ),
                  const SizedBox(height: NeuSpace.md),
                ],
                if (Platform.isAndroid &&
                    !_batteryBannerDismissed &&
                    (overview?.serverRunning ?? false)) ...<Widget>[
                  _buildAndroidKeepAliveTip(context),
                  const SizedBox(height: NeuSpace.md),
                ],
                if (isWide)
                  _buildDesktopKpiRow(
                    context,
                    overview: overview,
                    onlineCount: liveOnlineCount,
                    totalPeers: totalPeers,
                  )
                else
                  _buildMobileStatusSummary(
                    context,
                    overview: overview,
                    onlineCount: liveOnlineCount,
                    totalPeers: totalPeers,
                  ),
                if (_isSyncActive(_lastSync)) ...<Widget>[
                  const SizedBox(height: NeuSpace.md),
                  _buildActiveSyncBanner(context, _lastSync!),
                ],
                const SizedBox(height: NeuSpace.md),
                if (isWide)
                  Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: <Widget>[
                      Expanded(
                        flex: 5,
                        child: _buildStorageCard(context, overview),
                      ),
                      const SizedBox(width: NeuSpace.md),
                      Expanded(
                        flex: 6,
                        child: _buildRecentActivityCard(context, overview),
                      ),
                    ],
                  )
                else ...<Widget>[
                  _buildStorageCard(context, overview),
                  const SizedBox(height: NeuSpace.md),
                  _buildDataHealthRow(context, overview),
                  const SizedBox(height: NeuSpace.md),
                  _buildRecentActivityCard(context, overview),
                ],
              ],
            ),
          ),
        );
      },
    );
  }

  Widget _buildAndroidKeepAliveTip(BuildContext context) {
      final neu = NeuTheme.of(context);
    return NeuCard(
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
        backgroundColor: neu.accentSoft,
        borderColor: neu.accent.withValues(alpha: 0.2),
      child: Row(
        children: <Widget>[
            Icon(
            Icons.battery_saver_rounded,
            size: 18,
              color: neu.accent,
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Text(
              '开启电池白名单可防止锁屏后后台服务休眠',
              style: Theme.of(context).textTheme.bodySmall?.copyWith(
                      color: neu.textPrimary,
                  ),
            ),
          ),
          TextButton(
            onPressed: _requestBatteryWhitelist,
            child: const Text('去开启'),
          ),
          IconButton(
            tooltip: '关闭提示',
            icon: const Icon(Icons.close_rounded, size: 16),
            visualDensity: VisualDensity.compact,
            onPressed: () => setState(() => _batteryBannerDismissed = true),
          ),
        ],
      ),
    );
  }

  Widget _buildDesktopKpiRow(
    BuildContext context, {
    required SystemOverviewModel? overview,
    required int onlineCount,
    required int totalPeers,
  }) {
      final neu = NeuTheme.of(context);
    final running = overview?.serverRunning ?? false;
    final portText = overview == null
        ? (_loadFailed ? '状态读取失败' : '正在读取状态…')
        : (overview.serverPort != null
            ? '端口 :${overview.serverPort}'
            : '等待启动');
    final lanIp = overview?.lanIp ?? '';
    final showIp = lanIp.isNotEmpty &&
        !lanIp.startsWith('198.18.') &&
        !lanIp.startsWith('172.19.');

    return Row(
      children: <Widget>[
        Expanded(
          child: NeuCard(
            padding: const EdgeInsets.all(18),
              backgroundColor: running ? neu.healthySoft : neu.surface,
            borderColor: running
                  ? neu.healthy.withValues(alpha: 0.25)
                  : neu.hairlineBorder,
            onTap: widget.onOpenSettings,
            child: Row(
              children: <Widget>[
                NeuIconBadge(
                  icon: running
                      ? Icons.verified_user_rounded
                      : Icons.pause_circle_outline_rounded,
                    color: running ? neu.healthy : neu.warning,
                  backgroundColor: running
                        ? neu.healthy.withValues(alpha: 0.14)
                        : neu.folderGoldSoft,
                  size: 44,
                  iconSize: 22,
                ),
                const SizedBox(width: 14),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: <Widget>[
                      Text(
                        running ? '系统正常' : '本地服务未启动',
                        style: Theme.of(context).textTheme.titleMedium?.copyWith(
                              fontWeight: FontWeight.w700,
                            ),
                      ),
                      const SizedBox(height: 3),
                      Text(
                        showIp ? '$portText · $lanIp' : portText,
                        maxLines: 1,
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
        const SizedBox(width: NeuSpace.md),
        Expanded(
          child: NeuCard(
            padding: const EdgeInsets.all(18),
            onTap: widget.onOpenPairing,
            child: Row(
              children: <Widget>[
                  NeuIconBadge(
                  icon: Icons.devices_rounded,
                    color: neu.accent,
                    backgroundColor: neu.accentSoft,
                  size: 44,
                  iconSize: 22,
                ),
                const SizedBox(width: 14),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: <Widget>[
                      Text(
                        '$onlineCount',
                        style:
                            Theme.of(context).textTheme.headlineSmall?.copyWith(
                                  fontWeight: FontWeight.w800,
                                ),
                      ),
                      Text(
                        '在线设备 · 共 $totalPeers 台',
                        style: Theme.of(context).textTheme.bodySmall,
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ),
        const SizedBox(width: NeuSpace.md),
        Expanded(
          child: NeuCard(
            padding: const EdgeInsets.all(18),
            onTap: widget.onOpenSnapshots,
            child: Row(
              children: <Widget>[
                  NeuIconBadge(
                  icon: Icons.cloud_done_rounded,
                    color: neu.chartIndigo,
                    backgroundColor: neu.chartIndigoSoft,
                  size: 44,
                  iconSize: 22,
                ),
                const SizedBox(width: 14),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: <Widget>[
                      Text(
                        SystemEnvHelper.formatBytes(
                          overview?.vaultUsedBytes ?? 0,
                        ),
                        style:
                            Theme.of(context).textTheme.headlineSmall?.copyWith(
                                  fontWeight: FontWeight.w800,
                                ),
                      ),
                      Text(
                        '已保护 · ${overview?.syncRuleCount ?? 0} 个备份任务',
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

  Widget _buildMobileStatusSummary(
    BuildContext context, {
    required SystemOverviewModel? overview,
    required int onlineCount,
    required int totalPeers,
  }) {
      final neu = NeuTheme.of(context);
    final running = overview?.serverRunning ?? false;
    final portText = overview == null
        ? (_loadFailed ? '状态读取失败' : '正在读取状态…')
        : (overview.serverPort != null
            ? '端口 :${overview.serverPort}'
            : '点击进入设置启动');

    return Column(
      children: <Widget>[
        NeuCard(
          padding: const EdgeInsets.all(16),
            backgroundColor: running ? neu.healthySoft : neu.surface,
          borderColor: running
                ? neu.healthy.withValues(alpha: 0.25)
                : neu.hairlineBorder,
          onTap: widget.onOpenSettings,
          child: Row(
            children: <Widget>[
              NeuIconBadge(
                icon: running
                    ? Icons.verified_user_rounded
                    : Icons.pause_circle_outline_rounded,
                  color: running ? neu.healthy : neu.warning,
                backgroundColor: running
                      ? neu.healthy.withValues(alpha: 0.14)
                      : neu.folderGoldSoft,
                size: 42,
                iconSize: 21,
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: <Widget>[
                    Text(
                      running ? '系统正常' : '本地服务未启动',
                      style: Theme.of(context).textTheme.titleMedium?.copyWith(
                            fontWeight: FontWeight.w700,
                          ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      running ? '所有服务运行正常 · $portText' : portText,
                      style: Theme.of(context).textTheme.bodySmall,
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
        const SizedBox(height: NeuSpace.sm + 4),
        Row(
          children: <Widget>[
            Expanded(
              child: NeuCard(
                padding: const EdgeInsets.all(14),
                onTap: widget.onOpenPairing,
                child: Row(
                  children: <Widget>[
                      NeuIconBadge(
                      icon: Icons.devices_rounded,
                        color: neu.accent,
                        backgroundColor: neu.accentSoft,
                      size: 36,
                      iconSize: 18,
                    ),
                    const SizedBox(width: 10),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: <Widget>[
                          Text(
                            '$onlineCount / $totalPeers',
                            style: Theme.of(context)
                                .textTheme
                                .titleMedium
                                ?.copyWith(fontWeight: FontWeight.w800),
                          ),
                          Text(
                            '在线设备',
                            style: Theme.of(context).textTheme.bodySmall,
                          ),
                        ],
                      ),
                    ),
                  ],
                ),
              ),
            ),
            const SizedBox(width: NeuSpace.sm + 4),
            Expanded(
              child: NeuCard(
                padding: const EdgeInsets.all(14),
                onTap: widget.onOpenSnapshots,
                child: Row(
                  children: <Widget>[
                      NeuIconBadge(
                      icon: Icons.cloud_sync_rounded,
                        color: neu.chartIndigo,
                        backgroundColor: neu.chartIndigoSoft,
                      size: 36,
                      iconSize: 18,
                    ),
                    const SizedBox(width: 10),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: <Widget>[
                          Text(
                            '${overview?.syncRuleCount ?? 0}',
                            style: Theme.of(context)
                                .textTheme
                                .titleMedium
                                ?.copyWith(fontWeight: FontWeight.w800),
                          ),
                          Text(
                            '备份任务',
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
        ),
      ],
    );
  }

  Widget _buildActiveSyncBanner(BuildContext context, SyncProgressEvent sync) {
    final neu = NeuTheme.of(context);
    final fileName = sync.filePath.isNotEmpty
        ? SystemEnvHelper.folderDisplayName(sync.filePath)
        : sync.ruleId;
    final speed = sync.throughputBps > 0
        ? '${SystemEnvHelper.formatBytes(sync.throughputBps)}/s'
        : '同步中';
    final ratio = sync.totalBytes > 0
        ? (sync.bytesProcessed / sync.totalBytes).clamp(0.0, 1.0)
        : 0.0;

    return NeuCard(
      padding: const EdgeInsets.all(14),
      backgroundColor: neu.accentSoft,
      borderColor: neu.accent.withValues(alpha: 0.25),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Row(
            children: <Widget>[
              Icon(
                Icons.sync_rounded,
                size: 16,
                color: neu.accent,
              ),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  '正在同步：$fileName',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: Theme.of(context).textTheme.labelLarge?.copyWith(
                        color: neu.accent,
                        fontWeight: FontWeight.w600,
                      ),
                ),
              ),
              const SizedBox(width: 8),
              NeuStatusBadge(label: speed, tone: NeuStatusTone.accent),
            ],
          ),
          const SizedBox(height: 10),
          NeuProgressBar(value: ratio, height: 6, showPercentage: true),
        ],
      ),
    );
  }

  Widget _buildStorageCard(
    BuildContext context,
    SystemOverviewModel? overview,
  ) {
    final neu = NeuTheme.of(context);
    final used = overview?.vaultUsedBytes ?? 0;
    final quota = overview?.vaultQuotaBytes ?? 0;
    final ratio = quota > 0 ? (used / quota).clamp(0.0, 1.0) : 0.0;

    return NeuCard(
      padding: const EdgeInsets.all(20),
      onTap: widget.onOpenVault,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: <Widget>[
              Text('存储使用', style: Theme.of(context).textTheme.titleMedium),
              Text(
                '${SystemEnvHelper.formatBytes(used)} / ${SystemEnvHelper.formatBytes(quota)}',
                style: Theme.of(context).textTheme.labelMedium?.copyWith(
                      color: neu.textSecondary,
                    ),
              ),
            ],
          ),
          const SizedBox(height: 18),
          Row(
            children: <Widget>[
              StorageDonutRing(
                ratio: ratio,
                size: 108,
                strokeWidth: 11,
              ),
              const SizedBox(width: 20),
              Expanded(
                child: _vaults.isEmpty
                    ? Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: <Widget>[
                          Text(
                            _loadFailed ? '存储池读取失败。' : '暂无存储池。',
                            style: Theme.of(context).textTheme.bodySmall,
                          ),
                          const SizedBox(height: 8),
                          NeuButton(
                            label: _loadFailed ? '重新加载' : '新建存储池',
                            tone: NeuButtonTone.neutral,
                            onPressed: _loadFailed
                                ? () => _refreshAll(silent: false)
                                : widget.onOpenVault,
                          ),
                        ],
                      )
                    : Column(
                        children: <Widget>[
                          for (var i = 0; i < _vaults.length; i++) ...<Widget>[
                            Row(
                              children: <Widget>[
                                Container(
                                  width: 8,
                                  height: 8,
                                  decoration: BoxDecoration(
                                    shape: BoxShape.circle,
                                    color: i == 0
                                        ? neu.accent
                                        : neu.chartSky,
                                  ),
                                ),
                                const SizedBox(width: 8),
                                Expanded(
                                  child: Text(
                                    _vaults[i].name,
                                    maxLines: 1,
                                    overflow: TextOverflow.ellipsis,
                                    style:
                                        Theme.of(context).textTheme.bodyMedium,
                                  ),
                                ),
                                Text(
                                  SystemEnvHelper.formatBytes(
                                    _vaults[i].usedBytes,
                                  ),
                                  style:
                                      Theme.of(context).textTheme.labelMedium,
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
        ],
      ),
    );
  }

  Widget _buildDataHealthRow(
    BuildContext context,
    SystemOverviewModel? overview,
  ) {
    final neu = NeuTheme.of(context);
    final corrupted = overview?.corruptedChunks ?? 0;
    final healthy = corrupted == 0;

    return NeuCard(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
      onTap: widget.onOpenSnapshots,
      child: Row(
        children: <Widget>[
          NeuIconBadge(
            icon: healthy
                ? Icons.health_and_safety_rounded
                : Icons.warning_amber_rounded,
            color: healthy ? neu.healthy : neu.warning,
            backgroundColor:
                healthy ? neu.healthySoft : neu.folderGoldSoft,
            size: 38,
            iconSize: 20,
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: <Widget>[
                Text(
                  '数据健康与历史版本',
                  style: Theme.of(context).textTheme.titleSmall,
                ),
                const SizedBox(height: 2),
                Text(
                  healthy
                      ? '最近检查正常 · 点击查看历史版本'
                      : '检测到 $corrupted 个异常块 · 点击处理',
                  style: Theme.of(context).textTheme.bodySmall,
                ),
              ],
            ),
          ),
          Icon(
            Icons.chevron_right_rounded,
            color: neu.textSecondary,
          ),
        ],
      ),
    );
  }

  Widget _buildRecentActivityCard(
    BuildContext context,
    SystemOverviewModel? overview,
  ) {
    final neu = NeuTheme.of(context);
    final activities = overview?.recentActivities ?? const <RecentActivityModel>[];

    return NeuCard(
      padding: const EdgeInsets.all(20),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: <Widget>[
              Text('最近活动', style: Theme.of(context).textTheme.titleMedium),
              Text(
                '${activities.length} 条记录',
                style: Theme.of(context).textTheme.labelMedium?.copyWith(
                      color: neu.textSecondary,
                    ),
              ),
            ],
          ),
          const SizedBox(height: 14),
          if (activities.isEmpty)
            NeuEmptyState(
              icon: Icons.bolt_rounded,
              title: '暂无最近活动',
              subtitle: '开始备份、配对设备或校验数据后将在此显示。',
              action: NeuButton(
                label: '去配对设备',
                tone: NeuButtonTone.neutral,
                onPressed: widget.onOpenPairing,
              ),
            )
          else
            for (var i = 0; i < activities.length; i++) ...<Widget>[
              Row(
                children: <Widget>[
                  NeuIconBadge(
                    icon: _iconForActivity(activities[i].kind),
                    color: neu.accent,
                    backgroundColor: neu.accentSoft,
                    size: 36,
                    iconSize: 18,
                  ),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: <Widget>[
                        Text(
                          activities[i].title,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: Theme.of(context).textTheme.titleSmall,
                        ),
                        if (activities[i].detail.isNotEmpty) ...<Widget>[
                          const SizedBox(height: 2),
                          Text(
                            activities[i].detail,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: Theme.of(context).textTheme.bodySmall,
                          ),
                        ],
                      ],
                    ),
                  ),
                  const SizedBox(width: 8),
                  Text(
                    LanVaultBridge.formatRelativeTime(activities[i].timestamp),
                    style: Theme.of(context).textTheme.labelSmall?.copyWith(
                          color: neu.textSecondary,
                        ),
                  ),
                ],
              ),
              if (i < activities.length - 1)
                const Padding(
                  padding: EdgeInsets.symmetric(vertical: 10),
                  child: Divider(height: 1),
                ),
            ],
        ],
      ),
    );
  }

  IconData _iconForActivity(String kind) {
    final lower = kind.toLowerCase();
    if (lower.contains('pair') || lower.contains('device')) {
      return Icons.devices_rounded;
    }
    if (lower.contains('scrub') || lower.contains('health')) {
      return Icons.verified_user_outlined;
    }
    if (lower.contains('backup') || lower.contains('sync')) {
      return Icons.cloud_done_outlined;
    }
    return Icons.history_rounded;
  }
}
