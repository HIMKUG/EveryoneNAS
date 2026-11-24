import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../../bridge/lanvault_bridge.dart';
import '../../bridge/system_env_helper.dart';
import '../../theme/neu_theme.dart';
import '../../theme/neu_toast.dart';

/// 历史版本与数据完整性检查（已嵌入备份页“历史版本”子视图）：
/// - 将 Bit-Rot 术语翻译为“数据完整性检查”，作为全 App 唯一手动检查入口。
/// - 桌面端宽屏下左右分栏呈现版本时间线与完整性检查。
class SnapshotsScreen extends StatefulWidget {
  const SnapshotsScreen({
    super.key,
    required this.bridge,
    this.embedded = false,
  });

  final LanVaultBridge bridge;
  final bool embedded;

  @override
  State<SnapshotsScreen> createState() => _SnapshotsScreenState();
}

class _SnapshotsScreenState extends State<SnapshotsScreen> {
  List<VaultSummaryModel> _vaults = const <VaultSummaryModel>[];
  List<SnapshotSummaryModel> _snapshots = const <SnapshotSummaryModel>[];
  String? _selectedVaultId;
  BitRotScrubResultModel? _lastScrub;
  bool _loading = true;
  bool _scrubbing = false;
  String? _restoringSnapshotId;

  @override
  void initState() {
    super.initState();
    unawaited(_refreshAll(silent: false));
  }

  Future<void> _refreshAll({required bool silent}) async {
    if (!silent && mounted) setState(() => _loading = true);
    try {
      final results = await Future.wait<Object>(<Future<Object>>[
        widget.bridge.listVaults(),
        widget.bridge.listSnapshots(vaultId: _selectedVaultId),
      ]);
      if (!mounted) return;
      setState(() {
        _vaults = results[0] as List<VaultSummaryModel>;
        _snapshots = results[1] as List<SnapshotSummaryModel>;
        _loading = false;
      });
    } catch (err) {
      if (!mounted) return;
      setState(() => _loading = false);
      if (!silent) {
        NeuToast.error(context, LanVaultBridge.formatUserError(err));
      }
    }
  }

  Future<void> _runIntegrityCheck() async {
    if (_scrubbing) return;
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
        res.corruptedChunks == 0
            ? '检查完成：${res.checkedChunks} 个数据块全部完好'
            : '发现 ${res.corruptedChunks} 个异常块，已自动修复 ${res.healedChunks} 个',
      );
    } catch (err) {
      if (!mounted) return;
      setState(() => _scrubbing = false);
      NeuToast.error(context, LanVaultBridge.formatUserError(err));
    }
  }

  Future<void> _restoreSnapshot(SnapshotSummaryModel snapshot) async {
    if (_restoringSnapshotId != null) return;
    final targetDir = await widget.bridge.pickDirectory(
      description: '选择恢复文件保存目录',
    );
    if (targetDir == null || targetDir.isEmpty || !mounted) return;

    setState(() => _restoringSnapshotId = snapshot.snapshotId);
    try {
      final res = await widget.bridge.restoreSnapshot(
        snapshotId: snapshot.snapshotId,
        targetDir: targetDir,
      );
      if (!mounted) return;
      NeuToast.success(
        context,
        '已恢复 ${SystemEnvHelper.formatBytes(res.restoredBytes)} 至 ${res.outputPath}',
      );
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, LanVaultBridge.formatUserError(err));
    } finally {
      if (mounted) setState(() => _restoringSnapshotId = null);
    }
  }

  @override
  Widget build(BuildContext context) {
    final isWide =
        MediaQuery.sizeOf(context).width >= NeuTokens.breakpointWide;

    final filterBar = Row(
      children: <Widget>[
        Expanded(
          child: NeuDropdown<String>(
            prefixIcon: Icons.layers_outlined,
            value: _selectedVaultId ?? '__all__',
            options: <NeuDropdownOption<String>>[
              const NeuDropdownOption<String>(
                value: '__all__',
                label: '全部存储池',
              ),
              ..._vaults.map(
                (v) => NeuDropdownOption<String>(
                  value: v.vaultId,
                  label: v.name,
                ),
              ),
            ],
            onChanged: (v) {
              setState(() {
                _selectedVaultId = (v == null || v == '__all__') ? null : v;
              });
              unawaited(_refreshAll(silent: false));
            },
          ),
        ),
        const SizedBox(width: 8),
        IconButton(
          tooltip: '刷新版本列表',
          icon: const Icon(Icons.refresh_rounded, size: 20),
          onPressed: () => _refreshAll(silent: false),
        ),
      ],
    );

    final timelineCard = _buildTimelineCard(context);
    final integrityCard = _buildIntegrityCard(context);

    return RefreshIndicator(
      onRefresh: () => _refreshAll(silent: false),
      child: SingleChildScrollView(
        physics: const AlwaysScrollableScrollPhysics(),
        padding: EdgeInsets.fromLTRB(
          isWide ? NeuSpace.lg : NeuSpace.md,
          NeuSpace.sm,
          isWide ? NeuSpace.lg : NeuSpace.md,
          NeuSpace.lg,
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: <Widget>[
            filterBar,
            const SizedBox(height: NeuSpace.md),
            if (isWide)
              Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: <Widget>[
                  Expanded(flex: 6, child: timelineCard),
                  const SizedBox(width: NeuSpace.md),
                  Expanded(flex: 4, child: integrityCard),
                ],
              )
            else ...<Widget>[
              timelineCard,
              const SizedBox(height: NeuSpace.md),
              integrityCard,
            ],
          ],
        ),
      ),
    );
  }

  Widget _buildTimelineCard(BuildContext context) {
    return NeuCard(
      padding: const EdgeInsets.all(18),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Text(
            '历史版本 (${_snapshots.length})',
            style: Theme.of(context).textTheme.titleMedium?.copyWith(
                  fontWeight: FontWeight.w700,
                ),
          ),
          const SizedBox(height: 14),
          if (_loading && _snapshots.isEmpty)
            const Padding(
              padding: EdgeInsets.symmetric(vertical: 28),
              child: Center(child: CircularProgressIndicator()),
            )
          else if (_snapshots.isEmpty)
            const NeuEmptyState(
              icon: Icons.history_rounded,
              title: '暂无历史版本',
              subtitle: '完成首次备份后，这里会自动按时间保留可恢复的历史版本。',
            )
          else
            for (var i = 0; i < _snapshots.length; i++) ...<Widget>[
              Row(
                children: <Widget>[
                  const NeuIconBadge(
                    icon: Icons.history_rounded,
                    color: NeuTokens.accent,
                    backgroundColor: NeuTokens.accentSoft,
                    size: 36,
                    iconSize: 18,
                  ),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: <Widget>[
                        Text(
                          LanVaultBridge.formatRelativeTime(
                            _snapshots[i].createdAt,
                          ),
                          style: Theme.of(context)
                              .textTheme
                              .titleSmall
                              ?.copyWith(fontWeight: FontWeight.w600),
                        ),
                        const SizedBox(height: 2),
                        Text(
                          '${_snapshots[i].fileCount} 个文件 · ${SystemEnvHelper.formatBytes(_snapshots[i].totalBytes)}',
                          style: Theme.of(context).textTheme.bodySmall,
                        ),
                      ],
                    ),
                  ),
                  NeuButton(
                    label: '恢复',
                    icon: Icons.restore_rounded,
                    loading: _restoringSnapshotId == _snapshots[i].snapshotId,
                    onPressed: () => unawaited(_restoreSnapshot(_snapshots[i])),
                  ),
                ],
              ),
              if (i < _snapshots.length - 1)
                const Padding(
                  padding: EdgeInsets.symmetric(vertical: 10),
                  child: Divider(height: 1),
                ),
            ],
        ],
      ),
    );
  }

  Widget _buildIntegrityCard(BuildContext context) {
    final scrub = _lastScrub;
    return NeuCard(
      padding: const EdgeInsets.all(18),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Row(
            children: <Widget>[
              const NeuIconBadge(
                icon: Icons.verified_user_outlined,
                color: NeuTokens.healthy,
                backgroundColor: NeuTokens.healthySoft,
                size: 38,
                iconSize: 20,
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: <Widget>[
                    Text(
                      '数据完整性检查',
                      style: Theme.of(context).textTheme.titleMedium?.copyWith(
                            fontWeight: FontWeight.w700,
                          ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      '校验本地备份块是否损坏并利用副本自动修复',
                      style: Theme.of(context).textTheme.bodySmall,
                    ),
                  ],
                ),
              ),
            ],
          ),
          const SizedBox(height: 14),
          NeuButton(
            label: _scrubbing ? '正在检查…' : '立即检查',
            icon: Icons.fact_check_outlined,
            tone: NeuButtonTone.primary,
            loading: _scrubbing,
            expanded: true,
            onPressed: _runIntegrityCheck,
          ),
          if (scrub != null) ...<Widget>[
            const SizedBox(height: 12),
            Container(
              width: double.infinity,
              padding: const EdgeInsets.all(12),
              decoration: const BoxDecoration(
                color: NeuTokens.insetSurface,
                borderRadius: BorderRadius.all(Radius.circular(12)),
              ),
              child: Text(
                '已检查 ${scrub.checkedChunks} 块 · 异常 ${scrub.corruptedChunks} 块 · 自动修复 ${scrub.healedChunks} 块',
                style: Theme.of(context).textTheme.bodySmall,
              ),
            ),
          ],
        ],
      ),
    );
  }
}
