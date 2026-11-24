import 'dart:async';

import 'package:material_ui/material_ui.dart';

import '../../bridge/lanvault_bridge.dart';
import '../../bridge/system_env_helper.dart';
import '../../theme/neu_theme.dart';
import '../../theme/neu_toast.dart';
import '../snapshots/snapshots_screen.dart';

/// 备份页（重构版）：
/// - 顶部轻量分段切换：备份任务 | 历史版本（收口原独立快照页）。
/// - 实时监控卡：常驻展示当前备份文件名、实时速度与中文阻塞提示，支持“增量备份 / 快速备份”双模切换。
/// - 删掉右下角孤立 + FAB，将“+ 新建”放到“备份任务”标题右侧、轻量刷新图标左侧。
/// - 删掉纯静态 FastCDC / GFS 宣传卡与底部的“主/备异盘存储目录配置”（已归入设置页）。
class BackupScreen extends StatefulWidget {
  const BackupScreen({
    super.key,
    required this.bridge,
    this.subTabNotifier,
  });

  final LanVaultBridge bridge;
  final ValueNotifier<int>? subTabNotifier;

  @override
  State<BackupScreen> createState() => _BackupScreenState();
}

class _BackupScreenState extends State<BackupScreen> {
  List<SyncRuleModel> _rules = const <SyncRuleModel>[];
  List<VaultSummaryModel> _vaults = const <VaultSummaryModel>[];
  SyncProgressEvent? _lastSync;
  StreamSubscription<SyncProgressEvent>? _syncSub;
  bool _loading = true;
  bool _syncPaused = false;
  bool _togglingPause = false;
  int _subTab = 0; // 0 = 备份任务, 1 = 历史版本

  @override
  void initState() {
    super.initState();
    if (widget.subTabNotifier != null) {
      _subTab = widget.subTabNotifier!.value;
      widget.subTabNotifier!.addListener(_onExternalSubTab);
    }
    _syncSub = widget.bridge.subscribeSyncProgress().listen((event) {
      if (!mounted) return;
      setState(() => _lastSync = event);
    });
    unawaited(_refreshAll(silent: false));
  }

  @override
  void dispose() {
    widget.subTabNotifier?.removeListener(_onExternalSubTab);
    _syncSub?.cancel();
    super.dispose();
  }

  void _onExternalSubTab() {
    if (!mounted || widget.subTabNotifier == null) return;
    final next = widget.subTabNotifier!.value.clamp(0, 1);
    if (next != _subTab) {
      setState(() => _subTab = next);
    }
  }

  Future<void> _refreshAll({required bool silent}) async {
    if (!silent && mounted) setState(() => _loading = true);
    try {
      final results = await Future.wait<Object>(<Future<Object>>[
        widget.bridge.listSyncRules(),
        widget.bridge.listVaults(),
        widget.bridge.getSyncPaused(),
      ]);
      if (!mounted) return;
      setState(() {
        _rules = results[0] as List<SyncRuleModel>;
        _vaults = results[1] as List<VaultSummaryModel>;
        _syncPaused = results[2] as bool;
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

  Future<void> _toggleGlobalPause() async {
    if (_togglingPause) return;
    setState(() => _togglingPause = true);
    try {
      final next = !_syncPaused;
      final applied = await widget.bridge.setSyncPaused(next);
      if (!mounted) return;
      setState(() => _syncPaused = applied);
      NeuToast.info(context, applied ? '已暂停全部自动备份' : '已恢复自动备份');
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, LanVaultBridge.formatUserError(err));
    } finally {
      if (mounted) setState(() => _togglingPause = false);
    }
  }

  Future<void> _triggerRuleScan(SyncRuleModel rule) async {
    try {
      await widget.bridge.triggerIncrementalScan(rule.ruleId);
      if (!mounted) return;
      NeuToast.info(
        context,
        '已开始备份 ${SystemEnvHelper.folderDisplayName(rule.localDir)}',
      );
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, LanVaultBridge.formatUserError(err));
    }
  }

  Future<void> _toggleRule(SyncRuleModel rule, bool enabled) async {
    try {
      await widget.bridge.toggleSyncRule(ruleId: rule.ruleId, enabled: enabled);
      if (!mounted) return;
      await _refreshAll(silent: true);
      if (!mounted) return;
      NeuToast.info(
        context,
        enabled
            ? '已开启 ${SystemEnvHelper.folderDisplayName(rule.localDir)}'
            : '已暂停 ${SystemEnvHelper.folderDisplayName(rule.localDir)}',
      );
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, LanVaultBridge.formatUserError(err));
    }
  }

  Future<void> _changeSchedule(SyncRuleModel rule, String mode) async {
    try {
      await widget.bridge.setSyncRuleSchedule(
        ruleId: rule.ruleId,
        scheduleMode: mode,
      );
      if (!mounted) return;
      await _refreshAll(silent: true);
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, LanVaultBridge.formatUserError(err));
    }
  }

  Future<void> _deleteRule(SyncRuleModel rule) async {
    try {
      await widget.bridge.deleteSyncRule(rule.ruleId);
      if (!mounted) return;
      NeuToast.success(context, '已删除备份任务');
      await _refreshAll(silent: true);
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, LanVaultBridge.formatUserError(err));
    }
  }

  Future<void> _openCreateTaskDialog() async {
    final dirController = TextEditingController();
    var targetVaultId = _vaults.isNotEmpty ? _vaults.first.vaultId : 'default';
    var submitting = false;

    await showNeuDialog<void>(
      context: context,
      builder: (dialogCtx) {
        return StatefulBuilder(
          builder: (context, setDialogState) {
            return NeuDialog(
              title: '新建备份任务',
              subtitle: '选择需要自动保护的本地文件夹',
              maxWidth: 440,
              actions: <Widget>[
                NeuButton(
                  label: '取消',
                  onPressed: submitting
                      ? null
                      : () => Navigator.of(dialogCtx).pop(),
                ),
                NeuButton(
                  label: '创建任务',
                  icon: Icons.add_rounded,
                  tone: NeuButtonTone.primary,
                  loading: submitting,
                  onPressed: submitting
                      ? null
                      : () async {
                          final localDir = dirController.text.trim();
                          if (localDir.isEmpty) {
                            NeuToast.warning(context, '请先选择要备份的文件夹');
                            return;
                          }
                          setDialogState(() => submitting = true);
                          try {
                            final ruleId =
                                'rule-${DateTime.now().millisecondsSinceEpoch}';
                            await widget.bridge.upsertSyncRule(
                              ruleId: ruleId,
                              localDir: localDir,
                              targetVaultId: targetVaultId,
                              enabled: true,
                            );
                            if (dialogCtx.mounted) {
                              Navigator.of(dialogCtx).pop();
                            }
                            if (mounted) {
                              NeuToast.success(this.context, '已新建备份任务');
                              await _refreshAll(silent: true);
                            }
                          } catch (err) {
                            if (dialogCtx.mounted) setDialogState(() => submitting = false);
                            if (mounted) {
                              NeuToast.error(
                                this.context,
                                LanVaultBridge.formatUserError(err),
                              );
                            }
                          }
                        },
                ),
              ],
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: <Widget>[
                  Row(
                    crossAxisAlignment: CrossAxisAlignment.end,
                    children: <Widget>[
                      Expanded(
                        child: NeuInsetField(
                          controller: dirController,
                          label: '本地文件夹路径',
                          hint: '点击右侧选择文件夹',
                          prefixIcon: Icons.folder_outlined,
                        ),
                      ),
                      const SizedBox(width: 8),
                      NeuButton(
                        label: '选择',
                        onPressed: () async {
                          final picked = await widget.bridge.pickDirectory(
                            description: '选择要备份的本地目录',
                          );
                          if (dialogCtx.mounted && picked != null && picked.isNotEmpty) {
                            dirController.text = picked;
                          }
                        },
                      ),
                    ],
                  ),
                  if (_vaults.isNotEmpty) ...<Widget>[
                    const SizedBox(height: 12),
                    NeuDropdown<String>(
                      label: '目标存储池',
                      prefixIcon: Icons.layers_outlined,
                      value: targetVaultId,
                      options: _vaults
                          .map(
                            (v) => NeuDropdownOption<String>(
                              value: v.vaultId,
                              label: v.name,
                            ),
                          )
                          .toList(growable: false),
                      onChanged: (v) {
                        if (v != null) {
                          setDialogState(() => targetVaultId = v);
                        }
                      },
                    ),
                  ],
                ],
              ),
            );
          },
        );
      },
    );

    unawaited(Future<void>.delayed(const Duration(milliseconds: 350), dirController.dispose));
  }

  @override
  Widget build(BuildContext context) {
    final isWide =
        MediaQuery.sizeOf(context).width >= NeuTokens.breakpointWide;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: <Widget>[
        Padding(
          padding: EdgeInsets.fromLTRB(
            isWide ? NeuSpace.lg : NeuSpace.md,
            isWide ? NeuSpace.lg : NeuSpace.sm,
            isWide ? NeuSpace.lg : NeuSpace.md,
            NeuSpace.sm,
          ),
          child: Row(
            children: <Widget>[
              if (isWide) ...<Widget>[
                Text(
                  '备份',
                  style: Theme.of(context)
                      .textTheme
                      .headlineSmall
                      ?.copyWith(fontWeight: FontWeight.w800),
                ),
                const SizedBox(width: 20),
              ],
              isWide
                  ? SizedBox(
                      width: 240,
                      child: NeuSegmentedControl<int>(
                        semanticLabel: '备份视角切换',
                        value: _subTab,
                        onChanged: (v) {
                          setState(() => _subTab = v);
                          widget.subTabNotifier?.value = v;
                        },
                        segments: const <NeuSegment<int>>[
                          NeuSegment<int>(value: 0, label: '备份任务'),
                          NeuSegment<int>(value: 1, label: '历史版本'),
                        ],
                      ),
                    )
                  : Expanded(
                      child: NeuSegmentedControl<int>(
                        semanticLabel: '备份视角切换',
                        value: _subTab,
                        onChanged: (v) {
                          setState(() => _subTab = v);
                          widget.subTabNotifier?.value = v;
                        },
                        segments: const <NeuSegment<int>>[
                          NeuSegment<int>(value: 0, label: '备份任务'),
                          NeuSegment<int>(value: 1, label: '历史版本'),
                        ],
                      ),
                    ),
              if (isWide) const Spacer(),
            ],
          ),
        ),
        Expanded(
          child: _subTab == 1
              ? SnapshotsScreen(bridge: widget.bridge, embedded: true)
              : _buildTasksTab(context, isWide),
        ),
      ],
    );
  }

  Widget _buildTasksTab(BuildContext context, bool isWide) {
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
            _buildStatusAndModeCard(context, isWide),
            const SizedBox(height: NeuSpace.md),
            _buildTaskListCard(context, isWide),
          ],
        ),
      ),
    );
  }

  Widget _buildStatusAndModeCard(BuildContext context, bool isWide) {
    final sync = _lastSync;
    final st = sync?.status.toLowerCase() ?? '';
    final isActive =
        st.contains('running') || st.contains('uploading') || st.contains('syncing');
    final statusLine = _syncPaused
        ? '已暂停自动备份'
        : (sync == null
            ? '空闲待命'
            : LanVaultBridge.formatSyncBlockedReason(sync.status));
    final currentFile = (sync != null && sync.filePath.isNotEmpty)
        ? SystemEnvHelper.folderDisplayName(sync.filePath)
        : null;
    final speedText = (sync != null && sync.throughputBps > 0)
        ? '${SystemEnvHelper.formatBytes(sync.throughputBps)}/s'
        : (isActive ? '测速中…' : '0 B/s');
    final processedText = sync == null
        ? '0 B / 0 B'
        : '${SystemEnvHelper.formatBytes(sync.bytesProcessed)} / ${SystemEnvHelper.formatBytes(sync.totalBytes)}';
    final ratio = (sync != null && sync.totalBytes > 0)
        ? (sync.bytesProcessed / sync.totalBytes).clamp(0.0, 1.0)
        : 0.0;

    return NeuCard(
      padding: const EdgeInsets.all(18),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Row(
            children: <Widget>[
              NeuIconBadge(
                icon: isActive
                    ? Icons.cloud_upload_rounded
                    : Icons.cloud_done_outlined,
                color: _syncPaused ? NeuTokens.warning : NeuTokens.accent,
                backgroundColor: _syncPaused
                    ? NeuTokens.folderGoldSoft
                    : NeuTokens.accentSoft,
                size: 40,
                iconSize: 20,
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: <Widget>[
                    Text(
                      currentFile != null && isActive
                          ? '正在备份：$currentFile'
                          : statusLine,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: Theme.of(context).textTheme.titleSmall?.copyWith(
                            fontWeight: FontWeight.w700,
                          ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      '$processedText · $speedText',
                      style: Theme.of(context).textTheme.bodySmall?.copyWith(
                            color: NeuTokens.textSecondary,
                          ),
                    ),
                  ],
                ),
              ),
              IconButton(
                tooltip: _syncPaused ? '恢复自动备份' : '暂停全部备份',
                icon: Icon(
                  _syncPaused ? Icons.play_arrow_rounded : Icons.pause_rounded,
                  color: _syncPaused ? NeuTokens.warning : NeuTokens.accent,
                ),
                onPressed: _togglingPause ? null : _toggleGlobalPause,
              ),
            ],
          ),
          const SizedBox(height: 12),
          NeuProgressBar(value: ratio, height: 6, showPercentage: true),
          const SizedBox(height: 14),
          const Divider(height: 1),
          const SizedBox(height: 12),
          ValueListenableBuilder<bool>(
            valueListenable: widget.bridge.fastBackupModeNotifier,
            builder: (context, fastMode, _) {
              return Row(
                children: <Widget>[
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: <Widget>[
                        Text(
                          '备份模式',
                          style: Theme.of(context)
                              .textTheme
                              .labelLarge
                              ?.copyWith(fontWeight: FontWeight.w600),
                        ),
                        const SizedBox(height: 2),
                        Text(
                          fastMode
                              ? '快速模式：大块高速直传，适合照片视频首次备份'
                              : '增量模式：仅传变动分块，省流量且支持多版本',
                          style: Theme.of(context).textTheme.bodySmall,
                        ),
                      ],
                    ),
                  ),
                  const SizedBox(width: 12),
                  SizedBox(
                    width: 132,
                    child: NeuSegmentedControl<bool>(
                      semanticLabel: '备份模式选择',
                      value: fastMode,
                      onChanged: (v) async {
                        try {
                          await widget.bridge.setFastBackupMode(v);
                          if (!context.mounted) return;
                          NeuToast.info(
                            context,
                            v ? '已切换为快速备份模式（4 MiB 直传）' : '已切换为增量备份模式（分块去重）',
                          );
                        } catch (err) {
                          if (!context.mounted) return;
                          NeuToast.error(
                            context,
                            LanVaultBridge.formatUserError(err),
                          );
                        }
                      },
                      segments: const <NeuSegment<bool>>[
                        NeuSegment<bool>(value: false, label: '增量'),
                        NeuSegment<bool>(value: true, label: '快速'),
                      ],
                    ),
                  ),
                ],
              );
            },
          ),
        ],
      ),
    );
  }

  Widget _buildTaskListCard(BuildContext context, bool isWide) {
    return NeuCard(
      padding: const EdgeInsets.all(18),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Row(
            children: <Widget>[
              Text(
                '备份任务 (${_rules.length})',
                style: Theme.of(context).textTheme.titleMedium?.copyWith(
                      fontWeight: FontWeight.w700,
                    ),
              ),
              const Spacer(),
              NeuButton(
                label: '新建',
                icon: Icons.add_rounded,
                tone: NeuButtonTone.primary,
                onPressed: _openCreateTaskDialog,
              ),
              const SizedBox(width: 6),
              IconButton(
                tooltip: '刷新任务',
                icon: const Icon(Icons.refresh_rounded, size: 20),
                onPressed: () => _refreshAll(silent: false),
              ),
            ],
          ),
          const SizedBox(height: 14),
          if (_loading && _rules.isEmpty)
            const Padding(
              padding: EdgeInsets.symmetric(vertical: 28),
              child: Center(child: CircularProgressIndicator()),
            )
          else if (_rules.isEmpty)
            const NeuEmptyState(
              icon: Icons.folder_open_rounded,
              title: '还没有备份任务',
              subtitle: '点击右上角“新建”选择需要自动保护的本地文件夹。',
            )
          else
            for (var i = 0; i < _rules.length; i++) ...<Widget>[
              _buildRuleRow(context, _rules[i], isWide),
              if (i < _rules.length - 1)
                const Padding(
                  padding: EdgeInsets.symmetric(vertical: 10),
                  child: Divider(height: 1),
                ),
            ],
        ],
      ),
    );
  }

  Widget _buildRuleRow(BuildContext context, SyncRuleModel rule, bool isWide) {
    final folderName = SystemEnvHelper.folderDisplayName(rule.localDir);
    return Row(
      children: <Widget>[
        const NeuIconBadge(
          icon: Icons.folder_rounded,
          color: NeuTokens.folderGold,
          backgroundColor: NeuTokens.folderGoldSoft,
          size: 38,
          iconSize: 20,
        ),
        const SizedBox(width: 12),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: <Widget>[
              Text(
                folderName,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: Theme.of(context).textTheme.titleSmall?.copyWith(
                      fontWeight: FontWeight.w600,
                    ),
              ),
              const SizedBox(height: 2),
              Text(
                isWide
                    ? rule.localDir
                    : '${rule.scheduleLabel} · ${rule.localDir}',
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: Theme.of(context).textTheme.bodySmall,
              ),
            ],
          ),
        ),
        if (isWide) ...<Widget>[
          const SizedBox(width: 10),
          NeuStatusBadge(label: rule.scheduleLabel, tone: NeuStatusTone.accent),
        ],
        const SizedBox(width: 8),
        Switch(
          value: rule.enabled,
          onChanged: (v) => unawaited(_toggleRule(rule, v)),
        ),
        if (isWide)
          IconButton(
            tooltip: '立即备份',
            icon: const Icon(
              Icons.play_arrow_rounded,
              size: 20,
              color: NeuTokens.accent,
            ),
            onPressed: () => unawaited(_triggerRuleScan(rule)),
          ),
        PopupMenuButton<String>(
          tooltip: '更多操作',
          icon: const Icon(Icons.more_horiz_rounded, size: 20),
          onSelected: (action) {
            switch (action) {
              case 'scan':
                unawaited(_triggerRuleScan(rule));
              case 'realtime':
                unawaited(_changeSchedule(rule, 'realtime'));
              case 'daily':
                unawaited(_changeSchedule(rule, 'daily'));
              case 'manual':
                unawaited(_changeSchedule(rule, 'manual'));
              case 'delete':
                unawaited(_deleteRule(rule));
            }
          },
          itemBuilder: (_) => const <PopupMenuEntry<String>>[
            PopupMenuItem<String>(value: 'scan', child: Text('立即备份')),
            PopupMenuItem<String>(value: 'realtime', child: Text('设为：实时增量')),
            PopupMenuItem<String>(value: 'daily', child: Text('设为：每日备份')),
            PopupMenuItem<String>(value: 'manual', child: Text('设为：仅手动')),
            PopupMenuDivider(),
            PopupMenuItem<String>(
              value: 'delete',
              child: Text(
                '删除备份任务',
                style: TextStyle(color: NeuTokens.danger),
              ),
            ),
          ],
        ),
      ],
    );
  }
}
