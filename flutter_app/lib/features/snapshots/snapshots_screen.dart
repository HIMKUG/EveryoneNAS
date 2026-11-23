import 'package:material_ui/material_ui.dart';

import '../../bridge/lanvault_bridge.dart';
import '../../bridge/system_env_helper.dart';
import '../../theme/neu_theme.dart';
import '../../theme/neu_toast.dart';

enum _SnapshotTab { timeline, scrub, restoreHistory }

class _RestoreRecord {
  const _RestoreRecord({
    required this.snapshotId,
    required this.vaultId,
    required this.fileCount,
    required this.totalBytes,
    required this.outputPath,
    required this.triggeredAt,
  });

  final String snapshotId;
  final String vaultId;
  final int fileCount;
  final int totalBytes;
  final String outputPath;
  final DateTime triggeredAt;
}

/// 5. 快照页：
/// - 顶部 Segmented 标签切换：版本时间线 / 修复校验 / 恢复记录
/// - 桌面端横向时间线节点轴 + 移动端纵向时间线节点轴，绑定 list_snapshots 真实数据
/// - 下方可恢复快照列表（文件夹图标、文件数/大小、保留层级、浏览详情弹窗、恢复按钮）
/// - 修复校验页内置一键触发 trigger_bit_rot_scrub 巡检并展示已校验/损坏/自愈块数
class SnapshotsScreen extends StatefulWidget {
  const SnapshotsScreen({super.key, required this.bridge});

  final LanVaultBridge bridge;

  @override
  State<SnapshotsScreen> createState() => _SnapshotsScreenState();
}

class _SnapshotsScreenState extends State<SnapshotsScreen> {
  static const String _allVaultsSentinel = '__all_vaults__';

  _SnapshotTab _activeTab = _SnapshotTab.timeline;
  List<VaultSummaryModel> _vaults = const <VaultSummaryModel>[];
  List<SnapshotSummaryModel> _snapshots = const <SnapshotSummaryModel>[];
  final List<_RestoreRecord> _restoreHistory = <_RestoreRecord>[];

  String _selectedVaultId = _allVaultsSentinel;
  String? _selectedSnapshotId;
  BitRotScrubResultModel? _lastScrubResult;

  bool _loading = true;
  bool _scrubbing = false;
  bool _restoring = false;

  @override
  void initState() {
    super.initState();
    _loadData();
  }

  Future<void> _loadData() async {
    setState(() => _loading = true);
    try {
      final vaultFilter =
          _selectedVaultId == _allVaultsSentinel ? null : _selectedVaultId;
      final results = await Future.wait<Object>(<Future<Object>>[
        widget.bridge.listVaults(),
        widget.bridge.listSnapshots(vaultId: vaultFilter),
      ]);
      if (!mounted) return;
      final vaults = results[0] as List<VaultSummaryModel>;
      final snapshots = results[1] as List<SnapshotSummaryModel>;
      setState(() {
        _vaults = vaults;
        _snapshots = snapshots;
        if (snapshots.isNotEmpty &&
            (_selectedSnapshotId == null ||
                !snapshots.any((s) => s.snapshotId == _selectedSnapshotId))) {
          _selectedSnapshotId = snapshots.first.snapshotId;
        } else if (snapshots.isEmpty) {
          _selectedSnapshotId = null;
        }
        _loading = false;
      });
    } catch (err) {
      if (!mounted) return;
      setState(() => _loading = false);
      NeuToast.error(context, '加载快照列表失败：$err');
    }
  }

  Future<void> _runBitRotScrub() async {
    setState(() => _scrubbing = true);
    try {
      final result = await widget.bridge.triggerBitRotScrub();
      if (!mounted) return;
      setState(() {
        _lastScrubResult = result;
        _scrubbing = false;
      });
      if (result.corruptedChunks == 0) {
        NeuToast.success(
          context,
          '校验完成：已检查 ${result.checkedChunks} 个数据块，未发现损坏',
        );
      } else {
        NeuToast.warning(
          context,
          '校验发现 ${result.corruptedChunks} 个损坏块，已自动修复 ${result.healedChunks} 个',
        );
      }
    } catch (err) {
      if (!mounted) return;
      setState(() => _scrubbing = false);
      NeuToast.error(context, '执行完整性校验失败：$err');
    }
  }

  void _openSnapshotDetail(SnapshotSummaryModel snapshot) {
    showDialog<void>(
      context: context,
      builder: (dialogContext) {
        final textTheme = Theme.of(dialogContext).textTheme;
        return Dialog(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 440),
            child: Padding(
              padding: const EdgeInsets.all(24),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: <Widget>[
                  Row(
                    mainAxisAlignment: MainAxisAlignment.spaceBetween,
                    children: <Widget>[
                      Text(
                        '快照详情',
                        style: textTheme.titleMedium?.copyWith(fontSize: 18),
                      ),
                      IconButton(
                        icon: const Icon(Icons.close_rounded, size: 20),
                        onPressed: () => Navigator.of(dialogContext).pop(),
                      ),
                    ],
                  ),
                  const SizedBox(height: 12),
                  _DetailPair(label: '快照编号', value: snapshot.snapshotId),
                  _DetailPair(label: '所属存储池', value: snapshot.vaultId),
                  _DetailPair(label: '来源设备', value: snapshot.deviceId),
                  _DetailPair(label: '保留层级', value: snapshot.retentionLabel),
                  _DetailPair(label: '文件数量', value: '${snapshot.fileCount} 个文件'),
                  _DetailPair(
                    label: '总容量',
                    value: SystemEnvHelper.formatBytes(snapshot.totalBytes),
                  ),
                  _DetailPair(label: '创建时间', value: snapshot.createdAt),
                  const SizedBox(height: 18),
                  Row(
                    mainAxisAlignment: MainAxisAlignment.end,
                    children: <Widget>[
                      NeuButton(
                        label: '关闭',
                        onPressed: () => Navigator.of(dialogContext).pop(),
                      ),
                      const SizedBox(width: NeuSpace.sm),
                      NeuButton(
                        label: '恢复此快照',
                        icon: Icons.restore_rounded,
                        tone: NeuButtonTone.primary,
                        onPressed: () {
                          Navigator.of(dialogContext).pop();
                          _triggerRestore(snapshot);
                        },
                      ),
                    ],
                  ),
                ],
              ),
            ),
          ),
        );
      },
    );
  }

  Future<void> _triggerRestore(SnapshotSummaryModel snapshot) async {
    if (_restoring) return;
    try {
      final targetDir = await widget.bridge.pickDirectory(
        description: '选择快照恢复目标文件夹',
      );
      if (targetDir == null || !mounted) return;
      setState(() => _restoring = true);
      NeuToast.info(context, '正在解密并校验恢复快照 ${snapshot.snapshotId}…');
      final res = await widget.bridge.restoreSnapshot(
        snapshotId: snapshot.snapshotId,
        targetDir: targetDir,
      );
      if (!mounted) return;
      setState(() {
        _restoring = false;
        _restoreHistory.insert(
          0,
          _RestoreRecord(
            snapshotId: snapshot.snapshotId,
            vaultId: snapshot.vaultId,
            fileCount: res.restoredChunks,
            totalBytes: res.restoredBytes,
            outputPath: res.outputPath,
            triggeredAt: DateTime.now(),
          ),
        );
      });
      NeuToast.success(
        context,
        '快照已恢复（${res.restoredChunks} 块，${SystemEnvHelper.formatBytes(res.restoredBytes)}）-> ${res.outputPath}',
      );
    } catch (err) {
      if (!mounted) return;
      setState(() => _restoring = false);
      NeuToast.error(context, '恢复快照失败：$err');
    }
  }

  NeuStatusTone _tierTone(String tier) {
    return switch (tier.toLowerCase()) {
      'daily' => NeuStatusTone.accent,
      'weekly' => NeuStatusTone.healthy,
      'monthly' => NeuStatusTone.warning,
      _ => NeuStatusTone.neutral,
    };
  }

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    final isWide = MediaQuery.sizeOf(context).width >= 840;

    final vaultOptions = <NeuDropdownOption<String>>[
      const NeuDropdownOption<String>(
        value: _allVaultsSentinel,
        label: '全部存储池',
      ),
      for (final v in _vaults)
        NeuDropdownOption<String>(
          value: v.vaultId,
          label: v.name,
        ),
    ];

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
                      Text('快照', style: textTheme.headlineSmall),
                      const SizedBox(height: 4),
                      Text(
                        '基于 GFS 的历史版本与数据恢复',
                        style: textTheme.bodySmall,
                      ),
                    ],
                  ),
                ),
                SizedBox(
                  width: 210,
                  child: NeuDropdown<String>(
                    options: vaultOptions,
                    value: _selectedVaultId,
                    prefixIcon: Icons.layers_outlined,
                    onChanged: (val) {
                      if (val != null) {
                        setState(() => _selectedVaultId = val);
                        _loadData();
                      }
                    },
                  ),
                ),
                const SizedBox(width: NeuSpace.sm),
                NeuButton(
                  label: '刷新',
                  icon: Icons.refresh_rounded,
                  loading: _loading,
                  onPressed: _loadData,
                ),
              ],
            ),
            const SizedBox(height: 18),
          ] else ...<Widget>[
            Row(
              children: <Widget>[
                Expanded(
                  child: NeuDropdown<String>(
                    options: vaultOptions,
                    value: _selectedVaultId,
                    prefixIcon: Icons.layers_outlined,
                    onChanged: (val) {
                      if (val != null) {
                        setState(() => _selectedVaultId = val);
                        _loadData();
                      }
                    },
                  ),
                ),
                const SizedBox(width: NeuSpace.sm),
                NeuButton(
                  label: '刷新',
                  icon: Icons.refresh_rounded,
                  loading: _loading,
                  onPressed: _loadData,
                ),
              ],
            ),
            const SizedBox(height: 14),
          ],
          ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 420),
            child: NeuSegmentedControl<_SnapshotTab>(
              semanticLabel: '快照功能分类',
              value: _activeTab,
              onChanged: (tab) => setState(() => _activeTab = tab),
              segments: const <NeuSegment<_SnapshotTab>>[
                NeuSegment<_SnapshotTab>(
                  value: _SnapshotTab.timeline,
                  label: '版本时间线',
                ),
                NeuSegment<_SnapshotTab>(
                  value: _SnapshotTab.scrub,
                  label: '修复校验',
                ),
                NeuSegment<_SnapshotTab>(
                  value: _SnapshotTab.restoreHistory,
                  label: '恢复记录',
                ),
              ],
            ),
          ),
          const SizedBox(height: NeuSpace.md),
          NeuMotionSwitcher(
            child: KeyedSubtree(
              key: ValueKey<_SnapshotTab>(_activeTab),
              child: _activeTab == _SnapshotTab.timeline
                  ? _buildTimelineView(context, isWide)
                  : (_activeTab == _SnapshotTab.scrub
                      ? _buildScrubView(context)
                      : _buildRestoreHistoryView(context)),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildTimelineView(BuildContext context, bool isWide) {
    final textTheme = Theme.of(context).textTheme;

    if (_loading && _snapshots.isEmpty) {
      return NeuCard(
        padding: const EdgeInsets.all(24),
        child: Center(
          child: Text('正在加载历史快照…', style: textTheme.bodySmall),
        ),
      );
    }

    if (_snapshots.isEmpty) {
      return NeuCard(
        padding: const EdgeInsets.all(28),
        child: Column(
          children: <Widget>[
            Container(
              width: 52,
              height: 52,
              decoration: const BoxDecoration(
                color: NeuTokens.accentSoft,
                shape: BoxShape.circle,
              ),
              child: const Icon(
                Icons.history_rounded,
                size: 26,
                color: NeuTokens.accent,
              ),
            ),
            const SizedBox(height: 12),
            Text('暂无历史快照', style: textTheme.titleMedium),
            const SizedBox(height: 4),
            Text(
              '完成首次增量备份后，系统将按 GFS（7日 / 4周 / 12月）策略自动生成历史版本时间线。',
              textAlign: TextAlign.center,
              style: textTheme.bodySmall,
            ),
          ],
        ),
      );
    }

    final selectedSnap = _snapshots.firstWhere(
      (s) => s.snapshotId == _selectedSnapshotId,
      orElse: () => _snapshots.first,
    );

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: <Widget>[
        NeuCard(
          padding: const EdgeInsets.all(20),
          child: isWide
              ? _buildHorizontalTimeline(context, selectedSnap)
              : _buildVerticalTimeline(context, selectedSnap),
        ),
        const SizedBox(height: NeuSpace.md),
        NeuCard(
          padding: const EdgeInsets.all(20),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: <Widget>[
              Row(
                mainAxisAlignment: MainAxisAlignment.spaceBetween,
                children: <Widget>[
                  Text(
                    '可恢复的快照 (${_snapshots.length})',
                    style: textTheme.titleMedium,
                  ),
                  Text(
                    '已选：${selectedSnap.createdAt}',
                    style: textTheme.labelMedium,
                  ),
                ],
              ),
              const SizedBox(height: 14),
              for (var i = 0; i < _snapshots.length; i++) ...<Widget>[
                Builder(
                  builder: (context) {
                    final snap = _snapshots[i];
                    final isSelected = snap.snapshotId == selectedSnap.snapshotId;
                    return InkWell(
                      onTap: () =>
                          setState(() => _selectedSnapshotId = snap.snapshotId),
                      borderRadius: const BorderRadius.all(Radius.circular(12)),
                      child: Container(
                        padding: const EdgeInsets.symmetric(
                          horizontal: 12,
                          vertical: 10,
                        ),
                        decoration: BoxDecoration(
                          color: isSelected
                              ? NeuTokens.accentSoft
                              : Colors.transparent,
                          borderRadius:
                              const BorderRadius.all(Radius.circular(12)),
                        ),
                        child: Row(
                          children: <Widget>[
                            Container(
                              width: 40,
                              height: 40,
                              decoration: const BoxDecoration(
                                color: NeuTokens.folderGoldSoft,
                                borderRadius:
                                    BorderRadius.all(Radius.circular(10)),
                              ),
                              child: const Icon(
                                Icons.folder_rounded,
                                size: 21,
                                color: NeuTokens.folderGold,
                              ),
                            ),
                            const SizedBox(width: 12),
                            Expanded(
                              child: Column(
                                crossAxisAlignment: CrossAxisAlignment.start,
                                children: <Widget>[
                                  Row(
                                    children: <Widget>[
                                      Flexible(
                                        child: Text(
                                          snap.snapshotId,
                                          overflow: TextOverflow.ellipsis,
                                          style: textTheme.titleSmall,
                                        ),
                                      ),
                                      const SizedBox(width: 8),
                                      NeuStatusBadge(
                                        label: snap.retentionLabel,
                                        tone: _tierTone(snap.retentionTier),
                                        showDot: false,
                                      ),
                                    ],
                                  ),
                                  const SizedBox(height: 3),
                                  Text(
                                    '${snap.fileCount} 个文件 · ${SystemEnvHelper.formatBytes(snap.totalBytes)} · ${snap.createdAt}',
                                    maxLines: 1,
                                    overflow: TextOverflow.ellipsis,
                                    style: textTheme.bodySmall,
                                  ),
                                ],
                              ),
                            ),
                            const SizedBox(width: 8),
                            NeuButton(
                              label: '浏览',
                              onPressed: () => _openSnapshotDetail(snap),
                            ),
                            const SizedBox(width: 8),
                            NeuButton(
                              label: '恢复',
                              tone: NeuButtonTone.primary,
                              onPressed: () => _triggerRestore(snap),
                            ),
                          ],
                        ),
                      ),
                    );
                  },
                ),
                if (i < _snapshots.length - 1) const Divider(height: 1),
              ],
            ],
          ),
        ),
      ],
    );
  }

  Widget _buildHorizontalTimeline(
    BuildContext context,
    SnapshotSummaryModel selectedSnap,
  ) {
    final textTheme = Theme.of(context).textTheme;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.center,
      children: <Widget>[
        Text(
          selectedSnap.createdAt,
          style: textTheme.titleSmall?.copyWith(color: NeuTokens.accent),
        ),
        const SizedBox(height: 4),
        Text(
          '${selectedSnap.retentionLabel} · ${selectedSnap.fileCount} 个文件 · ${SystemEnvHelper.formatBytes(selectedSnap.totalBytes)}',
          style: textTheme.bodySmall,
        ),
        const SizedBox(height: 18),
        SingleChildScrollView(
          scrollDirection: Axis.horizontal,
          child: Row(
            children: <Widget>[
              for (var i = 0; i < _snapshots.length; i++) ...<Widget>[
                Builder(
                  builder: (context) {
                    final snap = _snapshots[i];
                    final isSelected =
                        snap.snapshotId == selectedSnap.snapshotId;
                    return GestureDetector(
                      onTap: () =>
                          setState(() => _selectedSnapshotId = snap.snapshotId),
                      child: MouseRegion(
                        cursor: SystemMouseCursors.click,
                        child: Column(
                          children: <Widget>[
                            AnimatedContainer(
                              duration: NeuMotion.resolveState(context),
                              curve: NeuMotion.resolveStateCurve(context),
                              width: isSelected ? 34 : 16,
                              height: isSelected ? 34 : 16,
                              decoration: BoxDecoration(
                                color: isSelected
                                    ? NeuTokens.accent
                                    : NeuTokens.timelineInactive,
                                shape: BoxShape.circle,
                                border: isSelected
                                    ? Border.all(
                                        color: NeuTokens.timelineHalo,
                                        width: 5,
                                      )
                                    : null,
                                boxShadow: isSelected
                                    ? NeuTokens.primaryButtonShadows
                                    : null,
                              ),
                              child: isSelected
                                  ? const Center(
                                      child: SizedBox(
                                        width: 8,
                                        height: 8,
                                        child: DecoratedBox(
                                          decoration: BoxDecoration(
                                            color: Colors.white,
                                            shape: BoxShape.circle,
                                          ),
                                        ),
                                      ),
                                    )
                                  : null,
                            ),
                            const SizedBox(height: 8),
                            SizedBox(
                              width: 92,
                              child: Text(
                                snap.createdAt,
                                textAlign: TextAlign.center,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: textTheme.labelMedium?.copyWith(
                                  fontSize: 11,
                                  color: isSelected
                                      ? NeuTokens.accent
                                      : NeuTokens.textSecondary,
                                ),
                              ),
                            ),
                          ],
                        ),
                      ),
                    );
                  },
                ),
                if (i < _snapshots.length - 1)
                  Container(
                    width: 44,
                    height: 2,
                    margin: const EdgeInsets.only(bottom: 22),
                    color: NeuTokens.timelineLine,
                  ),
              ],
            ],
          ),
        ),
      ],
    );
  }

  Widget _buildVerticalTimeline(
    BuildContext context,
    SnapshotSummaryModel selectedSnap,
  ) {
    final textTheme = Theme.of(context).textTheme;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: <Widget>[
        for (var i = 0; i < _snapshots.length; i++) ...<Widget>[
          Builder(
            builder: (context) {
              final snap = _snapshots[i];
              final isSelected = snap.snapshotId == selectedSnap.snapshotId;
              return GestureDetector(
                behavior: HitTestBehavior.opaque,
                onTap: () =>
                    setState(() => _selectedSnapshotId = snap.snapshotId),
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: <Widget>[
                    Column(
                      children: <Widget>[
                        Container(
                          width: isSelected ? 22 : 12,
                          height: isSelected ? 22 : 12,
                          margin: EdgeInsets.symmetric(
                            horizontal: isSelected ? 0 : 5,
                            vertical: 2,
                          ),
                          decoration: BoxDecoration(
                            color: isSelected
                                ? NeuTokens.accent
                                : NeuTokens.timelineInactive,
                            shape: BoxShape.circle,
                            border: isSelected
                                ? Border.all(
                                    color: NeuTokens.timelineHalo,
                                    width: 4,
                                  )
                                : null,
                          ),
                        ),
                        if (i < _snapshots.length - 1)
                          Container(
                            width: 2,
                            height: 36,
                            color: NeuTokens.timelineLine,
                          ),
                      ],
                    ),
                    const SizedBox(width: 12),
                    Expanded(
                      child: Padding(
                        padding: const EdgeInsets.only(bottom: 12),
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: <Widget>[
                            Text(
                              snap.createdAt,
                              style: textTheme.titleSmall?.copyWith(
                                color: isSelected
                                    ? NeuTokens.accent
                                    : NeuTokens.textPrimary,
                              ),
                            ),
                            const SizedBox(height: 2),
                            Text(
                              '${snap.retentionLabel} · ${snap.fileCount} 个文件 · ${SystemEnvHelper.formatBytes(snap.totalBytes)}',
                              style: textTheme.bodySmall,
                            ),
                          ],
                        ),
                      ),
                    ),
                  ],
                ),
              );
            },
          ),
        ],
        const SizedBox(height: 8),
        NeuButton(
          label: '预览并恢复所选版本',
          icon: Icons.restore_rounded,
          tone: NeuButtonTone.primary,
          expanded: true,
          onPressed: () => _openSnapshotDetail(selectedSnap),
        ),
      ],
    );
  }

  Widget _buildScrubView(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    final scrub = _lastScrubResult;

    return NeuCard(
      padding: const EdgeInsets.all(22),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Row(
            children: <Widget>[
              Container(
                width: 44,
                height: 44,
                decoration: const BoxDecoration(
                  color: NeuTokens.healthySoft,
                  borderRadius: BorderRadius.all(Radius.circular(12)),
                ),
                child: const Icon(
                  Icons.health_and_safety_rounded,
                  size: 24,
                  color: NeuTokens.healthy,
                ),
              ),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: <Widget>[
                    Text('Bit-Rot 数据完整性校验与自愈', style: textTheme.titleMedium),
                    const SizedBox(height: 2),
                    Text(
                      '逐块重算 BLAKE3 校验和，自动识别静默位翻转并利用副本自愈修复。',
                      style: textTheme.bodySmall,
                    ),
                  ],
                ),
              ),
            ],
          ),
          const SizedBox(height: 18),
          NeuButton(
            label: _scrubbing ? '正在逐块校验…' : '立即触发 Bit-Rot 巡检',
            icon: Icons.fact_check_outlined,
            tone: NeuButtonTone.primary,
            loading: _scrubbing,
            onPressed: _runBitRotScrub,
          ),
          const SizedBox(height: 18),
          if (scrub == null)
            Container(
              width: double.infinity,
              padding: const EdgeInsets.all(16),
              decoration: BoxDecoration(
                color: NeuTokens.insetSurface,
                borderRadius: const BorderRadius.all(Radius.circular(12)),
              ),
              child: Text(
                '尚未在当前会话触发巡检。点击上方按钮立即对本地存储池数据块执行完整性校验。',
                style: textTheme.bodySmall,
              ),
            )
          else
            Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: <Widget>[
                Row(
                  children: <Widget>[
                    Expanded(
                      child: _ScrubMetricTile(
                        label: '已校验块数',
                        value: '${scrub.checkedChunks}',
                        color: NeuTokens.accent,
                      ),
                    ),
                    const SizedBox(width: 10),
                    Expanded(
                      child: _ScrubMetricTile(
                        label: '损坏块数',
                        value: '${scrub.corruptedChunks}',
                        color: scrub.corruptedChunks > 0
                            ? NeuTokens.danger
                            : NeuTokens.healthy,
                      ),
                    ),
                    const SizedBox(width: 10),
                    Expanded(
                      child: _ScrubMetricTile(
                        label: '已自愈块数',
                        value: '${scrub.healedChunks}',
                        color: NeuTokens.healthy,
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 12),
                Text(
                  '巡检任务：${scrub.scrubId} · 状态：${scrub.status}',
                  style: textTheme.bodySmall,
                ),
              ],
            ),
        ],
      ),
    );
  }

  Widget _buildRestoreHistoryView(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    if (_restoreHistory.isEmpty) {
      return NeuCard(
        padding: const EdgeInsets.all(28),
        child: Center(
          child: Text(
            '暂无恢复记录。在“版本时间线”中选择历史快照并点击“恢复”后将在此展示。',
            textAlign: TextAlign.center,
            style: textTheme.bodySmall,
          ),
        ),
      );
    }

    return NeuCard(
      padding: const EdgeInsets.all(18),
      child: Column(
        children: <Widget>[
          for (var i = 0; i < _restoreHistory.length; i++) ...<Widget>[
            Builder(
              builder: (context) {
                final rec = _restoreHistory[i];
                final ts =
                    '${rec.triggeredAt.hour.toString().padLeft(2, '0')}:${rec.triggeredAt.minute.toString().padLeft(2, '0')}:${rec.triggeredAt.second.toString().padLeft(2, '0')}';
                return Padding(
                  padding: const EdgeInsets.symmetric(vertical: 8),
                  child: Row(
                    children: <Widget>[
                      const Icon(
                        Icons.restore_rounded,
                        color: NeuTokens.accent,
                      ),
                      const SizedBox(width: 12),
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: <Widget>[
                            Text(rec.snapshotId, style: textTheme.titleSmall),
                            Text(
                              '存储池 ${rec.vaultId} · ${rec.fileCount} 个文件 · ${SystemEnvHelper.formatBytes(rec.totalBytes)}',
                              style: textTheme.bodySmall,
                            ),
                          ],
                        ),
                      ),
                      Text(ts, style: textTheme.labelMedium),
                    ],
                  ),
                );
              },
            ),
            if (i < _restoreHistory.length - 1) const Divider(height: 1),
          ],
        ],
      ),
    );
  }
}

class _ScrubMetricTile extends StatelessWidget {
  const _ScrubMetricTile({
    required this.label,
    required this.value,
    required this.color,
  });

  final String label;
  final String value;
  final Color color;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: NeuTokens.insetSurface,
        borderRadius: const BorderRadius.all(Radius.circular(12)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Text(
            value,
            style: Theme.of(context).textTheme.headlineSmall?.copyWith(
                  fontSize: 20,
                  color: color,
                  fontFeatures: const <FontFeature>[
                    FontFeature.tabularFigures(),
                  ],
                ),
          ),
          const SizedBox(height: 2),
          Text(label, style: Theme.of(context).textTheme.bodySmall),
        ],
      ),
    );
  }
}

class _DetailPair extends StatelessWidget {
  const _DetailPair({required this.label, required this.value});

  final String label;
  final String value;

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 6),
      child: Row(
        mainAxisAlignment: MainAxisAlignment.spaceBetween,
        children: <Widget>[
          Text(label, style: textTheme.bodySmall),
          const SizedBox(width: 12),
          Flexible(
            child: Text(
              value,
              textAlign: TextAlign.right,
              overflow: TextOverflow.ellipsis,
              style: textTheme.titleSmall,
            ),
          ),
        ],
      ),
    );
  }
}
